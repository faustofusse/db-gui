//! Importing connections from other tools (DBeaver).

use std::path::Path;

use crate::{ConnectionConfig, DbError};

#[derive(uniffi::Record)]
pub struct ImportedConnection {
    /// Includes the imported password, if any (to be moved to the Keychain on save).
    pub config: ConnectionConfig,
    pub warnings: Vec<String>,
    /// An equivalent connection already exists.
    pub already_added: bool,
}

#[derive(uniffi::Record)]
pub struct SkippedConnection {
    pub name: String,
    pub reason: String,
}

#[derive(uniffi::Record)]
pub struct ImportScan {
    pub connections: Vec<ImportedConnection>,
    pub skipped: Vec<SkippedConnection>,
}

impl From<dbcore::import::ImportScan> for ImportScan {
    fn from(scan: dbcore::import::ImportScan) -> Self {
        Self {
            connections: scan
                .connections
                .into_iter()
                .map(|c| ImportedConnection { config: c.config.into(), warnings: c.warnings, already_added: c.already_added })
                .collect(),
            skipped: scan.skipped.into_iter().map(|s| SkippedConnection { name: s.name, reason: s.reason }).collect(),
        }
    }
}

/// Reads DBeaver's connections, from its default data folder or from `path`
/// (a data-sources.json file or a folder containing one). `existing` marks duplicates.
#[uniffi::export]
pub fn scan_dbeaver(path: Option<String>, existing: Vec<ConnectionConfig>) -> Result<ImportScan, DbError> {
    use dbcore::import::dbeaver;
    let scan = match path {
        Some(path) => dbeaver::scan_path(Path::new(&path)),
        None => dbeaver::scan_default(),
    }?;
    let existing: Vec<dbcore::ConnectionConfig> = existing.into_iter().map(Into::into).collect();
    Ok(scan.mark_existing(&existing).into())
}

/// Whether DBeaver's data folder exists on this machine (to offer the import at all).
#[uniffi::export]
pub fn dbeaver_installed() -> bool {
    !dbcore::import::dbeaver::default_data_dirs().is_empty()
}
