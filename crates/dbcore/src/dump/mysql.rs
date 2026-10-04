//! MySQL/MariaDB dumps, like `mysqldump --single-transaction --hex-blob --routines --triggers`
//! of one database: tables (`SHOW CREATE TABLE`) with extended INSERTs, then views, routines
//! and triggers. Names aren't qualified with the database, so a dump restores into any database.
//! `DEFINER=` clauses are dropped, so restoring doesn't need the original users.

use std::sync::LazyLock;

use mysql_async::consts::ColumnType;
use mysql_async::prelude::Queryable;
use mysql_async::{Column, Conn, Row};
use regex::Regex;

use super::literal::{mysql_hex, mysql_string};
use super::order::topo_sort;
use super::{Ctx, DumpPhase};
use crate::dialect::Dialect;
use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind};
use crate::mysql::query_error;

const MYSQL: Dialect = Dialect(DatabaseKind::Mysql);
/// Character set number of binary strings (BLOB, VARBINARY…).
const BINARY_CHARSET: u16 = 63;
/// Extended INSERTs are cut at about this size (mysqldump's `net_buffer_length` default is 1 MB).
const MAX_INSERT: usize = 1024 * 1024;
/// Engines with transactions, read consistently by `START TRANSACTION WITH CONSISTENT SNAPSHOT`.
const TRANSACTIONAL: &[&str] = &["innodb", "ndbcluster", "ndb", "rocksdb", "tokudb"];

fn ident(name: &str) -> String {
    MYSQL.quote_ident(name)
}

fn err(e: mysql_async::Error) -> Error {
    query_error(&e)
}

static DEFINER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+DEFINER\s*=\s*(`(?:[^`]|``)*`|'(?:[^']|'')*'|[^\s@]+)@(`(?:[^`]|``)*`|'(?:[^']|'')*'|\S+)").unwrap());

/// `CREATE DEFINER=`root`@`%` VIEW …` → `CREATE VIEW …`.
fn strip_definer(sql: &str) -> String {
    DEFINER.replace(sql, "").into_owned()
}

struct Table {
    name: String,
    is_view: bool,
    engine: Option<String>,
    rows: Option<u64>,
}

