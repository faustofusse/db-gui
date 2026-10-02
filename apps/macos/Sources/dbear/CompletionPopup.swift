import AppKit
import DBKit

/// Borderless, non-activating popup listing SQL completions. Entirely keyboard-driven
/// (⌃Space opens it, ↑↓ move, ⏎/⇥ accept, ⎋ dismisses) so the editor never loses focus.
@MainActor
final class CompletionPopup: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private let panel: NSPanel
    private let tableView = NSTableView()
    private(set) var items: [CompletionItem] = []
    /// Called when the user accepts the selected item (⏎/⇥/double-click).
    var onAccept: ((CompletionItem) -> Void)?

    var isVisible: Bool { panel.isVisible }

    var selectedItem: CompletionItem? {
        items.indices.contains(tableView.selectedRow) ? items[tableView.selectedRow] : nil
    }

    override init() {
        let column = NSTableColumn(identifier: .init("item"))
        column.width = 300
        tableView.addTableColumn(column)
        tableView.headerView = nil
        tableView.backgroundColor = .clear
        tableView.rowHeight = 20
        tableView.intercellSpacing = .zero
        tableView.style = .plain
        tableView.selectionHighlightStyle = .regular

        let scroll = NSScrollView()
        scroll.documentView = tableView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentInsets = NSEdgeInsets(top: 4, left: 0, bottom: 4, right: 0)

        panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 300, height: 160),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .popUpMenu
        panel.hidesOnDeactivate = true

        let container = NSVisualEffectView()
        container.material = .popover
        container.state = .active
        container.wantsLayer = true
        container.layer?.cornerRadius = 8
        container.layer?.masksToBounds = true
        container.addSubview(scroll)
        scroll.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: container.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: container.trailingAnchor),
        ])
        panel.contentView = container

        super.init()
        tableView.dataSource = self
        tableView.delegate = self
        tableView.doubleAction = #selector(acceptSelected)
        tableView.target = self
    }

    /// Shows (or repositions) the popup with `items`, anchored below `screenRect`
    /// (the caret's bounding rect, in screen coordinates — see `NSTextInputClient.firstRect`).
    func show(items: [CompletionItem], below screenRect: NSRect, in window: NSWindow) {
        guard !items.isEmpty else {
            hide()
            return
        }
        self.items = items
        tableView.reloadData()
        let rows = min(items.count, 8)
        let height = CGFloat(rows) * tableView.rowHeight + 8
        let width: CGFloat = 300
        let origin = NSPoint(x: screenRect.minX, y: screenRect.minY - height - 4)
        panel.setFrame(NSRect(origin: origin, size: NSSize(width: width, height: height)), display: true)
        if panel.parent == nil {
            window.addChildWindow(panel, ordered: .above)
        }
        panel.orderFront(nil)
        tableView.selectRowIndexes([0], byExtendingSelection: false)
    }

    func hide() {
        guard panel.isVisible else { return }
        panel.parent?.removeChildWindow(panel)
        panel.orderOut(nil)
        items = []
    }

    func moveSelection(by delta: Int) {
        guard !items.isEmpty else { return }
        let current = tableView.selectedRow
        let next = ((current == -1 ? 0 : current + delta) % items.count + items.count) % items.count
        tableView.selectRowIndexes([next], byExtendingSelection: false)
        tableView.scrollRowToVisible(next)
    }

    @objc private func acceptSelected() {
        if let item = selectedItem { onAccept?(item) }
    }

    func numberOfRows(in tableView: NSTableView) -> Int { items.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let view = CompletionRowView()
        view.configure(items[row])
        return view
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { true }
}

/// One row: an icon for the kind, the label, and a dimmed detail (type or schema) on the right.
private final class CompletionRowView: NSTableCellView {
    private let iconView = NSImageView()
    private let labelField = NSTextField(labelWithString: "")
    private let detailField = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        labelField.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        labelField.lineBreakMode = .byTruncatingTail
        labelField.setContentHuggingPriority(.defaultHigh, for: .horizontal)

        detailField.font = .systemFont(ofSize: 11)
        detailField.textColor = .secondaryLabelColor
        detailField.alignment = .right
        detailField.lineBreakMode = .byTruncatingTail

        iconView.symbolConfiguration = .init(pointSize: 11, weight: .regular)
        iconView.setContentHuggingPriority(.required, for: .horizontal)

        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)

        let stack = NSStackView(views: [iconView, labelField, spacer, detailField])
        stack.orientation = .horizontal
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 0, left: 8, bottom: 0, right: 8)
        addSubview(stack)
        stack.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            iconView.widthAnchor.constraint(equalToConstant: 14),
        ])
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func configure(_ item: CompletionItem) {
        labelField.stringValue = item.label
        detailField.stringValue = item.detail ?? ""
        iconView.image = NSImage(systemSymbolName: Self.symbolName(for: item.kind), accessibilityDescription: nil)
        iconView.contentTintColor = Self.tintColor(for: item.kind)
    }

    private static func symbolName(for kind: CompletionKind) -> String {
        switch kind {
        case .keyword: "textformat"
        case .schema: "square.stack.3d.up"
        case .table: "tablecells"
        case .view: "eye"
        case .column: "circle.grid.2x2"
        case .function: "function"
        }
    }

    private static func tintColor(for kind: CompletionKind) -> NSColor {
        switch kind {
        case .keyword: .systemPurple
        case .schema: .systemGray
        case .table: .systemBlue
        case .view: .systemTeal
        case .column: .systemOrange
        case .function: .systemGreen
        }
    }
}
