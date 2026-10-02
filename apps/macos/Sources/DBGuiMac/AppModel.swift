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

@Observable
@MainActor
final class AppModel {
    var connections: [ConnectionConfig] = Drivers.sampleConnections()
    var selectedConnectionID: ConnectionConfig.ID?

    var schemas: LoadState<[Schema]> = .idle
    /// Connections whose last attempt failed (shows a warning in the sidebar).
    var failedConnections: Set<ConnectionConfig.ID> = []

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

    private func driver(for config: ConnectionConfig) -> any DatabaseDriver {
        if let d = drivers[config.id] { return d }
        let d = Drivers.make(for: config)
        drivers[config.id] = d
        return d
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
    }

    func run(_ tab: ScriptTab) async {
        guard !tab.result.isLoading else { return }
        tab.result = .loading
        let clock = ContinuousClock()
        let start = clock.now
        do {
            let result = try await driver(for: tab.connection).execute(tab.text)
            tab.lastDuration = clock.now - start
            tab.result = .loaded(result)
        } catch {
            tab.lastDuration = clock.now - start
            tab.result = .failed(error.localizedDescription)
        }
    }
}
