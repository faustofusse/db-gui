import Testing
@testable import DBKit

import Foundation

// End-to-end through the FFI boundary: Swift → UniFFI → Rust core.
// Core behaviour itself is tested in Rust (`cargo test -p dbcore`, `scripts/test-postgres.sh`).

private let connections = Drivers.sampleConnections()
/// Real dev database (scripts/dev-db.sh up).
private var devDB: ConnectionConfig { connections.first { $0.id == "local-pg" }! }
/// Mock Postgres with the sample schema.
private var appDev: ConnectionConfig { connections.first { $0.id == "staging-pg" }! }
private let postgresEnabled = ProcessInfo.processInfo.environment["DBEAR_TEST_POSTGRES"] == "1"
private let mysqlEnabled = ProcessInfo.processInfo.environment["DBEAR_TEST_MYSQL"] == "1"
/// Real dev MySQL (scripts/dev-db.sh up mysql) and SQLite file (scripts/dev-db.sh up sqlite).
private var devMySQL: ConnectionConfig { connections.first { $0.id == "local-mysql" }! }
private var devSQLite: ConnectionConfig { connections.first { $0.id == "local-sqlite" }! }
private let sqliteSeeded = FileManager.default.fileExists(atPath: Drivers.sampleConnections().first { $0.id == "local-sqlite" }!.database)

@Test func loadsSampleConnectionsWithSummary() {
    #expect(connections.count == 6)
    #expect(devDB.summary == "PostgreSQL · localhost:54329/app_dev")
    #expect(devDB.password == "postgres" && devDB.sslMode == .prefer)
}

@Test(.enabled(if: postgresEnabled)) func realPostgresRoundTrip() async throws {
    let driver = Drivers.make(for: devDB)
    let schemas = try await driver.listSchemas()
    #expect(schemas.map(\.name) == ["analytics", "archive", "billing", "public"])

    let affected = try await driver.execute("create temp table t (x int); insert into t values (1), (2)")
    #expect(affected.columns.isEmpty && affected.rowsAffected == 2)

    #expect(await driver.isConnected())
    await driver.disconnect()
    #expect(await !driver.isConnected())
}

@Test(.enabled(if: mysqlEnabled)) func realMySQLRoundTrip() async throws {
    #expect(!devMySQL.supportsMultipleDatabases && devMySQL.showAllDatabases)
    let driver = Drivers.make(for: devMySQL)
    // Databases are schemas: one connection lists them all.
    #expect(try await driver.listSchemas().map(\.name) == ["archive", "blog", "shop"])
    let page = try await driver.fetchRows(of: TableInfo(schema: "shop", name: "orders"), limit: 10, offset: 0)
    #expect(page.rows.count == 10 && page.totalCount == 1200)
    #expect(page.rows[0].values[3] == .decimal("27.31"))
    await driver.disconnect()
}

@Test(.enabled(if: sqliteSeeded)) func realSQLiteRoundTrip() async throws {
    #expect(!devSQLite.supportsMultipleDatabases)
    let driver = Drivers.make(for: devSQLite)
    let schemas = try await driver.listSchemas()
    #expect(schemas.map(\.name) == ["main"])
    let page = try await driver.fetchRows(of: TableInfo(schema: "main", name: "settings"), limit: 10, offset: 0)
    #expect(page.rows.map { $0.values[1] } == [.text("dark"), .int(13), .double(1.25), .null, .text("0xdeadbeef")])
    await driver.disconnect()
}

@Test(.enabled(if: postgresEnabled)) func cancelsRunningQuery() async throws {
    let driver = Drivers.make(for: devDB)
    try await driver.connect()
    async let sleep = driver.execute("select pg_sleep(10)")
    try await Task.sleep(for: .milliseconds(500))
    await driver.cancel()
    do {
        _ = try await sleep
        Issue.record("query should have been cancelled")
    } catch DatabaseError.cancelled {
        // expected
    }
}

@Test func listsSchemas() async throws {
    let schemas = try await Drivers.make(for: appDev).listSchemas()
    #expect(schemas.map(\.name) == ["analytics", "billing", "public"])
    #expect(schemas.last?.tables.first?.estimatedRowCount == 248)
}

