//! `date`, `time`, `timestamp` and `interval`: their readers and renderers,
//! the PostgreSQL-epoch forms the server stores a `time` and a timestamp in
//! and its comparison orders — where the readers `decode` declares yield what
//! Arrow holds — and the hand-off of what they read no value from to the
//! input functions' [`grammar`].

pub(crate) mod grammar;

use self::grammar::{DateTimeInput, datetime_reads, interval_reads};
use crate::declared::IntervalQualifier;
use crate::decode::{Read, Unread};
use crate::value::digits::{push_fraction, push_two, push_year};

/// A `time`'s microseconds since midnight as PostgreSQL holds it, its inclusive
/// upper bound `24:00:00` included — what its comparison orders (`crate::key`),
/// where [`decode_time64_micros`] is what Arrow
/// holds.
pub fn time_of_day_micros(s: &str) -> Read<i64> {
    let (seconds, micros) = parse_time_of_day(s)?;
    Ok(seconds * 1_000_000 + micros)
}

/// A finite timestamp's microseconds from PostgreSQL's epoch, 2000-01-01 UTC —
/// the `int64` PostgreSQL stores it as (I49), so every value it admits fits,
/// the three decades
/// [`decode_timestamp_micros`] cannot count
/// from 1970 included. What its comparison orders (`crate::key`), where
/// [`decode_timestamp_micros`] is what Arrow
/// holds; [`Unread::Unparsed`] for the infinities, and
/// an [`Unread`] for text that is not a timestamp.
pub fn timestamp_postgres_micros(s: &str, with_tz: bool) -> Read<i64> {
    let postgres = timestamp_micros_wide(s, with_tz)? - POSTGRES_EPOCH_UNIX_MICROS;
    Ok(i64::try_from(postgres).expect("PostgreSQL's range is an i64 of microseconds"))
}

/// [`render_timestamp_micros`] of a
/// [`timestamp_postgres_micros`] value, so a timestamp past what `i64` counts
/// from 1970 is written as the dump writes it.
pub fn render_timestamp_postgres_micros(v: i64, with_tz: bool) -> String {
    let unix = i128::from(v) + POSTGRES_EPOCH_UNIX_MICROS;
    let days = i64::try_from(unix.div_euclid(86_400_000_000)).expect("an i64 of micros is days");
    let of_day = i64::try_from(unix.rem_euclid(86_400_000_000)).expect("under a day");
    let mut out = String::with_capacity(32);
    push_timestamp(days, of_day, with_tz, &mut out);
    out
}

/// Days from a proleptic-Gregorian civil date to the 1970-01-01 epoch.
/// Howard Hinnant's public-domain `days_from_civil` algorithm — correct for
/// negative (BCE, astronomical numbering) years, which is why it's used here
/// rather than any calendar library assuming a year > 0.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(m) + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

/// The `YYYY-MM-DD` head a `date` and a `timestamp` share, out of a day count.
/// Returns whether the value is before the common era: PostgreSQL writes the
/// `" BC"` marker at the very end, past the time and the zone, so the caller
/// appends it rather than this.
fn push_civil_date(out: &mut String, days: i64) -> bool {
    let (y, m, d) = civil_from_days(days);
    let (out_year, bc) = if y <= 0 { (1 - y, true) } else { (y, false) };
    push_year(out, out_year);
    out.push('-');
    push_two(out, i64::from(m));
    out.push('-');
    push_two(out, i64::from(d));
    bc
}

/// Strip a trailing `" BC"` era marker, PostgreSQL's spelling for a
/// before-common-era date/timestamp. Returns the remaining text and whether
/// the marker was present.
fn split_era(s: &str) -> (&str, bool) {
    match s.strip_suffix(" BC") {
        Some(rest) => (rest, true),
        None => (s, false),
    }
}

