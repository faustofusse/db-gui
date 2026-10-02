import DBKit
import SwiftUI

private struct IndexedColumn: Identifiable {
    let index: Int
    let info: ColumnInfo
    var id: Int { index }
}

/// Read-only result grid used by table tabs and script results.
struct DataGrid: View {
    let result: QueryResult
    let search: String
    var duration: Duration? = nil

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
                }
                .width(min: 60, ideal: idealWidth(for: column.info))
            }
        }
        .tableStyle(.inset(alternatesRowBackgrounds: true))
        .font(.system(.body, design: .monospaced))
        .safeAreaInset(edge: .bottom, spacing: 0) {
            StatusBar(shown: rows.count, total: result.totalCount ?? result.rows.count,
                      columns: result.columns.count, duration: duration)
        }
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
    let total: Int
    let columns: Int
    let duration: Duration?

    var body: some View {
        HStack {
            Text("\(shown.formatted()) of \(total.formatted()) rows")
            Spacer()
            if let duration {
                Text(duration.formatted(.units(allowed: [.seconds, .milliseconds], width: .narrow)))
                Text("·")
            }
            Text("\(columns) columns")
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }
}
