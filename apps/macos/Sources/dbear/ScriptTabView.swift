import DBKit
import SwiftUI

/// SQL editor on top, results below, resizable.
struct ScriptTabView: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: ScriptTab

    var body: some View {
        // Split position lives on the tab, so it survives switching tabs.
        VerticalSplit(topHeight: $tab.editorHeight, minTop: 120, minBottom: 150) {
            VStack(spacing: 0) {
                editorBar
                SQLEditor(text: $tab.text, focusOnAppear: consumeInitialFocus())
            }
        } bottom: {
            results
        }
    }

    /// New scripts start focused with the caret at the end (only read when the editor is created).
    private func consumeInitialFocus() -> Bool {
        guard tab.needsInitialFocus else { return false }
        // Mutating observed state during body evaluation would re-render; defer it.
        DispatchQueue.main.async { tab.needsInitialFocus = false }
        return true
    }

    private var editorBar: some View {
        HStack(spacing: 8) {
            Label(tab.connection.name, systemImage: tab.connection.kind.symbolName)
                .font(.callout)
                .foregroundStyle(.secondary)
                .help(tab.connection.summary)
            Spacer()
            if tab.result.isLoading {
                ProgressView().controlSize(.small)
                Button {
                    Task { await model.cancel(tab) }
                } label: {
                    Label("Stop", systemImage: "stop.fill")
                }
                .keyboardShortcut(".", modifiers: .command)
                .help("Stop Script (⌘.)")
            } else {
                Button {
                    Task { await model.run(tab) }
                } label: {
                    Label("Run", systemImage: "play.fill")
                }
                .keyboardShortcut(.return, modifiers: .command)
                .help("Run Script (⌘↩)")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }

    @ViewBuilder
    private var results: some View {
        switch tab.result {
        case .idle:
            Text(tab.wasCancelled ? "Query cancelled" : "Press ⌘↩ to run")
                .foregroundStyle(.tertiary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .loading:
            ProgressView().controlSize(.small)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .failed(let message):
            ContentUnavailableView {
                Label("Query Failed", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
            }
        case .loaded(let result) where result.columns.isEmpty:
            // INSERT/UPDATE/DDL: nothing to show in a grid.
            VStack(spacing: 6) {
                Text(result.rowsAffected.map { "\($0.formatted()) \($0 == 1 ? "row" : "rows") affected" } ?? "Done")
                    .font(.title3)
                if let duration = tab.lastDuration {
                    Text(duration.formatted(.units(allowed: [.seconds, .milliseconds], width: .narrow)))
                        .foregroundStyle(.secondary)
                }
            }
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .loaded(let result):
            DataGrid(result: result, search: tab.search, version: tab.runCount, duration: tab.lastDuration)
        }
    }
}
