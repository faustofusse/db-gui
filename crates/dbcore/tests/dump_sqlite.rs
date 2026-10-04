//! SQLite dump → restore round trips. Always run (temporary files, `dev/dump/sqlite_fixture.sql`).
//! Also checks compatibility with the `sqlite3` CLI when it's installed (`nix develop` has it).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use dbcore::dump::{self, CancelToken, Compression, DumpContent, DumpOptions, DumpProgress, DumpScope};
use dbcore::restore::{self, RestoreOptions};
use dbcore::{ConnectionConfig, DatabaseKind, Error, TableInfo};
use rusqlite::types::Value;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn fixture(dir: &Path) -> PathBuf {
    let path = dir.join("source.db");
    let seed = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/dump/sqlite_fixture.sql")).unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch(&seed).unwrap();
    path
}

fn config(path: &Path) -> ConnectionConfig {
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

fn run_dump(source: &Path, out: &Path, options: DumpOptions) -> dbcore::Result<dump::DumpSummary> {
    block_on(dump::dump(config(source), out.to_path_buf(), options, Arc::new(|_: &DumpProgress| {}), CancelToken::new()))
}

fn run_restore(target: &Path, script: &Path) -> dbcore::Result<restore::RestoreSummary> {
    block_on(restore::restore(
        config(target),
        script.to_path_buf(),
        RestoreOptions::default(),
        Arc::new(|_: &restore::RestoreProgress| {}),
        CancelToken::new(),
    ))
}

/// Schema objects and every row of every table, comparable across files.
fn snapshot(path: &Path) -> Snapshot {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut schema: Vec<(String, String, Option<String>)> = conn
        .prepare("select type, name, sql from sqlite_master where name not like 'sqlite_autoindex%' order by type, name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    schema.retain(|(_, name, _)| name != "sqlite_sequence");
    let tables: Vec<String> = schema.iter().filter(|(kind, ..)| kind == "table").map(|(_, n, _)| n.clone()).collect();
    let mut data = Vec::new();
    for table in tables.iter().chain(std::iter::once(&"sqlite_sequence".to_string())) {
        let Ok(mut statement) = conn.prepare(&format!("select * from \"{}\"", table.replace('"', "\"\""))) else { continue };
        let width = statement.column_count();
        let mut rows: Vec<Vec<Value>> = statement
            .query_map([], |r| (0..width).map(|i| r.get::<_, Value>(i)).collect())
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        data.push((table.clone(), rows));
    }
    (schema, data)
}

type Snapshot = (Vec<(String, String, Option<String>)>, Vec<(String, Vec<Vec<Value>>)>);

/// Compares table by table, so a failure shows the rows that differ.
fn assert_same(left: &Snapshot, right: &Snapshot, schema: bool) {
    if schema {
        assert_eq!(left.0, right.0);
    }
    assert_eq!(left.1.len(), right.1.len());
    for ((table, a), (_, b)) in left.1.iter().zip(&right.1) {
        let only_left: Vec<_> = a.iter().filter(|r| !b.contains(r)).collect();
        let only_right: Vec<_> = b.iter().filter(|r| !a.contains(r)).collect();
        assert!(only_left.is_empty() && only_right.is_empty() && a.len() == b.len(), "{table}: {only_left:?} vs {only_right:?}");
    }
}

#[test]
fn round_trips_schema_and_data() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(dir.path());
    let script = dir.path().join("dump.sql");
    let summary = run_dump(&source, &script, DumpOptions::default()).unwrap();
    assert_eq!(summary.tables, 4);
    assert!(summary.rows > 2000 && summary.bytes > 0 && summary.warnings.is_empty(), "{summary:?}");
    assert!(!dump::partial_path(&script).exists());

    let target = empty_db(dir.path(), "target.db");
    let restored = run_restore(&target, &script).unwrap();
    assert!(restored.statements > 2000 && restored.errors.is_empty(), "{restored:?}");
    assert_same(&snapshot(&source), &snapshot(&target), true);
}

#[test]
fn round_trips_gzipped_and_reports_progress() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(dir.path());
    let script = dir.path().join("dump.sql.gz");
    let events = Arc::new(Mutex::new(Vec::<DumpProgress>::new()));
    let seen = events.clone();
    let options = DumpOptions { compression: Compression::Gzip, drop_objects: true, ..Default::default() };
    block_on(dump::dump(config(&source), script.clone(), options, Arc::new(move |p: &DumpProgress| seen.lock().unwrap().push(p.clone())), CancelToken::new()))
        .unwrap();
    let bytes = std::fs::read(&script).unwrap();
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
    let events = events.lock().unwrap();
    let last = events.last().unwrap();
    assert_eq!((last.tables_done, last.tables_total), (4, 4));
    assert!(events.iter().any(|e| e.object.as_deref() == Some("kv") && e.table_rows_estimate == Some(2003)));

    // Restoring twice works thanks to the DROP statements.
    let target = empty_db(dir.path(), "target.db");
    run_restore(&target, &script).unwrap();
    run_restore(&target, &script).unwrap();
    assert_same(&snapshot(&source), &snapshot(&target), true);
}

