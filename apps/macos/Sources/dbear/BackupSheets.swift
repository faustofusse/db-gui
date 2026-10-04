import AppKit
import DBKit
import SwiftUI

// MARK: - Entry points

extension AppModel {
    /// A saved connection with its Keychain password, pointed at `database` (nil: its own).
    func backupTarget(_ connection: ConnectionConfig, database: String?) -> ConnectionConfig {
        var config = connections.first { $0.id == connection.id } ?? connection
        if let database, database != config.database { config = config.withDatabase(database) }
        if config.password == nil { config.password = savedPassword(config.id) }
        return config
    }

    /// The database shown for `connection` when it's the selected one, else its own.
    private func shownDatabase(of connection: ConnectionConfig) -> String? {
        connection.id == selectedConnectionID ? selectedTarget?.database : nil
    }

    /// Opens the dump options sheet ("Dump Database…", "Dump Schema…", "Dump Table…").
    func requestDump(of connection: ConnectionConfig, preset: DumpPreset = .database) {
        dumpRequest = DumpRequest(
            target: backupTarget(connection, database: shownDatabase(of: connection)),
            databases: databases(of: connection) ?? [],
            preset: preset
        )
    }

    /// Asks for a SQL file, then confirms where to run it.
    func requestRestore(into connection: ConnectionConfig) {
        let panel = NSOpenPanel()
        panel.title = "Restore from File"
        panel.message = "Choose a SQL dump (.sql or .sql.gz) to run against “\(BackupCenter.name(of: backupTarget(connection, database: shownDatabase(of: connection))))”."
        panel.prompt = "Choose"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        guard panel.runModal() == .OK, let file = panel.url else { return }
        restoreRequest = RestoreRequest(
            target: backupTarget(connection, database: shownDatabase(of: connection)),
            databases: databases(of: connection) ?? [],
            file: file
        )
    }
}

// MARK: - Dump sheet

