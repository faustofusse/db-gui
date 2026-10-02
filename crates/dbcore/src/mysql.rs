//! MySQL / MariaDB driver (mysql_async + rustls).
//!
//! In MySQL a database *is* a schema, so a connection lists every database it can see as a
//! schema section (or only the configured one when "show all databases" is off). Tables are
//! always addressed as `` `db`.`table` ``, so one session serves them all.
//!
//! Values come over the text protocol (`COM_QUERY`), exactly as the mysql client prints them;
//! column metadata turns them into typed values (ints, floats, exact decimals, booleans).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::prelude::*;
use mysql_async::{Column, Conn, Opts, OptsBuilder, SslOpts};
use tokio::sync::{Mutex, MutexGuard};

use crate::dialect::{error_chain, hex_preview, Dialect};
use crate::driver::{Driver, Error, Result};
use crate::model::*;

const MYSQL: Dialect = Dialect(DatabaseKind::Mysql);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Idle sessions are pinged before reuse: the server drops them after `wait_timeout`.
const PING_AFTER_IDLE: Duration = Duration::from_secs(60);
/// Tables smaller than this (by the server's estimate) get an exact `count(*)`.
const EXACT_COUNT_THRESHOLD: u64 = 100_000;
const SYSTEM_SCHEMAS: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];
/// Character set number of binary strings (BLOB, VARBINARY…).
const BINARY_CHARSET: u16 = 63;
const ER_QUERY_INTERRUPTED: u16 = 1317;

pub struct MysqlDriver {
    config: ConnectionConfig,
    /// Catalog and table browsing.
    browse: Session,
    /// User scripts: a long query doesn't block browsing, and `cancel` only hits scripts.
    query: Session,
    /// Set by `cancel`, so an interrupted script reports "cancelled" even when the server
    /// just ends it early (e.g. `sleep()` returns 1 instead of failing).
    cancelled: AtomicBool,
}

#[derive(Default)]
struct Session {
    conn: Mutex<Option<(Conn, Instant)>>,
    /// Server thread id of the open connection (0 = none), readable while a query holds `conn`.
    thread_id: AtomicU32,
}

/// A locked, open session connection.
struct Lease<'a> {
    guard: MutexGuard<'a, Option<(Conn, Instant)>>,
    session: &'a Session,
}

impl Lease<'_> {
    fn conn(&mut self) -> &mut Conn {
        &mut self.guard.as_mut().expect("leased sessions are open").0
    }

    /// Forgets a connection that hit a network error, so the next call reconnects.
    fn check<T>(&mut self, result: std::result::Result<T, mysql_async::Error>) -> std::result::Result<T, mysql_async::Error> {
        if let Err(mysql_async::Error::Io(_) | mysql_async::Error::Driver(_)) = &result {
            self.guard.take();
            self.session.thread_id.store(0, Ordering::Relaxed);
        }
        result
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some((_, used)) = self.guard.as_mut() {
            *used = Instant::now();
        }
    }
}

impl Session {
    async fn lease(&self, config: &ConnectionConfig) -> Result<Lease<'_>> {
        let mut guard = self.conn.lock().await;
        if let Some((conn, used)) = guard.as_mut() {
            if used.elapsed() > PING_AFTER_IDLE && conn.ping().await.is_err() {
                guard.take();
            }
        }
        if guard.is_none() {
            let conn = connect(config).await?;
            self.thread_id.store(conn.id(), Ordering::Relaxed);
            *guard = Some((conn, Instant::now()));
        }
        Ok(Lease { guard, session: self })
    }

    fn is_open(&self) -> bool {
        self.thread_id.load(Ordering::Relaxed) != 0
    }

    async fn close(&self) {
        self.thread_id.store(0, Ordering::Relaxed);
        if let Some((conn, _)) = self.conn.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), conn.disconnect()).await;
        }
    }
}

impl MysqlDriver {
    pub fn new(config: ConnectionConfig) -> Self {
        Self { config, browse: Session::default(), query: Session::default(), cancelled: AtomicBool::new(false) }
    }

    /// Databases shown as schemas: all visible ones, or only the configured one.
    fn only_database(&self) -> Option<&str> {
        let database = self.config.database.trim();
        (!self.config.show_all_databases && !database.is_empty()).then_some(database)
    }
}