#[test]
fn dumps_schema_only_data_only_and_selected_tables() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(dir.path());

    let schema = dir.path().join("schema.sql");
    run_dump(&source, &schema, DumpOptions { content: DumpContent::SchemaOnly, ..Default::default() }).unwrap();
    let text = std::fs::read_to_string(&schema).unwrap();
    assert!(text.contains("CREATE TRIGGER child_audit") && !text.contains("INSERT INTO"), "{text}");

    let data = dir.path().join("data.sql");
    run_dump(&source, &data, DumpOptions { content: DumpContent::DataOnly, ..Default::default() }).unwrap();
    let text = std::fs::read_to_string(&data).unwrap();
    assert!(!text.contains("CREATE ") && text.contains("INSERT INTO"), "{text}");

    // Schema, then data, into a fresh file: same as a full dump. Triggers fire while loading
    // data only (a full dump creates them after the rows), so drop the one that writes rows.
    let target = empty_db(dir.path(), "target.db");
    run_restore(&target, &schema).unwrap();
    rusqlite::Connection::open(&target).unwrap().execute_batch("drop trigger child_audit").unwrap();
    run_restore(&target, &data).unwrap();
    assert_same(&snapshot(&source), &snapshot(&target), false);

    let one = dir.path().join("one.sql");
    let scope = DumpScope::Tables(vec![TableInfo::new("main", "child")]);
    let summary = run_dump(&source, &one, DumpOptions { scope, ..Default::default() }).unwrap();
    assert_eq!(summary.tables, 1);
    let text = std::fs::read_to_string(&one).unwrap();
    assert!(text.contains("CREATE TABLE child") && text.contains("child_parent") && text.contains("child_audit"));
    assert!(!text.contains("kv") && !text.contains("big_amounts"), "{text}");
}

#[test]
fn cancel_leaves_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("big.db");
    rusqlite::Connection::open(&source)
        .unwrap()
        .execute_batch(
            "create table t (id integer primary key, payload text);
             with recursive n(i) as (select 1 union all select i + 1 from n where i < 300000)
             insert into t select i, hex(randomblob(32)) from n;",
        )
        .unwrap();
    let script = dir.path().join("big.sql");
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let progress = Arc::new(move |p: &DumpProgress| {
        // As soon as rows are being written (a fast machine dumps them all within one report).
        if p.phase == dump::DumpPhase::Data {
            trigger.cancel();
        }
    });
    let result = block_on(dump::dump(config(&source), script.clone(), DumpOptions::default(), progress, cancel));
    assert_eq!(result, Err(Error::Cancelled));
    assert!(!script.exists() && !dump::partial_path(&script).exists());
}

#[test]
fn stops_or_continues_on_errors() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("bad.sql");
    std::fs::write(&script, "create table a (x);\ninsert into a values (1);\ninsert into nope values (2);\ninsert into a values (3);\n").unwrap();

    // Single transaction: all or nothing.
    let target = empty_db(dir.path(), "t1.db");
    let err = run_restore(&target, &script).unwrap_err();
    assert!(matches!(&err, Error::Query(m) if m.starts_with("Line 3:") && m.contains("no such table")), "{err:?}");
    let conn = rusqlite::Connection::open(&target).unwrap();
    assert!(conn.query_row("select count(*) from a", [], |r| r.get::<_, i64>(0)).is_err());

    // Keep going: the error is reported, the rest ran.
    let target = empty_db(dir.path(), "t2.db");
    let options = RestoreOptions { single_transaction: false, stop_on_error: false };
    let summary = block_on(restore::restore(config(&target), script.clone(), options, Arc::new(|_: &_| {}), CancelToken::new())).unwrap();
    assert_eq!((summary.statements, summary.error_count), (3, 1));
    let conn = rusqlite::Connection::open(&target).unwrap();
    assert_eq!(conn.query_row("select sum(x) from a", [], |r| r.get::<_, i64>(0)).unwrap(), 4);
}

fn sqlite3() -> Option<&'static str> {
    Command::new("sqlite3").arg("-version").output().ok().filter(|o| o.status.success()).map(|_| "sqlite3")
}

#[test]
fn compatible_with_the_sqlite3_cli() {
    let Some(cli) = sqlite3() else {
        eprintln!("skipped: no sqlite3 on PATH");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(dir.path());

    // Our dump, loaded by sqlite3.
    let script = dir.path().join("dump.sql");
    run_dump(&source, &script, DumpOptions::default()).unwrap();
    let by_cli = dir.path().join("by_cli.db");
    let status = Command::new(cli)
        .arg(&by_cli)
        .arg(format!(".read '{}'", script.display()))
        .status()
        .unwrap();
    assert!(status.success());
    assert_same(&snapshot(&source), &snapshot(&by_cli), true);

    // sqlite3's own .dump, restored by us. (Some sqlite3 versions write infinities as `Inf`,
    // which they can't read back either, and cut text at a NUL: leave those rows out.)
    rusqlite::Connection::open(&source).unwrap().execute_batch("delete from \"we\"\"ird table\" where name in ('inf', 'nul')").unwrap();
    let output = Command::new(cli).arg(&source).arg(".dump").output().unwrap();
    assert!(output.status.success());
    let theirs = dir.path().join("theirs.sql");
    std::fs::write(&theirs, &output.stdout).unwrap();
    let target = empty_db(dir.path(), "from_cli_dump.db");
    run_restore(&target, &theirs).unwrap();
    assert_same(&snapshot(&source), &snapshot(&target), false);
}