pub(super) async fn dump(config: &ConnectionConfig, ctx: &mut Ctx) -> Result<()> {
    let database = config.database.trim().to_string();
    if database.is_empty() {
        return Err(Error::InvalidConfig("Choose a database to dump".into()));
    }
    let mut conn = crate::mysql::connect(config).await?;
    let server: String = conn.query_first("select version()").await.map_err(err)?.unwrap_or_default();
    conn.query_drop(
        "set session time_zone = '+00:00';
         set session sql_mode = '';
         set session sql_quote_show_create = 1;
         set session net_write_timeout = 3600;
         set session transaction isolation level repeatable read;
         start transaction with consistent snapshot, read only",
    )
    .await
    .map_err(err)?;

    super::header(ctx, config, &server);
    ctx.line("SET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0;");
    ctx.line("SET @OLD_UNIQUE_CHECKS=@@UNIQUE_CHECKS, UNIQUE_CHECKS=0;");
    ctx.line("SET @OLD_SQL_MODE=@@SQL_MODE, SQL_MODE='NO_AUTO_VALUE_ON_ZERO';");
    ctx.line("SET @OLD_TIME_ZONE=@@TIME_ZONE, TIME_ZONE='+00:00';");
    ctx.line("SET NAMES utf8mb4;");
    ctx.line("");

    let options = ctx.options.clone();
    let content = options.content;
    let scope = &options.scope;
    if options.create_database && content.schema() {
        let row: Option<(String, String)> =
            conn.query_first(format!("show create database if not exists {}", ident(&database))).await.map_err(err)?;
        if let Some((_, create)) = row {
            ctx.line(&format!("{create};"));
        }
        ctx.line(&format!("USE {};", ident(&database)));
        ctx.line("");
    }

    ctx.phase(DumpPhase::Schema);
    let tables: Vec<Table> = conn
        .query::<(String, String, Option<String>, Option<u64>), _>(
            "select table_name, table_type, engine, table_rows from information_schema.tables
             where table_schema = database() order by table_name",
        )
        .await
        .map_err(err)?
        .into_iter()
        .filter(|(name, ..)| scope.includes_table(&database, name))
        .map(|(name, kind, engine, rows)| Table { name, is_view: kind == "VIEW", engine, rows })
        .collect();

    let non_transactional: Vec<&str> = tables
        .iter()
        .filter(|t| !t.is_view && t.engine.as_deref().is_some_and(|e| !TRANSACTIONAL.contains(&e.to_lowercase().as_str())))
        .map(|t| t.name.as_str())
        .collect();
    if !non_transactional.is_empty() && content.data() {
        let warning = format!(
            "Tables without transactions were dumped without a consistent snapshot: {}",
            non_transactional.join(", ")
        );
        ctx.line(&format!("-- Warning: {warning}"));
        ctx.warn(warning);
    }

    let base_tables: Vec<&Table> = tables.iter().filter(|t| !t.is_view).collect();
    ctx.set_tables_total(if content.data() { base_tables.len() as u32 } else { 0 });
    for table in &base_tables {
        if content.schema() {
            let (_, create): (String, String) = conn
                .query_first(format!("show create table {}", ident(&table.name)))
                .await
                .map_err(err)?
                .ok_or_else(|| Error::TableNotFound(table.name.clone()))?;
            if options.drop_objects {
                ctx.line(&format!("DROP TABLE IF EXISTS {};", ident(&table.name)));
            }
            ctx.line(&format!("{create};"));
            ctx.line("");
        }
        if content.data() {
            ctx.phase(DumpPhase::Data);
            ctx.begin_table(table.name.clone(), table.rows);
            insert_rows(&mut conn, ctx, &table.name).await?;
            ctx.end_table();
        }
    }

    if content.schema() {
        ctx.phase(DumpPhase::PostData);
        write_views(&mut conn, ctx, &tables).await?;
        if scope.whole_schemas() {
            write_routines(&mut conn, ctx).await?;
        }
        write_triggers(&mut conn, ctx, &base_tables).await?;
        if scope.whole_schemas() {
            let events: Option<u64> =
                conn.query_first("select count(*) from information_schema.events where event_schema = database()").await.map_err(err)?;
            if let Some(n @ 1..) = events {
                ctx.warn(format!("Skipped {n} scheduled event{}", if n == 1 { "" } else { "s" }));
            }
        }
    }

    ctx.line("SET TIME_ZONE=@OLD_TIME_ZONE;");
    ctx.line("SET SQL_MODE=@OLD_SQL_MODE;");
    ctx.line("SET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS;");
    ctx.line("SET UNIQUE_CHECKS=@OLD_UNIQUE_CHECKS;");
    conn.query_drop("commit").await.map_err(err)?;
    let _ = conn.disconnect().await;
    Ok(())
}

async fn insert_rows(conn: &mut Conn, ctx: &mut Ctx, table: &str) -> Result<()> {
    // Generated columns can't be inserted into.
    let columns: Vec<String> = conn
        .query::<(String, String), _>(format!(
            "select column_name, extra from information_schema.columns
             where table_schema = database() and table_name = {} order by ordinal_position",
            MYSQL.quote_literal(table)
        ))
        .await
        .map_err(err)?
        .into_iter()
        .filter(|(_, extra)| {
            let extra = extra.to_uppercase();
            !extra.contains("VIRTUAL GENERATED") && !extra.contains("STORED GENERATED")
        })
        .map(|(name, _)| name)
        .collect();
    if columns.is_empty() {
        return Ok(());
    }
    let list = columns.iter().map(|c| ident(c)).collect::<Vec<_>>().join(",");
    let prefix = format!("INSERT INTO {} ({list}) VALUES ", ident(table));

    let mut result = conn.query_iter(format!("select {list} from {}", ident(table))).await.map_err(err)?;
    let meta = result.columns().unwrap_or_else(|| Vec::new().into());
    let mut statement = String::with_capacity(MAX_INSERT + 64 * 1024);
    let mut pending = 0u64;
    while let Some(row) = result.next().await.map_err(err)? {
        statement.push_str(if pending == 0 { &prefix } else { "," });
        write_row(&mut statement, row, &meta);
        pending += 1;
        if statement.len() >= MAX_INSERT {
            statement.push_str(";\n");
            ctx.push(&statement);
            statement.clear();
            ctx.rows(pending);
            pending = 0;
            ctx.tick().await?;
        }
    }
    if pending > 0 {
        statement.push_str(";\n");
        ctx.push(&statement);
        ctx.rows(pending);
    }
    ctx.line("");
    Ok(())
}

