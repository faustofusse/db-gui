import DBKit
import Foundation
import Observation

enum LoadState<Value> {
    case idle
    case loading
    case loaded(Value)
    case failed(String)

    var value: Value? { if case .loaded(let v) = self { v } else { nil } }
    var isLoading: Bool { if case .loading = self { true } else { false } }
}

// MARK: - Tabs

@Observable
@MainActor
final class TableTab: Identifiable {
    let id = UUID()
    let connection: ConnectionConfig
    let table: TableInfo
    /// Preview tabs get replaced by the next table you click (shown in italics).
    var isPreview: Bool
    var data: LoadState<QueryResult> = .idle
    var search = ""

    init(connection: ConnectionConfig, table: TableInfo, isPreview: Bool) {
        self.connection = connection
        self.table = table
        self.isPreview = isPreview
    }
}

@Observable
@MainActor
final class ScriptTab: Identifiable {
    let id = UUID()
    let connection: ConnectionConfig
    var title: String
    var text: String
    var result: LoadState<QueryResult> = .idle
    var lastDuration: Duration?
    /// The last run was stopped by the user.
    var wasCancelled = false
    /// Editor pane height once the user drags the divider; `nil` = half the available height.
    var editorHeight: CGFloat?
    /// New scripts focus the editor the first time they're shown.
    var needsInitialFocus = true
    var search = ""

    init(connection: ConnectionConfig, title: String, text: String) {
        self.connection = connection
        self.title = title
        self.text = text
    }
}

enum WorkspaceTab: Identifiable {
    case table(TableTab)
    case script(ScriptTab)

    nonisolated var id: UUID {
        switch self {
        case .table(let t): t.id
        case .script(let s): s.id
        }
    }

    @MainActor var connection: ConnectionConfig {
        switch self {
        case .table(let t): t.connection
        case .script(let s): s.connection
        }
    }

    @MainActor var title: String {
        switch self {
        case .table(let t): t.table.name
        case .script(let s): s.title
        }
    }

    @MainActor var systemImage: String {
        switch self {
        case .table(let t): t.table.kind == .view ? "eye" : "tablecells"
        case .script: "chevron.left.forwardslash.chevron.right"
        }
    }

    @MainActor var isPreview: Bool {
        if case .table(let t) = self { t.isPreview } else { false }
    }
}

// MARK: - App model

/// Opens the connection editor sheet; `original == nil` means a new connection.
struct ConnectionEditorRequest: Identifiable {
    let id = UUID()
    let original: ConnectionConfig?
}

extension ConnectionConfig {
    /// Anything besides name/group changed, so an open connection is stale.
    func connectsDifferently(than other: ConnectionConfig) -> Bool {
        (kind, host, port, database, user, sslMode) != (other.kind, other.host, other.port, other.database, other.user, other.sslMode)
    }
}

@Observable
@MainActor
final class AppModel {
    /// Saved connections, in user order. Passwords are not included (they're in `secrets`).
    var connections: [ConnectionConfig] = []
    /// Set when the connections file couldn't be read (shown in the sidebar).
    var storeError: String?
    /// The open "New / Edit Connection" sheet.
    var editor: ConnectionEditorRequest?
    /// Connection waiting for delete confirmation.
    var pendingDeletion: ConnectionConfig?

    private let store: ConnectionStore?
    private let secrets: any SecretStore

    init(store: ConnectionStore? = nil, secrets: any SecretStore = KeychainSecretStore()) {
        self.secrets = secrets
        do {
            // DBGUI_CONNECTIONS_FILE points at another file (handy for testing).
            let override = ProcessInfo.processInfo.environment["DBGUI_CONNECTIONS_FILE"]
            self.store = try store ?? override.map(ConnectionStore.open(path:)) ?? ConnectionStore.openDefault()
            connections = self.store?.connections() ?? []
        } catch {
            self.store = nil
            storeError = error.localizedDescription
        }
    }
    var selectedConnectionID: ConnectionConfig.ID? {
        // Show a spinner, not the previous connection's tables, until the new ones load.
        didSet { if selectedConnectionID != oldValue { schemas = .loading } }
    }

