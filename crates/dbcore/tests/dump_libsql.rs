//! Turso / libSQL dump → restore round trips against the dev server (`scripts/dev-db.sh up libsql`).
//! Skipped unless `DBEAR_TEST_LIBSQL=1`. The server holds one shared database: the seed tables are
//! only read, and restores go into scratch tables of this test run (`dump_rt_<pid>_<test>_*`), dropped after.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dbcore::dump::{self, CancelToken, Compression, DumpOptions, DumpProgress, DumpScope};
use dbcore::restore::{self, RestoreOptions, RestoreProgress};
use dbcore::{mock, Connection, ConnectionConfig, DatabaseKind, Error, TableInfo, Value};

fn enabled() -> bool {
    let on = std::env::var("DBEAR_TEST_LIBSQL").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set DBEAR_TEST_LIBSQL=1 (scripts/dev-db.sh up libsql)");
    }
    on
}

fn dev_config() -> ConnectionConfig {
    mock::connections().into_iter().find(|c| c.id == mock::DEV_LIBSQL).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn sqlite_config(path: &Path) -> ConnectionConfig {
    let mut config = ConnectionConfig::new_empty(DatabaseKind::Sqlite);
    config.id = "dump-test".into();
    config.database = path.display().to_string();
    config
}

fn empty_db(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::File::create(&path).unwrap();
    path
}

fn run_dump(config: ConnectionConfig, out: &Path, options: DumpOptions) -> dump::DumpSummary {
    block_on(dump::dump(config, out.to_path_buf(), options, Arc::new(|_: &DumpProgress| {}), CancelToken::new())).unwrap()
}

fn run_restore(config: ConnectionConfig, script: &Path, options: RestoreOptions) -> dbcore::Result<restore::RestoreSummary> {
    block_on(restore::restore(config, script.to_path_buf(), options, Arc::new(|_: &RestoreProgress| {}), CancelToken::new()))
}

/// Rows of `table` (sorted), and its `CREATE` statement, through the core's own driver.
fn table_snapshot(config: &ConnectionConfig, table: &str) -> (String, Vec<String>) {
    let conn = Connection::new(config.clone());
    let quoted = format!("\"{}\"", table.replace('"', "\"\""));
    let ddl = block_on(conn.execute(format!("select sql from sqlite_master where name = '{}'", table.replace('\'', "''")))).unwrap();
    let rows = block_on(conn.execute(format!("select * from {quoted}"))).unwrap();
    let mut rows: Vec<String> = rows.rows.iter().map(|r| format!("{:?}", r.iter().collect::<Vec<&Value>>())).collect();
    rows.sort();
    (ddl.rows.first().map(|r| r[0].display()).unwrap_or_default(), rows)
}

const SEED_TABLES: &[&str] = &["notes", "tags", "note_tags", "settings", "events"];

#[test]
fn dumps_the_seed_into_a_local_sqlite_file() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("libsql.sql.gz");
    let scope = DumpScope::Tables(SEED_TABLES.iter().map(|t| TableInfo::new("main", *t)).collect());
    let events = Arc::new(std::sync::Mutex::new(Vec::<DumpProgress>::new()));
    let seen = events.clone();
    let options = DumpOptions { scope, compression: Compression::Gzip, ..Default::default() };
    let summary = block_on(dump::dump(
        dev_config(),
        script.clone(),
        options,
        Arc::new(move |p: &DumpProgress| seen.lock().unwrap().push(p.clone())),
        CancelToken::new(),
    ))
    .unwrap();
    assert_eq!(summary.tables, SEED_TABLES.len() as u32, "{summary:?}");
    // No row counts on Turso: progress per table is indeterminate.
    assert!(events.lock().unwrap().iter().all(|e| e.table_rows_estimate.is_none()));

    let local = empty_db(dir.path(), "local.db");
    run_restore(sqlite_config(&local), &script, RestoreOptions::default()).unwrap();
    for table in SEED_TABLES {
        assert_eq!(table_snapshot(&dev_config(), table), table_snapshot(&sqlite_config(&local), table), "{table}");
    }
}

/// Scratch objects of this run on the shared server, dropped when done.
struct ScratchTables {
    prefix: String,
}

impl ScratchTables {
    fn new(label: &str) -> Self {
        Self { prefix: format!("dump_rt_{}_{label}", std::process::id()) }
    }

