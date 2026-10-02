use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::driver::{Driver, Error, Result};
use crate::model::{ConnectionConfig, QueryResult, Schema, TableInfo};
use crate::mock::{self, MockDriver};
use crate::model::DatabaseKind;
use crate::postgres::PostgresDriver;

/// The core owns its tokio runtime, so callers can await from any executor
/// (Swift concurrency through FFI, GPUI's executor, or tokio itself).
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("dbcore")
            .enable_all()
            .build()
            .expect("failed to start dbcore runtime")
    })
}

/// Aborts the spawned task when dropped, so cancelling the caller's future cancels the work.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|r| r.map_err(|e| Error::Internal(e.to_string())))
    }
}

async fn on_runtime<T, F>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    AbortOnDrop(runtime().spawn(future)).await?
}

/// Picks the driver for a connection.
fn make_driver(config: ConnectionConfig) -> Arc<dyn Driver> {
    if mock::is_mock(&config) {
        return Arc::new(MockDriver::new(config));
    }
    match config.kind {
        DatabaseKind::Postgres => Arc::new(PostgresDriver::new(config)),
        // No real drivers yet: keep the UI usable with sample data.
        DatabaseKind::Mysql | DatabaseKind::Sqlite => Arc::new(MockDriver::new(config)),
    }
}

/// Entry point for frontends: one per configured connection. Cheap to clone.
#[derive(Clone)]
pub struct Connection {
    driver: Arc<dyn Driver>,
}

impl Connection {
    pub fn new(config: ConnectionConfig) -> Self {
        Self { driver: make_driver(config) }
    }

    pub fn config(&self) -> &ConnectionConfig {
        self.driver.config()
    }

    pub async fn connect(&self) -> Result<()> {
        let d = self.driver.clone();
        on_runtime(async move { d.connect().await }).await
    }

    pub async fn disconnect(&self) {
        let d = self.driver.clone();
        let _ = on_runtime(async move {
            d.disconnect().await;
            Ok(())
        })
        .await;
    }

    pub async fn list_schemas(&self) -> Result<Vec<Schema>> {
        let d = self.driver.clone();
        on_runtime(async move { d.list_schemas().await }).await
    }

    pub async fn fetch_rows(&self, table: TableInfo, limit: u32, offset: u64) -> Result<QueryResult> {
        let d = self.driver.clone();
        on_runtime(async move { d.fetch_rows(&table, limit, offset).await }).await
    }

    /// Dropping the returned future also cancels the query on the server.
    pub async fn execute(&self, sql: String) -> Result<QueryResult> {
        let d = self.driver.clone();
        on_runtime(async move { d.execute(&sql).await }).await
    }

    /// Cancels the running [`Connection::execute`], which then fails with [`Error::Cancelled`].
    pub async fn cancel(&self) {
        let d = self.driver.clone();
        let _ = on_runtime(async move {
            d.cancel().await;
            Ok(())
        })
        .await;
    }
}
