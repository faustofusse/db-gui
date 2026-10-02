import Foundation

/// What kind of thing a `CompletionItem` is, so the popup can pick an icon.
public enum CompletionKind: Sendable, Hashable {
    case keyword
    case schema
    case table
    case view
    case column
    case function
}

public struct CompletionItem: Sendable, Hashable, Identifiable {
    public let label: String
    public let insertText: String
    public let kind: CompletionKind
    /// e.g. a column's type, or a table's schema.
    public let detail: String?

    public var id: String { "\(kind)\u{0}\(label)" }

    public init(label: String, insertText: String, kind: CompletionKind, detail: String?) {
        self.label = label
        self.insertText = insertText
        self.kind = kind
        self.detail = detail
    }
}

/// Completions for one caret position: the UTF-16 range of the editor's text to replace
/// (the word being typed) with an item's `insertText`.
public struct Completions: Sendable {
    public let range: NSRange
    public let items: [CompletionItem]

    public init(range: NSRange, items: [CompletionItem]) {
        self.range = range
        self.items = items
    }
}
