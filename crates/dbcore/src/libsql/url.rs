//! Turso / libSQL connection URLs: `libsql://db-org.turso.io?authToken=…`, `https://…`, `http://localhost:8080`.
//!
//! Mapping onto [`ConnectionConfig`] (no libSQL-specific fields):
//! - `host` / `port`: the server.
//! - `password`: the auth token (kept in the platform keychain like any password).
//! - `ssl_mode`: `Disable` → `http://`; `Prefer`/`Require` → `https://` without verifying the certificate
//!   (self-hosted `sqld` with a self-signed one); `VerifyFull` (the default) → `https://`, verified.
//! - `user` and `database` are unused: a URL is one database.

use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use url::Url;

use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind, SslMode};

/// Parses a `libsql:`, `https:`, `http:`, `wss:` or `ws:` URL. `libsql:` URLs without a host (a local
/// file, `libsql:///path/app.db`) become SQLite connections: a libSQL file is a SQLite file.
pub(crate) fn from_url(url: &Url) -> Result<ConnectionConfig> {
    let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
    let host = url.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']').to_string();
    if host.is_empty() {
        if url.scheme() == "libsql" && url.path().len() > 1 {
            let mut config = ConnectionConfig::new_empty(DatabaseKind::Sqlite);
            config.database = decode(url.path());
            config.name = config.default_name();
            return Ok(config);
        }
        return Err(Error::InvalidConfig("The URL has no host.".into()));
    }

    let mut config = ConnectionConfig::new_empty(DatabaseKind::Libsql);
    config.host = host;
    config.port = url.port();
    config.ssl_mode = match url.scheme() {
        "http" | "ws" => SslMode::Disable,
        _ => SslMode::VerifyFull,
    };
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "authToken" | "auth_token" | "token" => config.password = Some(value.into_owned()).filter(|t| !t.is_empty()),
            "tls" if value == "0" || value == "false" => config.ssl_mode = SslMode::Disable,
            "tls" if (value == "1" || value == "true") && config.ssl_mode == SslMode::Disable => {
                config.ssl_mode = SslMode::VerifyFull;
            }
            _ => {}
        }
    }
    // Some tools put the token in the userinfo (`libsql://:token@host`).
    if config.password.is_none() {
        config.password = url.password().map(decode).filter(|t| !t.is_empty());
    }
    config.name = config.default_name();
    Ok(config)
}

/// `libsql://host[:port][?tls=0][&authToken=…]`. The token is only included when asked for.
pub(crate) fn to_url(config: &ConnectionConfig, include_token: bool) -> String {
    let mut query = Vec::new();
    if config.ssl_mode == SslMode::Disable {
        query.push("tls=0".to_string());
    }
    if let (true, Some(token)) = (include_token, config.password.as_deref().filter(|t| !t.is_empty())) {
        query.push(format!("authToken={}", utf8_percent_encode(token, NON_ALPHANUMERIC)));
    }
    let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };
    format!("libsql://{}{query}", authority(config))
}

/// The base URL requests go to, e.g. `https://db-org.turso.io` or `http://localhost:8080`.
pub(crate) fn base_url(config: &ConnectionConfig) -> String {
    let scheme = if config.ssl_mode == SslMode::Disable { "http" } else { "https" };
    format!("{scheme}://{}", authority(config))
}

