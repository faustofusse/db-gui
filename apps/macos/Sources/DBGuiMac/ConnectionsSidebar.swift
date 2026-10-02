import DBKit
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
                            isOpen: model.openConnections.contains(connection.id),
                            failed: model.failedConnections.contains(connection.id)
                        )
                        .mailSelection(connection.id == model.selectedConnectionID) {
                            model.selectedConnectionID = connection.id
                            focused = true
                        }
                        .contextMenu { menu(for: connection) }
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

    @ViewBuilder
    private func menu(for connection: ConnectionConfig) -> some View {
        if model.openConnections.contains(connection.id) {
            Button("Disconnect") { Task { await model.disconnect(connection) } }
        } else {
            Button("Connect") { Task { await model.connect(connection) } }
        }
        Divider()
        Button("New SQL Script") {
            model.selectedConnectionID = connection.id
            model.newScript()
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
    let isOpen: Bool
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
                } else if isOpen {
                    Circle()
                        .fill(.green)
                        .frame(width: 7, height: 7)
                        .padding(.trailing, 4)
                        .help("Connected")
                }
            }
        } icon: {
            Image(systemName: connection.kind.symbolName)
        }
        .help(connection.summary)
        .accessibilityValue(failed ? "Connection failed" : isOpen ? "Connected" : "Not connected")
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