/// A date or time part as `strtoint` reads it once `ParseDateTime` has split
/// the text at its signs: ASCII digits only, at least one. A sign is where the
/// server starts a zone (`12:-5:00` is 12:00 at zone `-5`, `+2020-01-01`
/// refused), so a part carrying one is not read here, rather than read as a
/// signed number — a shortfall where the server reads it
/// (`docs/design/decisions.md`, "D55"). Past `i64` is `None` too.
fn unsigned_part(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    s.bytes().try_fold(0i64, |value, b| {
        if !b.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(i64::from(b - b'0'))
    })
}

/// `JULIAN_MAXYEAR`, the year `IS_VALID_JULIAN` refuses from (its June, which
/// the day ranges below lie well short of): every year a date or timestamp
/// holds is below it, and a year bounded by it cannot overflow
/// [`days_from_civil`].
const JULIAN_MAX_YEAR: i64 = 5_874_898;

/// The first day a `date` holds, `4714-11-24 BC` — Julian day 0,
/// `DATETIME_MIN_JULIAN` — in days from 1970.
const DATE_MIN_DAYS: i64 = -2_440_588;

/// The first day past a `date`'s range, `5874898-01-01` — `DATE_END_JULIAN`
/// — in days from 1970.
const DATE_END_DAYS: i64 = 2_145_042_906;

/// The days in month `m` (1–12) of astronomical year `y`, by `day_tab` and
/// `isleap` over the proleptic Gregorian calendar, so 1 BC (year 0) is leap.
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `YYYY-MM-DD`, its era already split off (`bc`), as days from 1970 —
/// [`Unread::Refused`] for a day the calendar does not hold, as `ValidateDate` refuses it.
///
/// The year is three digits or more, as `pg_dump` writes four: a shorter one
/// is read by `DateOrder` (`20-01-01` is a month 20 under `MDY`, the year 2020
/// under `YMD`), so it is [`Unread::Unparsed`], as is a month of three
/// digits, which the server reads as a day of the year (D55).
fn civil_days(s: &str, bc: bool) -> Read<i64> {
    let unparsed = Unread::Unparsed;
    let (year, rest) = s.split_once('-').ok_or(unparsed)?;
    let (month, day) = rest.split_once('-').ok_or(unparsed)?;
    if year.len() < 3 || month.len() > 2 {
        return Err(unparsed);
    }
    let part = |s| unsigned_part(s).ok_or(unparsed);
    let (y, m, d) = (part(year)?, part(month)?, part(day)?);
    // pg-refuses: I61 — no year zero either side of the era, a month past
    // twelve, a day past its month's, a year past `IS_VALID_JULIAN`'s.
    if y == 0 || y > JULIAN_MAX_YEAR || !(1..=12).contains(&m) {
        return Err(Unread::Refused);
    }
    let y = astronomical_year(y, bc);
    if !(1..=days_in_month(y, m)).contains(&d) {
        return Err(Unread::Refused);
    }
    Ok(days_from_civil(y, m as u32, d as u32))
}

/// `1 - year` turns a `" BC"`-suffixed calendar year into PostgreSQL's (and
/// the proleptic Gregorian calendar's) astronomical year numbering: 1 BC is
/// astronomical year 0, 44 BC is -43, and so on.
fn astronomical_year(y: i64, bc: bool) -> i64 {
    if bc { 1 - y } else { y }
}

/// `None` for `infinity`/`-infinity` (PostgreSQL's own pseudo-values for
/// "unbounded" — real, but `Date32` has no sentinel for them, so this is a
/// decode failure by construction, not by accident), and for a day outside
/// PostgreSQL's range, which `Date32` holds whole.
///
/// **The anchor for every one of these refusals** ([`decode_interval`]'s
/// infinities and overflowing time part, `NaN` on a `Decimal128`, a
/// timestamp past `i64` microseconds from 1970, which PostgreSQL's range
/// outlasts by three decades, and [`decode_time64_micros`]'s `24:00:00`): a
/// typed column cannot hold the value, nor DataFusion display a `date`, or a
/// timestamp short of `i64`'s end, past `262142-12-31`, which decodes and
/// which `arrow-cast` formats through a calendar ending there. No query asks
/// a decoder for one: the null mode reads it as NULL first
/// (`docs/design/decisions.md`, "D98"), the untyped mode reads its column as
/// text ("D100"), and the refuse mode refuses at planning a query
/// materializing its column ("D99").
pub fn decode_date32(s: &str) -> Option<i32> {
    date_days(s).ok()
}