fn authority(config: &ConnectionConfig) -> String {
    let host = config.host.trim();
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    match config.port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

pub(crate) fn validate(config: &ConnectionConfig) -> Result<()> {
    let invalid = |msg: &str| Err(Error::InvalidConfig(msg.into()));
    let host = config.host.trim();
    if host.is_empty() {
        return invalid("Enter the database host, like mydb-org.turso.io.");
    }
    if host.contains('/') || host.contains('?') || host.contains(char::is_whitespace) {
        return invalid("Enter just the host, like mydb-org.turso.io (paste full URLs in the URL field).");
    }
    if config.port == Some(0) {
        return invalid("Port must be between 1 and 65535.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(url: &str) -> ConnectionConfig {
        ConnectionConfig::from_url(url).unwrap()
    }

    #[test]
    fn parses_turso_urls() {
        let c = parse("libsql://mydb-acme.aws-us-east-1.turso.io?authToken=eyJ.abc-_.x");
        assert_eq!(c.kind, DatabaseKind::Libsql);
        assert_eq!((c.host.as_str(), c.port, c.ssl_mode), ("mydb-acme.aws-us-east-1.turso.io", None, SslMode::VerifyFull));
        assert_eq!(c.password.as_deref(), Some("eyJ.abc-_.x"));
        assert_eq!((c.name.as_str(), c.user.as_deref(), c.database.as_str()), ("mydb-acme", None, ""));
        assert_eq!(c.summary(), "Turso · mydb-acme.aws-us-east-1.turso.io");

        let local = parse("http://127.0.0.1:8080");
        assert_eq!((local.kind, local.port, local.ssl_mode, local.password), (DatabaseKind::Libsql, Some(8080), SslMode::Disable, None));
        assert_eq!(parse("libsql://localhost:8080?tls=0").ssl_mode, SslMode::Disable);
        assert_eq!(parse("https://db.example.com").ssl_mode, SslMode::VerifyFull);
        assert_eq!(parse("http://db.example.com?tls=1").ssl_mode, SslMode::VerifyFull);
        assert_eq!(parse("wss://db.example.com").kind, DatabaseKind::Libsql);
        assert_eq!(parse("libsql://[::1]:8080?tls=0").host, "::1");
    }

    #[test]
    fn local_libsql_files_are_sqlite() {
        let c = parse("libsql:///Users/me/app.db");
        assert_eq!((c.kind, c.database.as_str()), (DatabaseKind::Sqlite, "/Users/me/app.db"));
        assert!(ConnectionConfig::from_url("https://").is_err());
    }

    #[test]
    fn formats_urls() {
        let c = parse("libsql://db.turso.io?authToken=a%2Bb");
        assert_eq!(c.password.as_deref(), Some("a+b"));
        assert_eq!(c.to_url(false), "libsql://db.turso.io");
        assert_eq!(c.to_url(true), "libsql://db.turso.io?authToken=a%2Bb");
        assert_eq!(parse(&c.to_url(true)), c);
        let local = parse("http://localhost:8080");
        assert_eq!(local.to_url(false), "libsql://localhost:8080?tls=0");
        assert_eq!(parse(&local.to_url(false)), local);
        assert_eq!(base_url(&local), "http://localhost:8080");
        assert_eq!(base_url(&c), "https://db.turso.io");
        assert_eq!(base_url(&parse("libsql://[::1]:9000")), "https://[::1]:9000");
    }

    #[test]
    fn store_keeps_the_token_off_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbear.db");
        let mut store = crate::ConnectionStore::open(&path).unwrap();
        let saved = store.upsert(parse("libsql://mydb-acme.turso.io?authToken=sup3r-s3cret-token")).unwrap();
        assert_eq!((saved.kind, saved.password.as_deref(), saved.name.as_str()), (DatabaseKind::Libsql, None, "mydb-acme"));
        drop(store);
        assert_eq!(crate::ConnectionStore::open(&path).unwrap().connections(), [saved]);
        for file in std::fs::read_dir(dir.path()).unwrap() {
            let bytes = std::fs::read(file.unwrap().path()).unwrap();
            assert!(!bytes.windows(6).any(|w| w == b"s3cret"), "token leaked to disk");
        }
    }

    #[test]
    fn same_target_ignores_the_token() {
        let a = parse("libsql://db.turso.io?authToken=a");
        let b = parse("https://DB.turso.io:443");
        assert!(crate::import::same_target(&a, &b));
        assert!(!crate::import::same_target(&a, &parse("libsql://other.turso.io")));
    }

    #[test]
    fn validates() {
        let mut c = ConnectionConfig::new_empty(DatabaseKind::Libsql);
        assert_eq!((c.ssl_mode, c.show_all_databases, c.host.as_str()), (SslMode::VerifyFull, false, ""));
        assert!(c.validate().is_err());
        c.host = "db.turso.io".into();
        assert!(c.validate().is_ok());
        c.host = "libsql://db.turso.io".into();
        assert!(c.validate().is_err());
        c.host = "db.turso.io".into();
        c.port = Some(0);
        assert!(c.validate().is_err());
        assert!(!c.supports_multiple_databases());
    }
}
