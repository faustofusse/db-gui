import Testing
@testable import DBCore

@Test func listsSchemasForPostgres() async throws {
    let driver = Drivers.make(for: MockData.connections[0])
    let schemas = try await driver.listSchemas()
    #expect(schemas.map(\.name) == ["analytics", "billing", "public"])
}

@Test func fetchesRowsWithMatchingColumnCount() async throws {
    let driver = Drivers.make(for: MockData.connections[0])
    let users = TableInfo(schema: "public", name: "users")
    let result = try await driver.fetchRows(of: users, limit: 50, offset: 0)
    #expect(result.rows.count == 50)
    #expect(result.rows.allSatisfy { $0.values.count == result.columns.count })
}

@Test func unreachableConnectionThrows() async {
    let config = MockData.connections.first { MockData.unreachable.contains($0.id) }!
    await #expect(throws: DatabaseError.self) {
        try await Drivers.make(for: config).listSchemas()
    }
}

@Test func executesSimpleSelect() async throws {
    let driver = Drivers.make(for: MockData.connections[0])
    let result = try await driver.execute("-- comment\nselect id, email from users limit 5;")
    #expect(result.columns.map(\.name) == ["id", "email"])
    #expect(result.rows.count == 5)
}

@Test func rejectsUnsupportedSQL() async {
    let driver = Drivers.make(for: MockData.connections[0])
    await #expect(throws: DatabaseError.self) { try await driver.execute("delete from users") }
}
