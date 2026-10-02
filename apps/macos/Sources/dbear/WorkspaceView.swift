import DBKit
import SwiftUI

/// Right-hand pane: a tab strip with table and SQL script tabs.
struct WorkspaceView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        content
            .toolbar {
                ToolbarItem { newScriptButton }

                // Mail-like order: … [Search] [Refresh]
                if #available(macOS 26.0, *) {
                    ToolbarSpacer(.flexible)
                }
                ToolbarItem { searchField }
                if #available(macOS 26.0, *) {
                    ToolbarSpacer(.fixed)
                }
                ToolbarItem { RefreshButton() }
            }
    }

    private var searchField: some View {
        @Bindable var model = model
        return ToolbarSearchField(text: $model.activeSearch, focusRequest: model.searchFocusRequest)
            .frame(minWidth: 160, idealWidth: 280, maxWidth: 320)
    }

    private var newScriptButton: some View {
        Button {
            model.newScript()
        } label: {
            Label("New SQL Script", systemImage: "square.and.pencil")
        }
        .keyboardShortcut("t", modifiers: .command)
        .disabled(model.selectedConnection == nil)
        .help("New SQL Script (⌘T)")
    }

    @ViewBuilder
    private var content: some View {
        if model.tabs.isEmpty {
            EmptyPlaceholder(text: "No Table Selected")
        } else {
            VStack(spacing: 0) {
                TabStrip()
                switch model.activeTab {
                case .table(let tab): TableTabView(tab: tab).id(tab.id)
                case .script(let tab): ScriptTabView(tab: tab).id(tab.id)
                case nil: EmptyPlaceholder(text: "No Tab Selected")
                }
            }
            // Tab switches, opens and closes are instant: no implicit or inherited animations.
            .transaction { $0.disablesAnimations = true; $0.animation = nil }
        }
    }
}

// MARK: - Tab strip (Finder / Safari style)

private struct TabStrip: View {
    @Environment(AppModel.self) private var model
    @State private var hoveredID: UUID?

    var body: some View {
        HStack(spacing: 8) {
            HStack(spacing: 0) {
                ForEach(Array(model.tabs.enumerated()), id: \.element.id) { index, tab in
                    if index > 0 {
                        TabSeparator(hidden: isHighlighted(model.tabs[index - 1].id) || isHighlighted(tab.id))
                    }
                    TabItem(
                        tab: tab,
                        isActive: tab.id == model.activeTabID,
                        hovered: Binding(
                            get: { hoveredID == tab.id },
                            set: { hoveredID = $0 ? tab.id : (hoveredID == tab.id ? nil : hoveredID) }
                        )
                    )
                }
            }
            .padding(2)
            .frame(height: 30)
            .background(Capsule().fill(.primary.opacity(0.06)))

            NewTabButton()
        }
        .padding(.horizontal, 10)
        .padding(.top, 4)
        .padding(.bottom, 6)
    }

    private func isHighlighted(_ id: UUID) -> Bool {
        id == model.activeTabID || id == hoveredID
    }
}

private struct TabSeparator: View {
    let hidden: Bool

    var body: some View {
        Rectangle()
            .fill(.primary.opacity(0.12))
            .frame(width: 1, height: 14)
            .opacity(hidden ? 0 : 1)
    }
}

private struct TabItem: View {
    @Environment(AppModel.self) private var model
    let tab: WorkspaceTab
    let isActive: Bool
    @Binding var hovered: Bool

    var body: some View {
        ZStack {
            HStack(spacing: 5) {
                Image(systemName: tab.systemImage)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Text(tab.title)
                    .italic(tab.isPreview)
                    .fontWeight(isActive ? .semibold : .regular)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .padding(.horizontal, 26)

            HStack {
                CloseButton { model.close(tab.id) }
                    .opacity(hovered ? 1 : 0)
                Spacer(minLength: 0)
            }
            .padding(.leading, 5)
        }
        .font(.system(size: 13))
        .foregroundStyle(isActive ? .primary : .secondary)
        .frame(minWidth: 80, maxWidth: .infinity, maxHeight: .infinity)
        .background {
            if isActive {
                Capsule()
                    .fill(.primary.opacity(0.14))
                    .overlay(Capsule().strokeBorder(.primary.opacity(0.08), lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.15), radius: 1, y: 0.5)
            } else if hovered {
                Capsule().fill(.primary.opacity(0.05))
            }
        }
        .contentShape(Capsule())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(tab.title)
        .accessibilityAddTraits(isActive ? [.isButton, .isSelected] : .isButton)
        .accessibilityAction { model.activate(tab.id) }
        .onHover { hovered = $0 }
        .onTapGesture { model.activate(tab.id) }
        .simultaneousGesture(TapGesture(count: 2).onEnded { model.pin(tab.id) })
        .help("\(tab.connection.name) · \(tooltip)")
        .contextMenu {
            if tab.isPreview {
                Button("Keep Open") { model.pin(tab.id) }
            }
            Button("Close Tab") { model.close(tab.id) }
            Button("Close Other Tabs") { model.closeOthers(than: tab.id) }
                .disabled(model.tabs.count < 2)
        }
    }

    private var tooltip: String {
        switch tab {
        case .table(let t): t.table.id
        case .script(let s): s.title
        }
    }
}

private struct CloseButton: View {
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(.system(size: 8, weight: .bold))
                .foregroundStyle(.secondary)
                .frame(width: 18, height: 18)
                .background(Circle().fill(.primary.opacity(hovering ? 0.12 : 0)))
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("Close Tab")
    }
}

private struct NewTabButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if #available(macOS 26.0, *) {
                button.buttonStyle(.glass).buttonBorderShape(.circle)
            } else {
                button.buttonStyle(.borderless)
            }
        }
        .disabled(model.selectedConnection == nil)
        .help("New SQL Script")
    }

    private var button: some View {
        Button { model.newScript() } label: {
            Image(systemName: "plus")
                .font(.system(size: 13, weight: .medium))
                .frame(width: 18, height: 18)
        }
    }
}

// MARK: - Table tab

private struct TableTabView: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab

    var body: some View {
        switch tab.data {
        case .idle, .loading:
            ProgressView().controlSize(.small)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .failed(let message):
            ContentUnavailableView("Couldn’t Load Rows", systemImage: "exclamationmark.triangle",
                                   description: Text(message))
        case .loaded(let result):
            DataGrid(
                result: result, search: tab.search, version: tab.generation,
                paging: GridPaging(
                    hasMore: !tab.reachedEnd, isLoading: tab.isLoadingMore, error: tab.loadMoreError,
                    loadMore: { Task { await model.loadMore(tab) } },
                    retry: { model.retryLoadMore(tab) }
                )
            )
        }
    }
}
