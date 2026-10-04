//! TDS values → [`Value`], and type names.
//!
//! Dates and times are formatted here (no chrono): SQL Server sends day counts and clock ticks,
//! shown like SSMS does (`2024-01-02 03:04:05.1234567 +02:00`).

use tiberius::numeric::Numeric;
use tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime};
use tiberius::{ColumnData, ColumnType};

use crate::dialect::hex_preview;
use crate::model::Value;

/// Days from 0001-01-01 (`date`, `datetime2`) and 1900-01-01 (`datetime`) to 1970-01-01.
const DAYS_0001_TO_UNIX: i64 = 719_162;
const DAYS_1900_TO_UNIX: i64 = 25_567;

pub(super) fn value(data: &ColumnData<'_>) -> Value {
    match data {
        ColumnData::U8(v) => v.map_or(Value::Null, |v| Value::Int(v.into())),
        ColumnData::I16(v) => v.map_or(Value::Null, |v| Value::Int(v.into())),
        ColumnData::I32(v) => v.map_or(Value::Null, |v| Value::Int(v.into())),
        ColumnData::I64(v) => v.map_or(Value::Null, Value::Int),
        // Via text, so `real` 0.1 stays 0.1 instead of 0.10000000149011612.
        ColumnData::F32(v) => v.map_or(Value::Null, |v| Value::Float(v.to_string().parse().unwrap_or(v.into()))),
        ColumnData::F64(v) => v.map_or(Value::Null, Value::Float),
        ColumnData::Bit(v) => v.map_or(Value::Null, Value::Bool),
        ColumnData::String(v) => v.as_ref().map_or(Value::Null, |s| Value::Text(s.to_string())),
        ColumnData::Guid(v) => v.map_or(Value::Null, |g| Value::Text(g.hyphenated().to_string().to_uppercase())),
        ColumnData::Binary(v) => v.as_ref().map_or(Value::Null, |b| Value::Text(hex_preview(b))),
        ColumnData::Numeric(v) => v.map_or(Value::Null, |n| Value::Decimal(numeric(n))),
        ColumnData::Xml(v) => v.as_ref().map_or(Value::Null, |x| Value::Text(x.to_string())),
        ColumnData::DateTime(v) => v.map_or(Value::Null, |d| Value::Text(datetime(d))),
        ColumnData::SmallDateTime(v) => v.map_or(Value::Null, |d| Value::Text(smalldatetime(d))),
        ColumnData::Time(v) => v.map_or(Value::Null, |t| Value::Text(time(t.increments(), t.scale()))),
        ColumnData::Date(v) => v.map_or(Value::Null, |d| Value::Text(date(d))),
        ColumnData::DateTime2(v) => v.map_or(Value::Null, |d| Value::Text(datetime2(d))),
        ColumnData::DateTimeOffset(v) => v.map_or(Value::Null, |d| Value::Text(datetimeoffset(d))),
    }
}

/// Exact decimal text: `-12.50`, `42` (scale 0), `0.001`.
pub(super) fn numeric(n: Numeric) -> String {
    let digits = n.value().unsigned_abs().to_string();
    let scale = n.scale() as usize;
    let sign = if n.value() < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = digits.split_at(digits.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// (year, month, day) of a day count relative to 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days_since_unix: i64) -> (i64, u32, u32) {
    let z = days_since_unix + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn ymd(days_since_unix: i64) -> String {
    let (y, m, d) = civil(days_since_unix);
    format!("{y:04}-{m:02}-{d:02}")
}

fn date(d: Date) -> String {
    ymd(i64::from(d.days()) - DAYS_0001_TO_UNIX)
}

/// `hh:mm:ss[.fffffff]` from ticks of 10^-scale seconds since midnight.
fn time(increments: u64, scale: u8) -> String {
    let per_second = 10u64.pow(u32::from(scale));
    let seconds = increments / per_second;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if scale == 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}.{:0width$}", increments % per_second, width = scale as usize)
    }
}

fn datetime2(d: DateTime2) -> String {
    format!("{} {}", date(d.date()), time(d.time().increments(), d.time().scale()))
}

/// Stored in UTC; shown in its own offset, like SQL Server does.
fn datetimeoffset(d: DateTimeOffset) -> String {
    let utc = d.datetime2();
    let scale = utc.time().scale();
    let per_minute = 60 * 10i128.pow(u32::from(scale));
    let per_day = 24 * 60 * per_minute;
    let offset = i128::from(d.offset());
    let total = i128::from(utc.date().days()) * per_day + i128::from(utc.time().increments()) + offset * per_minute;
    let (days, ticks) = (total.div_euclid(per_day), total.rem_euclid(per_day));
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.unsigned_abs();
    format!(
        "{} {} {sign}{:02}:{:02}",
        ymd(days as i64 - DAYS_0001_TO_UNIX),
        time(ticks as u64, scale),
        offset / 60,
        offset % 60
    )
}

/// `datetime` ticks are 1/300 s, shown rounded to milliseconds (.000, .003, .007) like SQL Server.
fn datetime(d: DateTime) -> String {
    let ms = (u64::from(d.seconds_fragments()) * 10 + 1) / 3;
    format!("{} {}", ymd(i64::from(d.days()) - DAYS_1900_TO_UNIX), time(ms, 3))
}

/// `smalldatetime` counts whole minutes.
fn smalldatetime(d: SmallDateTime) -> String {
    let minutes = u64::from(d.seconds_fragments());
    format!("{} {}", ymd(i64::from(d.days()) - DAYS_1900_TO_UNIX), time(minutes * 60, 0))
}

