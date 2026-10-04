//! Editing helpers for [`ConnectionConfig`]: validation and connection URLs.
//! They live in the core so every frontend's "Add Connection" form behaves the same.

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use url::Url;

use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind, SslMode};

/// Characters escaped in the user/password part of a URL.
const USERINFO: &AsciiSet = &CONTROLS
    .add(b' ').add(b'"').add(b'#').add(b'%').add(b'/').add(b':').add(b';').add(b'<').add(b'=').add(b'>')
    .add(b'?').add(b'@').add(b'[').add(b'\\').add(b']').add(b'^').add(b'`').add(b'{').add(b'|').add(b'}');

impl ConnectionConfig {
    /// An empty config for the "Add Connection" form.
    pub fn new_empty(kind: DatabaseKind) -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            group: String::new(),
            kind,
            host: if kind.is_sqlite_family() { String::new() } else { "localhost".into() },
            port: None,
            database: String::new(),
            user: None,
            password: None,
            // Turso is always reached over the internet: verify its certificate by default.
            ssl_mode: if kind == DatabaseKind::Libsql { SslMode::VerifyFull } else { SslMode::default() },
            show_all_databases: !kind.is_sqlite_family(),
        }
    }

    /// Checks what the form needs before saving or testing. Returns the first problem found.
    pub fn validate(&self) -> Result<()> {
        // Name and database are optional (see `default_name` / `default_database`).
        let invalid = |msg: &str| Err(Error::InvalidConfig(msg.into()));
        if self.kind == DatabaseKind::Sqlite {
            if self.database.trim().is_empty() {
                return invalid("Choose a database file.");
            }
            return Ok(());
        }
        if self.kind == DatabaseKind::Libsql {
            return crate::libsql::url::validate(self);
        }
        if self.host.trim().is_empty() {
            return invalid("Enter a host.");
        }
        if self.port == Some(0) {
            return invalid("Port must be between 1 and 65535.");
        }
        Ok(())
    }

    /// Parses `postgres://user:pass@host:5432/db?sslmode=require`, `mysql://…`, `sqlite:///path/file.db`
    /// or a Turso / libSQL URL (`libsql://db-org.turso.io?authToken=…`, `https://…`, `http://localhost:8080`).
    /// The name defaults to [`ConnectionConfig::default_name`]; id and group are left empty.
    pub fn from_url(input: &str) -> Result<Self> {
        let invalid = |msg: String| Error::InvalidConfig(msg);
        let url = Url::parse(input.trim()).map_err(|e| invalid(format!("Not a valid connection URL ({e}).")))?;
        let kind = match url.scheme() {
            "postgres" | "postgresql" => DatabaseKind::Postgres,
            "mysql" | "mariadb" => DatabaseKind::Mysql,
            "sqlite" | "file" => DatabaseKind::Sqlite,
            "libsql" | "http" | "https" | "ws" | "wss" => return crate::libsql::url::from_url(&url),
            other => return Err(invalid(format!("Unsupported URL scheme “{other}”."))),
        };
        let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
        let mut config = Self::new_empty(kind);

        if kind == DatabaseKind::Sqlite {
            // sqlite:///abs/path or sqlite://relative/path
            let path = format!("{}{}", url.host_str().unwrap_or(""), url.path());
            config.database = decode(&path);
        } else {
            config.host = url.host_str().unwrap_or("localhost").trim_start_matches('[').trim_end_matches(']').into();
            config.port = url.port();
            config.database = decode(url.path().trim_start_matches('/'));
            config.user = Some(decode(url.username())).filter(|u| !u.is_empty());
            config.password = url.password().map(decode);
            for (key, value) in url.query_pairs() {
                if key == "sslmode" || key == "ssl-mode" {
                    config.ssl_mode = match value.to_ascii_lowercase().replace('_', "-").as_str() {
                        "disable" | "disabled" => SslMode::Disable,
                        "allow" | "prefer" | "preferred" => SslMode::Prefer,
                        "require" | "required" => SslMode::Require,
                        "verify-ca" | "verify-full" | "verify-identity" => SslMode::VerifyFull,
                        other => return Err(invalid(format!("Unknown sslmode “{other}”."))),
                    };
                }
            }
        }
        config.name = config.default_name();
        Ok(config)
    }

    /// The connection as a URL, e.g. for "Copy URL". The password is only included when asked for.
    pub fn to_url(&self, include_password: bool) -> String {
        if self.kind == DatabaseKind::Sqlite {
            return format!("sqlite://{}", self.database);
        }
        if self.kind == DatabaseKind::Libsql {
            return crate::libsql::url::to_url(self, include_password);
        }
        let scheme = if self.kind == DatabaseKind::Postgres { "postgres" } else { "mysql" };
        let enc = |s: &str| utf8_percent_encode(s, USERINFO).to_string();
        let mut userinfo = String::new();
        if let Some(user) = self.user.as_deref().filter(|u| !u.is_empty()) {
            userinfo.push_str(&enc(user));
            if let (true, Some(pw)) = (include_password, self.password.as_deref()) {
                userinfo.push(':');
                userinfo.push_str(&enc(pw));
            }
            userinfo.push('@');
        }
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        let ssl = match self.ssl_mode {
            SslMode::Prefer => "",
            SslMode::Disable => "?sslmode=disable",
            SslMode::Require => "?sslmode=require",
            SslMode::VerifyFull => "?sslmode=verify-full",
        };
        format!("{scheme}://{userinfo}{host}{port}/{}{ssl}", enc(&self.database))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_postgres_url() {
        let c = ConnectionConfig::from_url("postgresql://app%40corp:p%3Ass@db.example.com:6543/billing?sslmode=verify-full")
            .unwrap();
        assert_eq!(c.kind, DatabaseKind::Postgres);
        assert_eq!(c.host, "db.example.com");
        assert_eq!(c.port, Some(6543));
        assert_eq!(c.database, "billing");
        assert_eq!(c.name, "billing");
        assert_eq!(c.user.as_deref(), Some("app@corp"));
        assert_eq!(c.password.as_deref(), Some("p:ss"));
        assert_eq!(c.ssl_mode, SslMode::VerifyFull);
    }

    #[test]
    fn parses_minimal_and_ipv6_urls() {
        let c = ConnectionConfig::from_url("postgres://[::1]/app").unwrap();
        assert_eq!((c.host.as_str(), c.port, c.user), ("::1", None, None));
        let s = ConnectionConfig::from_url("sqlite:///Users/me/notes.db").unwrap();
        assert_eq!((s.kind, s.database.as_str(), s.name.as_str()), (DatabaseKind::Sqlite, "/Users/me/notes.db", "notes.db"));
    }

    #[test]
    fn defaults_name_and_database() {
        let c = ConnectionConfig::from_url("postgres://u@db.example.com:5432").unwrap();
        assert_eq!((c.name.as_str(), c.database.as_str(), c.default_database()), ("db.example.com", "", "postgres"));
        assert_eq!(c.summary(), "PostgreSQL · db.example.com:5432");
        let c = ConnectionConfig::from_url("postgres://u@db.example.com/app").unwrap();
        assert_eq!((c.name.as_str(), c.default_database()), ("app", "app"));
        let m = ConnectionConfig::from_url("mysql://root@localhost").unwrap();
        assert_eq!((m.name.as_str(), m.default_database()), ("localhost", ""));
    }

    #[test]
    fn rejects_bad_urls() {
        assert!(matches!(ConnectionConfig::from_url("redis://x"), Err(Error::InvalidConfig(_))));
        assert!(matches!(ConnectionConfig::from_url("not a url"), Err(Error::InvalidConfig(_))));
        assert!(matches!(ConnectionConfig::from_url("postgres://h/db?sslmode=nope"), Err(Error::InvalidConfig(_))));
    }

    #[test]
    fn url_round_trips() {
        let url = "postgres://app%40corp:p%3Ass@db.example.com:6543/billing?sslmode=require";
        let c = ConnectionConfig::from_url(url).unwrap();
        assert_eq!(c.to_url(true), url);
        assert_eq!(c.to_url(false), "postgres://app%40corp@db.example.com:6543/billing?sslmode=require");
    }

    #[test]
    fn validates_required_fields() {
        // Name and database are optional for servers.
        let mut c = ConnectionConfig::new_empty(DatabaseKind::Postgres);
        assert!(c.validate().is_ok());
        assert!(ConnectionConfig::new_empty(DatabaseKind::Sqlite).validate().is_err());
        c.port = Some(0);
        assert!(c.validate().is_err());
        c.port = None;
        c.host = " ".into();
        assert!(c.validate().is_err());
    }
}
