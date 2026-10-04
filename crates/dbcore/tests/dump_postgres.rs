//! Postgres dump → restore round trips against the dev server (`scripts/dev-db.sh up`).
//! Skipped unless `DBEAR_TEST_POSTGRES=1`. Each test works in scratch databases of its own
//! (`dbear_dump_*`), created and dropped on the shared dev server; `app_dev` is only read.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use dbcore::dump::{self, CancelToken, Compression, DataStyle, DumpContent, DumpOptions, DumpProgress, DumpScope};
use dbcore::restore::{self, RestoreOptions, RestoreProgress};
use dbcore::{mock, Connection, ConnectionConfig, TableInfo};

const CONTAINER: &str = "dbear-postgres";

fn enabled() -> bool {
    let on = std::env::var("DBEAR_TEST_POSTGRES").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set DBEAR_TEST_POSTGRES=1 (scripts/dev-db.sh up)");
    }
    on
}

fn dev_config() -> ConnectionConfig {
    mock::connections().into_iter().find(|c| c.id == mock::DEV_DATABASE).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

/// A database of its own on the dev server, dropped afterwards.
struct Scratch {
    name: String,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("dbear_dump_{label}_{}_{n}", std::process::id());
        let admin = Connection::new(dev_config());
        block_on(admin.execute(format!("drop database if exists {name} with (force)"))).unwrap();
        block_on(admin.execute(format!("create database {name}"))).unwrap();
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
        let _ = block_on(admin.execute(format!("drop database if exists {} with (force)", self.name)));
    }
}