@Test func fetchesPageWithStableRowIDs() async throws {
    let users = TableInfo(schema: "public", name: "users")
    let page = try await Drivers.make(for: appDev).fetchRows(of: users, limit: 50, offset: 100)
    #expect(page.rows.count == 50)
    #expect(page.rows.first?.id == 100)
    #expect(page.rows.allSatisfy { $0.values.count == page.columns.count })
    #expect(page.totalCount == 248)
}

@Test func mapsCoreErrors() async {
    let replica = connections.first { $0.id == "prod-replica" }!
    await #expect {
        try await Drivers.make(for: replica).listSchemas()
    } throws: { error in
        guard case DatabaseError.connectionFailed(let msg) = error else { return false }
        return msg.contains("replica-db.internal")
    }
}

@Test func executesSelect() async throws {
    let result = try await Drivers.make(for: appDev).execute("select id, total from orders limit 3")
    #expect(result.columns.map(\.name) == ["id", "total"])
    #expect(result.rows.first?.values.last.map { if case .decimal = $0 { true } else { false } } == true)
}

@Test func capsScriptRows() async throws {
    let driver = Drivers.make(for: appDev)
    let capped = try await driver.execute("select * from users", maxRows: 100)
    #expect(capped.rows.count == 100 && capped.truncated && capped.totalCount == 248)
    let all = try await driver.execute("select * from users")
    #expect(all.rows.count == 248 && !all.truncated)
}

@Test func listsDatabasesAndSwitchesDatabase() async throws {
    #expect(appDev.showAllDatabases)
    let databases = try await Drivers.make(for: appDev).listDatabases()
    #expect(databases == ["app", "app_test", "postgres"])
    let other = appDev.withDatabase("app_test")
    #expect(other.id == appDev.id && other.summary.hasSuffix("/app_test"))
}

@Test(.enabled(if: postgresEnabled)) func realPostgresOtherDatabase() async throws {
    let databases = try await Drivers.make(for: devDB).listDatabases()
    #expect(databases.contains("app_dev") && databases.contains("postgres"))
    let result = try await Drivers.make(for: devDB.withDatabase("postgres")).execute("select current_database()")
    #expect(result.rows.first?.values.first == .text("postgres"))
}

@Test func emptyDatabaseAndNameFallBack() throws {
    let noDatabase = try ConnectionConfig.parse(url: "postgres://u@db.example.com:5432")
    #expect(noDatabase.validationError == nil)
    #expect(noDatabase.name == "db.example.com" && noDatabase.defaultName == "db.example.com")
    #expect(noDatabase.defaultDatabase == "postgres")
    var named = noDatabase
    named.database = "app"
    #expect(named.defaultName == "app" && named.defaultDatabase == "app")
}

@Test func sortsMockRowsAndRejectsFiltersThroughFFI() async throws {
    let driver = Drivers.make(for: appDev)
    let users = TableInfo(schema: "public", name: "users")
    let page = try await driver.fetchRows(
        of: users, query: RowQuery(sort: [SortKey(column: "id", descending: true)]), limit: 2, offset: 0)
    #expect(page.rows.map { $0.values[0] } == [.int(248), .int(247)])
    await #expect(throws: DatabaseError.self) {
        try await driver.fetchRows(of: users, query: RowQuery(filter: "id = 1"), limit: 2, offset: 0)
    }
    let structure = try await driver.describeTable(users)
    #expect(structure.primaryKey == ["id"] && structure.columns.first?.isPrimaryKey == true)
}

@Test(.enabled(if: postgresEnabled)) func realPostgresFilterAndStructure() async throws {
    let driver = Drivers.make(for: devDB)
    let orders = TableInfo(schema: "public", name: "orders")
    let page = try await driver.fetchRows(
        of: orders, query: RowQuery(sort: [SortKey(column: "id", descending: true)], filter: "id <= 5"), limit: 10, offset: 0)
    #expect(page.rows.count == 5 && page.totalCount == 5 && page.rows.first?.values.first == .int(5))
    do {
        _ = try await driver.fetchRows(of: orders, query: RowQuery(filter: "1 = 1; delete from orders"), limit: 1, offset: 0)
        Issue.record("a second statement must be rejected")
    } catch DatabaseError.query {}
    let structure = try await driver.describeTable(orders)
    #expect(structure.foreignKeys.contains { $0.referencedTable == "users" && $0.columns == ["user_id"] })
    #expect(structure.ddl?.hasPrefix("CREATE TABLE \"public\".\"orders\"") == true)
    await driver.disconnect()
}
