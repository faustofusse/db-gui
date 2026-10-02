//! Reads DBeaver's saved connections (`data-sources*.json`) and their credentials.
//!
//! Layout: `<DBeaverData>/workspace6/<project>/.dbeaver/data-sources*.json`, with users and
//! passwords in `credentials-config.json` next to it, AES-128-CBC encrypted with a fixed key
//! that is public in DBeaver's source (it obfuscates, it doesn't protect).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use serde_json::Value as Json;

use super::{ImportScan, ImportedConnection, SkippedConnection};
use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind, SslMode};

/// DBeaver's built-in key for `credentials-config.json`.
const CREDENTIALS_KEY: [u8; 16] = [
    0xba, 0xbb, 0x4a, 0x9f, 0x77, 0x4a, 0xb8, 0x53, 0xc9, 0x6c, 0x2d, 0x65, 0x3d, 0xfe, 0x54, 0x4a,
];

/// Where DBeaver keeps its data on this machine (existing directories only).
pub fn default_data_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    [
        home.join("Library/DBeaverData"),                                    // macOS
        xdg.join("DBeaverData"),                                             // Linux
        home.join(".var/app/io.dbeaver.DBeaverCommunity/data/DBeaverData"), // Linux, Flatpak
        home.join("snap/dbeaver-ce/current/.local/share/DBeaverData"),      // Linux, Snap
    ]
    .into_iter()
    .filter(|p| p.is_dir())
    .collect()
}

/// Scans DBeaver's default locations.
pub fn scan_default() -> Result<ImportScan> {
    let dirs = default_data_dirs();
    if dirs.is_empty() {
        return Err(Error::InvalidConfig("DBeaver’s data folder wasn’t found. Choose its data-sources.json instead.".into()));
    }
    let mut scan = ImportScan::default();
    for dir in dirs {
        scan.merge(scan_path(&dir)?);
    }
    Ok(scan)
}

/// Scans a `data-sources*.json` file, or any folder containing them (a `.dbeaver` folder,
/// a project, a workspace or the whole DBeaverData folder).
pub fn scan_path(path: &Path) -> Result<ImportScan> {
    let files = if path.is_file() { vec![path.to_path_buf()] } else { find_data_sources(path, 4) };
    if files.is_empty() {
        return Err(Error::InvalidConfig(format!("No DBeaver connections found in {}.", path.display())));
    }
    let mut scan = ImportScan::default();
    for file in files {
        scan.merge(scan_file(&file)?);
    }
    Ok(scan)
}

/// `data-sources*.json` files under `dir`, sorted (so `data-sources.json` comes first).
fn find_data_sources(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files = Vec::new();
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if path.is_file() && name.starts_with("data-sources") && name.ends_with(".json") {
            files.push(path);
        } else if path.is_dir() && depth > 0 && (!name.starts_with('.') || name == ".dbeaver") {
            files.extend(find_data_sources(&path, depth.saturating_sub(1)));
        }
    }
    files
}

fn scan_file(file: &Path) -> Result<ImportScan> {
    let read_error = |e: &dyn std::fmt::Display| Error::InvalidConfig(format!("Couldn’t read {}: {e}", file.display()));
    let text = std::fs::read_to_string(file).map_err(|e| read_error(&e))?;
    let json: Json = serde_json::from_str(&text).map_err(|e| read_error(&e))?;
    let credentials = file.parent().map(|dir| read_credentials(&dir.join("credentials-config.json"))).unwrap_or_default();
    let project = project_name(file);
    Ok(parse_data_sources(&json, &credentials, project.as_deref()))
}

