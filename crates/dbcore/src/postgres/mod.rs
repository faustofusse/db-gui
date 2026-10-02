//! PostgreSQL driver (tokio-postgres + rustls).
//!
//! Values are fetched in Postgres' text format (`simple_query`), so every type, including
//! extension types, renders exactly like psql shows it. Column types come from `prepare`
//! (or the catalog for table browsing), and are used to turn ints/floats/bools into typed
//! values and NUMERIC into an exact `Value::Decimal`.

mod tls;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{pin_mut, TryStreamExt};
use tokio::sync::Mutex;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{Kind, Type};
use tokio_postgres::{CancelToken, Client, SimpleQueryMessage};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::driver::{Driver, Error, Result};
use crate::model::*;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Tables smaller than this (by planner estimate) get an exact `count(*)`.
const EXACT_COUNT_THRESHOLD: f32 = 100_000.0;

pub struct PostgresDriver {
    config: ConnectionConfig,
    /// Schema listing and table browsing.
    browse: Session,
    /// User scripts: a long query here doesn't block browsing, and `cancel` only hits scripts.
    query: Session,
}

impl PostgresDriver {
    pub fn new(config: ConnectionConfig) -> Self {
        Self { config, browse: Session::default(), query: Session::default() }
    }

    async fn browse_client(&self) -> Result<Arc<Client>> {
        self.browse.client(&self.config).await
    }
}

/// One lazily (re)connected server connection. Queries on it are pipelined, not serialized here.
#[derive(Default)]
struct Session {
    client: Mutex<Option<Arc<Client>>>,
}

impl Session {
    async fn client(&self, config: &ConnectionConfig) -> Result<Arc<Client>> {
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref().filter(|c| !c.is_closed()) {
            return Ok(client.clone());
        }
        let client = Arc::new(connect(config).await?);
        *slot = Some(client.clone());
        Ok(client)
    }

    async fn is_open(&self) -> bool {
        self.client.lock().await.as_ref().is_some_and(|c| !c.is_closed())
    }

    /// Token for the current connection, if any (cancel needs no client lock).
    async fn cancel_token(&self) -> Option<CancelToken> {
        self.client.lock().await.as_ref().map(|c| c.cancel_token())
    }

    async fn close(&self) {
        // Dropping the client ends the connection task, which sends Terminate.
        self.client.lock().await.take();
    }
}

async fn connect(config: &ConnectionConfig) -> Result<Client> {
    let mut pg = tokio_postgres::Config::new();
    pg.host(&config.host)
        .port(config.port.unwrap_or(5432))
        .dbname(&config.database)
        .user(config.user.as_deref().unwrap_or("postgres"))
        .application_name("DBGui")
        .connect_timeout(CONNECT_TIMEOUT)
        .ssl_mode(match config.ssl_mode {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require | SslMode::VerifyFull => tokio_postgres::config::SslMode::Require,
        });
    if let Some(password) = &config.password {
        pg.password(password);
    }

    let (client, connection) = pg
        .connect(tls::connector(config.ssl_mode)?)
        .await
        .map_err(|e| Error::ConnectionFailed(error_chain(&e)))?;
    tokio::spawn(async move {
        // Ends when the client is dropped or the server goes away; `is_closed` reports the latter.
        let _ = connection.await;
    });
    Ok(client)
}

#[async_trait]
impl Driver for PostgresDriver {
    fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    async fn connect(&self) -> Result<()> {
        self.browse_client().await.map(drop)
    }

    async fn disconnect(&self) {
        // A running script holds its own client handle; stop it so the connection really goes away.
        self.cancel().await;
        self.browse.close().await;
        self.query.close().await;
    }

    async fn is_connected(&self) -> bool {
        self.browse.is_open().await || self.query.is_open().await
    }

