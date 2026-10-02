//! Saved connections: one JSON file shared by every frontend.
//!
//! Passwords are never written here. Frontends keep them in the platform keychain
//! (Keychain on macOS, libsecret on Linux) keyed by connection id, and set
//! `ConnectionConfig::password` right before connecting.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind, SslMode};

const FORMAT_VERSION: u32 = 1;

/// On-disk shape of a connection. Kept separate from [`ConnectionConfig`] so the file format
/// stays stable (and password-free) while the in-memory model evolves.
#[derive(Serialize, Deserialize)]
struct StoredConnection {
    id: String,
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    group: String,
    kind: DatabaseKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(default)]
    ssl_mode: SslMode,
    /// Missing in files written before multi-database support: show them all.
    #[serde(default = "default_true")]
    show_all_databases: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    connections: Vec<StoredConnection>,
}

impl From<&ConnectionConfig> for StoredConnection {
    fn from(c: &ConnectionConfig) -> Self {
        let name = c.name.trim();
        Self {
            id: c.id.clone(),
            name: if name.is_empty() { c.default_name() } else { name.into() },
            group: c.group.trim().into(),
            kind: c.kind,
            host: c.host.trim().into(),
            port: c.port,
            database: c.database.trim().into(),
            user: c.user.as_deref().map(str::trim).filter(|u| !u.is_empty()).map(Into::into),
            ssl_mode: c.ssl_mode,
            show_all_databases: c.show_all_databases,
        }
    }
}

impl From<StoredConnection> for ConnectionConfig {
    fn from(s: StoredConnection) -> Self {
        Self {
            id: s.id,
            name: s.name,
            group: s.group,
            kind: s.kind,
            host: s.host,
            port: s.port,
            database: s.database,
            user: s.user,
            password: None,
            ssl_mode: s.ssl_mode,
            show_all_databases: s.show_all_databases,
        }
    }
}

/// `~/Library/Application Support/dbear/connections.json` on macOS,
/// `$XDG_CONFIG_HOME/dbear/connections.json` (or `~/.config/…`) elsewhere.
pub fn default_path() -> Option<PathBuf> {
    Some(config_dir("dbear")?.join("connections.json"))
}

/// The app was called DBGui before; its folder is moved over on first use.
const LEGACY_DIR_NAME: &str = if cfg!(target_os = "macos") { "DBGui" } else { "dbgui" };

fn config_dir(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".config")))?
    };
    Some(base.join(name))
}

/// Moves `legacy` to `current` when only the legacy folder exists. Best effort: on failure the
/// old folder stays where it is and the app starts with an empty store.
fn migrate_dir(legacy: &Path, current: &Path) {
    if legacy.is_dir() && !current.exists() {
        let _ = fs::rename(legacy, current);
    }
}

/// A new unique connection id.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub struct ConnectionStore {
    path: PathBuf,
    connections: Vec<ConnectionConfig>,
}

impl ConnectionStore {
    /// Opens the store at [`default_path`], first moving over the folder from the app's old name.
    pub fn open_default() -> Result<Self> {
        let path = default_path().ok_or_else(|| Error::Storage("no home directory".into()))?;
        if let (Some(legacy), Some(dir)) = (config_dir(LEGACY_DIR_NAME), path.parent()) {
            migrate_dir(&legacy, dir);
        }
        Self::open(path)
    }

