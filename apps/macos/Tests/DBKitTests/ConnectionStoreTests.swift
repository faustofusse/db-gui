import Foundation
import Testing
@testable import DBKit

private func tempStorePath() -> String {
    FileManager.default.temporaryDirectory
        .appendingPathComponent("dbgui-tests-\(UUID().uuidString)/connections.json").path
}

@Test func storeRoundTripsWithoutPasswords() throws {
    let path = tempStorePath()
    defer { try? FileManager.default.removeItem(atPath: (path as NSString).deletingLastPathComponent) }

    let store = try ConnectionStore.open(path: path)
    #expect(store.connections().isEmpty)

    var draft = ConnectionConfig.blank()
    draft.name = "  prod  "
    draft.database = "app"
    draft.user = "readonly"
    draft.password = "hunter2"
    let saved = try store.upsert(draft)
    #expect(!saved.id.isEmpty && saved.name == "prod" && saved.password == nil)

    let json = try String(contentsOfFile: path, encoding: .utf8)
    #expect(!json.contains("hunter2"))
    #expect(try ConnectionStore.open(path: path).connections() == [saved])

    #expect(try store.remove(id: saved.id))
    #expect(store.connections().isEmpty)
}

@Test func storeRejectsInvalidConfig() throws {
    let store = try ConnectionStore.open(path: tempStorePath())
    #expect(throws: DatabaseError.self) { try store.upsert(.blank()) }
    #expect(ConnectionConfig.blank().validationError == "Enter a name for the connection.")
}

@Test func parsesConnectionURL() throws {
    let c = try ConnectionConfig.parse(url: "postgres://me:p%40ss@db.example.com:6543/shop?sslmode=require")
    #expect(c.host == "db.example.com" && c.port == 6543 && c.database == "shop" && c.name == "shop")
    #expect(c.user == "me" && c.password == "p@ss" && c.sslMode == .require)
    #expect(c.url() == "postgres://me@db.example.com:6543/shop?sslmode=require")
    #expect(DatabaseKind.postgres.defaultPort == 5432)
}

@Test func inMemorySecrets() throws {
    let secrets = InMemorySecretStore()
    try secrets.setPassword("x", for: "a")
    #expect(secrets.hasPassword(for: "a") && secrets.password(for: "a") == "x")
    secrets.deletePassword(for: "a")
    #expect(!secrets.hasPassword(for: "a"))
}
