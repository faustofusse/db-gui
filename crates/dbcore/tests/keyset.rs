//! Keyset paging (`Connection::fetch_page`) against OFFSET on every engine: paging through a table
//! with any sort (NULLs, duplicates, mixed directions) and filter must give exactly the rows of
//! one unpaged query, in the same order.
//!
//! SQLite always runs (temp file). Postgres and MySQL run with `DBEAR_TEST_POSTGRES=1` /
//! `DBEAR_TEST_MYSQL=1` against the dev databases, in a scratch `dbear_keyset` schema/database
//! that is dropped afterwards. Turso / libSQL runs with `DBEAR_TEST_LIBSQL=1` against the dev
//! libSQL server, in `dbear_keyset_*` tables that are dropped afterwards. SQL Server runs with
//! `DBEAR_TEST_SQLSERVER=1` against the dev SQL Server, in a scratch `dbear_keyset` schema of
//! `app_dev` that is dropped afterwards.

use dbcore::{mock, Connection, ConnectionConfig, DatabaseKind, PageCursor, RowQuery, SortKey, TableInfo, Value};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn env_on(var: &str) -> bool {
    let on = std::env::var(var).is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set {var}=1");
    }
    on
}

const ROWS: i64 = 61;
const PAGE: u32 = 7;

/// Pages through `table` with `fetch_page`. Returns the rows and whether any page seeked.
fn page_all(conn: &Connection, table: &TableInfo, query: &RowQuery) -> (Vec<Vec<Value>>, bool) {
    let mut rows = Vec::new();
    let mut after: Option<PageCursor> = None;
    let mut seeked = false;
    for _ in 0..1000 {
        let page = block_on(conn.fetch_page(table.clone(), query.clone(), PAGE, after.clone())).unwrap();
        assert!(page.result.rows.len() <= PAGE as usize);
        if after.is_none() {
            // Turso never counts (rows read are billed).
            let counts = conn.config().kind != DatabaseKind::Libsql;
            assert!(page.result.total_count.is_some() == counts || table.kind == dbcore::TableKind::View);
        } else {
            assert_eq!(page.result.total_count, None);
        }
        rows.extend(page.result.rows);
        let Some(next) = page.next else { return (rows, seeked) };
        assert_eq!(next.rows_before(), rows.len() as u64);
        // Cursors cross FFI as strings.
        let next = PageCursor::decode(&next.encode()).unwrap();
        seeked |= next.is_keyset();
        after = Some(next);
    }
    panic!("paging never ended")
}

fn sorted(keys: &[(&str, bool)]) -> RowQuery {
    RowQuery { sort: keys.iter().map(|&(c, d)| SortKey { column: c.into(), descending: d }).collect(), filter: None }
}

/// Every single-column sort both ways, some mixed pairs, a filter: keyset pages = one big page.
/// Tables without a unique tiebreak (views, keyless MySQL tables) have no stable order between
/// queries, so OFFSET pages can repeat or skip rows (as they always did): for them
/// (`stable: false`) only the row count is compared.
fn check_table(conn: &Connection, table: &TableInfo, columns: &[&str], stable: bool, expect_keyset: impl Fn(&RowQuery) -> bool) {
    let same = |a: &[Vec<Value>], b: &[Vec<Value>], what: &str| {
        assert_eq!(a.len(), b.len(), "{what}");
        if stable {
            assert_eq!(a, b, "{what}");
        }
    };
    let mut queries = vec![RowQuery::default()];
    for (i, &c) in columns.iter().enumerate() {
        queries.push(sorted(&[(c, false)]));
        queries.push(sorted(&[(c, true)]));
        let other = columns[(i + 1) % columns.len()];
        if other != c {
            queries.push(sorted(&[(c, true), (other, false)]));
            queries.push(sorted(&[(c, false), (other, true)]));
        }
    }
    let mut filtered = sorted(&[(columns[0], true)]);
    filtered.filter = Some(format!("{} is not null or {} is null", columns[0], columns[columns.len() - 1]));
    queries.push(filtered);

    for query in queries {
        let all = block_on(conn.fetch_rows_with(table.clone(), query.clone(), 100_000, 0)).unwrap();
        let (paged, seeked) = page_all(conn, table, &query);
        assert_eq!(paged.len(), all.rows.len(), "{} {query:?}", table.name);
        same(&paged, &all.rows, &format!("{} {query:?}", table.name));
        assert_eq!(seeked, expect_keyset(&query), "keyset used for {} {query:?}", table.name);
        // OFFSET pages (`fetch_rows`) agree too.
        let mut offset = Vec::new();
        while offset.len() < all.rows.len() {
            let page = block_on(conn.fetch_rows_with(table.clone(), query.clone(), PAGE, offset.len() as u64)).unwrap();
            assert!(!page.rows.is_empty());
            offset.extend(page.rows);
        }
        same(&offset, &all.rows, &format!("offset {} {query:?}", table.name));
    }
}

