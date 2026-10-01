import Foundation

public enum DatabaseError: Error, Sendable, LocalizedError {
    case connectionFailed(String)
    case tableNotFound(String)
    case unsupported(String)

    public var errorDescription: String? {
        switch self {
        case .connectionFailed(let msg): "Connection failed: \(msg)"
        case .tableNotFound(let name): "Table not found: \(name)"
        case .unsupported(let what): "Unsupported: \(what)"
        }
    }
}

/// The single boundary every frontend talks to.
/// Real drivers (Postgres, MySQL, SQLite) implement this; UIs never see wire protocols.
public protocol DatabaseDriver: Sendable {
    var config: ConnectionConfig { get }
    func connect() async throws
    func disconnect() async
    func listSchemas() async throws -> [Schema]
    func fetchRows(of table: TableInfo, limit: Int, offset: Int) async throws -> QueryResult
    func execute(_ sql: String) async throws -> QueryResult
}

public enum Drivers {
    /// Returns the driver for a connection. Everything is mocked for now.
    public static func make(for config: ConnectionConfig) -> any DatabaseDriver {
        MockDriver(config: config)
    }
}
