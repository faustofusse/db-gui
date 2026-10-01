import Foundation

/// Hardcoded sample connections, schemas and data so the UI can be built before real drivers exist.
public enum MockData {
    public static let connections: [ConnectionConfig] = [
        .init(id: "local-pg", name: "app_dev", group: "Local", kind: .postgres,
              host: "localhost", port: 5432, database: "app_dev", user: "postgres"),
        .init(id: "local-mysql", name: "wordpress", group: "Local", kind: .mysql,
              host: "localhost", port: 3306, database: "wordpress", user: "root"),
        .init(id: "local-sqlite", name: "notes.db", group: "Local", kind: .sqlite,
              host: "~/Library/Application Support/Notes", database: "notes.db"),
        .init(id: "staging-pg", name: "app_staging", group: "Staging", kind: .postgres,
              host: "staging-db.internal", port: 5432, database: "app", user: "readonly"),
        .init(id: "prod-pg", name: "app_production", group: "Production", kind: .postgres,
              host: "prod-db.internal", port: 5432, database: "app", user: "readonly"),
        .init(id: "prod-replica", name: "app_replica", group: "Production", kind: .postgres,
              host: "replica-db.internal", port: 5432, database: "app", user: "readonly"),
    ]

    /// Connections that simulate a failure (shows the warning state in the UI).
    public static let unreachable: Set<String> = ["prod-replica"]

    struct TableSpec {
        let name: String
        let kind: TableKind
        let rows: Int
        let columns: [ColumnInfo]
    }

    static func col(_ name: String, _ type: String, pk: Bool = false, null: Bool = false) -> ColumnInfo {
        ColumnInfo(name: name, typeName: type, isPrimaryKey: pk, isNullable: null)
    }

    static let appSchemas: [String: [TableSpec]] = [
        "public": [
            .init(name: "users", kind: .table, rows: 248, columns: [
                col("id", "bigint", pk: true), col("name", "text"), col("email", "text"),
                col("is_admin", "boolean"), col("last_login_at", "timestamptz", null: true),
                col("created_at", "timestamptz"),
            ]),
            .init(name: "orders", kind: .table, rows: 1_204, columns: [
                col("id", "bigint", pk: true), col("user_id", "bigint"), col("status", "text"),
                col("total", "numeric"), col("notes", "text", null: true), col("created_at", "timestamptz"),
            ]),
            .init(name: "products", kind: .table, rows: 86, columns: [
                col("id", "bigint", pk: true), col("sku", "text"), col("name", "text"),
                col("price", "numeric"), col("in_stock", "boolean"),
            ]),
            .init(name: "sessions", kind: .table, rows: 512, columns: [
                col("id", "uuid", pk: true), col("user_id", "bigint"), col("ip", "inet"),
                col("expires_at", "timestamptz"),
            ]),
            .init(name: "active_users", kind: .view, rows: 37, columns: [
                col("id", "bigint"), col("name", "text"), col("email", "text"),
                col("last_login_at", "timestamptz"),
            ]),
        ],
        "billing": [
            .init(name: "invoices", kind: .table, rows: 930, columns: [
                col("id", "bigint", pk: true), col("order_id", "bigint"), col("amount", "numeric"),
                col("paid", "boolean"), col("due_at", "timestamptz"),
            ]),
            .init(name: "payments", kind: .table, rows: 874, columns: [
                col("id", "bigint", pk: true), col("invoice_id", "bigint"), col("provider", "text"),
                col("amount", "numeric"), col("created_at", "timestamptz"),
            ]),
            .init(name: "subscriptions", kind: .table, rows: 61, columns: [
                col("id", "bigint", pk: true), col("user_id", "bigint"), col("plan", "text"),
                col("status", "text"), col("canceled_at", "timestamptz", null: true),
            ]),
        ],
        "analytics": [
            .init(name: "events", kind: .table, rows: 50_000, columns: [
                col("id", "bigint", pk: true), col("user_id", "bigint", null: true),
                col("name", "text"), col("path", "text"), col("created_at", "timestamptz"),
            ]),
            .init(name: "daily_signups", kind: .view, rows: 90, columns: [
                col("day", "date"), col("count", "bigint"),
            ]),
        ],
    ]