    async fn list_schemas(&self) -> Result<Vec<Schema>> {
        let client = self.browse_client().await?;
        let rows = client
            .query(
                r"
                select n.nspname, c.relname, c.relkind::text, c.reltuples
                from pg_namespace n
                left join pg_class c
                  on c.relnamespace = n.oid
                 and c.relkind in ('r', 'p', 'v', 'm', 'f')
                 and not c.relispartition
                where n.nspname not in ('information_schema')
                  and n.nspname not like 'pg\_%'
                order by n.nspname, c.relname
                ",
                &[],
            )
            .await
            .map_err(|e| query_error(&e, None))?;

        let mut schemas: Vec<Schema> = Vec::new();
        for row in rows {
            let schema: String = row.get(0);
            if schemas.last().is_none_or(|s| s.name != schema) {
                schemas.push(Schema { name: schema.clone(), tables: Vec::new() });
            }
            let Some(name) = row.get::<_, Option<String>>(1) else { continue };
            let relkind: String = row.get(2);
            let reltuples: f32 = row.get(3);
            let kind = if matches!(relkind.as_str(), "v" | "m") { TableKind::View } else { TableKind::Table };
            schemas.last_mut().unwrap().tables.push(TableInfo {
                schema,
                name,
                kind,
                // -1 means "never analyzed"; views have no estimate.
                estimated_row_count: (relkind != "v" && reltuples >= 0.0).then_some(reltuples as u64),
            });
        }
        Ok(schemas)
    }

    async fn fetch_rows(&self, table: &TableInfo, limit: u32, offset: u64) -> Result<QueryResult> {
        let client = self.browse_client().await?;
        let relation = quote_relation(&table.schema, &table.name);
        let meta = TableMeta::load(&client, &relation, table).await?;

        let order_by = if !meta.primary_key.is_empty() {
            format!(" order by {}", meta.primary_key.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", "))
        } else if meta.relkind == "r" {
            // No primary key: physical order is stable enough for paging an idle table.
            " order by ctid".into()
        } else {
            String::new()
        };
        let sql = format!("select * from {relation}{order_by} limit {limit} offset {offset}");

        let mut result = run_single(&client, &sql).await?;
        // Catalog names read better than wire type names ("timestamp with time zone" vs "timestamptz").
        result.columns = meta.columns;

        result.total_count = match meta.relkind.as_str() {
            "r" | "p" | "m" if meta.reltuples >= EXACT_COUNT_THRESHOLD => {
                Some(meta.reltuples as u64)
            }
            "r" | "p" | "m" => {
                let row = client
                    .query_one(&format!("select count(*) from {relation}"), &[])
                    .await
                    .map_err(|e| query_error(&e, None))?;
                Some(row.get::<_, i64>(0) as u64)
            }
            _ => None,
        };
        Ok(result)
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult> {
        let client = self.query.client(&self.config).await?;
        // If this future is dropped (e.g. the caller's task is cancelled), stop the server-side query too.
        let guard = CancelOnDrop::new(client.cancel_token(), self.config.ssl_mode);
        let result = run_script(&client, sql).await;
        guard.disarm();
        result
    }

    async fn cancel(&self) {
        if let Some(token) = self.query.cancel_token().await {
            let _ = send_cancel(token, self.config.ssl_mode).await;
        }
    }
}

// MARK: Running SQL

/// Runs one statement; column types come from `prepare`.
async fn run_single(client: &Client, sql: &str) -> Result<QueryResult> {
    let statement = client.prepare(sql).await.map_err(|e| query_error(&e, Some(sql)))?;
    let types: Vec<Type> = statement.columns().iter().map(|c| c.type_().clone()).collect();
    collect(client, sql, Some(&types)).await
}

/// Runs a script of one or more statements and returns the last result set, or, if no
/// statement returned rows, the affected-row count of the last one.
async fn run_script(client: &Client, sql: &str) -> Result<QueryResult> {
    match client.prepare(sql).await {
        Ok(statement) => {
            let types: Vec<Type> = statement.columns().iter().map(|c| c.type_().clone()).collect();
            collect(client, sql, Some(&types)).await
        }
        // Several statements can't be prepared; run them as-is with values left as text.
        Err(e) if is_multi_statement_error(&e) => collect(client, sql, None).await,
        Err(e) => Err(query_error(&e, Some(sql))),
    }
}