struct DumpSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let request: DumpRequest

    @State private var database: String
    @State private var schemas: LoadState<[Schema]> = .loading
    @State private var everything: Bool
    @State private var selected: Set<TableInfo.ID> = []
    @State private var content: DumpContent = .schemaAndData
    @State private var compression: DumpCompression = .none
    @State private var dataStyle: DumpDataStyle = .copy
    @State private var dropObjects = false
    @State private var createDatabase = false

    init(request: DumpRequest) {
        self.request = request
        _database = State(initialValue: request.target.database)
        _everything = State(initialValue: request.preset == .database)
    }

    private var target: ConnectionConfig {
        database == request.target.database ? request.target : request.target.withDatabase(database)
    }

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(title: "Dump “\(BackupCenter.name(of: target))”", subtitle: target.summary)
            Form {
                Section {
                    LabeledContent("Connection", value: request.target.name)
                    if request.databases.count > 1 {
                        Picker("Database", selection: $database) {
                            ForEach(request.databases, id: \.self) { Text($0).tag($0) }
                        }
                    } else {
                        LabeledContent("Database", value: BackupCenter.name(of: target))
                    }
                }
                Section("Contents") {
                    Picker("Dump", selection: $everything) {
                        Text("Whole database").tag(true)
                        Text("Selected schemas and tables").tag(false)
                    }
                    .pickerStyle(.radioGroup)
                    if !everything { tablePicker }
                }
                Section("Options") {
                    Picker("Include", selection: $content) {
                        Text("Schema and data").tag(DumpContent.schemaAndData)
                        Text("Schema only").tag(DumpContent.schemaOnly)
                        Text("Data only").tag(DumpContent.dataOnly)
                    }
                    Picker("Format", selection: $compression) {
                        Text("SQL (.sql)").tag(DumpCompression.none)
                        Text("Gzipped SQL (.sql.gz)").tag(DumpCompression.gzip)
                    }
                    if target.kind == .postgres && content != .schemaOnly {
                        Picker("Rows as", selection: $dataStyle) {
                            Text("COPY (fast, psql)").tag(DumpDataStyle.copy)
                            Text("INSERT statements").tag(DumpDataStyle.insert)
                        }
                    }
                    if content != .dataOnly {
                        Toggle("Drop existing objects before creating them", isOn: $dropObjects)
                    }
                    if target.kind == .mysql && content != .dataOnly {
                        Toggle("Include CREATE DATABASE and USE", isOn: $createDatabase)
                    }
                }
            }
            .formStyle(.grouped)

            HStack {
                if case .failed(let message) = schemas, !everything {
                    Text(message).foregroundStyle(.red).lineLimit(2).font(.callout)
                }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Dump…") { chooseFile() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!everything && selected.isEmpty)
            }
            .padding([.horizontal, .bottom], 20)
            .padding(.top, 4)
        }
        .frame(width: 520)
        .navigationTitle("Dump Database")
        .task(id: database) { await loadSchemas() }
    }

    // MARK: Tables

    @ViewBuilder
    private var tablePicker: some View {
        switch schemas {
        case .idle, .loading:
            ProgressView().controlSize(.small).frame(maxWidth: .infinity)
        case .failed(let message):
            Text(message).foregroundStyle(.secondary)
        case .loaded(let schemas):
            List {
                ForEach(schemas) { schema in
                    Toggle(isOn: schemaBinding(schema)) {
                        Text(schema.name).fontWeight(.semibold)
                    }
                    ForEach(schema.tables) { table in
                        Toggle(isOn: tableBinding(table)) {
                            Label(table.name, systemImage: table.kind == .view ? "eye" : "tablecells")
                        }
                        .padding(.leading, 20)
                    }
                }
            }
            .frame(height: 220)
            .listStyle(.bordered(alternatesRowBackgrounds: false))
        }
    }

    private func schemaBinding(_ schema: Schema) -> Binding<Bool> {
        Binding(
            get: { !schema.tables.isEmpty && schema.tables.allSatisfy { selected.contains($0.id) } },
            set: { on in
                for table in schema.tables {
                    if on { selected.insert(table.id) } else { selected.remove(table.id) }
                }
            }
        )
    }

    private func tableBinding(_ table: TableInfo) -> Binding<Bool> {
        Binding(
            get: { selected.contains(table.id) },
            set: { on in if on { selected.insert(table.id) } else { selected.remove(table.id) } }
        )
    }

    private func loadSchemas() async {
        if target.driverKey == model.selectedTarget?.driverKey, let loaded = model.schemas.value {
            schemas = .loaded(loaded)
        } else {
            schemas = .loading
            let driver = Drivers.make(for: target)
            do {
                schemas = .loaded(try await driver.listSchemas())
            } catch {
                schemas = .failed(error.localizedDescription)
            }
            await driver.disconnect()
        }
        applyPreset()
    }

    private func applyPreset() {
        guard let loaded = schemas.value else { return }
        switch request.preset {
        case .database:
            selected = []
        case .schema(let name):
            selected = Set(loaded.first { $0.name == name }?.tables.map(\.id) ?? [])
        case .table(let table):
            selected = [table.id]
        }
    }

    /// Whole database, whole schemas (with their functions, types…), or just some tables.
    private var scope: DumpScope {
        guard !everything, let loaded = schemas.value else { return .database }
        let all = loaded.flatMap(\.tables)
        if all.allSatisfy({ selected.contains($0.id) }) && loaded.allSatisfy({ !$0.tables.isEmpty }) { return .database }
        let whole = loaded.filter { !$0.tables.isEmpty && $0.tables.allSatisfy { selected.contains($0.id) } }
        let rest = all.filter { table in selected.contains(table.id) && !whole.contains { $0.name == table.schema } }
        if rest.isEmpty { return .schemas(whole.map(\.name)) }
        return .tables(all.filter { selected.contains($0.id) })
    }

    private func chooseFile() {
        let options = DumpOptions(
            content: content, scope: scope, compression: compression,
            dataStyle: target.kind == .postgres ? dataStyle : .insert,
            dropObjects: content != .dataOnly && dropObjects,
            createDatabase: target.kind == .mysql && content != .dataOnly && createDatabase
        )
        let panel = NSSavePanel()
        panel.title = "Dump Database"
        panel.prompt = "Dump"
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false
        panel.nameFieldStringValue = Backups.defaultFileName(database: BackupCenter.name(of: target), compression: compression)
        let target = target
        let start = { (response: NSApplication.ModalResponse) in
            guard response == .OK, let url = panel.url else { return }
            model.backups.dump(target, to: url, options: options)
            dismiss()
        }
        if let window = NSApp.keyWindow {
            panel.beginSheetModal(for: window, completionHandler: start)
        } else {
            start(panel.runModal())
        }
    }
}

// MARK: - Restore sheet

