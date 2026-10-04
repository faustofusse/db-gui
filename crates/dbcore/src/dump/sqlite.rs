//! SQLite dumps, in the format of `sqlite3 .dump`: tables with their rows, then indexes,
//! views and triggers, in one transaction.
//!
//! rusqlite is synchronous: a blocking thread reads the file (its own read-only connection, in
//! one read transaction) and sends SQL text to the async side, which writes it out.

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::OpenFlags;
use tokio::sync::mpsc;

use super::literal::{sqlite_blob, sqlite_float, sqlite_text};
use super::{Ctx, DumpOptions, DumpPhase};
use crate::dialect::Dialect;
use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind};

const SQLITE: Dialect = Dialect(DatabaseKind::Sqlite);
/// Text is handed to the writer in pieces of about this size.
const PIECE: usize = 64 * 1024;

enum Event {
    Text(String),
    Phase(DumpPhase),
    TablesTotal(u32),
    Table { name: String, rows: Option<u64> },
    Rows(u64),
    EndTable,
    Warning(String),
}

pub(super) async fn dump(config: &ConnectionConfig, ctx: &mut Ctx) -> Result<()> {
    let path = crate::sqlite::database_path(&config.database);
    let options = ctx.options.clone();
    let (tx, mut rx) = mpsc::channel(16);
    let producer = tokio::task::spawn_blocking(move || {
        let mut emitter = Emitter { tx, text: String::with_capacity(PIECE * 2) };
        let result = produce(&path, &options, &mut emitter);
        let flushed = emitter.flush();
        result.and(flushed)
    });
    // When this future is dropped (cancel), `rx` goes away and the producer stops at its next send.
    header(ctx, config);
    while let Some(event) = rx.recv().await {
        match event {
            Event::Text(text) => ctx.push(&text),
            Event::Phase(phase) => ctx.phase(phase),
            Event::TablesTotal(n) => ctx.set_tables_total(n),
            Event::Table { name, rows } => ctx.begin_table(name, rows),
            Event::Rows(n) => ctx.rows(n),
            Event::EndTable => ctx.end_table(),
            Event::Warning(w) => ctx.warn(w),
        }
        ctx.tick().await?;
    }
    producer.await.map_err(|e| Error::Internal(e.to_string()))?
}

fn header(ctx: &mut Ctx, config: &ConnectionConfig) {
    let version = rusqlite::version();
    super::header(ctx, config, version);
}

struct Emitter {
    tx: mpsc::Sender<Event>,
    text: String,
}

impl Emitter {
    fn send(&self, event: Event) -> Result<()> {
        // The receiver is gone only when the dump was cancelled or failed on the async side.
        self.tx.blocking_send(event).map_err(|_| Error::Cancelled)
    }

    fn flush(&mut self) -> Result<()> {
        if self.text.is_empty() {
            return Ok(());
        }
        let text = std::mem::replace(&mut self.text, String::with_capacity(PIECE * 2));
        self.send(Event::Text(text))
    }

    fn event(&mut self, event: Event) -> Result<()> {
        self.flush()?;
        self.send(event)
    }

    fn line(&mut self, s: &str) {
        self.text.push_str(s);
        self.text.push('\n');
    }

}

fn sql_error(e: rusqlite::Error) -> Error {
    match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::OperationInterrupted) => Error::Cancelled,
        _ => Error::Query(e.to_string()),
    }
}

/// A row of `sqlite_master`.
pub(super) struct Object {
    pub kind: String,
    pub name: String,
    pub table: String,
    pub sql: String,
}

/// Schema objects in creation order (`rowid` order is creation order).
pub(super) const MASTER_QUERY: &str = "select type, name, tbl_name, sql from main.sqlite_master where sql is not null order by rowid";

/// What a dump contains, shared with the Turso / libSQL dumper (same SQL, same catalog).
pub(super) struct Plan<'a> {
    pub tables: Vec<&'a Object>,
    pub views: Vec<&'a Object>,
    /// Indexes, then views, then triggers: created after the rows.
    pub post_data: Vec<&'a Object>,
    pub warnings: Vec<String>,
    /// Whether to write `sqlite_sequence` (AUTOINCREMENT counters).
    pub sequence: bool,
}

