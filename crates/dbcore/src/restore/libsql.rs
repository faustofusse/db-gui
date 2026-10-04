//! Runs a script on Turso / libSQL over one Hrana stream (so the script's transaction, or ours,
//! spans every statement). Statements are sent in groups to save round trips: chained (each runs
//! only if the one before succeeded) when stopping at the first error, independent otherwise.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::split::Item;
use super::{copy_unsupported, Tally};
use crate::driver::{Error, Result};
use crate::dump::libsql::{execute, StreamGuard};
use crate::libsql::hrana::{Batch, Client, Stmt, Stream, StreamRequest, StreamResponse, StreamResult};
use crate::model::{ConnectionConfig, DatabaseKind};

/// Statements per request, and the most SQL text sent at once.
const GROUP: usize = 200;
const GROUP_BYTES: usize = 512 * 1024;

pub(super) async fn run(config: &ConnectionConfig, mut rx: mpsc::Receiver<Result<Item>>, mut tally: Tally) -> Result<Tally> {
    let client = Arc::new(Client::new(config)?);
    let mut guard = StreamGuard(Some(client.stream()));
    let stream = guard.stream();
    if tally.single_transaction() {
        execute(stream, [Stmt::new("begin")]).await?.pop().expect("one result")?;
    }
    let mut group: Vec<(String, usize)> = Vec::new();
    let mut bytes = 0;
    while let Some(item) = rx.recv().await {
        match item? {
            Item::Statement { sql, line } => {
                if tally.skips(&sql) {
                    continue;
                }
                bytes += sql.len();
                group.push((sql, line));
                if group.len() >= GROUP || bytes >= GROUP_BYTES {
                    send(stream, &mut group, &mut tally).await?;
                    bytes = 0;
                }
            }
            Item::CopyStart { line, .. } => {
                send(stream, &mut group, &mut tally).await?;
                bytes = 0;
                tally.failed(line, copy_unsupported(DatabaseKind::Libsql))?;
            }
            Item::CopyData(_) | Item::CopyEnd => {}
            Item::Meta { command, line } => tally.meta(&command, line)?,
        }
    }
    send(stream, &mut group, &mut tally).await?;
    if tally.single_transaction() {
        execute(stream, [Stmt::new("commit")])
            .await?
            .pop()
            .expect("one result")
            .map_err(|e| Error::Query(format!("{e}\nNothing was restored.")))?;
    }
    if let Some(stream) = guard.0.take() {
        stream.close().await;
    }
    Ok(tally)
}

async fn send(stream: &mut Stream, group: &mut Vec<(String, usize)>, tally: &mut Tally) -> Result<()> {
    if group.is_empty() {
        return Ok(());
    }
    let statements: Vec<(String, usize)> = std::mem::take(group);
    if tally.stops_on_error() {
        let batch = Batch::chained(statements.iter().map(|(sql, _)| Stmt { want_rows: false, ..Stmt::new(sql.as_str()) }));
        let mut results = stream.pipeline(&[StreamRequest::Batch { batch }]).await?;
        let result = match results.pop() {
            Some(StreamResult::Ok { response: StreamResponse::Batch { result } }) => result,
            Some(StreamResult::Error { error }) => return Err(error.into_error()),
            _ => return Err(Error::Internal("unexpected response type".into())),
        };
        for (i, (_, line)) in statements.iter().enumerate() {
            if let Some(Some(error)) = result.step_errors.get(i) {
                return tally.failed(*line, error.clone().into_error());
            }
            tally.ran();
        }
    } else {
        let results = execute(stream, statements.iter().map(|(sql, _)| Stmt { want_rows: false, ..Stmt::new(sql.as_str()) })).await?;
        for (result, (_, line)) in results.into_iter().zip(&statements) {
            match result {
                Ok(_) => tally.ran(),
                Err(e) => tally.failed(*line, e)?,
            }
        }
    }
    Ok(())
}