/// SQL-ish names for script result columns (table columns use the catalog's spelling).
pub(super) fn type_name(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Null => "",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money => "money",
        ColumnType::Money4 => "smallmoney",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Decimaln => "decimal",
        ColumnType::Numericn => "numeric",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::BigVarBin => "varbinary",
        ColumnType::BigBinary => "binary",
        ColumnType::BigVarChar => "varchar",
        ColumnType::BigChar => "char",
        ColumnType::NVarchar => "nvarchar",
        ColumnType::NChar => "nchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::Text => "text",
        ColumnType::Image => "image",
        ColumnType::NText => "ntext",
        ColumnType::SSVariant => "sql_variant",
    }
}

/// A catalog column's type as written in DDL: `nvarchar(50)`, `varbinary(max)`, `decimal(10,2)`,
/// `datetime2(3)`. `max_length` is in bytes (-1 = max), as `sys.columns` reports it.
pub(super) fn column_type(name: &str, max_length: i64, precision: i64, scale: i64) -> String {
    let length = |bytes_per_char: i64| match max_length {
        -1 => "max".to_string(),
        n => (n / bytes_per_char).to_string(),
    };
    match name {
        "varchar" | "char" | "varbinary" | "binary" => format!("{name}({})", length(1)),
        "nvarchar" | "nchar" => format!("{name}({})", length(2)),
        "decimal" | "numeric" => format!("{name}({precision},{scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{name}({scale})"),
        // `timestamp` is SQL Server's old name for `rowversion`, nothing like the SQL standard type.
        "timestamp" => "rowversion".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiberius::time::Time;

    #[test]
    fn formats_numerics_exactly() {
        assert_eq!(numeric(Numeric::new_with_scale(-1250, 2)), "-12.50");
        assert_eq!(numeric(Numeric::new_with_scale(42, 0)), "42");
        assert_eq!(numeric(Numeric::new_with_scale(1, 3)), "0.001");
        let big = Numeric::new_with_scale(12_345_678_901_234_567_890_123_456_789_012_345_678, 10);
        assert_eq!(numeric(big), "1234567890123456789012345678.9012345678");
        // money's maximum, which an f64 can't hold.
        assert_eq!(numeric(Numeric::new_with_scale(9_223_372_036_854_775_807, 4)), "922337203685477.5807");
    }

    #[test]
    fn formats_dates_and_times() {
        // 2024-01-02 is day 738_886 counting from 0001-01-01.
        let day = Date::new(738_886);
        assert_eq!(date(day), "2024-01-02");
        assert_eq!(date(Date::new(0)), "0001-01-01");
        assert_eq!(date(Date::new(3_652_058)), "9999-12-31");
        assert_eq!(time(110_451_234_567, 7), "03:04:05.1234567");
        assert_eq!(time(11_045, 0), "03:04:05");
        assert_eq!(datetime2(DateTime2::new(day, Time::new(11_045_123, 3))), "2024-01-02 03:04:05.123");
        // 01:30 UTC at +02:00 is 03:30 local; 23:00 UTC at +02:00 is the next day.
        let utc = DateTime2::new(day, Time::new(54_000_000_000, 7));
        assert_eq!(datetimeoffset(DateTimeOffset::new(utc, 120)), "2024-01-02 03:30:00.0000000 +02:00");
        let late = DateTime2::new(day, Time::new(82_800, 0));
        assert_eq!(datetimeoffset(DateTimeOffset::new(late, 120)), "2024-01-03 01:00:00 +02:00");
        assert_eq!(datetimeoffset(DateTimeOffset::new(late, -330)), "2024-01-02 17:30:00 -05:30");
        // 1900-01-01 + 45_291 days = 2024-01-02; 1/300 s ticks round to .003/.007.
        assert_eq!(datetime(DateTime::new(45_291, 11_045 * 300 + 1)), "2024-01-02 03:04:05.003");
        assert_eq!(datetime(DateTime::new(45_291, 11_045 * 300 + 2)), "2024-01-02 03:04:05.007");
        assert_eq!(smalldatetime(SmallDateTime::new(45_291, 184)), "2024-01-02 03:04:00");
    }

    #[test]
    fn decodes_values() {
        assert_eq!(value(&ColumnData::F32(Some(0.1))), Value::Float(0.1));
        assert_eq!(value(&ColumnData::I32(None)), Value::Null);
        assert_eq!(value(&ColumnData::Bit(Some(true))), Value::Bool(true));
        assert_eq!(value(&ColumnData::Binary(Some(vec![0xde, 0xad].into()))), Value::Text("0xdead".into()));
        let guid = uuid::Uuid::parse_str("6f9619ff-8b86-d011-b42d-00c04fc964ff").unwrap();
        assert_eq!(value(&ColumnData::Guid(Some(guid))), Value::Text("6F9619FF-8B86-D011-B42D-00C04FC964FF".into()));
    }

    #[test]
    fn spells_catalog_types() {
        assert_eq!(column_type("nvarchar", 100, 0, 0), "nvarchar(50)");
        assert_eq!(column_type("varbinary", -1, 0, 0), "varbinary(max)");
        assert_eq!(column_type("decimal", 9, 10, 2), "decimal(10,2)");
        assert_eq!(column_type("datetime2", 8, 27, 7), "datetime2(7)");
        assert_eq!(column_type("timestamp", 8, 0, 0), "rowversion");
        assert_eq!(column_type("int", 4, 10, 0), "int");
    }
}