/// [`decode_date32`] telling a day PostgreSQL refuses from text that is no
/// day here. The infinities are [`Unread::Unparsed`]: values of the type, but
/// no day, which a caller ranks first if it can (`crate::key`).
pub(crate) fn date_days(s: &str) -> Read<i32> {
    if s == "infinity" || s == "-infinity" {
        return Err(Unread::Unparsed);
    }
    let (rest, bc) = split_era(s);
    let days = civil_days(rest, bc)?;
    // pg-refuses: I61 — `IS_VALID_DATE`'s range.
    if !(DATE_MIN_DAYS..DATE_END_DAYS).contains(&days) {
        return Err(Unread::Refused);
    }
    Ok(i32::try_from(days).expect("a date's range is an i32 of days"))
}

pub fn render_date32(days: i32) -> String {
    // Room for every `Date32` PostgreSQL writes: a seven-digit year, `-MM-DD`
    // and ` BC`.
    let mut out = String::with_capacity(16);
    render_date32_into(days, &mut out);
    out
}

/// [`render_date32`] appending to a caller's buffer instead of returning one.
/// The `_into` form is the one that does the work; the owned form above is a
/// wrapper, so the two cannot drift (`docs/design/decisions.md`, "D44").
pub fn render_date32_into(days: i32, out: &mut String) {
    if push_civil_date(out, i64::from(days)) {
        out.push_str(" BC");
    }
}

/// Powers of ten up to a microsecond, indexed by how many fractional digits
/// a time-of-day literal is short of six.
const POW10: [i64; 7] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000];

/// `HH:MM:SS[.ffffff]`, no offset. Returns whole seconds since midnight and
/// the microsecond remainder separately, since callers need both a plain
/// `Time64` value (`seconds*1_000_000 + micros`) and, for a timestamp, the
/// same pair added onto a day count. `crate::key` reads it too, for the
/// time half of a `time with time zone` comparison — that type has no Arrow
/// mapping of its own, so its *ordering* is the only path that decodes it.
///
/// [`Unread::Refused`] past `time_overflows`' bounds, which `time`, `timetz` and both
/// timestamps share: `24:00:00` is the last time of day, and `23:59:60` and
/// `24:00:00` are both read, a timestamp's carrying into the next day.
pub(crate) fn parse_time_of_day(s: &str) -> Read<(i64, i64)> {
    let unparsed = Unread::Unparsed;
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut parts = hms.splitn(3, ':');
    let mut part = || parts.next().and_then(unsigned_part).ok_or(unparsed);
    let (h, mi, se) = (part()?, part()?, part()?);
    if parts.next().is_some() || frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(unparsed);
    }
    // pg-refuses: I61 — `time_overflows`, part by part.
    if h > 24 || mi > 59 || se > 60 {
        return Err(Unread::Refused);
    }
    // `frac` is checked above to be at most six ASCII digits, so its digits
    // scaled by a power of ten are what padding it to six and parsing would
    // read, with no allocation: `.5` is 500000 µs, `.000001` is 1.
    let mut micros: i64 = 0;
    for b in frac.bytes() {
        micros = micros * 10 + i64::from(b - b'0');
    }
    let (seconds, micros) = (h * 3600 + mi * 60 + se, micros * POW10[6 - frac.len()]);
    // pg-refuses: I61 — and past `24:00:00` as a whole.
    if seconds * 1_000_000 + micros > DAY_MICROS {
        return Err(Unread::Refused);
    }
    Ok((seconds, micros))
}

