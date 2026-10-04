//! Runs a script on Postgres: statements with the simple query protocol, `COPY … FROM stdin`
//! blocks through the COPY protocol.

use std::pin::Pin;

use bytes::Bytes;
use futures_util::SinkExt;
use tokio::sync::mpsc;
use tokio_postgres::CopyInSink;

use super::split::Item;
use super::Tally;
use crate::driver::Result;
use crate::model::ConnectionConfig;
use crate::postgres::query_error;

pub(super) async fn run(config: &ConnectionConfig, mut rx: mpsc::Receiver<Result<Item>>, mut tally: Tally) -> Result<Tally> {
    // Dropping this client (cancel, error) ends the session: an open transaction rolls back.
    let client = crate::postgres::connect(config).await?;
    let fail = |e: tokio_postgres::Error| query_error(&e, None);
    if tally.single_transaction() {
        client.batch_execute("begin").await.map_err(fail)?;
    }
    let mut copy: Option<Pin<Box<CopyInSink<Bytes>>>> = None;
    let mut copy_line = 0;
    while let Some(item) = rx.recv().await {
        match item? {
            Item::Statement { sql, line } => {
                if tally.skips(&sql) {
                    continue;
                }
                match client.batch_execute(&sql).await {
                    Ok(()) => tally.ran(),
                    Err(e) => tally.failed(line, fail(e))?,
                }
            }
            Item::CopyStart { sql, line } => {
                copy_line = line;
                match client.copy_in::<_, Bytes>(&sql).await {
                    Ok(sink) => copy = Some(Box::pin(sink)),
                    Err(e) => tally.failed(line, fail(e))?,
                }
            }
            Item::CopyData(data) => {
                if let Some(sink) = copy.as_mut() {
                    if let Err(e) = sink.send(Bytes::from(data)).await {
                        // Dropping the sink aborts the COPY; its remaining data is skipped.
                        copy = None;
                        tally.failed(copy_line, fail(e))?;
                    }
                }
            }
            Item::CopyEnd => {
                if let Some(mut sink) = copy.take() {
                    match sink.as_mut().finish().await {
                        Ok(rows) => {
                            tally.rows(rows);
                            tally.ran();
                        }
                        Err(e) => tally.failed(copy_line, fail(e))?,
                    }
                }
            }
            Item::Meta { command, line } => tally.meta(&command, line)?,
        }
    }
    if tally.single_transaction() {
        client.batch_execute("commit").await.map_err(|e| {
            crate::driver::Error::Query(format!("{}\nNothing was restored.", fail(e)))
        })?;
    }
    Ok(tally)
}
