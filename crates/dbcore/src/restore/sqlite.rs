//! Runs a script on a SQLite file. rusqlite is synchronous, so the whole run happens on a
//! blocking thread; cancelling interrupts the running statement.

use std::sync::{Arc, Mutex};

use rusqlite::{InterruptHandle, OpenFlags};
use tokio::sync::mpsc;

use super::split::Item;
use super::{copy_unsupported, Tally};
use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind};

fn sql_error(e: rusqlite::Error) -> Error {
    match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::OperationInterrupted) => Error::Cancelled,
        _ => Error::Query(e.to_string()),
    }
}

/// Interrupts the statement running on the blocking thread when the restore is dropped (cancel).
struct InterruptOnDrop(Arc<Mutex<Option<InterruptHandle>>>);

impl Drop for InterruptOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            handle.interrupt();
        }
    }
}

pub(super) async fn run(config: &ConnectionConfig, rx: mpsc::Receiver<Result<Item>>, tally: Tally) -> Result<Tally> {
    let path = crate::sqlite::database_path(&config.database);
    let interrupt = Arc::new(Mutex::new(None));
    let guard = InterruptOnDrop(interrupt.clone());
    let result = tokio::task::spawn_blocking(move || {
        // Never create a file: restore into an existing database (an empty file is fine).
        if !path.is_file() {
            return Err(Error::ConnectionFailed(format!("No database file at {}", path.display())));
        }
        let conn = rusqlite::Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI)
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(sql_error)?;
        *interrupt.lock().unwrap_or_else(|p| p.into_inner()) = Some(conn.get_interrupt_handle());
        execute(&conn, rx, tally)
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))?;
    // Finished: nothing left to interrupt.
    guard.0.lock().unwrap_or_else(|p| p.into_inner()).take();
    result
}

fn execute(conn: &rusqlite::Connection, mut rx: mpsc::Receiver<Result<Item>>, mut tally: Tally) -> Result<Tally> {
    if tally.single_transaction() {
        conn.execute_batch("begin").map_err(sql_error)?;
    }
    // On any early return the connection is dropped and an open transaction rolls back.
    while let Some(item) = rx.blocking_recv() {
        if tally.cancelled() {
            return Err(Error::Cancelled);
        }
        match item? {
            Item::Statement { sql, line } => {
                if tally.skips(&sql) {
                    continue;
                }
                match conn.execute_batch(&sql) {
                    Ok(()) => tally.ran(),
                    Err(e) => tally.failed(line, sql_error(e))?,
                }
            }
            Item::CopyStart { line, .. } => tally.failed(line, copy_unsupported(DatabaseKind::Sqlite))?,
            Item::CopyData(_) | Item::CopyEnd => {}
            Item::Meta { command, line } => tally.meta(&command, line)?,
        }
    }
    if tally.single_transaction() {
        conn.execute_batch("commit").map_err(sql_error)?;
    }
    Ok(tally)
}
