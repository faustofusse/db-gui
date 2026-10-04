//! SQL Server dump → restore round trips against the dev server (`scripts/dev-db.sh up sqlserver`).
//! Skipped unless `DBEAR_TEST_SQLSERVER=1`. Each test works in scratch databases of its own
//! (`dbear_dump_*`), created and dropped on the dev server; `app_dev` is only read.

use std::path::Path;
use std::sync::Arc;

use dbcore::dump::{self, CancelToken, Compression, DumpContent, DumpOptions, DumpProgress, DumpScope};
use dbcore::restore::{self, RestoreOptions, RestoreProgress};
use dbcore::{mock, Connection, ConnectionConfig, Error, TableInfo};

fn enabled() -> bool {
    let on = std::env::var("DBEAR_TEST_SQLSERVER").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set DBEAR_TEST_SQLSERVER=1 (scripts/dev-db.sh up sqlserver)");
    }
    on
}

fn dev_config() -> ConnectionConfig {
    mock::connections().into_iter().find(|c| c.id == mock::DEV_SQLSERVER).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

struct Scratch {
    name: String,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("dbear_dump_{label}_{}_{n}", std::process::id());
        let admin = Connection::new(dev_config().with_database("master"));
        block_on(admin.execute(format!("if db_id(N'{name}') is not null drop database [{name}]; create database [{name}]"))).unwrap();
        block_on(admin.disconnect());
        Self { name }
    }

    fn config(&self) -> ConnectionConfig {
        dev_config().with_database(&self.name)
    }

    fn execute(&self, sql: &str) {
        let conn = Connection::new(self.config());
        block_on(conn.execute(sql.to_string())).unwrap();
        block_on(conn.disconnect());
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let admin = Connection::new(dev_config().with_database("master"));
        let _ = block_on(admin.execute(format!(
            "if db_id(N'{0}') is not null begin alter database [{0}] set single_user with rollback immediate; drop database [{0}]; end",
            self.name
        )));
        block_on(admin.disconnect());
    }
}

fn fixture() -> Scratch {
    let db = Scratch::new("src");
    db.execute(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/dump/sqlserver_fixture.sql")).unwrap());
    db
}

fn run_dump(config: ConnectionConfig, out: &Path, options: DumpOptions) -> dump::DumpSummary {
    block_on(dump::dump(config, out.to_path_buf(), options, Arc::new(|_: &DumpProgress| {}), CancelToken::new())).unwrap()
}

fn run_restore(config: ConnectionConfig, script: &Path, options: RestoreOptions) -> dbcore::Result<restore::RestoreSummary> {
    block_on(restore::restore(config, script.to_path_buf(), options, Arc::new(|_: &RestoreProgress| {}), CancelToken::new()))
}

/// Definitions and per-table checksums (rowversion left out: the server assigns it).
fn fingerprint(config: ConnectionConfig) -> Vec<String> {
    let conn = Connection::new(config);
    // Catalog columns mix collations: compare every part in one binary collation.
    let cat = |parts: &[&str]| {
        let parts: Vec<String> =
            parts.iter().map(|p| format!("isnull(cast({p} as nvarchar(max)) collate Latin1_General_BIN2, N'-')")).collect();
        format!("concat_ws(N' ', {})", parts.join(", "))
    };
    let catalog = [
        format!(
            "select {} from sys.columns c join sys.objects o on o.object_id = c.object_id
             left join sys.identity_columns ic on ic.object_id = c.object_id and ic.column_id = c.column_id
             left join sys.computed_columns cc on cc.object_id = c.object_id and cc.column_id = c.column_id
             left join sys.default_constraints dc on dc.object_id = c.default_object_id
             where o.is_ms_shipped = 0 and o.type in ('U', 'V')",
            cat(&["'col'", "object_schema_name(c.object_id)", "object_name(c.object_id)", "c.name", "type_name(c.user_type_id)",
                  "c.max_length", "c.precision", "c.scale", "c.is_nullable", "c.is_identity", "c.collation_name", "ic.seed_value",
                  "ic.increment_value", "ic.last_value", "cc.definition", "cc.is_persisted", "dc.name", "dc.definition"])
        ),
        format!(
            "select {} from sys.indexes i join sys.objects o on o.object_id = i.object_id where o.is_ms_shipped = 0 and i.type > 0",
            cat(&["'idx'", "object_schema_name(i.object_id)", "object_name(i.object_id)", "i.name", "i.type_desc", "i.is_unique",
                  "i.is_primary_key", "i.is_unique_constraint", "i.filter_definition",
                  "(select string_agg(concat(col_name(ic.object_id, ic.column_id), ic.is_descending_key, ic.is_included_column), ',')
                           within group (order by ic.is_included_column, ic.key_ordinal, ic.index_column_id)
                    from sys.index_columns ic where ic.object_id = i.object_id and ic.index_id = i.index_id)"])
        ),
        format!("select {} from sys.check_constraints", cat(&["'check'", "object_name(parent_object_id)", "name", "definition", "is_disabled"])),
        format!(
            "select {} from sys.foreign_keys",
            cat(&["'fk'", "object_name(parent_object_id)", "name", "object_name(referenced_object_id)", "delete_referential_action_desc",
                  "update_referential_action_desc", "is_disabled", "is_not_trusted"])
        ),
        format!(
            "select {} from sys.sql_modules m join sys.objects o on o.object_id = m.object_id where o.is_ms_shipped = 0",
            cat(&["'module'", "object_schema_name(m.object_id)", "object_name(m.object_id)", "m.definition"])
        ),
        format!("select {} from sys.schemas where schema_id between 5 and 16383", cat(&["'schema'", "name"])),
    ]
    .join(" union all ");
    // Sorted here: the catalog's columns mix collations, which ORDER BY can't compare.
    let mut lines: Vec<String> = block_on(conn.execute(catalog)).unwrap().rows.iter().map(|r| r[0].display()).collect();
    lines.sort();
    let tables = block_on(conn.execute(
        "select concat(quotename(schema_name(t.schema_id)), '.', quotename(t.name)),
                (select string_agg(case when type_name(c.system_type_id) = 'xml' then concat('cast(', quotename(c.name), ' as nvarchar(max))')
                                        else quotename(c.name) end, ', ')
                 from sys.columns c where c.object_id = t.object_id and type_name(c.system_type_id) <> 'timestamp')
         from sys.tables t where t.is_ms_shipped = 0 order by 1"
            .into(),
    ))
    .unwrap();
    for row in &tables.rows {
        let (table, columns) = (row[0].display(), row[1].display());
        let sums = block_on(conn.execute(format!(
            "select concat(count_big(*), ' ', checksum_agg(binary_checksum({columns}))) from {table}"
        )))
        .unwrap();
        lines.push(format!("data {table} {}", sums.rows[0][0].display()));
    }
    block_on(conn.disconnect());
    lines
}

fn assert_same(left: &[String], right: &[String]) {
    let only_left: Vec<_> = left.iter().filter(|l| !right.contains(l)).collect();
    let only_right: Vec<_> = right.iter().filter(|r| !left.contains(r)).collect();
    assert!(only_left.is_empty() && only_right.is_empty(), "source only: {only_left:#?}\nrestored only: {only_right:#?}");
}

#[test]
fn round_trips_the_fixture() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dump.sql");
    let summary = run_dump(source.config(), &script, DumpOptions::default());
    assert!(summary.rows >= 495, "{summary:?}");
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.contains("SET IDENTITY_INSERT [sales].[customers] ON;") && text.contains("\nGO\n"), "{text}");
    assert!(text.contains("-- Not included:"));

    let target = Scratch::new("dst");
    let restored = run_restore(target.config(), &script, RestoreOptions::default()).unwrap();
    assert!(restored.errors.is_empty(), "{restored:?}");
    assert_same(&fingerprint(source.config()), &fingerprint(target.config()));
}