    /// Loads the file at `path`. A missing file is an empty store (created on first save).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let connections = match fs::read(&path) {
            Ok(bytes) => {
                let file: StoreFile = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Storage(format!("{} is not valid: {e}", path.display())))?;
                if file.version > FORMAT_VERSION {
                    return Err(Error::Storage(format!(
                        "{} was written by a newer version of dbear",
                        path.display()
                    )));
                }
                file.connections.into_iter().map(Into::into).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(Error::Storage(format!("{}: {e}", path.display()))),
        };
        Ok(Self { path, connections })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Saved connections in user order. Passwords are always `None`.
    pub fn connections(&self) -> &[ConnectionConfig] {
        &self.connections
    }

    pub fn get(&self, id: &str) -> Option<&ConnectionConfig> {
        self.connections.iter().find(|c| c.id == id)
    }

    /// Adds or replaces (by id) a connection and saves. An empty id gets a fresh one.
    /// Returns the stored config (password stripped).
    pub fn upsert(&mut self, config: ConnectionConfig) -> Result<ConnectionConfig> {
        config.validate()?;
        let mut stored: ConnectionConfig = StoredConnection::from(&config).into();
        if stored.id.is_empty() {
            stored.id = new_id();
        }
        let mut next = self.connections.clone();
        match next.iter_mut().find(|c| c.id == stored.id) {
            Some(existing) => *existing = stored.clone(),
            None => next.push(stored.clone()),
        }
        self.write(&next)?;
        self.connections = next;
        Ok(stored)
    }

    /// Removes a connection and saves. Returns whether it existed.
    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let next: Vec<_> = self.connections.iter().filter(|c| c.id != id).cloned().collect();
        if next.len() == self.connections.len() {
            return Ok(false);
        }
        self.write(&next)?;
        self.connections = next;
        Ok(true)
    }

    /// Atomic write: temp file in the same directory, then rename. Only touches memory on success.
    fn write(&self, connections: &[ConnectionConfig]) -> Result<()> {
        let err = |e: std::io::Error| Error::Storage(format!("{}: {e}", self.path.display()));
        let dir = self.path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir).map_err(err)?;

        let file = StoreFile { version: FORMAT_VERSION, connections: connections.iter().map(Into::into).collect() };
        let mut json = serde_json::to_vec_pretty(&file).map_err(|e| Error::Storage(e.to_string()))?;
        json.push(b'\n');

        let tmp = dir.join(format!(".{}.tmp", self.path.file_name().and_then(|n| n.to_str()).unwrap_or("connections")));
        let mut f = fs::File::create(&tmp).map_err(err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Hostnames and usernames are still worth keeping private.
            f.set_permissions(fs::Permissions::from_mode(0o600)).map_err(err)?;
        }
        f.write_all(&json).and_then(|_| f.sync_all()).map_err(err)?;
        fs::rename(&tmp, &self.path).map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> ConnectionConfig {
        ConnectionConfig {
            name: name.into(),
            group: "Local".into(),
            database: "app".into(),
            user: Some("postgres".into()),
            password: Some("secret".into()),
            ..ConnectionConfig::new_empty(DatabaseKind::Postgres)
        }
    }

    #[test]
    fn missing_file_is_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(dir.path().join("nested/connections.json")).unwrap();
        assert!(store.connections().is_empty());
    }

    #[test]
    fn saves_and_reloads_without_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/connections.json");
        let mut store = ConnectionStore::open(&path).unwrap();
        let saved = store.upsert(sample("dev")).unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(saved.password, None);

        let json = fs::read_to_string(&path).unwrap();
        assert!(!json.contains("secret"), "password leaked into {json}");
        assert!(json.contains("\"ssl_mode\": \"prefer\""));

        let reloaded = ConnectionStore::open(&path).unwrap();
        assert_eq!(reloaded.connections(), [saved]);
    }

    #[test]
    fn old_files_show_all_databases_and_the_flag_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        fs::write(&path, r#"{"version":1,"connections":[{"id":"x","name":"x","kind":"postgres","host":"h","database":"d"}]}"#).unwrap();
        let mut store = ConnectionStore::open(&path).unwrap();
        assert!(store.connections()[0].show_all_databases);
        let only_one = ConnectionConfig { show_all_databases: false, ..store.connections()[0].clone() };
        store.upsert(only_one).unwrap();
        assert!(!ConnectionStore::open(&path).unwrap().connections()[0].show_all_databases);
    }

    #[test]
    fn empty_name_defaults_to_database_then_host() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ConnectionStore::open(dir.path().join("c.json")).unwrap();
        let with_db = store.upsert(ConnectionConfig { name: " ".into(), ..sample("x") }).unwrap();
        assert_eq!(with_db.name, "app");
        let no_db = ConnectionConfig { name: String::new(), database: String::new(), host: "db.internal".into(), ..sample("x") };
        assert_eq!(store.upsert(no_db).unwrap().name, "db.internal");
    }

    #[test]
    fn upsert_replaces_in_place_and_remove_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ConnectionStore::open(dir.path().join("c.json")).unwrap();
        let a = store.upsert(sample("a")).unwrap();
        let b = store.upsert(sample("b")).unwrap();
        store.upsert(ConnectionConfig { name: "a2".into(), ..a.clone() }).unwrap();
        let names: Vec<_> = store.connections().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a2", "b"]);

        assert!(store.remove(&a.id).unwrap());
        assert!(!store.remove(&a.id).unwrap());
        assert_eq!(ConnectionStore::open(store.path()).unwrap().connections(), [b]);
    }

    #[test]
    fn rejects_invalid_config_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let mut store = ConnectionStore::open(&path).unwrap();
        let no_host = ConnectionConfig { host: " ".into(), ..sample("x") };
        assert!(matches!(store.upsert(no_host), Err(Error::InvalidConfig(_))));
        assert!(!path.exists());
    }

    #[test]
    fn reports_corrupt_and_future_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        fs::write(&path, "{nope").unwrap();
        assert!(matches!(ConnectionStore::open(&path), Err(Error::Storage(_))));
        fs::write(&path, r#"{"version": 99, "connections": []}"#).unwrap();
        assert!(matches!(ConnectionStore::open(&path), Err(Error::Storage(_))));
    }

    #[test]
    fn migrates_the_legacy_folder_once() {
        let dir = tempfile::tempdir().unwrap();
        let (legacy, current) = (dir.path().join("DBGui"), dir.path().join("dbear"));
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("connections.json"), r#"{"version": 1, "connections": []}"#).unwrap();
        migrate_dir(&legacy, &current);
        assert!(!legacy.exists() && current.join("connections.json").exists());

        // Never overwrites an existing folder.
        fs::create_dir_all(&legacy).unwrap();
        migrate_dir(&legacy, &current);
        assert!(legacy.exists());
    }

    #[test]
    fn default_path_is_platform_specific() {
        let p = default_path().unwrap();
        assert!(p.ends_with("connections.json"));
        if cfg!(target_os = "macos") {
            assert!(p.to_string_lossy().contains("Library/Application Support/dbear"));
        }
    }
}