impl Plan<'_> {
    pub fn includes(&self, table: &str) -> bool {
        self.tables.iter().any(|t| t.name.eq_ignore_ascii_case(table))
    }

    /// `PRAGMA foreign_keys=OFF; BEGIN TRANSACTION;` and the optional drops.
    pub fn preamble(&self, options: &DumpOptions) -> String {
        let mut out = String::from("PRAGMA foreign_keys=OFF;\nBEGIN TRANSACTION;\n");
        if options.content.schema() && options.drop_objects {
            for view in self.views.iter().rev() {
                out.push_str(&format!("DROP VIEW IF EXISTS {};\n", SQLITE.quote_ident(&view.name)));
            }
            for table in self.tables.iter().rev() {
                out.push_str(&format!("DROP TABLE IF EXISTS {};\n", SQLITE.quote_ident(&table.name)));
            }
        }
        out
    }
}

pub(super) fn plan<'a>(objects: &'a [Object], options: &DumpOptions) -> Plan<'a> {
    let scope = &options.scope;
    let virtual_tables: Vec<&str> = objects
        .iter()
        .filter(|o| o.kind == "table" && starts_with_ignore_case(o.sql.trim_start(), "create virtual table"))
        .map(|o| o.name.as_str())
        .collect();
    let warnings = virtual_tables
        .iter()
        .filter(|name| scope.includes_table("main", name))
        .map(|name| format!("Skipped virtual table “{name}” (and its shadow tables)"))
        .collect();
    // Shadow tables of virtual tables (`fts_data`, `fts_idx`…) are recreated by their module.
    let is_shadow = |name: &str| virtual_tables.iter().any(|v| name.len() > v.len() + 1 && starts_with_ignore_case(name, &format!("{v}_")));
    let tables: Vec<&Object> = objects
        .iter()
        .filter(|o| o.kind == "table" && !o.name.starts_with("sqlite_") && !virtual_tables.contains(&o.name.as_str()))
        .filter(|o| !is_shadow(&o.name) && scope.includes_table("main", &o.name))
        .collect();
    let included = |table: &str| tables.iter().any(|t| t.name.eq_ignore_ascii_case(table));
    let views: Vec<&Object> = objects
        .iter()
        .filter(|o| o.kind == "view" && (scope.whole_schemas() || scope.includes_table("main", &o.name)))
        .collect();
    let view_included = |name: &str| views.iter().any(|v| v.name.eq_ignore_ascii_case(name));
    let indexes = objects.iter().filter(|o| o.kind == "index" && included(&o.table));
    let triggers = objects.iter().filter(|o| o.kind == "trigger" && (included(&o.table) || view_included(&o.table)));
    let post_data = indexes.chain(views.iter().copied()).chain(triggers).collect();
    let sequence = options.content.data() && scope.whole_schemas() && objects.iter().any(|o| o.name == "sqlite_sequence");
    Plan { tables, views, post_data, warnings, sequence }
}

/// `CREATE …` from `sqlite_master`, which stores statements without their `;`.
pub(super) fn statement(sql: &str) -> String {
    format!("{};\n", sql.trim_end().trim_end_matches(';'))
}

/// `INSERT INTO sqlite_sequence …` for the included tables' AUTOINCREMENT counters.
pub(super) fn sequence_rows(plan: &Plan<'_>, rows: &[(String, i64)]) -> String {
    let mut out = String::from("DELETE FROM sqlite_sequence;\n");
    for (name, seq) in rows.iter().filter(|(name, _)| plan.includes(name)) {
        out.push_str("INSERT INTO sqlite_sequence VALUES(");
        super::literal::sql_string(&mut out, name);
        out.push_str(&format!(",{seq});\n"));
    }
    out
}

/// Columns from `pragma table_xinfo` (`hidden`: 0 normal, 1 hidden, 2/3 generated, which can't be
/// inserted): the `INSERT … VALUES(` prefix and the `SELECT` reading those columns.
pub(super) fn insert_shape(table: &str, columns: &[(String, i64)]) -> Option<(String, String, usize)> {
    let ident = SQLITE.quote_ident(table);
    let insertable: Vec<String> = columns.iter().filter(|(_, hidden)| *hidden == 0).map(|(n, _)| SQLITE.quote_ident(n)).collect();
    if insertable.is_empty() {
        return None;
    }
    let prefix = if insertable.len() == columns.len() {
        format!("INSERT INTO {ident} VALUES(")
    } else {
        format!("INSERT INTO {ident}({}) VALUES(", insertable.join(","))
    };
    Some((prefix, format!("select {} from {ident}", insertable.join(",")), insertable.len()))
}

