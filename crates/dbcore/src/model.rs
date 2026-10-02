//! Plain data types shared by every frontend.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    Postgres,
    Mysql,
    Sqlite,
}

impl DatabaseKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Mysql => "MySQL",
            Self::Sqlite => "SQLite",
        }
    }

    /// The server's standard port (`None` for file databases).
    pub fn default_port(self) -> Option<u16> {
        match self {
            Self::Postgres => Some(5432),
            Self::Mysql => Some(3306),
            Self::Sqlite => None,
        }
    }
}

/// TLS behaviour, named after libpq's `sslmode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    /// Plain TCP.
    Disable,
    /// TLS if the server supports it, without verifying its certificate.
    #[default]
    Prefer,
    /// TLS required, certificate not verified (same as libpq).
    Require,
    /// TLS required, certificate chain and hostname verified.
    VerifyFull,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub group: String,
    pub kind: DatabaseKind,
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    pub user: Option<String>,
    /// Supplied by the frontend from the platform keychain; never persisted by the core.
    pub password: Option<String>,
    pub ssl_mode: SslMode,
    /// List every database on the server, not just `database`: in the sidebar for Postgres,
    /// as schemas for MySQL. `database` stays the one used to connect first and the default selection.
    pub show_all_databases: bool,
}

impl std::fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("group", &self.group)
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "•••"))
            .field("ssl_mode", &self.ssl_mode)
            .field("show_all_databases", &self.show_all_databases)
            .finish()
    }
}

impl ConnectionConfig {
    /// Whether sibling databases are listed under the connection in the sidebar, each opened
    /// with its own session (Postgres). MySQL lists them as schemas instead; SQLite has none.
    pub fn supports_multiple_databases(&self) -> bool {
        self.kind == DatabaseKind::Postgres
    }

    /// The same connection pointed at another database on the server.
    pub fn with_database(&self, database: &str) -> Self {
        Self { database: database.into(), ..self.clone() }
    }

    /// The database actually opened. `database` is optional for servers: Postgres then uses its
    /// `postgres` maintenance database (present on virtually every server), MySQL needs none.
    pub fn default_database(&self) -> &str {
        let configured = self.database.trim();
        match self.kind {
            DatabaseKind::Postgres if configured.is_empty() => "postgres",
            _ => configured,
        }
    }

    /// Name used when the user leaves it empty: the database (file name for SQLite), else the host.
    pub fn default_name(&self) -> String {
        let database = self.database.trim();
        let host = self.host.trim();
        match self.kind {
            DatabaseKind::Sqlite => database.rsplit('/').next().unwrap_or_default().to_string(),
            _ if !database.is_empty() => database.to_string(),
            _ => host.to_string(),
        }
    }

    /// e.g. "PostgreSQL · localhost:5432/app_dev", or "PostgreSQL · localhost:5432" without a database.
    pub fn summary(&self) -> String {
        let kind = self.kind.display_name();
        if self.kind == DatabaseKind::Sqlite {
            return format!("{kind} · {}", self.database);
        }
        let address = match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        };
        if self.database.trim().is_empty() {
            format!("{kind} · {address}")
        } else {
            format!("{kind} · {address}/{}", self.database)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableKind {
    Table,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    pub estimated_row_count: Option<u64>,
}

impl TableInfo {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self { schema: schema.into(), name: name.into(), kind: TableKind::Table, estimated_row_count: None }
    }

    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<TableInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub is_primary_key: bool,
    pub is_nullable: bool,
}

/// A single cell. Drivers decode wire types into one of these.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Exact numerics (NUMERIC/DECIMAL) kept as text to avoid precision loss.
    Decimal(String),
    Text(String),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Canonical display string. Frontends may format differently, but this is the reference.
    pub fn display(&self) -> String {
        match self {
            Self::Null => "NULL".into(),
            Self::Bool(b) => b.to_string(),
            Self::Int(i) => i.to_string(),
            Self::Float(f) => f.to_string(),
            Self::Decimal(s) | Self::Text(s) => s.clone(),
        }
    }
}

/// One page of rows. Rows are sent in pages, never cell by cell.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Value>>,
    /// Total rows available (e.g. table size), when known. For a truncated script result,
    /// the number of rows the statement actually returned.
    pub total_count: Option<u64>,
    /// `rows` stops at the requested row limit; the statement returned more.
    pub truncated: bool,
    /// For statements that return no rows (INSERT/UPDATE/DDL…): rows affected, as reported by the server.
    pub rows_affected: Option<u64>,
}
