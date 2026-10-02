import Foundation
import Testing
@testable import DBKit

// DBeaver import through the FFI; parsing details are tested in Rust (`import::dbeaver`).

@Test func importsDBeaverConnectionsFromAFolder() throws {
    let dir = FileManager.default.temporaryDirectory.appending(path: "dbeaver-\(UUID())/workspace6/General/.dbeaver")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()) }
    let json = """
    {"connections": {
      "postgres-jdbc-1": {"provider": "postgresql", "driver": "postgres-jdbc", "name": "Billing", "folder": "prod",
        "configuration": {"host": "db.example.com", "port": "5432", "database": "billing", "user": "app"}},
      "mysql8-2": {"provider": "mysql", "driver": "mysql8", "name": "Shop",
        "configuration": {"url": "jdbc:mysql://shop:3306/shop", "configurationType": "URL"}},
      "azure-3": {"provider": "sqlserver", "driver": "azure", "name": "Azure", "configuration": {}}
    }}
    """
    try json.write(to: dir.appending(path: "data-sources.json"), atomically: true, encoding: .utf8)

    let existing = try ConnectionConfig.parse(url: "postgres://app@db.example.com/billing")
    let scan = try DBeaverImport.scan(path: dir.path, existing: [existing])
    #expect(scan.connections.map(\.config.name) == ["Shop", "Billing"])
    let billing = try #require(scan.connections.first { $0.config.name == "Billing" })
    #expect(billing.alreadyAdded && billing.config.group == "prod" && billing.config.kind == .postgres)
    #expect(scan.connections.first { $0.config.name == "Shop" }?.alreadyAdded == false)
    #expect(scan.skipped.map(\.reason) == ["SQL Server isn’t supported yet."])

    #expect(throws: DatabaseError.self) { try DBeaverImport.scan(path: "/nonexistent", existing: []) }
}