fn write_row(out: &mut String, row: Row, columns: &[Column]) {
    out.push('(');
    for (i, (value, column)) in row.unwrap().into_iter().zip(columns).enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_value(out, value, column);
    }
    out.push(')');
}

/// A text-protocol value as a literal: numbers bare, binary data as hex, everything else quoted.
fn write_value(out: &mut String, value: mysql_async::Value, column: &Column) {
    use mysql_async::Value as V;
    let bytes = match value {
        V::NULL => return out.push_str("NULL"),
        V::Bytes(bytes) => bytes,
        other => return out.push_str(&other.as_sql(true)),
    };
    match column.column_type() {
        ColumnType::MYSQL_TYPE_TINY
        | ColumnType::MYSQL_TYPE_SHORT
        | ColumnType::MYSQL_TYPE_INT24
        | ColumnType::MYSQL_TYPE_LONG
        | ColumnType::MYSQL_TYPE_LONGLONG
        | ColumnType::MYSQL_TYPE_YEAR
        | ColumnType::MYSQL_TYPE_DECIMAL
        | ColumnType::MYSQL_TYPE_NEWDECIMAL
        | ColumnType::MYSQL_TYPE_FLOAT
        | ColumnType::MYSQL_TYPE_DOUBLE
            if bytes.iter().all(|b| b.is_ascii_digit() || b"+-.eE".contains(b)) && !bytes.is_empty() =>
        {
            out.push_str(std::str::from_utf8(&bytes).unwrap_or("0"));
        }
        ColumnType::MYSQL_TYPE_BIT | ColumnType::MYSQL_TYPE_GEOMETRY => mysql_hex(out, &bytes),
        // Binary strings and BLOBs (dates and numbers also report the binary charset: not those).
        ColumnType::MYSQL_TYPE_STRING
        | ColumnType::MYSQL_TYPE_VAR_STRING
        | ColumnType::MYSQL_TYPE_VARCHAR
        | ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_BLOB
            if column.character_set() == BINARY_CHARSET =>
        {
            mysql_hex(out, &bytes)
        }
        _ => match std::str::from_utf8(&bytes) {
            Ok(text) => mysql_string(out, text),
            // Not valid in the connection's utf8mb4: keep the exact bytes.
            Err(_) => mysql_hex(out, &bytes),
        },
    }
}

async fn write_views(conn: &mut Conn, ctx: &mut Ctx, tables: &[Table]) -> Result<()> {
    let views: Vec<String> = tables.iter().filter(|t| t.is_view).map(|t| t.name.clone()).collect();
    if views.is_empty() {
        return Ok(());
    }
    // VIEW_TABLE_USAGE needs MySQL 8.0.13+; without it, views keep their name order.
    let deps: Vec<(String, String)> = conn
        .query("select view_name, table_name from information_schema.view_table_usage where view_schema = database()")
        .await
        .unwrap_or_default();
    for view in topo_sort(&views, &deps) {
        // Without the database's own views qualified, it restores anywhere: `SHOW CREATE VIEW`
        // leaves out the current database's name.
        let row: Option<Row> = conn.query_first(format!("show create view {}", ident(&view))).await.map_err(err)?;
        let Some(create) = row.and_then(|r| r.get_opt::<String, _>(1).and_then(|v| v.ok())) else {
            ctx.warn(format!("Skipped view {view}: no permission to read its definition"));
            continue;
        };
        if ctx.options.drop_objects {
            ctx.line(&format!("DROP VIEW IF EXISTS {};", ident(&view)));
        }
        ctx.line(&format!("{};", strip_definer(&create)));
        ctx.line("");
    }
    Ok(())
}

