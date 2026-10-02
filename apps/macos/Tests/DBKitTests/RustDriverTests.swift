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