/// `insert` statements for the shared test data: duplicates and NULLs in every column.
/// `bytes` spells a binary literal, `float` a double expression.
fn data_rows(bytes: impl Fn(&str) -> String, float: impl Fn(i64) -> String) -> Vec<String> {
    (0..ROWS)
        .map(|i| {
            let null_or = |cond: bool, v: String| if cond { "null".to_string() } else { v };
            let words = ["'x'", "'X'", "'y'", "'é'", "'x '"];
            let a = null_or(i % 7 == 0, (i % 5).to_string());
            let b = null_or(i % 6 == 0, words[(i % 5) as usize].to_string());
            let c = null_or(i % 4 == 0, format!("'2025-01-0{} 10:00:0{}'", 1 + i % 3, i % 2));
            let d = null_or(i % 9 == 0, format!("{}.25", i % 4));
            let e = null_or(i % 8 == 0, float(i % 3));
            let g = null_or(i % 5 == 0, bytes(&format!("{:02x}ff", i % 3)));
            format!("({i}, {a}, {b}, {c}, {d}, {e}, {g})")
        })
        .collect()
}

const SORT_COLUMNS: [&str; 6] = ["a", "b", "c", "d", "e", "g"];

// MARK: SQLite

#[test]
fn sqlite_keyset_matches_offset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    let values = data_rows(|hex| format!("X'{hex}'"), |k| format!("{k} / 3.0")).join(", ");
    db.execute_batch(&format!(
        "create table main_t (id integer primary key, a int, b text, c text, d numeric, e real, g blob);
         insert into main_t values {values};
         -- Composite key: paged by rowid.
         create table pair (x int, y text, a int, b text, c text, d numeric, e real, g blob, primary key (x, y));
         insert into pair select id % 5, 'k' || id, a, b, c, d, e, g from main_t;
         -- Single non-integer key that (SQLite quirk) holds NULLs: key, then rowid.
         create table loose (k text primary key, a int, b text, c text, d numeric, e real, g blob);
         insert into loose select case when id % 10 = 0 then null else 'k' || id end, a, b, c, d, e, g from main_t;
         create table strict_t (k text primary key, a int, b text, c text, d numeric, e real, g blob) without rowid;
         insert into strict_t select 'k' || id, a, b, c, d, e, g from main_t;
         -- Mixed storage classes in one column.
         create table mixed (id integer primary key, a, b, c, d, e, g);
         insert into mixed select id, case when id % 3 = 0 then 'text' || a when id % 3 = 1 then a * 1.5 else a end, b, c, d, e, g from main_t;
         create table shadow (rowid_ text, \"rowid\" int, a int, b text, c text, d numeric, e real, g blob);
         insert into shadow select 'r', id, a, b, c, d, e, g from main_t;
         create view v as select * from main_t;"
    ))
    .unwrap();
    drop(db);

    let mut config = ConnectionConfig::new_empty(DatabaseKind::Sqlite);
    config.id = "keyset-test".into();
    config.database = path.display().to_string();
    let conn = Connection::new(config);
    let t = |name: &str| TableInfo::new("main", name);
    for name in ["main_t", "pair", "loose", "strict_t", "mixed"] {
        check_table(&conn, &t(name), &SORT_COLUMNS, true, |_| true);
    }
    // A column named `rowid` hides the row id: OFFSET.
    check_table(&conn, &t("shadow"), &SORT_COLUMNS, true, |_| false);
    let mut view = t("v");
    view.kind = dbcore::TableKind::View;
    check_table(&conn, &view, &SORT_COLUMNS, false, |_| false);
}