pub(super) fn table_xinfo_query(table: &str) -> String {
    format!("select name, hidden from pragma_table_xinfo({})", SQLITE.quote_literal(table))
}

fn produce(path: &Path, options: &DumpOptions, out: &mut Emitter) -> Result<()> {
    if !path.is_file() {
        return Err(Error::ConnectionFailed(format!("No database file at {}", path.display())));
    }
    let conn = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
        .map_err(|e| Error::ConnectionFailed(e.to_string()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(sql_error)?;
    // One read transaction: every table is read from the same snapshot.
    conn.execute_batch("begin").map_err(sql_error)?;

    let objects: Vec<Object> = conn
        .prepare(MASTER_QUERY)
        .and_then(|mut s| {
            s.query_map([], |r| Ok(Object { kind: r.get(0)?, name: r.get(1)?, table: r.get(2)?, sql: r.get(3)? }))?
                .collect()
        })
        .map_err(sql_error)?;
    let plan = plan(&objects, options);
    for warning in &plan.warnings {
        out.event(Event::Warning(warning.clone()))?;
    }

    let content = options.content;
    out.event(Event::TablesTotal(if content.data() { plan.tables.len() as u32 } else { 0 }))?;
    out.text.push_str(&plan.preamble(options));

    out.event(Event::Phase(DumpPhase::Schema))?;
    for table in &plan.tables {
        if content.schema() {
            out.text.push_str(&statement(&table.sql));
        }
        if content.data() {
            out.event(Event::Phase(DumpPhase::Data))?;
            dump_rows(&conn, &table.name, out)?;
        }
    }

    if plan.sequence {
        let rows: Vec<(String, i64)> = conn
            .prepare("select name, seq from main.sqlite_sequence")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .map_err(sql_error)?;
        out.text.push_str(&sequence_rows(&plan, &rows));
    }

    if content.schema() {
        out.event(Event::Phase(DumpPhase::PostData))?;
        for object in &plan.post_data {
            out.text.push_str(&statement(&object.sql));
        }
    }
    out.line("COMMIT;");
    Ok(())
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len() && text.is_char_boundary(prefix.len()) && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn dump_rows(conn: &rusqlite::Connection, table: &str, out: &mut Emitter) -> Result<()> {
    let ident = SQLITE.quote_ident(table);
    let columns: Vec<(String, i64)> = conn
        .prepare(&table_xinfo_query(table))
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(sql_error)?;
    let Some((prefix, select, width)) = insert_shape(table, &columns) else { return Ok(()) };

    let total: Option<u64> = conn.query_row(&format!("select count(*) from {ident}"), [], |r| r.get::<_, i64>(0)).ok().map(|n| n as u64);
    out.event(Event::Table { name: table.to_string(), rows: total })?;

    let mut statement = conn.prepare(&select).map_err(sql_error)?;
    let mut rows = statement.query([]).map_err(sql_error)?;
    let mut pending = 0u64;
    while let Some(row) = rows.next().map_err(sql_error)? {
        out.text.push_str(&prefix);
        for i in 0..width {
            if i > 0 {
                out.text.push(',');
            }
            match row.get_ref(i).map_err(sql_error)? {
                ValueRef::Null => out.text.push_str("NULL"),
                ValueRef::Integer(n) => out.text.push_str(&n.to_string()),
                ValueRef::Real(f) => out.text.push_str(&sqlite_float(f)),
                ValueRef::Text(t) => sqlite_text(&mut out.text, t),
                ValueRef::Blob(b) => sqlite_blob(&mut out.text, b),
            }
        }
        out.text.push_str(");\n");
        pending += 1;
        if out.text.len() >= PIECE {
            out.event(Event::Rows(pending))?;
            pending = 0;
        }
    }
    out.event(Event::Rows(pending))?;
    out.event(Event::EndTable)
}
