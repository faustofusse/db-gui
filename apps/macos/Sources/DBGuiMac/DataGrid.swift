import DBKit
import SwiftUI

private struct IndexedColumn: Identifiable {
    let index: Int
    let info: ColumnInfo
    var id: Int { index }
}

/// Infinite-scroll hooks for grids backed by a paged table.
struct GridPaging {
    /// More rows exist on the server.
    var hasMore: Bool
    var isLoading: Bool
    var error: String?
    var loadMore: () -> Void
    var retry: () -> Void
}

/// Read-only result grid used by table tabs and script results.
struct DataGrid: View {
    let result: QueryResult
    let search: String
    var duration: Duration? = nil
    var paging: GridPaging? = nil

    /// Start fetching the next page this many rows before the end.
    private let prefetchDistance = 150

    private var columns: [IndexedColumn] {
        result.columns.enumerated().map { IndexedColumn(index: $0.offset, info: $0.element) }
    }

    private var filteredRows: [Row] {
        let q = search.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return result.rows }
        return result.rows.filter { row in
            row.values.contains { $0.displayString.localizedCaseInsensitiveContains(q) }
        }
    }

    var body: some View {
        let rows = filteredRows
        Table(rows) {
            TableColumnForEach(columns) { column in
                TableColumn(column.info.name) { (row: Row) in
                    CellView(value: row.values[column.index])
                        .onAppear { if column.index == 0 { rowAppeared(row) } }
                }
                .width(min: 60, ideal: idealWidth(for: column.info))
            }
        }
        .tableStyle(.inset(alternatesRowBackgrounds: true))
        .font(.system(.body, design: .monospaced))
        .safeAreaInset(edge: .bottom, spacing: 0) {
            StatusBar(
                shown: rows.count, loaded: result.rows.count, total: result.totalCount,
                truncated: result.truncated, isFiltered: !search.trimmingCharacters(in: .whitespaces).isEmpty,
                columns: result.columns.count, duration: duration, paging: paging
            )
        }
    }

    /// Rows are created lazily as they scroll into view; one near the end means "load the next page".
    /// Not while searching: filtered rows would keep the end visible and load the whole table.
    private func rowAppeared(_ row: Row) {
        guard let paging, paging.hasMore, !paging.isLoading, paging.error == nil,
              search.trimmingCharacters(in: .whitespaces).isEmpty,
              row.id >= result.rows.count - prefetchDistance
        else { return }
        paging.loadMore()
    }

    private func idealWidth(for column: ColumnInfo) -> CGFloat {
        switch column.typeName.lowercased() {
        case "boolean", "integer", "bigint": 80
        case "uuid": 300
        case let t where t.contains("time"): 180
        default: 160
        }
    }
}

private struct CellView: View {
    let value: DBValue

    var body: some View {
        switch value {
        case .null:
            Text("NULL").foregroundStyle(.tertiary).italic()
        case .int, .double, .decimal:
            Text(value.displayString).frame(maxWidth: .infinity, alignment: .trailing)
        case .bool(let b):
            Text(value.displayString).foregroundStyle(b ? .primary : .secondary)
        case .text(let s):
            Text(s).lineLimit(1).truncationMode(.tail)
        }
    }
}

private struct StatusBar: View {
    let shown: Int
    let loaded: Int
    let total: Int?
    let truncated: Bool
    let isFiltered: Bool
    let columns: Int
    let duration: Duration?
    let paging: GridPaging?

    var body: some View {
        HStack(spacing: 6) {
            Text(rowsText)
            if truncated {
                Image(systemName: "info.circle")
                    .help("Scripts keep the first \(loaded.formatted()) rows. Add a LIMIT or open the table to page through everything.")
            }
            if let paging {
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
        .font(.callout)
        .foregroundStyle(.secondary)
        .monospacedDigit()
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
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
