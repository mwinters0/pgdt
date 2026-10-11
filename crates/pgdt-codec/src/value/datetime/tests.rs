use super::*;

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
