use async_trait::async_trait;

use crate::model::{ConnectionConfig, QueryResult, Schema, TableInfo};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Table not found: {0}")]
    TableNotFound(String),
    #[error("Unsupported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Query(String),
    #[error("Query cancelled")]
    Cancelled,
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Implemented by every database backend (Postgres, MySQL, SQLite, mock).
///
/// Drivers run on the core's own tokio runtime; frontends never call them directly
/// but go through [`crate::Connection`], which is executor-agnostic.
#[async_trait]
pub trait Driver: Send + Sync + 'static {
    fn config(&self) -> &ConnectionConfig;
    async fn connect(&self) -> Result<()>;
    /// Closes server connections (cancelling a running `execute`). The next call reconnects.
    async fn disconnect(&self);
    /// Whether a server connection is currently open (and not dropped by the server).
    async fn is_connected(&self) -> bool;
    async fn list_schemas(&self) -> Result<Vec<Schema>>;
    async fn fetch_rows(&self, table: &TableInfo, limit: u32, offset: u64) -> Result<QueryResult>;
    async fn execute(&self, sql: &str) -> Result<QueryResult>;
    /// Cancels the running `execute`, if any. It then fails with [`Error::Cancelled`].
    async fn cancel(&self) {}
}
