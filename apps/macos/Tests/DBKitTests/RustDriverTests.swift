import Testing
@testable import DBKit

// End-to-end through the FFI boundary: Swift → UniFFI → Rust core (mock driver).
// Core behaviour itself is tested in Rust (`cargo test -p dbcore`).

private let connections = Drivers.sampleConnections()
private var appDev: ConnectionConfig { connections.first { $0.id == "local-pg" }! }

@Test func loadsSampleConnectionsWithSummary() {
    #expect(connections.count == 6)
    #expect(appDev.summary == "PostgreSQL · localhost:5432/app_dev")
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
