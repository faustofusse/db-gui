import DBKit
import SwiftUI

/// "New Connection" / "Edit Connection" sheet.
struct ConnectionEditor: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    let original: ConnectionConfig?

    @State private var draft: ConnectionConfig
    @State private var password = ""
    /// The password field was touched; otherwise the Keychain copy is kept as is.
    @State private var passwordEdited = false
    @State private var hasSavedPassword = false
    @State private var url = ""
    @State private var urlError: String?
    @State private var test: TestState = .idle
    @State private var saveError: String?

    enum TestState: Equatable {
        case idle, running, succeeded
        case failed(String)
    }

    init(original: ConnectionConfig?) {
        self.original = original
        _draft = State(initialValue: original ?? .blank(.postgres))
    }

    private var isNew: Bool { original == nil }
    private var validationError: String? { draft.validationError }

    var body: some View {
        VStack(spacing: 0) {
            header
            Form {
                if isNew { urlSection }
                generalSection
                serverSection
                authSection
            }
            .formStyle(.grouped)
            .scrollBounceBehavior(.basedOnSize)
            footer
        }
        .frame(width: 520)
        .fixedSize(horizontal: false, vertical: true)
        .onAppear { hasSavedPassword = model.hasSavedPassword(draft.id) }
        .onChange(of: draft) { test = .idle; saveError = nil }
        .onChange(of: password) { test = .idle }
    }

    // MARK: Sections

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: draft.kind.symbolName)
                .font(.system(size: 22, weight: .regular))
                .foregroundStyle(.white)
                .frame(width: 40, height: 40)
                .background(.tint, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text(isNew ? "New Connection" : "Edit Connection")
                    .font(.headline)
                Text(draft.validationError == nil ? draft.refreshed.summary : draft.kind.displayName)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.top, 20)
    }

    private var urlSection: some View {
        Section {
            TextField("URL", text: $url, prompt: Text(verbatim: "postgres://user:password@host:5432/database"))
                .textContentType(.URL)
                .onChange(of: url) { apply(url: url) }
        } footer: {
            Text(urlError ?? "Paste a connection URL to fill in the fields below.")
                .foregroundStyle(urlError == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(.red))
                .font(.caption)
        }
    }

    private var generalSection: some View {
        Section {
            // Left empty, the name is the database (or host); the placeholder shows which.
            TextField("Name", text: $draft.name, prompt: Text(verbatim: draft.defaultName.isEmpty ? "Optional" : draft.defaultName))
            LabeledContent("Group") {
                HStack(spacing: 4) {
                    TextField("Group", text: $draft.group, prompt: Text("None"))
                        .labelsHidden()
                    if !existingGroups.isEmpty {
                        Menu {
                            ForEach(existingGroups, id: \.self) { group in
                                Button(group) { draft.group = group }
                            }
                        } label: {
                            Image(systemName: "chevron.up.chevron.down")
                        }
                        .menuStyle(.borderlessButton)
                        .menuIndicator(.hidden)
                        .fixedSize()
                        .help("Choose an existing group")
                    }
                }
            }
        }
    }

    private var serverSection: some View {
        Section("Server") {
            Picker("Type", selection: $draft.kind) {
                ForEach(DatabaseKind.allCases, id: \.self) { kind in
                    Text(kind == .postgres ? kind.displayName : "\(kind.displayName) (coming soon)")
                        .tag(kind)
                        .selectionDisabled(kind != .postgres)
                }
            }
            TextField("Host", text: $draft.host, prompt: Text(verbatim: "localhost"))
            TextField(
                "Port", value: $draft.port, format: .number.grouping(.never),
                prompt: Text(verbatim: draft.kind.defaultPort.map(String.init) ?? "")
            )
            TextField("Database", text: $draft.database, prompt: Text(verbatim: databasePrompt))
            if draft.supportsMultipleDatabases {
                Toggle(isOn: $draft.showAllDatabases) {
                    Text("Show all databases")
                    Text("List every database on the server in the sidebar. The one above opens by default.")
                }
            }
        }
    }

    /// SQLite needs its file; servers fall back to their default database.
    private var databasePrompt: String {
        if draft.kind == .sqlite { return "Required" }
        let fallback = ConnectionConfig(id: "", name: "", group: "", kind: draft.kind, host: "", database: "").defaultDatabase
        return fallback.isEmpty ? "Optional" : "Optional (\(fallback))"
    }

    private var authSection: some View {
        Section {
            TextField("User", text: userBinding, prompt: Text(verbatim: "postgres"))
            SecureField("Password", text: $password, prompt: Text(passwordPrompt))
                .onChange(of: password) { passwordEdited = true }
            Picker("SSL", selection: $draft.sslMode) {
                Text("Disable").tag(SslMode.disable)
                Text("Prefer").tag(SslMode.prefer)
                Text("Require").tag(SslMode.require)
                Text("Verify Certificate").tag(SslMode.verifyFull)
            }
        } header: {
            Text("Authentication")
        } footer: {
            Text("Passwords are stored in your login Keychain, never in the connections file.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Button("Test Connection") { Task { await runTest() } }
                .disabled(validationError != nil || test == .running)
            testStatus
            Spacer(minLength: 12)
            Button("Cancel", role: .cancel) { dismiss() }
                .keyboardShortcut(.cancelAction)
            Button(isNew ? "Add" : "Save") { save() }
                .keyboardShortcut(.defaultAction)
                .disabled(validationError != nil)
                .help(validationError ?? "")
        }
        .controlSize(.large)
        .padding(.horizontal, 20)
        .padding(.bottom, 20)
    }

    @ViewBuilder
    private var testStatus: some View {
        let error = saveError.map(TestState.failed) ?? test
        switch error {
        case .idle:
            EmptyView()
        case .running:
            ProgressView().controlSize(.small)
            Text("Connecting…").foregroundStyle(.secondary)
        case .succeeded:
            Label("Connected", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
        case .failed(let message):
            Label(message, systemImage: "xmark.octagon.fill")
                .foregroundStyle(.red)
                .lineLimit(2)
                .help(message)
                .textSelection(.enabled)
        }
    }

    // MARK: Helpers

    private var existingGroups: [String] {
        var seen = Set<String>()
        return model.connections.map(\.group).filter { !$0.isEmpty && seen.insert($0).inserted }
    }

    private var passwordPrompt: String {
        hasSavedPassword && !passwordEdited ? "Saved in Keychain" : "None"
    }

    private var userBinding: Binding<String> {
        Binding(get: { draft.user ?? "" }, set: { draft.user = $0.isEmpty ? nil : $0 })
    }

    /// The password to connect with: what was typed, or the saved one if untouched.
    private var effectivePassword: String? {
        if passwordEdited { return password.isEmpty ? nil : password }
        return hasSavedPassword ? model.savedPassword(draft.id) : nil
    }

    private func apply(url: String) {
        let trimmed = url.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { urlError = nil; return }
        do {
            let parsed = try ConnectionConfig.parse(url: trimmed)
            urlError = nil
            draft.kind = parsed.kind
            draft.host = parsed.host
            draft.port = parsed.port
            draft.database = parsed.database
            draft.user = parsed.user
            draft.sslMode = parsed.sslMode
            if draft.name.isEmpty || draft.name == lastAutoName { draft.name = parsed.name }
            lastAutoName = parsed.name
            if let pw = parsed.password { password = pw }
        } catch {
            urlError = error.localizedDescription
        }
    }
    @State private var lastAutoName = ""

    private func runTest() async {
        test = .running
        var config = draft
        config.password = effectivePassword
        let error = await model.test(config)
        test = error.map(TestState.failed) ?? .succeeded
    }

    private func save() {
        do {
            let saved = try model.save(draft, password: passwordEdited ? password : nil)
            if isNew { model.selectedConnectionID = saved.id }
            dismiss()
        } catch {
            saveError = error.localizedDescription
        }
    }
}
