//! Per-database SQL spelling, shared by the drivers and by features that generate SQL
//! (sorting, filters, editing…), so those are written once for every backend.

use crate::model::DatabaseKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialect(pub DatabaseKind);

impl Dialect {
    /// Quotes an identifier: `"name"` (Postgres, SQLite) or `` `name` `` (MySQL).
    pub fn quote_ident(self, name: &str) -> String {
        match self.0 {
            DatabaseKind::Mysql => format!("`{}`", name.replace('`', "``")),
            DatabaseKind::Postgres | DatabaseKind::Sqlite => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// `schema.table`, both quoted. For MySQL the schema is the database; for SQLite the attached database name.
    pub fn quote_relation(self, schema: &str, name: &str) -> String {
        format!("{}.{}", self.quote_ident(schema), self.quote_ident(name))
    }

    /// A string literal, e.g. for catalog queries that can't take parameters.
    pub fn quote_literal(self, value: &str) -> String {
        match self.0 {
            // Backslash is an escape character in MySQL strings (unless NO_BACKSLASH_ESCAPES).
            DatabaseKind::Mysql => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''")),
            DatabaseKind::Postgres | DatabaseKind::Sqlite => format!("'{}'", value.replace('\'', "''")),
        }
    }

    /// One page of rows: `select * from rel [order by …] limit n offset m` (same spelling everywhere).
    pub fn page_query(self, relation: &str, order_by: &[String], limit: u32, offset: u64) -> String {
        let order = if order_by.is_empty() { String::new() } else { format!(" order by {}", order_by.join(", ")) };
        format!("select * from {relation}{order} limit {limit} offset {offset}")
    }
}

/// Binary values shown in the grid: `0x0a1b…`, cut after `MAX_BLOB_PREVIEW` bytes.
pub(crate) fn hex_preview(bytes: &[u8]) -> String {
    const MAX_BLOB_PREVIEW: usize = 4096;
    let shown = &bytes[..bytes.len().min(MAX_BLOB_PREVIEW)];
    let mut out = String::with_capacity(2 + shown.len() * 2 + 1);
    out.push_str("0x");
    for b in shown {
        out.push_str(&format!("{b:02x}"));
    }
    if bytes.len() > MAX_BLOB_PREVIEW {
        out.push('…');
    }
    out
}

/// An error with its sources: "error connecting to server: Connection refused (os error 61)".
pub(crate) fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut message = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = s.source();
    }
    message
}

/// 1-based line and column of a character offset (0-based) in `sql`.
pub(crate) fn line_column(sql: &str, char_offset: usize) -> (usize, usize) {
    let before: String = sql.chars().take(char_offset).collect();
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_per_dialect() {
        let pg = Dialect(DatabaseKind::Postgres);
        let my = Dialect(DatabaseKind::Mysql);
        assert_eq!(pg.quote_relation("public", r#"we"ird"#), r#""public"."we""ird""#);
        assert_eq!(my.quote_relation("shop", "we`ird"), "`shop`.`we``ird`");
        assert_eq!(my.quote_literal(r"it's a\b"), r"'it''s a\\b'");
        assert_eq!(pg.quote_literal(r"it's a\b"), r"'it''s a\b'");
    }

    #[test]
    fn builds_page_queries() {
        let d = Dialect(DatabaseKind::Sqlite);
        assert_eq!(d.page_query("\"main\".\"t\"", &[], 10, 0), "select * from \"main\".\"t\" limit 10 offset 0");
        assert_eq!(d.page_query("t", &["\"id\"".into()], 5, 20), "select * from t order by \"id\" limit 5 offset 20");
    }

    #[test]
    fn previews_blobs() {
        assert_eq!(hex_preview(&[0, 0xab, 0xff]), "0x00abff");
        assert!(hex_preview(&vec![1; 5000]).ends_with('…'));
    }

    #[test]
    fn maps_offsets_to_line_and_column() {
        assert_eq!(line_column("select\n  fro", 9), (2, 3));
        assert_eq!(line_column("selec 1", 0), (1, 1));
    }
}