// MARK: Postgres

#[test]
fn postgres_keyset_matches_offset() {
    if !env_on("DBEAR_TEST_POSTGRES") {
        return;
    }
    let config = mock::connections().into_iter().find(|c| c.id == mock::DEV_DATABASE).unwrap();
    let conn = Connection::new(config);
    let values = data_rows(|hex| format!("'\\x{hex}'::bytea"), |k| format!("{k}::float8 / 3")).join(", ");
    let columns = "a int, b text, c timestamptz, d numeric, e float8, g bytea";
    block_on(conn.execute(format!(
        "drop schema if exists dbear_keyset cascade;
         create schema dbear_keyset;
         create type dbear_keyset.mood as enum ('sad', 'ok', 'happy');
         create table dbear_keyset.main_t (id int primary key, {columns}, m dbear_keyset.mood);
         insert into dbear_keyset.main_t (id, a, b, c, d, e, g) values {values};
         update dbear_keyset.main_t set m = (array['happy', 'sad', null, 'ok']::dbear_keyset.mood[])[1 + id % 4];
         create table dbear_keyset.pair (x int, y text, {columns}, primary key (x, y));
         insert into dbear_keyset.pair select id % 5, 'k' || id, a, b, c, d, e, g from dbear_keyset.main_t;
         -- No primary key: the NOT NULL unique index wins over the nullable one and over ctid.
         create table dbear_keyset.uniq (n int, u int not null, {columns});
         create unique index on dbear_keyset.uniq (n);
         create unique index on dbear_keyset.uniq (u);
         insert into dbear_keyset.uniq select case when id % 2 = 0 then id end, 1000 - id, a, b, c, d, e, g from dbear_keyset.main_t;
         create table dbear_keyset.heap ({columns});
         insert into dbear_keyset.heap select a, b, c, d, e, g from dbear_keyset.main_t;
         create materialized view dbear_keyset.mv as select * from dbear_keyset.heap;
         create view dbear_keyset.v as select * from dbear_keyset.main_t;"
    )))
    .unwrap();

    let t = |name: &str| TableInfo::new("dbear_keyset", name);
    let mut with_enum = SORT_COLUMNS.to_vec();
    with_enum.push("m");
    check_table(&conn, &t("main_t"), &with_enum, true, |_| true);
    for name in ["pair", "uniq", "heap", "mv"] {
        check_table(&conn, &t(name), &SORT_COLUMNS, true, |_| true);
    }
    let mut view = t("v");
    view.kind = dbcore::TableKind::View;
    check_table(&conn, &view, &SORT_COLUMNS, false, |_| false);

    // Partitioned with a composite key, from the seed: a deep keyset walk matches OFFSET.
    let events = TableInfo::new("analytics", "events");
    let query = sorted(&[("user_id", true)]);
    let mut after = None;
    for _ in 0..20 {
        let page = block_on(conn.fetch_page(events.clone(), query.clone(), 500, after)).unwrap();
        after = page.next;
    }
    let after = after.unwrap();
    assert!(after.is_keyset());
    let page = block_on(conn.fetch_page(events.clone(), query.clone(), 500, Some(after.clone()))).unwrap();
    let offset = block_on(conn.fetch_rows_with(events, query, 500, after.rows_before())).unwrap();
    assert_eq!(page.result.rows, offset.rows);

    block_on(conn.execute("drop schema dbear_keyset cascade".into())).unwrap();
}

// MARK: MySQL