    var schemas: LoadState<[Schema]> = .idle
    /// Connections whose last attempt failed (shows a warning in the sidebar).
    var failedConnections: Set<ConnectionConfig.ID> = []
    /// Connections with an open server connection (green dot in the sidebar).
    var openConnections: Set<ConnectionConfig.ID> = []

    var tabs: [WorkspaceTab] = []
    var activeTabID: UUID?

    var pageSize = 200
    /// Incremented by ⌘F to focus the toolbar search field.
    var searchFocusRequest = 0

    private var drivers: [ConnectionConfig.ID: any DatabaseDriver] = [:]
    private var scriptCounter = 0

    // MARK: Derived

    var groupedConnections: [(group: String, connections: [ConnectionConfig])] {
        var order: [String] = []
        var byGroup: [String: [ConnectionConfig]] = [:]
        for c in connections {
            if byGroup[c.group] == nil { order.append(c.group) }
            byGroup[c.group, default: []].append(c)
        }
        return order.map { ($0, byGroup[$0]!) }
    }

    var selectedConnection: ConnectionConfig? {
        connections.first { $0.id == selectedConnectionID }
    }

    var activeTab: WorkspaceTab? {
        tabs.first { $0.id == activeTabID }
    }

    /// Highlighted row in the tables column: the active tab's table, if it belongs to the shown connection.
    var selectedTableID: TableInfo.ID? {
        guard case .table(let t) = activeTab, t.connection.id == selectedConnectionID else { return nil }
        return t.table.id
    }

    /// Search text of the active tab (each tab keeps its own).
    var activeSearch: String {
        get {
            switch activeTab {
            case .table(let t): t.search
            case .script(let s): s.search
            case nil: ""
            }
        }
        set {
            switch activeTab {
            case .table(let t): t.search = newValue
            case .script(let s): s.search = newValue
            case nil: break
            }
        }
    }

    func table(withID id: TableInfo.ID) -> TableInfo? {
        schemas.value?.lazy.flatMap(\.tables).first { $0.id == id }
    }

    /// The driver for a connection, created on first use with the latest saved settings
    /// and the password from the Keychain (read only now, so browsing never prompts).
    private func driver(for config: ConnectionConfig) -> any DatabaseDriver {
        if let d = drivers[config.id] { return d }
        var current = connections.first { $0.id == config.id } ?? config
        if current.password == nil { current.password = secrets.password(for: config.id) }
        let d = Drivers.make(for: current)
        drivers[config.id] = d
        return d
    }

    // MARK: Saved connections

    func newConnection() {
        editor = ConnectionEditorRequest(original: nil)
    }

    func edit(_ config: ConnectionConfig) {
        editor = ConnectionEditorRequest(original: config)
    }

    func hasSavedPassword(_ id: ConnectionConfig.ID) -> Bool {
        !id.isEmpty && secrets.hasPassword(for: id)
    }

    func savedPassword(_ id: ConnectionConfig.ID) -> String? {
        id.isEmpty ? nil : secrets.password(for: id)
    }

    /// Saves a new or edited connection. `password`: nil keeps the saved one, "" removes it.
    /// Changing how to connect drops the live connection and closes its tabs.
    @discardableResult
    func save(_ config: ConnectionConfig, password: String?) throws -> ConnectionConfig {
        guard let store else { throw DatabaseError.storage(storeError ?? "no connections file") }
        let previous = connections.first { $0.id == config.id }
        let saved = try store.upsert(config).refreshed
        if let password {
            if password.isEmpty { secrets.deletePassword(for: saved.id) } else { try secrets.setPassword(password, for: saved.id) }
        }
        if let previous, previous.connectsDifferently(than: saved) || password != nil {
            resetConnection(saved.id)
        }
        connections = store.connections()
        failedConnections.remove(saved.id)
        return saved
    }

