//! Hardcoded sample connections, schemas and data so frontends can be built before real drivers exist.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;

use crate::driver::{Driver, Error, Result};
use crate::model::*;

/// Connections that simulate a failure (shows the warning state in the UI).
pub const UNREACHABLE: &[&str] = &["prod-replica"];

/// The sample connection backed by the real dev database (`scripts/dev-db.sh up`).
pub const DEV_DATABASE: &str = "local-pg";

/// Sample connections served by [`MockDriver`] (everything except the dev database).
pub fn is_mock(config: &ConnectionConfig) -> bool {
    config.id != DEV_DATABASE && connections().iter().any(|c| c.id == config.id)
}

pub fn connections() -> Vec<ConnectionConfig> {
    #[allow(clippy::too_many_arguments)]
    fn conn(
        id: &str, name: &str, group: &str, kind: DatabaseKind, host: &str, port: Option<u16>, db: &str,
        user: Option<&str>,
    ) -> ConnectionConfig {
        ConnectionConfig {
            id: id.into(),
            name: name.into(),
            group: group.into(),
            kind,
            host: host.into(),
            port,
            database: db.into(),
            user: user.map(Into::into),
            password: None,
            ssl_mode: SslMode::default(),
        }
    }
    use DatabaseKind::*;
    vec![
        ConnectionConfig {
            password: Some("postgres".into()),
            ..conn(DEV_DATABASE, "app_dev", "Local", Postgres, "localhost", Some(54329), "app_dev", Some("postgres"))
        },
        conn("local-mysql", "wordpress", "Local", Mysql, "localhost", Some(3306), "wordpress", Some("root")),
        conn("local-sqlite", "notes.db", "Local", Sqlite, "~/Library/Application Support/Notes", None, "notes.db", None),
        conn("staging-pg", "app_staging", "Staging", Postgres, "staging-db.internal", Some(5432), "app", Some("readonly")),
        conn("prod-pg", "app_production", "Production", Postgres, "prod-db.internal", Some(5432), "app", Some("readonly")),
        conn("prod-replica", "app_replica", "Production", Postgres, "replica-db.internal", Some(5432), "app", Some("readonly")),
    ]
}

struct TableSpec {
    name: &'static str,
    kind: TableKind,
    rows: u64,
    columns: Vec<ColumnInfo>,
}

type Specs = BTreeMap<&'static str, Vec<TableSpec>>;

fn col(name: &str, ty: &str) -> ColumnInfo {
    ColumnInfo { name: name.into(), type_name: ty.into(), is_primary_key: false, is_nullable: false }
}
fn pk(name: &str, ty: &str) -> ColumnInfo {
    ColumnInfo { is_primary_key: true, ..col(name, ty) }
}
fn nullable(name: &str, ty: &str) -> ColumnInfo {
    ColumnInfo { is_nullable: true, ..col(name, ty) }
}
fn table(name: &'static str, rows: u64, columns: Vec<ColumnInfo>) -> TableSpec {
    TableSpec { name, kind: TableKind::Table, rows, columns }
}
fn view(name: &'static str, rows: u64, columns: Vec<ColumnInfo>) -> TableSpec {
    TableSpec { name, kind: TableKind::View, rows, columns }
}

fn postgres_specs() -> &'static Specs {
    static S: OnceLock<Specs> = OnceLock::new();
    S.get_or_init(|| {
        BTreeMap::from([
            ("public", vec![
                table("users", 248, vec![pk("id", "bigint"), col("name", "text"), col("email", "text"),
                    col("is_admin", "boolean"), nullable("last_login_at", "timestamptz"), col("created_at", "timestamptz")]),
                table("orders", 1_204, vec![pk("id", "bigint"), col("user_id", "bigint"), col("status", "text"),
                    col("total", "numeric"), nullable("notes", "text"), col("created_at", "timestamptz")]),
                table("products", 86, vec![pk("id", "bigint"), col("sku", "text"), col("name", "text"),
                    col("price", "numeric"), col("in_stock", "boolean")]),
                table("sessions", 512, vec![pk("id", "uuid"), col("user_id", "bigint"), col("ip", "inet"),
                    col("expires_at", "timestamptz")]),
                view("active_users", 37, vec![col("id", "bigint"), col("name", "text"), col("email", "text"),
                    col("last_login_at", "timestamptz")]),
            ]),
            ("billing", vec![
                table("invoices", 930, vec![pk("id", "bigint"), col("order_id", "bigint"), col("amount", "numeric"),
                    col("paid", "boolean"), col("due_at", "timestamptz")]),
                table("payments", 874, vec![pk("id", "bigint"), col("invoice_id", "bigint"), col("provider", "text"),
                    col("amount", "numeric"), col("created_at", "timestamptz")]),
                table("subscriptions", 61, vec![pk("id", "bigint"), col("user_id", "bigint"), col("plan", "text"),
                    col("status", "text"), nullable("canceled_at", "timestamptz")]),
            ]),
            ("analytics", vec![
                table("events", 50_000, vec![pk("id", "bigint"), nullable("user_id", "bigint"), col("name", "text"),
                    col("path", "text"), col("created_at", "timestamptz")]),
                view("daily_signups", 90, vec![col("day", "date"), col("count", "bigint")]),
            ]),
        ])
    })
}