/// The DBeaver project, when it isn't the default one ("General").
fn project_name(file: &Path) -> Option<String> {
    let project = file.parent()?.parent()?.file_name()?.to_str()?;
    (project != "General" && !project.starts_with("workspace")).then(|| project.to_string())
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Credentials {
    pub user: Option<String>,
    pub password: Option<String>,
}

/// Users and passwords by connection id. A missing or unreadable file means none were saved.
fn read_credentials(path: &Path) -> HashMap<String, Credentials> {
    std::fs::read(path).ok().and_then(|bytes| decrypt_credentials(&bytes)).unwrap_or_default()
}

pub(crate) fn decrypt_credentials(bytes: &[u8]) -> Option<HashMap<String, Credentials>> {
    let (iv, ciphertext) = (bytes.get(..16)?, bytes.get(16..)?);
    let plain = cbc::Decryptor::<aes::Aes128>::new_from_slices(&CREDENTIALS_KEY, iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .ok()?;
    let json: Json = serde_json::from_slice(&plain).ok()?;
    Some(
        json.as_object()?
            .iter()
            .map(|(id, entry)| {
                let connection = &entry["#connection"];
                let field = |k: &str| connection[k].as_str().filter(|s| !s.is_empty()).map(String::from);
                (id.clone(), Credentials { user: field("user"), password: field("password") })
            })
            .collect(),
    )
}

pub(crate) fn parse_data_sources(json: &Json, credentials: &HashMap<String, Credentials>, project: Option<&str>) -> ImportScan {
    let mut scan = ImportScan::default();
    let Some(connections) = json["connections"].as_object() else { return scan };
    for (id, source) in connections {
        let name = source["name"].as_str().unwrap_or(id).to_string();
        match convert(id, source, credentials.get(id), project) {
            Ok(imported) => scan.connections.push(imported),
            Err(reason) => scan.skipped.push(SkippedConnection { name, reason }),
        }
    }
    // The JSON object's order isn't meaningful (ids); list by folder, then name.
    let key = |c: &ConnectionConfig| (c.group.to_lowercase(), c.name.to_lowercase());
    scan.connections.sort_by_key(|c| key(&c.config));
    scan.skipped.sort_by_key(|s| s.name.to_lowercase());
    scan
}

/// One DBeaver connection → ours, or why it can't be imported.
fn convert(id: &str, source: &Json, credentials: Option<&Credentials>, project: Option<&str>) -> Result<ImportedConnection, String> {
    let provider = source["provider"].as_str().unwrap_or_default();
    let driver = source["driver"].as_str().unwrap_or_default();
    let kind = match (provider, driver) {
        ("postgresql", _) => DatabaseKind::Postgres,
        ("mysql", _) => DatabaseKind::Mysql,
        (_, d) if d.contains("libsql") => return Err("libSQL / Turso isn’t supported yet.".into()),
        ("sqlite", _) => DatabaseKind::Sqlite,
        ("generic", d) if d.contains("sqlite") => DatabaseKind::Sqlite,
        (other, _) => return Err(format!("{} isn’t supported yet.", provider_display_name(other))),
    };

    let conf = &source["configuration"];
    let text = |v: &Json| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let mut config = ConnectionConfig::new_empty(kind);
    config.name = text(&source["name"]).unwrap_or_default();
    config.group = [project.map(String::from), text(&source["folder"])].into_iter().flatten().collect::<Vec<_>>().join(" / ");

    // Fields first; the JDBC URL fills whatever they leave out ("URL" configurations).
    let from_url = text(&conf["url"]).and_then(|url| from_jdbc_url(kind, &url));
    if kind == DatabaseKind::Sqlite {
        config.database = text(&conf["database"]).or_else(|| from_url.as_ref().map(|c| c.database.clone())).unwrap_or_default();
        if config.database.is_empty() {
            return Err("No database file is set.".into());
        }
    } else {
        config.host = text(&conf["host"]).or_else(|| from_url.as_ref().map(|c| c.host.clone())).unwrap_or_else(|| "localhost".into());
        config.port = text(&conf["port"]).and_then(|p| p.parse().ok()).or(from_url.as_ref().and_then(|c| c.port));
        config.database = text(&conf["database"]).or_else(|| from_url.as_ref().map(|c| c.database.clone())).unwrap_or_default();
        config.user = credentials
            .and_then(|c| c.user.clone())
            .or_else(|| text(&conf["user"]))
            .or_else(|| from_url.as_ref().and_then(|c| c.user.clone()));
        if source["save-password"].as_bool() != Some(false) {
            config.password = credentials.and_then(|c| c.password.clone()).or_else(|| text(&conf["password"]));
        }
        config.ssl_mode = ssl_mode(kind, conf).or(from_url.as_ref().map(|c| c.ssl_mode)).unwrap_or_default();
    }

    let provider_flag = |key: &str| conf["provider-properties"][key].as_str().map(|v| v == "true");
    config.show_all_databases = match kind {
        // DBeaver shows only the configured Postgres database unless told otherwise…
        DatabaseKind::Postgres => provider_flag("@dbeaver-show-non-default-db@").unwrap_or(false),
        // …and every MySQL database by default.
        DatabaseKind::Mysql => provider_flag("@dbeaver-show-all-dbs@").unwrap_or(true),
        DatabaseKind::Sqlite => false,
    };
    if config.name.is_empty() {
        config.name = config.default_name();
    }

    let mut warnings = Vec::new();
    let handler_enabled = |name: &str| conf["handlers"][name]["enabled"].as_bool() == Some(true);
    if handler_enabled("ssh_tunnel") {
        warnings.push("Uses an SSH tunnel, which dbear doesn’t support yet.".into());
    }
    let proxy = conf["handlers"].as_object().is_some_and(|h| h.iter().any(|(k, v)| k.contains("proxy") && v["enabled"].as_bool() == Some(true)));
    if proxy {
        warnings.push("Uses a proxy, which dbear doesn’t support yet.".into());
    }
    match conf["auth-model"].as_str() {
        None | Some("native") => {}
        Some(model) => warnings.push(format!("Uses “{model}” authentication; dbear will connect with a user and password.")),
    }
    if kind != DatabaseKind::Sqlite && config.password.is_none() {
        warnings.push("No saved password.".into());
    }
    Ok(ImportedConnection { config, source_id: id.to_string(), warnings, already_added: false })
}

/// `jdbc:postgresql://h:5432/db?sslmode=require`, `jdbc:mysql://…`, `jdbc:sqlite:/path/file.db`.
fn from_jdbc_url(kind: DatabaseKind, url: &str) -> Option<ConnectionConfig> {
    let url = url.strip_prefix("jdbc:").unwrap_or(url);
    if kind == DatabaseKind::Sqlite {
        let path = url.strip_prefix("sqlite:")?.trim_start_matches("file:");
        let path = path.split('?').next().unwrap_or(path);
        let mut config = ConnectionConfig::new_empty(kind);
        config.database = path.to_string();
        return Some(config);
    }
    // Only the SSL setting is kept from the driver parameters (`sslmode`, MySQL's `sslMode`/`useSSL`).
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let ssl = query.split('&').find_map(|param| {
        let (key, value) = param.split_once('=')?;
        match (key.to_ascii_lowercase().as_str(), value.to_ascii_lowercase().as_str()) {
            ("sslmode", mode) => Some(mode.to_string()),
            ("usessl", "false") => Some("disable".into()),
            _ => None,
        }
    });
    let url = match ssl {
        Some(mode) => format!("{base}?sslmode={mode}"),
        None => base.to_string(),
    };
    ConnectionConfig::from_url(&url).ok()
}

/// SSL settings from the SSL handler or the driver properties, if DBeaver has any.
fn ssl_mode(kind: DatabaseKind, conf: &Json) -> Option<SslMode> {
    let handlers = conf["handlers"].as_object();
    let ssl_handler = handlers.and_then(|h| h.iter().find(|(k, _)| k.contains("ssl")).map(|(_, v)| v));
    let props = &conf["properties"];
    let prop = |key: &str| props[key].as_str().map(str::to_ascii_lowercase);
    let parse = |mode: &str| match mode.replace('_', "-").as_str() {
        "disable" | "disabled" => Some(SslMode::Disable),
        "allow" | "prefer" | "preferred" => Some(SslMode::Prefer),
        "require" | "required" => Some(SslMode::Require),
        "verify-ca" | "verify-full" | "verify-identity" => Some(SslMode::VerifyFull),
        _ => None,
    };
    if let Some(handler) = ssl_handler {
        if handler["enabled"].as_bool() == Some(true) {
            let hp = &handler["properties"];
            let mode = hp["sslMode"].as_str().or(hp["sslmode"].as_str()).and_then(|m| parse(&m.to_ascii_lowercase()));
            let verify = hp["ssl.verify.server"].as_bool().or(hp["verifyServerCert"].as_bool()) == Some(true);
            return Some(mode.unwrap_or(if verify { SslMode::VerifyFull } else { SslMode::Require }));
        }
    }
    match kind {
        DatabaseKind::Postgres => prop("sslmode").and_then(|m| parse(&m)),
        DatabaseKind::Mysql => prop("sslMode")
            .and_then(|m| parse(&m))
            .or_else(|| (prop("useSSL").as_deref() == Some("false")).then_some(SslMode::Disable)),
        DatabaseKind::Sqlite => None,
    }
}

fn provider_display_name(provider: &str) -> String {
    match provider {
        "sqlserver" => "SQL Server".into(),
        "oracle" => "Oracle".into(),
        "db2" => "Db2".into(),
        "mongodb" => "MongoDB".into(),
        "redis" => "Redis".into(),
        "clickhouse" => "ClickHouse".into(),
        "snowflake" => "Snowflake".into(),
        "duckdb" => "DuckDB".into(),
        "" => "This database".into(),
        other => {
            let mut chars = other.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;
    use serde_json::json;

    fn encrypt(plain: &str) -> Vec<u8> {
        let iv = [7u8; 16];
        let mut out = iv.to_vec();
        out.extend(cbc::Encryptor::<aes::Aes128>::new_from_slices(&CREDENTIALS_KEY, &iv).unwrap().encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes()));
        out
    }

    fn sample() -> Json {
        json!({
            "folders": {"prod": {}},
            "connections": {
                "postgres-jdbc-1": {
                    "provider": "postgresql", "driver": "postgres-jdbc", "name": "Billing", "folder": "prod",
                    "save-password": true,
                    "configuration": {
                        "host": "db.example.com", "port": "5433", "database": "billing",
                        "url": "jdbc:postgresql://db.example.com:5433/billing", "configurationType": "MANUAL",
                        "provider-properties": {"@dbeaver-show-non-default-db@": "true"},
                        "properties": {"sslmode": "verify-full"},
                        "auth-model": "native"
                    }
                },
                "mysql8-2": {
                    "provider": "mysql", "driver": "mysql8", "name": "Shop", "save-password": true,
                    "configuration": {
                        "url": "jdbc:mysql://shop.internal:3307/shop?useSSL=false&serverTimezone=UTC", "configurationType": "URL",
                        "provider-properties": {"@dbeaver-show-all-dbs@": "false"},
                        "handlers": {"ssh_tunnel": {"type": "TUNNEL", "enabled": true}}
                    }
                },
                "mariaDB-3": {
                    "provider": "mysql", "driver": "mariaDB", "name": "Legacy", "save-password": false,
                    "configuration": {"host": "maria", "port": "3306", "handlers": {"mysql_ssl": {"enabled": true, "properties": {}}}}
                },
                "sqlite_jdbc-4": {
                    "provider": "sqlite", "driver": "sqlite_jdbc", "name": "Notes",
                    "configuration": {"url": "jdbc:sqlite:/Users/me/notes.db", "configurationType": "URL"}
                },
                "libsql_jdbc-5": {"provider": "sqlite", "driver": "libsql_jdbc", "name": "Turso", "configuration": {}},
                "azure-6": {"provider": "sqlserver", "driver": "azure", "name": "Azure", "configuration": {}},
                "postgres-jdbc-7": {
                    "provider": "postgresql", "driver": "postgres-jdbc", "save-password": true,
                    "configuration": {"host": "localhost", "port": "5432", "database": "app"}
                }
            }
        })
    }

    #[test]
    fn decrypts_credentials() {
        let file = encrypt(r##"{"postgres-jdbc-1":{"#connection":{"user":"app","password":"s3cret"}},"mysql8-2":{"#connection":{"user":"root"}}}"##);
        let creds = decrypt_credentials(&file).unwrap();
        assert_eq!(creds["postgres-jdbc-1"], Credentials { user: Some("app".into()), password: Some("s3cret".into()) });
        assert_eq!(creds["mysql8-2"].password, None);
        assert!(decrypt_credentials(b"too short").is_none());
    }

    #[test]
    fn converts_supported_connections_and_explains_the_rest() {
        let creds = HashMap::from([
            ("postgres-jdbc-1".to_string(), Credentials { user: Some("app".into()), password: Some("s3cret".into()) }),
            ("mysql8-2".to_string(), Credentials { user: Some("root".into()), password: Some("pw".into()) }),
            ("mariaDB-3".to_string(), Credentials { user: Some("old".into()), password: Some("ignored".into()) }),
        ]);
        let scan = parse_data_sources(&sample(), &creds, None);
        let by_name = |n: &str| scan.connections.iter().find(|c| c.config.name == n).unwrap_or_else(|| panic!("{n}"));

        let pg = by_name("Billing");
        assert_eq!((pg.config.kind, pg.config.host.as_str(), pg.config.port, pg.config.database.as_str()), (DatabaseKind::Postgres, "db.example.com", Some(5433), "billing"));
        assert_eq!((pg.config.user.as_deref(), pg.config.password.as_deref()), (Some("app"), Some("s3cret")));
        assert_eq!((pg.config.group.as_str(), pg.config.ssl_mode, pg.config.show_all_databases), ("prod", SslMode::VerifyFull, true));
        assert!(pg.warnings.is_empty(), "{:?}", pg.warnings);

        let my = by_name("Shop");
        assert_eq!((my.config.kind, my.config.host.as_str(), my.config.port, my.config.database.as_str()), (DatabaseKind::Mysql, "shop.internal", Some(3307), "shop"));
        assert_eq!((my.config.ssl_mode, my.config.show_all_databases), (SslMode::Disable, false));
        assert!(my.warnings.iter().any(|w| w.contains("SSH tunnel")));

        let maria = by_name("Legacy");
        assert_eq!((maria.config.password.as_deref(), maria.config.ssl_mode), (None, SslMode::Require));
        assert!(maria.warnings.iter().any(|w| w.contains("No saved password")));

        let notes = by_name("Notes");
        assert_eq!((notes.config.kind, notes.config.database.as_str()), (DatabaseKind::Sqlite, "/Users/me/notes.db"));

        // Unnamed connections get the usual default name.
        assert!(!by_name("app").config.show_all_databases);

        let skipped: Vec<_> = scan.skipped.iter().map(|s| (s.name.as_str(), s.reason.as_str())).collect();
        assert_eq!(skipped, [("Azure", "SQL Server isn’t supported yet."), ("Turso", "libSQL / Turso isn’t supported yet.")]);
    }

    #[test]
    fn scans_a_dbeaver_folder() {
        let dir = tempfile::tempdir().unwrap();
        let dot = dir.path().join("workspace6/Work/.dbeaver");
        std::fs::create_dir_all(&dot).unwrap();
        std::fs::write(dot.join("data-sources.json"), sample().to_string()).unwrap();
        std::fs::write(dot.join("credentials-config.json"), encrypt(r##"{"postgres-jdbc-1":{"#connection":{"user":"app","password":"s3cret"}}}"##)).unwrap();

        let scan = scan_path(dir.path()).unwrap();
        assert_eq!((scan.connections.len(), scan.skipped.len()), (5, 2));
        let pg = scan.connections.iter().find(|c| c.config.name == "Billing").unwrap();
        assert_eq!((pg.config.password.as_deref(), pg.config.group.as_str()), (Some("s3cret"), "Work / prod"));

        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(scan_path(empty.path()), Err(Error::InvalidConfig(_))));
    }
}
