//! SQL literals for dumped values, written from raw driver values (exact, never truncated).

use std::fmt::Write;

/// `'it''s'`: standard SQL string (Postgres with `standard_conforming_strings`, SQLite).
pub(crate) fn sql_string(out: &mut String, text: &str) {
    out.reserve(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
}

/// A MySQL string literal with mysqldump's escapes (`\0`, `\n`, `\r`, `\\`, `\'`, `\"`, `\Z`).
pub(crate) fn mysql_string(out: &mut String, text: &str) {
    out.reserve(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        match c {
            '\0' => out.push_str("\\0"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\x1a' => out.push_str("\\Z"),
            c => out.push(c),
        }
    }
    out.push('\'');
}

fn hex_digits(out: &mut String, bytes: &[u8]) {
    out.reserve(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
}

/// MySQL hex literal: `0x0aff` (`''` when empty: `0x` alone isn't valid).
pub(crate) fn mysql_hex(out: &mut String, bytes: &[u8]) {
    if bytes.is_empty() {
        out.push_str("''");
    } else {
        out.push_str("0x");
        hex_digits(out, bytes);
    }
}

/// SQLite blob literal: `X'0aff'`.
pub(crate) fn sqlite_blob(out: &mut String, bytes: &[u8]) {
    out.push_str("X'");
    hex_digits(out, bytes);
    out.push('\'');
}

/// SQLite text: a plain literal, or `CAST(X'…' AS TEXT)` for text with NULs or invalid UTF-8,
/// which a literal can't carry.
pub(crate) fn sqlite_text(out: &mut String, bytes: &[u8]) {
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => sql_string(out, text),
        _ => {
            out.push_str("CAST(");
            sqlite_blob(out, bytes);
            out.push_str(" AS TEXT)");
        }
    }
}

/// A REAL that reads back as the same `f64` (and stays REAL: `1.0`, not `1`).
/// Infinities are written as `1e999`/`-1e999`, like `sqlite3 .dump`; NaN (stored as NULL by SQLite) as NULL.
pub fn sqlite_float(value: f64) -> String {
    if value.is_nan() {
        "NULL".into()
    } else if value.is_infinite() {
        if value > 0.0 { "1e999".into() } else { "-1e999".into() }
    } else {
        // Debug is the shortest round-trip form, with an exponent for very big/small values.
        format!("{value:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(f: impl FnOnce(&mut String)) -> String {
        let mut s = String::new();
        f(&mut s);
        s
    }

    #[test]
    fn strings() {
        assert_eq!(with(|s| sql_string(s, r"it's a\b ü")), r"'it''s a\b ü'");
        assert_eq!(with(|s| mysql_string(s, "a'b\"c\\d\0e\nf\r\x1a")), r#"'a\'b\"c\\d\0e\nf\r\Z'"#);
    }

    #[test]
    fn binary() {
        assert_eq!(with(|s| mysql_hex(s, &[0, 0xab])), "0x00ab");
        assert_eq!(with(|s| mysql_hex(s, &[])), "''");
        assert_eq!(with(|s| sqlite_blob(s, &[1, 2])), "X'0102'");
        assert_eq!(with(|s| sqlite_blob(s, &[])), "X''");
        assert_eq!(with(|s| sqlite_text(s, b"o'k")), "'o''k'");
        assert_eq!(with(|s| sqlite_text(s, b"a\0b")), "CAST(X'610062' AS TEXT)");
        assert_eq!(with(|s| sqlite_text(s, &[0xff])), "CAST(X'ff' AS TEXT)");
    }

    #[test]
    fn floats() {
        assert_eq!(sqlite_float(1.0), "1.0");
        assert_eq!(sqlite_float(-0.0), "-0.0");
        assert_eq!(sqlite_float(0.1), "0.1");
        assert_eq!(sqlite_float(1e300), "1e300");
        assert_eq!(sqlite_float(f64::INFINITY), "1e999");
        assert_eq!(sqlite_float(f64::NEG_INFINITY), "-1e999");
        assert_eq!(sqlite_float(f64::NAN), "NULL");
        for v in [std::f64::consts::PI, 1.0 / 3.0, 5e-324, f64::MAX] {
            assert_eq!(sqlite_float(v).parse::<f64>().unwrap(), v);
        }
    }
}