fn opts(config: &ConnectionConfig, tls: Option<SslOpts>) -> Opts {
    let database = config.database.trim();
    OptsBuilder::default()
        .ip_or_hostname(config.host.trim())
        .tcp_port(config.port.unwrap_or(3306))
        .user(Some(config.user.as_deref().unwrap_or("root")))
        .pass(config.password.clone())
        .db_name((!database.is_empty()).then(|| database.to_string()))
        // `localhost` would otherwise switch to the server's unix socket, which may not be ours (containers).
        .prefer_socket(false)
        .ssl_opts(tls)
        .into()
}

async fn connect(config: &ConnectionConfig) -> Result<Conn> {
    let attempt = |tls: Option<SslOpts>| async move {
        match tokio::time::timeout(CONNECT_TIMEOUT, Conn::new(opts(config, tls))).await {
            Ok(result) => result,
            Err(_) => Err(mysql_async::Error::Other("timed out".into())),
        }
    };
    let insecure = || SslOpts::default().with_danger_accept_invalid_certs(true).with_danger_skip_domain_validation(true);
    let result = match config.ssl_mode {
        SslMode::Disable => attempt(None).await,
        SslMode::Require => attempt(Some(insecure())).await,
        SslMode::VerifyFull => attempt(Some(SslOpts::default())).await,
        // Like libpq's "prefer": TLS when the server offers it, plain otherwise.
        SslMode::Prefer => match attempt(Some(insecure())).await {
            Err(e) if !matches!(e, mysql_async::Error::Server(_)) => attempt(None).await.map_err(|_| e),
            other => other,
        },
    };
    result.map_err(|e| Error::ConnectionFailed(connect_error(&e)))
}

#[async_trait]
impl Driver for MysqlDriver {
    fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    async fn connect(&self) -> Result<()> {
        self.browse.lease(&self.config).await.map(drop)
    }

    async fn disconnect(&self) {
        self.cancel().await;
        self.browse.close().await;
        self.query.close().await;
    }

    async fn is_connected(&self) -> bool {
        self.browse.is_open() || self.query.is_open()
    }

    async fn list_databases(&self) -> Result<Vec<String>> {
        Ok(self.list_schemas().await?.into_iter().map(|s| s.name).collect())
    }

    async fn list_schemas(&self) -> Result<Vec<Schema>> {
        let filter = match self.only_database() {
            Some(db) => format!("s.schema_name = {}", MYSQL.quote_literal(db)),
            None => format!(
                "s.schema_name not in ({})",
                SYSTEM_SCHEMAS.iter().map(|s| MYSQL.quote_literal(s)).collect::<Vec<_>>().join(", ")
            ),
        };
        // Empty databases are listed too (left join), so they show up as empty sections.
        let sql = format!(
            "select s.schema_name, t.table_name, t.table_type, t.table_rows
             from information_schema.schemata s
             left join information_schema.tables t on t.table_schema = s.schema_name
             where {filter}
             order by s.schema_name, t.table_name"
        );
        let mut lease = self.browse.lease(&self.config).await?;
        let result = lease.conn().query::<(String, Option<String>, Option<String>, Option<u64>), _>(sql).await;
        let rows = lease.check(result).map_err(|e| query_error(&e))?;

        let mut schemas: Vec<Schema> = Vec::new();
        for (schema, table, table_type, table_rows) in rows {
            if schemas.last().is_none_or(|s| s.name != schema) {
                schemas.push(Schema { name: schema.clone(), tables: Vec::new() });
            }
            let Some(name) = table else { continue };
            let kind = if table_type.as_deref() == Some("VIEW") { TableKind::View } else { TableKind::Table };
            schemas.last_mut().unwrap().tables.push(TableInfo {
                schema,
                name,
                kind,
                // InnoDB estimates are cached (`information_schema_stats_expiry`, 24h by default), so a
                // fresh or never-analyzed table reports 0: show no count rather than a wrong one.
                estimated_row_count: if kind == TableKind::View { None } else { table_rows.filter(|&n| n > 0) },
            });
        }
        Ok(schemas)
    }

