//! Runs a T-SQL script on SQL Server: each `GO` batch is sent as one SQL batch on a dedicated
//! session, so `SET` options (`IDENTITY_INSERT`, `DATEFORMAT`…) carry over, as in sqlcmd.

use tokio::sync::mpsc;

use super::split::Item;
use super::{copy_unsupported, Tally};
use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind};
use crate::sqlserver::{connect, query_error, Client};

async fn batch(client: &mut Client, sql: &str, first_line: usize) -> Result<()> {
    let result = async { client.simple_query(sql).await?.into_results().await }.await;
    result.map(drop).map_err(|e| query_error(&e, first_line as u32))
}

pub(super) async fn run(config: &ConnectionConfig, mut rx: mpsc::Receiver<Result<Item>>, mut tally: Tally) -> Result<Tally> {
    // Dropping the client (cancel, error) ends the session: an open transaction rolls back.
    let mut client = connect(config, "set nocount on").await?;
    if tally.single_transaction() {
        // An error inside the transaction rolls it all back, whatever the statement.
        batch(&mut client, "set xact_abort on; begin transaction", 1).await?;
    }
    while let Some(item) = rx.recv().await {
        match item? {
            Item::Statement { sql, line } => {
                if tally.skips(&sql) {
                    continue;
                }
                match batch(&mut client, &sql, line).await {
                    Ok(()) => tally.ran(),
                    Err(e) => tally.failed(line, e)?,
                }
            }
            Item::CopyStart { line, .. } => tally.failed(line, copy_unsupported(DatabaseKind::SqlServer))?,
            Item::CopyData(_) | Item::CopyEnd => {}
            Item::Meta { command, line } => tally.meta(&command, line)?,
        }
    }
    if tally.single_transaction() {
        batch(&mut client, "commit", 1).await.map_err(|e| Error::Query(format!("{e}\nNothing was restored.")))?;
    }
    let _ = client.close().await;
    Ok(tally)
}