fn is_multi_statement_error(e: &tokio_postgres::Error) -> bool {
    e.as_db_error().is_some_and(|db| {
        db.code() == &SqlState::SYNTAX_ERROR && db.message().contains("multiple commands")
    })
}

async fn collect(client: &Client, sql: &str, types: Option<&[Type]>) -> Result<QueryResult> {
    let stream = client.simple_query_raw(sql).await.map_err(|e| query_error(&e, Some(sql)))?;
    pin_mut!(stream);

    let mut current: Option<QueryResult> = None;
    let mut last_rows: Option<QueryResult> = None;
    let mut last_affected: Option<u64> = None;

    while let Some(message) = stream.try_next().await.map_err(|e| query_error(&e, Some(sql)))? {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                current = Some(QueryResult {
                    columns: columns
                        .iter()
                        .enumerate()
                        .map(|(i, c)| ColumnInfo {
                            name: c.name().into(),
                            type_name: types.and_then(|t| t.get(i)).map(type_name).unwrap_or_default(),
                            is_primary_key: false,
                            is_nullable: true,
                        })
                        .collect(),
                    ..Default::default()
                });
            }
            SimpleQueryMessage::Row(row) => {
                if let Some(result) = current.as_mut() {
                    let values = (0..row.len())
                        .map(|i| decode(row.get(i), types.and_then(|t| t.get(i))))
                        .collect();
                    result.rows.push(values);
                }
            }
            SimpleQueryMessage::CommandComplete(count) => match current.take() {
                Some(result) => last_rows = Some(result),
                None => last_affected = Some(count),
            },
            _ => {}
        }
    }

    Ok(last_rows.unwrap_or(QueryResult { rows_affected: last_affected, ..Default::default() }))
}

/// Turns a text-format value into a typed `Value` when the type is known.
fn decode(text: Option<&str>, ty: Option<&Type>) -> Value {
    let Some(text) = text else { return Value::Null };
    let typed = match ty {
        Some(&Type::BOOL) => match text {
            "t" => Some(Value::Bool(true)),
            "f" => Some(Value::Bool(false)),
            _ => None,
        },
        Some(&Type::INT2 | &Type::INT4 | &Type::INT8 | &Type::OID) => text.parse().ok().map(Value::Int),
        Some(&Type::FLOAT4 | &Type::FLOAT8) => text.parse().ok().map(Value::Float),
        Some(&Type::NUMERIC) => Some(Value::Decimal(text.into())),
        _ => None,
    };
    typed.unwrap_or_else(|| Value::Text(text.into()))
}

/// SQL-ish type names: `text[]` instead of `_text`.
fn type_name(ty: &Type) -> String {
    match ty.kind() {
        Kind::Array(inner) => format!("{}[]", inner.name()),
        _ => ty.name().into(),
    }
}

// MARK: Table metadata

struct TableMeta {
    relkind: String,
    reltuples: f32,
    columns: Vec<ColumnInfo>,
    primary_key: Vec<String>,
}

