//! UniFFI surface of `dbcore` for the SwiftUI app.
//!
//! Types are mirrored here (instead of deriving UniFFI traits in `dbcore`) so the core
//! stays free of FFI concerns; the GPUI app links `dbcore` directly and never sees this crate.
//! Keep this surface small: a few objects plus plain records, with rows sent in pages.

use std::sync::{Arc, Mutex};

uniffi::setup_scaffolding!();

// MARK: Records & enums

#[derive(uniffi::Enum, Clone, Copy)]
pub enum DatabaseKind {
    Postgres,
    Mysql,
    Sqlite,
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyFull,
}

#[derive(uniffi::Record, Clone)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub group: String,
    pub kind: DatabaseKind,
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    pub user: Option<String>,
    pub password: Option<String>,
    pub ssl_mode: SslMode,
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum TableKind {
    Table,
    View,
}

#[derive(uniffi::Record, Clone)]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    pub estimated_row_count: Option<u64>,
}

#[derive(uniffi::Record)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<TableInfo>,
}

#[derive(uniffi::Record)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub is_primary_key: bool,
    pub is_nullable: bool,
}

#[derive(uniffi::Enum)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(String),
    Text(String),
}

#[derive(uniffi::Record)]
pub struct QueryResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Value>>,
    pub total_count: Option<u64>,
    pub rows_affected: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum DbError {
    #[error("Connection failed: {message}")]
    ConnectionFailed { message: String },
    #[error("Table not found: {name}")]
    TableNotFound { name: String },
    #[error("Unsupported: {message}")]
    Unsupported { message: String },
    #[error("{message}")]
    Query { message: String },
    #[error("Query cancelled")]
    Cancelled,
    #[error("{message}")]
    InvalidConfig { message: String },
    #[error("Couldn’t save connections: {message}")]
    Storage { message: String },
    #[error("Internal error: {message}")]
    Internal { message: String },
}

// MARK: Objects & functions

#[derive(uniffi::Object)]
pub struct Connection {
    inner: dbcore::Connection,
}

#[uniffi::export]
impl Connection {
    #[uniffi::constructor]
    pub fn new(config: ConnectionConfig) -> Arc<Self> {
        Arc::new(Self { inner: dbcore::Connection::new(config.into()) })
    }

    pub fn config(&self) -> ConnectionConfig {
        self.inner.config().clone().into()
    }

    pub async fn connect(&self) -> Result<(), DbError> {
        Ok(self.inner.connect().await?)
    }

    pub async fn disconnect(&self) {
        self.inner.disconnect().await
    }

    pub async fn is_connected(&self) -> bool {
        self.inner.is_connected().await
    }

    pub async fn list_schemas(&self) -> Result<Vec<Schema>, DbError> {
        Ok(self.inner.list_schemas().await?.into_iter().map(Into::into).collect())
    }

    pub async fn fetch_rows(&self, table: TableInfo, limit: u32, offset: u64) -> Result<QueryResult, DbError> {
        Ok(self.inner.fetch_rows(table.into(), limit, offset).await?.into())
    }

    /// Runs a script, keeping at most `max_rows` rows (`None` = all).
    pub async fn execute(&self, sql: String, max_rows: Option<u32>) -> Result<QueryResult, DbError> {
        Ok(self.inner.execute_limited(sql, max_rows).await?.into())
    }

    /// Cancels the running `execute`, which then fails with `DbError::Cancelled`.
    /// (Swift task cancellation doesn't reach Rust futures through UniFFI, so call this.)
    pub async fn cancel(&self) {
        self.inner.cancel().await
    }
}

/// Saved connections (JSON file, no passwords). Passwords live in the Keychain, owned by the app.
#[derive(uniffi::Object)]
pub struct ConnectionStore {
    inner: Mutex<dbcore::ConnectionStore>,
}

#[uniffi::export]
impl ConnectionStore {
    /// Opens the store at `path` (a missing file is an empty store).
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, DbError> {
        Ok(Arc::new(Self { inner: Mutex::new(dbcore::ConnectionStore::open(path)?) }))
    }

