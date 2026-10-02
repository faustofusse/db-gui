import Foundation

public enum DatabaseError: Error, Sendable, LocalizedError {
    case connectionFailed(String)
    case tableNotFound(String)
    case unsupported(String)
    case query(String)
    case cancelled
    case `internal`(String)

    public var errorDescription: String? {
        switch self {
        case .connectionFailed(let msg): "Connection failed: \(msg)"
        case .tableNotFound(let name): "Table not found: \(name)"
        case .unsupported(let what): "Unsupported: \(what)"
        case .query(let msg): msg
        case .cancelled: "Query cancelled"
        case .internal(let msg): "Internal error: \(msg)"
        }
    }
}

/// The boundary the SwiftUI app talks to. Backed by the Rust core (`RustDriver`);
/// other conformances (e.g. previews) can stand in without touching the UI.
public protocol DatabaseDriver: Sendable {
    var config: ConnectionConfig { get }
    func connect() async throws
    func disconnect() async
    func listSchemas() async throws -> [Schema]
    func fetchRows(of table: TableInfo, limit: Int, offset: Int) async throws -> QueryResult
    func execute(_ sql: String) async throws -> QueryResult
    /// Stops the running `execute`, which then throws `DatabaseError.cancelled`.
    func cancel() async
}

public enum Drivers {
    public static func make(for config: ConnectionConfig) -> any DatabaseDriver {
        RustDriver(config: config)
    }

    /// Hardcoded connections provided by the core until real connection storage exists.
    public static func sampleConnections() -> [ConnectionConfig] {
        RustDriver.sampleConnections()
    }

    public static var coreVersion: String { RustDriver.coreVersion }
}
