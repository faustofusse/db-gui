//! MySQL dump → restore round trips against the dev server (`scripts/dev-db.sh up mysql`).
//! Skipped unless `DBEAR_TEST_MYSQL=1`. Each test works in scratch databases of its own
//! (`dbear_dump_*`), created and dropped on the shared dev server.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use dbcore::dump::{self, CancelToken, Compression, DumpContent, DumpOptions, DumpProgress, DumpScope};
use dbcore::restore::{self, RestoreOptions, RestoreProgress};
use dbcore::{mock, Connection, ConnectionConfig, TableInfo};

const CONTAINER: &str = "dbear-mysql";

fn enabled() -> bool {
    let on = std::env::var("DBEAR_TEST_MYSQL").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set DBEAR_TEST_MYSQL=1 (scripts/dev-db.sh up mysql)");
    }
    on
}

fn dev_config() -> ConnectionConfig {
    mock::connections().into_iter().find(|c| c.id == mock::DEV_MYSQL).unwrap()
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
        let admin = Connection::new(dev_config());
        block_on(admin.execute(format!("drop database if exists {name}; create database {name}"))).unwrap();
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
        let admin = Connection::new(dev_config());
        let _ = block_on(admin.execute(format!("drop database if exists {}", self.name)));
        block_on(admin.disconnect());
    }
}

fn fixture() -> Scratch {
    let db = Scratch::new("src");
    db.execute(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/dump/mysql_fixture.sql")).unwrap());
    db
}

fn run_dump(config: ConnectionConfig, out: &Path, options: DumpOptions) -> dump::DumpSummary {
    block_on(dump::dump(config, out.to_path_buf(), options, Arc::new(|_: &DumpProgress| {}), CancelToken::new())).unwrap()
}

fn run_restore(config: ConnectionConfig, script: &Path) -> dbcore::Result<restore::RestoreSummary> {
    let options = RestoreOptions { single_transaction: false, stop_on_error: true };
    block_on(restore::restore(config, script.to_path_buf(), options, Arc::new(|_: &RestoreProgress| {}), CancelToken::new()))
}

/// Definitions (with the database name taken out) and per-table checksums.
fn fingerprint(db: &ConnectionConfig) -> Vec<String> {
    let conn = Connection::new(db.clone());
    let catalog = "
        select concat_ws(' ', 'col', table_name, column_name, column_type, is_nullable, coalesce(column_default, '-'), extra,
                         generation_expression, coalesce(collation_name, '-'), column_comment)
          from information_schema.columns where table_schema = database()
        union all select concat_ws(' ', 'table', table_name, table_type, coalesce(engine, '-'), coalesce(auto_increment, '-'), table_comment)
          from information_schema.tables where table_schema = database()
        union all select concat_ws(' ', 'idx', table_name, index_name, seq_in_index, coalesce(column_name, '-'), non_unique,
                         index_type, coalesce(sub_part, '-'), coalesce(expression, '-'), collation)
          from information_schema.statistics where table_schema = database()
        union all select concat_ws(' ', 'fk', table_name, constraint_name, referenced_table_name, update_rule, delete_rule)
          from information_schema.referential_constraints where constraint_schema = database()
        union all select concat_ws(' ', 'view', table_name, replace(view_definition, concat('`', database(), '`.'), ''), security_type)
          from information_schema.views where table_schema = database()
        union all select concat_ws(' ', 'routine', routine_type, routine_name, routine_definition, is_deterministic, sql_data_access)
          from information_schema.routines where routine_schema = database()
        union all select concat_ws(' ', 'trigger', trigger_name, event_object_table, action_timing, event_manipulation, action_statement)
          from information_schema.triggers where trigger_schema = database()
        order by 1";
    let mut lines: Vec<String> = block_on(conn.execute(catalog.into())).unwrap().rows.iter().map(|r| r[0].display()).collect();
    let tables = block_on(conn.execute(
        "select table_name from information_schema.tables where table_schema = database() and table_type = 'BASE TABLE' order by 1".into(),
    ))
    .unwrap();
    for table in tables.rows.iter().map(|r| r[0].display()) {
        let quoted = format!("`{}`", table.replace('`', "``"));
        let sum = block_on(conn.execute(format!("checksum table {quoted}"))).unwrap();
        let count = block_on(conn.execute(format!("select count(*) from {quoted}"))).unwrap();
        lines.push(format!("data {table} {} {}", count.rows[0][0].display(), sum.rows[0][1].display()));
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
    assert!(summary.rows >= 500, "{summary:?}");
    // The MyISAM table can't be read in the snapshot.
    assert!(summary.warnings.iter().any(|w| w.contains("logs")), "{summary:?}");
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.contains("'2024-02-29 12:34:56.123456'") && text.contains("0xDEADBEEF".to_lowercase().as_str()), "dates quoted, binary as hex");
    assert!(!text.contains("DEFINER=") && !text.contains(&format!("`{}`", source.name)), "definers and the database name are left out");

    let target = Scratch::new("dst");
    run_restore(target.config(), &script).unwrap();
    assert_same(&fingerprint(&source.config()), &fingerprint(&target.config()));
}

#[test]
fn round_trips_gzipped_with_drops_and_create_database() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dump.sql.gz");
    let options = DumpOptions { compression: Compression::Gzip, drop_objects: true, ..Default::default() };
    run_dump(source.config(), &script, options);
    let target = Scratch::new("gz");
    run_restore(target.config(), &script).unwrap();
    run_restore(target.config(), &script).unwrap();
    assert_same(&fingerprint(&source.config()), &fingerprint(&target.config()));

    // CREATE DATABASE + USE: restores into the original name from any connection.
    let named = dir.path().join("named.sql");
    run_dump(source.config(), &named, DumpOptions { create_database: true, content: DumpContent::SchemaOnly, ..Default::default() });
    let text = std::fs::read_to_string(&named).unwrap();
    assert!(text.contains(&format!("CREATE DATABASE /*!32312 IF NOT EXISTS*/ `{}`", source.name)), "{text}");
    assert!(text.contains(&format!("USE `{}`;", source.name)));
}