    /// Opens the store at the platform default location (migrating the old DBGui folder).
    #[uniffi::constructor]
    pub fn open_default() -> Result<Arc<Self>, DbError> {
        Ok(Arc::new(Self { inner: Mutex::new(dbcore::ConnectionStore::open_default()?) }))
    }

    pub fn path(&self) -> String {
        self.lock().path().display().to_string()
    }

    pub fn connections(&self) -> Vec<ConnectionConfig> {
        self.lock().connections().iter().cloned().map(Into::into).collect()
    }

    /// Adds or replaces (by id) and saves. An empty id gets a new one. Returns the stored config.
    pub fn upsert(&self, config: ConnectionConfig) -> Result<ConnectionConfig, DbError> {
        Ok(self.lock().upsert(config.into())?.into())
    }

    pub fn remove(&self, id: String) -> Result<bool, DbError> {
        Ok(self.lock().remove(&id)?)
    }
}

impl ConnectionStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, dbcore::ConnectionStore> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Blank config for the "Add Connection" form.
#[uniffi::export]
pub fn new_connection_config(kind: DatabaseKind) -> ConnectionConfig {
    dbcore::ConnectionConfig::new_empty(kind.into()).into()
}

/// First problem with the config, or `None` if it can be saved.
#[uniffi::export]
pub fn validate_connection(config: ConnectionConfig) -> Option<String> {
    dbcore::ConnectionConfig::from(config).validate().err().map(|e| e.to_string())
}

/// Parses `postgres://user:pass@host:port/db?sslmode=…` (and mysql/sqlite URLs).
#[uniffi::export]
pub fn parse_connection_url(url: String) -> Result<ConnectionConfig, DbError> {
    Ok(dbcore::ConnectionConfig::from_url(&url)?.into())
}

#[uniffi::export]
pub fn connection_url(config: ConnectionConfig, include_password: bool) -> String {
    dbcore::ConnectionConfig::from(config).to_url(include_password)
}

#[uniffi::export]
pub fn default_port(kind: DatabaseKind) -> Option<u16> {
    dbcore::DatabaseKind::from(kind).default_port()
}

/// Sample connections (mock data + the dev database) for development.
#[uniffi::export]
pub fn sample_connections() -> Vec<ConnectionConfig> {
    dbcore::mock::connections().into_iter().map(Into::into).collect()
}

/// e.g. "PostgreSQL · localhost:5432/app_dev"
#[uniffi::export]
pub fn connection_summary(config: ConnectionConfig) -> String {
    dbcore::ConnectionConfig::from(config).summary()
}

#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

// MARK: Conversions

impl From<DatabaseKind> for dbcore::DatabaseKind {
    fn from(k: DatabaseKind) -> Self {
        match k {
            DatabaseKind::Postgres => Self::Postgres,
            DatabaseKind::Mysql => Self::Mysql,
            DatabaseKind::Sqlite => Self::Sqlite,
        }
    }
}

impl From<dbcore::DatabaseKind> for DatabaseKind {
    fn from(k: dbcore::DatabaseKind) -> Self {
        match k {
            dbcore::DatabaseKind::Postgres => Self::Postgres,
            dbcore::DatabaseKind::Mysql => Self::Mysql,
            dbcore::DatabaseKind::Sqlite => Self::Sqlite,
        }
    }
}

impl From<ConnectionConfig> for dbcore::ConnectionConfig {
    fn from(c: ConnectionConfig) -> Self {
        Self {
            id: c.id,
            name: c.name,
            group: c.group,
            kind: c.kind.into(),
            host: c.host,
            port: c.port,
            database: c.database,
            user: c.user,
            password: c.password,
            ssl_mode: c.ssl_mode.into(),
        }
    }
}