    async fn list_columns(&self) -> Result<Vec<TableColumns>> {
        let filter = match self.only_database() {
            Some(db) => format!("table_schema = {}", MYSQL.quote_literal(db)),
            None => format!(
                "table_schema not in ({})",
                SYSTEM_SCHEMAS.iter().map(|s| MYSQL.quote_literal(s)).collect::<Vec<_>>().join(", ")
            ),
        };
        let sql = format!(
            "select table_schema, table_name, column_name, column_type, is_nullable, column_key
             from information_schema.columns
             where {filter}
             order by table_schema, table_name, ordinal_position"
        );
        let mut lease = self.browse.lease(&self.config).await?;
        let result = lease.conn().query::<(String, String, String, String, String, String), _>(sql).await;
        let rows = lease.check(result).map_err(|e| query_error(&e))?;

        let mut tables: Vec<TableColumns> = Vec::new();
        for (schema, table, column, type_name, nullable, key) in rows {
            if tables.last().is_none_or(|t| t.schema != schema || t.table != table) {
                tables.push(TableColumns { schema, table, columns: Vec::new() });
            }
            tables.last_mut().unwrap().columns.push(ColumnInfo {
                name: column,
                type_name,
                is_primary_key: key == "PRI",
                is_nullable: nullable == "YES",
            });
        }
        Ok(tables)
    }

    async fn fetch_rows(&self, table: &TableInfo, limit: u32, offset: u64) -> Result<QueryResult> {
        let mut lease = self.browse.lease(&self.config).await?;
        let meta = TableMeta::load(&mut lease, table).await?;
        let relation = MYSQL.quote_relation(&table.schema, &table.name);
        let order_by: Vec<String> = meta.primary_key.iter().map(|c| MYSQL.quote_ident(c)).collect();
        let sql = MYSQL.page_query(&relation, &order_by, limit, offset);

        let result = run_script(lease.conn(), &sql, None).await;
        let mut result = lease.check(result).map_err(|e| query_error(&e))?;
        result.columns = meta.columns;

        result.total_count = match meta.estimated_rows {
            _ if offset > 0 => None,
            None => None,
            Some(rows) if rows >= EXACT_COUNT_THRESHOLD => Some(rows),
            Some(_) => {
                let count = lease.conn().query_first::<u64, _>(format!("select count(*) from {relation}")).await;
                lease.check(count).map_err(|e| query_error(&e))?
            }
        };
        Ok(result)
    }

    async fn execute(&self, sql: &str, max_rows: Option<u32>) -> Result<QueryResult> {
        let mut lease = self.query.lease(&self.config).await?;
        self.cancelled.store(false, Ordering::SeqCst);
        // If this future is dropped mid-query, stop it on the server too.
        let guard = KillOnDrop { config: Some(self.config.clone()), thread_id: lease.conn().id() };
        let result = run_script(lease.conn(), sql, max_rows).await;
        let result = lease.check(result);
        guard.disarm();
        match result {
            _ if self.cancelled.swap(false, Ordering::SeqCst) => Err(Error::Cancelled),
            Ok(result) => Ok(result),
            Err(e) => Err(query_error(&e)),
        }
    }

    async fn cancel(&self) {
        let thread_id = self.query.thread_id.load(Ordering::Relaxed);
        if thread_id == 0 {
            return;
        }
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = kill_query(&self.config, thread_id).await;
    }
}

/// `KILL QUERY` must come from another connection: the script's own one is busy.
async fn kill_query(config: &ConnectionConfig, thread_id: u32) -> Result<()> {
    let mut conn = connect(config).await?;
    let result = conn.query_drop(format!("kill query {thread_id}")).await;
    let _ = conn.disconnect().await;
    result.map_err(|e| query_error(&e))
}

struct KillOnDrop {
    config: Option<ConnectionConfig>,
    thread_id: u32,
}

impl KillOnDrop {
    fn disarm(mut self) {
        self.config = None;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let Some(config) = self.config.take() else { return };
        let thread_id = self.thread_id;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = kill_query(&config, thread_id).await;
            });
        }
    }
}

// MARK: Table metadata

struct TableMeta {
    columns: Vec<ColumnInfo>,
    primary_key: Vec<String>,
    /// Server estimate for base tables; `None` for views.
    estimated_rows: Option<u64>,
}

impl TableMeta {
    async fn load(lease: &mut Lease<'_>, table: &TableInfo) -> Result<Self> {
        let schema = MYSQL.quote_literal(&table.schema);
        let name = MYSQL.quote_literal(&table.name);
        let info = lease
            .conn()
            .query_first::<(String, Option<u64>), _>(format!(
                "select table_type, table_rows from information_schema.tables where table_schema = {schema} and table_name = {name}"
            ))
            .await;
        let (table_type, table_rows) =
            lease.check(info).map_err(|e| query_error(&e))?.ok_or_else(|| Error::TableNotFound(table.qualified_name()))?;

        let columns = lease
            .conn()
            .query::<(String, String, String, String), _>(format!(
                "select column_name, column_type, is_nullable, column_key from information_schema.columns
                 where table_schema = {schema} and table_name = {name} order by ordinal_position"
            ))
            .await;
        let columns: Vec<ColumnInfo> = lease
            .check(columns)
            .map_err(|e| query_error(&e))?
            .into_iter()
            .map(|(name, type_name, nullable, key)| ColumnInfo {
                name,
                type_name,
                is_primary_key: key == "PRI",
                is_nullable: nullable == "YES",
            })
            .collect();
        Ok(Self {
            primary_key: columns.iter().filter(|c| c.is_primary_key).map(|c| c.name.clone()).collect(),
            columns,
            estimated_rows: if table_type == "VIEW" { None } else { Some(table_rows.unwrap_or(0)) },
        })
    }
}