/// The exact inverse of [`parse_time_of_day`]'s micros-since-midnight value —
/// shared by [`render_time64_micros`] and [`render_timestamp_micros`]'s
/// time-of-day component. PostgreSQL trims trailing zeros from the fraction
/// (`00:00:00.5`, not `.500000`) and omits it entirely when zero.
fn format_hms_frac_into(out: &mut String, total_micros: i64) {
    let seconds = total_micros.div_euclid(1_000_000);
    let micros = total_micros.rem_euclid(1_000_000);
    push_two(out, seconds / 3600);
    out.push(':');
    push_two(out, (seconds % 3600) / 60);
    out.push(':');
    push_two(out, seconds % 60);
    if micros != 0 {
        out.push('.');
        push_fraction(out, micros);
    }
}

/// Arrow's `Time64` day, in microseconds: the first value past it.
const DAY_MICROS: i64 = 86_400_000_000;

/// `None` for anything unparseable, and for `24:00:00`: a real, valid
/// boundary value (PostgreSQL's inclusive upper bound for `time`) that
/// Arrow's `Time64`, holding `[0, 86400 s)`, cannot, so it is refused as
/// every other unrepresentable value is (the anchor on [`decode_date32`]).
pub fn decode_time64_micros(s: &str) -> Option<i64> {
    time_of_day_micros(s).ok().filter(|&micros| micros < DAY_MICROS)
}

pub fn render_time64_micros(v: i64) -> String {
    // `HH:MM:SS.ffffff`.
    let mut out = String::with_capacity(16);
    render_time64_micros_into(v, &mut out);
    out
}

/// [`render_time64_micros`] appending to a caller's buffer.
pub fn render_time64_micros_into(v: i64, out: &mut String) {
    format_hms_frac_into(out, v);
}

/// Pull a `+HH[:MM[:SS]]` / `-HH[:MM[:SS]]` UTC offset off the end of a
/// timestamptz's time-of-day text, returning what's left and the offset in
/// seconds (positive east of UTC, matching the sign convention `local - offset
/// = UTC`). The offset is the only place a `+`/`-` can occur in this
/// substring — the fractional-seconds part, if any, is digits and a `.` only.
///
/// `timetz_out` writes the same `EncodeTimezone` form (I40), so
/// `crate::key` splits a `time with time zone` with this. What it does
/// with the two halves is not what a timestamp does: the offset is kept, not
/// discarded, because two `timetz` values are equal only when the zone
/// matches as well as the instant.
///
/// Refused past `DecodeTimezone`'s `±15:59:59`, part by part. The
/// run-together `+0530`, which the server reads as `+05:30`, is unparsed, an
/// hour of more than two digits being a shortfall rather than an hour past
/// fifteen (`docs/design/decisions.md`, "D55").
pub(crate) fn extract_offset(s: &str) -> Read<(&str, i64)> {
    let unparsed = Unread::Unparsed;
    let idx = s.find(['+', '-']).ok_or(unparsed)?;
    let (time_only, off) = s.split_at(idx);
    let (sign, rest) = off.split_at(1);
    let sign_mult: i64 = if sign == "-" { -1 } else { 1 };
    let mut parts = rest.splitn(3, ':');
    let hours = parts.next().ok_or(unparsed)?;
    if hours.len() > 2 {
        return Err(unparsed);
    }
    let hh = unsigned_part(hours).ok_or(unparsed)?;
    let mut part = || parts.next().map_or(Some(0), unsigned_part).ok_or(unparsed);
    let (mm, ss) = (part()?, part()?);
    // pg-refuses: I61 — `MAX_TZDISP_HOUR`, and a minute or second past 59.
    if hh > 15 || mm > 59 || ss > 59 {
        return Err(Unread::Refused);
    }
    Ok((time_only, sign_mult * (hh * 3600 + mm * 60 + ss)))
}

