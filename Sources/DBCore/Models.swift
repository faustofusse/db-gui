import Foundation

public enum DatabaseKind: String, Sendable, Hashable, CaseIterable, Codable {
    case postgres
    case mysql
    case sqlite

    public var displayName: String {
        switch self {
        case .postgres: "PostgreSQL"
        case .mysql: "MySQL"
        case .sqlite: "SQLite"
        }
    }
}

public struct ConnectionConfig: Identifiable, Hashable, Sendable, Codable {
    public let id: String
    public var name: String
    public var group: String
    public var kind: DatabaseKind
    public var host: String
    public var port: Int?
    public var database: String
    public var user: String?

    public init(
        id: String, name: String, group: String, kind: DatabaseKind,
        host: String, port: Int? = nil, database: String, user: String? = nil
    ) {
        self.id = id
        self.name = name
        self.group = group
        self.kind = kind
        self.host = host
        self.port = port
        self.database = database
        self.user = user
    }

    /// e.g. "PostgreSQL · localhost:5432/app_dev"
    public var summary: String {
        let address = port.map { "\(host):\($0)" } ?? host
        return kind == .sqlite
            ? "\(kind.displayName) · \(database)"
            : "\(kind.displayName) · \(address)/\(database)"
    }
}

public enum TableKind: String, Sendable, Hashable, Codable {
    case table
    case view
}

public struct TableInfo: Identifiable, Hashable, Sendable {
    public var schema: String
    public var name: String
    public var kind: TableKind
    public var estimatedRowCount: Int?

    public var id: String { "\(schema).\(name)" }

    public init(schema: String, name: String, kind: TableKind = .table, estimatedRowCount: Int? = nil) {
        self.schema = schema
        self.name = name
        self.kind = kind
        self.estimatedRowCount = estimatedRowCount
    }
}

public struct Schema: Identifiable, Hashable, Sendable {
    public var name: String
    public var tables: [TableInfo]
    public var id: String { name }

    public init(name: String, tables: [TableInfo]) {
        self.name = name
        self.tables = tables
    }
}

public struct ColumnInfo: Identifiable, Hashable, Sendable {
    public var name: String
    public var typeName: String
    public var isPrimaryKey: Bool
    public var isNullable: Bool
    public var id: String { name }

    public init(name: String, typeName: String, isPrimaryKey: Bool = false, isNullable: Bool = false) {
        self.name = name
        self.typeName = typeName
        self.isPrimaryKey = isPrimaryKey
        self.isNullable = isNullable
    }
}

public enum DBValue: Hashable, Sendable {
    case null
    case bool(Bool)
    case int(Int64)
    case double(Double)
    case text(String)

    public var isNull: Bool { if case .null = self { true } else { false } }

    public var displayString: String {
        switch self {
        case .null: "NULL"
        case .bool(let b): b ? "true" : "false"
        case .int(let i): String(i)
        case .double(let d): String(format: "%.2f", d)
        case .text(let s): s
        }
    }
}

public struct Row: Identifiable, Hashable, Sendable {
    public let id: Int
    public var values: [DBValue]

    public init(id: Int, values: [DBValue]) {
        self.id = id
        self.values = values
    }
}

public struct QueryResult: Sendable {
    public var columns: [ColumnInfo]
    public var rows: [Row]
    public var totalCount: Int?

    public init(columns: [ColumnInfo], rows: [Row], totalCount: Int? = nil) {
        self.columns = columns
        self.rows = rows
        self.totalCount = totalCount
    }
}
