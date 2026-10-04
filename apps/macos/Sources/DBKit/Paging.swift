import Foundation

/// Where the next page of a table starts. Opaque: get it from one `TablePage` and pass it back
/// for the next. The core seeks past the previous page's last row (keyset paging) where it can,
/// so deep pages load as fast as the first.
public struct PageCursor: Sendable, Hashable {
    let token: String
}

/// One page of a table and the cursor for the next (`nil`: this was the last page).
public struct TablePage: Sendable {
    public var result: QueryResult
    public var next: PageCursor?

    public init(result: QueryResult, next: PageCursor?) {
        self.result = result
        self.next = next
    }
}
