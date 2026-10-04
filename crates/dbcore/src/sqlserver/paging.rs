//! Keyset paging for SQL Server: page SQL, cursor values from the wire, and literals for them.
//!
//! The seek conditions themselves come from [`crate::keyset`]: SQL Server sorts NULLs first
//! ascending and has no row-value comparisons, so they use the expanded `a > x or (a = x and …)`
//! form.

use tiberius::ColumnData;

use crate::keyset::CursorValue;

use super::{decode, MSSQL};

/// `select * from rel [where (filter) and (seek)] order by … offset m rows fetch next n rows only`.
/// OFFSET/FETCH needs an ORDER BY: `(select null)` when there's nothing to sort by.
/// `filter` must come from [`crate::dialect::normalize_filter`] and `check_filter`.
pub(super) fn page_sql(relation: &str, filter: Option<&str>, seek: Option<&str>, order_by: &[String], limit: u64, offset: u64) -> String {
    let mut sql = format!("select * from {relation}");
    // The filter goes on its own lines so a trailing `-- comment` can't swallow the closing paren.
    match (filter, seek) {
        (Some(f), Some(s)) => sql.push_str(&format!(" where (\n{f}\n) and ({s})")),
        (Some(f), None) => sql.push_str(&format!(" where (\n{f}\n)")),
        (None, Some(s)) => sql.push_str(&format!(" where {s}")),
        (None, None) => {}
    }
    let order = if order_by.is_empty() { "(select null)".to_string() } else { order_by.join(", ") };
    sql.push_str(&format!(" order by {order} offset {offset} rows fetch next {limit} rows only"));
    sql
}

/// How a sort key's value is written back into a seek predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyKind {
    /// Integers, bit, decimal/numeric, money: bare digits.
    Number,
    /// `float`: `1.5e0`, a float literal (a bare decimal could overflow `numeric`'s 38 digits).
    Float,
    /// `varchar`/`char`: `'…'`. An `N'…'` literal would turn the comparison into a Unicode one,
    /// which can order differently from the column's (SQL) collation.
    Ansi,
    /// `nvarchar`/`nchar`: `N'…'`.
    Unicode,
    /// `datetime`/`smalldatetime`: ISO 8601 with a `T`, the only text form these read the same
    /// whatever the session's `DATEFORMAT`.
    LegacyDateTime,
    /// date, time, datetime2, datetimeoffset, uniqueidentifier: `'…'`, unambiguous for these types.
    Quoted,
    /// `binary`/`varbinary`: `0x…`.
    Binary,
}

/// Types whose `>`/`<` agree with `ORDER BY` and whose values round-trip exactly. Not `real`
/// (its values never equal their decimal text), `max` types, xml, sql_variant, CLR/alias types,
/// rowversion, text/ntext/image (not comparable).
pub(super) fn key_kind(type_name: &str) -> Option<KeyKind> {
    let lower = type_name.to_ascii_lowercase();
    if lower.contains("(max)") {
        return None;
    }
    let base = lower.split('(').next().unwrap_or_default().trim();
    match base {
        "tinyint" | "smallint" | "int" | "bigint" | "bit" | "decimal" | "numeric" | "money" | "smallmoney" => Some(KeyKind::Number),
        "float" => Some(KeyKind::Float),
        "varchar" | "char" => Some(KeyKind::Ansi),
        "nvarchar" | "nchar" => Some(KeyKind::Unicode),
        "datetime" | "smalldatetime" => Some(KeyKind::LegacyDateTime),
        "date" | "time" | "datetime2" | "datetimeoffset" | "uniqueidentifier" => Some(KeyKind::Quoted),
        "binary" | "varbinary" => Some(KeyKind::Binary),
        _ => None,
    }
}

/// The exact value of a sort-key cell, to resume after it.
pub(super) fn key_value(data: &ColumnData<'_>) -> CursorValue {
    match data {
        ColumnData::Binary(Some(b)) => CursorValue::Bytes(b.to_vec()),
        ColumnData::F64(Some(f)) => CursorValue::float(*f),
        ColumnData::F32(Some(f)) => CursorValue::float(f64::from(*f)),
        ColumnData::Bit(Some(b)) => CursorValue::Int(i64::from(*b)),
        other => match decode::value(other) {
            crate::Value::Null => CursorValue::Null,
            crate::Value::Int(i) => CursorValue::Int(i),
            crate::Value::Bool(b) => CursorValue::Int(i64::from(b)),
            crate::Value::Float(f) => CursorValue::float(f),
            crate::Value::Decimal(s) | crate::Value::Text(s) => CursorValue::Text(s),
        },
    }
}

