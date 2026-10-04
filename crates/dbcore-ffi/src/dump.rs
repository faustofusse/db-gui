//! Dumping and restoring databases (`dbcore::dump`, `dbcore::restore`).
//!
//! Progress comes back through a foreign listener; cancelling goes through a [`CancelHandle`]
//! (Swift task cancellation doesn't reach Rust futures through UniFFI).

use std::sync::Arc;

use crate::{ConnectionConfig, DbError, TableInfo};

#[derive(uniffi::Enum, Clone, Copy)]
pub enum DumpContent {
    SchemaAndData,
    SchemaOnly,
    DataOnly,
}

#[derive(uniffi::Enum, Clone)]
pub enum DumpScope {
    Database,
    Schemas { schemas: Vec<String> },
    Tables { tables: Vec<TableInfo> },
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum DumpCompression {
    None,
    Gzip,
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum DumpDataStyle {
    Copy,
    Insert,
}

#[derive(uniffi::Record, Clone)]
pub struct DumpOptions {
    pub content: DumpContent,
    pub scope: DumpScope,
    pub compression: DumpCompression,
    pub data_style: DumpDataStyle,
    pub drop_objects: bool,
    pub create_database: bool,
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum DumpPhase {
    Connecting,
    Schema,
    Data,
    PostData,
    Finishing,
}

#[derive(uniffi::Record, Clone)]
pub struct DumpProgress {
    pub phase: DumpPhase,
    pub object: Option<String>,
    pub tables_done: u32,
    pub tables_total: u32,
    pub rows_done: u64,
    pub table_rows_done: u64,
    pub table_rows_estimate: Option<u64>,
    pub bytes_written: u64,
}

#[derive(uniffi::Record)]
pub struct DumpSummary {
    pub tables: u32,
    pub rows: u64,
    pub bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(uniffi::Record, Clone, Copy)]
pub struct RestoreOptions {
    pub single_transaction: bool,
    pub stop_on_error: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct RestoreProgress {
    pub bytes_read: u64,
    pub bytes_total: u64,
    pub statements: u64,
    pub errors: u32,
}

#[derive(uniffi::Record)]
pub struct RestoreSummary {
    pub statements: u64,
    pub rows: u64,
    pub errors: Vec<String>,
    pub error_count: u32,
    pub warnings: Vec<String>,
}

/// Receives dump progress (about 10 times a second, from a background thread).
#[uniffi::export(with_foreign)]
pub trait DumpListener: Send + Sync {
    fn on_progress(&self, progress: DumpProgress);
}

/// Receives restore progress (about 10 times a second, from a background thread).
#[uniffi::export(with_foreign)]
pub trait RestoreListener: Send + Sync {
    fn on_progress(&self, progress: RestoreProgress);
}

/// Cancels a running `dump_database` / `restore_database`, which then fails with `DbError::Cancelled`.
#[derive(uniffi::Object, Default)]
pub struct CancelHandle {
    inner: dbcore::dump::CancelToken,
}

#[uniffi::export]
impl CancelHandle {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn cancel(&self) {
        self.inner.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

/// Dumps the database `config` points at into `path` (written only once complete).
#[uniffi::export]
pub async fn dump_database(
    config: ConnectionConfig,
    path: String,
    options: DumpOptions,
    listener: Arc<dyn DumpListener>,
    cancel: Arc<CancelHandle>,
) -> Result<DumpSummary, DbError> {
    let progress = Arc::new(move |p: &dbcore::dump::DumpProgress| listener.on_progress(p.clone().into()));
    let summary = dbcore::dump::dump(config.into(), path.into(), options.into(), progress, cancel.inner.clone()).await?;
    Ok(DumpSummary { tables: summary.tables, rows: summary.rows, bytes: summary.bytes, warnings: summary.warnings })
}

/// Runs the SQL script at `path` (plain or gzipped) against the database `config` points at.
#[uniffi::export]
pub async fn restore_database(
    config: ConnectionConfig,
    path: String,
    options: RestoreOptions,
    listener: Arc<dyn RestoreListener>,
    cancel: Arc<CancelHandle>,
) -> Result<RestoreSummary, DbError> {
    let progress = Arc::new(move |p: &dbcore::restore::RestoreProgress| {
        listener.on_progress(RestoreProgress {
            bytes_read: p.bytes_read,
            bytes_total: p.bytes_total,
            statements: p.statements,
            errors: p.errors,
        })
    });
    let options = dbcore::restore::RestoreOptions { single_transaction: options.single_transaction, stop_on_error: options.stop_on_error };
    let s = dbcore::restore::restore(config.into(), path.into(), options, progress, cancel.inner.clone()).await?;
    Ok(RestoreSummary { statements: s.statements, rows: s.rows, errors: s.errors, error_count: s.error_count, warnings: s.warnings })
}

/// Suggested file name: `app_dev-2025-06-01.sql` (`.sql.gz` when gzipped).
#[uniffi::export]
pub fn default_dump_file_name(database: String, date: String, compression: DumpCompression) -> String {
    dbcore::dump::default_file_name(&database, &date, compression.into())
}

impl From<DumpCompression> for dbcore::dump::Compression {
    fn from(c: DumpCompression) -> Self {
        match c {
            DumpCompression::None => Self::None,
            DumpCompression::Gzip => Self::Gzip,
        }
    }
}

impl From<DumpOptions> for dbcore::dump::DumpOptions {
    fn from(o: DumpOptions) -> Self {
        use dbcore::dump as d;
        Self {
            content: match o.content {
                DumpContent::SchemaAndData => d::DumpContent::SchemaAndData,
                DumpContent::SchemaOnly => d::DumpContent::SchemaOnly,
                DumpContent::DataOnly => d::DumpContent::DataOnly,
            },
            scope: match o.scope {
                DumpScope::Database => d::DumpScope::Database,
                DumpScope::Schemas { schemas } => d::DumpScope::Schemas(schemas),
                DumpScope::Tables { tables } => d::DumpScope::Tables(tables.into_iter().map(Into::into).collect()),
            },
            compression: o.compression.into(),
            data_style: match o.data_style {
                DumpDataStyle::Copy => d::DataStyle::Copy,
                DumpDataStyle::Insert => d::DataStyle::Insert,
            },
            drop_objects: o.drop_objects,
            create_database: o.create_database,
        }
    }
}

impl From<dbcore::dump::DumpProgress> for DumpProgress {
    fn from(p: dbcore::dump::DumpProgress) -> Self {
        use dbcore::dump::DumpPhase as P;
        Self {
            phase: match p.phase {
                P::Connecting => DumpPhase::Connecting,
                P::Schema => DumpPhase::Schema,
                P::Data => DumpPhase::Data,
                P::PostData => DumpPhase::PostData,
                P::Finishing => DumpPhase::Finishing,
            },
            object: p.object,
            tables_done: p.tables_done,
            tables_total: p.tables_total,
            rows_done: p.rows_done,
            table_rows_done: p.table_rows_done,
            table_rows_estimate: p.table_rows_estimate,
            bytes_written: p.bytes_written,
        }
    }
}
