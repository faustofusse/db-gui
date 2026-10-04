//! T-SQL text handling: `GO` batches, the filter guard and SSMS-style error messages.

use crate::driver::{Error, Result};

/// One batch of a script, as sent to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub sql: String,
    /// 1-based line in the script where the batch starts, to turn server line numbers into script ones.
    pub first_line: u32,
    /// `GO 3` runs the batch three times.
    pub repeat: u32,
}

/// Lexer state carried from one line to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Code,
    /// Inside `'…'`, `"…"` or `[…]`; holds the closing character.
    Quoted(char),
    /// Inside `/* … */`, which nest in T-SQL.
    Comment(u32),
}

/// Splits a script on `GO` lines, like sqlcmd and SSMS: `GO` is a client command, never sent to
/// the server. A `GO` inside a string, quoted name or block comment doesn't count. Empty batches
/// are dropped.
pub fn split_batches(script: &str) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut first_line = 1;
    let mut state = State::Code;
    for (index, line) in script.split_inclusive('\n').enumerate() {
        let number = index as u32 + 1;
        if state == State::Code {
            if let Some(repeat) = go_line(line) {
                if !current.trim().is_empty() {
                    batches.push(Batch { sql: std::mem::take(&mut current), first_line, repeat });
                }
                current.clear();
                first_line = number + 1;
                continue;
            }
        }
        state = scan(line, state, &mut |_| {});
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        batches.push(Batch { sql: current, first_line, repeat: 1 });
    }
    batches
}

/// `GO`, `go 5`, `GO -- comment`: the repeat count, or `None` for any other line.
fn go_line(line: &str) -> Option<u32> {
    let line = line.trim();
    let line = line.split_once("--").map_or(line, |(code, _)| code).trim();
    if !line.get(..2).is_some_and(|go| go.eq_ignore_ascii_case("go")) {
        return None;
    }
    let rest = &line[2..];
    if rest.is_empty() {
        return Some(1);
    }
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    rest.trim().parse().ok().filter(|n| *n > 0)
}

/// Lexes `text` from `state`, calling `code` for every character outside strings, quoted names
/// and comments. Returns the state at the end.
fn scan(text: &str, mut state: State, code: &mut dyn FnMut(char)) -> State {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            State::Quoted(close) => {
                if c == close {
                    // A doubled closing character is an escaped one.
                    if chars.peek() == Some(&close) {
                        chars.next();
                    } else {
                        state = State::Code;
                    }
                }
            }
            State::Comment(depth) => {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    state = if depth == 1 { State::Code } else { State::Comment(depth - 1) };
                } else if c == '/' && chars.peek() == Some(&'*') {
                    chars.next();
                    state = State::Comment(depth + 1);
                }
            }
            State::Code => match c {
                '\'' => state = State::Quoted('\''),
                '"' => state = State::Quoted('"'),
                '[' => state = State::Quoted(']'),
                '-' if chars.peek() == Some(&'-') => {
                    for d in chars.by_ref() {
                        if d == '\n' {
                            break;
                        }
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    state = State::Comment(1);
                }
                c => code(c),
            },
        }
    }
    state
}

/// Extra check for table filters. A T-SQL batch runs several statements without any `;`, so
/// `1=1) delete from t where (1=1` would turn the browse query into two statements. The filter is
/// wrapped in `where ( … )`: requiring balanced parentheses (outside strings, names and comments)
/// keeps it inside them, where only an expression parses.
pub fn check_filter(filter: &str) -> Result<()> {
    let mut depth: i64 = 0;
    let mut unbalanced = false;
    let end = scan(filter, State::Code, &mut |c| match c {
        '(' => depth += 1,
        ')' => {
            depth -= 1;
            unbalanced |= depth < 0;
        }
        _ => {}
    });
    if unbalanced || depth != 0 || end != State::Code {
        return Err(Error::Query(
            "A filter is a single condition, like `status = 'paid'`: check its parentheses and quotes.".into(),
        ));
    }
    Ok(())
}

/// `Msg 208, Level 16, State 1, Line 3` + message, like SSMS. `first_line` shifts the batch-relative
/// line number to the script's.
pub fn format_error(e: &tiberius::error::TokenError, first_line: u32) -> String {
    let mut header = format!("Msg {}, Level {}, State {}", e.code(), e.class(), e.state());
    if !e.procedure().is_empty() {
        // Inside a procedure or trigger the line is the module's own.
        header.push_str(&format!(", Procedure {}, Line {}", e.procedure(), e.line()));
    } else if e.line() > 0 {
        header.push_str(&format!(", Line {}", e.line() + first_line - 1));
    }
    format!("{header}\n{}", e.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batches(script: &str) -> Vec<(String, u32, u32)> {
        split_batches(script).into_iter().map(|b| (b.sql, b.first_line, b.repeat)).collect()
    }

    #[test]
    fn splits_on_go_lines() {
        let script = "create table t (id int)\nGO\n\ninsert t values (1)\n  go 3  \nselect * from t -- go\ngo -- done\n";
        assert_eq!(
            batches(script),
            [
                ("create table t (id int)\n".into(), 1, 1),
                ("\ninsert t values (1)\n".into(), 3, 3),
                ("select * from t -- go\n".into(), 6, 1),
            ]
        );
        assert_eq!(batches("select 1"), [("select 1".into(), 1, 1)]);
        assert!(batches("GO\n go \n").is_empty());
        // Not GO lines: an identifier, GO inside a string or a comment.
        assert_eq!(batches("select 1 as gopher\ngopher\n").len(), 1);
        assert_eq!(batches("select '\nGO\n'").len(), 1);
        assert_eq!(batches("/* outer /* inner */\nGO\n*/ select 1").len(), 1);
        assert_eq!(batches("select [a\nGO\n]").len(), 1);
    }

    #[test]
    fn guards_filters() {
        assert!(check_filter("status = 'paid' and (total > 10 or total < 0)").is_ok());
        assert!(check_filter("[we(ird] = N')' -- )\n").is_ok());
        assert!(check_filter("id in (select id from t /* ( */)").is_ok());
        assert!(check_filter("1=1) delete from t where (1=1").is_err());
        assert!(check_filter("1=1) delete from t --").is_err());
        assert!(check_filter("(1=1").is_err());
        // No backslash escapes in T-SQL: this string ends after `x\`.
        assert!(check_filter(r"a = 'x\') delete from t where (1=1 --'").is_err());
        assert!(check_filter("a = 'unterminated").is_err());
    }
}