/// Microseconds since the 1970-01-01 UTC epoch. `with_tz` selects
/// `timestamp with time zone` parsing (an offset is expected and subtracted
/// out — I4: the offset is always explicit in the data, and this build
/// normalizes to UTC same as `pg_dump`'s own `extra_float_digits`-independent
/// convention) versus `timestamp without time zone` (a bare wall-clock
/// reading, stored as-is). `None` for `infinity`/`-infinity` — real
/// PostgreSQL values, but `Timestamp` has no sentinel for them, same
/// reasoning as [`decode_date32`].
pub fn decode_timestamp_micros(s: &str, with_tz: bool) -> Option<i64> {
    i64::try_from(timestamp_micros_wide(s, with_tz).ok()?).ok()
}

/// [`decode_timestamp_micros`] before it narrows to `i64`: microseconds from
/// the 1970 UTC epoch for any finite value the grammar reads and PostgreSQL's
/// range holds, so a value past what `Timestamp(Microsecond)` holds is told
/// from one that does not parse (`crate::unrepresentable`).
pub(crate) fn timestamp_micros_wide(s: &str, with_tz: bool) -> Read<i128> {
    if s == "infinity" || s == "-infinity" {
        return Err(Unread::Unparsed);
    }
    let (rest, bc) = split_era(s);
    let (date_part, time_part) = rest.split_once(' ').ok_or(Unread::Unparsed)?;
    let days = civil_days(date_part, bc)?;

    let (time_only, offset_secs) =
        if with_tz { extract_offset(time_part)? } else { (time_part, 0) };
    let (seconds, micros) = parse_time_of_day(time_only)?;

    let local_micros =
        i128::from(days) * 86_400_000_000 + i128::from(seconds) * 1_000_000 + i128::from(micros);
    let utc = local_micros - i128::from(offset_secs) * 1_000_000;
    // pg-refuses: I61 — `IS_VALID_TIMESTAMP`, of the instant: an offset can
    // carry a local time on either side of the range across its edge.
    let postgres = utc - POSTGRES_EPOCH_UNIX_MICROS;
    if !(i128::from(MIN_TIMESTAMP)..i128::from(END_TIMESTAMP)).contains(&postgres) {
        return Err(Unread::Refused);
    }
    Ok(utc)
}

/// `MIN_TIMESTAMP`, `4714-11-24 00:00:00 BC`, the first instant a timestamp
/// holds, in microseconds from PostgreSQL's epoch (I49).
const MIN_TIMESTAMP: i64 = -211_813_488_000_000_000;

/// `END_TIMESTAMP`, `294277-01-01 00:00:00`, the first instant past a
/// timestamp's range, in microseconds from PostgreSQL's epoch (I49).
const END_TIMESTAMP: i64 = 9_223_371_331_200_000_000;

/// PostgreSQL's epoch, 2000-01-01 00:00:00 UTC, in microseconds from the
/// Unix epoch.
const POSTGRES_EPOCH_UNIX_MICROS: i128 = 946_684_800_000_000;

pub fn render_timestamp_micros(v: i64, with_tz: bool) -> String {
    // `YYYY-MM-DD HH:MM:SS.ffffff+00`, with room for the era marker.
    let mut out = String::with_capacity(32);
    render_timestamp_micros_into(v, with_tz, &mut out);
    out
}

/// [`render_timestamp_micros`] appending to a caller's buffer.
pub fn render_timestamp_micros_into(v: i64, with_tz: bool, out: &mut String) {
    push_timestamp(v.div_euclid(86_400_000_000), v.rem_euclid(86_400_000_000), with_tz, out);
}

/// A timestamp's text out of its day count from 1970 and its microseconds
/// into that day.
fn push_timestamp(days: i64, of_day: i64, with_tz: bool, out: &mut String) {
    let bc = push_civil_date(out, days);
    out.push(' ');
    format_hms_frac_into(out, of_day);
    if with_tz {
        out.push_str("+00");
    }
    if bc {
        out.push_str(" BC");
    }
}

/// A signed count of `year`/`mon`/`day` units out of an `interval`'s text.
/// The leading `+` is `AddPostgresIntPart`'s, written on a positive part that
/// follows a negative one (I40); everything else is digits.
fn interval_count(text: &str) -> Option<i64> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    Some(if text.starts_with('-') { -magnitude } else { magnitude })
}