impl TableMeta {
    async fn load(client: &Client, relation: &str, table: &TableInfo) -> Result<Self> {
        let rows = client
            .query(
                r"
                select c.relkind::text, c.reltuples, a.attname,
                       format_type(a.atttypid, a.atttypmod), a.attnotnull,
                       coalesce(a.attnum = any(i.indkey), false)
                from pg_class c
                join pg_attribute a on a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped
                left join pg_index i on i.indrelid = c.oid and i.indisprimary
                where c.oid = $1::text::regclass
                order by a.attnum
                ",
                &[&relation],
            )
            .await
            .map_err(|e| match e.code() {
                Some(&SqlState::UNDEFINED_TABLE) | Some(&SqlState::INVALID_SCHEMA_NAME) => {
                    Error::TableNotFound(table.qualified_name())
                }
                _ => query_error(&e, None),
            })?;

        let first = rows.first().ok_or_else(|| Error::TableNotFound(table.qualified_name()))?;
        let columns: Vec<ColumnInfo> = rows
            .iter()
            .map(|r| ColumnInfo {
                name: r.get(2),
                type_name: r.get(3),
                is_primary_key: r.get(5),
                is_nullable: !r.get::<_, bool>(4),
            })
            .collect();
        Ok(Self {
            relkind: first.get(0),
            reltuples: first.get(1),
            primary_key: columns.iter().filter(|c| c.is_primary_key).map(|c| c.name.clone()).collect(),
            columns,
        })
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn quote_relation(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(name))
}

// MARK: Cancellation

async fn send_cancel(token: CancelToken, mode: SslMode) -> Result<()> {
    let tls: MakeRustlsConnect = tls::connector(mode)?;
    token.cancel_query(tls).await.map_err(|e| Error::ConnectionFailed(error_chain(&e)))
}

/// Sends a cancel request for the in-flight query unless disarmed before being dropped.
struct CancelOnDrop {
    token: Option<CancelToken>,
    mode: SslMode,
}

impl CancelOnDrop {
    fn new(token: CancelToken, mode: SslMode) -> Self {
        Self { token: Some(token), mode }
    }

    fn disarm(mut self) {
        self.token = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else { return };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let mode = self.mode;
            handle.spawn(async move {
                let _ = send_cancel(token, mode).await;
            });
        }
    }
}

// MARK: Errors

/// `ERROR: message` plus DETAIL/HINT and, when we have the SQL, the line and column.
fn query_error(e: &tokio_postgres::Error, sql: Option<&str>) -> Error {
    let Some(db) = e.as_db_error() else {
        return if e.is_closed() {
            Error::ConnectionFailed("the server closed the connection".into())
        } else {
            Error::Query(error_chain(e))
        };
    };
    if db.code() == &SqlState::QUERY_CANCELED {
        return Error::Cancelled;
    }

    let mut message = format!("{}: {}", db.severity(), db.message());
    if let (Some(sql), Some(tokio_postgres::error::ErrorPosition::Original(position))) = (sql, db.position()) {
        let (line, column) = line_column(sql, *position as usize);
        message.push_str(&format!(" (line {line}, column {column})"));
    }
    if let Some(detail) = db.detail() {
        message.push_str(&format!("\nDETAIL: {detail}"));
    }
    if let Some(hint) = db.hint() {
        message.push_str(&format!("\nHINT: {hint}"));
    }
    Error::Query(message)
}

/// Postgres positions are 1-based character offsets.
fn line_column(sql: &str, position: usize) -> (usize, usize) {
    let before: String = sql.chars().take(position.saturating_sub(1)).collect();
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// "error connecting to server: Connection refused (os error 61)" instead of just the top level.
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut message = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = s.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_values_by_type() {
        assert_eq!(decode(None, Some(&Type::INT8)), Value::Null);
        assert_eq!(decode(Some("42"), Some(&Type::INT4)), Value::Int(42));
        assert_eq!(decode(Some("t"), Some(&Type::BOOL)), Value::Bool(true));
        assert_eq!(decode(Some("1.5"), Some(&Type::FLOAT8)), Value::Float(1.5));
        assert_eq!(decode(Some("NaN"), Some(&Type::NUMERIC)), Value::Decimal("NaN".into()));
        assert_eq!(decode(Some("{a,b}"), Some(&Type::TEXT_ARRAY)), Value::Text("{a,b}".into()));
        assert_eq!(decode(Some("7"), None), Value::Text("7".into()));
    }

    #[test]
    fn quotes_identifiers() {
        assert_eq!(quote_relation("public", r#"we"ird"#), r#""public"."we""ird""#);
    }

    #[test]
    fn maps_positions_to_line_and_column() {
        assert_eq!(line_column("select\n  fro", 10), (2, 3));
        assert_eq!(line_column("selec 1", 1), (1, 1));
    }
}