#[test]
fn dumps_selected_tables() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("orders.sql");
    let scope = DumpScope::Tables(vec![TableInfo::new(&source.name, "customers"), TableInfo::new(&source.name, "orders")]);
    let summary = run_dump(source.config(), &script, DumpOptions { scope, ..Default::default() });
    assert_eq!(summary.tables, 2);
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.contains("orders_note") && !text.contains("add_log") && !text.contains("order_totals"), "{text}");
    let target = Scratch::new("sel");
    run_restore(target.config(), &script).unwrap();
}

fn in_container(args: &[&str], stdin: Option<&Path>) -> Option<std::process::Output> {
    let mut command = Command::new("container");
    command.arg("exec").args(if stdin.is_some() { vec!["-i"] } else { vec![] }).arg(CONTAINER).args(args);
    if let Some(path) = stdin {
        command.stdin(Stdio::from(std::fs::File::open(path).unwrap()));
    }
    command.output().ok()
}

#[test]
fn compatible_with_the_mysql_client_and_mysqldump() {
    if !enabled() {
        return;
    }
    if in_container(&["true"], None).is_none_or(|o| !o.status.success()) {
        eprintln!("skipped: can't `container exec` into {CONTAINER}");
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();

    let script = dir.path().join("dump.sql");
    run_dump(source.config(), &script, DumpOptions::default());
    let by_client = Scratch::new("cli");
    let output = in_container(&["mysql", "-uroot", "-pmysql", &by_client.name], Some(&script)).unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_same(&fingerprint(&source.config()), &fingerprint(&by_client.config()));

    let output = in_container(
        &["mysqldump", "-uroot", "-pmysql", "--single-transaction", "--routines", "--triggers", "--hex-blob", &source.name],
        None,
    )
    .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let theirs = dir.path().join("mysqldump.sql");
    std::fs::write(&theirs, &output.stdout).unwrap();
    let from_mysqldump = Scratch::new("md");
    run_restore(from_mysqldump.config(), &theirs).unwrap();
    assert_same(&fingerprint(&source.config()), &fingerprint(&from_mysqldump.config()));
}