// MARK: Running SQL

/// Runs a script (several statements allowed) and returns the last result set, or, if no
/// statement returned rows, the affected-row count of the last one. Rows past `max_rows` are
/// counted, not kept: the stream is drained so later statements still run.
async fn run_script(conn: &mut Conn, sql: &str, max_rows: Option<u32>) -> std::result::Result<QueryResult, mysql_async::Error> {
    let max_rows = max_rows.map_or(usize::MAX, |m| m as usize);
    let mut stream = conn.query_iter(sql).await?;
    let mut last_rows: Option<QueryResult> = None;
    let mut last_affected: Option<u64> = None;

    // `columns()` is `Some` while a statement's result is pending (empty for INSERT/DDL…).
    // `is_empty()` can't be used: it is already true while the last OK result is pending.
    while let Some(columns) = stream.columns() {
        if columns.is_empty() {
            last_affected = Some(stream.affected_rows());
            stream.reduce((), |(), _: mysql_async::Row| ()).await?;
            continue;
        }
        let mut result = QueryResult {
            columns: columns.iter().map(|c| ColumnInfo {
                name: c.name_str().into_owned(),
                type_name: type_name(c),
                is_primary_key: false,
                is_nullable: true,
            }).collect(),
            ..Default::default()
        };
        result = stream
            .reduce(result, |mut result, row: mysql_async::Row| {
                if result.rows.len() < max_rows {
                    let values = row.unwrap().into_iter().zip(columns.iter()).map(|(v, c)| decode(v, c)).collect();
                    result.rows.push(values);
                } else {
                    result.truncated = true;
                    *result.total_count.get_or_insert(max_rows as u64) += 1;
                }
                result
            })
            .await?;
        last_rows = Some(result);
    }
    // An error in a later statement arrives after the previous result: surface it.
    stream.drop_result().await?;
    Ok(last_rows.unwrap_or(QueryResult { rows_affected: last_affected, ..Default::default() }))
}

/// Turns a text-protocol value into a typed `Value` using the column metadata.
fn decode(value: mysql_async::Value, column: &Column) -> Value {
    use mysql_async::Value as V;
    let bytes = match value {
        V::NULL => return Value::Null,
        V::Bytes(b) => b,
        // The text protocol only sends bytes; keep the binary-protocol cases sensible anyway.
        V::Int(i) => return Value::Int(i),
        V::UInt(u) => return i64::try_from(u).map_or_else(|_| Value::Decimal(u.to_string()), Value::Int),
        V::Float(f) => return Value::Float(f.into()),
        V::Double(f) => return Value::Float(f),
        other => return Value::Text(other.as_sql(true)),
    };
    let text = || String::from_utf8_lossy(&bytes).into_owned();
    match column.column_type() {
        // TINYINT(1) is MySQL's BOOLEAN.
        ColumnType::MYSQL_TYPE_TINY if column.column_length() == 1 && !column.flags().contains(ColumnFlags::UNSIGNED_FLAG) => {
            match bytes.as_slice() {
                b"0" => Value::Bool(false),
                b"1" => Value::Bool(true),
                _ => Value::Text(text()),
            }
        }
        ColumnType::MYSQL_TYPE_TINY
        | ColumnType::MYSQL_TYPE_SHORT
        | ColumnType::MYSQL_TYPE_INT24
        | ColumnType::MYSQL_TYPE_LONG
        | ColumnType::MYSQL_TYPE_LONGLONG
        | ColumnType::MYSQL_TYPE_YEAR => {
            // BIGINT UNSIGNED above i64::MAX stays exact as a decimal.
            let text = text();
            text.parse().map_or(Value::Decimal(text), Value::Int)
        }
        ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE => {
            let text = text();
            text.parse().map_or(Value::Text(text), Value::Float)
        }
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => Value::Decimal(text()),
        ColumnType::MYSQL_TYPE_BIT => {
            // Sent as big-endian bytes, e.g. b'101' → [0x05].
            if bytes.len() <= 8 {
                Value::Int(bytes.iter().fold(0i64, |acc, b| (acc << 8) | i64::from(*b)))
            } else {
                Value::Text(hex_preview(&bytes))
            }
        }
        ColumnType::MYSQL_TYPE_JSON => Value::Text(text()),
        ColumnType::MYSQL_TYPE_GEOMETRY => Value::Text(hex_preview(&bytes)),
        _ if column.character_set() == BINARY_CHARSET && is_string_type(column.column_type()) => {
            Value::Text(hex_preview(&bytes))
        }
        _ => Value::Text(text()),
    }
}