/// The `[+|-]HH:MM:SS[.ffffff]` tail of an `interval`, in microseconds.
/// `EncodeInterval` writes one sign for the whole time part — `minus` is set
/// if any of hours, minutes, seconds or the fraction is negative, and the
/// three fields are then printed as absolute values — so the sign is applied
/// to the total rather than per field (I40).
///
/// The hour field is bounded only by the total, so this is not
/// [`decode_time64_micros`]: `720:00:00` is an ordinary `interval` and not a
/// `time`. Every field is checked to be digits, which is what keeps
/// `04:-5:06` — a string no `interval_out` writes and no `interval_in`
/// accepts — from parsing as a negative minute count.
fn interval_time_micros(text: &str) -> Read<i64> {
    let unparsed = Unread::Unparsed;
    let (negative, rest) = match text.strip_prefix(['+', '-']) {
        Some(rest) => (text.starts_with('-'), rest),
        None => (false, text),
    };
    let (hms, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut parts = hms.split(':');
    let mut field = |max_len: Option<usize>| -> Read<i128> {
        let digits = parts.next().ok_or(unparsed)?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(unparsed);
        }
        if max_len.is_some_and(|n| digits.len() != n) {
            return Err(unparsed);
        }
        digits.parse::<i128>().map_err(|_| unparsed)
    };
    let hours = field(None)?;
    let minutes = field(Some(2))?;
    let seconds = field(Some(2))?;
    if parts.next().is_some() {
        return Err(unparsed);
    }
    // pg-refuses: I62 — `DecodeTimeCommon`'s minute past 59 and second past 60.
    if minutes > 59 || seconds > 60 {
        return Err(Unread::Refused);
    }
    if frac.is_empty() && text.contains('.') {
        return Err(unparsed);
    }
    if frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(unparsed);
    }
    let micros: i128 = if frac.is_empty() {
        0
    } else {
        let mut padded = frac.to_string();
        padded.push_str(&"0".repeat(6 - padded.len()));
        padded.parse().map_err(|_| unparsed)?
    };
    // pg-refuses: I62 — the `Interval` struct's `int64` microseconds, either
    // sign: the text carries the magnitude, so `i64::MIN` is no value of it.
    let total = hours
        .checked_mul(3600)
        .and_then(|t| t.checked_add(minutes * 60 + seconds))
        .and_then(|t| t.checked_mul(1_000_000))
        .and_then(|t| t.checked_add(micros))
        .and_then(|t| i64::try_from(t).ok())
        .ok_or(Unread::Refused)?;
    Ok(if negative { -total } else { total })
}