#[test]
fn mysql_keyset_matches_offset() {
    if !env_on("DBEAR_TEST_MYSQL") {
        return;
    }
    let config = mock::connections().into_iter().find(|c| c.id == mock::DEV_MYSQL).unwrap();
    let conn = Connection::new(config);
    let values = data_rows(|hex| format!("X'{hex}'"), |k| format!("{k} / 3e0")).join(", ");
    let columns = "a int, b varchar(10), c datetime, d decimal(6,2), e double, g varbinary(8)";
    for sql in [
        "drop database if exists dbear_keyset".to_string(),
        "create database dbear_keyset".into(),
        format!("create table dbear_keyset.main_t (id bigint unsigned primary key, {columns}, f float, m enum('sad','ok','happy'), t text)"),
        format!("insert into dbear_keyset.main_t (id, a, b, c, d, e, g) values {values}"),
        "update dbear_keyset.main_t set f = e, m = elt(1 + id % 4, 'happy', 'sad', null, 'ok'), t = b".into(),
        format!("create table dbear_keyset.pair (x int, y varchar(10), {columns}, primary key (x, y))"),
        "insert into dbear_keyset.pair select id % 5, concat('k', id), a, b, c, d, e, g from dbear_keyset.main_t".into(),
        format!("create table dbear_keyset.uniq (n int, u int not null, {columns}, unique key (n), unique key (u))"),
        "insert into dbear_keyset.uniq select case when id % 2 = 0 then id end, 1000 - id, a, b, c, d, e, g from dbear_keyset.main_t".into(),
        format!("create table dbear_keyset.heap ({columns})"),
        "insert into dbear_keyset.heap select a, b, c, d, e, g from dbear_keyset.main_t".into(),
    ] {
        block_on(conn.execute(sql)).unwrap();
    }

    let t = |name: &str| TableInfo::new("dbear_keyset", name);
    let mut all_columns = SORT_COLUMNS.to_vec();
    all_columns.extend(["f", "m", "t"]);
    // FLOAT, ENUM and TEXT sort keys page with OFFSET.
    let seekable = |q: &RowQuery| !q.sort.iter().any(|k| ["f", "m", "t"].contains(&k.column.as_str()));
    check_table(&conn, &t("main_t"), &all_columns, true, seekable);
    check_table(&conn, &t("pair"), &SORT_COLUMNS, true, |_| true);
    check_table(&conn, &t("uniq"), &SORT_COLUMNS, true, |_| true);
    // No key at all: OFFSET.
    check_table(&conn, &t("heap"), &SORT_COLUMNS, false, |_| false);

    block_on(conn.execute("drop database dbear_keyset".into())).unwrap();
}

// MARK: Turso / libSQL

