import DBCoreFFI
import Foundation

/// A connection found in another tool, ready to be saved.
public struct ImportedConnection: Identifiable, Sendable {
    public let id = UUID()
    /// Includes the imported password, if any.
    public var config: ConnectionConfig
    /// What won't carry over (SSH tunnels, unsupported authentication…).
    public var warnings: [String]
    /// An equivalent connection is already saved.
    public var alreadyAdded: Bool
}

public struct SkippedConnection: Identifiable, Sendable {
    public let id = UUID()
    public var name: String
    public var reason: String
}

public struct ImportScan: Sendable {
    public var connections: [ImportedConnection]
    public var skipped: [SkippedConnection]
}

public enum DBeaverImport {
    /// Whether DBeaver's data folder exists on this Mac.
    public static var isInstalled: Bool { DBCoreFFI.dbeaverInstalled() }

    /// Reads DBeaver's connections from its data folder, or from `path` (a data-sources.json
    /// or a folder containing one). Connections matching `existing` are flagged.
    public static func scan(path: String? = nil, existing: [ConnectionConfig]) throws -> ImportScan {
        try bridged {
            let scan = try DBCoreFFI.scanDbeaver(path: path, existing: existing.map(DBCoreFFI.ConnectionConfig.init))
            return ImportScan(
                connections: scan.connections.map {
                    ImportedConnection(config: ConnectionConfig($0.config), warnings: $0.warnings, alreadyAdded: $0.alreadyAdded)
                },
                skipped: scan.skipped.map { SkippedConnection(name: $0.name, reason: $0.reason) }
            )
        }
    }
}