    fn fixture(&self) -> String {
        let p = &self.prefix;
        format!(
            "create table \"{p}_items\" (
               id integer primary key autoincrement, name text not null, amount real, data blob, any_v any,
               slug text generated always as (lower(name)) virtual
             );
             create table \"{p}_kv\" (k text primary key, v text) without rowid;
             create index \"{p}_items_name\" on \"{p}_items\" (name) where amount > 0;
             create view \"{p}_big\" as select id, name from \"{p}_items\" where amount > 10;
             create trigger \"{p}_items_kv\" after insert on \"{p}_items\" begin
               insert or replace into \"{p}_kv\" values ('last; name', case when new.name is null then 'none' else new.name end);
             end;
             insert into \"{p}_items\" (name, amount, data, any_v) values
               ('it''s; \"quoted\"', 1.5, x'00ff10', 42), ('üñí 🐻', -0.0, x'', 'text'), ('nul', 1e300, null, cast(x'610062' as text));
             with recursive n(i) as (select 1 union all select i + 1 from n where i < 1500)
             insert into \"{p}_items\" (name, amount) select 'row ' || i, i * 0.25 from n;"
        )
    }

    fn tables(&self) -> Vec<String> {
        vec![format!("{}_items", self.prefix), format!("{}_kv", self.prefix)]
    }
}

impl Drop for ScratchTables {
    fn drop(&mut self) {
        let conn = Connection::new(dev_config());
        let p = &self.prefix;
        let _ = block_on(conn.execute(format!(
            "drop view if exists \"{p}_big\"; drop table if exists \"{p}_items\"; drop table if exists \"{p}_kv\";"
        )));
    }
}

#[test]
fn restores_into_libsql_and_dumps_it_back() {
    if !enabled() {
        return;
    }
    let scratch = ScratchTables::new("rt");
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    rusqlite::Connection::open(&source).unwrap().execute_batch(&scratch.fixture()).unwrap();

    // Local SQLite → script → Turso (one transaction).
    let script = dir.path().join("local.sql");
    run_dump(sqlite_config(&source), &script, DumpOptions::default());
    let restored = run_restore(dev_config(), &script, RestoreOptions::default()).unwrap();
    assert!(restored.statements > 1500 && restored.errors.is_empty(), "{restored:?}");

    // Turso → script → a fresh local file: same rows and definitions as the source.
    let back = dir.path().join("back.sql");
    let mut scope: Vec<TableInfo> = scratch.tables().iter().map(|t| TableInfo::new("main", t.as_str())).collect();
    scope.push(TableInfo::new("main", format!("{}_big", scratch.prefix)));
    let summary = run_dump(dev_config(), &back, DumpOptions { scope: DumpScope::Tables(scope), ..Default::default() });
    assert_eq!(summary.tables, 2);
    let text = std::fs::read_to_string(&back).unwrap();
    assert!(text.contains(&format!("{}_items_kv", scratch.prefix)) && text.contains(&format!("{}_big", scratch.prefix)), "{text}");

    let local = empty_db(dir.path(), "back.db");
    run_restore(sqlite_config(&local), &back, RestoreOptions::default()).unwrap();
    for table in scratch.tables() {
        assert_eq!(table_snapshot(&sqlite_config(&source), &table), table_snapshot(&sqlite_config(&local), &table), "{table}");
    }
}

#[test]
fn reports_errors_by_line_or_keeps_going() {
    if !enabled() {
        return;
    }
    let scratch = ScratchTables::new("err");
    let dir = tempfile::tempdir().unwrap();
    let table = &scratch.tables()[1];
    let script = dir.path().join("bad.sql");
    std::fs::write(
        &script,
        format!("create table \"{table}\" (k text primary key, v text);\ninsert into \"{table}\" values ('a', '1');\ninsert into nope values (2);\ninsert into \"{table}\" values ('b', '2');\n"),
    )
    .unwrap();

    // One transaction: nothing stays.
    let err = run_restore(dev_config(), &script, RestoreOptions::default()).unwrap_err();
    assert!(matches!(&err, Error::Query(m) if m.starts_with("Line 3:") && m.contains("nope")), "{err:?}");
    let conn = Connection::new(dev_config());
    assert!(block_on(conn.execute(format!("select count(*) from \"{table}\""))).is_err(), "rolled back");

    // Keep going: the rest ran.
    let options = RestoreOptions { single_transaction: false, stop_on_error: false };
    let summary = run_restore(dev_config(), &script, options).unwrap();
    assert_eq!((summary.statements, summary.error_count), (3, 1), "{summary:?}");
    let count = block_on(conn.execute(format!("select count(*) from \"{table}\""))).unwrap();
    assert_eq!(count.rows[0][0], Value::Int(2));
}
