//! Dump output: engines append SQL to an in-memory buffer ([`Out`]); full buffers go through a
//! bounded channel to a blocking thread that writes (and gzips) them into `<path>.partial`.
//! The bound gives backpressure, so memory stays flat however big the tables are, and file I/O
//! and compression never run on the core's async workers.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::Compression;
use crate::driver::{Error, Result};

const CHUNK: usize = 256 * 1024;
const QUEUE: usize = 8;

enum Message {
    Data(Vec<u8>),
    /// Everything was sent: finish the file and move it into place.
    Finish,
}

pub(crate) struct Out {
    buffer: Vec<u8>,
    tx: Option<mpsc::Sender<Message>>,
    bytes: u64,
}

/// `dump.sql` → `dump.sql.partial`.
pub(crate) fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".partial");
    path.with_file_name(name)
}

/// Creates `<path>.partial` and starts the writer thread. Its result is the final file size;
/// if the [`Out`] is dropped or aborted before [`Out::finish`], the partial file is removed.
pub(crate) fn start(path: &Path, compression: Compression) -> Result<(Out, JoinHandle<Result<u64>>)> {
    let partial = partial_path(path);
    let file = File::create(&partial).map_err(|e| io_error(&partial, &e))?;
    let (tx, rx) = mpsc::channel(QUEUE);
    let path = path.to_path_buf();
    let handle = tokio::task::spawn_blocking(move || {
        let result = write_all(file, compression, rx, &partial, &path);
        if result.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        result
    });
    Ok((Out { buffer: Vec::with_capacity(CHUNK + 4096), tx: Some(tx), bytes: 0 }, handle))
}

fn write_all(file: File, compression: Compression, mut rx: mpsc::Receiver<Message>, partial: &Path, path: &Path) -> Result<u64> {
    let fail = |e: std::io::Error| io_error(path, &e);
    let buffered = BufWriter::with_capacity(CHUNK, file);
    let mut sink = match compression {
        Compression::None => Sink::Plain(buffered),
        Compression::Gzip => Sink::Gzip(GzEncoder::new(buffered, flate2::Compression::default())),
    };
    loop {
        match rx.blocking_recv() {
            Some(Message::Data(chunk)) => sink.write_all(&chunk).map_err(fail)?,
            Some(Message::Finish) => break,
            // Dropped without finishing: cancelled or failed.
            None => return Err(Error::Cancelled),
        }
    }
    let buffered = match sink {
        Sink::Plain(b) => b,
        Sink::Gzip(gz) => gz.finish().map_err(fail)?,
    };
    let file = buffered.into_inner().map_err(|e| fail(e.into_error()))?;
    file.sync_all().map_err(fail)?;
    let size = file.metadata().map_err(fail)?.len();
    std::fs::rename(partial, path).map_err(fail)?;
    Ok(size)
}

enum Sink {
    Plain(BufWriter<File>),
    Gzip(GzEncoder<BufWriter<File>>),
}

impl Sink {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(w) => w.write_all(bytes),
            Self::Gzip(w) => w.write_all(bytes),
        }
    }
}

fn io_error(path: &Path, e: &std::io::Error) -> Error {
    Error::Query(format!("Couldn’t write {}: {e}", path.display()))
}

impl Out {
    pub fn push(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len() as u64;
        self.buffer.extend_from_slice(bytes);
    }

    /// Uncompressed bytes produced so far.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub async fn flush_if_full(&mut self) -> Result<()> {
        if self.buffer.len() >= CHUNK {
            self.send().await?;
        }
        Ok(())
    }

    async fn send(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.buffer, Vec::with_capacity(CHUNK + 4096));
        let tx = self.tx.as_ref().ok_or(Error::Cancelled)?;
        // The writer only hangs up after an I/O error; its join result carries the message.
        tx.send(Message::Data(chunk)).await.map_err(|_| Error::Internal("dump writer stopped".into()))
    }

    pub async fn finish(&mut self) -> Result<()> {
        self.send().await?;
        let tx = self.tx.take().ok_or(Error::Cancelled)?;
        tx.send(Message::Finish).await.map_err(|_| Error::Internal("dump writer stopped".into()))
    }

    /// Stops the writer without finishing: the partial file is removed.
    pub fn abort(&mut self) {
        self.tx = None;
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[tokio::test]
    async fn writes_and_renames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.sql");
        let (mut out, handle) = start(&path, Compression::None).unwrap();
        assert!(partial_path(&path).exists());
        for _ in 0..50_000 {
            out.push(b"insert into t values (1);\n");
            out.flush_if_full().await.unwrap();
        }
        out.finish().await.unwrap();
        let size = handle.await.unwrap().unwrap();
        assert_eq!(size, 26 * 50_000);
        assert!(!partial_path(&path).exists());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
    }

    #[tokio::test]
    async fn gzips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.sql.gz");
        let (mut out, handle) = start(&path, Compression::Gzip).unwrap();
        out.push("select 'héllo';\n".as_bytes());
        out.finish().await.unwrap();
        handle.await.unwrap().unwrap();
        let mut text = String::new();
        flate2::read::GzDecoder::new(File::open(&path).unwrap()).read_to_string(&mut text).unwrap();
        assert_eq!(text, "select 'héllo';\n");
    }

    #[tokio::test]
    async fn abort_removes_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.sql");
        let (mut out, handle) = start(&path, Compression::None).unwrap();
        out.push(&[b'x'; CHUNK * 2]);
        out.flush_if_full().await.unwrap();
        out.abort();
        assert!(matches!(handle.await.unwrap(), Err(Error::Cancelled)));
        assert!(!path.exists() && !partial_path(&path).exists());
    }
}
