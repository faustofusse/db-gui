import DBKit
import SwiftUI

/// A table tab: rows (with a WHERE filter bar and sortable headers) or the table's structure,
/// switched from the bottom bar like TablePlus.
struct TableTabView: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab

    var body: some View {
        switch tab.mode {
        case .data:
            rows.frame(maxWidth: .infinity, maxHeight: .infinity)
            .sheet(isPresented: Binding(get: { tab.isReviewingEdits }, set: { tab.isReviewingEdits = $0 })) {
                ReviewChangesSheet(tab: tab)
            }
        case .structure:
            StructureView(tab: tab)
        }
    }

    @ViewBuilder
    private var rows: some View {
        switch tab.data {
        case .idle, .loading:
            ProgressView().controlSize(.small)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .safeAreaInset(edge: .bottom, spacing: 0) { BottomBar { ModePicker(tab: tab); Spacer() } }
        case .failed(let message):
            ContentUnavailableView {
                Label(tab.appliedFilter == nil ? "Couldn’t Load Rows" : "Couldn’t Filter Rows",
                      systemImage: "exclamationmark.triangle")
            } description: {
                Text(message).textSelection(.enabled)
            } actions: {
                if tab.appliedFilter != nil {
                    Button("Clear Filter") { model.clearFilter(tab) }
                }
                Button("Try Again") { Task { await model.load(tab) } }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .safeAreaInset(edge: .bottom, spacing: 0) { BottomBar { ModePicker(tab: tab); Spacer() } }
        case .loaded(let result):
            DataGrid(
                // The search field is this tab's WHERE filter: rows are filtered by the server, not here.
                result: result, search: "", version: tab.dataVersion,
                paging: GridPaging(
                    hasMore: !tab.reachedEnd, isLoading: tab.isLoadingMore, error: tab.loadMoreError,
                    loadMore: { Task { await model.loadMore(tab) } },
                    retry: { model.retryLoadMore(tab) }
                ),
                sorting: GridSorting(keys: tab.sort) { model.toggleSort(tab, column: $0) },
                editing: tab.readOnlyReason == nil ? editing : nil,
                isReloading: tab.isReloading,
                statusLeading: AnyView(HStack(spacing: 6) {
                    ModePicker(tab: tab)
                    RowButtons(tab: tab)
                    FilterIndicator(tab: tab)
                })
            )
        }
    }

    private var editing: GridEditing {
        GridEditing(
            edits: tab.edits,
            editRequest: tab.editRequest,
            setCell: { model.setCell(tab, row: $0, column: $1, to: $2) },
            addRow: { model.addRow(tab) },
            deleteRows: { model.deleteRows(tab, ids: $0) },
            revertRows: { model.revertRows(tab, ids: $0) },
            selectionChanged: { tab.selectedRowIDs = $0 },
            requestHandled: { tab.editRequest = nil }
        )
    }
}

/// Funnel shown while the rows are filtered, with the condition in its tooltip; click to clear.
private struct FilterIndicator: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab

    var body: some View {
        if let filter = tab.appliedFilter {
            Button { model.clearFilter(tab) } label: {
                Image(systemName: "line.3.horizontal.decrease.circle.fill").foregroundStyle(.tint)
            }
            .buttonStyle(.borderless)
            .help("Filtered: WHERE \(filter)\nClick to clear")
            .padding(.trailing, 4)
        }
    }
}

// MARK: - Editing

/// `+` / `−` next to the mode switch, or a lock saying why the rows are read-only.
private struct RowButtons: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab

    var body: some View {
        if let reason = tab.readOnlyReason {
            Image(systemName: "lock")
                .foregroundStyle(.tertiary)
                .help("Read-only: \(reason)")
                .padding(.trailing, 4)
        } else {
            HStack(spacing: 2) {
                Button { model.addRow(tab) } label: { Image(systemName: "plus").frame(width: 18, height: 18) }
                    .help("Add Row")
                Button { model.deleteRows(tab, ids: tab.selectedRowIDs) } label: {
                    Image(systemName: "minus").frame(width: 18, height: 18)
                }
                .disabled(tab.selectedRowIDs.isEmpty)
                .help("Delete Selected Rows (⌫)")
            }
            .buttonStyle(.borderless)
            .padding(.trailing, 4)
        }
    }
}