fn is_string_type(ty: ColumnType) -> bool {
    matches!(
        ty,
        ColumnType::MYSQL_TYPE_STRING
            | ColumnType::MYSQL_TYPE_VAR_STRING
            | ColumnType::MYSQL_TYPE_VARCHAR
            | ColumnType::MYSQL_TYPE_BLOB
            | ColumnType::MYSQL_TYPE_TINY_BLOB
            | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
            | ColumnType::MYSQL_TYPE_LONG_BLOB
    )
}

/// SQL-ish names for script result columns (tables use `information_schema.columns.column_type`).
fn type_name(column: &Column) -> String {
    let binary = column.character_set() == BINARY_CHARSET;
    let unsigned = column.flags().contains(ColumnFlags::UNSIGNED_FLAG);
    let base = match column.column_type() {
        ColumnType::MYSQL_TYPE_TINY if column.column_length() == 1 && !unsigned => "tinyint(1)",
        ColumnType::MYSQL_TYPE_TINY => "tinyint",
        ColumnType::MYSQL_TYPE_SHORT => "smallint",
        ColumnType::MYSQL_TYPE_INT24 => "mediumint",
        ColumnType::MYSQL_TYPE_LONG => "int",
        ColumnType::MYSQL_TYPE_LONGLONG => "bigint",
        ColumnType::MYSQL_TYPE_FLOAT => "float",
        ColumnType::MYSQL_TYPE_DOUBLE => "double",
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => "decimal",
        ColumnType::MYSQL_TYPE_YEAR => "year",
        ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => "date",
        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => "time",
        ColumnType::MYSQL_TYPE_DATETIME | ColumnType::MYSQL_TYPE_DATETIME2 => "datetime",
        ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        ColumnType::MYSQL_TYPE_BIT => "bit",
        ColumnType::MYSQL_TYPE_JSON => "json",
        ColumnType::MYSQL_TYPE_ENUM => "enum",
        ColumnType::MYSQL_TYPE_SET => "set",
        ColumnType::MYSQL_TYPE_GEOMETRY => "geometry",
        ColumnType::MYSQL_TYPE_STRING if binary => "binary",
        ColumnType::MYSQL_TYPE_STRING => "char",
        ColumnType::MYSQL_TYPE_VAR_STRING | ColumnType::MYSQL_TYPE_VARCHAR if binary => "varbinary",
        ColumnType::MYSQL_TYPE_VAR_STRING | ColumnType::MYSQL_TYPE_VARCHAR => "varchar",
        ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_BLOB
            if binary => "blob",
        ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_BLOB => "text",
        ColumnType::MYSQL_TYPE_NULL => "null",
        _ => "",
    };
    if unsigned && !base.is_empty() { format!("{base} unsigned") } else { base.into() }
}

// MARK: Errors

/// `ERROR 1064 (42000): You have an error in your SQL syntax; … at line 1`, like the mysql client.
fn query_error(e: &mysql_async::Error) -> Error {
    match e {
        mysql_async::Error::Server(s) if s.code == ER_QUERY_INTERRUPTED => Error::Cancelled,
        mysql_async::Error::Server(s) => Error::Query(format!("ERROR {} ({}): {}", s.code, s.state, s.message)),
        mysql_async::Error::Io(_) | mysql_async::Error::Driver(_) => Error::ConnectionFailed(error_chain(e)),
        _ => Error::Query(error_chain(e)),
    }
}

/// Server-reported startup errors (bad password, unknown database…) without the wrapper text.
fn connect_error(e: &mysql_async::Error) -> String {
    match e {
        mysql_async::Error::Server(s) => s.message.clone(),
        _ => error_chain(e),
    }
}