struct RestoreSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let request: RestoreRequest

    @State private var database: String
    @State private var singleTransaction: Bool
    @State private var stopOnError = true

    init(request: RestoreRequest) {
        self.request = request
        _database = State(initialValue: request.target.database)
        // MySQL commits schema changes as it goes: a transaction can't make it all or nothing.
        _singleTransaction = State(initialValue: request.target.kind != .mysql)
    }

    private var target: ConnectionConfig {
        database == request.target.database ? request.target : request.target.withDatabase(database)
    }

    private var fileSize: String {
        let bytes = (try? request.file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        return ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(title: "Restore into “\(BackupCenter.name(of: target))”", subtitle: target.summary)
            Form {
                Section {
                    LabeledContent("File") {
                        Text("\(request.file.lastPathComponent) · \(fileSize)")
                            .help(request.file.path)
                    }
                    LabeledContent("Connection", value: request.target.name)
                    if request.databases.count > 1 {
                        Picker("Database", selection: $database) {
                            ForEach(request.databases, id: \.self) { Text($0).tag($0) }
                        }
                    } else {
                        LabeledContent("Database", value: BackupCenter.name(of: target))
                    }
                } footer: {
                    Text("Every statement in the file runs against this database. Objects with the same names may be replaced, or make the restore fail. To restore into a new database, create it first (e.g. in a SQL script).")
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Section {
                    if target.kind != .mysql {
                        Toggle("All or nothing (one transaction)", isOn: $singleTransaction)
                    }
                    Toggle("Stop at the first error", isOn: $stopOnError)
                        .disabled(singleTransaction && target.kind != .mysql)
                }
            }
            .formStyle(.grouped)

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Restore") {
                    let transaction = target.kind != .mysql && singleTransaction
                    let options = RestoreOptions(singleTransaction: transaction, stopOnError: transaction || stopOnError)
                    model.backups.restore(target, from: request.file, options: options)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
            .padding([.horizontal, .bottom], 20)
            .padding(.top, 4)
        }
        .frame(width: 480)
        .navigationTitle("Restore from File")
    }
}

private struct SheetHeader: View {
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.headline)
            Text(subtitle).font(.callout).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding([.horizontal, .top], 20)
    }
}

// MARK: - Jobs panel

/// Running and finished dumps/restores, floating over the window's bottom-trailing corner.
struct BackupJobsPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 8) {
            ForEach(model.backups.jobs) { job in
                BackupJobCard(job: job, center: model.backups)
            }
        }
        .frame(width: 340)
        .padding(model.backups.jobs.isEmpty ? 0 : 16)
    }
}

private struct BackupJobCard: View {
    let job: BackupJob
    let center: BackupCenter

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .font(.title2)
                .foregroundStyle(tint)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 4) {
                Text(job.title).fontWeight(.semibold).lineLimit(1)
                switch job.state {
                case .running:
                    if let fraction = job.fraction {
                        ProgressView(value: fraction).controlSize(.small)
                    } else {
                        ProgressView().progressViewStyle(.linear).controlSize(.small)
                    }
                    Text(job.detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                case .finished(let message):
                    Text(message).font(.caption).foregroundStyle(.secondary)
                    if !job.warnings.isEmpty {
                        Text(BackupCenter.count(job.warnings.count, job.kind == .dump ? "warning" : "note"))
                            .font(.caption)
                            .foregroundStyle(.orange)
                            .help(job.warnings.prefix(20).joined(separator: "\n"))
                    }
                case .failed(let message):
                    Text(message).font(.caption).foregroundStyle(.red).lineLimit(4).textSelection(.enabled)
                case .cancelled:
                    Text("Cancelled").font(.caption).foregroundStyle(.secondary)
                }
                if case .finished = job.state, job.kind == .dump {
                    Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([job.file]) }
                        .buttonStyle(.link)
                        .font(.caption)
                }
            }
            Spacer(minLength: 0)
            Button {
                if job.isRunning { center.cancel(job) } else { center.dismiss(job) }
            } label: {
                Image(systemName: job.isRunning ? "stop.circle.fill" : "xmark.circle.fill")
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help(job.isRunning ? "Stop" : "Close")
        }
        .padding(12)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
        .shadow(color: .black.opacity(0.12), radius: 8, y: 2)
    }

    private var icon: String {
        switch job.state {
        case .running: job.kind == .dump ? "square.and.arrow.down" : "square.and.arrow.up"
        case .finished: "checkmark.circle.fill"
        case .failed: "exclamationmark.triangle.fill"
        case .cancelled: "xmark.circle"
        }
    }

    private var tint: Color {
        switch job.state {
        case .running: .accentColor
        case .finished: .green
        case .failed: .red
        case .cancelled: .secondary
        }
    }
}

