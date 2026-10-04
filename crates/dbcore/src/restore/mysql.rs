//! Runs a script on MySQL, one statement at a time on a dedicated connection.

use mysql_async::prelude::Queryable;
use tokio::sync::mpsc;

use super::split::Item;
use super::{copy_unsupported, Tally};
use crate::driver::Result;
use crate::model::{ConnectionConfig, DatabaseKind};
use crate::mysql::query_error;

pub(super) async fn run(config: &ConnectionConfig, mut rx: mpsc::Receiver<Result<Item>>, mut tally: Tally) -> Result<Tally> {
    let mut conn = crate::mysql::connect(config).await?;
    let fail = |e: mysql_async::Error| query_error(&e);
    if tally.single_transaction() {
        conn.query_drop("start transaction").await.map_err(fail)?;
    }
    while let Some(item) = rx.recv().await {
        match item? {
            Item::Statement { sql, line } => {
                if tally.skips(&sql) {
                    continue;
                }
                match conn.query_drop(&sql).await {
                    Ok(()) => tally.ran(),
                    Err(e) => tally.failed(line, fail(e))?,
                }
            }
            Item::CopyStart { line, .. } => tally.failed(line, copy_unsupported(DatabaseKind::Mysql))?,
            Item::CopyData(_) | Item::CopyEnd => {}
            Item::Meta { command, line } => tally.meta(&command, line)?,
        }
    }
    if tally.single_transaction() {
        conn.query_drop("commit").await.map_err(fail)?;
    }
    let _ = conn.disconnect().await;
    Ok(tally)
}
