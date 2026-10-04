//! Times a 500-row page deep into a table: OFFSET (`fetch_rows`) vs keyset (`fetch_page`).
//!
//! ```sh
//! cargo run --release -p dbcore --example bench_paging -- <url> <schema> <table> [column[:desc]…]
//! ```
//! e.g. `postgres://postgres:postgres@localhost:54329/app_dev analytics events user_id:desc`.
//! `scripts/bench-paging.sh` seeds big tables in throwaway containers and runs this.

use std::time::{Duration, Instant};

use dbcore::{Connection, ConnectionConfig, PageCursor, RowQuery, SortKey, TableInfo};

const PAGE: u32 = 500;
const STEP: u32 = 20_000;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut best = Duration::MAX;
    let mut out = None;
    for _ in 0..n {
        let start = Instant::now();
        let value = f();
        best = best.min(start.elapsed());
        out = Some(value);
    }
    (best, out.unwrap())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, schema, name, sort @ ..] = args.as_slice() else {
        eprintln!("usage: bench_paging <url> <schema> <table> [column[:desc]…]");
        std::process::exit(2);
    };
    let mut config = ConnectionConfig::from_url(url).expect("connection URL");
    config.id = "bench".into();
    if config.password.is_none() {
        config.password = std::env::var("PGPASSWORD").ok();
    }
    let conn = Connection::new(config);
    let table = TableInfo::new(schema.as_str(), name.as_str());
    let query = RowQuery {
        sort: sort
            .iter()
            .map(|s| match s.split_once(':') {
                Some((c, d)) => SortKey { column: c.into(), descending: d == "desc" },
                None => SortKey { column: s.clone(), descending: false },
            })
            .collect(),
        filter: None,
    };

    let first = block_on(conn.fetch_page(table.clone(), query.clone(), PAGE, None)).expect("first page");
    let total = first.result.total_count.unwrap_or(0);
    println!("{}.{} sort {:?}: ~{total} rows", schema, name, sort);
    println!("{:>10}  {:>10}  {:>10}  keyset?", "depth", "offset ms", "keyset ms");

    let depths: Vec<u64> = [0u64, 10_000, 100_000, 500_000, total.saturating_sub(1_000)]
        .into_iter()
        .filter(|&d| d == 0 || d < total)
        .collect();
    // Walk forward with big keyset pages to get a cursor at each depth.
    let mut cursor: Option<PageCursor> = None;
    let mut at = 0u64;
    for depth in depths {
        while at < depth {
            let step = STEP.min((depth - at) as u32);
            let page = block_on(conn.fetch_page(table.clone(), query.clone(), step, cursor.clone())).expect("walk");
            at += page.result.rows.len() as u64;
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        if at < depth {
            break;
        }
        let (offset, a) = best_of(3, || block_on(conn.fetch_rows_with(table.clone(), query.clone(), PAGE, depth)));
        let (keyset, b) = best_of(3, || block_on(conn.fetch_page(table.clone(), query.clone(), PAGE, cursor.clone())));
        let cell = |d: Duration, ok: bool| if ok { format!("{:.1}", ms(d)) } else { "error".into() };
        if let (Ok(a), Ok(b)) = (&a, &b) {
            assert_eq!(a.rows, b.result.rows, "pages differ at {depth}");
        }
        for e in [a.as_ref().err(), b.as_ref().err()].into_iter().flatten() {
            eprintln!("  at {depth}: {e}");
        }
        let seeks = cursor.as_ref().is_some_and(PageCursor::is_keyset);
        println!("{depth:>10}  {:>10}  {:>10}  {}", cell(offset, a.is_ok()), cell(keyset, b.is_ok()), if seeks { "yes" } else { "no" });
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
