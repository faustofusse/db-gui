import AppKit
import DBKit
import SwiftUI

/// Infinite-scroll hooks for grids backed by a paged table.
struct GridPaging {
    /// More rows exist on the server.
    var hasMore: Bool
    var isLoading: Bool
    var error: String?
    var loadMore: () -> Void
    var retry: () -> Void
}

/// Server-side sorting for grids backed by a table: header clicks call `toggle` with the column name.
struct GridSorting {
    var keys: [SortKey]
    var toggle: (String) -> Void
}

/// Read-only result grid used by table tabs and script results.
///
/// Backed by a plain `NSTableView` rather than SwiftUI's `Table`: cells are reused text fields,
/// so scrolling stays smooth with thousands of rows, and appending a page only tells the table
/// the row count grew instead of diffing every row.
struct DataGrid: View {
    let result: QueryResult
    let search: String
    /// Changes when the data is replaced (reload / re-run), not when pages are appended.
    var version: Int = 0
    var duration: Duration? = nil
    var paging: GridPaging? = nil
    var sorting: GridSorting? = nil
    /// New rows are loading (re-sort, filter, refresh) while these stay on screen.
    var isReloading = false
    /// Shown at the start of the status bar (e.g. the Data | Structure switch).
    var statusLeading: AnyView? = nil

    var body: some View {
        let query = search.trimmingCharacters(in: .whitespaces)
        GridTable(result: result, search: query, version: version, paging: paging, sorting: sorting)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                StatusBar(
                    shown: query.isEmpty ? result.rows.count : matchCount(query),
                    loaded: result.rows.count, total: result.totalCount,
                    truncated: result.truncated, isFiltered: !query.isEmpty,
                    columns: result.columns.count, duration: duration, paging: paging,
                    isReloading: isReloading, leading: statusLeading
                )
            }
    }

    private func matchCount(_ query: String) -> Int {
        result.rows.reduce(0) { $0 + (GridData.matches($1, query) ? 1 : 0) }
    }
}

// MARK: - NSTableView bridge

private struct GridTable: NSViewRepresentable {
    let result: QueryResult
    let search: String
    let version: Int
    let paging: GridPaging?
    let sorting: GridSorting?

    func makeCoordinator() -> GridData { GridData() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = NSTableView()
        table.style = .inset
        table.usesAlternatingRowBackgroundColors = true
        table.rowHeight = 24
        table.intercellSpacing = NSSize(width: 12, height: 0)
        table.usesAutomaticRowHeights = false
        table.allowsMultipleSelection = true
        table.allowsColumnReordering = true
        table.allowsColumnResizing = true
        // Header clicks sort instead of selecting the column.
        table.allowsColumnSelection = false
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.headerView = NSTableHeaderView()
        table.dataSource = context.coordinator
        table.delegate = context.coordinator

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.useThinScrollers()
        scroll.drawsBackground = false
        scroll.contentView.postsBoundsChangedNotifications = true

        context.coordinator.attach(table: table, scrollView: scroll)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.update(result: result, search: search, version: version, paging: paging, sorting: sorting)
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: GridData) {
        coordinator.detach()
    }
}

