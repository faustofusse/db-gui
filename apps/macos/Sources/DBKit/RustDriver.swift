import DBCoreFFI

/// `DatabaseDriver` backed by the shared Rust core through UniFFI.
///
/// Only DBKit knows about the generated `DBCoreFFI` types; the app uses DBKit's own models.
final class RustDriver: DatabaseDriver {
    let config: ConnectionConfig
    private let connection: DBCoreFFI.Connection

    init(config: ConnectionConfig) {
        self.config = config
        self.connection = DBCoreFFI.Connection(config: DBCoreFFI.ConnectionConfig(config))
    }

    static func sampleConnections() -> [ConnectionConfig] {
        DBCoreFFI.sampleConnections().map(ConnectionConfig.init)
    }

    static var coreVersion: String { DBCoreFFI.coreVersion() }

    func connect() async throws {
        try await bridged { try await connection.connect() }
    }

    func disconnect() async {
        await connection.disconnect()
    }

    func isConnected() async -> Bool {
        await connection.isConnected()
    }

    func listSchemas() async throws -> [Schema] {
        try await bridged { try await connection.listSchemas() }.map(Schema.init)
    }

    func fetchRows(of table: TableInfo, limit: Int, offset: Int) async throws -> QueryResult {
        let page = try await bridged {
            try await connection.fetchRows(
                table: DBCoreFFI.TableInfo(table), limit: UInt32(clamping: limit), offset: UInt64(max(0, offset)))
        }
        return QueryResult(page, firstRowID: offset)
    }

    func execute(_ sql: String) async throws -> QueryResult {
        QueryResult(try await bridged { try await connection.execute(sql: sql) }, firstRowID: 0)
    }

    func cancel() async {
        await connection.cancel()
    }

    /// Rethrows core errors as `DatabaseError`.
    private func bridged<T>(_ body: () async throws -> T) async throws -> T {
        do {
            return try await body()
        } catch let error as DBCoreFFI.DbError {
            throw DatabaseError(error)
        }
    }
}

// MARK: - Conversions (FFI ⇄ DBKit)

extension DatabaseError {
    init(_ error: DBCoreFFI.DbError) {
        switch error {
        case .ConnectionFailed(let message): self = .connectionFailed(message)
        case .TableNotFound(let name): self = .tableNotFound(name)
        case .Unsupported(let message): self = .unsupported(message)
        case .Query(let message): self = .query(message)
        case .Cancelled: self = .cancelled
        case .InvalidConfig(let message): self = .invalidConfig(message)
        case .Storage(let message): self = .storage(message)
        case .Internal(let message): self = .internal(message)
        }
    }
}

extension DatabaseKind {
    init(_ kind: DBCoreFFI.DatabaseKind) {
        switch kind {
        case .postgres: self = .postgres
        case .mysql: self = .mysql
        case .sqlite: self = .sqlite
        }
    }
}

extension DBCoreFFI.DatabaseKind {
    init(_ kind: DatabaseKind) {
        switch kind {
        case .postgres: self = .postgres
        case .mysql: self = .mysql
        case .sqlite: self = .sqlite
        }
    }
}

extension SslMode {
    init(_ mode: DBCoreFFI.SslMode) {
        switch mode {
        case .disable: self = .disable
        case .prefer: self = .prefer
        case .require: self = .require
        case .verifyFull: self = .verifyFull
        }
    }
}

extension DBCoreFFI.SslMode {
    init(_ mode: SslMode) {
        switch mode {
        case .disable: self = .disable
        case .prefer: self = .prefer
        case .require: self = .require
        case .verifyFull: self = .verifyFull
        }
    }
}

extension ConnectionConfig {
    init(_ c: DBCoreFFI.ConnectionConfig) {
        self.init(
            id: c.id, name: c.name, group: c.group, kind: DatabaseKind(c.kind),
            host: c.host, port: c.port.map(Int.init), database: c.database, user: c.user,
            password: c.password, sslMode: SslMode(c.sslMode),
            summary: DBCoreFFI.connectionSummary(config: c)
        )
    }
}

extension DBCoreFFI.ConnectionConfig {
    init(_ c: ConnectionConfig) {
        self.init(
            id: c.id, name: c.name, group: c.group, kind: DBCoreFFI.DatabaseKind(c.kind),
            host: c.host, port: c.port.map { UInt16(clamping: $0) }, database: c.database, user: c.user,
            password: c.password, sslMode: DBCoreFFI.SslMode(c.sslMode)
        )
    }
}

extension TableKind {
    init(_ kind: DBCoreFFI.TableKind) {
        switch kind {
        case .table: self = .table
        case .view: self = .view
        }
    }
}

extension TableInfo {
    init(_ t: DBCoreFFI.TableInfo) {
        self.init(schema: t.schema, name: t.name, kind: TableKind(t.kind),
                  estimatedRowCount: t.estimatedRowCount.map { Int(clamping: $0) })
    }
}

extension DBCoreFFI.TableInfo {
    init(_ t: TableInfo) {
        self.init(schema: t.schema, name: t.name, kind: t.kind == .view ? .view : .table,
                  estimatedRowCount: t.estimatedRowCount.map { UInt64(max(0, $0)) })
    }
}

extension Schema {
    init(_ s: DBCoreFFI.Schema) {
        self.init(name: s.name, tables: s.tables.map(TableInfo.init))
    }
}

extension ColumnInfo {
    init(_ c: DBCoreFFI.ColumnInfo) {
        self.init(name: c.name, typeName: c.typeName, isPrimaryKey: c.isPrimaryKey, isNullable: c.isNullable)
    }
}

extension DBValue {
    init(_ v: DBCoreFFI.Value) {
        switch v {
        case .null: self = .null
        case .bool(let b): self = .bool(b)
        case .int(let i): self = .int(i)
        case .float(let d): self = .double(d)
        case .decimal(let s): self = .decimal(s)
        case .text(let s): self = .text(s)
        }
    }
}

extension QueryResult {
    init(_ r: DBCoreFFI.QueryResult, firstRowID: Int) {
        self.init(
            columns: r.columns.map(ColumnInfo.init),
            rows: r.rows.enumerated().map { Row(id: firstRowID + $0.offset, values: $0.element.map(DBValue.init)) },
            totalCount: r.totalCount.map { Int(clamping: $0) },
            rowsAffected: r.rowsAffected.map { Int(clamping: $0) }
        )
    }
}
