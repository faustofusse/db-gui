import AppKit
import DBKit
import Observation
import UserNotifications

/// "Dump Database…" waiting for its options sheet.
struct DumpRequest: Identifiable {
    let id = UUID()
    /// The connection (with its password), pointed at the database to dump.
    var target: ConnectionConfig
    /// Databases the user can pick from instead (servers with "show all databases").
    var databases: [String]
    /// What to preselect.
    var preset: DumpPreset
}

enum DumpPreset: Hashable {
    case database
    case schema(String)
    case table(TableInfo)
}

/// "Restore from File…" waiting for confirmation.
struct RestoreRequest: Identifiable {
    let id = UUID()
    var target: ConnectionConfig
    var databases: [String]
    var file: URL
}

/// A running or finished dump/restore, shown in the jobs panel.
@MainActor
@Observable
final class BackupJob: Identifiable {
    enum Kind { case dump, restore }
    enum State {
        case running
        case finished(String)
        case failed(String)
        case cancelled
    }

    let id = UUID()
    let kind: Kind
    /// The database, e.g. `app_dev`.
    let name: String
    let file: URL
    /// The connection and database, to refresh after a restore.
    let target: ConnectionConfig
    let cancellation = BackupCancellation()
    var state: State = .running
    /// 0…1 when known.
    var fraction: Double?
    var detail = "Connecting…"
    var warnings: [String] = []

    init(kind: Kind, file: URL, target: ConnectionConfig) {
        self.kind = kind
        self.name = BackupCenter.name(of: target)
        self.file = file
        self.target = target
    }

    var isRunning: Bool { if case .running = state { true } else { false } }

    /// "Dumping app_dev", "Dumped app_dev", "Couldn’t restore into app_dev"…
    var title: String {
        switch (kind, state) {
        case (.dump, .running): "Dumping \(name)"
        case (.dump, .finished): "Dumped \(name)"
        case (.dump, .failed): "Couldn’t dump \(name)"
        case (.dump, .cancelled): "Dump of \(name) stopped"
        case (.restore, .running): "Restoring into \(name)"
        case (.restore, .finished): "Restored into \(name)"
        case (.restore, .failed): "Couldn’t restore into \(name)"
        case (.restore, .cancelled): "Restore into \(name) stopped"
        }
    }
}

/// Runs dumps and restores in the background (several at once; closing a sheet doesn't stop them),
/// tracks their progress and posts a notification when one ends while dbear isn't frontmost.
@MainActor
@Observable
final class BackupCenter {
    private(set) var jobs: [BackupJob] = []
    /// Called after a restore finishes (successfully or not): the schema may have changed.
    @ObservationIgnored var onRestored: ((ConnectionConfig) -> Void)?

    func dump(_ target: ConnectionConfig, to file: URL, options: DumpOptions) {
        let job = BackupJob(kind: .dump, file: file, target: target)
        jobs.append(job)
        Self.requestNotificationPermission()
        Task {
            do {
                let summary = try await Backups.dump(target, to: file, options: options, cancellation: job.cancellation) { progress in
                    Task { @MainActor in job.update(progress) }
                }
                let size = ByteCountFormatter.string(fromByteCount: Int64(summary.bytes), countStyle: .file)
                job.fraction = 1
                job.warnings = summary.warnings
                job.state = .finished("\(Self.count(summary.tables, "table")), \(Self.count(summary.rows, "row")) · \(size)")
            } catch {
                job.state = Self.state(for: error)
            }
            notify(job)
        }
    }

    func restore(_ target: ConnectionConfig, from file: URL, options: RestoreOptions) {
        let job = BackupJob(kind: .restore, file: file, target: target)
        job.detail = file.lastPathComponent
        jobs.append(job)
        Self.requestNotificationPermission()
        Task {
            do {
                let summary = try await Backups.restore(target, from: file, options: options, cancellation: job.cancellation) { progress in
                    Task { @MainActor in job.update(progress) }
                }
                job.fraction = 1
                job.warnings = summary.errors + summary.warnings
                var message = Self.count(summary.statements, "statement")
                if summary.rows > 0 { message += ", \(Self.count(summary.rows, "row")) copied" }
                if summary.errorCount > 0 { message += " · \(Self.count(summary.errorCount, "error"))" }
                job.state = .finished(message)
            } catch {
                job.state = Self.state(for: error)
            }
            onRestored?(target)
            notify(job)
        }
    }

    func cancel(_ job: BackupJob) {
        job.cancellation.cancel()
    }

    func dismiss(_ job: BackupJob) {
        if job.isRunning { job.cancellation.cancel() }
        jobs.removeAll { $0.id == job.id }
    }

    static func name(of target: ConnectionConfig) -> String {
        if target.kind == .sqlite { return URL(fileURLWithPath: target.database).lastPathComponent }
        return target.database.isEmpty ? target.name : target.database
    }

    static func count(_ n: Int, _ noun: String) -> String {
        "\(n.formatted()) \(noun)\(n == 1 ? "" : "s")"
    }

    private static func state(for error: Error) -> BackupJob.State {
        if case DatabaseError.cancelled = error { return .cancelled }
        return .failed(error.localizedDescription)
    }

    // MARK: Notifications

    /// Notifications need an app bundle (`swift run` has none).
    private static var canNotify: Bool { Bundle.main.bundleIdentifier != nil }

    private static func requestNotificationPermission() {
        guard canNotify else { return }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    private func notify(_ job: BackupJob) {
        guard Self.canNotify, !NSApp.isActive else { return }
        let content = UNMutableNotificationContent()
        switch (job.kind, job.state) {
        case (.dump, .finished(let message)):
            content.title = "Dump finished"
            content.body = "\(job.file.lastPathComponent): \(message)"
        case (.restore, .finished(let message)):
            content.title = "Restore finished"
            content.body = "\(Self.name(of: job.target)): \(message)"
        case (_, .failed(let message)):
            content.title = job.kind == .dump ? "Dump failed" : "Restore failed"
            content.body = message
        default:
            return
        }
        content.sound = .default
        let request = UNNotificationRequest(identifier: job.id.uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }
}

extension BackupJob {
    func update(_ progress: DumpProgress) {
        guard isRunning else { return }
        fraction = progress.fraction
        let size = ByteCountFormatter.string(fromByteCount: Int64(progress.bytesWritten), countStyle: .file)
        switch progress.phase {
        case .connecting:
            detail = "Connecting…"
        case .schema:
            detail = "Writing the schema…"
        case .data:
            let table = progress.object.map { "\($0) · " } ?? ""
            detail = "\(table)\(progress.tablesDone) of \(progress.tablesTotal) tables · \(size)"
        case .postData:
            detail = "Writing indexes, constraints and views… · \(size)"
        case .finishing:
            detail = "Finishing…"
        }
    }

    func update(_ progress: RestoreProgress) {
        guard isRunning else { return }
        fraction = progress.fraction
        let read = ByteCountFormatter.string(fromByteCount: Int64(progress.bytesRead), countStyle: .file)
        let total = ByteCountFormatter.string(fromByteCount: Int64(progress.bytesTotal), countStyle: .file)
        var text = "\(read) of \(total) · \(BackupCenter.count(progress.statements, "statement"))"
        if progress.errors > 0 { text += " · \(BackupCenter.count(progress.errors, "error"))" }
        detail = text
    }
}