impl From<dbcore::ConnectionConfig> for ConnectionConfig {
    fn from(c: dbcore::ConnectionConfig) -> Self {
        Self {
            id: c.id,
            name: c.name,
            group: c.group,
            kind: c.kind.into(),
            host: c.host,
            port: c.port,
            database: c.database,
            user: c.user,
            password: c.password,
            ssl_mode: c.ssl_mode.into(),
        }
    }
}

impl From<SslMode> for dbcore::SslMode {
    fn from(m: SslMode) -> Self {
        match m {
            SslMode::Disable => Self::Disable,
            SslMode::Prefer => Self::Prefer,
            SslMode::Require => Self::Require,
            SslMode::VerifyFull => Self::VerifyFull,
        }
    }
}

impl From<dbcore::SslMode> for SslMode {
    fn from(m: dbcore::SslMode) -> Self {
        match m {
            dbcore::SslMode::Disable => Self::Disable,
            dbcore::SslMode::Prefer => Self::Prefer,
            dbcore::SslMode::Require => Self::Require,
            dbcore::SslMode::VerifyFull => Self::VerifyFull,
        }
    }
}

impl From<TableKind> for dbcore::TableKind {
    fn from(k: TableKind) -> Self {
        match k {
            TableKind::Table => Self::Table,
            TableKind::View => Self::View,
        }
    }
}

impl From<dbcore::TableKind> for TableKind {
    fn from(k: dbcore::TableKind) -> Self {
        match k {
            dbcore::TableKind::Table => Self::Table,
            dbcore::TableKind::View => Self::View,
        }
    }
}

impl From<TableInfo> for dbcore::TableInfo {
    fn from(t: TableInfo) -> Self {
        Self { schema: t.schema, name: t.name, kind: t.kind.into(), estimated_row_count: t.estimated_row_count }
    }
}

impl From<dbcore::TableInfo> for TableInfo {
    fn from(t: dbcore::TableInfo) -> Self {
        Self { schema: t.schema, name: t.name, kind: t.kind.into(), estimated_row_count: t.estimated_row_count }
    }
}

impl From<dbcore::Schema> for Schema {
    fn from(s: dbcore::Schema) -> Self {
        Self { name: s.name, tables: s.tables.into_iter().map(Into::into).collect() }
    }
}

impl From<dbcore::ColumnInfo> for ColumnInfo {
    fn from(c: dbcore::ColumnInfo) -> Self {
        Self { name: c.name, type_name: c.type_name, is_primary_key: c.is_primary_key, is_nullable: c.is_nullable }
    }
}

impl From<dbcore::Value> for Value {
    fn from(v: dbcore::Value) -> Self {
        match v {
            dbcore::Value::Null => Self::Null,
            dbcore::Value::Bool(b) => Self::Bool(b),
            dbcore::Value::Int(i) => Self::Int(i),
            dbcore::Value::Float(f) => Self::Float(f),
            dbcore::Value::Decimal(s) => Self::Decimal(s),
            dbcore::Value::Text(s) => Self::Text(s),
        }
    }
}

impl From<dbcore::QueryResult> for QueryResult {
    fn from(r: dbcore::QueryResult) -> Self {
        Self {
            columns: r.columns.into_iter().map(Into::into).collect(),
            rows: r.rows.into_iter().map(|row| row.into_iter().map(Into::into).collect()).collect(),
            total_count: r.total_count,
            rows_affected: r.rows_affected,
            truncated: r.truncated,
        }
    }
}

impl From<dbcore::Error> for DbError {
    fn from(e: dbcore::Error) -> Self {
        use dbcore::Error as E;
        match e {
            E::ConnectionFailed(message) => Self::ConnectionFailed { message },
            E::TableNotFound(name) => Self::TableNotFound { name },
            E::Unsupported(message) => Self::Unsupported { message },
            E::Query(message) => Self::Query { message },
            E::Cancelled => Self::Cancelled,
            E::InvalidConfig(message) => Self::InvalidConfig { message },
            E::Storage(message) => Self::Storage { message },
            E::Internal(message) => Self::Internal { message },
        }
    }
}