pub(super) fn render_key(kind: KeyKind, value: &CursorValue) -> String {
    match (kind, value) {
        (_, CursorValue::Null) => unreachable!("NULL keys are never rendered"),
        (_, CursorValue::Int(i)) => i.to_string(),
        (_, CursorValue::Float(bits)) => format!("{:e}", f64::from_bits(*bits)),
        (_, CursorValue::Bytes(b)) => {
            format!("0x{}", b.iter().map(|b| format!("{b:02x}")).collect::<String>())
        }
        (KeyKind::Number, CursorValue::Text(t)) if is_number(t) => t.clone(),
        (KeyKind::Unicode, CursorValue::Text(t)) => MSSQL.quote_literal(t),
        (KeyKind::LegacyDateTime, CursorValue::Text(t)) => ansi(&t.replacen(' ', "T", 1)),
        (_, CursorValue::Text(t)) => ansi(t),
    }
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit() || b == b'-' || b == b'.')
}

/// `'…'` without the `N` prefix.
fn ansi(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyset::{seek_segments, SeekColumn};
    use crate::model::DatabaseKind;

    #[test]
    fn builds_page_sql() {
        assert_eq!(page_sql("[t]", None, None, &[], 11, 0), "select * from [t] order by (select null) offset 0 rows fetch next 11 rows only");
        assert_eq!(
            page_sql("[t]", Some("a = 1 -- x"), Some("[id] > 5"), &["[id]".into()], 8, 0),
            "select * from [t] where (\na = 1 -- x\n) and ([id] > 5) order by [id] offset 0 rows fetch next 8 rows only"
        );
        assert_eq!(page_sql("[t]", None, None, &["[a] desc".into()], 8, 16), "select * from [t] order by [a] desc offset 16 rows fetch next 8 rows only");
    }

    #[test]
    fn spells_keys_per_type() {
        assert_eq!(key_kind("decimal(10,2)"), Some(KeyKind::Number));
        assert_eq!(key_kind("nvarchar(50)"), Some(KeyKind::Unicode));
        assert_eq!(key_kind("varchar(max)"), None);
        assert_eq!(key_kind("real"), None);
        assert_eq!(key_kind("datetime2(7)"), Some(KeyKind::Quoted));
        assert_eq!(key_kind("rowversion"), None);

        assert_eq!(render_key(KeyKind::Number, &CursorValue::Text("-12.50".into())), "-12.50");
        assert_eq!(render_key(KeyKind::Float, &CursorValue::float(1.0 / 3.0)), "3.333333333333333e-1");
        assert_eq!(render_key(KeyKind::Float, &CursorValue::float(1e300)), "1e300");
        assert_eq!(render_key(KeyKind::Ansi, &CursorValue::Text("it's".into())), "'it''s'");
        assert_eq!(render_key(KeyKind::Unicode, &CursorValue::Text("é".into())), "N'é'");
        assert_eq!(render_key(KeyKind::LegacyDateTime, &CursorValue::Text("2024-01-02 03:04:05.003".into())), "'2024-01-02T03:04:05.003'");
        assert_eq!(render_key(KeyKind::Quoted, &CursorValue::Text("2024-01-02 03:04:05 +02:00".into())), "'2024-01-02 03:04:05 +02:00'");
        assert_eq!(render_key(KeyKind::Binary, &CursorValue::Bytes(vec![0, 0xff])), "0x00ff");
        assert_eq!(key_value(&ColumnData::Bit(Some(true))), CursorValue::Int(1));
    }

    #[test]
    fn seeks_with_nulls_first_and_no_row_values() {
        let a = SeekColumn { expr: "[a]".into(), descending: false, nullable: true, index: 1 };
        let id = SeekColumn { expr: "[id]".into(), descending: false, nullable: false, index: 0 };
        let render = |_: usize, v: &CursorValue| render_key(KeyKind::Number, v);
        let segments = seek_segments(DatabaseKind::SqlServer, &[a.clone(), id.clone()], &[CursorValue::Int(3), CursorValue::Int(9)], render);
        assert_eq!(segments, ["[a] >= 3 and (([a] > 3) or ([a] = 3 and [id] > 9))"]);
        // After a NULL: the remaining NULLs, then every non-NULL value (NULLs sort first).
        let segments = seek_segments(DatabaseKind::SqlServer, &[a, id], &[CursorValue::Null, CursorValue::Int(9)], render);
        assert_eq!(segments, ["[a] is null and ([id] > 9)", "[a] is not null"]);
    }
}