/// Data source + delegate. Keeps the rows the table shows (filtered by the search text).
@MainActor
final class GridData: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private weak var table: NSTableView?
    private var columns: [ColumnInfo] = []
    private var allRows: [Row] = []
    private var rows: [Row] = []
    private var search = ""
    private var version = Int.min
    private var paging: GridPaging?
    private var sorting: GridSorting?
    private var observer: NSObjectProtocol?
    private var loadRequested = false

    /// Start fetching the next page this many rows before the end: about a page ahead,
    /// so fast scrolling doesn't run into the end and wait.
    private let prefetchDistance = 500
    private static let cellID = NSUserInterfaceItemIdentifier("cell")
    private static let rowID = NSUserInterfaceItemIdentifier("row")

    func attach(table: NSTableView, scrollView: NSScrollView) {
        self.table = table
        observer = NotificationCenter.default.addObserver(
            forName: NSView.boundsDidChangeNotification, object: scrollView.contentView, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.prefetchIfNeeded() }
        }
    }

    func detach() {
        if let observer { NotificationCenter.default.removeObserver(observer) }
    }

    func update(result: QueryResult, search: String, version: Int, paging: GridPaging?, sorting: GridSorting?) {
        self.paging = paging
        self.sorting = sorting
        defer { if let table { showSortIndicators(table) } }
        if !(paging?.isLoading ?? false) { loadRequested = false }
        guard let table else { return }

        if version != self.version || result.columns != columns {
            self.version = version
            self.search = search
            allRows = result.rows
            if result.columns != columns {
                columns = result.columns
                rebuildColumns(table)
            }
            applyFilter()
            table.reloadData()
            table.scrollRowToVisible(0)
        } else if search != self.search {
            self.search = search
            allRows = result.rows
            applyFilter()
            table.reloadData()
            table.scrollRowToVisible(0)
        } else if result.rows.count != allRows.count {
            let appended = result.rows.count > allRows.count && search.isEmpty
            allRows = result.rows
            applyFilter()
            // A new page only extends the table: no reload, no diff, scroll position untouched.
            if appended { table.noteNumberOfRowsChanged() } else { table.reloadData() }
        }
        // Tall windows can show the whole first page; ask for more on the next run loop turn
        // (not during SwiftUI's view update).
        DispatchQueue.main.async { [weak self] in self?.prefetchIfNeeded() }
    }

    private func applyFilter() {
        rows = search.isEmpty ? allRows : allRows.filter { Self.matches($0, search) }
    }

    nonisolated static func matches(_ row: Row, _ query: String) -> Bool {
        row.values.contains { $0.displayString.localizedCaseInsensitiveContains(query) }
    }

    /// Loads the next page once the last visible row is near the end. Not while searching:
    /// filtered rows keep the end in view and would pull in the whole table.
    private func prefetchIfNeeded() {
        guard let table, let paging, paging.hasMore, !paging.isLoading, paging.error == nil,
              !loadRequested, search.isEmpty
        else { return }
        let visible = table.rows(in: table.visibleRect)
        guard NSMaxRange(visible) >= rows.count - prefetchDistance else { return }
        loadRequested = true
        paging.loadMore()
    }

    /// Native ▲/▼ in the header of the sorted column (only the primary key of the sort).
    private func showSortIndicators(_ table: NSTableView) {
        let primary = sorting?.keys.first
        var highlighted: NSTableColumn?
        for column in table.tableColumns {
            guard let index = Int(column.identifier.rawValue), columns.indices.contains(index) else { continue }
            let sorted = primary?.column == columns[index].name
            let image = sorted ? NSImage(named: primary!.descending ? "NSDescendingSortIndicator" : "NSAscendingSortIndicator") : nil
            if table.indicatorImage(in: column) !== image { table.setIndicatorImage(image, in: column) }
            if sorted { highlighted = column }
        }
        if table.highlightedTableColumn !== highlighted { table.highlightedTableColumn = highlighted }
    }

    func tableView(_ tableView: NSTableView, didClick tableColumn: NSTableColumn) {
        guard let sorting, let index = Int(tableColumn.identifier.rawValue), columns.indices.contains(index) else { return }
        sorting.toggle(columns[index].name)
    }

    private func rebuildColumns(_ table: NSTableView) {
        table.tableColumns.forEach(table.removeTableColumn)
        for (index, info) in columns.enumerated() {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(String(index)))
            column.title = info.name
            let type = info.typeName.isEmpty ? info.name : "\(info.name) · \(info.typeName)"
            column.headerToolTip = sorting == nil ? type : "\(type)\nClick to sort"
            column.minWidth = 40
            column.width = Self.idealWidth(for: info)
            column.headerCell.alignment = info.isNumeric ? .right : .left
            table.addTableColumn(column)
        }
    }

    private static func idealWidth(for column: ColumnInfo) -> CGFloat {
        let type = column.typeName.lowercased()
        return switch type {
        case "boolean", "bool", "smallint", "integer", "int4", "int2": 80
        case "bigint", "int8": 100
        case "uuid": 300
        case let t where t.contains("time"): 220
        default: 160
        }
    }

    // MARK: NSTableViewDataSource / Delegate

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        tableView.makeView(withIdentifier: Self.rowID, owner: nil) as? GridRowView ?? {
            let r = GridRowView()
            r.identifier = Self.rowID
            return r
        }()
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let tableColumn, let index = Int(tableColumn.identifier.rawValue),
              rows.indices.contains(row), columns.indices.contains(index)
        else { return nil }
        let cell = tableView.makeView(withIdentifier: Self.cellID, owner: nil) as? GridCell ?? {
            let c = GridCell()
            c.identifier = Self.cellID
            return c
        }()
        let values = rows[row].values
        cell.show(index < values.count ? values[index] : .null)
        return cell
    }
}

// MARK: - Cells

/// Row container that flattens its cells into a single layer: one texture per row instead of
/// one per cell, which is what keeps fast scrolling cheap.
private final class GridRowView: NSTableRowView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        canDrawSubviewsIntoLayer = true
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// A cell that draws its text directly: no text field, no Auto Layout, no extra layer.
private final class GridCell: NSTableCellView {
    private var text = ""
    private var style = Style.text