/// A routine or trigger body contains `;`: wrapped in `DELIMITER ;;` like mysqldump, with the
/// sql_mode it was created with.
fn write_compound(ctx: &mut Ctx, sql_mode: &str, create: &str) {
    ctx.line(&format!("SET SESSION sql_mode = {};", MYSQL.quote_literal(sql_mode)));
    ctx.line("DELIMITER ;;");
    ctx.line(&format!("{} ;;", strip_definer(create)));
    ctx.line("DELIMITER ;");
    ctx.line("SET SESSION sql_mode = 'NO_AUTO_VALUE_ON_ZERO';");
    ctx.line("");
}

async fn write_routines(conn: &mut Conn, ctx: &mut Ctx) -> Result<()> {
    let routines: Vec<(String, String)> = conn
        .query(
            "select routine_name, routine_type from information_schema.routines
             where routine_schema = database() order by routine_type, routine_name",
        )
        .await
        .map_err(err)?;
    for (name, kind) in routines {
        let row: Option<Row> = conn.query_first(format!("show create {kind} {}", ident(&name))).await.map_err(err)?;
        let parts = row.map(|r| (r.get_opt::<String, _>(1).and_then(|v| v.ok()), r.get_opt::<String, _>(2).and_then(|v| v.ok())));
        let Some((Some(sql_mode), Some(create))) = parts else {
            ctx.warn(format!("Skipped {} {name}: no permission to read its definition", kind.to_lowercase()));
            continue;
        };
        if ctx.options.drop_objects {
            ctx.line(&format!("DROP {kind} IF EXISTS {};", ident(&name)));
        }
        write_compound(ctx, &sql_mode, &create);
    }
    Ok(())
}

async fn write_triggers(conn: &mut Conn, ctx: &mut Ctx, tables: &[&Table]) -> Result<()> {
    let triggers: Vec<(String, String)> = conn
        .query(
            "select trigger_name, event_object_table from information_schema.triggers
             where trigger_schema = database() order by event_object_table, action_timing, event_manipulation, action_order",
        )
        .await
        .map_err(err)?;
    for (name, table) in triggers {
        if !tables.iter().any(|t| t.name == table) {
            continue;
        }
        let row: Option<Row> = conn.query_first(format!("show create trigger {}", ident(&name))).await.map_err(err)?;
        let parts = row.map(|r| (r.get_opt::<String, _>(1).and_then(|v| v.ok()), r.get_opt::<String, _>(2).and_then(|v| v.ok())));
        let Some((Some(sql_mode), Some(create))) = parts else {
            ctx.warn(format!("Skipped trigger {name}: no permission to read its definition"));
            continue;
        };
        if ctx.options.drop_objects {
            ctx.line(&format!("DROP TRIGGER IF EXISTS {};", ident(&name)));
        }
        write_compound(ctx, &sql_mode, &create);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_definers() {
        assert_eq!(
            strip_definer("CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`%` SQL SECURITY DEFINER VIEW `v` AS select 1"),
            "CREATE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `v` AS select 1"
        );
        assert_eq!(
            strip_definer("CREATE DEFINER=`we``ird`@`localhost` PROCEDURE `p`() BEGIN END"),
            "CREATE PROCEDURE `p`() BEGIN END"
        );
        assert_eq!(strip_definer("CREATE DEFINER=root@localhost TRIGGER t"), "CREATE TRIGGER t");
    }
}
