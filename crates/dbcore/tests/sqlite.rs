//! SQLite driver tests. Always run: each test seeds its own temporary file from `dev/sqlite/init.sql`.

use std::time::{Duration, Instant};

use dbcore::{Connection, ConnectionConfig, DatabaseKind, Error, TableInfo, TableKind, Value};

struct TempDb {
    _dir: tempfile::TempDir,
    path: String,
}

fn seeded() -> TempDb {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");
    let seed = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/sqlite/init.sql")).unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch(&seed).unwrap();
    TempDb { path: path.display().to_string(), _dir: dir }
}

fn open(db: &TempDb) -> Connection {
    let mut config = ConnectionConfig::new_empty(DatabaseKind::Sqlite);
    config.id = "test".into();
    config.database = db.path.clone();
    Connection::new(config)
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn column<'a>(result: &'a dbcore::QueryResult, name: &str) -> Vec<&'a Value> {
    let i = result.columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no column {name}"));
    result.rows.iter().map(|r| &r[i]).collect()
}

#[test]
fn lists_tables_and_views_with_counts() {
    let db = seeded();
    let schemas = block_on(open(&db).list_schemas()).unwrap();
    assert_eq!(schemas.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["main"]);
    let tables: Vec<_> = schemas[0].tables.iter().map(|t| (t.name.as_str(), t.kind, t.estimated_row_count)).collect();
    assert_eq!(
        tables,
        [
            ("events", TableKind::Table, Some(20_000)),
            ("note_tags", TableKind::Table, Some(168)),
            ("notes", TableKind::Table, Some(120)),
            ("pinned_notes", TableKind::View, None),
            ("settings", TableKind::Table, Some(5)),
            ("tags", TableKind::Table, Some(5)),
        ]
    );
}

#[test]
fn pages_tables_with_typed_values() {
    let db = seeded();
    let conn = open(&db);
    let first = block_on(conn.fetch_rows(TableInfo::new("main", "notes"), 50, 0)).unwrap();
    assert_eq!((first.rows.len(), first.total_count), (50, Some(120)));
    let id = first.columns.iter().find(|c| c.name == "id").unwrap();
    assert!(id.is_primary_key && !id.is_nullable);
    assert_eq!(first.columns.iter().find(|c| c.name == "price").unwrap().type_name, "decimal(10,2)");
    assert_eq!(column(&first, "id")[..3], [&Value::Int(1), &Value::Int(2), &Value::Int(3)]);
    assert_eq!(column(&first, "pinned")[3], &Value::Bool(true));
    assert_eq!(column(&first, "price")[2], &Value::Decimal("19.9".into()));
    assert_eq!(column(&first, "body")[5], &Value::Null);
    assert!(matches!(column(&first, "attachment")[9], Value::Text(t) if t.starts_with("0x") && t.len() == 18));

    let last = block_on(conn.fetch_rows(TableInfo::new("main", "notes"), 50, 100)).unwrap();
    assert_eq!((last.rows.len(), last.total_count), (20, None));
    assert_eq!(column(&last, "id")[0], &Value::Int(101));
}

#[test]
fn pages_without_rowid_and_keyless_tables_and_views() {
    let db = seeded();
    let conn = open(&db);
    let tags = block_on(conn.fetch_rows(TableInfo::new("main", "note_tags"), 3, 0)).unwrap();
    assert_eq!(tags.rows[0][..2], [Value::Int(1), Value::Int(2)]);
    let settings = block_on(conn.fetch_rows(TableInfo::new("main", "settings"), 10, 0)).unwrap();
    assert_eq!(
        column(&settings, "value"),
        [&Value::Text("dark".into()), &Value::Int(13), &Value::Float(1.25), &Value::Null, &Value::Text("0xdeadbeef".into())]
    );
    let view = block_on(conn.fetch_rows(TableInfo::new("main", "pinned_notes"), 100, 0)).unwrap();
    assert_eq!((view.rows.len(), view.total_count), (30, None));
}

#[test]
fn missing_tables_and_files_fail_clearly() {
    let db = seeded();
    let err = block_on(open(&db).fetch_rows(TableInfo::new("main", "nope"), 10, 0)).unwrap_err();
    assert_eq!(err, Error::TableNotFound("main.nope".into()));

    let missing = TempDb { path: "/definitely/not/here.db".into(), _dir: tempfile::tempdir().unwrap() };
    let err = block_on(open(&missing).connect()).unwrap_err();
    assert!(matches!(err, Error::ConnectionFailed(ref m) if m.contains("No database file")), "{err:?}");
}

#[test]
fn runs_scripts() {
    let db = seeded();
    let conn = open(&db);
    // Last statement's rows win; writes before it are applied.
    let r = block_on(conn.execute(
        "update notes set pinned = 1 where id <= 3; select count(*) as n from notes where pinned".into(),
    ))
    .unwrap();
    assert_eq!(r.rows, [[Value::Int(33)]]);

    let r = block_on(conn.execute("insert into tags (name) values ('a'), ('b')".into())).unwrap();
    assert_eq!((r.rows_affected, r.columns.len()), (Some(2), 0));

    let r = block_on(conn.execute_limited("select * from events".into(), Some(1000))).unwrap();
    assert_eq!((r.rows.len(), r.truncated, r.total_count), (1000, true, Some(20_000)));

    // Attached databases show up as schemas.
    block_on(conn.execute("attach ':memory:' as scratch; create table scratch.t (x)".into())).unwrap();
    let schemas = block_on(conn.list_schemas()).unwrap();
    assert!(schemas.iter().any(|s| s.name == "main"));
}

#[test]
fn reports_errors_with_position() {
    let db = seeded();
    let err = block_on(open(&db).execute("select 1;\nselect nope from notes".into())).unwrap_err();
    let Error::Query(message) = err else { panic!("{err:?}") };
    assert!(message.contains("no such column: nope") && message.contains("line 2, column 8"), "{message}");
}

#[test]
fn cancels_long_scripts() {
    let db = seeded();
    let conn = open(&db);
    let started = Instant::now();
    let runner = conn.clone();
    let handle = std::thread::spawn(move || {
        block_on(runner.execute(
            "with recursive r(n) as (select 1 union all select n + 1 from r) select count(*) from r".into(),
        ))
    });
    std::thread::sleep(Duration::from_millis(300));
    block_on(conn.cancel());
    assert_eq!(handle.join().unwrap().unwrap_err(), Error::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    // The session is still usable.
    assert_eq!(block_on(conn.execute("select 1".into())).unwrap().rows, [[Value::Int(1)]]);
}

#[test]
fn tracks_connection_state() {
    let db = seeded();
    let conn = open(&db);
    assert!(!block_on(conn.is_connected()));
    block_on(conn.list_schemas()).unwrap();
    assert!(block_on(conn.is_connected()));
    block_on(conn.disconnect());
    assert!(!block_on(conn.is_connected()));
}
