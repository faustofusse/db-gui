import DBKit
import SwiftUI

struct TablesList: View {
    @Environment(AppModel.self) private var model
    @State private var tablesOnly = false
    @State private var collapsed: Set<String> = []
    @FocusState private var focused: Bool

    var body: some View {
        content
            .navigationTitle(model.selectedTarget.map { model.displayName(of: $0) } ?? "Tables")
            .navigationSubtitle(subtitle)
            .toolbar {
                ToolbarItemGroup {
                    Toggle(isOn: $tablesOnly) {
                        Label("Filter", systemImage: "line.3.horizontal.decrease")
                    }
                    .help(tablesOnly ? "Showing tables only" : "Filter: tables only")

                    Menu {
                        Button("Refresh") { Task { await model.loadSchemas() } }
                        Divider()
                        Button("Expand All") { collapsed.removeAll() }
                        Button("Collapse All") {
                            collapsed = Set(model.schemas.value?.map(\.name) ?? [])
                        }
                    } label: {
                        Label("More", systemImage: "ellipsis")
                    }
                    .menuIndicator(.hidden)
                }
            }
            .task(id: model.selectedTarget?.driverKey) {
                collapsed = []
                await model.loadSchemas()
            }
    }

    private var subtitle: String {
        guard let schemas = model.schemas.value else {
            return model.selectedTarget?.summary ?? ""
        }
        if tablesOnly { return "Filter by: Tables only" }
        let tables = schemas.reduce(0) { $0 + $1.tables.count }
        // MySQL databases are listed as the sections; SQLite's are attached databases.
        let section = model.selectedTarget?.kind == .postgres ? "schema" : "database"
        func count(_ n: Int, _ noun: String) -> String { "\(n) \(noun)\(n == 1 ? "" : "s")" }
        return "\(count(schemas.count, section)), \(count(tables, "table"))"
    }

    @ViewBuilder
    private var content: some View {
        if model.selectedConnection == nil {
            Color.clear
        } else {
            switch model.schemas {
            case .idle, .loading:
                ProgressView().controlSize(.small)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .failed(let message):
                ContentUnavailableView {
                    Label("Couldn’t Connect", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message)
                } actions: {
                    Button("Try Again") { Task { await model.loadSchemas() } }
                }
            case .loaded(let schemas):
                ScrollViewReader { proxy in
                List {
                    ForEach(schemas) { schema in
                        Section(isExpanded: expansion(for: schema.name)) {
                            ForEach(visibleTables(in: schema)) { table in
                                TableRow(table: table)
                                    .id(table.id)
                                    .mailSelection(table.id == model.selectedTableID) {
                                        model.openTable(table, pinned: false)
                                        focused = true
                                    }
                                    // Double-click keeps the tab open instead of previewing.
                                    .simultaneousGesture(TapGesture(count: 2).onEnded {
                                        model.openTable(table, pinned: true)
                                    })
                                    .contextMenu {
                                        Button("Open in New Tab") { model.openTable(table, pinned: true) }
                                    }
                            }
                        } header: {
                            Text(schema.name)
                        }
                    }
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)
                .arrowKeySelection(
                    ids: schemas.filter { !collapsed.contains($0.name) }.flatMap { visibleTables(in: $0).map(\.id) },
                    selected: model.selectedTableID,
                    focus: $focused
                ) { id in
                    if let table = model.table(withID: id) { model.openTable(table, pinned: false) }
                }
                .onChange(of: model.selectedTableID) { _, id in
                    if let id { proxy.scrollTo(id) }
                }
                }
            }
        }
    }

    private func visibleTables(in schema: Schema) -> [TableInfo] {
        schema.tables.filter { !tablesOnly || $0.kind == .table }
    }

    private func expansion(for schema: String) -> Binding<Bool> {
        Binding(
            get: { !collapsed.contains(schema) },
            set: { expanded in
                if expanded { collapsed.remove(schema) } else { collapsed.insert(schema) }
            }
        )
    }
}

private struct TableRow: View {
    let table: TableInfo

    var body: some View {
        Label {
            HStack {
                Text(table.name)
                    .lineLimit(1)
                Spacer()
                if let count = table.estimatedRowCount {
                    // Mail-style trailing count, stays gray when selected.
                    Text(count.formatted())
                        .font(.callout)
                        .monospacedDigit()
                        .foregroundStyle(Color.secondary)
                }
            }
        } icon: {
            Image(systemName: table.kind == .view ? "eye" : "tablecells")
        }
        .help("\(table.kind == .view ? "View" : "Table") \(table.id)")
    }
}