/// Toolbar buttons shown while a tab has unsaved edits: Discard, what changed (click → review, ⇧⌘S),
/// and Save (⌘S, no review).
struct PendingChangesButtons: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab

    var body: some View {
        Button { model.discardEdits(tab) } label: {
            Label("Discard", systemImage: "arrow.uturn.backward")
        }
        .help("Discard unsaved changes")

        Button { model.reviewEdits(tab) } label: {
            Label {
                Text(tab.edits.summary).monospacedDigit()
            } icon: {
                Image(systemName: "pencil.circle.fill").foregroundStyle(.orange)
            }
            .labelStyle(.titleAndIcon)
        }
        .help("Not saved yet. Review the SQL before saving (⇧⌘S)")

        Button { model.saveEditsNow(tab) } label: {
            Label {
                Text("Save")
            } icon: {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
            }
            .labelStyle(.titleAndIcon)
        }
        .disabled(tab.isSaving)
        .help("Save in one transaction (⌘S)")
    }
}

/// The exact statements that will run, and Save. Errors keep the sheet (and the edits) open.
private struct ReviewChangesSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let tab: TableTab
    @State private var statements: [EditStatement] = []
    @State private var error: String?
    @State private var saving = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Save Changes to “\(tab.table.name)”?").font(.headline)
                Text("\(tab.edits.summary). These statements run in one transaction: if any fails, or a row changed since it was loaded, nothing is saved.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            ScrollView {
                DDLView(sql: statements.map(\.sql).joined(separator: "\n"), fontSize: 12)
            }
            .frame(minHeight: 80, maxHeight: 320)
            if let error {
                Label {
                    Text(error).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red)
                }
                .font(.callout)
            }
            HStack {
                if saving { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Save") { save() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(saving || statements.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 640)
        .task {
            // A failed direct save (⌘S) opens this sheet to show why.
            error = tab.saveError
            tab.saveError = nil
            do {
                statements = try model.previewEdits(tab)
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    private func save() {
        saving = true
        error = nil
        Task {
            do {
                try await model.saveEdits(tab)
                dismiss()
            } catch {
                self.error = error.localizedDescription
            }
            saving = false
        }
    }
}

// MARK: - Mode switch

private struct ModePicker: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab

    var body: some View {
        Picker("View", selection: Binding(get: { tab.mode }, set: { model.setMode($0, of: tab) })) {
            Text("Data").tag(TableTabMode.data)
            Text("Structure").tag(TableTabMode.structure)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .controlSize(.small)
        .fixedSize()
        .padding(.trailing, 6)
        .help("Show rows or the table’s columns, indexes and DDL (⌥⌘1 / ⌥⌘2)")
    }
}

// MARK: - Structure

private struct StructureView: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab

    var body: some View {
        Group {
            switch tab.structure {
            case .idle, .loading:
                ProgressView().controlSize(.small)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .failed(let message):
                ContentUnavailableView {
                    Label("Couldn’t Load Structure", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message).textSelection(.enabled)
                } actions: {
                    Button("Try Again") { Task { await model.loadStructure(tab) } }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .loaded(let structure):
                StructureContent(tab: tab, structure: structure, search: tab.search)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            BottomBar {
                ModePicker(tab: tab)
                if let s = tab.structure.value {
                    Text(summary(s))
                }
                Spacer()
            }
        }
    }

    private func summary(_ s: TableStructure) -> String {
        var parts = [count(s.columns.count, "column")]
        if !s.indexes.isEmpty { parts.append(count(s.indexes.count, "index", plural: "indexes")) }
        if !s.foreignKeys.isEmpty { parts.append(count(s.foreignKeys.count, "foreign key")) }
        return parts.joined(separator: " · ")
    }

    private func count(_ n: Int, _ singular: String, plural: String? = nil) -> String {
        "\(n.formatted()) \(n == 1 ? singular : plural ?? singular + "s")"
    }
}

private struct StructureContent: View {
    @Environment(AppModel.self) private var model
    let tab: TableTab
    let structure: TableStructure
    let search: String

    private var columns: [ColumnDetail] {
        let q = search.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return structure.columns }
        return structure.columns.filter { $0.name.localizedCaseInsensitiveContains(q) || $0.typeName.localizedCaseInsensitiveContains(q) }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 28) {
                section("Columns", count: structure.columns.count) { columnsGrid }
                if !structure.indexes.isEmpty {
                    section("Indexes", count: structure.indexes.count) { indexesGrid }
                }
                if !structure.foreignKeys.isEmpty {
                    section("Foreign Keys", count: structure.foreignKeys.count) { foreignKeysGrid }
                }
                if let ddl = structure.ddl {
                    section("Definition", count: nil) { DDLView(sql: ddl, fontSize: 12) }
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .textSelection(.enabled)
        }
        .scrollContentBackground(.hidden)
    }

    private func section<Content: View>(_ title: String, count: Int?, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(title).font(.headline)
                if let count { Text(count.formatted()).foregroundStyle(.secondary) }
            }
            content()
        }
    }

    // MARK: Columns

    private var hasComments: Bool { structure.columns.contains { $0.comment != nil } }

    private var columnsGrid: some View {
        StructureGrid {
            GridRow {
                HeaderCell("")
                HeaderCell("Name")
                HeaderCell("Type")
                HeaderCell("Nullable")
                HeaderCell("Default")
                if hasComments { HeaderCell("Comment") }
            }
        } rows: {
            DividedRows(items: columns) { column in
                GridRow {
                    Group {
                        if column.isPrimaryKey {
                            Image(systemName: "key.fill")
                                .foregroundStyle(.orange)
                                .help(primaryKeyHelp(column.name))
                        } else {
                            Color.clear
                        }
                    }
                    .font(.system(size: 10))
                    .frame(width: 12)
                    Text(column.name).fontWeight(column.isPrimaryKey ? .semibold : .regular)
                    Code(column.typeName).foregroundStyle(.secondary)
                    Group {
                        if column.isNullable {
                            Image(systemName: "checkmark").foregroundStyle(.secondary)
                        } else {
                            Text("NOT NULL").font(.system(size: 10, weight: .medium, design: .monospaced))
                                .foregroundStyle(.tertiary)
                        }
                    }
                    .gridColumnAlignment(.center)
                    Code(column.defaultValue ?? "", truncate: true)
                    if hasComments {
                        Text(column.comment ?? "").foregroundStyle(.secondary).lineLimit(2)
                            .frame(maxWidth: 360, alignment: .leading)
                    }
                }
            }
        }
    }

    private func primaryKeyHelp(_ name: String) -> String {
        guard structure.primaryKey.count > 1, let position = structure.primaryKey.firstIndex(of: name) else {
            return "Primary key"
        }
        return "Primary key (column \(position + 1) of \(structure.primaryKey.count))"
    }

    // MARK: Indexes

    private var indexesGrid: some View {
        StructureGrid {
            GridRow {
                HeaderCell("Name")
                HeaderCell("Columns")
                HeaderCell("Kind")
            }
        } rows: {
            DividedRows(items: structure.indexes) { index in
                GridRow {
                    Text(index.name).help(index.definition ?? index.name)
                    Code(index.columns.joined(separator: ", "), truncate: true)
                    Text(index.isPrimary ? "Primary key" : index.isUnique ? "Unique" : "Index")
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    // MARK: Foreign keys

    private var foreignKeysGrid: some View {
        StructureGrid {
            GridRow {
                HeaderCell("Columns")
                HeaderCell("References")
                HeaderCell("On Update")
                HeaderCell("On Delete")
                HeaderCell("Name")
            }
        } rows: {
            DividedRows(items: structure.foreignKeys) { fk in
                GridRow {
                    Code(fk.columns.joined(separator: ", "))
                    Button {
                        model.openReferencedTable(fk, from: tab)
                    } label: {
                        HStack(spacing: 4) {
                            Text(reference(fk))
                            Image(systemName: "arrow.right.circle.fill").font(.system(size: 11))
                        }
                    }
                    .buttonStyle(.link)
                    .help("Open \(fk.referencedSchema).\(fk.referencedTable)")
                    Text(fk.onUpdate).foregroundStyle(.secondary)
                    Text(fk.onDelete).foregroundStyle(fk.onDelete == "CASCADE" ? .orange : .secondary)
                    Text(fk.name).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func reference(_ fk: ForeignKeyInfo) -> String {
        let table = fk.referencedSchema == tab.table.schema ? fk.referencedTable : "\(fk.referencedSchema).\(fk.referencedTable)"
        return fk.referencedColumns.isEmpty ? table : "\(table) (\(fk.referencedColumns.joined(separator: ", ")))"
    }
}

/// Grid rows with a hairline between them.
private struct DividedRows<Item: Identifiable, Row: View>: View {
    let items: [Item]
    @ViewBuilder let row: (Item) -> Row

    var body: some View {
        ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
            if index > 0 { Divider().opacity(0.5) }
            row(item)
        }
    }
}

/// A light table built on `Grid`: columns size to their content, hairlines between rows.
private struct StructureGrid<Header: View, Rows: View>: View {
    @ViewBuilder var header: Header
    @ViewBuilder var rows: Rows

    var body: some View {
        Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 7) {
            header
            Divider()
            rows
        }
        .font(.system(size: 13))
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 8).fill(.primary.opacity(0.03)))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.primary.opacity(0.08), lineWidth: 0.5))
    }
}

private struct HeaderCell: View {
    let title: String
    init(_ title: String) { self.title = title }

    var body: some View {
        Text(title)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(.secondary)
    }
}

/// Monospaced text for types, defaults and column lists. Long values are cut with a tooltip.
private struct Code: View {
    let text: String
    var truncate = false

    init(_ text: String, truncate: Bool = false) {
        self.text = text
        self.truncate = truncate
    }

    var body: some View {
        Text(text)
            .font(.system(size: 12, design: .monospaced))
            .lineLimit(truncate ? 1 : nil)
            .truncationMode(.tail)
            .frame(maxWidth: truncate ? 420 : nil, alignment: .leading)
            .help(truncate && text.count > 50 ? text : "")
    }
}

// MARK: - DDL

/// The table's CREATE statement, highlighted like the editor, with a copy button.
private struct DDLView: View {
    let sql: String
    let fontSize: CGFloat
    @State private var copied = false

    var body: some View {
        Text(highlighted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 8).fill(.primary.opacity(0.03)))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.primary.opacity(0.08), lineWidth: 0.5))
            .overlay(alignment: .topTrailing) {
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(sql, forType: .string)
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .padding(8)
            }
    }

    private var highlighted: AttributedString {
        let theme = SQLTheme(fontSize: fontSize)
        var result = AttributedString(sql)
        result.font = Font(theme.font as CTFont)
        result.foregroundColor = Color(nsColor: .textColor)
        for span in SQLSyntax.highlight(sql) {
            guard let stringRange = Range(span.range, in: sql),
                  let range = Range(stringRange, in: result)
            else { continue }
            let attributes = theme.attributes(for: span.kind)
            if let color = attributes[.foregroundColor] as? NSColor { result[range].foregroundColor = Color(nsColor: color) }
            if let font = attributes[.font] as? NSFont { result[range].font = Font(font as CTFont) }
        }
        return result
    }
}