    enum Style { case text, number, null, dimmed }

    private static let font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    private static let nullFont = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask)
    /// Long values (JSON, text blobs) are cut before reaching text layout.
    private static let maxChars = 512
    private static let left: NSParagraphStyle = paragraph(.left)
    private static let right: NSParagraphStyle = paragraph(.right)

    private static func paragraph(_ alignment: NSTextAlignment) -> NSParagraphStyle {
        let p = NSMutableParagraphStyle()
        p.alignment = alignment
        p.lineBreakMode = .byTruncatingTail
        return p
    }

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { if backgroundStyle != oldValue { needsDisplay = true } }
    }

    func show(_ value: DBValue) {
        let (text, style): (String, Style) = switch value {
        case .null: ("NULL", .null)
        case .int, .double, .decimal: (value.displayString, .number)
        case .bool(let b): (value.displayString, b ? .text : .dimmed)
        case .text(let s): (Self.singleLine(s), .text)
        }
        guard text != self.text || style != self.style else { return }
        self.text = text
        self.style = style
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        let selected = backgroundStyle == .emphasized
        let color: NSColor = switch style {
        case _ where selected: .alternateSelectedControlTextColor
        case .null: .tertiaryLabelColor
        case .dimmed: .secondaryLabelColor
        case .text, .number: .labelColor
        }
        let font = style == .null ? Self.nullFont : Self.font
        let lineHeight = ceil(font.ascender - font.descender + font.leading)
        let rect = NSRect(x: 0, y: ((bounds.height - lineHeight) / 2).rounded(), width: bounds.width, height: lineHeight)
        (text as NSString).draw(
            with: rect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
            attributes: [
                .font: font, .foregroundColor: color,
                .paragraphStyle: style == .number ? Self.right : Self.left,
            ]
        )
    }

    private static func singleLine(_ s: String) -> String {
        let clipped = s.count > maxChars ? String(s.prefix(maxChars)) + "…" : s
        guard clipped.contains(where: \.isNewline) else { return clipped }
        return clipped.replacingOccurrences(of: "\r\n", with: " ↵ ").replacingOccurrences(of: "\n", with: " ↵ ")
    }
}

extension ColumnInfo {
    var isNumeric: Bool {
        let t = typeName.lowercased()
        return ["int", "numeric", "decimal", "real", "double", "float", "serial", "money", "oid"].contains { t.contains($0) }
            && !t.hasSuffix("[]")
    }
}

// MARK: - Status bar

private struct StatusBar: View {
    let shown: Int
    let loaded: Int
    let total: Int?
    let truncated: Bool
    let isFiltered: Bool
    let columns: Int
    let duration: Duration?
    let paging: GridPaging?
    var isReloading = false
    var leading: AnyView?

    var body: some View {
        BottomBar {
            if let leading {
                leading
            }
            Text(rowsText)
            if truncated {
                Image(systemName: "info.circle")
                    .help("Scripts keep the first \(loaded.formatted()) rows. Add a LIMIT or open the table to page through everything.")
            }
            if isReloading {
                ProgressView().controlSize(.mini)
            } else if let paging {
                if paging.isLoading {
                    ProgressView().controlSize(.mini)
                    Text("Loading more…")
                } else if let error = paging.error {
                    Text("Couldn’t load more rows").foregroundStyle(.red).help(error)
                    Button("Retry", action: paging.retry).buttonStyle(.link)
                }
            }
            Spacer()
            if let duration {
                Text(duration.formatted(.units(allowed: [.seconds, .milliseconds], width: .narrow)))
                Text("·")
            }
            Text("\(columns) columns")
        }
    }

    private var rowsText: String {
        if isFiltered {
            return "\(shown.formatted()) matching of \(loaded.formatted()) loaded rows"
        }
        if truncated, let total {
            return "First \(loaded.formatted()) of \(total.formatted()) rows"
        }
        let hasMore = paging?.hasMore ?? false
        // Big tables report a planner estimate, which can be below what's already loaded.
        if let total, hasMore || total > loaded {
            return "\(loaded.formatted()) of \(max(total, loaded).formatted()) rows"
        }
        return hasMore ? "\(loaded.formatted())+ rows" : "\(loaded.formatted()) rows"
    }
}

/// The strip under a grid or the structure view: secondary text on the bar material.
struct BottomBar<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        HStack(spacing: 6) { content }
            .font(.callout)
            .foregroundStyle(.secondary)
            .monospacedDigit()
            .padding(.horizontal, 12)
            .frame(height: 30)
            .background(.bar)
            .overlay(alignment: .top) { Divider() }
    }
}