    /// Saves a copy (with the same password) and selects it.
    func duplicate(_ config: ConnectionConfig) {
        let fresh = ConnectionConfig(
            id: "", name: "\(config.name) copy", group: config.group, kind: config.kind, host: config.host,
            port: config.port, database: config.database, user: config.user, sslMode: config.sslMode)
        do {
            let saved = try save(fresh, password: savedPassword(config.id))
            selectedConnectionID = saved.id
        } catch {
            storeError = error.localizedDescription
        }
    }

    func delete(_ config: ConnectionConfig) {
        guard let store else { return }
        do {
            try store.remove(id: config.id)
        } catch {
            storeError = error.localizedDescription
            return
        }
        resetConnection(config.id)
        secrets.deletePassword(for: config.id)
        connections = store.connections()
        failedConnections.remove(config.id)
        if selectedConnectionID == config.id {
            selectedConnectionID = nil
            schemas = .idle
        }
    }

    /// Tries a config from the editor without saving it. Returns an error message or nil.
    func test(_ config: ConnectionConfig) async -> String? {
        let driver = Drivers.make(for: config)
        defer { Task { await driver.disconnect() } }
        do {
            try await driver.connect()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    /// Disconnects and forgets the driver (so the next use picks up new settings) and closes its tabs.
    private func resetConnection(_ id: ConnectionConfig.ID) {
        if let driver = drivers.removeValue(forKey: id) {
            Task { await driver.disconnect() }
        }
        openConnections.remove(id)
        tabs.filter { $0.connection.id == id }.forEach { close($0.id) }
        if id == selectedConnectionID { schemas = .loading; Task { await loadSchemas() } }
    }

    #if DEBUG
    /// Adds the core's sample connections (mock data + the dev database on :54329).
    func addSampleConnections() {
        for sample in Drivers.sampleConnections() where !connections.contains(where: { $0.id == sample.id }) {
            do { try save(sample, password: sample.password) } catch { storeError = error.localizedDescription }
        }
    }
    #endif

    // MARK: Connection state

    func connect(_ config: ConnectionConfig) async {
        do {
            try await driver(for: config).connect()
            failedConnections.remove(config.id)
            if config.id == selectedConnectionID, schemas.value == nil { await loadSchemas() }
        } catch {
            failedConnections.insert(config.id)
        }
        await updateConnectionState(config.id)
    }

    /// Closes the server connection (stopping a running script) and puts the UI back the way it is
    /// at launch for that connection: its tabs close and, if selected, nothing is selected.
    func disconnect(_ config: ConnectionConfig) async {
        guard let driver = drivers[config.id] else { return }
        await driver.disconnect()
        tabs.filter { $0.connection.id == config.id }.forEach { close($0.id) }
        if config.id == selectedConnectionID {
            selectedConnectionID = nil
            schemas = .idle
        }
        await updateConnectionState(config.id)
    }

    func updateConnectionState(_ id: ConnectionConfig.ID) async {
        let open = await drivers[id]?.isConnected() ?? false
        if open { openConnections.insert(id) } else { openConnections.remove(id) }
    }

    /// Notices connections the server closed. Cheap: only asks drivers that exist, no network I/O.
    func monitorConnections() async {
        while !Task.isCancelled {
            for id in drivers.keys { await updateConnectionState(id) }
            try? await Task.sleep(for: .seconds(3))
        }
    }

    // MARK: Schemas

    func loadSchemas() async {
        guard let config = selectedConnection else { schemas = .idle; return }
        schemas = .loading
        do {
            let result = try await driver(for: config).listSchemas()
            guard config.id == selectedConnectionID else { return }
            failedConnections.remove(config.id)
            schemas = .loaded(result)
        } catch {
            guard config.id == selectedConnectionID else { return }
            failedConnections.insert(config.id)
            schemas = .failed(error.localizedDescription)
        }
        await updateConnectionState(config.id)
    }

    // MARK: Tabs

    func activate(_ id: UUID) {
        guard let tab = tabs.first(where: { $0.id == id }) else { return }
        activeTabID = id
        if tab.connection.id != selectedConnectionID {
            selectedConnectionID = tab.connection.id
        }
    }

    /// Opens a table from the selected connection. Reuses an existing tab for the same table,
    /// otherwise replaces the current preview tab (unless `pinned`).
    func openTable(_ table: TableInfo, pinned: Bool) {
        guard let connection = selectedConnection else { return }

        if let existing = tabs.first(where: {
            if case .table(let t) = $0 { t.connection.id == connection.id && t.table.id == table.id } else { false }
        }) {
            if pinned, case .table(let t) = existing { t.isPreview = false }
            activeTabID = existing.id
            return
        }

        let tab = TableTab(connection: connection, table: table, isPreview: !pinned)
        if !pinned, let previewIndex = tabs.firstIndex(where: \.isPreview) {
            tabs[previewIndex] = .table(tab)
        } else {
            insertAfterActive(.table(tab))
        }
        activeTabID = tab.id
        Task { await load(tab) }
    }

    /// ⌘1…⌘8 select by position, ⌘9 always selects the last tab (Safari behavior).
    func selectTab(number: Int) {
        guard !tabs.isEmpty else { return }
        let index = number == 9 ? tabs.count - 1 : number - 1
        guard tabs.indices.contains(index) else { return }
        activate(tabs[index].id)
    }

    /// Cycles through tabs; wraps around at the ends.
    func selectAdjacentTab(offset: Int) {
        guard !tabs.isEmpty else { return }
        let current = tabs.firstIndex { $0.id == activeTabID } ?? 0
        let next = ((current + offset) % tabs.count + tabs.count) % tabs.count
        activate(tabs[next].id)
    }

    func pin(_ id: UUID) {
        if case .table(let t) = tabs.first(where: { $0.id == id }) { t.isPreview = false }
    }

    func newScript() {
        guard let connection = selectedConnection else { return }
        scriptCounter += 1
        let example = schemas.value?.first?.tables.first.map { "select * from \($0.schema).\($0.name) limit 50;" } ?? ""
        let tab = ScriptTab(
            connection: connection,
            title: "Script \(scriptCounter)",
            text: "-- \(connection.name)\n\(example)\n"
        )
        insertAfterActive(.script(tab))
        activeTabID = tab.id
    }

    func close(_ id: UUID) {
        guard let index = tabs.firstIndex(where: { $0.id == id }) else { return }
        tabs.remove(at: index)
        guard activeTabID == id else { return }
        if tabs.isEmpty {
            activeTabID = nil
        } else {
            activate(tabs[min(index, tabs.count - 1)].id)
        }
    }

    func closeOthers(than id: UUID) {
        tabs.removeAll { $0.id != id }
        activate(id)
    }

    private func insertAfterActive(_ tab: WorkspaceTab) {
        if let active = tabs.firstIndex(where: { $0.id == activeTabID }) {
            tabs.insert(tab, at: active + 1)
        } else {
            tabs.append(tab)
        }
    }

    // MARK: Loading

    func refreshActiveTab() async {
        switch activeTab {
        case .table(let t): await load(t)
        case .script(let s): await run(s)
        case nil: break
        }
    }

    func load(_ tab: TableTab) async {
        tab.data = .loading
        do {
            tab.data = .loaded(try await driver(for: tab.connection).fetchRows(of: tab.table, limit: pageSize, offset: 0))
        } catch {
            tab.data = .failed(error.localizedDescription)
        }
        await updateConnectionState(tab.connection.id)
    }

    func run(_ tab: ScriptTab) async {
        guard !tab.result.isLoading else { return }
        tab.result = .loading
        tab.wasCancelled = false
        let clock = ContinuousClock()
        let start = clock.now
        do {
            let result = try await driver(for: tab.connection).execute(tab.text)
            tab.lastDuration = clock.now - start
            tab.result = .loaded(result)
        } catch DatabaseError.cancelled {
            tab.lastDuration = clock.now - start
            tab.result = .idle
            tab.wasCancelled = true
        } catch {
            tab.lastDuration = clock.now - start
            tab.result = .failed(error.localizedDescription)
        }
        await updateConnectionState(tab.connection.id)
    }

    /// Stops the script running in `tab` (server-side cancel).
    func cancel(_ tab: ScriptTab) async {
        guard tab.result.isLoading else { return }
        await driver(for: tab.connection).cancel()
    }
}
