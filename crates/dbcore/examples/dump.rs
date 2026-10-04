//! Dumps or restores a database from the command line, e.g.
//!   cargo run -p dbcore --example dump -- dump postgres://postgres:postgres@localhost:54329/app_dev app_dev.sql.gz
//!   cargo run -p dbcore --example dump -- restore sqlite:///tmp/copy.db app.sql
//! A `.gz` output is gzipped.

use std::sync::Arc;

use dbcore::dump::{self, CancelToken, Compression, DumpOptions, DumpProgress};
use dbcore::restore::{self, RestoreOptions, RestoreProgress};
use dbcore::ConnectionConfig;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, url, path] = args.as_slice() else {
        eprintln!("usage: dump (dump|restore) <url> <file>");
        std::process::exit(2);
    };
    let config = ConnectionConfig::from_url(url).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let result = match command.as_str() {
        "dump" => {
            let compression = if path.ends_with(".gz") { Compression::Gzip } else { Compression::None };
            let options = DumpOptions { compression, ..Default::default() };
            let progress = Arc::new(|p: &DumpProgress| {
                eprint!("\r{:?} {}/{} tables, {} rows   ", p.phase, p.tables_done, p.tables_total, p.rows_done)
            });
            runtime
                .block_on(dump::dump(config, path.into(), options, progress, CancelToken::new()))
                .map(|s| format!("{} tables, {} rows, {} bytes, warnings: {:?}", s.tables, s.rows, s.bytes, s.warnings))
        }
        "restore" => {
            let progress = Arc::new(|p: &RestoreProgress| eprint!("\r{}/{} bytes, {} statements   ", p.bytes_read, p.bytes_total, p.statements));
            runtime
                .block_on(restore::restore(config, path.into(), RestoreOptions::default(), progress, CancelToken::new()))
                .map(|s| format!("{} statements, {} rows, warnings: {:?}", s.statements, s.rows, s.warnings))
        }
        _ => {
            eprintln!("usage: dump (dump|restore) <url> <file>");
            std::process::exit(2);
        }
    };
    eprintln!();
    match result {
        Ok(summary) => println!("{summary}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