fn mysql_specs() -> &'static Specs {
    static S: OnceLock<Specs> = OnceLock::new();
    S.get_or_init(|| {
        BTreeMap::from([("wordpress", vec![
            table("wp_posts", 312, vec![pk("ID", "bigint"), col("post_title", "varchar"), col("status", "varchar"),
                col("post_author", "bigint"), col("created_at", "datetime")]),
            table("wp_users", 12, vec![pk("ID", "bigint"), col("user_login", "varchar"), col("email", "varchar"),
                col("created_at", "datetime")]),
            table("wp_options", 140, vec![pk("option_id", "bigint"), col("option_name", "varchar"),
                nullable("option_value", "longtext")]),
        ])])
    })
}

fn sqlite_specs() -> &'static Specs {
    static S: OnceLock<Specs> = OnceLock::new();
    S.get_or_init(|| {
        BTreeMap::from([("main", vec![
            table("notes", 57, vec![pk("id", "INTEGER"), col("title", "TEXT"), nullable("body", "TEXT"),
                col("pinned", "INTEGER"), col("created_at", "TEXT")]),
            table("tags", 9, vec![pk("id", "INTEGER"), col("name", "TEXT")]),
        ])])
    })
}

fn specs(kind: DatabaseKind) -> &'static Specs {
    match kind {
        DatabaseKind::Postgres => postgres_specs(),
        DatabaseKind::Mysql => mysql_specs(),
        DatabaseKind::Sqlite => sqlite_specs(),
    }
}

// MARK: Deterministic fake values

const NAMES: &[&str] = &["Ada Lovelace", "Alan Turing", "Grace Hopper", "Linus Torvalds", "Barbara Liskov",
    "Ken Thompson", "Dennis Ritchie", "Margaret Hamilton", "Edsger Dijkstra", "Donald Knuth"];
const STATUSES: &[&str] = &["pending", "paid", "shipped", "delivered", "canceled", "refunded"];
const WORDS: &[&str] = &["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel"];

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn value(column: &ColumnInfo, i: u64) -> Value {
    if column.is_nullable && i % 4 == 1 {
        return Value::Null;
    }
    let n = column.name.to_lowercase();
    let t = column.type_name.to_lowercase();
    let w = |k: u64| WORDS[(k % WORDS.len() as u64) as usize];
    let text = |s: String| Value::Text(s);

    if column.is_primary_key && t == "uuid" {
        return text(format!("{:08x}-4b1c-9e2a-{:012x}", i.wrapping_mul(2_654_435_761) & 0xffff_ffff, i * 7919));
    }
    if column.is_primary_key || n == "id" {
        return Value::Int(i as i64 + 1);
    }
    if n.ends_with("_id") || n == "post_author" {
        return Value::Int(((i * 37) % 250 + 1) as i64);
    }
    if t == "boolean" || n == "pinned" {
        return Value::Bool(!i.is_multiple_of(3));
    }
    if t == "numeric" {
        return Value::Decimal(format!("{:.2}", ((i * 1_733) % 50_000) as f64 / 100.0 + 4.99));
    }
    if t.contains("time") || t == "date" || n.ends_with("_at") || n == "day" {
        let (day, month) = (1 + i % 28, 1 + (i / 28) % 12);
        let (hour, minute) = ((i * 7) % 24, (i * 13) % 60);
        let date = format!("2025-{month:02}-{day:02}");
        return text(if t == "date" { date } else { format!("{date} {hour:02}:{minute:02}:00") });
    }
    if n.ends_with("email") {
        let name = NAMES[(i % NAMES.len() as u64) as usize].to_lowercase().replace(' ', ".");
        return text(format!("{name}{i}@example.com"));
    }
    if n == "option_name" {
        return text(format!("option_{}_{i}", w(i)));
    }
    if n == "post_title" || n == "title" {
        return text(format!("{} note #{}", capitalized(w(i)), i + 1));
    }
    if n.contains("name") || n == "user_login" {
        return text(NAMES[(i % NAMES.len() as u64) as usize].into());
    }
    match n.as_str() {
        "status" => text(STATUSES[(i % STATUSES.len() as u64) as usize].into()),
        "plan" => text(["free", "pro", "team"][(i % 3) as usize].into()),
        "provider" => text(["stripe", "paypal", "mercadopago"][(i % 3) as usize].into()),
        "sku" => text(format!("SKU-{:05}", i * 17)),
        "ip" => text(format!("10.0.{}.{}", i % 255, (i * 3) % 255)),
        "path" => text(format!("/{}/{}", w(i), w(i + 3))),
        _ if t == "bigint" || t == "integer" => Value::Int(((i * 31) % 1_000) as i64),
        _ => text(format!("{} {}", w(i), w(i * 5))),
    }
}

