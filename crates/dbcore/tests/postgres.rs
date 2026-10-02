//! Integration tests against the dev Postgres (`scripts/dev-db.sh up`).
//! Skipped unless `DBGUI_TEST_POSTGRES=1`; `scripts/test-postgres.sh` sets it up.

use std::time::{Duration, Instant};

use dbcore::{mock, Connection, ConnectionConfig, Error, SslMode, TableInfo, TableKind, Value};

fn enabled() -> bool {
    let on = std::env::var("DBGUI_TEST_POSTGRES").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set DBGUI_TEST_POSTGRES=1 (scripts/dev-db.sh up)");
    }
    on
}

fn dev_config() -> ConnectionConfig {
    mock::connections().into_iter().find(|c| c.id == mock::DEV_DATABASE).unwrap()
}

fn dev() -> Connection {
    Connection::new(dev_config())
}

/// Any executor works: the core runs on its own runtime.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn column<'a>(result: &'a dbcore::QueryResult, name: &str) -> Vec<&'a Value> {
    let i = result.columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no column {name}"));
    result.rows.iter().map(|r| &r[i]).collect()
}

#[test]
fn lists_schemas_and_relations() {
    if !enabled() {
        return;
    }
    let schemas = block_on(dev().list_schemas()).unwrap();
    let names: Vec<_> = schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["analytics", "archive", "billing", "public"]);

    let analytics = &schemas[0];
    let tables: Vec<_> = analytics.tables.iter().map(|t| t.name.as_str()).collect();
    // Partitions are hidden behind their parent; materialized views show as views.
    assert_eq!(tables, ["daily_signups", "events", "raw_log"]);
    assert_eq!(analytics.tables[0].kind, TableKind::View);
    assert_eq!(analytics.tables[1].estimated_row_count, Some(50_000));

    assert!(schemas[1].tables.is_empty(), "archive is empty");

    let public = &schemas[3];
    let active = public.tables.iter().find(|t| t.name == "active_users").unwrap();
    assert_eq!((active.kind, active.estimated_row_count), (TableKind::View, None));
}

#[test]
fn fetches_pages_in_primary_key_order() {
    if !enabled() {
        return;
    }
    let users = TableInfo::new("public", "users");
    let page = block_on(dev().fetch_rows(users.clone(), 50, 100)).unwrap();
    assert_eq!(page.rows.len(), 50);
    assert_eq!(page.total_count, Some(248));
    assert_eq!(page.rows[0][0], Value::Int(101));
    assert_eq!(page.rows[49][0], Value::Int(150));

    let id = &page.columns[0];
    assert!(id.is_primary_key && !id.is_nullable);
    assert_eq!(id.type_name, "bigint");
    let last_login = page.columns.iter().find(|c| c.name == "last_login_at").unwrap();
    assert!(last_login.is_nullable);
    assert_eq!(last_login.type_name, "timestamp with time zone");

    let tail = block_on(dev().fetch_rows(users, 50, 240)).unwrap();
    assert_eq!(tail.rows.len(), 8);
}

#[test]
fn decodes_column_types() {
    if !enabled() {
        return;
    }
    let users = block_on(dev().fetch_rows(TableInfo::new("public", "users"), 4, 0)).unwrap();
    assert!(matches!(column(&users, "is_admin")[0], Value::Bool(false)));
    assert_eq!(column(&users, "tags")[0], &Value::Text("{customer,vip}".into()));
    assert!(matches!(column(&users, "settings")[0], Value::Text(s) if s.starts_with('{') && s.contains("theme")));
    assert_eq!(column(&users, "last_login_at")[0], &Value::Null);

    let sessions = block_on(dev().fetch_rows(TableInfo::new("public", "sessions"), 1, 0)).unwrap();
    assert!(matches!(column(&sessions, "id")[0], Value::Text(s) if s.len() == 36));
    assert!(matches!(column(&sessions, "token")[0], Value::Text(s) if s.starts_with("\\x")));
    // Ordered by a random uuid, so check shapes rather than exact values.
    assert!(matches!(column(&sessions, "ip")[0], Value::Text(s) if s.starts_with("10.0.")));
    assert!(matches!(column(&sessions, "ttl")[0], Value::Text(s) if s.ends_with(":00:00") || s.contains("day")));

    let orders = block_on(dev().fetch_rows(TableInfo::new("public", "orders"), 1, 0)).unwrap();
    assert_eq!(column(&orders, "status")[0], &Value::Text("paid".into()));
    assert!(matches!(column(&orders, "total")[0], Value::Decimal(_)));

    // NUMERIC beyond f64 precision must survive untouched.
    let payments = block_on(dev().fetch_rows(TableInfo::new("billing", "payments"), 1, 0)).unwrap();
    assert!(matches!(column(&payments, "fx_rate")[0], Value::Decimal(s) if s.ends_with(".56789012345678901234")));
}