    static let wordpressSchemas: [String: [TableSpec]] = [
        "wordpress": [
            .init(name: "wp_posts", kind: .table, rows: 312, columns: [
                col("ID", "bigint", pk: true), col("post_title", "varchar"), col("status", "varchar"),
                col("post_author", "bigint"), col("created_at", "datetime"),
            ]),
            .init(name: "wp_users", kind: .table, rows: 12, columns: [
                col("ID", "bigint", pk: true), col("user_login", "varchar"), col("email", "varchar"),
                col("created_at", "datetime"),
            ]),
            .init(name: "wp_options", kind: .table, rows: 140, columns: [
                col("option_id", "bigint", pk: true), col("option_name", "varchar"),
                col("option_value", "longtext", null: true),
            ]),
        ],
    ]

    static let sqliteSchemas: [String: [TableSpec]] = [
        "main": [
            .init(name: "notes", kind: .table, rows: 57, columns: [
                col("id", "INTEGER", pk: true), col("title", "TEXT"), col("body", "TEXT", null: true),
                col("pinned", "INTEGER"), col("created_at", "TEXT"),
            ]),
            .init(name: "tags", kind: .table, rows: 9, columns: [
                col("id", "INTEGER", pk: true), col("name", "TEXT"),
            ]),
        ],
    ]

    static func specs(for config: ConnectionConfig) -> [String: [TableSpec]] {
        switch config.kind {
        case .postgres: appSchemas
        case .mysql: wordpressSchemas
        case .sqlite: sqliteSchemas
        }
    }

    // MARK: Deterministic fake values

    static let names = ["Ada Lovelace", "Alan Turing", "Grace Hopper", "Linus Torvalds", "Barbara Liskov",
                        "Ken Thompson", "Dennis Ritchie", "Margaret Hamilton", "Edsger Dijkstra", "Donald Knuth"]
    static let statuses = ["pending", "paid", "shipped", "delivered", "canceled", "refunded"]
    static let words = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel"]

    static func value(for column: ColumnInfo, row i: Int) -> DBValue {
        if column.isNullable && i % 4 == 1 { return .null }
        let n = column.name.lowercased()
        let t = column.typeName.lowercased()

        if column.isPrimaryKey && t == "uuid" {
            return .text(String(format: "%08x-4b1c-9e2a-%012x", i &* 2_654_435_761 & 0xffff_ffff, i * 7919))
        }
        if column.isPrimaryKey || n == "id" { return .int(Int64(i + 1)) }
        if n.hasSuffix("_id") || n == "post_author" { return .int(Int64((i * 37) % 250 + 1)) }
        if t == "boolean" || n == "pinned" { return .bool(i % 3 != 0) }
        if t == "numeric" { return .double(Double((i * 1_733) % 50_000) / 100 + 4.99) }
        if t.contains("time") || t == "date" || n.hasSuffix("_at") || n == "day" {
            let day = 1 + (i % 28), month = 1 + (i / 28) % 12, hour = (i * 7) % 24, minute = (i * 13) % 60
            let date = String(format: "2025-%02d-%02d", month, day)
            return .text(t == "date" ? date : date + String(format: " %02d:%02d:00", hour, minute))
        }
        if n == "email" || n.hasSuffix("email") {
            let name = names[i % names.count].lowercased().replacingOccurrences(of: " ", with: ".")
            return .text("\(name)\(i)@example.com")
        }
        if n.contains("name") || n == "user_login" || n == "post_title" || n == "title" {
            if n == "option_name" { return .text("option_\(words[i % words.count])_\(i)") }
            if n == "post_title" || n == "title" { return .text("\(words[i % words.count].capitalized) note #\(i + 1)") }
            return .text(names[i % names.count])
        }
        if n == "status" { return .text(statuses[i % statuses.count]) }
        if n == "plan" { return .text(["free", "pro", "team"][i % 3]) }
        if n == "provider" { return .text(["stripe", "paypal", "mercadopago"][i % 3]) }
        if n == "sku" { return .text(String(format: "SKU-%05d", i * 17)) }
        if n == "ip" { return .text("10.0.\(i % 255).\((i * 3) % 255)") }
        if n == "path" { return .text("/\(words[i % words.count])/\(words[(i + 3) % words.count])") }
        if t == "bigint" || t == "integer" { return .int(Int64((i * 31) % 1_000)) }
        return .text("\(words[i % words.count]) \(words[(i * 5) % words.count])")
    }
}