pub struct MockDriver {
    config: ConnectionConfig,
    connected: AtomicBool,
}

impl MockDriver {
    pub fn new(config: ConnectionConfig) -> Self {
        Self { config, connected: AtomicBool::new(false) }
    }

    async fn latency() {
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    fn find(&self, schema: Option<&str>, name: &str) -> Option<(&'static str, &'static TableSpec)> {
        let specs = specs(self.config.kind);
        // Unqualified names resolve against "public" first, like a default search_path.
        let mut order: Vec<_> = specs.iter().collect();
        order.sort_by_key(|(k, _)| (**k != "public", **k));
        order.into_iter().filter(|(k, _)| schema.is_none_or(|s| s == **k)).find_map(|(k, tables)| {
            tables.iter().find(|t| t.name == name).map(|t| (*k, t))
        })
    }
}

#[async_trait]
impl Driver for MockDriver {
    fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    async fn connect(&self) -> Result<()> {
        Self::latency().await;
        if UNREACHABLE.contains(&self.config.id.as_str()) {
            return Err(Error::ConnectionFailed(format!(
                "could not connect to server at \"{}\" (timeout)",
                self.config.host
            )));
        }
        self.connected.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&self) {
        self.connected.store(false, Ordering::Relaxed);
    }

    async fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    async fn list_schemas(&self) -> Result<Vec<Schema>> {
        self.connect().await?;
        Ok(specs(self.config.kind)
            .iter()
            .map(|(name, tables)| Schema {
                name: (*name).into(),
                tables: tables
                    .iter()
                    .map(|t| TableInfo {
                        schema: (*name).into(),
                        name: t.name.into(),
                        kind: t.kind,
                        estimated_row_count: Some(t.rows),
                    })
                    .collect(),
            })
            .collect())
    }

    async fn fetch_rows(&self, table: &TableInfo, limit: u32, offset: u64) -> Result<QueryResult> {
        self.connect().await?;
        let (_, spec) = self
            .find(Some(&table.schema), &table.name)
            .ok_or_else(|| Error::TableNotFound(table.qualified_name()))?;
        let end = spec.rows.min(offset + limit as u64);
        Ok(QueryResult {
            columns: spec.columns.clone(),
            rows: (offset..end.max(offset)).map(|i| spec.columns.iter().map(|c| value(c, i)).collect()).collect(),
            total_count: Some(spec.rows),
            rows_affected: None,
            truncated: false,
        })
    }

