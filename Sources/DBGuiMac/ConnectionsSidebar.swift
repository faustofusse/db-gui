import DBCore
import SwiftUI

struct ConnectionsSidebar: View {
    @Environment(AppModel.self) private var model
    @State private var collapsed: Set<String> = []
    @FocusState private var focused: Bool

    var body: some View {
        List {
            ForEach(model.groupedConnections, id: \.group) { group in
                Section(isExpanded: expansion(for: group.group)) {
                    ForEach(group.connections) { connection in
                        ConnectionRow(
                            connection: connection,
                            failed: model.failedConnections.contains(connection.id)
                        )
                        .mailSelection(connection.id == model.selectedConnectionID) {
                            model.selectedConnectionID = connection.id
                            focused = true
                        }
                    }
                } header: {
                    Text(group.group)
                }
            }
        }
        .listStyle(.sidebar)
        .arrowKeySelection(ids: visibleIDs, selected: model.selectedConnectionID, focus: $focused) {
            model.selectedConnectionID = $0
        }
    }

    private var visibleIDs: [ConnectionConfig.ID] {
        model.groupedConnections
            .filter { !collapsed.contains($0.group) }
            .flatMap { $0.connections.map(\.id) }
    }

    private func expansion(for group: String) -> Binding<Bool> {
        Binding(
            get: { !collapsed.contains(group) },
            set: { if $0 { collapsed.remove(group) } else { collapsed.insert(group) } }
        )
    }
}

private struct ConnectionRow: View {
    let connection: ConnectionConfig
    let failed: Bool

    var body: some View {
        Label {
            HStack {
                Text(connection.name)
                Spacer()
                if failed {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                        .help("Could not connect")
                }
            }
        } icon: {
            Image(systemName: connection.kind.symbolName)
        }
        .help(connection.summary)
    }
}

extension DatabaseKind {
    var symbolName: String {
        switch self {
        case .postgres: "cylinder.split.1x2"
        case .mysql: "cylinder"
        case .sqlite: "doc"
        }
    }
}
