//! Importing connections saved by other tools.

pub mod dbeaver;

use crate::model::{ConnectionConfig, DatabaseKind};

/// A connection ready to be saved. `config.password` holds the imported password, if any;
/// frontends move it to the platform keychain like any other.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedConnection {
    pub config: ConnectionConfig,
    /// The other tool's id for it.
    pub source_id: String,
    /// Things that won't carry over (SSH tunnels, unsupported auth…), for the user to review.
    pub warnings: Vec<String>,
    /// An equivalent connection already exists (see [`mark_existing`]).
    pub already_added: bool,
}

/// A connection that can't be imported, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedConnection {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportScan {
    pub connections: Vec<ImportedConnection>,
    pub skipped: Vec<SkippedConnection>,
}

impl ImportScan {
    fn merge(&mut self, other: ImportScan) {
        self.connections.extend(other.connections);
        self.skipped.extend(other.skipped);
    }

    /// Flags connections that point where an existing one already does, so they can be left out.
    pub fn mark_existing(mut self, existing: &[ConnectionConfig]) -> Self {
        for imported in &mut self.connections {
            imported.already_added = existing.iter().any(|e| same_target(e, &imported.config));
        }
        self
    }
}

/// Same server (or file), database and user: importing it again would only add a duplicate.
pub fn same_target(a: &ConnectionConfig, b: &ConnectionConfig) -> bool {
    if a.kind != b.kind {
        return false;
    }
    if a.kind == DatabaseKind::Sqlite {
        return a.database.trim() == b.database.trim();
    }
    let port = |c: &ConnectionConfig| c.port.or(c.kind.default_port());
    let user = |c: &ConnectionConfig| c.user.clone().unwrap_or_default();
    a.host.trim().eq_ignore_ascii_case(b.host.trim())
        && port(a) == port(b)
        && a.default_database() == b.default_database()
        && user(a) == user(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_existing_connections() {
        let existing = ConnectionConfig::from_url("postgres://app@DB.example.com/billing").unwrap();
        let same = ConnectionConfig::from_url("postgres://app:pw@db.example.com:5432/billing").unwrap();
        let other_db = ConnectionConfig::from_url("postgres://app@db.example.com/other").unwrap();
        let other_user = ConnectionConfig::from_url("postgres://admin@db.example.com/billing").unwrap();
        assert!(same_target(&existing, &same));
        assert!(!same_target(&existing, &other_db) && !same_target(&existing, &other_user));

        let scan = ImportScan {
            connections: [same, other_db]
                .into_iter()
                .map(|config| ImportedConnection { config, source_id: String::new(), warnings: vec![], already_added: false })
                .collect(),
            skipped: vec![],
        }
        .mark_existing(&[existing]);
        assert_eq!(scan.connections.iter().map(|c| c.already_added).collect::<Vec<_>>(), [true, false]);
    }
}