fn fixture() -> Scratch {
    let db = Scratch::new("src");
    db.execute(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/dump/postgres_fixture.sql")).unwrap());
    db
}

fn run_dump(config: ConnectionConfig, out: &Path, options: DumpOptions) -> dump::DumpSummary {
    block_on(dump::dump(config, out.to_path_buf(), options, Arc::new(|_: &DumpProgress| {}), CancelToken::new())).unwrap()
}

fn run_restore(config: ConnectionConfig, script: &Path) -> dbcore::Result<restore::RestoreSummary> {
    block_on(restore::restore(config, script.to_path_buf(), RestoreOptions::default(), Arc::new(|_: &RestoreProgress| {}), CancelToken::new()))
}

const USER_SCHEMAS: &str = "nspname not in ('information_schema', 'pg_catalog', 'pg_toast') and nspname not like 'pg\\_%'";

/// What a database holds: catalog definitions, then a checksum and count of every table's rows.
fn fingerprint(config: ConnectionConfig) -> Vec<String> {
    let conn = Connection::new(config);
    let catalog = format!(
        "with s as (select oid from pg_namespace where {USER_SCHEMAS})
         select 'col ' || c.oid::regclass || '.' || a.attname || ' ' || format_type(a.atttypid, a.atttypmod) || ' '
                || a.attnotnull || ' ' || coalesce(pg_get_expr(d.adbin, d.adrelid), '') || ' ' || a.attidentity::text || a.attgenerated::text
                || ' ' || coalesce(col_description(c.oid, a.attnum), '') || ' ' || a.attcollation::regcollation
           from pg_attribute a join pg_class c on c.oid = a.attrelid left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
           where c.relnamespace in (select oid from s) and a.attnum > 0 and not a.attisdropped and c.relkind in ('r', 'p', 'v', 'm', 'c')
         union all select 'rel ' || c.oid::regclass || ' ' || c.relkind::text || c.relpersistence::text || c.relrowsecurity::text || ' '
                || coalesce(obj_description(c.oid, 'pg_class'), '') || ' ' || coalesce(array_to_string(c.reloptions, ','), '')
                || ' ' || coalesce(pg_get_expr(c.relpartbound, c.oid), '')
           from pg_class c where c.relnamespace in (select oid from s) and c.relkind in ('r', 'p', 'v', 'm', 'S', 'i', 'I', 'c')
         union all select 'con ' || coalesce(conrelid::regclass::text, contypid::regtype::text) || ' ' || conname || ' ' || pg_get_constraintdef(oid)
           from pg_constraint where connamespace in (select oid from s)
         union all select 'idx ' || pg_get_indexdef(indexrelid) from pg_index i join pg_class c on c.oid = i.indrelid
           where c.relnamespace in (select oid from s)
         union all select 'view ' || oid::regclass || ' ' || pg_get_viewdef(oid) from pg_class
           where relnamespace in (select oid from s) and relkind in ('v', 'm')
         union all select 'type ' || t.oid::regtype || ' ' || t.typtype::text || ' ' || coalesce(format_type(t.typbasetype, t.typtypmod), '')
                || ' ' || coalesce((select string_agg(enumlabel, ',' order by enumsortorder) from pg_enum where enumtypid = t.oid), '')
           from pg_type t where t.typnamespace in (select oid from s) and t.typtype in ('e', 'd', 'r', 'c', 'm')
         union all select 'func ' || p.oid::regprocedure || ' ' || pg_get_functiondef(p.oid) || coalesce(obj_description(p.oid, 'pg_proc'), '')
           from pg_proc p where p.pronamespace in (select oid from s) and p.prokind <> 'a'
         union all select 'trig ' || pg_get_triggerdef(oid) || tgenabled::text from pg_trigger where not tgisinternal
         union all select 'seq ' || schemaname || '.' || sequencename || ' ' || coalesce(last_value, 0) || ' ' || increment_by
           from pg_sequences where schemaname not like 'pg\\_%'
         union all select 'policy ' || tablename || ' ' || policyname || ' ' || coalesce(qual, '') from pg_policies
         union all select 'ext ' || extname from pg_extension
         order by 1"
    );
    let mut lines: Vec<String> = block_on(conn.execute(catalog)).unwrap().rows.iter().map(|r| r[0].display()).collect();
    let tables = block_on(conn.execute(format!(
        "select c.oid::regclass::text from pg_class c join pg_namespace n on n.oid = c.relnamespace
         where c.relkind in ('r', 'm') and {USER_SCHEMAS} order by 1"
    )))
    .unwrap();
    for table in tables.rows.iter().map(|r| r[0].display()) {
        let sums = block_on(conn.execute(format!(
            "select count(*) || ' ' || md5(coalesce(string_agg(t::text, E'\\n' order by t::text), '')) from only {table} t"
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
fn round_trips_the_fixture_with_copy() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dump.sql");
    let summary = run_dump(source.config(), &script, DumpOptions::default());
    assert!(summary.rows >= 900 && summary.warnings.is_empty(), "{summary:?}");

    let target = Scratch::new("dst");
    let restored = run_restore(target.config(), &script).unwrap();
    assert!(restored.rows >= 900, "{restored:?}");
    assert_same(&fingerprint(source.config()), &fingerprint(target.config()));
}

#[test]
fn round_trips_the_fixture_with_inserts_gzipped_and_drops() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dump.sql.gz");
    let options = DumpOptions { data_style: DataStyle::Insert, compression: Compression::Gzip, drop_objects: true, ..Default::default() };
    run_dump(source.config(), &script, options);

    let target = Scratch::new("ins");
    run_restore(target.config(), &script).unwrap();
    // The DROP statements make it restorable over itself.
    run_restore(target.config(), &script).unwrap();
    assert_same(&fingerprint(source.config()), &fingerprint(target.config()));
}

#[test]
fn schema_only_then_data_only() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let (schema, data) = (dir.path().join("schema.sql"), dir.path().join("data.sql"));
    run_dump(source.config(), &schema, DumpOptions { content: DumpContent::SchemaOnly, ..Default::default() });
    run_dump(source.config(), &data, DumpOptions { content: DumpContent::DataOnly, ..Default::default() });
    let target = Scratch::new("split");
    run_restore(target.config(), &schema).unwrap();
    // Data only fires triggers; this one fills in notes, which are all set or null already…
    // except null notes, which it would change: disable it like `pg_dump --disable-triggers`.
    target.execute("alter table shop.orders disable trigger orders_touch");
    run_restore(target.config(), &data).unwrap();
    target.execute("alter table shop.orders enable trigger orders_touch; refresh materialized view shop.mood_counts");
    assert_same(&fingerprint(source.config()), &fingerprint(target.config()));
}

#[test]
fn dumps_selected_tables_with_what_they_need() {
    if !enabled() {
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("tables.sql");
    let scope = DumpScope::Tables(vec![TableInfo::new("shop", "events"), TableInfo::new("we\"ird", "Mixed Case")]);
    let summary = run_dump(source.config(), &script, DumpOptions { scope, ..Default::default() });
    // The partitioned table brings its partitions.
    assert_eq!(summary.tables, 3, "{summary:?}");
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.contains("PARTITION OF shop.events") && !text.contains("customers"), "{text}");

    let target = Scratch::new("tables");
    run_restore(target.config(), &script).unwrap();
    let fingerprint = fingerprint(target.config());
    assert!(fingerprint.iter().any(|l| l.starts_with("data shop.events_2025 187 ")), "{fingerprint:#?}");
    assert!(fingerprint.iter().any(|l| l.starts_with("data shop.events_2024 213 ")), "{fingerprint:#?}");
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
    run_restore(target.config(), &script).unwrap();
    assert_same(&fingerprint(dev_config()), &fingerprint(target.config()));
}

/// `container exec` into the dev server, when the CLI is there.
fn in_container(args: &[&str], stdin: Option<&Path>) -> Option<std::process::Output> {
    let mut command = Command::new("container");
    command.arg("exec").args(if stdin.is_some() { vec!["-i"] } else { vec![] }).arg(CONTAINER).args(args);
    if let Some(path) = stdin {
        command.stdin(Stdio::from(std::fs::File::open(path).unwrap()));
    }
    command.output().ok().filter(|o| o.status.success() || !o.stderr.is_empty())
}

#[test]
fn compatible_with_psql_and_pg_dump() {
    if !enabled() {
        return;
    }
    if in_container(&["true"], None).is_none_or(|o| !o.status.success()) {
        eprintln!("skipped: can't `container exec` into {CONTAINER}");
        return;
    }
    let source = fixture();
    let dir = tempfile::tempdir().unwrap();

    // Our dump, loaded by psql.
    let script = dir.path().join("dump.sql");
    run_dump(source.config(), &script, DumpOptions::default());
    let by_psql = Scratch::new("psql");
    let output = in_container(&["psql", "-q", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", &by_psql.name], Some(&script)).unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_same(&fingerprint(source.config()), &fingerprint(by_psql.config()));

    // pg_dump's plain output, restored by us.
    let output = in_container(&["pg_dump", "-U", "postgres", "--no-owner", "--no-privileges", &source.name], None).unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let theirs = dir.path().join("pg_dump.sql");
    std::fs::write(&theirs, &output.stdout).unwrap();
    let from_pg_dump = Scratch::new("pgdump");
    run_restore(from_pg_dump.config(), &theirs).unwrap();
    assert_same(&fingerprint(source.config()), &fingerprint(from_pg_dump.config()));
}

#[test]
fn cancels_a_dump_quickly() {
    if !enabled() {
        return;
    }
    let source = Scratch::new("big");
    source.execute("create table big as select i, md5(i::text) as h from generate_series(1, 2000000) i");
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("big.sql");
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let cancelled_at = Arc::new(std::sync::Mutex::new(None));
    let at = cancelled_at.clone();
    let progress = Arc::new(move |p: &DumpProgress| {
        if p.rows_done > 10_000 && !trigger.is_cancelled() {
            *at.lock().unwrap() = Some(std::time::Instant::now());
            trigger.cancel();
        }
    });
    let result = block_on(dump::dump(source.config(), script.clone(), DumpOptions::default(), progress, cancel));
    assert_eq!(result, Err(dbcore::Error::Cancelled));
    let elapsed = cancelled_at.lock().unwrap().unwrap().elapsed();
    assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
    assert!(!script.exists() && !dump::partial_path(&script).exists());
}
