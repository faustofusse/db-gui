import Foundation

// Database dumps and restores (`dbcore::dump` / `dbcore::restore`), bridged in `RustDriver.swift`.

public enum DumpContent: String, Sendable, Hashable, CaseIterable {
    case schemaAndData, schemaOnly, dataOnly
}

/// Which objects to dump. A MySQL connection dumps its one database and SQLite its `main`.
public enum DumpScope: Sendable, Hashable {
    case database
    /// Whole schemas: their tables plus types, functions, sequences…
    case schemas([String])
    /// Just these tables and views (with their indexes, triggers and the types they use).
    case tables([TableInfo])
}

public enum DumpCompression: String, Sendable, Hashable, CaseIterable {
    case none, gzip
}

/// How rows are written. `copy` is Postgres only; other engines always use INSERTs.
public enum DumpDataStyle: String, Sendable, Hashable, CaseIterable {
    case copy, insert
}

public struct DumpOptions: Sendable, Hashable {
    public var content: DumpContent
    public var scope: DumpScope
    public var compression: DumpCompression
    public var dataStyle: DumpDataStyle
    /// `DROP … IF EXISTS` before each `CREATE`.
    public var dropObjects: Bool
    /// MySQL: `CREATE DATABASE` + `USE`, so it restores into the same database name.
    public var createDatabase: Bool

    public init(
        content: DumpContent = .schemaAndData, scope: DumpScope = .database, compression: DumpCompression = .none,
        dataStyle: DumpDataStyle = .copy, dropObjects: Bool = false, createDatabase: Bool = false
    ) {
        self.content = content
        self.scope = scope
        self.compression = compression
        self.dataStyle = dataStyle
        self.dropObjects = dropObjects
        self.createDatabase = createDatabase
    }
}

public enum DumpPhase: Sendable, Hashable {
    case connecting, schema, data, postData, finishing
}

public struct DumpProgress: Sendable, Hashable {
    public var phase: DumpPhase
    /// The table being written, e.g. `public.users`.
    public var object: String?
    public var tablesDone: Int
    public var tablesTotal: Int
    public var rowsDone: Int
    public var tableRowsDone: Int
    /// The planner's estimate of the current table's rows, when known.
    public var tableRowsEstimate: Int?
    /// Uncompressed SQL written so far.
    public var bytesWritten: Int

    public init(phase: DumpPhase, object: String? = nil, tablesDone: Int = 0, tablesTotal: Int = 0, rowsDone: Int = 0,
                tableRowsDone: Int = 0, tableRowsEstimate: Int? = nil, bytesWritten: Int = 0) {
        self.phase = phase
        self.object = object
        self.tablesDone = tablesDone
        self.tablesTotal = tablesTotal
        self.rowsDone = rowsDone
        self.tableRowsDone = tableRowsDone
        self.tableRowsEstimate = tableRowsEstimate
        self.bytesWritten = bytesWritten
    }

    /// Overall completion (0…1), counting the current table's share when its size is known.
    public var fraction: Double? {
        guard tablesTotal > 0 else { return nil }
        var done = Double(tablesDone)
        if let estimate = tableRowsEstimate, estimate > 0, tablesDone < tablesTotal {
            done += min(1, Double(tableRowsDone) / Double(estimate))
        }
        return min(1, done / Double(tablesTotal))
    }
}

public struct DumpSummary: Sendable, Hashable {
    public var tables: Int
    public var rows: Int
    /// File size on disk.
    public var bytes: Int
    /// Objects that were skipped or may not restore exactly.
    public var warnings: [String]

    public init(tables: Int, rows: Int, bytes: Int, warnings: [String]) {
        self.tables = tables
        self.rows = rows
        self.bytes = bytes
        self.warnings = warnings
    }
}

public struct RestoreOptions: Sendable, Hashable {
    /// All or nothing (Postgres, SQLite); MySQL commits schema changes as it goes.
    public var singleTransaction: Bool
    public var stopOnError: Bool

    public init(singleTransaction: Bool = true, stopOnError: Bool = true) {
        self.singleTransaction = singleTransaction
        self.stopOnError = stopOnError
    }
}

public struct RestoreProgress: Sendable, Hashable {
    public var bytesRead: Int
    public var bytesTotal: Int
    public var statements: Int
    public var errors: Int

    public init(bytesRead: Int, bytesTotal: Int, statements: Int, errors: Int) {
        self.bytesRead = bytesRead
        self.bytesTotal = bytesTotal
        self.statements = statements
        self.errors = errors
    }

    public var fraction: Double? {
        bytesTotal > 0 ? min(1, Double(bytesRead) / Double(bytesTotal)) : nil
    }
}

public struct RestoreSummary: Sendable, Hashable {
    public var statements: Int
    public var rows: Int
    /// Failed statements (when not stopping on errors), with their line.
    public var errors: [String]
    public var errorCount: Int
    public var warnings: [String]

    public init(statements: Int, rows: Int, errors: [String], errorCount: Int, warnings: [String]) {
        self.statements = statements
        self.rows = rows
        self.errors = errors
        self.errorCount = errorCount
        self.warnings = warnings
    }
}

/// Cancels a running dump or restore: it stops at once and throws `DatabaseError.cancelled`.
public final class BackupCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    private var handlers: [@Sendable () -> Void] = []

    public init() {}

    public var isCancelled: Bool { lock.withLock { cancelled } }

    public func cancel() {
        let run = lock.withLock { () -> [@Sendable () -> Void] in
            guard !cancelled else { return [] }
            cancelled = true
            defer { handlers = [] }
            return handlers
        }
        run.forEach { $0() }
    }

    /// Runs `handler` on cancel (right away if already cancelled).
    func onCancel(_ handler: @escaping @Sendable () -> Void) {
        let now = lock.withLock { () -> Bool in
            if cancelled { return true }
            handlers.append(handler)
            return false
        }
        if now { handler() }
    }
}

public enum Backups {
    /// Dumps the database `config` points at into `url`. The file only appears once complete.
    /// `progress` is called from a background thread, about 10 times a second.
    public static func dump(
        _ config: ConnectionConfig, to url: URL, options: DumpOptions, cancellation: BackupCancellation,
        progress: @escaping @Sendable (DumpProgress) -> Void
    ) async throws -> DumpSummary {
        try await RustDriver.dump(config, to: url, options: options, cancellation: cancellation, progress: progress)
    }

    /// Runs the SQL script at `url` (plain or gzipped) against the database `config` points at.
    public static func restore(
        _ config: ConnectionConfig, from url: URL, options: RestoreOptions, cancellation: BackupCancellation,
        progress: @escaping @Sendable (RestoreProgress) -> Void
    ) async throws -> RestoreSummary {
        try await RustDriver.restore(config, from: url, options: options, cancellation: cancellation, progress: progress)
    }

    /// `app_dev-2025-06-01.sql` (`.sql.gz` when gzipped).
    public static func defaultFileName(database: String, date: Date = Date(), compression: DumpCompression) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return RustDriver.defaultDumpFileName(database: database, date: formatter.string(from: date), compression: compression)
    }
}
