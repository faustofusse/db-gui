import SwiftUI

struct ContentView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationSplitView {
            ConnectionsSidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 240, max: 320)
        } content: {
            TablesList()
                .navigationSplitViewColumnWidth(min: 240, ideal: 300, max: 420)
        } detail: {
            WorkspaceView()
        }
        .separatorColoredSplitDividers()
        .task { await model.monitorConnections() }
    }
}

/// The big faded placeholder Mail shows ("No Message Selected").
struct EmptyPlaceholder: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 26))
            .foregroundStyle(.tertiary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct RefreshButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Button {
            Task { await model.refreshActiveTab() }
        } label: {
            Label("Refresh", systemImage: "arrow.clockwise")
        }
        .disabled(model.activeTab == nil)
        .help("Reload")
    }
}