/// The three parts of an `interval`'s text as PostgreSQL's `Interval` struct
/// holds them: whole months (a `year` part folded in at twelve each), whole
/// days, and the time tail in **microseconds**. The *comparison* built on
/// this fuses all three into a 128-bit span (`interval_cmp_value`, I40),
/// where the *decode* widens the time to Arrow's nanoseconds — one walk, two
/// consumers, and neither step belongs to it.
///
/// **The grammar covers `interval_out`'s under `IntervalStyle = postgres`**,
/// which `pg_dump` pins on its own connection (I4): an optional
/// `<n> year[s]`, `<n> mon[s]` and `<n> day[s]`, then an optional signed time
/// part, separated by single spaces, with a wholly-zero interval written
/// `00:00:00`. It is a little wider than that grammar — the counted parts are
/// taken in any order, either number of their unit, a `+` on any of them,
/// and the hours at any width — but no other unit: `1 hour`, `1.5 hours`,
/// `P1Y2M` and `1 month` are all spellings `interval_in` takes and
/// `interval_out` never writes, and are unparsed. `infinity`/`-infinity`
/// (v17's, I34) are not in the grammar either, so they fail here and each
/// consumer says what it does about them.
pub(crate) fn interval_parts(text: &str) -> Read<(i32, i32, i64)> {
    let tokens: Vec<&str> = text.split(' ').collect();
    // Years, months and days, each counted at most once.
    let mut counts: [Option<i32>; 3] = [None; 3];
    let mut time = 0i64;
    let mut at = 0;
    while at < tokens.len() {
        let unit = match tokens.get(at + 1).copied() {
            Some("year" | "years") => 0,
            Some("mon" | "mons") => 1,
            Some("day" | "days") => 2,
            // Not a counted part, so this token is the time tail — which is
            // last, and of which there is at most one.
            _ => {
                if at + 1 != tokens.len() {
                    return Err(Unread::Unparsed);
                }
                time = interval_time_micros(tokens[at])?;
                at += 1;
                break;
            }
        };
        // pg-refuses: I62 — a unit given twice, and a count past `int32`
        // but not past `i64`, which is unparsed.
        if counts[unit].is_some() {
            return Err(Unread::Refused);
        }
        let count = interval_count(tokens[at]).ok_or(Unread::Unparsed)?;
        counts[unit] = Some(i32::try_from(count).map_err(|_| Unread::Refused)?);
        at += 2;
    }
    if at != tokens.len() {
        return Err(Unread::Unparsed);
    }
    let [years, months, days] = counts.map(|count| i64::from(count.unwrap_or(0)));
    // pg-refuses: I62 — `itmin2interval`'s month total past `int32`.
    let months = i32::try_from(years * 12 + months).map_err(|_| Unread::Refused)?;
    Ok((months, i32::try_from(days).expect("a day count is an i32"), time))
}

/// `interval`, as Arrow's `Interval(MonthDayNano)` holds it: months, days and
/// a **nanosecond** time part, which are exactly PostgreSQL's own three
/// independent fields. `None` for a spelling outside [`interval_parts`]'s
/// grammar, which `interval_in` may read, and for three classes of value,
/// each a decode failure by construction the way [`decode_date32`]'s
/// infinities are:
///
/// - `infinity` and `-infinity` (v17's, I34) — every field of PostgreSQL's
///   struct is extremal and Arrow has no encoding for the value at all;
/// - a time part past `2562047:47:16.854775807` — PostgreSQL's field is
///   `int64` *microseconds* against Arrow's `int64` nanoseconds, a
///   thousandth of the range, and nothing normalizes hours into days (I40);
/// - a value `interval_in` refuses, which [`interval_parts`] refuses (I62).
// deficiency: KD99 — a field is read as written, not through its column's
// typmod: `AdjustIntervalForTypmod` zeroes the fields right of an `interval`'s
// qualifier and rounds its time part to the precision, and
// `AdjustTimeForTypmod` and `AdjustTimestampForTypmod` round a `time(p)`,
// `timetz(p)`, `timestamp(p)` or `timestamptz(p)` field, so `1 year 3 mons` in
// an `interval year` or `12:00:00.5` in a `time(0)` emits a value the server
// does not hold, and keys and compares as it. No `pg_dump` writes one, its
// `*_out` text being of the stored value. The fix carries each typmod to its
// kind, as `CompareKind::Interval` carries the qualifier, and adjusts the
// field in the read, the key and the bounds, as `typmod_unscaled_digits` does
// a `numeric(p,s)`.
pub fn decode_interval(s: &str) -> Option<(i32, i32, i64)> {
    let (months, days, micros) = interval_parts(s).ok()?;
    Some((months, days, micros.checked_mul(1_000)?))
}

