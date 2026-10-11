use arrow_buffer::i256;

use super::geometric::{MAX_ALLOC_SIZE, PATH_HEADER, POLYGON_HEADER, fits_allocation};
use super::*;

#[test]
fn bool_round_trips() {
    assert_eq!(decode_bool("t"), Some(true));
    assert_eq!(decode_bool("f"), Some(false));
    assert_eq!(decode_bool("x"), None);
    assert_eq!(render_bool(true), "t");
    assert_eq!(render_bool(false), "f");
}

#[test]
fn float_special_values_round_trip() {
    for (text, v) in
        [("NaN", f64::NAN), ("Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY)]
    {
        let decoded = decode_f64(text).unwrap();
        assert_eq!(decoded.is_nan(), v.is_nan());
        if !v.is_nan() {
            assert_eq!(decoded, v);
        }
        assert_eq!(render_f64(decoded), text);
    }
    assert_eq!(render_f64(decode_f64("-0").unwrap()), "-0");
}

/// **A float past its type's range is refused, as `float8in` and
/// `float4in` refuse it** (I59): every spelling `float8out` and
/// `float4out` round the largest finite value past at an
/// `--extra-float-digits` short enough (I57), down to `-15`'s one digit,
/// and a nonzero one read as zero. A spelling this build does not read,
/// which `strtod` does, is unparsed rather than refused.
#[test]
fn a_float_past_its_range_is_refused_and_one_in_it_read() {
    for text in [
        "1.79769313486232e+308",
        "1.7976931349e+308",
        "1.797693135e+308",
        "1.7977e+308",
        "1.798e+308",
        "1.8e+308",
        "2e+308",
        "1e+309",
        "1e-400",
    ] {
        assert_eq!(float_in::<f64>(text), Err(Unread::Refused), "{text}");
        assert_eq!(float_in::<f64>(&format!("-{text}")), Err(Unread::Refused), "-{text}");
    }
    for text in ["3.403e+38", "3.4e+39", "4e+38", "1e-46"] {
        assert_eq!(float_in::<f32>(text), Err(Unread::Refused), "{text}");
        assert_eq!(float_in::<f32>(&format!("-{text}")), Err(Unread::Refused), "-{text}");
    }
    for text in [" 1.5", "1.5 ", "0x1p3", "nan(1)"] {
        assert_eq!(float_in::<f64>(text), Err(Unread::Unparsed), "{text:?}");
    }
    for text in ["1_0", "", " 1e400", "0x1p1024"] {
        assert_eq!(float_in::<f64>(text), Err(Unread::Refused), "{text:?}");
    }
    assert_eq!(decode_f64("1.79769313486232e+308"), None);
    assert_eq!(decode_f64("1.7976931348623157e+308"), Some(f64::MAX));
    assert_eq!(decode_f32("3.40282e+38"), Some(3.40282e38));
    assert_eq!(decode_f64("Infinity"), Some(f64::INFINITY));
    assert_eq!(decode_f64("-Infinity"), Some(f64::NEG_INFINITY));
    assert_eq!(decode_f64("1e-310"), Some(1e-310));
    assert_eq!(decode_f64("0e-999"), Some(0.0));
}

/// **Told to ignore the refusal, a float field past its type's range is
/// read as the parse rounds it**: the largest finite value of its sign past
/// it, zero of its sign below it; a spelling this build does not read stays
/// unread, and told nothing, every one of them is.
#[test]
fn a_float_field_past_its_range_is_read_only_when_ignored() {
    use pgdt_model::PostgresInvalidValues::{Default, Ignore};
    for (text, read) in [
        ("1.79769313486232e+308", f64::MAX),
        ("-1.79769313486232e+308", -f64::MAX),
        ("1e+309", f64::MAX),
        ("1e-400", 0.0),
    ] {
        assert_eq!(float_field::<f64>(text, Default), None, "{text}");
        assert_eq!(float_field::<f64>(text, Ignore), Some(read), "{text}");
    }
    assert!(float_field::<f64>("-1e-400", Ignore).unwrap().is_sign_negative());
    assert_eq!(float_field::<f32>("3.403e+38", Ignore), Some(f32::MAX));
    assert_eq!(float_field::<f32>("-3.403e+38", Ignore), Some(-f32::MAX));
    for text in ["1.5", "Infinity", "NaN"] {
        let (default, ignored) = (float_field::<f64>(text, Default), float_field(text, Ignore));
        assert_eq!(default.map(f64::to_bits), ignored.map(f64::to_bits), "{text}");
        assert!(default.is_some(), "{text}");
    }
    assert_eq!(float_field::<f64>("0x1p3", Ignore), None);
}

/// Ground truth from a live server under `extra_float_digits = 3` —
/// exercises the fixed/scientific switch at both ends and at both
/// `FLT_DIG`/`DBL_DIG` thresholds, which a fixture round-trip alone
/// can't be relied on to hit.
#[test]
fn float_formatting_matches_postgresql_exactly() {
    let cases: &[(&str, f64)] = &[
        ("1e+100", 1e100),
        ("1e+20", 1e20),
        ("1e+21", 1e21),
        ("1.2345678901234568e+20", 1.2345678901234568e20),
        ("0.00015", 1.5e-4),
        ("1.5e-05", 1.5e-5),
        ("0.0001", 0.0001),
        ("-1e+100", -1e100),
        ("1e-300", 1e-300),
        ("0", 0.0),
        ("1.234567890123456e+15", 1234567890123456.0),
        ("100000000000000", 1e14),
        ("1e+15", 1e15),
        ("123456789012345", 123456789012345.0),
    ];
    for (text, v) in cases {
        assert_eq!(render_f64(*v), *text, "value {v}");
        assert_eq!(decode_f64(text), Some(*v), "text {text}");
    }
    let f32_cases: &[(&str, f32)] = &[
        ("1e+06", 1e6),
        ("100000", 1e5),
        ("1.2345678e+07", 1.234_567_8e7),
        ("1.1754944e-38", 1.175_494_4e-38),
    ];
    for (text, v) in f32_cases {
        assert_eq!(render_f32(*v), *text, "value {v}");
        assert_eq!(decode_f32(text), Some(*v), "text {text}");
    }
}

#[test]
fn date_boundary_values() {
    for text in ["0001-01-01", "9999-12-31", "0044-01-01 BC", "10000-01-01"] {
        let days = decode_date32(text).expect(text);
        assert_eq!(render_date32(days), text, "{text}");
    }
    assert_eq!(decode_date32("infinity"), None);
    assert_eq!(decode_date32("-infinity"), None);
}

#[test]
fn epoch_and_negative_days() {
    assert_eq!(decode_date32("1970-01-01"), Some(0));
    assert_eq!(decode_date32("1969-12-31"), Some(-1));
    assert_eq!(render_date32(0), "1970-01-01");
}

/// The range's two ends are `DATETIME_MIN_JULIAN` and `DATE_END_JULIAN`
/// as days from 1970, which `timestamp.h` states as Julian days (I61).
#[test]
fn the_date_range_constants_are_julian_days_from_1970() {
    let julian_1970 = 2_440_588;
    assert_eq!(days_from_civil(-4713, 11, 24), DATE_MIN_DAYS);
    assert_eq!(DATE_MIN_DAYS + julian_1970, 0);
    assert_eq!(days_from_civil(JULIAN_MAX_YEAR, 1, 1), DATE_END_DAYS);
    assert_eq!(DATE_END_DAYS + julian_1970, 2_147_483_494);
    let postgres_epoch_days = 10_957;
    assert_eq!(days_from_civil(2000, 1, 1), postgres_epoch_days);
    let day = 86_400_000_000;
    assert_eq!(i128::from(MIN_TIMESTAMP), (DATE_MIN_DAYS - postgres_epoch_days) as i128 * day);
    let end_days = days_from_civil(294_277, 1, 1) - postgres_epoch_days;
    assert_eq!(i128::from(END_TIMESTAMP), i128::from(end_days) * day);
}

/// **A date is bounded part by part as `ValidateDate` bounds it, and to
/// `IS_VALID_DATE`'s range** (I61). Each refusal was cast on PostgreSQL
/// 16; the shortfalls beside them are spellings the server reads by
/// `DateOrder` or as a day of the year, which this build does not (D55).
#[test]
fn a_date_the_calendar_does_not_hold_is_refused() {
    for text in [
        "2020-02-30",
        "2019-02-29",
        "2020-13-01",
        "2020-00-01",
        "2020-01-00",
        "2020-01-32",
        "2020-04-31",
        "0000-01-01",
        "0000-01-01 BC",
        "+2020-01-01",
        "2020-+1-01",
        "2020-01-+1",
        "4714-11-23 BC",
        "5874898-01-01",
        "99999999999999999999-01-01",
    ] {
        assert_eq!(decode_date32(text), None, "{text}");
    }
    // Shortfalls: the server reads each.
    for text in ["20-01-01", "1-01-01", "2020-001-05"] {
        assert_eq!(decode_date32(text), None, "{text}");
    }
    for (text, read_as) in [
        ("2020-02-29", "2020-02-29"),
        ("0001-02-29 BC", "0001-02-29 BC"),
        ("020-01-01", "0020-01-01"),
        ("2020-1-5", "2020-01-05"),
        ("2020-01-001", "2020-01-01"),
        ("4714-11-24 BC", "4714-11-24 BC"),
        ("5874897-12-31", "5874897-12-31"),
    ] {
        let days = decode_date32(text).expect(text);
        assert_eq!(render_date32(days), read_as, "{text}");
    }
}

/// **A time of day is bounded as `time_overflows` bounds it**, and a
/// part carrying a sign is unparsed rather than read as a signed number:
/// the server reads `12:-5:00` as 12:00 at zone `-5`, where the prior
/// reading made it 11:55 (I61).
#[test]
fn a_time_past_time_overflows_is_refused() {
    for text in [
        "12:60:00",
        "12:05:61",
        "25:00:00",
        "24:00:00.000001",
        "24:00:01",
        "24:01:00",
        "23:59:60.5",
    ] {
        assert_eq!(time_of_day_micros(text), Err(Unread::Refused), "{text}");
    }
    for text in ["12:-5:00", "12:+5:00", "12:05:+5", "+12:00:00", "-1:00:00"] {
        assert_eq!(time_of_day_micros(text), Err(Unread::Unparsed), "{text}");
    }
    for (text, micros) in [
        ("12:59:60", 46_800_000_000),
        ("23:59:60", DAY_MICROS),
        ("24:00:00", DAY_MICROS),
        ("012:05:00", 43_500_000_000),
    ] {
        assert_eq!(time_of_day_micros(text), Ok(micros), "{text}");
    }
}

/// **A numeric zone is bounded as `DecodeTimezone` bounds it**,
/// `±15:59:59`, part by part (I61), and the run-together `+0530` the
/// server reads as `+05:30` is unparsed, not refused.
#[test]
fn an_offset_past_fifteen_hours_is_refused() {
    for text in ["12:00:00+16", "12:00:00-16", "12:00:00+05:60", "12:00:00+05:30:60"] {
        assert_eq!(extract_offset(text), Err(Unread::Refused), "{text}");
    }
    for text in ["12:00:00+0530", "12:00:00+-5", "12:00:00+05:+3", "12:00:00"] {
        assert_eq!(extract_offset(text), Err(Unread::Unparsed), "{text}");
    }
    for (text, seconds) in [
        ("12:00:00+15:59:59", 57_599),
        ("12:00:00-15:59:59", -57_599),
        ("12:00:00-00:44:30", -2_670),
        ("12:00:00+05", 18_000),
    ] {
        assert_eq!(extract_offset(text), Ok(("12:00:00", seconds)), "{text}");
    }
}

/// **A timestamp is bounded to `IS_VALID_TIMESTAMP`'s range by its
/// instant**, after its date and time are bounded as a `date`'s and a
/// `time`'s are, so an offset carries a local time across either end
/// (I61). Each was cast on PostgreSQL 16.
#[test]
fn a_timestamp_past_postgresqls_range_is_refused() {
    for (text, with_tz) in [
        ("2020-02-30 00:00:00", false),
        ("2020-01-01 12:60:00", false),
        ("2020-01-01 24:00:01", false),
        ("0000-01-01 00:00:00", false),
        ("4714-11-23 23:59:59.999999 BC", false),
        ("294277-01-01 00:00:00", false),
        ("2020-01-01 00:00:00+16", true),
        ("4714-11-24 00:00:00+01 BC", true),
        ("294276-12-31 23:00:00-02", true),
    ] {
        assert_eq!(timestamp_micros_wide(text, with_tz), Err(Unread::Refused), "{text}");
    }
    for (text, with_tz) in [
        ("2020-01-01 12:-5:00", false),
        ("2020-01-01 12:-5:00", true),
        ("2020-01-01 00:00:00+0530", true),
        ("20-01-01 00:00:00", false),
        ("2020-01-01T00:00:00", false),
    ] {
        assert_eq!(timestamp_micros_wide(text, with_tz), Err(Unread::Unparsed), "{text}");
    }
    let at = |text: &str, with_tz: bool| timestamp_postgres_micros(text, with_tz).expect(text);
    assert_eq!(at("4714-11-24 00:00:00 BC", false), MIN_TIMESTAMP);
    assert_eq!(at("294276-12-31 23:59:59.999999", false), END_TIMESTAMP - 1);
    assert_eq!(at("4714-11-23 23:00:00-02 BC", true), MIN_TIMESTAMP + 3_600_000_000);
    assert_eq!(at("294276-12-31 23:30:00+01", true), END_TIMESTAMP - 5_400_000_000);
    assert_eq!(at("2020-01-01 24:00:00", false), at("2020-01-02 00:00:00", false));
    assert_eq!(at("2020-01-01 23:59:60", false), at("2020-01-02 00:00:00", false));
}

/// `interval_out` under `IntervalStyle = postgres` (I40), round-tripped
/// through the triple. Every string here is a real server's answer (four
/// of them `fixtures/*/types/default.sql`'s `t_interval`), because the
/// three rules that decide the form are all sign-conditional and none of
/// them is reachable from the fixture's own values: a unit takes an `s`
/// whenever the count is not exactly `1` — so `-1 mons` — and a part
/// that is positive and follows a negative one carries a `+`, the time
/// tail included.
#[test]
fn interval_round_trips_every_shape_of_encode_interval() {
    let cases: &[(&str, (i32, i32, i64))] = &[
        ("1 year 2 mons 3 days 04:05:06", (14, 3, 14_706_000_000_000)),
        ("-1 days", (0, -1, 0)),
        ("00:00:00", (0, 0, 0)),
        ("01:30:00", (0, 0, 5_400_000_000_000)),
        ("1 mon", (1, 0, 0)),
        ("30 days", (0, 30, 0)),
        ("720:00:00", (0, 0, 2_592_000_000_000_000)),
        ("100 years", (1200, 0, 0)),
        ("-11 mons", (-11, 0, 0)),
        ("11 mons", (11, 0, 0)),
        ("-1 years -2 mons", (-14, 0, 0)),
        ("2 mons", (2, 0, 0)),
        ("00:00:00.123", (0, 0, 123_000_000)),
        ("-00:00:00.000001", (0, 0, -1_000)),
        // The `+` on a positive part that follows a negative one, at each
        // of the three places it can appear.
        ("-1 days +01:00:00", (0, -1, 3_600_000_000_000)),
        ("-1 mons +1 day", (-1, 1, 0)),
        ("-1 mons +00:00:01", (-1, 0, 1_000_000_000)),
        // A negative time tail after a positive day, which takes `-`
        // rather than the day's sign.
        ("1 day -00:00:00.5", (0, 1, -500_000_000)),
        ("-1 mons -1 days -01:02:03.456789", (-1, -1, -3_723_456_789_000)),
    ];
    for (text, parts) in cases {
        assert_eq!(decode_interval(text), Some(*parts), "decode {text}");
        let (months, days, nanos) = *parts;
        assert_eq!(render_interval(months, days, nanos).as_deref(), Some(*text), "render {text}");
    }
}

/// The three classes with no `Interval(MonthDayNano)` encoding, each a
/// decode failure rather than a wrong value.
#[test]
fn interval_refuses_what_month_day_nano_cannot_hold() {
    // v17's infinities (I34): not in `EncodeInterval`'s grammar at all.
    assert_eq!(decode_interval("infinity"), None);
    assert_eq!(decode_interval("-infinity"), None);
    // The ceiling is Arrow's nanoseconds, a thousandth of PostgreSQL's
    // microsecond range (I40) — one microsecond either side of it.
    assert_eq!(decode_interval("2562047:47:16.854775"), Some((0, 0, 9_223_372_036_854_775_000)));
    assert_eq!(decode_interval("2562047:47:16.854776"), None);
    assert_eq!(decode_interval("-2562047:47:16.854776"), None);
    // Nothing normalizes hours into days, so an ordinary unnormalized
    // value reaches it: `interval '100000000 hours'` is written
    // `100000000:00:00` and is forty times over.
    assert_eq!(decode_interval("100000000:00:00"), None);
    // A month or day count past `i32`, which no `Interval` struct holds.
    assert_eq!(decode_interval("2147483648 days"), None);
    assert_eq!(decode_interval("2147483648 mons"), None);
    assert_eq!(decode_interval("2147483647 days"), Some((0, 2_147_483_647, 0)));
}

/// **What `interval_in` refuses is refused, and its edges read** (I62):
/// a minute past 59 or a second past 60, a count or month total past
/// `int32`, a time past `int64` microseconds, and a unit given twice.
/// Each was cast on PostgreSQL 16; `00:90:00` once read as `01:30:00`.
#[test]
fn an_interval_interval_in_refuses_is_refused() {
    for text in [
        "00:90:00",
        "00:59:61",
        "-00:60:00",
        "2147483648 days",
        "-2147483649 days",
        "2147483648 mons",
        "178956970 years 8 mons",
        "-178956970 years -9 mons",
        "2562047788:00:54.775808",
        "-2562047788:00:54.775808",
        "1 day 1 day",
        "1 mon 1 mons",
        "1 year 1 mon 1 day 1 year",
    ] {
        assert_eq!(interval_parts(text), Err(Unread::Refused), "{text}");
    }
    let micros_max = i64::MAX;
    for (text, parts) in [
        ("00:59:60", (0, 0, 3_600_000_000)),
        ("-00:59:60", (0, 0, -3_600_000_000)),
        ("2147483647 days", (0, i32::MAX, 0)),
        ("-2147483648 days", (0, i32::MIN, 0)),
        ("178956970 years 7 mons", (i32::MAX, 0, 0)),
        ("-178956970 years -8 mons", (i32::MIN, 0, 0)),
        ("178956971 years -12 mons", (2_147_483_640, 0, 0)),
        ("2562047788:00:54.775807", (0, 0, micros_max)),
        ("-2562047788:00:54.775807", (0, 0, -micros_max)),
        ("1 day 1 year", (12, 1, 0)),
    ] {
        assert_eq!(interval_parts(text), Ok(parts), "{text}");
    }
}

/// A unit `interval_out` never writes is outside the grammar — the same
/// refusal the ordering path makes, reached through the decoder that
/// shares its walk.
#[test]
fn interval_refuses_spellings_interval_out_never_writes() {
    for text in ["1 month", "1.5 hours", "P1Y2M", "1 hour", "04:-5:06", "1 year 2"] {
        assert_eq!(decode_interval(text), None, "{text}");
    }
}

/// The one value `render_interval` refuses, and the only refusal on the
/// way *out* rather than in: Arrow counts nanoseconds where PostgreSQL's
/// field counts microseconds, so a nanosecond count with a remainder is
/// not an `interval` at all. `decode_interval` cannot produce one, which
/// is why the whole-microsecond neighbours are asserted beside it —
/// truncating would have written `00:00:00.000001` for all three.
#[test]
fn render_refuses_a_sub_microsecond_nanosecond_count() {
    assert_eq!(render_interval(0, 0, 1), None);
    assert_eq!(render_interval(0, 0, -1), None);
    assert_eq!(render_interval(0, 0, 1_001), None);
    assert_eq!(render_interval(1, 1, 999), None);
    assert_eq!(render_interval(0, 0, 1_000).as_deref(), Some("00:00:00.000001"));
    assert_eq!(render_interval(0, 0, 2_000).as_deref(), Some("00:00:00.000002"));
}

#[test]
fn time_boundary_and_fraction_trimming() {
    // PostgreSQL's inclusive bound, past Arrow's day: refused by the
    // decoder, ordered by the comparison, and rendered back all the same.
    assert_eq!(decode_time64_micros("24:00:00"), None);
    assert_eq!(time_of_day_micros("24:00:00"), Ok(86_400_000_000));
    assert_eq!(decode_time64_micros("23:59:59.999999"), Some(86_399_999_999));
    assert_eq!(render_time64_micros(86_400_000_000), "24:00:00");
    assert_eq!(decode_time64_micros("00:00:00.000001"), Some(1));
    assert_eq!(render_time64_micros(1), "00:00:00.000001");
    // PostgreSQL trims trailing zeros from the fraction rather than
    // always showing 6 digits.
    assert_eq!(render_time64_micros(500_000), "00:00:00.5");
    assert_eq!(render_time64_micros(100_000), "00:00:00.1");
}

#[test]
fn timestamp_boundary_values_without_tz() {
    for text in [
        "2024-01-01 00:00:00",
        "2024-01-01 00:00:00.123456",
        "0001-01-01 00:00:00",
        "0044-01-01 00:00:00 BC",
    ] {
        let micros = decode_timestamp_micros(text, false).expect(text);
        assert_eq!(render_timestamp_micros(micros, false), text, "{text}");
    }
    assert_eq!(decode_timestamp_micros("infinity", false), None);
    assert_eq!(decode_timestamp_micros("-infinity", false), None);
}

/// PostgreSQL's own documented maximum timestamp is defined relative to
/// *its* epoch (2000-01-01); `Timestamp(Microsecond)` is `i64` micros
/// since the Unix epoch (1970-01-01), 30 years earlier, so the same
/// bit width runs out about 30 years sooner counted from 1970 than from
/// 2000: the true representable ceiling is 294247-01-10, not
/// PostgreSQL's 294276-12-31 — so PostgreSQL's own maximum value is a
/// genuine, expected `FieldDecode` overflow for this mapping, not a bug.
#[test]
fn postgresqls_own_max_timestamp_overflows_the_unix_epoch_i64_range() {
    assert_eq!(decode_timestamp_micros("294276-12-31 23:59:59.999999", false), None);
    assert_eq!(decode_timestamp_micros("294276-12-31 23:59:59.999999+00", true), None);
    // A date well inside both ceilings still round-trips fine.
    let text = "294246-12-31 23:59:59.999999";
    let micros = decode_timestamp_micros(text, false).unwrap();
    assert_eq!(render_timestamp_micros(micros, false), text);
}

#[test]
fn timestamptz_normalizes_and_renders_with_utc_offset() {
    for text in ["2023-12-31 18:30:00+00", "0001-01-01 00:00:00+00", "0044-01-01 00:00:00+00 BC"] {
        let micros = decode_timestamp_micros(text, true).expect(text);
        assert_eq!(render_timestamp_micros(micros, true), text, "{text}");
    }
    // A non-UTC offset is still normalized correctly even though every
    // real `pg_dump` output uses +00 (I4) -- +05:00 local is 05:00 UTC
    // earlier in wall-clock terms.
    let with_offset = decode_timestamp_micros("2024-01-01 05:00:00+05", true).unwrap();
    let utc = decode_timestamp_micros("2024-01-01 00:00:00+00", true).unwrap();
    assert_eq!(with_offset, utc);
}

#[test]
fn uuid_round_trips() {
    for text in ["00000000-0000-0000-0000-000000000000", "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"] {
        let bytes = decode_uuid(text).expect(text);
        assert_eq!(render_uuid(&bytes), text);
    }
    assert_eq!(decode_uuid("not-a-uuid"), None);
}

#[test]
fn bytea_round_trips_including_empty() {
    for text in ["\\x", "\\xdeadbeef00ff", "\\x5c6261636b736c617368"] {
        let bytes = decode_bytea(text).expect(text);
        assert_eq!(render_bytea(&bytes), text);
    }
    assert_eq!(decode_bytea("\\xnope"), None);
}

/// `byteaout`'s `escape` form, every byte through it and back, beside
/// what the server writes for a few (`select '\x005c7f41'::bytea` under
/// `bytea_output = escape` is `\000\\\177A`).
#[test]
fn bytea_escape_form_round_trips_every_byte() {
    let every_byte: Vec<u8> = (0..=u8::MAX).collect();
    let text = render_bytea_escape(&every_byte);
    assert_eq!(decode_bytea(&text), Some(every_byte.clone()));
    assert_eq!(decode_bytea_escape(&text, 3), Some(every_byte[..3].to_vec()));
    for b in 0..=u8::MAX {
        assert_eq!(decode_bytea(&render_bytea_escape(&[b])), Some(vec![b]), "{b}");
    }
    assert_eq!(render_bytea_escape(&[0x00, 0x5c, 0x7f, 0x41]), "\\000\\\\\\177A");
    assert_eq!(decode_bytea("\\000\\\\\\177A"), Some(vec![0x00, 0x5c, 0x7f, 0x41]));
    // The empty value is the empty text, and no escape spelling opens
    // `\x` (I56).
    assert_eq!(decode_bytea(""), Some(Vec::new()));
    assert!(!text.starts_with("\\x"));
}

/// What `byteaout` never writes is not read, a shortfall where `byteain`
/// reads it (`docs/design/decisions.md`, "D55"): a printable byte in octal,
/// an octal escape past a byte, one cut short, a lone backslash, a byte
/// outside space to `~` written bare.
#[test]
fn bytea_escape_form_refuses_what_byteaout_never_writes() {
    for text in ["\\101", "\\134", "\\400", "\\08", "\\00", "a\\", "\\y", "tab\there", "é"] {
        assert_eq!(decode_bytea(text), None, "{text:?}");
    }
}

#[test]
fn decimal_numeric_and_boundary_scales() {
    // 38-digit precision, scale 10 (fits Decimal128).
    let text = "1234567890123456789012345678.1234567890";
    let unscaled = decimal_unscaled_digits(text, 10).unwrap();
    assert_eq!(unscaled.parse::<i128>().unwrap().to_string(), unscaled);
    assert_eq!(render_decimal(&unscaled, 10), text);

    // A short value read through `i256`, the unscaled type of a precision
    // past 38 digits (Decimal256), at scale 10.
    let text39 = "123456789.1234567890";
    let unscaled39 = decimal_unscaled_digits(text39, 10).unwrap();
    let as_i256 = i256::from_string(&unscaled39).unwrap();
    assert_eq!(render_decimal(&as_i256.to_string(), 10), text39);

    for (text, scale) in
        [("0.0000000000", 10), ("-1.5000000000", 10), ("0.00", 2), ("-1.50", 2), ("100.00", 2)]
    {
        let unscaled = decimal_unscaled_digits(text, scale).unwrap();
        assert_eq!(render_decimal(&unscaled, scale), text);
    }
}

/// **`numeric_in`'s two storage bounds, each at its edge** (I63): a
/// display scale of 16383 digits, trailing zeros counting, and 131072
/// integer digits, leading zeros not counting, whatever the sign.
#[test]
fn numeric_in_stores_up_to_its_display_scale_and_weight() {
    let zeros = |n| "0".repeat(n);
    for (text, stored) in [
        (format!("1.{}", zeros(16383)), true),
        (format!("1.{}", zeros(16384)), false),
        (format!("0.{}", zeros(16384)), false),
        (format!("-.{}1", zeros(16382)), true),
        (format!("1{}", zeros(131071)), true),
        (format!("1{}", zeros(131072)), false),
        (format!("-1{}", zeros(131072)), false),
        (format!("{}1.5", zeros(200_000)), true),
        ("0".to_string(), true),
    ] {
        assert_eq!(numeric_in_stores(&text), stored, "{}", text.len());
    }
}

#[test]
fn nan_numeric_has_no_decimal_representation() {
    assert_eq!(decimal_unscaled_digits("NaN", 2), None);
    assert_eq!(typmod_unscaled_digits("NaN", 10, 2), Err(Unread::Unparsed));
}

/// **A field is rounded to its scale half away from zero and refused past
/// its precision once rounded**, as `apply_typmod` stores it (I51): every
/// numeric case is a cast's reading on the koji replica (PG16), each
/// refusal its "numeric field overflow" — the carry raising the weight
/// past the precision included. Of the rest, `NaN` has no decimal, the
/// server refuses `Infinity` under a typmod (I34), and it reads `1e5`,
/// which this grammar does not (`docs/design/decisions.md`, "D55"), and
/// refuses `1e8` past the precision as it refuses `.` and the empty text
/// ([`numeric_unread`]).
#[test]
fn a_field_is_put_through_its_typmod_as_copy_puts_it() {
    let zeros = |n| "0".repeat(n);
    for (text, precision, scale, stored) in [
        ("1.005", 10, 2, Ok("101")),
        ("-1.005", 10, 2, Ok("-101")),
        ("1.004", 10, 2, Ok("100")),
        ("-0.004", 10, 2, Ok("0")),
        ("-0.005", 10, 2, Ok("-1")),
        ("1.0", 10, 2, Ok("100")),
        ("12345678.995", 10, 2, Ok("1234567900")),
        ("99999999.995", 10, 2, Err(Unread::Refused)),
        ("123456789012", 10, 2, Err(Unread::Refused)),
        ("99999999.99", 10, 2, Ok("9999999999")),
        ("0.000125", 2, 5, Ok("13")),
        ("0.0000049", 2, 5, Ok("0")),
        (".00001", 2, 5, Ok("1")),
        ("0.000995", 2, 5, Err(Unread::Refused)),
        ("0.001", 2, 5, Err(Unread::Refused)),
        ("1250", 3, -2, Ok("13")),
        ("-1249", 3, -2, Ok("-12")),
        ("99950", 3, -2, Err(Unread::Refused)),
        ("5", 1, -1, Ok("1")),
        ("5", 1, -2, Ok("0")),
        ("0.5", 1, 0, Ok("1")),
        ("9.5", 1, 0, Err(Unread::Refused)),
        ("NaN", 10, 2, Err(Unread::Unparsed)),
        ("1e5", 10, 2, Err(Unread::Unparsed)),
        ("1e8", 10, 2, Err(Unread::Refused)),
        ("Infinity", 10, 2, Err(Unread::Refused)),
        (".", 10, 2, Err(Unread::Refused)),
        ("", 10, 2, Err(Unread::Refused)),
    ] {
        assert_eq!(
            typmod_unscaled_digits(text, precision, scale).as_deref().map_err(|e| *e),
            stored.map_err(|e: Unread| e),
            "{text:?} as numeric({precision},{scale})"
        );
    }
    // `numerictypmodin`'s extreme scales, past every Arrow decimal's.
    let tiny = format!("0.{}5", zeros(999));
    assert_eq!(typmod_unscaled_digits(&tiny, 1, 1000), Ok("5".to_string()));
    assert_eq!(typmod_unscaled_digits(&tiny, 1, 999), Ok("1".to_string()));
    let huge = format!("-1{}", zeros(1000));
    assert_eq!(typmod_unscaled_digits(&huge, 1, -1000), Ok("-1".to_string()));
    assert_eq!(typmod_unscaled_digits(&huge, 1, -999), Err(Unread::Refused));
}

/// **What `numeric_out` writes is stored unchanged**: a value at its
/// column's scale reads as the exact literal reader reads it, at every
/// scale either side of zero, so the typmod moves no value a `pg_dump`
/// wrote.
#[test]
fn a_field_numeric_out_wrote_reads_as_its_literal() {
    for scale in [-3i8, -1, 0, 1, 2, 6] {
        for unscaled in ["0", "7", "-7", "120", "-98765", "1234567"] {
            let text = render_decimal(unscaled, scale);
            assert_eq!(
                typmod_unscaled_digits(&text, 7, i16::from(scale)).ok(),
                decimal_unscaled_digits(&text, scale),
                "{text:?} at scale {scale}"
            );
            assert_eq!(decimal_unscaled_digits(&text, scale).as_deref(), Some(unscaled));
        }
    }
}

#[test]
fn negative_scale_numeric() {
    // PG15+ negative-scale numerics print with no fractional digits at
    // all — not exercised by the fixtures (no negative-scale column
    // there), so pinned here from the type's own documented semantics.
    let unscaled = decimal_unscaled_digits("1200", -2).unwrap();
    assert_eq!(unscaled, "12");
    assert_eq!(render_decimal(&unscaled, -2), "1200");
}

/// **Zero is `0` at every negative scale**, which is how `numeric_out`
/// prints it whatever the typmod, though every other value of the column
/// carries the scale's trailing zeros: it decodes, to `0`, and renders
/// back as `0`. A short digit string that is not zero stays refused, it
/// being no multiple of the scale's power of ten; and `i8::MIN` renders,
/// its magnitude being past `i8`.
#[test]
fn a_negative_scale_zero_is_zero_and_renders_as_written() {
    for scale in i8::MIN..0 {
        let zeros = usize::from(scale.unsigned_abs());
        for text in ["0", "-0", "0.0", "00"] {
            assert_eq!(
                decimal_unscaled_digits(text, scale).as_deref(),
                Some("0"),
                "{text:?} at scale {scale}"
            );
        }
        assert_eq!(render_decimal("0", scale), "0", "at scale {scale}");

        let one = format!("1{}", "0".repeat(zeros));
        assert_eq!(decimal_unscaled_digits(&one, scale).as_deref(), Some("1"), "at {scale}");
        assert_eq!(render_decimal("1", scale), one, "at scale {scale}");
        let short = format!("1{}", "0".repeat(zeros - 1));
        assert_eq!(decimal_unscaled_digits(&short, scale), None, "{short:?} at {scale}");
    }
}

/// **`boolin` is refused where the server refuses it, and what this build
/// reads is what the server reads** (I65): each spelling below was cast
/// on PostgreSQL 16, beside what it answered.
#[test]
fn a_boolean_is_refused_only_where_boolin_refuses_it() {
    let cases: &[(&str, Option<&str>)] = &[
        ("t", Some("true")),
        ("f", Some("false")),
        ("true", Some("true")),
        ("TRUE", Some("true")),
        ("tr", Some("true")),
        ("T", Some("true")),
        ("yes", Some("true")),
        ("y", Some("true")),
        ("YE", Some("true")),
        ("no", Some("false")),
        ("N", Some("false")),
        ("on", Some("true")),
        ("ON", Some("true")),
        ("of", Some("false")),
        ("off", Some("false")),
        ("OFF", Some("false")),
        ("o", None),
        ("O", None),
        ("1", Some("true")),
        ("0", Some("false")),
        ("10", None),
        ("01", None),
        ("", None),
        (" ", None),
        ("  t  ", Some("true")),
        ("\tyes\n", Some("true")),
        ("\u{b}f\u{c}", Some("false")),
        ("truex", None),
        ("tru e", None),
        ("yess", None),
        ("onn", None),
        ("offf", None),
        ("maybe", None),
        ("+1", None),
        ("-0", None),
        (" 1 ", Some("true")),
        ("t\u{a0}", None),
        ("falsE", Some("false")),
        ("fals", Some("false")),
        ("2", None),
    ];
    for (text, server) in cases {
        match decode_bool(text) {
            Some(read) => assert_eq!(Some(read), server.map(|v| v == "true"), "{text:?}"),
            None => {
                let want = if server.is_some() { Unread::Unparsed } else { Unread::Refused };
                assert_eq!(bool_unread(text), want, "{text:?}");
            }
        }
    }
}

/// **`oidin` is read as each major reads it** (I66): each spelling below
/// was cast on PostgreSQL 16, which reads it in `strtoul`'s base 0 under a
/// C23 glibc, and on 15, which reads it in base 10, beside what each
/// answered. Only a text both refuse is refused.
#[test]
fn an_oid_is_read_as_each_major_reads_it() {
    let cases: &[(&str, Option<u32>, Option<u32>)] = &[
        ("0", Some(0), Some(0)),
        ("010", Some(8), Some(10)),
        ("08", None, Some(8)),
        ("0x1F", Some(31), None),
        ("0X1f", Some(31), None),
        ("0x", None, None),
        ("0xg", None, None),
        ("0b101", Some(5), None),
        ("0b2", None, None),
        ("-1", Some(4294967295), Some(4294967295)),
        (" -1 ", Some(4294967295), Some(4294967295)),
        ("+5", Some(5), Some(5)),
        ("+", None, None),
        ("-", None, None),
        ("", None, None),
        ("  ", None, None),
        ("5 ", Some(5), Some(5)),
        (" 5", Some(5), Some(5)),
        ("5x", None, None),
        ("1e3", None, None),
        ("4294967295", Some(4294967295), Some(4294967295)),
        ("4294967296", None, None),
        ("18446744073709551615", Some(4294967295), Some(4294967295)),
        ("18446744073709551616", None, None),
        ("18446744073709551614", Some(4294967294), Some(4294967294)),
        ("18446744071562067968", Some(2147483648), Some(2147483648)),
        ("18446744071562067967", None, None),
        ("-2147483648", Some(2147483648), Some(2147483648)),
        ("-2147483649", None, None),
        ("-4294967295", None, None),
        ("-0", Some(0), Some(0)),
        ("0777", Some(511), Some(777)),
        ("0x100000000", None, None),
        ("0xFFFFFFFF", Some(4294967295), None),
        ("-0x80000000", Some(2147483648), None),
        ("-0x80000001", None, None),
        ("\t7\n", Some(7), Some(7)),
        ("\u{b}7", Some(7), Some(7)),
        ("5\u{c}", Some(5), Some(5)),
        ("00", Some(0), Some(0)),
        ("+0x10", Some(16), None),
        ("0x 1", None, None),
        ("\u{663}", None, None),
    ];
    for &(text, v16, v15) in cases {
        assert_eq!(oid_in(text.as_bytes(), true, true), v16, "{text:?} from v16");
        assert_eq!(oid_in(text.as_bytes(), false, false), v15, "{text:?} before v16");
        let refused = v16.is_none() && v15.is_none();
        assert_eq!(oid_unread(text) == Unread::Refused, refused, "{text:?}");
    }
    // Before C23, glibc reads no `0b`.
    assert_eq!(oid_in(b"0b101", true, false), None);
}

/// **`network_in` is read as the server reads it** (I67): each spelling
/// below was cast on PostgreSQL 16, beside the value it answered, whose
/// address and netmask the port must give.
#[test]
fn an_inet_or_cidr_is_read_as_network_in_reads_it() {
    let inet: &[(&str, Option<&str>)] = &[
        ("10.0.0.1", Some("10.0.0.1/32")),
        ("10", None),
        ("10.1", None),
        ("10.1.2", None),
        ("10.1.2/24", Some("10.1.2.0/24")),
        ("10.1.2.3/24", Some("10.1.2.3/24")),
        ("010.1.2.3", Some("10.1.2.3/32")),
        ("1.2.3.4.", Some("1.2.3.4/32")),
        ("1.2.3.4.5", None),
        ("1.2.3.4/", None),
        ("1.2.3.4/33", None),
        ("1.2.3.4/032", Some("1.2.3.4/32")),
        ("1.2.3.4/4294967304", Some("1.2.3.4/8")),
        ("1.2.3.4/4294967328", Some("1.2.3.4/32")),
        ("1..2.3", None),
        ("256.1.1.1", None),
        ("1.2.3.4 ", None),
        (" 1.2.3.4", None),
        ("::1", Some("::1/128")),
        ("::1/08", None),
        ("::1/128", Some("::1/128")),
        ("::1/129", None),
        ("::1/0", Some("::1/0")),
        ("::1/00", None),
        ("::", Some("::/128")),
        ("1::", Some("1::/128")),
        ("1:2:3:4:5:6:7:8", Some("1:2:3:4:5:6:7:8/128")),
        ("1:2:3:4:5:6:7::", Some("1:2:3:4:5:6:7:0/128")),
        ("::1:2:3:4:5:6:7", Some("0:1:2:3:4:5:6:7/128")),
        ("1:2:3:4:5:6:7:8:9", None),
        ("12345::", None),
        ("::ffff:1.2.3.4", Some("::ffff:1.2.3.4/128")),
        ("::1.2.3", Some("::1.2.3.0/128")),
        ("::1..2.3", Some("::1.0.2.3/128")),
        ("::01.2.3.4", None),
        ("::1.2.3.4/96", Some("::1.2.3.4/96")),
        ("::1.2.3.4/096", None),
        (":1::", None),
        ("1:::2", None),
        ("1::2::3", None),
        ("1:", None),
        ("fe80::1%eth0", None),
        ("::g", None),
        ("0x0a", None),
        ("10/8", Some("10.0.0.0/8")),
        ("10.0.0.0/8", Some("10.0.0.0/8")),
        ("10.0.0.1/8", Some("10.0.0.1/8")),
        ("1.2.3.4/-1", None),
        ("::1.2.3.4.5", None),
        ("1:2:3:4:5:6:1.2.3.4", Some("1:2:3:4:5:6:102:304/128")),
        ("1:2:3:4:5:6:7:1.2.3.4", None),
        ("1.2.3.4/8x", None),
        ("", None),
        ("::/0", Some("::/0")),
        ("::1/1281", None),
        ("::ABCD", Some("::abcd/128")),
    ];
    let cidr: &[(&str, Option<&str>)] = &[
        ("10", Some("10.0.0.0/8")),
        ("10.1", Some("10.1.0.0/16")),
        ("10.1.2", Some("10.1.2.0/24")),
        ("128", Some("128.0.0.0/16")),
        ("192", Some("192.0.0.0/24")),
        ("224", Some("224.0.0.0/4")),
        ("224.1", Some("224.1.0.0/16")),
        ("240", Some("240.0.0.0/32")),
        ("0x0a", Some("10.0.0.0/8")),
        ("0x0A0B", Some("10.11.0.0/16")),
        ("0x0a0b0c0d0e", None),
        ("0xa", Some("160.0.0.0/16")),
        ("10.0.0.0/8", Some("10.0.0.0/8")),
        ("10.0.0.1/8", None),
        ("10.0.0.0/7", Some("10.0.0.0/7")),
        ("10.1.2.0/24", Some("10.1.2.0/24")),
        ("10/8", Some("10.0.0.0/8")),
        ("1.2.3.4/32", Some("1.2.3.4/32")),
        ("1.2.3.4/33", None),
        ("::/0", Some("::/0")),
        ("::1/127", None),
        ("::/1", Some("::/1")),
        ("8000::/1", Some("8000::/1")),
        ("010", Some("10.0.0.0/8")),
        ("10.", None),
        ("1.2.3.4.5", None),
        ("1.2.3.4/4294967304", None),
        ("0x0a/8", Some("10.0.0.0/8")),
        ("0x", None),
        ("0xg", None),
        ("0.0.0.0/0", Some("0.0.0.0/0")),
        ("1.2.3.4/032", Some("1.2.3.4/32")),
        ("::1/08", None),
    ];
    for (cases, is_cidr) in [(inet, false), (cidr, true)] {
        for (text, server) in cases {
            let server = server.map(|value| {
                let (address, bits) = value.split_once('/').unwrap();
                let mut addr = [0u8; 16];
                let v6 = match address.parse::<std::net::IpAddr>().unwrap() {
                    std::net::IpAddr::V4(v4) => {
                        addr[..4].copy_from_slice(&v4.octets());
                        false
                    }
                    std::net::IpAddr::V6(v6) => {
                        addr = v6.octets();
                        true
                    }
                };
                (v6, bits.parse::<u8>().unwrap(), addr)
            });
            assert_eq!(network_in(text, is_cidr), server, "{text:?} as cidr: {is_cidr}");
        }
    }
}

/// **`macaddr_in` is refused where its `sscanf` layouts all refuse, and
/// `macaddr8_in` read as the server reads it** (I68): each spelling below
/// was cast on PostgreSQL 16, beside what it answered. A `macaddr` the
/// server reads is never refused, the ones the model leaves to glibc
/// among them — a sign, a `0x`, a run past eight digits — and nor is one
/// it refuses there.
#[test]
fn a_macaddr_is_refused_only_where_macaddr_in_refuses_it() {
    let macaddr: &[(&str, Option<&str>)] = &[
        ("08:00:2b:01:02:03", Some("08:00:2b:01:02:03")),
        ("08-00-2b-01-02-03", Some("08:00:2b:01:02:03")),
        ("08002b:010203", Some("08:00:2b:01:02:03")),
        ("08002b-010203", Some("08:00:2b:01:02:03")),
        ("0800.2b01.0203", Some("08:00:2b:01:02:03")),
        ("0800-2b01-0203", Some("08:00:2b:01:02:03")),
        ("08002b010203", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02:3", Some("08:00:2b:01:02:03")),
        ("8:0:2b:1:2:3", Some("08:00:2b:01:02:03")),
        (" 08:00:2b:01:02:03 ", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02:03x", None),
        ("08:00:2b:01:02", None),
        ("08:00:2b:01:02:03:04", None),
        ("08:00:2b:01:02:100", None),
        ("08:00:2b:01:02:0100", None),
        ("08:00:2b:01:02:ffffffff", None),
        ("08:00:2b:01:02:100000000ff", Some("08:00:2b:01:02:ff")),
        ("08:00:2b:01:02:-1", None),
        ("08:00:2b:01:02:+3", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02:-0", Some("08:00:2b:01:02:00")),
        ("08:00:2b:01:02:0x3", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02: 3", Some("08:00:2b:01:02:03")),
        ("08 :00:2b:01:02:03", None),
        ("zz:00:2b:01:02:03", None),
        ("08:00:2B:01:02:03", Some("08:00:2b:01:02:03")),
        ("08002b01020", Some("08:00:2b:01:02:00")),
        ("08002b0102030", None),
        ("08002b01020304", None),
        ("0x0800.2b01.0203", None),
        ("08:00:2b:01:02:03 x", None),
        ("", None),
        ("08:00-2b:01:02:03", None),
        ("0800.2b01.020", Some("08:00:2b:01:02:00")),
        ("08002b:01020", Some("08:00:2b:01:02:00")),
    ];
    // Refused by the server, through a conversion the model leaves to glibc.
    let unmodelled = ["08:00:2b:01:02:-1", "0x0800.2b01.0203"];
    for (text, server) in macaddr {
        let want = if server.is_some() || unmodelled.contains(text) {
            Unread::Unparsed
        } else {
            Unread::Refused
        };
        assert_eq!(macaddr_unread(text), want, "{text:?}");
    }
    let macaddr8: &[(&str, Option<&str>)] = &[
        ("08:00:2b:01:02:03:04:05", Some("08:00:2b:01:02:03:04:05")),
        ("08:00:2b:01:02:03", Some("08:00:2b:ff:fe:01:02:03")),
        ("08-00-2b-01-02-03-04-05", Some("08:00:2b:01:02:03:04:05")),
        ("08002b0102030405", Some("08:00:2b:01:02:03:04:05")),
        ("08002b010203", Some("08:00:2b:ff:fe:01:02:03")),
        ("0800.2b01.0203.0405", Some("08:00:2b:01:02:03:04:05")),
        ("08:00:2b:01:02:03:04", None),
        ("08:00:2b:01:02:03:04:05:06", None),
        ("08002b0102031", Some("08:00:2b:ff:fe:01:02:03")),
        ("08002b010203040", None),
        ("08002b01020304051", Some("08:00:2b:01:02:03:04:05")),
        (" 08:00:2b:01:02:03 ", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03  ", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03 x", None),
        ("08:00:2b:01:02:03:", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03:04:05:", Some("08:00:2b:01:02:03:04:05")),
        ("08:00-2b:01:02:03", None),
        ("08::00:2b:01:02:03", None),
        ("8:00:2b:01:02:03", None),
        ("08:00:2b:01:02:03:04:05 ", Some("08:00:2b:01:02:03:04:05")),
        ("zz:00:2b:01:02:03", None),
        ("", None),
        ("08:00:2b:01:02:03\t", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03:04 05", None),
        ("08:00:2b:01:02 03", None),
    ];
    for (text, server) in macaddr8 {
        let server = server.map(|value| {
            let mut bytes = [0u8; 8];
            for (byte, pair) in bytes.iter_mut().zip(value.split(':')) {
                *byte = u8::from_str_radix(pair, 16).unwrap();
            }
            bytes
        });
        assert_eq!(macaddr8_in(text), server, "{text:?}");
    }
}

/// **`byteain` is refused where the server refuses it, and what this
/// build reads is what the server reads** (I69): each spelling below was
/// cast on PostgreSQL 16, beside the value it answered.
#[test]
fn a_bytea_is_refused_only_where_byteain_refuses_it() {
    let cases: &[(&str, Option<&str>)] = &[
        ("\\x", Some("\\x")),
        ("\\x00", Some("\\x00")),
        ("\\xAB", Some("\\xab")),
        ("\\x a b", None),
        ("\\x0", None),
        ("\\xab c", None),
        ("\\x ab \n\tcd\r", Some("\\xabcd")),
        ("\\xgg", None),
        ("\\x\u{e9}", None),
        ("\\X41", None),
        ("abc", Some("\\x616263")),
        ("a\\\\b", Some("\\x615c62")),
        ("\\000", Some("\\x00")),
        ("\\377", Some("\\xff")),
        ("\\400", None),
        ("\\08", None),
        ("\\1", None),
        ("\\x", Some("\\x")),
        ("\\", None),
        ("a\\", None),
        ("\\101", Some("\\x41")),
        ("\u{e9}", Some("\\xc3a9")),
        ("tab\there", Some("\\x7461620968657265")),
        ("\\\\x41", Some("\\x5c783431")),
        ("\\xab\u{b}", None),
        ("\\x ab\u{c}", None),
    ];
    for (text, server) in cases {
        match server {
            None => {
                assert_eq!(decode_bytea(text), None, "{text:?}");
                assert_eq!(bytea_unread(text), Unread::Refused, "{text:?}");
            }
            Some(value) => {
                let want = decode_bytea(value).unwrap();
                match decode_bytea(text) {
                    Some(read) => assert_eq!(read, want, "{text:?}"),
                    None => assert_eq!(bytea_unread(text), Unread::Unparsed, "{text:?}"),
                }
            }
        }
    }
}

/// **`json_in` is read as the server reads it** (I72): each text below
/// was put to `pg_input_is_valid(…, 'json')` on PostgreSQL 16 and its
/// answer is beside it. The first five `jsonb_in` refuses.
#[test]
fn a_json_is_refused_only_where_json_in_refuses_it() {
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let cases: &[(&str, bool)] = &[
        (r#""\u0000""#, true),
        (r#""\ud800""#, true),
        (r#""\udc00""#, true),
        (r#""\ud800A""#, true),
        ("1e100000000", true),
        (&deep, true),
        ("-0", true),
        (r#"{"a":1,"a":2}"#, true),
        ("\"a\u{7f}b\"", true),
        ("[1] ", true),
        (" \t\n\r{}\r\n", true),
        ("\"\u{e9}\"", true),
        (r#""é""#, true),
        ("null", true),
        ("[true,false,null]", true),
        ("1E+2", true),
        ("-1.5e-3", true),
        (r#""\/""#, true),
        (r#"{"a": [1, {"b": {}}], "c": []}"#, true),
        ("[1,]", false),
        ("01", false),
        ("-01", false),
        ("", false),
        ("  ", false),
        (r#""\x41""#, false),
        ("\u{b}1", false),
        ("truex", false),
        ("tru", false),
        ("1x", false),
        ("1\u{e9}", false),
        ("\u{e9}", false),
        ("-", false),
        ("- 1", false),
        ("\"abc", false),
        ("1.", false),
        (".5", false),
        ("+1", false),
        ("NaN", false),
        ("1e", false),
        ("1e+", false),
        ("1_0", false),
        ("\"a\tb\"", false),
        ("1 2", false),
        ("{a:1}", false),
        (r#""\U0041""#, false),
        (r#""\u004""#, false),
        (r#"{"a" 1}"#, false),
        ("{,}", false),
        ("[,1]", false),
        (r#"{"a":1,}"#, false),
        ("[1]]", false),
        (r#"{"a":[}"#, false),
        (r#"{"a":1]"#, false),
        ("[1}", false),
    ];
    for (text, server) in cases {
        assert_eq!(json_in(text), *server, "{text:?}");
    }
}

/// **A bit string is refused exactly where `bit_in` or `varbit_in`
/// refuses it** (I73): every case below was put to `pg_input_is_valid`
/// under the type beside it on a PostgreSQL 16 server, `bit` being
/// `bit(1)` and `"bit"` the type with no length.
#[test]
fn a_bit_string_is_refused_only_where_bit_in_refuses_it() {
    // `(text, varying, length, the server reads it)`.
    let cases: &[(&str, bool, Option<u32>, bool)] = &[
        ("101", false, Some(3), true),
        ("10", false, Some(3), false),
        ("1010", false, Some(3), false),
        ("B101", false, Some(3), true),
        ("b101", false, Some(3), true),
        ("x5", false, Some(3), false),
        ("x5", false, Some(4), true),
        ("XA", false, Some(4), true),
        ("xg", false, Some(4), false),
        ("102", false, Some(3), false),
        (" 101", false, Some(3), false),
        ("101 ", false, Some(3), false),
        ("", false, Some(3), false),
        ("1", false, Some(1), true),
        ("11", false, Some(1), false),
        ("11", false, None, true),
        ("", false, None, true),
        ("b", false, None, true),
        ("B", false, None, true),
        ("x", false, None, true),
        ("xFfA0", false, None, true),
        ("xFfA0g", false, None, false),
        ("2", false, None, false),
        ("bb1", false, None, false),
        ("bx1", false, None, false),
        ("xx1", false, None, false),
        ("\u{e9}", false, None, false),
        ("101", true, None, true),
        ("", true, None, true),
        ("x", true, None, true),
        ("xab", true, None, true),
        ("1_0", true, None, false),
        ("+1", true, None, false),
        ("0x1", true, None, false),
        ("b 1", true, None, false),
        ("101", true, Some(3), true),
        ("10", true, Some(3), true),
        ("", true, Some(3), true),
        ("1010", true, Some(3), false),
        ("xA", true, Some(4), true),
        ("xAB", true, Some(4), false),
        ("xA", true, Some(3), false),
        ("b11", true, Some(2), true),
        ("b111", true, Some(2), false),
    ];
    for &(text, varying, length, server) in cases {
        assert_eq!(bit_in(text, varying, length), server, "{text:?} {varying} {length:?}");
    }
}

/// **A `character varying(n)` or `character(n)` field is refused exactly
/// where `varchar_input` or `bpchar_input` refuses it** (I75): past `n`
/// characters, counted in UTF-8, with anything but `0x20` past them —
/// every case put to `pg_input_is_valid` under `character varying(3)` and
/// `character(3)`, which agree on each, on a PostgreSQL 16 server.
#[test]
fn a_character_field_is_refused_only_past_its_length_but_for_blanks() {
    // `(text, the server reads it)`, at a length of 3.
    let cases: &[(&str, bool)] = &[
        ("abc", true),
        ("a", true),
        ("", true),
        ("abcd", false),
        ("abc   ", true),
        ("ab    ", true),
        ("abc\t", false),
        ("abc\n", false),
        (" abc", false),
        ("\u{e9}\u{e9}\u{e9}", true),
        ("\u{e9}\u{e9}\u{e9}  ", true),
        ("\u{e9}\u{e9}\u{e9}\u{e9}", false),
        ("\u{e9}\u{e9}\u{e9}x", false),
        ("\u{e9}\u{e9}\u{e9}\u{e9} ", false),
    ];
    for &(text, server) in cases {
        assert_eq!(!char_typmod_refuses(text, 3), server, "{text:?}");
    }
    assert!(char_typmod_refuses("ab", 1) && !char_typmod_refuses("a ", 1));
}

/// **A geometric field is refused exactly where its input function
/// refuses it at every supported major** (I74): every case below was cast
/// to its type on PostgreSQL 13.23, 16.15 and 18.6, and is read here where
/// any of the three reads it. They disagree on two lines alone, built
/// from two points by arithmetic v14 changed: v13 reads the one whose
/// slope is `DBL_MAX` and refuses the one whose run is infinite.
#[test]
fn a_geometric_field_is_refused_only_where_every_major_refuses_it() {
    // `(input function, text, some major reads it)`.
    type Case = (fn(&str) -> bool, &'static str, bool);
    let cases: &[Case] = &[
        (point_in, "(1,2)", true),
        (point_in, "1,2", true),
        (point_in, " ( 1 , 2 ) ", true),
        (point_in, "(1,2", false),
        (point_in, "1,2)", false),
        (point_in, "(1 2)", false),
        (point_in, "(1,2,3)", false),
        (point_in, "", false),
        (point_in, "(,2)", false),
        (point_in, "(1e400,2)", false),
        (point_in, "(1e-400,0)", false),
        (point_in, "(0e-400,0)", true),
        (point_in, "(1e-310,0)", true),
        (point_in, "(NaN,inf)", true),
        (point_in, "(-Infinity,+inf)", true),
        (point_in, "(nan(abc),1)", true),
        (point_in, "(nan(),1)", true),
        (point_in, "(nan(a-b),1)", false),
        (point_in, "(-nan,1)", true),
        (point_in, "(INF,infinity)", true),
        (point_in, "(infinit,1)", false),
        (point_in, "(0x1p3,1)", true),
        (point_in, "(0x1.8p1,0X.8)", true),
        (point_in, "(0x,1)", false),
        (point_in, "(0x.p1,1)", false),
        (point_in, "(0x1p,0)", false),
        (point_in, "(0x1p+,0)", false),
        (point_in, "(0x1p99999,1)", false),
        (point_in, "(0x1p1024,0)", false),
        (point_in, "(0x1.fffffffffffff8p1023,0)", false),
        (point_in, "(0x1.fffffffffffff7p1023,0)", true),
        (point_in, "(0x1p-1074,1)", true),
        (point_in, "(-0x1p-1074,0)", true),
        (point_in, "(0x1p-1075,1)", false),
        (point_in, "(0x1.00000000000000000001p-1075,0)", true),
        (point_in, "(0x0p-99999,0)", true),
        (point_in, "(0x0.0000000000000000000000000000001p+99999,0)", false),
        (point_in, "(1.,.5)", true),
        (point_in, "(.,1)", false),
        (point_in, "(1e,2)", false),
        (point_in, "(1e+,2)", false),
        (point_in, "(1.e5,2E-3)", true),
        (point_in, "(1,2) x", false),
        (point_in, "((1,2))", false),
        (point_in, "(1_0,2)", false),
        (point_in, "(١,2)", false),
        (line_in, "{1,2,3}", true),
        (line_in, " {1,2,3} ", true),
        (line_in, "{0,0,1}", false),
        (line_in, "{0,1e-7,1}", false),
        (line_in, "{1e-6,0,1}", false),
        (line_in, "{2e-6,0,1}", true),
        (line_in, "{1,2,3", false),
        (line_in, "{1,2}", false),
        (line_in, "{1,2,3,4}", false),
        (line_in, "{1 ,2, 3 }", true),
        (line_in, "{NaN,0,1}", true),
        (line_in, "{1,1,1e400}", false),
        (line_in, "[(0,0),(1,1)]", true),
        (line_in, "(0,0),(1,1)", true),
        (line_in, "((0,0),(1,1))", true),
        (line_in, "0,0,1,1", true),
        (line_in, "[(0,0),(0,0)]", false),
        (line_in, "[(0,0),(0.0000001,0)]", false),
        (line_in, "[(1,1),(1,2)]", true),
        (line_in, "[(1,1),(2,1)]", true),
        (line_in, "[(2,0),(3,1.7976931348623157e308)]", true),
        (line_in, "[(Infinity,1),(0,2)]", true),
        (line_in, "[(1e300,1e300),(-1e300,-1e300)]", true),
        (line_in, "[(1e308,0),(-1e308,1)]", false),
        (line_in, "[(1e-200,0),(1e308,1)]", false),
        (line_in, "[(100000,0),(100000.001,1e305)]", false),
        (line_in, "[(NaN,0),(NaN,0)]", false),
        (line_in, "[(NaN,0),(1,1)]", true),
        (line_in, "[(1,2),(3,4)", false),
        (line_in, "[(1,2),(3,4),(5,6)]", false),
        (lseg_in, "[(1,2),(3,4)]", true),
        (lseg_in, "((1,2),(3,4))", true),
        (lseg_in, "(1,2),(3,4)", true),
        (lseg_in, "1,2,3,4", true),
        (lseg_in, "(1,2,3,4)", true),
        (lseg_in, "[(1,2),(3,4))", true),
        (lseg_in, "((1,2),(3,4)]", false),
        (lseg_in, "[(1,2),(3,4)", false),
        (lseg_in, "(1,2),(3,4),(5,6)", false),
        (lseg_in, "[(1,2)]", false),
        (lseg_in, "[(1,1),(1,1)]", true),
        (lseg_in, " [ ( 1 , 2 ) , ( 3 , 4 ) ] ", true),
        (box_in, "(1,2),(3,4)", true),
        (box_in, "((1,2),(3,4))", true),
        (box_in, "1,2,3,4", true),
        (box_in, "(1,2,3,4)", true),
        (box_in, "[(1,2),(3,4)]", false),
        (box_in, "(1,2)", false),
        (box_in, "((1,2),(3,4)", false),
        (box_in, "(nan,1),(2,3)", true),
        (box_in, " ( 1 , 2 ) , ( 3 , 4 ) ", true),
        (box_in, "(1,2) (3,4)", true),
        (box_in, "(1,2),(3,4),", true),
        (path_in, "[(1,2),(3,4)]", true),
        (path_in, "((1,2),(3,4))", true),
        (path_in, "(1,2),(3,4)", true),
        (path_in, "[(1,2)]", true),
        (path_in, "(1,2)", true),
        (path_in, "1,2", true),
        (path_in, "[1,2,3,4]", true),
        (path_in, "[(1,2),(3,4)", false),
        (path_in, "", false),
        (path_in, "(1,2),(3,4),", false),
        (path_in, "[(1,2),(3,4),(5,6)]", true),
        (path_in, "((1,2),(3,4)]", false),
        (path_in, "[(1,2),(3,4))", true),
        (path_in, "( (1,2),(3,4) )", true),
        (path_in, "[1,2,3]", false),
        (path_in, "((1,2),(3,4)) )", false),
        (path_in, "(1,2),(3,4) x", false),
        (poly_in, "((1,2),(3,4),(5,6))", true),
        (poly_in, "(1,2),(3,4)", true),
        (poly_in, "1,2,3,4,5,6", true),
        (poly_in, "[(1,2),(3,4)]", false),
        (poly_in, "(1,2)", true),
        (poly_in, "", false),
        (poly_in, "(1,2),(3,4", false),
        (poly_in, "((1,2),(3,4)", false),
        (poly_in, "((1,2),(3,4),(5,6)) ", true),
        (poly_in, "(1,2),(3,4),", false),
        (circle_in, "<(1,2),3>", true),
        (circle_in, "((1,2),3)", true),
        (circle_in, "(1,2),3", true),
        (circle_in, "1,2,3", true),
        (circle_in, "(1,2,3)", false),
        (circle_in, "<(1,2),-1>", false),
        (circle_in, "<(1,2),NaN>", true),
        (circle_in, "<(1,2),-0>", true),
        (circle_in, "<(1,2),3", false),
        (circle_in, "<(1,2),3>>", false),
        (circle_in, "<(1,2) 3>", true),
        (circle_in, "<(1,2),3)", true),
        (circle_in, "((1,2),3>", true),
        (circle_in, "<1,2,3>", true),
        (circle_in, "<(1,2),-inf>", false),
        (circle_in, "<(1,2),1e400>", false),
        (circle_in, "(1,2)3", true),
        (circle_in, " < ( 1 , 2 ) , 3 > ", true),
    ];
    for &(read, text, server) in cases {
        assert_eq!(read(text), server, "{text:?}");
    }
}

/// **A number in a geometric field is read as glibc's `strtod` reads
/// it**, a hexadecimal one rounded to nearest, ties to even, at each edge
/// of `double precision`'s range: the value is what decides a `line`'s
/// coefficients and a `circle`'s radius.
#[test]
fn a_geometric_number_is_read_as_strtod_reads_it() {
    let read = |text: &str| strtod(text.as_bytes(), 0);
    for (text, value, end) in [
        ("0x1p3", 8.0, 5),
        ("-0X.8", -0.5, 5),
        ("0x1.8p1x", 3.0, 7),
        ("0x", 0.0, 1),
        ("0x1p", 1.0, 3),
        ("0x1p-1074", f64::from_bits(1), 9),
        ("0x1.00000000000000000001p-1075", f64::from_bits(1), 30),
        ("0x1.8p-1074", f64::from_bits(2), 11),
        ("0x1.fffffffffffff7p1023", f64::MAX, 23),
        ("0x0.fffffffffffff8p-1022", f64::MIN_POSITIVE, 24),
        ("0x1.0000000000000800p0", 1.0, 22),
        ("0x1.0000000000000801p0", 1.0 + f64::EPSILON, 22),
        ("0x1.0000000000001800p0", 1.0 + 2.0 * f64::EPSILON, 22),
        ("1.e5,", 1e5, 4),
        ("1e+,", 1.0, 1),
        ("infinit", f64::INFINITY, 3),
        ("-Infinity)", f64::NEG_INFINITY, 9),
        ("1e-310", 1e-310, 6),
    ] {
        assert_eq!(read(text), Some((value, end, false)), "{text}");
    }
    for text in ["0x1p-1075", "0x1.fffffffffffff8p1023", "0x1p1024", "1e400", "-1e-400"] {
        assert!(read(text).is_some_and(|(_, _, out_of_range)| out_of_range), "{text}");
    }
    assert!(read("nan(a_1)x").is_some_and(|(v, end, _)| v.is_nan() && end == 8));
    assert!(read("nan(a-1)").is_some_and(|(v, end, _)| v.is_nan() && end == 3));
    for text in ["", ".", "+", "-.e1", "x1", "_1"] {
        assert_eq!(read(text), None, "{text:?}");
    }
    assert!(path_in(&format!("[{}1,1]", "1,1,".repeat(16))));
    assert!(fits_allocation(PATH_HEADER, (MAX_ALLOC_SIZE - PATH_HEADER) / 16));
    assert!(!fits_allocation(PATH_HEADER, (MAX_ALLOC_SIZE - PATH_HEADER) / 16 + 1));
    assert!(!fits_allocation(POLYGON_HEADER, (MAX_ALLOC_SIZE - POLYGON_HEADER) / 16 + 1));
}