    /// Understands just enough SQL to be useful for UI work:
    /// `SELECT * | col, col FROM [schema.]table [LIMIT n]`.
    async fn execute(&self, sql: &str, max_rows: Option<u32>) -> Result<QueryResult> {
        self.connect().await?;

        static SELECT: OnceLock<Regex> = OnceLock::new();
        let re = SELECT.get_or_init(|| {
            Regex::new(r#"(?is)^select\s+(.+?)\s+from\s+"?(\w+)"?(?:\."?(\w+)"?)?(?:\s+limit\s+(\d+))?$"#).unwrap()
        });

        let statement = sql
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join(" ");
        let statement = statement.trim().trim_end_matches(';').trim();

        let caps = re.captures(statement).ok_or_else(|| {
            Error::Unsupported("the mock driver only understands SELECT … FROM table [LIMIT n]".into())
        })?;
        let (schema, name) = match (caps.get(2), caps.get(3)) {
            (Some(s), Some(t)) => (Some(s.as_str()), t.as_str()),
            (Some(t), None) => (None, t.as_str()),
            _ => unreachable!(),
        };
        let (schema, spec) = self.find(schema, name).ok_or_else(|| {
            Error::TableNotFound(schema.map_or(name.to_string(), |s| format!("{s}.{name}")))
        })?;
        let wanted: u64 = caps.get(4).and_then(|m| m.as_str().parse().ok()).unwrap_or(spec.rows).min(spec.rows);
        let kept = max_rows.map_or(wanted, |m| wanted.min(m as u64));
        let mut full = self.fetch_rows(&TableInfo::new(schema, spec.name), kept as u32, 0).await?;
        full.truncated = kept < wanted;
        full.total_count = full.truncated.then_some(wanted);

        let select_list = caps[1].trim();
        if select_list == "*" {
            return Ok(full);
        }
        let indices = select_list
            .split(',')
            .map(|c| {
                let c = c.trim();
                full.columns
                    .iter()
                    .position(|col| col.name.eq_ignore_ascii_case(c))
                    .ok_or_else(|| Error::Query(format!("column \"{c}\" does not exist")))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(QueryResult {
            columns: indices.iter().map(|&i| full.columns[i].clone()).collect(),
            rows: full.rows.iter().map(|r| indices.iter().map(|&i| r[i].clone()).collect()).collect(),
            ..full
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Connection;

    /// Mock Postgres connection with the same sample schema the old app_dev mock had.
    fn app_dev() -> Connection {
        Connection::new(connections().into_iter().find(|c| c.id == "staging-pg").unwrap())
    }

    // Plain #[test] + a throwaway executor proves Connection works outside tokio (as from Swift/GPUI).
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    #[test]
    fn lists_schemas_sorted() {
        let schemas = block_on(app_dev().list_schemas()).unwrap();
        let names: Vec<_> = schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["analytics", "billing", "public"]);
    }

    #[test]
    fn fetches_rows_with_matching_column_count() {
        let result = block_on(app_dev().fetch_rows(TableInfo::new("public", "users"), 50, 0)).unwrap();
        assert_eq!(result.rows.len(), 50);
        assert!(result.rows.iter().all(|r| r.len() == result.columns.len()));
        assert_eq!(result.total_count, Some(248));
    }

    #[test]
    fn pages_past_the_end_are_empty() {
        let result = block_on(app_dev().fetch_rows(TableInfo::new("public", "users"), 50, 1_000)).unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn unreachable_connection_fails() {
        let config = connections().into_iter().find(|c| UNREACHABLE.contains(&c.id.as_str())).unwrap();
        let err = block_on(Connection::new(config).list_schemas()).unwrap_err();
        assert!(matches!(err, Error::ConnectionFailed(_)));
    }

    #[test]
    fn executes_simple_select() {
        let result = block_on(app_dev().execute("-- comment\nselect id, email from users limit 5;".into())).unwrap();
        let cols: Vec<_> = result.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(cols, ["id", "email"]);
        assert_eq!(result.rows.len(), 5);
    }

    #[test]
    fn caps_script_rows() {
        let r = block_on(app_dev().execute_limited("select * from users".into(), Some(100))).unwrap();
        assert_eq!((r.rows.len(), r.truncated, r.total_count), (100, true, Some(248)));
        let r = block_on(app_dev().execute_limited("select * from users limit 10".into(), Some(100))).unwrap();
        assert_eq!((r.rows.len(), r.truncated, r.total_count), (10, false, None));
    }

    #[test]
    fn rejects_unsupported_sql() {
        let err = block_on(app_dev().execute("delete from users".into())).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
    }

    #[test]
    fn tracks_connection_state() {
        let conn = app_dev();
        assert!(!block_on(conn.is_connected()));
        block_on(conn.list_schemas()).unwrap();
        assert!(block_on(conn.is_connected()));
        block_on(conn.disconnect());
        assert!(!block_on(conn.is_connected()));
    }

    #[test]
    fn only_the_dev_database_is_real() {
        let real: Vec<_> = connections().into_iter().filter(|c| !is_mock(c)).map(|c| c.id).collect();
        assert_eq!(real, [DEV_DATABASE]);
        assert!(format!("{:?}", connections()[0]).contains("password: Some(\"•••\")"));
    }

    #[test]
    fn summary_formats() {
        let c = &connections();
        assert_eq!(c[0].summary(), "PostgreSQL · localhost:54329/app_dev");
        assert_eq!(c[2].summary(), "SQLite · notes.db");
    }
}
