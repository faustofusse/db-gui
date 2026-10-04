//! Integration tests against the dev SQL Server (`scripts/dev-db.sh up sqlserver`).
//! Skipped unless `DBEAR_TEST_SQLSERVER=1`; `scripts/test-sqlserver.sh` sets it up.

use std::time::{Duration, Instant};

use dbcore::edit::{CellEdit, EditValue, KeyValue, RowChange};
use dbcore::{mock, Connection, ConnectionConfig, Error, RowQuery, SortKey, SslMode, TableInfo, TableKind, Value};

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

fn dev() -> Connection {
    Connection::new(dev_config())
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn column<'a>(result: &'a dbcore::QueryResult, name: &str) -> Vec<&'a Value> {
    let i = result.columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no column {name}"));
    result.rows.iter().map(|r| &r[i]).collect()
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[test]
fn lists_databases_and_switches_between_them() {
    if !enabled() {
        return;
    }
    let names = block_on(dev().list_databases()).unwrap();
    assert!(names.contains(&"app_dev".to_string()) && names.contains(&"archive".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| ["master", "model", "msdb", "tempdb"].contains(&n.as_str())), "system databases hidden: {names:?}");

    // An empty database opens master, which is then listed.
    let master = Connection::new(ConnectionConfig { database: String::new(), ..dev_config() });
    assert!(block_on(master.list_databases()).unwrap().contains(&"master".to_string()));

    let archive = Connection::new(dev_config().with_database("archive"));
    let schemas = block_on(archive.list_schemas()).unwrap();
    assert_eq!(schemas.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["dbo"]);
    assert!(schemas[0].tables.is_empty());
}

#[test]
fn lists_schemas_tables_and_columns() {
    if !enabled() {
        return;
    }
    let schemas = block_on(dev().list_schemas()).unwrap();
    let names: Vec<_> = schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["dbo", "empty_schema", "sales"], "role schemas, sys, guest hidden; empty ones kept");
    let sales = schemas.iter().find(|s| s.name == "sales").unwrap();
    let tables: Vec<_> = sales.tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(tables, [("order_items", TableKind::Table), ("orders", TableKind::Table), ("paid_orders", TableKind::View)]);
    let customers = schemas[0].tables.iter().find(|t| t.name == "customers").unwrap();
    assert_eq!(customers.estimated_row_count, Some(250));
    assert_eq!(sales.tables[2].estimated_row_count, None);

    let columns = block_on(dev().list_columns()).unwrap();
    let customers = columns.iter().find(|t| t.schema == "dbo" && t.table == "customers").unwrap();
    let id = &customers.columns[0];
    assert_eq!((id.name.as_str(), id.type_name.as_str(), id.is_primary_key, id.is_nullable), ("id", "int", true, false));
    assert_eq!(customers.columns[1].type_name, "nvarchar(255)");
}

#[test]
fn pages_tables_with_typed_values() {
    if !enabled() {
        return;
    }
    let customers = TableInfo::new("dbo", "customers");
    let first = block_on(dev().fetch_rows(customers.clone(), 100, 0)).unwrap();
    assert_eq!((first.rows.len(), first.total_count), (100, Some(250)));
    assert_eq!(column(&first, "id")[..3], [&Value::Int(1), &Value::Int(2), &Value::Int(3)]);
    assert_eq!(column(&first, "is_active")[6], &Value::Bool(false)); // id 7
    assert_eq!(column(&first, "balance")[0], &Value::Decimal("37.13".into()));
    assert_eq!(column(&first, "created_at")[0], &text("2025-01-01 01:00:00.000"));
    assert!(first.columns[0].is_primary_key);

    let last = block_on(dev().fetch_rows(customers, 100, 200)).unwrap();
    assert_eq!((last.rows.len(), last.total_count), (50, None));
    assert_eq!(column(&last, "id")[0], &Value::Int(201));
}

