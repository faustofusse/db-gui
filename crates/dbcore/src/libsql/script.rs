//! Splits a script into statements for a Hrana batch (the server runs one statement per step).
//!
//! Boundaries come from SQLite's own `sqlite3_complete()` (rusqlite's bundled SQLite, already
//! linked), so quotes, comments and `CREATE TRIGGER … BEGIN …; …; END;` are read exactly as
//! SQLite reads them.

use std::ffi::CString;

/// One statement of a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Statement<'a> {
    /// The statement, with its trailing `;` (if any).
    pub sql: &'a str,
    /// Byte offset of its first token in the script, for error positions.
    pub start: usize,
}

/// The script's statements in order; comment-only and empty pieces are left out.
pub(crate) fn split(script: &str) -> Vec<Statement<'_>> {
    let mut statements = Vec::new();
    let mut start = 0;
    for (i, _) in script.match_indices(';') {
        let piece = &script[start..=i];
        if is_complete(piece) {
            push(&mut statements, script, start, i + 1);
            start = i + 1;
        }
    }
    push(&mut statements, script, start, script.len());
    statements
}

fn push<'a>(statements: &mut Vec<Statement<'a>>, script: &'a str, start: usize, end: usize) {
    let piece = &script[start..end];
    let Some(first) = first_token(piece) else { return };
    statements.push(Statement { sql: piece[first..].trim_end(), start: start + first });
}

fn is_complete(sql: &str) -> bool {
    // A NUL byte can't be passed to SQLite; such a piece never ends a statement here (the server
    // reports the error).
    let Ok(text) = CString::new(sql) else { return false };
    // SAFETY: `text` is a valid NUL-terminated string that outlives the call.
    unsafe { rusqlite::ffi::sqlite3_complete(text.as_ptr()) != 0 }
}

/// Byte offset of the first character that isn't whitespace, a comment or a `;`.
fn first_token(sql: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b if b.is_ascii_whitespace() || b == b';' => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 2;
            }
            _ => return Some(i),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqls(script: &str) -> Vec<&str> {
        split(script).into_iter().map(|s| s.sql).collect()
    }

    #[test]
    fn splits_on_top_level_semicolons() {
        assert_eq!(sqls("select 1; select 2"), ["select 1;", "select 2"]);
        assert_eq!(sqls("select 'a;b'; select \"c;d\" -- e;f\n; select 3;"), ["select 'a;b';", "select \"c;d\" -- e;f\n;", "select 3;"]);
        assert_eq!(sqls("/* x; */ select 1 /* ; */;"), ["select 1 /* ; */;"]);
    }

    #[test]
    fn keeps_triggers_whole() {
        let script = "create trigger t after insert on a begin\n  update a set x = 1;\n  delete from b;\nend;\nselect 1;";
        let pieces = sqls(script);
        assert_eq!(pieces.len(), 2);
        assert!(pieces[0].starts_with("create trigger") && pieces[0].ends_with("end;"));
    }

    #[test]
    fn drops_blank_pieces_and_tracks_offsets() {
        assert!(split("  ;; -- only a comment\n /* and this */ ").is_empty());
        let script = "select 1;\n\n  -- note\n  selec 2;";
        let statements = split(script);
        assert_eq!(statements.len(), 2);
        assert_eq!(&script[statements[1].start..], "selec 2;");
    }
}