public struct MockDriver: DatabaseDriver {
    public let config: ConnectionConfig

    public init(config: ConnectionConfig) {
        self.config = config
    }

    private func simulateLatency() async {
        try? await Task.sleep(for: .milliseconds(120))
    }

    public func connect() async throws {
        await simulateLatency()
        if MockData.unreachable.contains(config.id) {
            throw DatabaseError.connectionFailed("could not connect to server at \"\(config.host)\" (timeout)")
        }
    }

    public func disconnect() async {}

    public func listSchemas() async throws -> [Schema] {
        try await connect()
        return MockData.specs(for: config)
            .sorted { $0.key < $1.key }
            .map { name, specs in
                Schema(name: name, tables: specs.map {
                    TableInfo(schema: name, name: $0.name, kind: $0.kind, estimatedRowCount: $0.rows)
                })
            }
    }

    public func fetchRows(of table: TableInfo, limit: Int, offset: Int) async throws -> QueryResult {
        await simulateLatency()
        guard let spec = MockData.specs(for: config)[table.schema]?.first(where: { $0.name == table.name }) else {
            throw DatabaseError.tableNotFound(table.id)
        }
        let end = min(spec.rows, offset + limit)
        let rows = (offset..<max(offset, end)).map { i in
            Row(id: i, values: spec.columns.map { MockData.value(for: $0, row: i) })
        }
        return QueryResult(columns: spec.columns, rows: rows, totalCount: spec.rows)
    }

    /// Understands just enough SQL to be useful for UI work:
    /// `SELECT * | col, col FROM [schema.]table [LIMIT n]`.
    public func execute(_ sql: String) async throws -> QueryResult {
        await simulateLatency()
        try await connect()

        let statement = sql
            .split(separator: "\n")
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("--") }
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: ";")))

        let pattern = #"^select\s+(.+?)\s+from\s+"?(\w+)"?(?:\."?(\w+)"?)?(?:\s+limit\s+(\d+))?$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive, .dotMatchesLineSeparators]),
              let m = regex.firstMatch(in: statement, range: NSRange(statement.startIndex..., in: statement))
        else {
            throw DatabaseError.unsupported("the mock driver only understands SELECT … FROM table [LIMIT n]")
        }
        func group(_ i: Int) -> String? {
            Range(m.range(at: i), in: statement).map { String(statement[$0]) }
        }

        let specs = MockData.specs(for: config)
        let first = group(2)!, second = group(3)
        let (schemaName, tableName): (String?, String) = second.map { (first, $0) } ?? (nil, first)
        guard let (schema, spec) = specs
            .sorted(by: { $0.key == "public" || ($1.key != "public" && $0.key < $1.key) })
            .lazy
            .compactMap({ key, tables in
                (schemaName == nil || schemaName == key) ? tables.first { $0.name == tableName }.map { (key, $0) } : nil
            })
            .first
        else {
            throw DatabaseError.tableNotFound([schemaName, tableName].compactMap { $0 }.joined(separator: "."))
        }

        let limit = group(4).flatMap(Int.init) ?? 200
        let full = try await fetchRows(of: TableInfo(schema: schema, name: spec.name), limit: limit, offset: 0)

        let selectList = group(1)!.trimmingCharacters(in: .whitespaces)
        guard selectList != "*" else { return QueryResult(columns: full.columns, rows: full.rows) }

        let wanted = selectList.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        let indices = try wanted.map { name in
            guard let i = full.columns.firstIndex(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) else {
                throw DatabaseError.unsupported("column \"\(name)\" does not exist")
            }
            return i
        }
        return QueryResult(
            columns: indices.map { full.columns[$0] },
            rows: full.rows.map { row in Row(id: row.id, values: indices.map { row.values[$0] }) }
        )
    }
}