#[test]
fn round_trips_gzipped_with_drops() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dump.sql.gz");
    run_dump(source.config(), &script, DumpOptions { compression: Compression::Gzip, drop_objects: true, ..Default::default() });
    let target = Scratch::new("gz");
    run_restore(target.config(), &script, RestoreOptions::default()).unwrap();
    // The drops make it restorable over itself.
    run_restore(target.config(), &script, RestoreOptions::default()).unwrap();
    assert_same(&fingerprint(source.config()), &fingerprint(target.config()));
}

#[test]
fn schema_only_then_data_only_and_selected_tables() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let (schema, data) = (dir.path().join("schema.sql"), dir.path().join("data.sql"));
    run_dump(source.config(), &schema, DumpOptions { content: DumpContent::SchemaOnly, ..Default::default() });
    run_dump(source.config(), &data, DumpOptions { content: DumpContent::DataOnly, ..Default::default() });
    let target = Scratch::new("split");
    run_restore(target.config(), &schema, RestoreOptions::default()).unwrap();
    // Foreign keys exist already: load parents first (data tables are written in name order).
    target.execute("alter table sales.orders nocheck constraint all; disable trigger sales.orders_note on sales.orders");
    run_restore(target.config(), &data, RestoreOptions::default()).unwrap();
    target.execute("alter table sales.orders with check check constraint all; enable trigger sales.orders_note on sales.orders");
    assert_same(&fingerprint(source.config()), &fingerprint(target.config()));

    let one = dir.path().join("orders.sql");
    let scope = DumpScope::Tables(vec![TableInfo::new("sales", "orders"), TableInfo::new("sales", "customers")]);
    let summary = run_dump(source.config(), &one, DumpOptions { scope, ..Default::default() });
    assert_eq!(summary.tables, 2);
    let text = std::fs::read_to_string(&one).unwrap();
    assert!(text.contains("orders_note") && !text.contains("add_order") && !text.contains("order_totals"), "{text}");
}

#[test]
fn round_trips_the_dev_database() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("app_dev.sql");
    run_dump(dev_config(), &script, DumpOptions::default());
    let target = Scratch::new("appdev");
    run_restore(target.config(), &script, RestoreOptions::default()).unwrap();
    assert_same(&fingerprint(dev_config()), &fingerprint(target.config()));
}

#[test]
fn reports_errors_with_script_lines() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("bad.sql");
    std::fs::write(&script, "create table a (x int);\nGO\n-- comment\ninsert into a values (1);\ninsert into nope values (2);\nGO\n").unwrap();
    let target = Scratch::new("bad");
    let err = run_restore(target.config(), &script, RestoreOptions::default()).unwrap_err();
    assert!(matches!(&err, Error::Query(m) if m.contains("Line 5") && m.contains("nope")), "{err:?}");

    let options = RestoreOptions { single_transaction: false, stop_on_error: false };
    let summary = run_restore(target.config(), &script, options).unwrap();
    assert_eq!((summary.statements, summary.error_count), (1, 1), "{summary:?}");
}