#[test]
fn libsql_keyset_matches_offset() {
    if !env_on("DBEAR_TEST_LIBSQL") {
        return;
    }
    let config = mock::connections().into_iter().find(|c| c.id == mock::DEV_LIBSQL).unwrap();
    let conn = Connection::new(config);
    let values = data_rows(|hex| format!("X'{hex}'"), |k| format!("{k} / 3.0")).join(", ");
    // Same tables as the SQLite test (the server is SQLite), prefixed: there's one shared database.
    let names = ["main_t", "pair", "loose", "strict_t", "mixed", "shadow"];
    let drop_all = || {
        let drops: Vec<String> = names.iter().map(|n| format!("drop table if exists dbear_keyset_{n};")).collect();
        block_on(conn.execute(format!("drop view if exists dbear_keyset_v; {}", drops.join(" ")))).unwrap();
    };
    drop_all();
    block_on(conn.execute(format!(
        "create table dbear_keyset_main_t (id integer primary key, a int, b text, c text, d numeric, e real, g blob);
         insert into dbear_keyset_main_t values {values};
         create table dbear_keyset_pair (x int, y text, a int, b text, c text, d numeric, e real, g blob, primary key (x, y));
         insert into dbear_keyset_pair select id % 5, 'k' || id, a, b, c, d, e, g from dbear_keyset_main_t;
         create table dbear_keyset_loose (k text primary key, a int, b text, c text, d numeric, e real, g blob);
         insert into dbear_keyset_loose select case when id % 10 = 0 then null else 'k' || id end, a, b, c, d, e, g from dbear_keyset_main_t;
         create table dbear_keyset_strict_t (k text primary key, a int, b text, c text, d numeric, e real, g blob) without rowid;
         insert into dbear_keyset_strict_t select 'k' || id, a, b, c, d, e, g from dbear_keyset_main_t;
         create table dbear_keyset_mixed (id integer primary key, a, b, c, d, e, g);
         insert into dbear_keyset_mixed select id, case when id % 3 = 0 then 'text' || a when id % 3 = 1 then a * 1.5 else a end, b, c, d, e, g from dbear_keyset_main_t;
         create table dbear_keyset_shadow (rowid_ text, \"rowid\" int, a int, b text, c text, d numeric, e real, g blob);
         insert into dbear_keyset_shadow select 'r', id, a, b, c, d, e, g from dbear_keyset_main_t;
         create view dbear_keyset_v as select * from dbear_keyset_main_t;"
    )))
    .unwrap();

    let t = |name: &str| TableInfo::new("main", format!("dbear_keyset_{name}"));
    for name in ["main_t", "pair", "loose", "strict_t", "mixed"] {
        check_table(&conn, &t(name), &SORT_COLUMNS, true, |_| true);
    }
    check_table(&conn, &t("shadow"), &SORT_COLUMNS, true, |_| false);
    let mut view = t("v");
    view.kind = dbcore::TableKind::View;
    check_table(&conn, &view, &SORT_COLUMNS, false, |_| false);

    drop_all();
}

// MARK: SQL Server

#[test]
fn sqlserver_keyset_matches_offset() {
    if !env_on("DBEAR_TEST_SQLSERVER") {
        return;
    }
    let config = mock::connections().into_iter().find(|c| c.id == mock::DEV_SQLSERVER).unwrap();
    let conn = Connection::new(config);
    let values = data_rows(|hex| format!("0x{hex}"), |k| format!("{k} / 3e0")).join(", ");
    let columns = "a int, b nvarchar(10), c datetime2(0), d decimal(6,2), e float, g varbinary(8)";
    let drop_all = "if schema_id('dbear_keyset') is not null begin
           drop view if exists dbear_keyset.v;
           drop table if exists dbear_keyset.main_t, dbear_keyset.pair, dbear_keyset.uniq, dbear_keyset.heap;
           drop schema dbear_keyset;
         end";
    block_on(conn.execute(format!(
        "{drop_all}
         GO
         create schema dbear_keyset
         GO
         -- v (varchar, compared without N''), l (legacy datetime) and u seek; r (real) and m (max) can't.
         create table dbear_keyset.main_t (id int primary key, {columns},
           v varchar(10), l datetime, u uniqueidentifier, r real, m nvarchar(max));
         insert into dbear_keyset.main_t (id, a, b, c, d, e, g) values {values};
         update dbear_keyset.main_t set v = b, l = c, r = e, m = b,
           u = case when id % 3 = 0 then null else cast(hashbytes('MD5', cast(id % 11 as varchar)) as uniqueidentifier) end;
         create table dbear_keyset.pair (x int, y nvarchar(10), {columns}, primary key (x, y));
         insert into dbear_keyset.pair select id % 5, concat('k', id), a, b, c, d, e, g from dbear_keyset.main_t;
         -- No primary key: the NOT NULL unique index is the tiebreak; the filtered one doesn't qualify.
         create table dbear_keyset.uniq (n int, u2 int not null, {columns});
         create unique index ux_n on dbear_keyset.uniq (n) where n is not null;
         create unique index ux_u2 on dbear_keyset.uniq (u2);
         insert into dbear_keyset.uniq select case when id % 2 = 0 then id end, 1000 - id, a, b, c, d, e, g from dbear_keyset.main_t;
         create table dbear_keyset.heap ({columns});
         insert into dbear_keyset.heap select a, b, c, d, e, g from dbear_keyset.main_t;
         GO
         create view dbear_keyset.v as select * from dbear_keyset.main_t;"
    )))
    .unwrap();

    let t = |name: &str| TableInfo::new("dbear_keyset", name);
    let mut all_columns = SORT_COLUMNS.to_vec();
    all_columns.extend(["v", "l", "u", "r", "m"]);
    let seekable = |q: &RowQuery| !q.sort.iter().any(|k| ["r", "m"].contains(&k.column.as_str()));
    check_table(&conn, &t("main_t"), &all_columns, true, seekable);
    check_table(&conn, &t("pair"), &SORT_COLUMNS, true, |_| true);
    check_table(&conn, &t("uniq"), &SORT_COLUMNS, true, |_| true);
    // No key at all: OFFSET, in no particular order.
    check_table(&conn, &t("heap"), &SORT_COLUMNS, false, |_| false);
    let mut view = t("v");
    view.kind = dbcore::TableKind::View;
    check_table(&conn, &view, &SORT_COLUMNS, false, |_| false);

    block_on(conn.execute(drop_all.into())).unwrap();
}