#[test]
fn pages_tables_without_primary_key_and_views() {
    if !enabled() {
        return;
    }
    let log = block_on(dev().fetch_rows(TableInfo::new("analytics", "raw_log"), 10, 290)).unwrap();
    assert_eq!(log.rows.len(), 10);
    assert_eq!(log.total_count, Some(300));
    assert_eq!(log.rows[0][1], Value::Int(1)); // smallint

    let view = block_on(dev().fetch_rows(TableInfo::new("public", "active_users"), 500, 0)).unwrap();
    assert_eq!(view.total_count, None);
    assert!(!view.rows.is_empty());

    let events = block_on(dev().fetch_rows(TableInfo::new("analytics", "events"), 5, 0)).unwrap();
    assert_eq!(events.rows.len(), 5);
    assert_eq!(events.total_count, Some(50_000));
}

#[test]
fn missing_table_is_reported() {
    if !enabled() {
        return;
    }
    let err = block_on(dev().fetch_rows(TableInfo::new("public", "nope"), 5, 0)).unwrap_err();
    assert_eq!(err, Error::TableNotFound("public.nope".into()));
}

#[test]
fn executes_queries_with_typed_columns() {
    if !enabled() {
        return;
    }
    let result = block_on(dev().execute("select id, total, now() as at from orders order by id limit 3".into())).unwrap();
    let types: Vec<_> = result.columns.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(types, ["int8", "numeric", "timestamptz"]);
    assert_eq!(result.rows.len(), 3);
    assert_eq!(result.rows[2][0], Value::Int(3));
}

#[test]
fn scripts_return_last_result_or_affected_rows() {
    if !enabled() {
        return;
    }
    let conn = dev();
    let rows = block_on(conn.execute("select 1 as a; select 'x' as b, 2 as c;".into())).unwrap();
    assert_eq!(rows.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    // Multi-statement scripts can't be prepared, so values stay text.
    assert_eq!(rows.rows[0][1], Value::Text("2".into()));

    let affected = block_on(conn.execute(
        "create temp table t (x int); insert into t select generate_series(1, 5); update t set x = x + 1 where x > 2"
            .into(),
    ))
    .unwrap();
    assert!(affected.columns.is_empty());
    assert_eq!(affected.rows_affected, Some(3));

    // The temp table lives on the script session, so it's still there.
    let wrapped = block_on(conn.execute("begin; select count(*) from t; commit".into())).unwrap();
    assert_eq!(wrapped.rows[0][0], Value::Text("5".into()));
}

#[test]
fn reports_errors_with_position() {
    if !enabled() {
        return;
    }
    let err = block_on(dev().execute("select 1\nfrom users\nwhere nope = 1".into())).unwrap_err();
    let Error::Query(message) = err else { panic!("{err:?}") };
    assert!(message.starts_with("ERROR: column \"nope\" does not exist (line 3, column 7)"), "{message}");
}

#[test]
fn cancels_running_query() {
    if !enabled() {
        return;
    }
    let conn = dev();
    block_on(conn.connect()).unwrap();
    let started = Instant::now();
    let (result, ()) = block_on(async {
        tokio::join!(conn.execute("select pg_sleep(10)".into()), async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            conn.cancel().await;
        })
    });
    assert_eq!(result.unwrap_err(), Error::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn dropping_the_future_cancels_on_the_server() {
    if !enabled() {
        return;
    }
    let conn = dev();
    let marker = "dbgui_drop_test_marker";
    block_on(async {
        let sql = format!("select pg_sleep(20) as {marker}");
        let _ = tokio::time::timeout(Duration::from_millis(500), conn.execute(sql)).await;
    });

    // Watch from another connection until the backend is no longer running it.
    let observer = dev();
    let still_running = || {
        let sql = format!(
            "select count(*) from pg_stat_activity where state = 'active' and query like '%{marker}%' and pid <> pg_backend_pid()"
        );
        let r = block_on(observer.execute(sql)).unwrap();
        r.rows[0][0] != Value::Int(0)
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while still_running() {
        assert!(Instant::now() < deadline, "query kept running after its future was dropped");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn bad_credentials_and_unreachable_hosts_fail_to_connect() {
    if !enabled() {
        return;
    }
    let wrong_password = ConnectionConfig { password: Some("nope".into()), ..dev_config() };
    let err = block_on(Connection::new(wrong_password).connect()).unwrap_err();
    assert!(matches!(&err, Error::ConnectionFailed(m) if m.contains("password authentication failed")), "{err:?}");

    let closed_port = ConnectionConfig { port: Some(1), ..dev_config() };
    assert!(matches!(block_on(Connection::new(closed_port).connect()), Err(Error::ConnectionFailed(_))));

    // The dev server has no TLS: prefer falls back to plain, require must fail.
    let require = ConnectionConfig { ssl_mode: SslMode::Require, ..dev_config() };
    assert!(matches!(block_on(Connection::new(require).connect()), Err(Error::ConnectionFailed(_))));
}