#[test]
fn decodes_every_type() {
    if !enabled() {
        return;
    }
    let page = block_on(dev().fetch_rows(TableInfo::new("dbo", "types"), 10, 0)).unwrap();
    let cell = |name: &str| column(&page, name)[0].clone();
    let types: Vec<(&str, &str)> = page.columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str())).collect();
    assert!(types.contains(&("exact", "decimal(38,10)")) && types.contains(&("name", "nvarchar(100)")), "{types:?}");
    assert!(types.contains(&("memo", "nvarchar(max)")) && types.contains(&("version", "rowversion")), "{types:?}");
    assert_eq!(cell("tiny"), Value::Int(255));
    assert_eq!(cell("small"), Value::Int(-32768));
    assert_eq!(cell("big"), Value::Int(i64::MAX));
    assert_eq!(cell("flag"), Value::Bool(true));
    assert_eq!(cell("real_num"), Value::Float(0.1));
    assert_eq!(cell("float_num"), Value::Float(std::f64::consts::PI));
    assert_eq!(cell("exact"), Value::Decimal("1234567890123456789012345678.9012345678".into()));
    assert_eq!(cell("price"), Value::Decimal("922337203685477.5807".into()));
    assert_eq!(cell("small_price"), Value::Decimal("-214748.3648".into()));
    assert_eq!(cell("guid"), text("6F9619FF-8B86-D011-B42D-00C04FC964FF"));
    assert_eq!(cell("name"), text("Zoë 日本語 🐻"));
    assert_eq!(cell("ascii_name"), text("plain"));
    assert_eq!(cell("fixed"), text("abc"));
    assert_eq!(cell("memo").display().len(), 5000);
    assert_eq!(cell("payload"), text("0xdeadbeef"));
    assert_eq!(cell("bin"), text("0x00010203"));
    assert_eq!(cell("born"), text("2024-01-02"));
    assert_eq!(cell("alarm"), text("03:04:05.1234567"));
    assert_eq!(cell("legacy"), text("2024-01-02 03:04:05.003"));
    assert_eq!(cell("small_legacy"), text("2024-01-02 03:04:00"));
    assert_eq!(cell("happened"), text("2024-01-02 03:04:05.1234567"));
    assert_eq!(cell("happened_tz"), text("2024-01-02 03:04:05.1234567 +02:00"));
    assert_eq!(cell("doc"), text(r#"<a b="1">x</a>"#));
    assert_eq!(cell("variant"), Value::Int(42));
    assert!(matches!(cell("version"), Value::Text(t) if t.starts_with("0x") && t.len() == 18));

    // NULLs everywhere, and exact negative fractions.
    let nulls = &page.rows[1];
    assert_eq!(column(&page, "exact")[1], &Value::Decimal("-0.5000000000".into()));
    assert_eq!(nulls.iter().filter(|v| v.is_null()).count(), 20);
}

#[test]
fn pages_composite_keyless_and_view_tables() {
    if !enabled() {
        return;
    }
    let items = block_on(dev().fetch_rows(TableInfo::new("sales", "order_items"), 10, 0)).unwrap();
    assert_eq!(column(&items, "line"), [&Value::Int(1), &Value::Int(2), &Value::Int(1)]);

    // No primary key: ordered by the unique index on seq.
    let log = block_on(dev().fetch_rows(TableInfo::new("dbo", "audit_log"), 10, 30)).unwrap();
    assert_eq!(column(&log, "seq")[0], &Value::Int(31));
    let first = block_on(dev().fetch_rows(TableInfo::new("dbo", "audit_log"), 10, 0)).unwrap();
    assert_eq!((first.rows.len(), first.total_count), (10, Some(40)));

    // A heap still pages (ORDER BY (SELECT NULL)).
    let heap = block_on(dev().fetch_rows(TableInfo::new("dbo", "heap_notes"), 10, 0)).unwrap();
    assert_eq!((heap.rows.len(), heap.total_count), (3, Some(3)));

    let view = block_on(dev().fetch_rows(TableInfo::new("sales", "paid_orders"), 500, 0)).unwrap();
    assert!(!view.rows.is_empty() && view.total_count.is_none());
    let err = block_on(dev().fetch_rows(TableInfo::new("dbo", "nope"), 10, 0)).unwrap_err();
    assert_eq!(err, Error::TableNotFound("dbo.nope".into()));
}

#[test]
fn sorts_filters_and_rejects_smuggled_statements() {
    if !enabled() {
        return;
    }
    let customers = TableInfo::new("dbo", "customers");
    let query = RowQuery {
        sort: vec![SortKey { column: "balance".into(), descending: true }],
        filter: Some("is_active = 1 and name like N'Customer 1%'".into()),
    };
    let page = block_on(dev().fetch_rows_with(customers.clone(), query, 500, 0)).unwrap();
    assert!(!page.rows.is_empty());
    let balances: Vec<f64> = column(&page, "balance").iter().map(|v| v.display().parse().unwrap()).collect();
    assert!(balances.windows(2).all(|w| w[0] >= w[1]));
    assert_eq!(page.total_count, Some(page.rows.len() as u64));

    // T-SQL runs several statements without `;`: the driver keeps the filter inside its parentheses.
    for evil in ["1=1) delete from dbo.customers where (1=1", r"name = 'x\') delete from dbo.customers where (1=1 --'"] {
        let filter = RowQuery { filter: Some(evil.into()), ..Default::default() };
        let err = block_on(dev().fetch_rows_with(customers.clone(), filter, 1, 0)).unwrap_err();
        assert!(matches!(err, Error::Query(_)), "{evil}: {err:?}");
    }
    assert_eq!(block_on(dev().fetch_rows(customers, 1, 0)).unwrap().total_count, Some(250));
}

#[test]
fn runs_scripts_with_batches() {
    if !enabled() {
        return;
    }
    let conn = dev();
    // The last result set wins; GO separates batches; temp tables live on in the session.
    let r = block_on(conn.execute(
        "create table #t (id int, name nvarchar(10))\nGO\ninsert #t values (1, N'a'), (2, N'b')\nselect count(*) as n from #t\nselect name from #t order by id".into(),
    ))
    .unwrap();
    assert_eq!(column(&r, "name"), [&text("a"), &text("b")]);
    assert_eq!(r.columns[0].type_name, "nvarchar");

    let r = block_on(conn.execute("update #t set name = N'z'".into())).unwrap();
    assert_eq!((r.rows_affected, r.columns.len()), (Some(2), 0));
    let r = block_on(conn.execute("insert #t values (3, N'c')\nGO 3\nselect count(*) from #t".into())).unwrap();
    assert_eq!(r.rows, [[Value::Int(5)]]);
    assert_eq!(r.columns[0].name, "(No column name)");

    let r = block_on(conn.execute_limited("select top 5000 a.object_id from sys.all_objects a cross join sys.all_objects b".into(), Some(1000))).unwrap();
    assert_eq!((r.rows.len(), r.truncated, r.total_count), (1000, true, Some(5000)));
}

#[test]
fn reports_server_errors_with_script_lines() {
    if !enabled() {
        return;
    }
    let err = block_on(dev().execute("select 1\nGO\n\nselect * from dbo.nope".into())).unwrap_err();
    assert_eq!(err, Error::Query("Msg 208, Level 16, State 1, Line 4\nInvalid object name 'dbo.nope'.".into()));
    let err = block_on(dev().execute("select 1\nselec 2 from".into())).unwrap_err();
    let Error::Query(message) = err else { panic!("{err:?}") };
    assert!(message.starts_with("Msg 102, Level 15, State 1, Line 2"), "{message}");
}

#[test]
fn cancels_long_scripts() {
    if !enabled() {
        return;
    }
    let conn = dev();
    block_on(conn.connect()).unwrap();
    let started = Instant::now();
    let runner = conn.clone();
    let handle = std::thread::spawn(move || block_on(runner.execute("waitfor delay '00:00:30'; select 1".into())));
    std::thread::sleep(Duration::from_millis(800));
    block_on(conn.cancel());
    assert_eq!(handle.join().unwrap().unwrap_err(), Error::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(10));
    // The session survives the attention.
    assert_eq!(block_on(conn.execute("select 2".into())).unwrap().rows, [[Value::Int(2)]]);

    // Dropping the future also stops it (the connection is dropped and reopened).
    let dropped = block_on(async {
        tokio::time::timeout(Duration::from_millis(500), conn.execute("waitfor delay '00:00:30'".into())).await
    });
    assert!(dropped.is_err());
    assert_eq!(block_on(conn.execute("select 3".into())).unwrap().rows, [[Value::Int(3)]]);
}

#[test]
fn connects_with_each_ssl_mode_and_reports_auth_errors() {
    if !enabled() {
        return;
    }
    // Disable only encrypts the login; the others encrypt the whole session.
    for (mode, encrypted) in [(SslMode::Disable, "FALSE"), (SslMode::Prefer, "TRUE"), (SslMode::Require, "TRUE")] {
        let config = ConnectionConfig { ssl_mode: mode, ..dev_config() };
        let sql = "select encrypt_option from sys.dm_exec_connections where session_id = @@spid";
        let r = block_on(Connection::new(config).execute(sql.into())).unwrap_or_else(|e| panic!("{mode:?}: {e}"));
        assert_eq!(r.rows, [[text(encrypted)]], "{mode:?}");
    }
    // The container's certificate is self-signed.
    let verify = ConnectionConfig { ssl_mode: SslMode::VerifyFull, ..dev_config() };
    let err = block_on(Connection::new(verify).connect()).unwrap_err();
    assert!(matches!(err, Error::ConnectionFailed(ref m) if m.contains("certificate")), "{err:?}");
    eprintln!("verify-full: {err}");

    let wrong = ConnectionConfig { password: Some("wrong".into()), ..dev_config() };
    let err = block_on(Connection::new(wrong).connect()).unwrap_err();
    assert_eq!(err, Error::ConnectionFailed("Login failed for user 'sa'.".into()));
}

#[test]
fn reports_and_closes_connections() {
    if !enabled() {
        return;
    }
    let conn = dev();
    assert!(!block_on(conn.is_connected()));
    block_on(conn.connect()).unwrap();
    assert!(block_on(conn.is_connected()));
    block_on(conn.disconnect());
    assert!(!block_on(conn.is_connected()));
    assert!(!block_on(conn.list_schemas()).unwrap().is_empty());
}

#[test]
fn describes_tables_and_views() {
    if !enabled() {
        return;
    }
    let orders = block_on(dev().describe_table(TableInfo::new("sales", "orders"))).unwrap();
    assert_eq!(orders.primary_key, ["id"]);
    let column = |name: &str| orders.columns.iter().find(|c| c.name == name).unwrap();
    assert_eq!(column("id").default_value.as_deref(), Some("IDENTITY(1,1)"));
    assert_eq!(column("status").default_value.as_deref(), Some("('pending')"));
    assert_eq!(column("tax").default_value.as_deref(), Some("AS ([total]*(0.2)) PERSISTED"));
    let fk = &orders.foreign_keys[0];
    assert_eq!((fk.name.as_str(), fk.referenced_schema.as_str(), fk.referenced_table.as_str()), ("fk_orders_customers", "dbo", "customers"));
    assert_eq!((fk.on_delete.as_str(), fk.on_update.as_str()), ("CASCADE", "NO ACTION"));
    assert!(orders.indexes.iter().any(|i| i.is_primary && i.columns == ["id"]));
    let ix = orders.indexes.iter().find(|i| i.name == "ix_orders_status").unwrap();
    assert_eq!(ix.columns, ["status DESC"]);
    assert_eq!(
        ix.definition.as_deref(),
        Some("CREATE NONCLUSTERED INDEX [ix_orders_status] ON [sales].[orders] ([status] DESC) INCLUDE ([total]) WHERE ([status]<>'cancelled');")
    );
    let ddl = orders.ddl.unwrap();
    assert!(ddl.starts_with("CREATE TABLE [sales].[orders] (\n    [id] bigint IDENTITY(1,1) NOT NULL,"), "{ddl}");
    assert!(ddl.contains("CONSTRAINT [ck_orders_total] CHECK ([total]>=(0))"), "{ddl}");
    assert!(ddl.contains("REFERENCES [dbo].[customers] ([id]) ON DELETE CASCADE"), "{ddl}");

    let customers = block_on(dev().describe_table(TableInfo::new("dbo", "customers"))).unwrap();
    let email = customers.columns.iter().find(|c| c.name == "email").unwrap();
    assert_eq!(email.comment.as_deref(), Some("Login e-mail"));
    assert!(customers.indexes.iter().any(|i| i.is_unique && i.columns == ["email"]));

    let items = block_on(dev().describe_table(TableInfo::new("sales", "order_items"))).unwrap();
    assert_eq!(items.primary_key, ["order_id", "line"]);

    let view = block_on(dev().describe_table(TableInfo::new("sales", "paid_orders"))).unwrap();
    assert!(view.ddl.unwrap().starts_with("create view sales.paid_orders as"));
    assert!(matches!(block_on(dev().describe_table(TableInfo::new("dbo", "nope"))), Err(Error::TableNotFound(_))));
}

// MARK: Editing

fn edit_key(id: i64) -> Vec<KeyValue> {
    vec![KeyValue { column: "id".into(), value: Value::Int(id) }]
}

fn edit_set(column: &str, value: EditValue) -> CellEdit {
    CellEdit { column: column.into(), value }
}

fn edit_text(s: &str) -> EditValue {
    EditValue::Text(s.into())
}

#[test]
fn saves_row_edits_in_one_transaction() {
    if !enabled() {
        return;
    }
    let conn = dev();
    // A trigger with its own row count: each statement must still report exactly its row.
    block_on(conn.execute(
        "drop table if exists dbo.dbear_edit_test; drop table if exists dbo.dbear_edit_audit;
         create table dbo.dbear_edit_test (
           id int identity primary key, name nvarchar(50) not null unique,
           n int, active bit not null default 1, note nvarchar(20) default N'hi', born datetime);
         create table dbo.dbear_edit_audit (id int identity primary key, person_id int);
         insert dbo.dbear_edit_test (name) values (N'a'), (N'b'), (N'c');
         GO
         create trigger dbo.tr_dbear_edit_test on dbo.dbear_edit_test after insert, update, delete as
           insert dbo.dbear_edit_audit (person_id) select id from inserted union all select id from deleted;"
            .into(),
    ))
    .unwrap();
    let table = TableInfo::new("dbo", "dbear_edit_test");
    let names = || -> Vec<String> {
        let page = block_on(conn.fetch_rows(table.clone(), 100, 0)).unwrap();
        column(&page, "name").iter().map(|v| v.display()).collect()
    };
    let columns = block_on(conn.fetch_rows(table.clone(), 1, 0)).unwrap().columns;
    let apply = |changes: Vec<RowChange>| block_on(conn.apply_changes(table.clone(), columns.clone(), changes));

    let affected = apply(vec![
        RowChange::Update {
            key: edit_key(1),
            set: vec![
                edit_set("name", edit_text("Ada ☃")),
                edit_set("n", EditValue::Null),
                edit_set("active", edit_text("false")),
                // Read as y-m-d whatever the login's language.
                edit_set("born", edit_text("2024-01-02 03:04:05.003")),
            ],
        },
        RowChange::Delete { key: edit_key(2) },
        RowChange::Insert { values: vec![edit_set("id", EditValue::Default), edit_set("name", edit_text("Grace")), edit_set("n", edit_text("7"))] },
    ])
    .unwrap();
    assert_eq!(affected, 3);
    assert_eq!(names(), ["Ada ☃", "c", "Grace"]);
    let page = block_on(conn.fetch_rows(table.clone(), 100, 0)).unwrap();
    assert_eq!(column(&page, "n")[0], &Value::Null);
    assert_eq!(column(&page, "n")[2], &Value::Int(7));
    assert_eq!(column(&page, "note")[2], &text("hi"), "defaults fill omitted columns");
    assert_eq!(column(&page, "active")[0], &Value::Bool(false));
    assert_eq!(column(&page, "born")[0], &text("2024-01-02 03:04:05.003"));

    // Resetting to the default works in an UPDATE.
    assert_eq!(apply(vec![RowChange::Update { key: edit_key(1), set: vec![edit_set("active", EditValue::Default)] }]).unwrap(), 1);

    // A vanished row fails the whole batch: the other update is rolled back too.
    let err = apply(vec![
        RowChange::Update { key: edit_key(3), set: vec![edit_set("name", edit_text("changed"))] },
        RowChange::Update { key: edit_key(2), set: vec![edit_set("name", edit_text("ghost"))] },
    ])
    .unwrap_err();
    assert!(matches!(&err, Error::Query(m) if m.contains("No row matches") && m.contains("Nothing was saved")), "{err:?}");
    assert_eq!(names(), ["Ada ☃", "c", "Grace"]);

    // Server errors (unique violation) name the row and roll everything back.
    let err = apply(vec![
        RowChange::Update { key: edit_key(3), set: vec![edit_set("name", edit_text("changed"))] },
        RowChange::Insert { values: vec![edit_set("name", edit_text("Grace"))] },
    ])
    .unwrap_err();
    assert!(matches!(&err, Error::Query(m) if m.starts_with("Couldn’t save a new row") && m.contains("Msg 2627") && m.ends_with("Nothing was saved.")), "{err:?}");
    assert_eq!(names(), ["Ada ☃", "c", "Grace"]);

    // Still usable afterwards.
    assert_eq!(apply(vec![RowChange::Delete { key: edit_key(3) }]).unwrap(), 1);
    block_on(conn.execute("drop table dbo.dbear_edit_test; drop table dbo.dbear_edit_audit".into())).unwrap();
}
