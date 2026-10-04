import Foundation
import SQLite3
import Testing
@testable import DBKit

// Dump → restore through the FFI boundary (Swift → UniFFI → Rust), on temporary SQLite files.
// The dump formats themselves are tested in Rust (`crates/dbcore/tests/dump_*.rs`).

private func temporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("dbear-dump-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

/// Creates a SQLite file by running `sql` (empty `sql`: an empty file, which is a valid empty database).
private func sqliteFile(_ url: URL, _ sql: String = "") {
    FileManager.default.createFile(atPath: url.path, contents: nil)
    guard !sql.isEmpty else { return }
    var db: OpaquePointer?
    #expect(sqlite3_open(url.path, &db) == SQLITE_OK)
    #expect(sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK)
    sqlite3_close(db)
}

private func count(_ url: URL, _ table: String) -> Int {
    var db: OpaquePointer?
    var statement: OpaquePointer?
    sqlite3_open(url.path, &db)
    defer { sqlite3_close(db) }
    guard sqlite3_prepare_v2(db, "select count(*) from \(table)", -1, &statement, nil) == SQLITE_OK else { return -1 }
    defer { sqlite3_finalize(statement) }
    sqlite3_step(statement)
    return Int(sqlite3_column_int64(statement, 0))
}

private func sqliteConfig(_ url: URL) -> ConnectionConfig {
    ConnectionConfig(id: "dump-test", name: "test", group: "", kind: .sqlite, host: "", database: url.path)
}

private final class Events<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [T] = []
    func append(_ item: T) { lock.withLock { items.append(item) } }
    var all: [T] { lock.withLock { items } }
}

@Test func dumpsAndRestoresThroughTheBridge() async throws {
    let dir = try temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let source = dir.appendingPathComponent("source.db")
    sqliteFile(source, """
        create table notes (id integer primary key, body text, data blob);
        with recursive n(i) as (select 1 union all select i + 1 from n where i < 1000)
        insert into notes select i, 'note ' || i, randomblob(4) from n;
        create index notes_body on notes (body);
        """)
    let script = dir.appendingPathComponent(Backups.defaultFileName(database: source.path, compression: .gzip))
    #expect(script.lastPathComponent.hasPrefix("source-") && script.lastPathComponent.hasSuffix(".sql.gz"))

    let events = Events<DumpProgress>()
    let summary = try await Backups.dump(
        sqliteConfig(source), to: script, options: DumpOptions(compression: .gzip), cancellation: BackupCancellation()
    ) { events.append($0) }
    #expect(summary.tables == 1 && summary.rows == 1000 && summary.bytes > 0)
    #expect(events.all.last?.fraction == 1)
    #expect(FileManager.default.fileExists(atPath: script.path))

    let target = dir.appendingPathComponent("target.db")
    sqliteFile(target)
    let restoreEvents = Events<RestoreProgress>()
    let restored = try await Backups.restore(
        sqliteConfig(target), from: script, options: RestoreOptions(), cancellation: BackupCancellation()
    ) { restoreEvents.append($0) }
    #expect(restored.statements > 1000 && restored.errorCount == 0)
    #expect(restoreEvents.all.last?.fraction == 1)
    #expect(count(target, "notes") == 1000)
}

@Test func cancelledDumpThrowsAndLeavesNoFile() async throws {
    let dir = try temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let source = dir.appendingPathComponent("big.db")
    sqliteFile(source, """
        create table t (id integer primary key, payload text);
        with recursive n(i) as (select 1 union all select i + 1 from n where i < 200000)
        insert into t select i, hex(randomblob(32)) from n;
        """)
    let script = dir.appendingPathComponent("big.sql")
    let cancellation = BackupCancellation()
    await #expect(throws: DatabaseError.self) {
        _ = try await Backups.dump(sqliteConfig(source), to: script, options: DumpOptions(), cancellation: cancellation) { progress in
            if progress.phase == .data { cancellation.cancel() }
        }
    }
    #expect(cancellation.isCancelled)
    #expect(!FileManager.default.fileExists(atPath: script.path))
    #expect(!FileManager.default.fileExists(atPath: script.path + ".partial"))
}

@Test func restoreErrorsCarryTheLine() async throws {
    let dir = try temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let script = dir.appendingPathComponent("bad.sql")
    try "create table a (x);\ninsert into nope values (1);\n".write(to: script, atomically: true, encoding: .utf8)
    let target = dir.appendingPathComponent("t.db")
    sqliteFile(target)
    do {
        _ = try await Backups.restore(sqliteConfig(target), from: script, options: RestoreOptions(), cancellation: BackupCancellation()) { _ in }
        Issue.record("expected an error")
    } catch let DatabaseError.query(message) {
        #expect(message.hasPrefix("Line 2:"))
    }
}