/// `EncodeInterval` under `INTSTYLE_POSTGRES`, the exact inverse of
/// [`decode_interval`] (I40). Three rules carry the whole form, and none of
/// them is `format_hms_frac_into`'s: a part is suppressed when its value is zero,
/// its unit takes an `s` whenever the value is not exactly `1`, and a part
/// that is positive and *follows a negative one* carries a `+`. The time tail
/// is written when it is nonzero or when nothing else was — which is what
/// makes a wholly-zero interval `00:00:00` — with one sign for the whole
/// tail, its hour field at least two digits and unbounded above.
///
/// **`None` for a `nanos` that is not a whole number of microseconds.**
/// PostgreSQL's time field counts microseconds, so no `interval` has such a
/// value and there is no text to write; the value is refused rather than
/// truncated (`docs/design/decisions.md`, "D44"). [`decode_interval`] cannot
/// produce one — it multiplies microseconds by a thousand — so this is
/// reachable only from an `Interval(MonthDayNano)` array a caller built.
pub fn render_interval(months: i32, days: i32, nanos: i64) -> Option<String> {
    if nanos % 1_000 != 0 {
        return None;
    }
    let mut out = String::new();
    let mut is_zero = true;
    let mut is_before = false;
    let months = i64::from(months);
    for (value, unit) in [(months / 12, "year"), (months % 12, "mon"), (i64::from(days), "day")] {
        if value == 0 {
            continue;
        }
        if !is_zero {
            out.push(' ');
        }
        if is_before && value > 0 {
            out.push('+');
        }
        out.push_str(&value.to_string());
        out.push(' ');
        out.push_str(unit);
        if value != 1 {
            out.push('s');
        }
        is_before = value < 0;
        is_zero = false;
    }
    let micros = nanos / 1_000;
    let (hours, rest) = (micros / 3_600_000_000, micros % 3_600_000_000);
    let (minutes, rest) = (rest / 60_000_000, rest % 60_000_000);
    let (seconds, frac) = (rest / 1_000_000, rest % 1_000_000);
    if is_zero || micros != 0 {
        if !is_zero {
            out.push(' ');
        }
        out.push_str(if micros < 0 {
            "-"
        } else if is_before {
            "+"
        } else {
            ""
        });
        out.push_str(&format!("{:02}:{:02}:{:02}", hours.abs(), minutes.abs(), seconds.abs()));
        if frac != 0 {
            let mut digits = format!("{:06}", frac.abs());
            while digits.ends_with('0') {
                digits.pop();
            }
            out.push('.');
            out.push_str(&digits);
        }
    }
    Some(out)
}

/// `interval_cmp_value`'s span, in microseconds: months collapse to 30 days,
/// days to 86400 seconds, and the time field is added on (I40), so `1 mon`,
/// `30 days` and `720:00:00` are one value written three ways — which is why
/// an `interval` cannot canonicalize once and compare bytewise.
///
/// The walk over the text is
/// [`interval_parts`](crate::value::datetime::interval_parts), shared with the
/// decoder, so a field the decoder refuses includes every literal this refuses
/// — and a time part past what Arrow's nanoseconds hold, besides. The fusing is
/// this function's alone: a decoder that did it would lose the fields Arrow
/// carries separately.
pub(crate) fn interval_span(text: &str) -> Read<i128> {
    let (months, days, time) = interval_parts(text)?;
    let whole_days = i128::from(months) * 30 + i128::from(days);
    Ok(whole_days * 86_400_000_000 + i128::from(time))
}

/// A date, time or timestamp reader's answer, a text it reads no value from
/// refused where no major's grammar reads it under any setting
/// ([`datetime_reads`](crate::value::datetime::grammar::datetime_reads)). The
/// readers read `*_out`'s form and little more, splitting a text otherwise than
/// `ParseDateTime` does where it is not that form, so a refusal of theirs is
/// put to the grammar too.
pub(crate) fn datetime_read<T>(read: Read<T>, input: DateTimeInput, text: &str) -> Read<T> {
    read.map_err(|_| if datetime_reads(input, text) { Unread::Unparsed } else { Unread::Refused })
}

/// [`datetime_read`] for an `interval` of a column qualified by `qualifier`
/// ([`interval_reads`](crate::value::datetime::grammar::interval_reads)).
pub(crate) fn interval_read<T>(
    read: Read<T>,
    text: &str,
    qualifier: Option<IntervalQualifier>,
) -> Read<T> {
    read.map_err(
        |_| {
            if interval_reads(text, qualifier) { Unread::Unparsed } else { Unread::Refused }
        },
    )
}

#[cfg(test)]
mod tests;
