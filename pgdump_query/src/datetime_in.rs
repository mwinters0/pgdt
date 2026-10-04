//! The date and time input functions' grammar — `ParseDateTime`,
//! `DecodeDateTime`, `DecodeTimeOnly`, `DecodeInterval` and
//! `DecodeISO8601Interval` (`src/backend/utils/adt/datetime.c`) — ported at
//! every supported major to answer one question of a text the readers in
//! `crate::decode` read no value from: **does some major, under some setting
//! the restoring server may hold, read it?** A field is refused only where
//! none does (I83, I84; `docs/design/decisions.md`, "D55").
//!
//! Each function below is its C namesake read as written: `strtol`'s blanks
//! and sign, `atoi`'s truncation, `int` arithmetic wrapping as the server's
//! `-fwrapv` build wraps it, and a `double` converted to an integer as x86-64
//! converts one past its range, to the integer's least value. Where majors
//! differ the code is gated on [`Major`]; 13 and 14 read alike.
//!
//! **What the restoring server decides is taken at its most permissive**,
//! since a field one server reads is no refusal:
//!
//! - `DateStyle`'s field order, and `IntervalStyle`: every one is tried;
//! - a word no keyword names, which may name a zone some tzdata holds, zone
//!   names being open: it is read as a fixed-offset zone, the most
//!   permissive reading of a zone, setting the zone and nothing else, which
//!   `pg_tzset`'s zone name and an abbreviation are no more permissive than;
//! - a keyword, which `DecodeTimezoneAbbrev` reads as a zone first where a
//!   `timezone_abbreviations` file or, from v18, the session zone's tzdata
//!   names it: abbreviations are closed, so only `SHADOWED_KEYWORDS` are (D55);
//! - a zone named with punctuation (`america/new_york`, `est5edt`), which
//!   `pg_tzset` reads from the server's zone files or as a POSIX zone spec:
//!   such a zone exists, fixed or not;
//! - the offset of a zone the text names by word or by name, and of the
//!   session's zone where it names none: within a week either way of a
//!   timestamp's range, some offset is taken to bring it in.
//!
//! A byte past `0x7F` is classed by the server's locale, so a text holding
//! one is taken as read.

use crate::decode;
use crate::pgtype::IntervalQualifier;

/// A supported major whose grammar differs from the one before it: 14 reads
/// as 13 does.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Major {
    V13,
    V15,
    V16,
    V17,
    V18,
}

const MAJORS: [Major; 5] = [Major::V13, Major::V15, Major::V16, Major::V17, Major::V18];

/// `DateOrder`, which `DateStyle` sets: how a run of short numbers is read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DateOrder {
    Ymd,
    Dmy,
    Mdy,
}

const DATE_ORDERS: [DateOrder; 3] = [DateOrder::Ymd, DateOrder::Dmy, DateOrder::Mdy];

/// Why a decode fails: `interval_in` tries ISO 8601 only after a bad format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dterr {
    BadFormat,
    Overflow,
}

type Dt<T> = Result<T, Dterr>;

const BAD: Dterr = Dterr::BadFormat;
const OVER: Dterr = Dterr::Overflow;

/// Which input function a text is put to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DateTimeInput {
    Date,
    Time,
    TimeTz,
    Timestamp,
    TimestampTz,
}

const MAXDATEFIELDS: usize = 25;
const MAXDATELEN: usize = 128;
const TOKMAXLEN: usize = 10;

// Field types (`datetime.h`), a mask's bit numbers.
const RESERV: i32 = 0;
const MONTH: i32 = 1;
const YEAR: i32 = 2;
const DAY: i32 = 3;
const TZ: i32 = 5;
const DTZ: i32 = 6;
const IGNORE_DTF: i32 = 8;
const AMPM: i32 = 9;
const HOUR: i32 = 10;
const MINUTE: i32 = 11;
const SECOND: i32 = 12;
const MILLISECOND: i32 = 13;
const MICROSECOND: i32 = 14;
const DOY: i32 = 15;
const DOW: i32 = 16;
const UNITS: i32 = 17;
const ADBC: i32 = 18;
const AGO: i32 = 19;
const ISOTIME: i32 = 23;
const WEEK: i32 = 24;
const DECADE: i32 = 25;
const CENTURY: i32 = 26;
const MILLENNIUM: i32 = 27;
const DTZMOD: i32 = 28;
const UNKNOWN_FIELD: i32 = 31;

// Token values (`datetime.h`).
const DTK_DATE: i32 = 2;
const DTK_TIME: i32 = 3;
const DTK_TZ: i32 = 4;
const DTK_EARLY: i32 = 9;
const DTK_LATE: i32 = 10;
const DTK_EPOCH: i32 = 11;
const DTK_NOW: i32 = 12;
const DTK_YESTERDAY: i32 = 13;
const DTK_TODAY: i32 = 14;
const DTK_TOMORROW: i32 = 15;
const DTK_ZULU: i32 = 16;
const DTK_DELTA: i32 = 17;
const DTK_SECOND: i32 = 18;
const DTK_MINUTE: i32 = 19;
const DTK_HOUR: i32 = 20;
const DTK_DAY: i32 = 21;
const DTK_WEEK: i32 = 22;
const DTK_MONTH: i32 = 23;
const DTK_QUARTER: i32 = 24;
const DTK_YEAR: i32 = 25;
const DTK_DECADE: i32 = 26;
const DTK_CENTURY: i32 = 27;
const DTK_MILLENNIUM: i32 = 28;
const DTK_MILLISEC: i32 = 29;
const DTK_MICROSEC: i32 = 30;
const DTK_JULIAN: i32 = 31;
const DTK_DOW: i32 = 32;
const DTK_DOY: i32 = 33;
const DTK_TZ_HOUR: i32 = 34;
const DTK_TZ_MINUTE: i32 = 35;
const DTK_ISOYEAR: i32 = 36;
const DTK_ISODOW: i32 = 37;

const AM: i32 = 0;
const PM: i32 = 1;
const HR24: i32 = 2;
const BC: i32 = 1;

const fn m(t: i32) -> u32 {
    1 << t
}

const DATE_M: u32 = m(YEAR) | m(MONTH) | m(DAY);
const ALL_SECS_M: u32 = m(SECOND) | m(MILLISECOND) | m(MICROSECOND);
const TIME_M: u32 = m(HOUR) | m(MINUTE) | ALL_SECS_M;

const USECS_PER_SEC: i64 = 1_000_000;
const USECS_PER_MINUTE: i64 = 60 * USECS_PER_SEC;
const USECS_PER_HOUR: i64 = 60 * USECS_PER_MINUTE;
const USECS_PER_DAY: i64 = 24 * USECS_PER_HOUR;
const POSTGRES_EPOCH_JDATE: i32 = 2_451_545;
const DATE_END_JULIAN: i32 = 2_147_483_494;
const MIN_TIMESTAMP: i64 = -211_813_488_000_000_000;
const END_TIMESTAMP: i64 = 9_223_371_331_200_000_000;

/// How far a zone the text does not fix may move a timestamp: a POSIX zone
/// spec reads an offset of up to 167 hours (`tzparse`'s `getsecs`).
const ZONE_OFFSET_BOUND: i64 = 168 * USECS_PER_HOUR;

// `INTERVAL_MASK` bits (`timestamp.h`).
const fn interval_mask(b: i32) -> i32 {
    1 << b
}
const INTERVAL_FULL_RANGE: i32 = 0x7FFF;

/// `datetktbl`, the keywords a date or time reads; `+infinity` from v16.
const DATETKTBL: &[(&str, i32, i32)] = &[
    ("+infinity", RESERV, DTK_LATE),
    ("-infinity", RESERV, DTK_EARLY),
    ("ad", ADBC, 0),
    ("allballs", RESERV, DTK_ZULU),
    ("am", AMPM, AM),
    ("apr", MONTH, 4),
    ("april", MONTH, 4),
    ("at", IGNORE_DTF, 0),
    ("aug", MONTH, 8),
    ("august", MONTH, 8),
    ("bc", ADBC, BC),
    ("d", UNITS, DTK_DAY),
    ("dec", MONTH, 12),
    ("december", MONTH, 12),
    ("dow", UNITS, DTK_DOW),
    ("doy", UNITS, DTK_DOY),
    ("dst", DTZMOD, 3600),
    ("epoch", RESERV, DTK_EPOCH),
    ("feb", MONTH, 2),
    ("february", MONTH, 2),
    ("fri", DOW, 5),
    ("friday", DOW, 5),
    ("h", UNITS, DTK_HOUR),
    ("infinity", RESERV, DTK_LATE),
    ("isodow", UNITS, DTK_ISODOW),
    ("isoyear", UNITS, DTK_ISOYEAR),
    ("j", UNITS, DTK_JULIAN),
    ("jan", MONTH, 1),
    ("january", MONTH, 1),
    ("jd", UNITS, DTK_JULIAN),
    ("jul", MONTH, 7),
    ("julian", UNITS, DTK_JULIAN),
    ("july", MONTH, 7),
    ("jun", MONTH, 6),
    ("june", MONTH, 6),
    ("m", UNITS, DTK_MONTH),
    ("mar", MONTH, 3),
    ("march", MONTH, 3),
    ("may", MONTH, 5),
    ("mm", UNITS, DTK_MINUTE),
    ("mon", DOW, 1),
    ("monday", DOW, 1),
    ("nov", MONTH, 11),
    ("november", MONTH, 11),
    ("now", RESERV, DTK_NOW),
    ("oct", MONTH, 10),
    ("october", MONTH, 10),
    ("on", IGNORE_DTF, 0),
    ("pm", AMPM, PM),
    ("s", UNITS, DTK_SECOND),
    ("sat", DOW, 6),
    ("saturday", DOW, 6),
    ("sep", MONTH, 9),
    ("sept", MONTH, 9),
    ("september", MONTH, 9),
    ("sun", DOW, 0),
    ("sunday", DOW, 0),
    ("t", ISOTIME, DTK_TIME),
    ("thu", DOW, 4),
    ("thur", DOW, 4),
    ("thurs", DOW, 4),
    ("thursday", DOW, 4),
    ("today", RESERV, DTK_TODAY),
    ("tomorrow", RESERV, DTK_TOMORROW),
    ("tue", DOW, 2),
    ("tues", DOW, 2),
    ("tuesday", DOW, 2),
    ("wed", DOW, 3),
    ("wednesday", DOW, 3),
    ("weds", DOW, 3),
    ("y", UNITS, DTK_YEAR),
    ("yesterday", RESERV, DTK_YESTERDAY),
];

/// The keywords a `timezone_abbreviations` file PostgreSQL ships names as a
/// zone abbreviation, which `DecodeTimezoneAbbrev` reads first: `SAT`, South
/// Australian time in `Australia`. A file of the server's own naming another
/// keyword is not taken to exist (D55).
const SHADOWED_KEYWORDS: [&[u8]; 1] = [b"sat"];

/// `deltatktbl`, the units an `interval` reads, alike at every major.
const DELTATKTBL: &[(&str, i32, i32)] = &[
    ("@", IGNORE_DTF, 0),
    ("ago", AGO, 0),
    ("c", UNITS, DTK_CENTURY),
    ("cent", UNITS, DTK_CENTURY),
    ("centuries", UNITS, DTK_CENTURY),
    ("century", UNITS, DTK_CENTURY),
    ("d", UNITS, DTK_DAY),
    ("day", UNITS, DTK_DAY),
    ("days", UNITS, DTK_DAY),
    ("dec", UNITS, DTK_DECADE),
    ("decade", UNITS, DTK_DECADE),
    ("decades", UNITS, DTK_DECADE),
    ("decs", UNITS, DTK_DECADE),
    ("h", UNITS, DTK_HOUR),
    ("hour", UNITS, DTK_HOUR),
    ("hours", UNITS, DTK_HOUR),
    ("hr", UNITS, DTK_HOUR),
    ("hrs", UNITS, DTK_HOUR),
    ("m", UNITS, DTK_MINUTE),
    ("microsecon", UNITS, DTK_MICROSEC),
    ("mil", UNITS, DTK_MILLENNIUM),
    ("millennia", UNITS, DTK_MILLENNIUM),
    ("millennium", UNITS, DTK_MILLENNIUM),
    ("millisecon", UNITS, DTK_MILLISEC),
    ("mils", UNITS, DTK_MILLENNIUM),
    ("min", UNITS, DTK_MINUTE),
    ("mins", UNITS, DTK_MINUTE),
    ("minute", UNITS, DTK_MINUTE),
    ("minutes", UNITS, DTK_MINUTE),
    ("mon", UNITS, DTK_MONTH),
    ("mons", UNITS, DTK_MONTH),
    ("month", UNITS, DTK_MONTH),
    ("months", UNITS, DTK_MONTH),
    ("ms", UNITS, DTK_MILLISEC),
    ("msec", UNITS, DTK_MILLISEC),
    ("msecond", UNITS, DTK_MILLISEC),
    ("mseconds", UNITS, DTK_MILLISEC),
    ("msecs", UNITS, DTK_MILLISEC),
    ("qtr", UNITS, DTK_QUARTER),
    ("quarter", UNITS, DTK_QUARTER),
    ("s", UNITS, DTK_SECOND),
    ("sec", UNITS, DTK_SECOND),
    ("second", UNITS, DTK_SECOND),
    ("seconds", UNITS, DTK_SECOND),
    ("secs", UNITS, DTK_SECOND),
    ("timezone", UNITS, DTK_TZ),
    ("timezone_h", UNITS, DTK_TZ_HOUR),
    ("timezone_m", UNITS, DTK_TZ_MINUTE),
    ("us", UNITS, DTK_MICROSEC),
    ("usec", UNITS, DTK_MICROSEC),
    ("usecond", UNITS, DTK_MICROSEC),
    ("useconds", UNITS, DTK_MICROSEC),
    ("usecs", UNITS, DTK_MICROSEC),
    ("w", UNITS, DTK_WEEK),
    ("week", UNITS, DTK_WEEK),
    ("weeks", UNITS, DTK_WEEK),
    ("y", UNITS, DTK_YEAR),
    ("year", UNITS, DTK_YEAR),
    ("years", UNITS, DTK_YEAR),
    ("yr", UNITS, DTK_YEAR),
    ("yrs", UNITS, DTK_YEAR),
];

/// **Whether some supported major, under some setting the restoring server
/// may hold, reads `text` as a value of `input`'s type** — `date_in`,
/// `time_in`, `timetz_in`, `timestamp_in` or `timestamptz_in`, each a
/// `ParseDateTime` and a `DecodeDateTime` or `DecodeTimeOnly`, then its own
/// range check.
// pg-refuses: I83 — every refusal here is every major's, under every setting
// and set of zone names, abbreviations closed (D55).
pub(crate) fn datetime_reads(input: DateTimeInput, text: &str) -> bool {
    let b = text.as_bytes();
    if b.iter().any(|&c| c == 0 || c >= 0x80) {
        return true;
    }
    let buflen = match input {
        DateTimeInput::Date | DateTimeInput::Time | DateTimeInput::TimeTz => MAXDATELEN + 1,
        DateTimeInput::Timestamp | DateTimeInput::TimestampTz => MAXDATELEN + MAXDATEFIELDS,
    };
    let Some(fields) = parse_date_time(b, buflen) else {
        return false;
    };
    MAJORS.iter().any(|&major| {
        // A keyword a shipped `timezone_abbreviations` file shadows, one at a
        // time: two zones in one text are refused whatever they name.
        let keywords = fields.iter().enumerate().filter_map(|(i, field)| {
            let word = matches!(field.ftype, Ftype::String | Ftype::Special);
            (word && SHADOWED_KEYWORDS.contains(&&field.text[..])).then_some(Some(i))
        });
        let shadows: Vec<Option<usize>> = std::iter::once(None).chain(keywords).collect();
        DATE_ORDERS.iter().any(|&order| {
            let server = Server { major, order };
            shadows.iter().any(|&shadowed| datetime_accepts(input, &fields, server, shadowed))
        })
    })
}

/// **Whether some supported major, under some `IntervalStyle`, reads `text`
/// as an `interval` of a column qualified by `qualifier`** — `None` for one
/// with none: `interval_in`'s `ParseDateTime` and `DecodeInterval` under the
/// qualifier's range, then `DecodeISO8601Interval` where those found a bad
/// format, then its month total's range. The column's precision refuses
/// nothing every major refuses, 13 to 16 rounding past `int64` unchecked.
// pg-refuses: I84 — every refusal here is every major's, under every setting.
pub(crate) fn interval_reads(text: &str, qualifier: Option<IntervalQualifier>) -> bool {
    let b = text.as_bytes();
    if b.iter().any(|&c| c == 0 || c >= 0x80) {
        return true;
    }
    let range = qualifier.map_or(INTERVAL_FULL_RANGE, IntervalQualifier::range);
    let fields = parse_date_time(b, 256);
    MAJORS.iter().any(|&major| {
        [false, true].iter().any(|&sql_standard| {
            if major >= Major::V15 {
                interval_accepts(b, fields.as_deref(), major, sql_standard, range)
            } else {
                interval_accepts_v13(b, fields.as_deref(), sql_standard, range)
            }
        })
    })
}

// ---------------------------------------------------------------------------
// C's library, as glibc and x86-64 read it.

/// `b[i]` as C reads a string, `0` past its end.
fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

fn isdigit(c: u8) -> bool {
    c.is_ascii_digit()
}

fn isalpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

fn isalnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

fn isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

fn ispunct(c: u8) -> bool {
    c.is_ascii_punctuation()
}

/// `strtol(b + i, &end, 10)`: its value saturated at `long`'s range, where it
/// ends — `i` itself where it reads no digit — and whether it saturated.
fn strtol(b: &[u8], i: usize) -> (i64, usize, bool) {
    let mut j = i;
    while isspace(at(b, j)) {
        j += 1;
    }
    let negative = at(b, j) == b'-';
    if matches!(at(b, j), b'+' | b'-') {
        j += 1;
    }
    let first = j;
    let mut magnitude: i128 = 0;
    while isdigit(at(b, j)) {
        magnitude = (magnitude * 10 + i128::from(at(b, j) - b'0')).min(1 << 70);
        j += 1;
    }
    if j == first {
        return (0, i, false);
    }
    let value = if negative { -magnitude } else { magnitude };
    match i64::try_from(value) {
        Ok(value) => (value, j, false),
        Err(_) => (if negative { i64::MIN } else { i64::MAX }, j, true),
    }
}

/// `strtoint`: [`strtol`] held to `int`, `ERANGE` past it, truncated.
fn strtoint(b: &[u8], i: usize) -> (i32, usize, bool) {
    let (value, end, erange) = strtol(b, i);
    (value as i32, end, erange || i64::from(value as i32) != value)
}

/// `atoi`: `(int) strtol(b, NULL, 10)`.
fn atoi(b: &[u8]) -> i32 {
    strtol(b, 0).0 as i32
}

/// glibc's `strtod(b + i, &end)`: its value, where it ends — `i` where it
/// reads nothing — and whether it set `ERANGE`, which glibc sets past the
/// range and on a decimal read to a subnormal. A hexadecimal subnormal is
/// taken as exact.
fn strtod(b: &[u8], i: usize) -> (f64, usize, bool) {
    let mut j = i;
    while isspace(at(b, j)) {
        j += 1;
    }
    match decode::strtod(b, j) {
        None => (0.0, i, false),
        Some((value, end, out_of_range)) => {
            let sign = usize::from(matches!(at(b, j), b'+' | b'-'));
            let hex = at(b, j + sign) == b'0' && matches!(at(b, j + sign + 1), b'x' | b'X');
            let subnormal = value != 0.0 && value.is_finite() && value.abs() < f64::MIN_POSITIVE;
            (value, end, out_of_range || (subnormal && !hex))
        }
    }
}

/// `(int) x` as x86-64's `cvttsd2si` converts it: truncated, and `INT_MIN`
/// where that lies past `int` or `x` is NaN.
fn c_int(x: f64) -> i32 {
    let t = x.trunc();
    if t.is_nan() || !(-2_147_483_648.0..=2_147_483_647.0).contains(&t) {
        i32::MIN
    } else {
        t as i32
    }
}

/// `(int64) x` as x86-64 converts it.
fn c_int64(x: f64) -> i64 {
    let t = x.trunc();
    if t.is_nan() || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&t) {
        i64::MIN
    } else {
        t as i64
    }
}

/// `rint` in the default rounding mode.
fn rint(x: f64) -> f64 {
    x.round_ties_even()
}

/// `strncmp(key, token, TOKMAXLEN) == 0`, which is how every table here is
/// searched, so a word past ten letters matches a ten-letter token.
fn strncmp_matches(key: &[u8], token: &[u8]) -> bool {
    for k in 0..TOKMAXLEN {
        let (x, y) = (at(key, k), at(token, k));
        if x != y {
            return false;
        }
        if x == 0 {
            return true;
        }
    }
    true
}

/// `DecodeSpecial`: `token`'s keyword, its type and value.
fn keyword(major: Major, token: &[u8]) -> Option<(i32, i32)> {
    DATETKTBL
        .iter()
        .filter(|&&(word, ..)| major >= Major::V16 || word != "+infinity")
        .find(|&&(word, ..)| strncmp_matches(token, word.as_bytes()))
        .map(|&(_, ty, value)| (ty, value))
}

/// `DecodeUnits`: `token`'s interval unit, its type and value.
fn units(token: &[u8]) -> Option<(i32, i32)> {
    DELTATKTBL
        .iter()
        .find(|&&(word, ..)| strncmp_matches(token, word.as_bytes()))
        .map(|&(_, ty, value)| (ty, value))
}

// ---------------------------------------------------------------------------
// The calendar.

/// `date2j`, in `int` arithmetic that wraps.
fn date2j(year: i32, month: i32, day: i32) -> i32 {
    let (mut year, mut month) = (year, month);
    if month > 2 {
        month = month.wrapping_add(1);
        year = year.wrapping_add(4800);
    } else {
        month = month.wrapping_add(13);
        year = year.wrapping_add(4799);
    }
    let century = year / 100;
    let mut julian = year.wrapping_mul(365).wrapping_sub(32167);
    julian = julian.wrapping_add((year / 4).wrapping_sub(century).wrapping_add(century / 4));
    julian.wrapping_add(7834i32.wrapping_mul(month) / 256).wrapping_add(day)
}

/// `j2date`, in its `unsigned int` arithmetic.
fn j2date(jd: i32) -> (i32, i32, i32) {
    let mut julian = (jd as u32).wrapping_add(32044);
    let mut quad = julian / 146_097;
    let extra = julian.wrapping_sub(quad.wrapping_mul(146_097)).wrapping_mul(4).wrapping_add(3);
    julian =
        julian.wrapping_add(60u32.wrapping_add(quad.wrapping_mul(3)).wrapping_add(extra / 146_097));
    quad = julian / 1461;
    julian = julian.wrapping_sub(quad.wrapping_mul(1461));
    let mut y = (julian.wrapping_mul(4) / 1461) as i32;
    julian = if y != 0 { (julian + 305) % 365 } else { (julian + 306) % 366 } + 123;
    y = (y as u32).wrapping_add(quad.wrapping_mul(4)) as i32;
    let year = y.wrapping_sub(4800);
    quad = julian.wrapping_mul(2141) / 65536;
    let day = julian.wrapping_sub(7834u32.wrapping_mul(quad) / 256) as i32;
    let month = ((quad + 10) % 12 + 1) as i32;
    (year, month, day)
}

fn isleap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

const DAY_TAB: [[i32; 12]; 2] = [
    [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
    [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
];

/// `IS_VALID_JULIAN`.
fn is_valid_julian(y: i32, m: i32) -> bool {
    (y > -4713 || (y == -4713 && m >= 11)) && (y < 5_874_898 || (y == 5_874_898 && m < 6))
}

/// The broken-down time `DecodeDateTime` and `DecodeTimeOnly` fill.
#[derive(Clone, Copy, Default, Debug)]
struct Tm {
    year: i32,
    mon: i32,
    mday: i32,
    hour: i32,
    min: i32,
    sec: i32,
    yday: i32,
}

/// The transaction's start, as `now` and `today` read it: a date within every
/// range, so its date decides nothing, and noon, the one hour both `am` and
/// `pm` read.
const NOW: Tm = Tm { year: 2026, mon: 6, mday: 15, hour: 12, min: 0, sec: 0, yday: 0 };

/// A zone's offset as the decode leaves it: one the text fixes, or one the
/// server decides (a zone name or abbreviation, or the session's).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Zone {
    Known(i32),
    Unknown,
}

// ---------------------------------------------------------------------------
// ParseDateTime.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ftype {
    Number,
    String,
    Date,
    Time,
    Tz,
    Special,
}

#[derive(Debug)]
struct Field {
    ftype: Ftype,
    text: Vec<u8>,
}

/// `ParseDateTime` into a work buffer of `buflen` bytes, every field and its
/// terminator held in it: `None` where it refuses the text. Alike at every
/// major.
fn parse_date_time(s: &[u8], buflen: usize) -> Option<Vec<Field>> {
    let mut fields: Vec<Field> = Vec::new();
    let mut used = 0usize;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if isspace(c) {
            i += 1;
            continue;
        }
        if fields.len() >= MAXDATEFIELDS {
            return None;
        }
        let mut f: Vec<u8> = Vec::new();
        // `APPEND_CHAR`: room for the byte and a terminator, or a bad format.
        let mut append = |f: &mut Vec<u8>, byte: u8| -> Option<()> {
            if used + 1 >= buflen {
                return None;
            }
            f.push(byte);
            used += 1;
            Some(())
        };
        let ftype;
        if isdigit(c) {
            append(&mut f, c)?;
            i += 1;
            while isdigit(at(s, i)) {
                append(&mut f, s[i])?;
                i += 1;
            }
            if at(s, i) == b':' {
                ftype = Ftype::Time;
                append(&mut f, b':')?;
                i += 1;
                while isdigit(at(s, i)) || matches!(at(s, i), b':' | b'.') {
                    append(&mut f, s[i])?;
                    i += 1;
                }
            } else if matches!(at(s, i), b'-' | b'/' | b'.') {
                let delim = s[i];
                append(&mut f, delim)?;
                i += 1;
                if isdigit(at(s, i)) {
                    let mut t = if delim == b'.' { Ftype::Number } else { Ftype::Date };
                    while isdigit(at(s, i)) {
                        append(&mut f, s[i])?;
                        i += 1;
                    }
                    if at(s, i) == delim {
                        t = Ftype::Date;
                        append(&mut f, delim)?;
                        i += 1;
                        while isdigit(at(s, i)) || at(s, i) == delim {
                            append(&mut f, s[i])?;
                            i += 1;
                        }
                    }
                    ftype = t;
                } else {
                    ftype = Ftype::Date;
                    while isalnum(at(s, i)) || at(s, i) == delim {
                        append(&mut f, s[i].to_ascii_lowercase())?;
                        i += 1;
                    }
                }
            } else {
                ftype = Ftype::Number;
            }
        } else if c == b'.' {
            append(&mut f, c)?;
            i += 1;
            while isdigit(at(s, i)) {
                append(&mut f, s[i])?;
                i += 1;
            }
            ftype = Ftype::Number;
        } else if isalpha(c) {
            append(&mut f, c.to_ascii_lowercase())?;
            i += 1;
            while isalpha(at(s, i)) {
                append(&mut f, s[i].to_ascii_lowercase())?;
                i += 1;
            }
            // Only the core keywords are searched, which `+infinity`, not
            // being a word, never matches.
            let is_date = match at(s, i) {
                b'-' | b'/' | b'.' => true,
                b'+' => keyword(Major::V18, &f).is_none(),
                d if isdigit(d) => keyword(Major::V18, &f).is_none(),
                _ => false,
            };
            if is_date {
                loop {
                    append(&mut f, s[i].to_ascii_lowercase())?;
                    i += 1;
                    let next = at(s, i);
                    if !(matches!(next, b'+' | b'-' | b'/' | b'_' | b'.' | b':') || isalnum(next)) {
                        break;
                    }
                }
                ftype = Ftype::Date;
            } else {
                ftype = Ftype::String;
            }
        } else if c == b'+' || c == b'-' {
            append(&mut f, c)?;
            i += 1;
            while isspace(at(s, i)) {
                i += 1;
            }
            if isdigit(at(s, i)) {
                ftype = Ftype::Tz;
                append(&mut f, s[i])?;
                i += 1;
                while isdigit(at(s, i)) || matches!(at(s, i), b':' | b'.' | b'-') {
                    append(&mut f, s[i])?;
                    i += 1;
                }
            } else if isalpha(at(s, i)) {
                ftype = Ftype::Special;
                append(&mut f, s[i].to_ascii_lowercase())?;
                i += 1;
                while isalpha(at(s, i)) {
                    append(&mut f, s[i].to_ascii_lowercase())?;
                    i += 1;
                }
            } else {
                return None;
            }
        } else if ispunct(c) {
            i += 1;
            continue;
        } else {
            return None;
        }
        // The terminator, written unchecked.
        used += 1;
        fields.push(Field { ftype, text: f });
    }
    Some(fields)
}

// ---------------------------------------------------------------------------
// The pieces `DecodeDateTime` and `DecodeTimeOnly` share.

/// What a pass is read under: a major's grammar and a `DateOrder`.
#[derive(Clone, Copy, Debug)]
struct Server {
    major: Major,
    order: DateOrder,
}

/// `ParseFraction` of `s`, which opens with its point: v13 and v14 read it
/// by `strtod` alone, refusing a bare point, v15 reads a bare point as zero,
/// and v18 refuses anything but digits after it.
fn parse_fraction(major: Major, s: &[u8]) -> Dt<f64> {
    if major >= Major::V15 && s.len() == 1 {
        return Ok(0.0);
    }
    if major >= Major::V18 && !s[1..].iter().all(|&c| isdigit(c)) {
        return Err(BAD);
    }
    let (value, end, erange) = strtod(s, 0);
    if end != s.len() || erange {
        return Err(BAD);
    }
    Ok(value)
}

/// `ParseFractionalSecond`: [`parse_fraction`] in microseconds.
fn parse_fractional_second(major: Major, s: &[u8]) -> Dt<i32> {
    Ok(c_int(rint(parse_fraction(major, s)? * 1_000_000.0)))
}

/// `DecodeTimezone`: the zone `*tzp` takes, seconds west.
fn decode_timezone(s: &[u8]) -> Dt<i32> {
    if !matches!(at(s, 0), b'+' | b'-') {
        return Err(BAD);
    }
    let (mut hr, mut cp, erange) = strtoint(s, 1);
    if erange {
        return Err(OVER);
    }
    let mut sec = 0;
    let min;
    if at(s, cp) == b':' {
        let (value, end, erange) = strtoint(s, cp + 1);
        if erange {
            return Err(OVER);
        }
        (min, cp) = (value, end);
        if at(s, cp) == b':' {
            let (value, end, erange) = strtoint(s, cp + 1);
            if erange {
                return Err(OVER);
            }
            (sec, cp) = (value, end);
        }
    } else if at(s, cp) == 0 && s.len() > 3 {
        min = hr % 100;
        hr /= 100;
    } else {
        min = 0;
    }
    if !(0..=15).contains(&hr) || !(0..60).contains(&min) || !(0..60).contains(&sec) {
        return Err(OVER);
    }
    let mut tz = (hr * 60 + min) * 60 + sec;
    if s[0] == b'-' {
        tz = -tz;
    }
    if at(s, cp) != 0 {
        return Err(BAD);
    }
    Ok(-tz)
}

/// `DecodeTimeCommon` from v15, `DecodeTime` before it: `hh:mm[:ss[.f]]`,
/// or `mm:ss.f`, as hours, minutes, seconds and microseconds — the hour an
/// `int` before v15.
fn decode_time_common(major: Major, s: &[u8], range: i32) -> Dt<(i64, i32, i32, i32)> {
    let (hour, mut cp, erange) = if major >= Major::V15 {
        strtol(s, 0)
    } else {
        let (hour, cp, erange) = strtoint(s, 0);
        (i64::from(hour), cp, erange)
    };
    if erange {
        return Err(OVER);
    }
    if at(s, cp) != b':' {
        return Err(BAD);
    }
    let (mut min, end, erange) = strtoint(s, cp + 1);
    if erange {
        return Err(OVER);
    }
    cp = end;
    let (mut hour, mut sec, mut fsec) = (hour, 0i32, 0i32);
    // Before v15 the swap needs no range check, the hour being an `int`.
    let fits = |hour: i64| major < Major::V15 || i32::try_from(hour).is_ok();
    if at(s, cp) == 0 {
        if range == interval_mask(MINUTE) | interval_mask(SECOND) {
            if !fits(hour) {
                return Err(OVER);
            }
            (sec, min, hour) = (min, hour as i32, 0);
        }
    } else if at(s, cp) == b'.' {
        fsec = parse_fractional_second(major, &s[cp..])?;
        if !fits(hour) {
            return Err(OVER);
        }
        (sec, min, hour) = (min, hour as i32, 0);
    } else if at(s, cp) == b':' {
        let (value, end, erange) = strtoint(s, cp + 1);
        if erange {
            return Err(OVER);
        }
        (sec, cp) = (value, end);
        if at(s, cp) == b'.' {
            fsec = parse_fractional_second(major, &s[cp..])?;
        } else if at(s, cp) != 0 {
            return Err(BAD);
        }
    } else {
        return Err(BAD);
    }
    if hour < 0
        || !(0..60).contains(&min)
        || !(0..=60).contains(&sec)
        || !(0..=1_000_000).contains(&fsec)
    {
        return Err(OVER);
    }
    Ok((hour, min, sec, fsec))
}

/// `DecodeTime`, for a date or a time: [`decode_time_common`], its hour held
/// to `int`.
fn decode_time(major: Major, s: &[u8], tm: &mut Tm, fsec: &mut i32) -> Dt<u32> {
    let (hour, min, sec, usec) = decode_time_common(major, s, INTERVAL_FULL_RANGE)?;
    if hour > i64::from(i32::MAX) {
        return Err(OVER);
    }
    (tm.hour, tm.min, tm.sec, *fsec) = (hour as i32, min, sec, usec);
    Ok(TIME_M)
}

/// `DecodeNumber`: a plain number read in context, its mask.
#[allow(clippy::too_many_arguments)]
fn decode_number(
    server: Server,
    s: &[u8],
    have_text_month: bool,
    fmask: u32,
    tm: &mut Tm,
    fsec: &mut i32,
    is2digits: &mut bool,
) -> Dt<u32> {
    let flen = s.len();
    let (val, cp, erange) = strtoint(s, 0);
    if erange {
        return Err(OVER);
    }
    if cp == 0 {
        return Err(BAD);
    }
    if at(s, cp) == b'.' {
        if cp > 2 {
            return decode_number_field(server.major, s, fmask | DATE_M, tm, fsec, is2digits);
        }
        *fsec = parse_fractional_second(server.major, &s[cp..])?;
    } else if at(s, cp) != 0 {
        return Err(BAD);
    }
    if flen == 3 && fmask & DATE_M == m(YEAR) && (1..=366).contains(&val) {
        tm.yday = val;
        return Ok(m(DOY) | m(MONTH) | m(DAY));
    }
    let order = server.order;
    let tmask = match fmask & DATE_M {
        0 if flen >= 3 || order == DateOrder::Ymd => {
            tm.year = val;
            m(YEAR)
        }
        0 if order == DateOrder::Dmy => {
            tm.mday = val;
            m(DAY)
        }
        0 => {
            tm.mon = val;
            m(MONTH)
        }
        x if x == m(YEAR) => {
            tm.mon = val;
            m(MONTH)
        }
        x if x == m(MONTH) && have_text_month && (flen >= 3 || order == DateOrder::Ymd) => {
            tm.year = val;
            m(YEAR)
        }
        x if x == m(MONTH) => {
            tm.mday = val;
            m(DAY)
        }
        x if x == m(YEAR) | m(MONTH) && have_text_month && flen >= 3 && *is2digits => {
            tm.mday = tm.year;
            tm.year = val;
            *is2digits = false;
            m(DAY)
        }
        x if x == m(YEAR) | m(MONTH) => {
            tm.mday = val;
            m(DAY)
        }
        x if x == m(DAY) => {
            tm.mon = val;
            m(MONTH)
        }
        x if x == m(MONTH) | m(DAY) => {
            tm.year = val;
            m(YEAR)
        }
        DATE_M => return decode_number_field(server.major, s, fmask, tm, fsec, is2digits),
        _ => return Err(BAD),
    };
    if tmask == m(YEAR) {
        *is2digits = flen <= 2;
    }
    Ok(tmask)
}

/// `DecodeNumberField`: a run-together date or time, its mask. From v18 a
/// text of anything but digits and points is refused; before, `atoi` reads
/// whatever it holds.
fn decode_number_field(
    major: Major,
    s: &[u8],
    fmask: u32,
    tm: &mut Tm,
    fsec: &mut i32,
    is2digits: &mut bool,
) -> Dt<u32> {
    if major >= Major::V18 && !s.iter().all(|&c| isdigit(c) || c == b'.') {
        return Err(BAD);
    }
    let mut digits = s;
    if let Some(cp) = s.iter().position(|&c| c == b'.') {
        *fsec = if major >= Major::V18 {
            parse_fractional_second(major, &s[cp..])?
        } else if major >= Major::V15 && cp + 1 == s.len() {
            0
        } else {
            // `strtod` with no end pointer: only its `ERANGE` refuses.
            let (frac, _, erange) = strtod(s, cp);
            if erange {
                return Err(BAD);
            }
            c_int(rint(frac * 1_000_000.0))
        };
        digits = &s[..cp];
    } else if fmask & DATE_M != DATE_M && s.len() >= 6 {
        let len = s.len();
        tm.mday = atoi(&s[len - 2..]);
        tm.mon = atoi(&s[len - 4..len - 2]);
        tm.year = atoi(&s[..len - 4]);
        if len - 4 == 2 {
            *is2digits = true;
        }
        return Ok(DATE_M);
    }
    if fmask & TIME_M != TIME_M {
        match digits.len() {
            6 => {
                tm.sec = atoi(&digits[4..]);
                tm.min = atoi(&digits[2..4]);
                tm.hour = atoi(&digits[..2]);
                return Ok(TIME_M);
            }
            4 => {
                tm.sec = 0;
                tm.min = atoi(&digits[2..]);
                tm.hour = atoi(&digits[..2]);
                return Ok(TIME_M);
            }
            _ => {}
        }
    }
    Err(BAD)
}

/// `DecodeDate`: a date with delimiters, its mask. A run of digits or of
/// letters is a field, and the byte after each run is overwritten whatever
/// it is, so `12jan` reads as `12` and `an`.
fn decode_date(server: Server, s: &[u8], fmask: u32, is2digits: &mut bool, tm: &mut Tm) -> Dt<u32> {
    let mut fmask = fmask;
    let mut tmask = 0u32;
    let mut have_text_month = false;
    let mut parts: Vec<Option<&[u8]>> = Vec::new();
    let mut i = 0;
    while i < s.len() && parts.len() < MAXDATEFIELDS {
        while i < s.len() && !isalnum(s[i]) {
            i += 1;
        }
        if i >= s.len() {
            return Err(BAD);
        }
        let start = i;
        if isdigit(s[i]) {
            while isdigit(at(s, i)) {
                i += 1;
            }
        } else {
            while isalpha(at(s, i)) {
                i += 1;
            }
        }
        parts.push(Some(&s[start..i]));
        if i < s.len() {
            i += 1;
        }
    }
    for part in parts.iter_mut() {
        let Some(text) = *part else { continue };
        if !isalpha(text[0]) {
            continue;
        }
        let (ty, value) = keyword(server.major, text).unwrap_or((UNKNOWN_FIELD, 0));
        if ty == IGNORE_DTF {
            continue;
        }
        if ty != MONTH {
            return Err(BAD);
        }
        tm.mon = value;
        have_text_month = true;
        if fmask & m(MONTH) != 0 {
            return Err(BAD);
        }
        fmask |= m(MONTH);
        tmask |= m(MONTH);
        *part = None;
    }
    for text in parts.into_iter().flatten() {
        let mut fsec = 0;
        let dmask = decode_number(server, text, have_text_month, fmask, tm, &mut fsec, is2digits)?;
        if fmask & dmask != 0 {
            return Err(BAD);
        }
        fmask |= dmask;
        tmask |= dmask;
    }
    if fmask & !(m(DOY) | m(TZ)) != DATE_M {
        return Err(BAD);
    }
    Ok(tmask)
}

/// `ValidateDate`.
fn validate_date(fmask: u32, isjulian: bool, is2digits: bool, bc: bool, tm: &mut Tm) -> Dt<()> {
    if fmask & m(YEAR) != 0 {
        if isjulian {
        } else if bc {
            if tm.year <= 0 {
                return Err(OVER);
            }
            tm.year = -(tm.year - 1);
        } else if is2digits {
            if tm.year < 0 {
                return Err(OVER);
            }
            if tm.year < 70 {
                tm.year += 2000;
            } else if tm.year < 100 {
                tm.year += 1900;
            }
        } else if tm.year <= 0 {
            return Err(OVER);
        }
    }
    if fmask & m(DOY) != 0 {
        let jd = date2j(tm.year, 1, 1).wrapping_add(tm.yday).wrapping_sub(1);
        (tm.year, tm.mon, tm.mday) = j2date(jd);
    }
    if fmask & m(MONTH) != 0 && !(1..=12).contains(&tm.mon) {
        return Err(OVER);
    }
    if fmask & m(DAY) != 0 && !(1..=31).contains(&tm.mday) {
        return Err(OVER);
    }
    if fmask & DATE_M == DATE_M
        && tm.mday > DAY_TAB[usize::from(isleap(tm.year))][(tm.mon - 1) as usize]
    {
        return Err(OVER);
    }
    Ok(())
}

/// The AM/PM check and adjustment both decoders make after
/// [`validate_date`].
fn apply_meridian(mer: i32, tm: &mut Tm) -> Dt<()> {
    if mer != HR24 && tm.hour > 12 {
        return Err(OVER);
    }
    if mer == AM && tm.hour == 12 {
        tm.hour = 0;
    } else if mer == PM && tm.hour != 12 {
        tm.hour = tm.hour.wrapping_add(12);
    }
    Ok(())
}

/// `time_overflows`.
fn time_overflows(hour: i32, min: i32, sec: i32, fsec: i32) -> bool {
    if !(0..=24).contains(&hour)
        || !(0..60).contains(&min)
        || !(0..=60).contains(&sec)
        || !(0..=1_000_000).contains(&fsec)
    {
        return true;
    }
    i64::from((hour * 60 + min) * 60 + sec) * USECS_PER_SEC + i64::from(fsec) > USECS_PER_DAY
}

/// The Julian day a `j` prefix labels: `value`, and a fraction of a day
/// where `s` goes on past `cp`. Its mask.
fn julian_day(
    major: Major,
    value: i32,
    s: &[u8],
    cp: usize,
    tm: &mut Tm,
    fsec: &mut i32,
) -> Dt<u32> {
    if value < 0 {
        return Err(OVER);
    }
    let mut tmask = DATE_M;
    (tm.year, tm.mon, tm.mday) = j2date(value);
    if at(s, cp) == b'.' {
        let time = parse_fraction(major, &s[cp..])? * USECS_PER_DAY as f64;
        // `dt2time`.
        let mut time = c_int64(time);
        tm.hour = (time / USECS_PER_HOUR) as i32;
        time -= i64::from(tm.hour) * USECS_PER_HOUR;
        tm.min = (time / USECS_PER_MINUTE) as i32;
        time -= i64::from(tm.min) * USECS_PER_MINUTE;
        tm.sec = (time / USECS_PER_SEC) as i32;
        *fsec = (time - i64::from(tm.sec) * USECS_PER_SEC) as i32;
        tmask |= TIME_M;
    }
    Ok(tmask)
}

/// `pg_tzset` of a zone named with punctuation: a file of that name under
/// the server's zone directory, searched a `/`-separated level at a time with
/// every hidden name skipped, so a level that is empty or opens with `.`
/// names none — or a POSIX zone spec, which `tzparse` reads.
fn zone_name(name: &[u8]) -> Dt<bool> {
    let file = !name.split(|&c| c == b'/').any(|level| level.first().is_none_or(|&c| c == b'.'));
    if file || posix_zone(name) { Ok(true) } else { Err(BAD) }
}

/// Whether `tzparse` reads `s`, a token holding no `<`, `,` or `;`: a name
/// running to a digit or a sign, an offset of up to 167 hours, then
/// optionally a second name and offset, whose missing rule is the default.
fn posix_zone(s: &[u8]) -> bool {
    let name = |i: usize| {
        let mut j = i;
        while j < s.len() && !(isdigit(s[j]) || matches!(s[j], b',' | b'-' | b'+')) {
            j += 1;
        }
        j
    };
    // `getnum`: digits, refused once past `max`.
    let number = |i: usize, max: u32| -> Option<usize> {
        if !isdigit(at(s, i)) {
            return None;
        }
        let (mut value, mut j) = (0u32, i);
        while isdigit(at(s, j)) {
            value = value * 10 + u32::from(s[j] - b'0');
            if value > max {
                return None;
            }
            j += 1;
        }
        Some(j)
    };
    // `getoffset`.
    let offset = |i: usize| -> Option<usize> {
        let i = i + usize::from(matches!(at(s, i), b'+' | b'-'));
        let mut j = number(i, 167)?;
        if at(s, j) == b':' {
            j = number(j + 1, 59)?;
            if at(s, j) == b':' {
                j = number(j + 1, 60)?;
            }
        }
        Some(j)
    };
    let i = name(0);
    if i == s.len() {
        return false;
    }
    let Some(i) = offset(i) else { return false };
    if i == s.len() {
        return true;
    }
    let j = name(i);
    if j == i {
        return false;
    }
    if j == s.len() {
        return true;
    }
    offset(j) == Some(s.len())
}

/// What a decode leaves for its input function.
#[derive(Clone, Copy, Debug)]
struct Decoded {
    dtype: i32,
    tm: Tm,
    fsec: i32,
    zone: Zone,
}

/// A word's reading: its keyword, or a zone abbreviation where it names no
/// keyword or `shadowed` is it (a fixed one, [`Zone::Unknown`] its offset).
fn word(major: Major, text: &[u8], shadowed: bool) -> (i32, i32) {
    match keyword(major, text) {
        Some(found) if !shadowed => found,
        _ => (TZ, 0),
    }
}

/// A labelled number before v16: the field a `y`, `m`, `d`, `h`, `mm` or `s`
/// prefix names, `value` read into it. Its mask.
#[allow(clippy::too_many_arguments)]
fn labelled_number_v13(
    major: Major,
    ptype: i32,
    value: i32,
    s: &[u8],
    cp: usize,
    fmask: u32,
    tm: &mut Tm,
    fsec: &mut i32,
    zone: &mut Zone,
) -> Dt<Option<u32>> {
    Ok(Some(match ptype {
        DTK_YEAR => {
            tm.year = value;
            m(YEAR)
        }
        DTK_MONTH if fmask & m(MONTH) != 0 && fmask & m(HOUR) != 0 => {
            tm.min = value;
            m(MINUTE)
        }
        DTK_MONTH => {
            tm.mon = value;
            m(MONTH)
        }
        DTK_DAY => {
            tm.mday = value;
            m(DAY)
        }
        DTK_HOUR => {
            tm.hour = value;
            m(HOUR)
        }
        DTK_MINUTE => {
            tm.min = value;
            m(MINUTE)
        }
        DTK_SECOND => {
            tm.sec = value;
            if at(s, cp) == b'.' {
                *fsec = parse_fractional_second(major, &s[cp..])?;
                ALL_SECS_M
            } else {
                m(SECOND)
            }
        }
        DTK_TZ => {
            *zone = Zone::Known(decode_timezone(s)?);
            m(TZ)
        }
        _ => return Ok(None),
    }))
}

// ---------------------------------------------------------------------------
// DecodeDateTime and DecodeTimeOnly.

/// `DecodeDateTime`, `shadowed` naming the keyword read as a zone
/// abbreviation, if any. A time with no date, which it returns as `1`, is an
/// error to every caller.
fn decode_date_time(fields: &[Field], server: Server, shadowed: Option<usize>) -> Dt<Decoded> {
    let major = server.major;
    let v16 = major >= Major::V16;
    let nf = fields.len();
    let mut fmask = 0u32;
    let mut ptype = 0;
    let mut dtype = DTK_DATE;
    let mut tm = Tm::default();
    let mut fsec = 0i32;
    let mut zone = Zone::Known(0);
    let mut mer = HR24;
    let (mut have_text_month, mut isjulian, mut is2digits, mut bc) = (false, false, false, false);
    let mut named_zone = false;
    for (i, field) in fields.iter().enumerate() {
        let f = &field.text[..];
        let tmask: u32;
        match field.ftype {
            Ftype::Date => {
                if ptype == DTK_JULIAN {
                    let (jday, cp, erange) = strtoint(f, 0);
                    if erange || jday < 0 {
                        return Err(OVER);
                    }
                    (tm.year, tm.mon, tm.mday) = j2date(jday);
                    isjulian = true;
                    zone = Zone::Known(decode_timezone(&f[cp..])?);
                    tmask = DATE_M | TIME_M | m(TZ);
                    ptype = 0;
                } else if ptype != 0 || fmask & (m(MONTH) | m(DAY)) == m(MONTH) | m(DAY) {
                    if isdigit(at(f, 0)) || ptype != 0 {
                        if ptype != 0 {
                            if ptype != DTK_TIME {
                                return Err(BAD);
                            }
                            ptype = 0;
                        }
                        if fmask & TIME_M == TIME_M {
                            return Err(BAD);
                        }
                        let cp = f.iter().position(|&c| c == b'-').ok_or(BAD)?;
                        zone = Zone::Known(decode_timezone(&f[cp..])?);
                        tmask = decode_number_field(
                            major,
                            &f[..cp],
                            fmask,
                            &mut tm,
                            &mut fsec,
                            &mut is2digits,
                        )? | m(TZ);
                    } else {
                        named_zone = zone_name(f)?;
                        zone = Zone::Unknown;
                        tmask = m(TZ);
                    }
                } else {
                    tmask = decode_date(server, f, fmask, &mut is2digits, &mut tm)?;
                }
            }
            Ftype::Time => {
                if ptype != 0 {
                    if ptype != DTK_TIME {
                        return Err(BAD);
                    }
                    ptype = 0;
                }
                tmask = decode_time(major, f, &mut tm, &mut fsec)?;
                if time_overflows(tm.hour, tm.min, tm.sec, fsec) {
                    return Err(OVER);
                }
            }
            Ftype::Tz => {
                zone = Zone::Known(decode_timezone(f)?);
                tmask = m(TZ);
            }
            Ftype::Number if ptype != 0 => {
                let (value, cp, erange) = strtoint(f, 0);
                if erange {
                    return Err(OVER);
                }
                if at(f, cp) == b'.' {
                    if !v16 && !matches!(ptype, DTK_JULIAN | DTK_TIME | DTK_SECOND) {
                        return Err(BAD);
                    }
                } else if at(f, cp) != 0 {
                    return Err(BAD);
                }
                tmask = match ptype {
                    DTK_JULIAN => {
                        isjulian = true;
                        julian_day(major, value, f, cp, &mut tm, &mut fsec)?
                    }
                    DTK_TIME => {
                        let tmask = decode_number_field(
                            major,
                            f,
                            fmask | DATE_M,
                            &mut tm,
                            &mut fsec,
                            &mut is2digits,
                        )?;
                        if tmask != TIME_M {
                            return Err(BAD);
                        }
                        tmask
                    }
                    _ if v16 => return Err(BAD),
                    _ => labelled_number_v13(
                        major, ptype, value, f, cp, fmask, &mut tm, &mut fsec, &mut zone,
                    )?
                    .ok_or(BAD)?,
                };
                ptype = 0;
                dtype = DTK_DATE;
            }
            Ftype::Number => {
                let flen = f.len();
                let point = f.iter().position(|&c| c == b'.');
                tmask = match point {
                    Some(_) if fmask & DATE_M == 0 => {
                        decode_date(server, f, fmask, &mut is2digits, &mut tm)?
                    }
                    Some(cp) if cp > 2 => {
                        decode_number_field(major, f, fmask, &mut tm, &mut fsec, &mut is2digits)?
                    }
                    _ if flen >= 6 && (fmask & DATE_M == 0 || fmask & TIME_M == 0) => {
                        decode_number_field(major, f, fmask, &mut tm, &mut fsec, &mut is2digits)?
                    }
                    _ => decode_number(
                        server,
                        f,
                        have_text_month,
                        fmask,
                        &mut tm,
                        &mut fsec,
                        &mut is2digits,
                    )?,
                };
            }
            Ftype::String | Ftype::Special => {
                let (ty, value) = word(major, f, shadowed == Some(i));
                if ty == IGNORE_DTF {
                    continue;
                }
                let mut mask = m(ty);
                match ty {
                    RESERV => match value {
                        DTK_NOW => {
                            mask = DATE_M | TIME_M | m(TZ);
                            dtype = DTK_DATE;
                            tm = NOW;
                            fsec = 0;
                            zone = Zone::Unknown;
                        }
                        DTK_YESTERDAY | DTK_TODAY | DTK_TOMORROW => {
                            mask = DATE_M;
                            dtype = DTK_DATE;
                            let shift = value - DTK_TODAY;
                            (tm.year, tm.mon, tm.mday) =
                                j2date(date2j(NOW.year, NOW.mon, NOW.mday) + shift);
                        }
                        DTK_ZULU => {
                            mask = TIME_M | m(TZ);
                            dtype = DTK_DATE;
                            (tm.hour, tm.min, tm.sec) = (0, 0, 0);
                            zone = Zone::Known(0);
                        }
                        // `epoch` and the infinities. Before v16 the mask
                        // stays the keyword's own.
                        _ => {
                            if v16 {
                                mask = DATE_M | TIME_M | m(TZ);
                            }
                            dtype = value;
                        }
                    },
                    MONTH => {
                        if fmask & m(MONTH) != 0
                            && !have_text_month
                            && fmask & m(DAY) == 0
                            && (1..=31).contains(&tm.mon)
                        {
                            tm.mday = tm.mon;
                            mask = m(DAY);
                        }
                        have_text_month = true;
                        tm.mon = value;
                    }
                    DTZMOD => {
                        mask |= m(DTZ);
                        if let Zone::Known(tz) = zone {
                            zone = Zone::Known(tz.wrapping_sub(value));
                        }
                    }
                    TZ => zone = Zone::Unknown,
                    AMPM => mer = value,
                    ADBC => bc = value == BC,
                    DOW => {}
                    UNITS => {
                        mask = 0;
                        if v16 && ptype != 0 {
                            return Err(BAD);
                        }
                        ptype = value;
                    }
                    ISOTIME => {
                        mask = 0;
                        if fmask & DATE_M != DATE_M {
                            return Err(BAD);
                        }
                        if v16 {
                            if ptype != 0 {
                                return Err(BAD);
                            }
                        } else if i + 1 >= nf
                            || !matches!(
                                fields[i + 1].ftype,
                                Ftype::Number | Ftype::Time | Ftype::Date
                            )
                        {
                            return Err(BAD);
                        }
                        ptype = value;
                    }
                    _ => return Err(BAD),
                }
                tmask = mask;
            }
        }
        if tmask & fmask != 0 {
            return Err(BAD);
        }
        fmask |= tmask;
    }
    if v16 && ptype != 0 {
        return Err(BAD);
    }
    if !v16 {
        validate_date(fmask, isjulian, is2digits, bc, &mut tm)?;
        apply_meridian(mer, &mut tm)?;
    }
    if dtype == DTK_DATE {
        if v16 {
            validate_date(fmask, isjulian, is2digits, bc, &mut tm)?;
            apply_meridian(mer, &mut tm)?;
        }
        if fmask & DATE_M != DATE_M {
            return Err(BAD);
        }
        if named_zone && fmask & m(DTZMOD) != 0 {
            return Err(BAD);
        }
        if fmask & m(TZ) == 0 {
            if fmask & m(DTZMOD) != 0 {
                return Err(BAD);
            }
            zone = Zone::Unknown;
        }
    }
    Ok(Decoded { dtype, tm, fsec, zone })
}

/// `DecodeTimeOnly`, `shadowed` as [`decode_date_time`]'s. A zone the text
/// names is taken as one with no daylight-saving time, which needs no date.
fn decode_time_only(fields: &[Field], server: Server, shadowed: Option<usize>) -> Dt<()> {
    let major = server.major;
    let v16 = major >= Major::V16;
    let nf = fields.len();
    let mut fmask = 0u32;
    let mut ptype = 0;
    let mut tm = Tm::default();
    let mut fsec = 0i32;
    let mut zone = Zone::Known(0);
    let mut mer = HR24;
    let (mut isjulian, mut is2digits, mut bc) = (false, false, false);
    let mut named_zone = false;
    for (i, field) in fields.iter().enumerate() {
        let f = &field.text[..];
        let tmask: u32;
        match field.ftype {
            Ftype::Date => {
                if i == 0
                    && nf >= 2
                    && (fields[nf - 1].ftype == Ftype::Date || fields[1].ftype == Ftype::Time)
                {
                    tmask = decode_date(server, f, fmask, &mut is2digits, &mut tm)?;
                } else if isdigit(at(f, 0)) {
                    if fmask & TIME_M == TIME_M {
                        return Err(BAD);
                    }
                    let cp = f.iter().position(|&c| c == b'-').ok_or(BAD)?;
                    zone = Zone::Known(decode_timezone(&f[cp..])?);
                    tmask = decode_number_field(
                        major,
                        &f[..cp],
                        fmask | DATE_M,
                        &mut tm,
                        &mut fsec,
                        &mut is2digits,
                    )? | m(TZ);
                } else {
                    named_zone = zone_name(f)?;
                    zone = Zone::Unknown;
                    tmask = m(TZ);
                }
            }
            Ftype::Time => {
                if v16 && ptype != 0 {
                    if ptype != DTK_TIME {
                        return Err(BAD);
                    }
                    ptype = 0;
                }
                tmask = decode_time(major, f, &mut tm, &mut fsec)?;
            }
            Ftype::Tz => {
                zone = Zone::Known(decode_timezone(f)?);
                tmask = m(TZ);
            }
            Ftype::Number if ptype != 0 => {
                let (value, cp, erange) = strtoint(f, 0);
                if erange {
                    return Err(OVER);
                }
                if at(f, cp) == b'.' {
                    if !v16 && !matches!(ptype, DTK_JULIAN | DTK_TIME | DTK_SECOND) {
                        return Err(BAD);
                    }
                } else if at(f, cp) != 0 {
                    return Err(BAD);
                }
                tmask = match ptype {
                    DTK_JULIAN => {
                        isjulian = true;
                        julian_day(major, value, f, cp, &mut tm, &mut fsec)?
                    }
                    DTK_TIME => {
                        let tmask = decode_number_field(
                            major,
                            f,
                            fmask | DATE_M,
                            &mut tm,
                            &mut fsec,
                            &mut is2digits,
                        )?;
                        if tmask != TIME_M {
                            return Err(BAD);
                        }
                        tmask
                    }
                    _ if v16 => return Err(BAD),
                    _ => labelled_number_v13(
                        major, ptype, value, f, cp, fmask, &mut tm, &mut fsec, &mut zone,
                    )?
                    .ok_or(BAD)?,
                };
                ptype = 0;
            }
            Ftype::Number => {
                let flen = f.len();
                tmask = match f.iter().position(|&c| c == b'.') {
                    Some(_) if i == 0 && nf >= 2 && fields[nf - 1].ftype == Ftype::Date => {
                        decode_date(server, f, fmask, &mut is2digits, &mut tm)?
                    }
                    Some(cp) if cp > 2 => decode_number_field(
                        major,
                        f,
                        fmask | DATE_M,
                        &mut tm,
                        &mut fsec,
                        &mut is2digits,
                    )?,
                    Some(_) => return Err(BAD),
                    None if flen > 4 => decode_number_field(
                        major,
                        f,
                        fmask | DATE_M,
                        &mut tm,
                        &mut fsec,
                        &mut is2digits,
                    )?,
                    None => decode_number(
                        server,
                        f,
                        false,
                        fmask | DATE_M,
                        &mut tm,
                        &mut fsec,
                        &mut is2digits,
                    )?,
                };
            }
            Ftype::String | Ftype::Special => {
                let (ty, value) = word(major, f, shadowed == Some(i));
                if ty == IGNORE_DTF {
                    continue;
                }
                let mut mask = m(ty);
                match ty {
                    RESERV => match value {
                        DTK_NOW => {
                            mask = TIME_M;
                            tm = NOW;
                            fsec = 0;
                        }
                        DTK_ZULU => {
                            mask = TIME_M | m(TZ);
                            (tm.hour, tm.min, tm.sec) = (0, 0, 0);
                        }
                        _ => return Err(BAD),
                    },
                    DTZMOD => {
                        mask |= m(DTZ);
                        if let Zone::Known(tz) = zone {
                            zone = Zone::Known(tz.wrapping_sub(value));
                        }
                    }
                    TZ => zone = Zone::Unknown,
                    AMPM => mer = value,
                    ADBC => bc = value == BC,
                    UNITS | ISOTIME => {
                        mask = 0;
                        if v16 {
                            if ptype != 0 {
                                return Err(BAD);
                            }
                        } else if ty == ISOTIME
                            && (i + 1 >= nf
                                || !matches!(
                                    fields[i + 1].ftype,
                                    Ftype::Number | Ftype::Time | Ftype::Date
                                ))
                        {
                            return Err(BAD);
                        }
                        ptype = value;
                    }
                    _ => return Err(BAD),
                }
                tmask = mask;
            }
        }
        if tmask & fmask != 0 {
            return Err(BAD);
        }
        fmask |= tmask;
    }
    if v16 && ptype != 0 {
        return Err(BAD);
    }
    validate_date(fmask, isjulian, is2digits, bc, &mut tm)?;
    apply_meridian(mer, &mut tm)?;
    if time_overflows(tm.hour, tm.min, tm.sec, fsec) {
        return Err(OVER);
    }
    if fmask & TIME_M != TIME_M {
        return Err(BAD);
    }
    if named_zone && fmask & m(DTZMOD) != 0 {
        return Err(BAD);
    }
    if fmask & m(TZ) == 0 {
        if fmask & m(DTZMOD) != 0 {
            return Err(BAD);
        }
        if fmask & DATE_M != 0 && fmask & DATE_M != DATE_M {
            return Err(BAD);
        }
    }
    // No zone decides whether `time_in` or `timetz_in` reads a time.
    let _ = zone;
    Ok(())
}

/// `tm2timestamp`: whether the decoded value is a timestamp, the zone
/// applied where `zone` is given. Before v18 a product past `int64` is
/// caught by dividing back, and a time-of-day carrying a date across 2000's
/// epoch is refused; from v18 the arithmetic is checked.
fn tm2timestamp(major: Major, tm: &Tm, fsec: i32, zone: Option<Zone>) -> bool {
    if !is_valid_julian(tm.year, tm.mon) {
        return false;
    }
    let date = i64::from(date2j(tm.year, tm.mon, tm.mday)) - i64::from(POSTGRES_EPOCH_JDATE);
    // `time2t`, its seconds in `int`.
    let seconds =
        tm.hour.wrapping_mul(60).wrapping_add(tm.min).wrapping_mul(60).wrapping_add(tm.sec);
    let time = i64::from(seconds) * USECS_PER_SEC + i64::from(fsec);
    let result = if major >= Major::V18 {
        match date.checked_mul(USECS_PER_DAY).and_then(|r| r.checked_add(time)) {
            Some(result) => result,
            None => return false,
        }
    } else {
        let result = date.wrapping_mul(USECS_PER_DAY).wrapping_add(time);
        if result.wrapping_sub(time) / USECS_PER_DAY != date {
            return false;
        }
        if (result < 0 && date > 0) || (result > 0 && date < -1) {
            return false;
        }
        result
    };
    let valid = |t: i64| (MIN_TIMESTAMP..END_TIMESTAMP).contains(&t);
    match zone {
        None => valid(result),
        // `dt2local(result, -tz)`.
        Some(Zone::Known(tz)) => {
            valid(result.wrapping_sub(i64::from(tz.wrapping_neg()).wrapping_mul(USECS_PER_SEC)))
        }
        Some(Zone::Unknown) => {
            let result = i128::from(result);
            let bound = i128::from(ZONE_OFFSET_BOUND);
            result + bound >= i128::from(MIN_TIMESTAMP)
                && result - bound < i128::from(END_TIMESTAMP)
        }
    }
}

/// One pass of an input function over `fields`.
fn datetime_accepts(
    input: DateTimeInput,
    fields: &[Field],
    server: Server,
    shadowed: Option<usize>,
) -> bool {
    let decoded = match input {
        DateTimeInput::Time | DateTimeInput::TimeTz => {
            return decode_time_only(fields, server, shadowed).is_ok();
        }
        _ => match decode_date_time(fields, server, shadowed) {
            Ok(decoded) => decoded,
            Err(_) => return false,
        },
    };
    let Decoded { dtype, tm, fsec, zone } = decoded;
    match (input, dtype) {
        (_, DTK_LATE | DTK_EARLY | DTK_EPOCH) => true,
        (DateTimeInput::Date, DTK_DATE) => {
            if !is_valid_julian(tm.year, tm.mon) {
                return false;
            }
            let date = date2j(tm.year, tm.mon, tm.mday).wrapping_sub(POSTGRES_EPOCH_JDATE);
            (-POSTGRES_EPOCH_JDATE..DATE_END_JULIAN - POSTGRES_EPOCH_JDATE).contains(&date)
        }
        (DateTimeInput::Timestamp, DTK_DATE) => tm2timestamp(server.major, &tm, fsec, None),
        (DateTimeInput::TimestampTz, DTK_DATE) => tm2timestamp(server.major, &tm, fsec, Some(zone)),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// DecodeInterval and DecodeISO8601Interval from v15.

/// `pg_itm_in`.
#[derive(Clone, Copy, Default, Debug)]
struct Itm {
    usec: i64,
    mday: i32,
    mon: i32,
    year: i32,
}

/// `int64_multiply_add`; a sum that overflows is left wrapped, as
/// `pg_add_s64_overflow` leaves it.
fn int64_multiply_add(val: i64, multiplier: i64, sum: &mut i64) -> bool {
    let Some(product) = val.checked_mul(multiplier) else { return false };
    let (total, overflowed) = sum.overflowing_add(product);
    *sum = total;
    !overflowed
}

fn adjust_fract_microseconds(frac: f64, scale: i64, itm: &mut Itm) -> bool {
    if frac == 0.0 {
        return true;
    }
    let frac = frac * scale as f64;
    let mut usec = c_int64(frac);
    let rest = frac - usec as f64;
    if rest > 0.5 {
        usec = usec.wrapping_add(1);
    } else if rest < -0.5 {
        usec = usec.wrapping_sub(1);
    }
    let (total, overflowed) = itm.usec.overflowing_add(usec);
    itm.usec = total;
    !overflowed
}

fn adjust_fract_days(frac: f64, scale: i32, itm: &mut Itm) -> bool {
    if frac == 0.0 {
        return true;
    }
    let frac = frac * f64::from(scale);
    let extra_days = c_int(frac);
    let Some(mday) = itm.mday.checked_add(extra_days) else { return false };
    itm.mday = mday;
    adjust_fract_microseconds(frac - f64::from(extra_days), USECS_PER_DAY, itm)
}

fn adjust_fract_years(frac: f64, scale: i32, itm: &mut Itm) -> bool {
    let extra_months = c_int(rint(frac * f64::from(scale) * 12.0));
    match itm.mon.checked_add(extra_months) {
        Some(mon) => {
            itm.mon = mon;
            true
        }
        None => false,
    }
}

fn adjust_microseconds(val: i64, fval: f64, scale: i64, itm: &mut Itm) -> bool {
    int64_multiply_add(val, scale, &mut itm.usec) && adjust_fract_microseconds(fval, scale, itm)
}

fn adjust_days(val: i64, scale: i32, itm: &mut Itm) -> bool {
    let Ok(val) = i32::try_from(val) else { return false };
    match val.checked_mul(scale).and_then(|days| itm.mday.checked_add(days)) {
        Some(mday) => {
            itm.mday = mday;
            true
        }
        None => false,
    }
}

fn adjust_months(val: i64, itm: &mut Itm) -> bool {
    match i32::try_from(val).ok().and_then(|val| itm.mon.checked_add(val)) {
        Some(mon) => {
            itm.mon = mon;
            true
        }
        None => false,
    }
}

fn adjust_years(val: i64, scale: i32, itm: &mut Itm) -> bool {
    let Ok(val) = i32::try_from(val) else { return false };
    match val.checked_mul(scale).and_then(|years| itm.year.checked_add(years)) {
        Some(year) => {
            itm.year = year;
            true
        }
        None => false,
    }
}

/// `DecodeTimeForInterval`.
fn decode_time_for_interval(major: Major, s: &[u8], range: i32, itm: &mut Itm) -> Dt<u32> {
    let (hour, min, sec, usec) = decode_time_common(major, s, range)?;
    itm.usec = i64::from(usec);
    if !int64_multiply_add(hour, USECS_PER_HOUR, &mut itm.usec)
        || !int64_multiply_add(i64::from(min), USECS_PER_MINUTE, &mut itm.usec)
        || !int64_multiply_add(i64::from(sec), USECS_PER_SEC, &mut itm.usec)
    {
        return Err(OVER);
    }
    Ok(TIME_M)
}

/// The unit a number with none takes, by the column's field qualifier.
fn implicit_unit(range: i32) -> i32 {
    let (y, mo, d, h, mi) = (
        interval_mask(YEAR),
        interval_mask(MONTH),
        interval_mask(DAY),
        interval_mask(HOUR),
        interval_mask(MINUTE),
    );
    match range {
        r if r == y => DTK_YEAR,
        r if r == mo || r == y | mo => DTK_MONTH,
        r if r == d => DTK_DAY,
        r if r == h || r == d | h => DTK_HOUR,
        r if r == mi || r == h | mi || r == d | h | mi => DTK_MINUTE,
        _ => DTK_SECOND,
    }
}

/// `DecodeInterval` from v15: the decoded value and its `dtype`.
fn decode_interval(
    fields: &[Field],
    major: Major,
    sql_standard: bool,
    range: i32,
) -> Dt<(Itm, i32)> {
    let v17 = major >= Major::V17;
    let nf = fields.len();
    let mut force_negative = false;
    let mut is_before = false;
    let mut parsing_unit_val = false;
    let mut fmask = 0u32;
    let mut ty = IGNORE_DTF;
    let mut dtype = DTK_DELTA;
    let mut itm = Itm::default();
    if sql_standard && nf > 0 && fields[0].text[0] == b'-' {
        force_negative = !fields[1..].iter().any(|f| matches!(f.text[0], b'-' | b'+'));
    }
    for i in (0..nf).rev() {
        let f = &fields[i].text[..];
        let mut tmask: u32;
        let mut number = false;
        match fields[i].ftype {
            Ftype::Time => {
                tmask = decode_time_for_interval(major, f, range, &mut itm)?;
                if force_negative && itm.usec > 0 {
                    itm.usec = -itm.usec;
                }
                ty = DTK_DAY;
                parsing_unit_val = false;
            }
            Ftype::Tz => {
                let mut timed = Itm { ..itm };
                if f[1..].contains(&b':')
                    && decode_time_for_interval(major, &f[1..], range, &mut timed).is_ok()
                {
                    itm = timed;
                    if f[0] == b'-' {
                        if itm.usec == i64::MIN {
                            return Err(OVER);
                        }
                        itm.usec = -itm.usec;
                    }
                    if force_negative && itm.usec > 0 {
                        itm.usec = -itm.usec;
                    }
                    ty = DTK_DAY;
                    parsing_unit_val = false;
                    tmask = TIME_M;
                } else {
                    // A time it failed to read leaves no field the number
                    // below reads: holding a `:`, that refuses as a format.
                    tmask = 0;
                    number = true;
                }
            }
            Ftype::Date | Ftype::Number => {
                tmask = 0;
                number = true;
            }
            Ftype::String | Ftype::Special => {
                if v17 && parsing_unit_val {
                    return Err(BAD);
                }
                let found = units(f).or_else(|| if v17 { keyword(major, f) } else { None });
                let uval;
                // The unit's type is written to the one `type` a bare number
                // reads, so a word ignored leaves the next number unlabelled.
                (ty, uval) = found.unwrap_or((UNKNOWN_FIELD, 0));
                if ty == IGNORE_DTF {
                    continue;
                }
                tmask = 0;
                match ty {
                    UNITS => {
                        ty = uval;
                        parsing_unit_val = true;
                    }
                    AGO => {
                        if v17 && i != nf - 1 {
                            return Err(BAD);
                        }
                        is_before = true;
                        ty = uval;
                    }
                    RESERV => {
                        tmask = DATE_M | TIME_M;
                        if v17 && ((uval != DTK_LATE && uval != DTK_EARLY) || i != nf - 1) {
                            return Err(BAD);
                        }
                        dtype = uval;
                    }
                    _ => return Err(BAD),
                }
            }
        }
        if number {
            if ty == IGNORE_DTF {
                ty = implicit_unit(range);
            }
            let (mut val, cp, erange) = strtol(f, 0);
            if erange {
                return Err(OVER);
            }
            let mut fval;
            if at(f, cp) == b'-' {
                let (mut val2, cp2, erange2) = strtoint(f, cp + 1);
                if erange2 || !(0..12).contains(&val2) {
                    return Err(OVER);
                }
                if at(f, cp2) != 0 {
                    return Err(BAD);
                }
                ty = DTK_MONTH;
                if f[0] == b'-' {
                    val2 = -val2;
                }
                val =
                    val.checked_mul(12).and_then(|v| v.checked_add(i64::from(val2))).ok_or(OVER)?;
                fval = 0.0;
            } else if at(f, cp) == b'.' {
                fval = parse_fraction(major, &f[cp..])?;
                if f[0] == b'-' {
                    fval = -fval;
                }
            } else if at(f, cp) == 0 {
                fval = 0.0;
            } else {
                return Err(BAD);
            }
            if force_negative {
                if val > 0 {
                    val = -val;
                }
                if fval > 0.0 {
                    fval = -fval;
                }
            }
            let ok;
            (ok, tmask) = match ty {
                DTK_MICROSEC => (adjust_microseconds(val, fval, 1, &mut itm), m(MICROSECOND)),
                DTK_MILLISEC => (adjust_microseconds(val, fval, 1000, &mut itm), m(MILLISECOND)),
                DTK_SECOND => (
                    adjust_microseconds(val, fval, USECS_PER_SEC, &mut itm),
                    if fval == 0.0 { m(SECOND) } else { ALL_SECS_M },
                ),
                DTK_MINUTE => {
                    (adjust_microseconds(val, fval, USECS_PER_MINUTE, &mut itm), m(MINUTE))
                }
                DTK_HOUR => {
                    let ok = adjust_microseconds(val, fval, USECS_PER_HOUR, &mut itm);
                    ty = DTK_DAY;
                    (ok, m(HOUR))
                }
                DTK_DAY => (
                    adjust_days(val, 1, &mut itm)
                        && adjust_fract_microseconds(fval, USECS_PER_DAY, &mut itm),
                    m(DAY),
                ),
                DTK_WEEK => {
                    (adjust_days(val, 7, &mut itm) && adjust_fract_days(fval, 7, &mut itm), m(WEEK))
                }
                DTK_MONTH => (
                    adjust_months(val, &mut itm) && adjust_fract_days(fval, 30, &mut itm),
                    m(MONTH),
                ),
                DTK_YEAR => (
                    adjust_years(val, 1, &mut itm) && adjust_fract_years(fval, 1, &mut itm),
                    m(YEAR),
                ),
                DTK_DECADE => (
                    adjust_years(val, 10, &mut itm) && adjust_fract_years(fval, 10, &mut itm),
                    m(DECADE),
                ),
                DTK_CENTURY => (
                    adjust_years(val, 100, &mut itm) && adjust_fract_years(fval, 100, &mut itm),
                    m(CENTURY),
                ),
                DTK_MILLENNIUM => (
                    adjust_years(val, 1000, &mut itm) && adjust_fract_years(fval, 1000, &mut itm),
                    m(MILLENNIUM),
                ),
                _ => return Err(BAD),
            };
            if !ok {
                return Err(OVER);
            }
            parsing_unit_val = false;
        }
        if tmask & fmask != 0 {
            return Err(BAD);
        }
        fmask |= tmask;
    }
    if fmask == 0 {
        return Err(BAD);
    }
    if v17 && parsing_unit_val {
        return Err(BAD);
    }
    if is_before {
        if itm.usec == i64::MIN
            || itm.mday == i32::MIN
            || itm.mon == i32::MIN
            || itm.year == i32::MIN
        {
            return Err(OVER);
        }
        itm = Itm { usec: -itm.usec, mday: -itm.mday, mon: -itm.mon, year: -itm.year };
    }
    Ok((itm, dtype))
}

/// `ParseISO8601Number` from v15: the whole part, the fraction, and where
/// the number ends.
fn parse_iso8601_number(s: &[u8], i: usize) -> Dt<(i64, f64, usize)> {
    if !(isdigit(at(s, i)) || matches!(at(s, i), b'-' | b'.')) {
        return Err(BAD);
    }
    let (val, end, erange) = strtod(s, i);
    if end == i || erange {
        return Err(BAD);
    }
    if val.is_nan() || !(-1.0e15..=1.0e15).contains(&val) {
        return Err(OVER);
    }
    let ipart = if val >= 0.0 { val.floor() as i64 } else { -((-val).floor()) as i64 };
    Ok((ipart, val - ipart as f64, end))
}

/// `strspn(s + i, "0123456789")` past an optional `-`: the integer digits
/// of an ISO 8601 number.
fn iso8601_integer_width(s: &[u8], i: usize) -> usize {
    let i = i + usize::from(at(s, i) == b'-');
    s.get(i..).map_or(0, |rest| rest.iter().take_while(|&&c| isdigit(c)).count())
}

/// `DecodeISO8601Interval` from v15.
fn decode_iso8601_interval(s: &[u8]) -> Dt<Itm> {
    let mut itm = Itm::default();
    if s.len() < 2 || s[0] != b'P' {
        return Err(BAD);
    }
    let mut i = 1;
    let mut datepart = true;
    let mut havefield = false;
    let over = |ok: bool| if ok { Ok(()) } else { Err(OVER) };
    while at(s, i) != 0 {
        if s[i] == b'T' {
            datepart = false;
            havefield = false;
            i += 1;
            continue;
        }
        let fieldstart = i;
        let (val, fval, end) = parse_iso8601_number(s, i)?;
        i = end;
        let unit = at(s, i);
        i += 1;
        if datepart {
            match unit {
                b'Y' => {
                    over(adjust_years(val, 1, &mut itm) && adjust_fract_years(fval, 1, &mut itm))?
                }
                b'M' => {
                    over(adjust_months(val, &mut itm) && adjust_fract_days(fval, 30, &mut itm))?
                }
                b'W' => {
                    over(adjust_days(val, 7, &mut itm) && adjust_fract_days(fval, 7, &mut itm))?
                }
                b'D' => over(
                    adjust_days(val, 1, &mut itm)
                        && adjust_fract_microseconds(fval, USECS_PER_DAY, &mut itm),
                )?,
                b'T' | 0 | b'-' => {
                    if unit != b'-' && iso8601_integer_width(s, fieldstart) == 8 && !havefield {
                        over(
                            adjust_years(val / 10000, 1, &mut itm)
                                && adjust_months((val / 100) % 100, &mut itm)
                                && adjust_days(val % 100, 1, &mut itm)
                                && adjust_fract_microseconds(fval, USECS_PER_DAY, &mut itm),
                        )?;
                        if unit == 0 {
                            return Ok(itm);
                        }
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    if havefield {
                        return Err(BAD);
                    }
                    over(adjust_years(val, 1, &mut itm) && adjust_fract_years(fval, 1, &mut itm))?;
                    if unit == 0 {
                        return Ok(itm);
                    }
                    if unit == b'T' {
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    let (val, fval, end) = parse_iso8601_number(s, i)?;
                    i = end;
                    over(adjust_months(val, &mut itm) && adjust_fract_days(fval, 30, &mut itm))?;
                    if at(s, i) == 0 {
                        return Ok(itm);
                    }
                    if at(s, i) == b'T' {
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    if at(s, i) != b'-' {
                        return Err(BAD);
                    }
                    i += 1;
                    let (val, fval, end) = parse_iso8601_number(s, i)?;
                    i = end;
                    over(
                        adjust_days(val, 1, &mut itm)
                            && adjust_fract_microseconds(fval, USECS_PER_DAY, &mut itm),
                    )?;
                    if at(s, i) == 0 {
                        return Ok(itm);
                    }
                    if at(s, i) == b'T' {
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    return Err(BAD);
                }
                _ => return Err(BAD),
            }
        } else {
            match unit {
                b'H' => over(adjust_microseconds(val, fval, USECS_PER_HOUR, &mut itm))?,
                b'M' => over(adjust_microseconds(val, fval, USECS_PER_MINUTE, &mut itm))?,
                b'S' => over(adjust_microseconds(val, fval, USECS_PER_SEC, &mut itm))?,
                0 | b':' => {
                    if unit == 0 && iso8601_integer_width(s, fieldstart) == 6 && !havefield {
                        over(
                            adjust_microseconds(val / 10000, 0.0, USECS_PER_HOUR, &mut itm)
                                && adjust_microseconds(
                                    (val / 100) % 100,
                                    0.0,
                                    USECS_PER_MINUTE,
                                    &mut itm,
                                )
                                && adjust_microseconds(val % 100, 0.0, USECS_PER_SEC, &mut itm)
                                && adjust_fract_microseconds(fval, 1, &mut itm),
                        )?;
                        return Ok(itm);
                    }
                    if havefield {
                        return Err(BAD);
                    }
                    over(adjust_microseconds(val, fval, USECS_PER_HOUR, &mut itm))?;
                    if unit == 0 {
                        return Ok(itm);
                    }
                    let (val, fval, end) = parse_iso8601_number(s, i)?;
                    i = end;
                    over(adjust_microseconds(val, fval, USECS_PER_MINUTE, &mut itm))?;
                    if at(s, i) == 0 {
                        return Ok(itm);
                    }
                    if at(s, i) != b':' {
                        return Err(BAD);
                    }
                    i += 1;
                    let (val, fval, end) = parse_iso8601_number(s, i)?;
                    i = end;
                    over(adjust_microseconds(val, fval, USECS_PER_SEC, &mut itm))?;
                    if at(s, i) == 0 {
                        return Ok(itm);
                    }
                    return Err(BAD);
                }
                _ => return Err(BAD),
            }
        }
        havefield = true;
    }
    Ok(itm)
}

/// `interval_in` from v15: the decode, ISO 8601 after a bad format, then the
/// month total's range; v17's infinities are read.
fn interval_accepts(
    s: &[u8],
    fields: Option<&[Field]>,
    major: Major,
    sql_standard: bool,
    range: i32,
) -> bool {
    let decoded = match fields {
        Some(fields) => decode_interval(fields, major, sql_standard, range),
        None => Err(BAD),
    };
    let decoded = match decoded {
        Err(Dterr::BadFormat) => decode_iso8601_interval(s).map(|itm| (itm, DTK_DELTA)),
        other => other,
    };
    match decoded {
        Ok((itm, DTK_DELTA)) => {
            let months = i64::from(itm.year) * 12 + i64::from(itm.mon);
            i32::try_from(months).is_ok()
        }
        Ok((_, dtype)) => dtype == DTK_LATE || dtype == DTK_EARLY,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// DecodeInterval and DecodeISO8601Interval before v15: `int` fields that
// wrap, a microsecond `fsec_t`, and each token's number range-checked but no
// sum of them but the months'.

/// `struct pg_tm`'s fields an interval fills, and `fsec`.
#[derive(Clone, Copy, Default, Debug)]
struct TmInterval {
    year: i32,
    mon: i32,
    mday: i32,
    hour: i32,
    min: i32,
    sec: i32,
    fsec: i32,
}

/// `*fsec += x`, a `double` added to an `int`.
fn add_to_int(target: &mut i32, x: f64) {
    *target = c_int(f64::from(*target) + x);
}

/// `AdjustFractSeconds` before v15.
fn adjust_fract_seconds_v13(frac: f64, tm: &mut TmInterval, scale: i32) {
    if frac == 0.0 {
        return;
    }
    let frac = frac * f64::from(scale);
    let sec = c_int(frac);
    tm.sec = tm.sec.wrapping_add(sec);
    add_to_int(&mut tm.fsec, rint((frac - f64::from(sec)) * 1_000_000.0));
}

/// `AdjustFractDays` before v15.
fn adjust_fract_days_v13(frac: f64, tm: &mut TmInterval, scale: i32) {
    if frac == 0.0 {
        return;
    }
    let frac = frac * f64::from(scale);
    let extra_days = c_int(frac);
    tm.mday = tm.mday.wrapping_add(extra_days);
    adjust_fract_seconds_v13(frac - f64::from(extra_days), tm, 86_400);
}

/// `DecodeInterval` before v15: the decoded value and its `dtype`.
fn decode_interval_v13(fields: &[Field], sql_standard: bool, range: i32) -> Dt<(TmInterval, i32)> {
    let nf = fields.len();
    let mut is_before = false;
    let mut fmask = 0u32;
    let mut ty = IGNORE_DTF;
    let mut dtype = DTK_DELTA;
    let mut tm = TmInterval::default();
    for i in (0..nf).rev() {
        let f = &fields[i].text[..];
        let mut tmask: u32;
        let mut number = false;
        let decode_time = |s: &[u8], tm: &mut TmInterval| -> Dt<()> {
            let (hour, min, sec, fsec) = decode_time_common(Major::V13, s, range)?;
            (tm.hour, tm.min, tm.sec, tm.fsec) = (hour as i32, min, sec, fsec);
            Ok(())
        };
        match fields[i].ftype {
            Ftype::Time => {
                decode_time(f, &mut tm)?;
                tmask = TIME_M;
                ty = DTK_DAY;
            }
            Ftype::Tz => {
                let mut timed = tm;
                if f[1..].contains(&b':') && decode_time(&f[1..], &mut timed).is_ok() {
                    tm = timed;
                    if f[0] == b'-' {
                        tm.hour = tm.hour.wrapping_neg();
                        tm.min = tm.min.wrapping_neg();
                        tm.sec = tm.sec.wrapping_neg();
                        tm.fsec = tm.fsec.wrapping_neg();
                    }
                    ty = DTK_DAY;
                    tmask = TIME_M;
                } else {
                    tmask = 0;
                    number = true;
                }
            }
            Ftype::Date | Ftype::Number => {
                tmask = 0;
                number = true;
            }
            Ftype::String | Ftype::Special => {
                let uval;
                (ty, uval) = units(f).unwrap_or((UNKNOWN_FIELD, 0));
                if ty == IGNORE_DTF {
                    continue;
                }
                tmask = 0;
                match ty {
                    UNITS => ty = uval,
                    AGO => {
                        is_before = true;
                        ty = uval;
                    }
                    RESERV => {
                        tmask = DATE_M | TIME_M;
                        dtype = uval;
                    }
                    _ => return Err(BAD),
                }
            }
        }
        if number {
            if ty == IGNORE_DTF {
                ty = implicit_unit(range);
            }
            let (mut val, cp, erange) = strtoint(f, 0);
            if erange {
                return Err(OVER);
            }
            let mut fval;
            if at(f, cp) == b'-' {
                let (mut val2, cp2, erange2) = strtoint(f, cp + 1);
                if erange2 || !(0..12).contains(&val2) {
                    return Err(OVER);
                }
                if at(f, cp2) != 0 {
                    return Err(BAD);
                }
                ty = DTK_MONTH;
                if f[0] == b'-' {
                    val2 = -val2;
                }
                let months = f64::from(val) * 12.0 + f64::from(val2);
                if months > f64::from(i32::MAX) || months < f64::from(i32::MIN) {
                    return Err(OVER);
                }
                val = val.wrapping_mul(12).wrapping_add(val2);
                fval = 0.0;
            } else if at(f, cp) == b'.' {
                fval = parse_fraction(Major::V13, &f[cp..])?;
                if f[0] == b'-' {
                    fval = -fval;
                }
            } else if at(f, cp) == 0 {
                fval = 0.0;
            } else {
                return Err(BAD);
            }
            let years = |tm: &mut TmInterval, scale: i32| {
                tm.year = tm.year.wrapping_add(val.wrapping_mul(scale));
                if fval != 0.0 {
                    add_to_int(&mut tm.mon, fval * 12.0 * f64::from(scale));
                }
            };
            tmask = match ty {
                DTK_MICROSEC => {
                    add_to_int(&mut tm.fsec, rint(f64::from(val) + fval));
                    m(MICROSECOND)
                }
                DTK_MILLISEC => {
                    tm.sec = tm.sec.wrapping_add(val / 1000);
                    let rest = val - (val / 1000) * 1000;
                    add_to_int(&mut tm.fsec, rint((f64::from(rest) + fval) * 1000.0));
                    m(MILLISECOND)
                }
                DTK_SECOND => {
                    tm.sec = tm.sec.wrapping_add(val);
                    add_to_int(&mut tm.fsec, rint(fval * 1_000_000.0));
                    if fval == 0.0 { m(SECOND) } else { ALL_SECS_M }
                }
                DTK_MINUTE => {
                    tm.min = tm.min.wrapping_add(val);
                    adjust_fract_seconds_v13(fval, &mut tm, 60);
                    m(MINUTE)
                }
                DTK_HOUR => {
                    tm.hour = tm.hour.wrapping_add(val);
                    adjust_fract_seconds_v13(fval, &mut tm, 3600);
                    ty = DTK_DAY;
                    m(HOUR)
                }
                DTK_DAY => {
                    tm.mday = tm.mday.wrapping_add(val);
                    adjust_fract_seconds_v13(fval, &mut tm, 86_400);
                    m(DAY)
                }
                DTK_WEEK => {
                    tm.mday = tm.mday.wrapping_add(val.wrapping_mul(7));
                    adjust_fract_days_v13(fval, &mut tm, 7);
                    m(WEEK)
                }
                DTK_MONTH => {
                    tm.mon = tm.mon.wrapping_add(val);
                    adjust_fract_days_v13(fval, &mut tm, 30);
                    m(MONTH)
                }
                DTK_YEAR => {
                    years(&mut tm, 1);
                    m(YEAR)
                }
                DTK_DECADE => {
                    years(&mut tm, 10);
                    m(DECADE)
                }
                DTK_CENTURY => {
                    years(&mut tm, 100);
                    m(CENTURY)
                }
                DTK_MILLENNIUM => {
                    years(&mut tm, 1000);
                    m(MILLENNIUM)
                }
                _ => return Err(BAD),
            };
        }
        if tmask & fmask != 0 {
            return Err(BAD);
        }
        fmask |= tmask;
    }
    if fmask == 0 {
        return Err(BAD);
    }
    if tm.fsec != 0 {
        let sec = tm.fsec / 1_000_000;
        tm.fsec -= sec * 1_000_000;
        tm.sec = tm.sec.wrapping_add(sec);
    }
    if sql_standard
        && fields[0].text[0] == b'-'
        && !fields[1..].iter().any(|f| matches!(f.text[0], b'-' | b'+'))
    {
        for part in [
            &mut tm.fsec,
            &mut tm.sec,
            &mut tm.min,
            &mut tm.hour,
            &mut tm.mday,
            &mut tm.mon,
            &mut tm.year,
        ] {
            if *part > 0 {
                *part = -*part;
            }
        }
    }
    if is_before {
        for part in [
            &mut tm.fsec,
            &mut tm.sec,
            &mut tm.min,
            &mut tm.hour,
            &mut tm.mday,
            &mut tm.mon,
            &mut tm.year,
        ] {
            *part = part.wrapping_neg();
        }
    }
    Ok((tm, dtype))
}

/// `ParseISO8601Number` before v15: an `int` whole part, a NaN read.
fn parse_iso8601_number_v13(s: &[u8], i: usize) -> Dt<(i32, f64, usize)> {
    if !(isdigit(at(s, i)) || matches!(at(s, i), b'-' | b'.')) {
        return Err(BAD);
    }
    let (val, end, erange) = strtod(s, i);
    if end == i || erange {
        return Err(BAD);
    }
    if val < f64::from(i32::MIN) || val > f64::from(i32::MAX) {
        return Err(OVER);
    }
    let ipart = if val >= 0.0 { c_int(val.floor()) } else { c_int(-(-val).floor()) };
    Ok((ipart, val - f64::from(ipart), end))
}

/// `DecodeISO8601Interval` before v15.
fn decode_iso8601_interval_v13(s: &[u8]) -> Dt<TmInterval> {
    let mut tm = TmInterval::default();
    if s.len() < 2 || s[0] != b'P' {
        return Err(BAD);
    }
    let mut i = 1;
    let mut datepart = true;
    let mut havefield = false;
    let years = |tm: &mut TmInterval, val: i32, fval: f64| {
        tm.year = tm.year.wrapping_add(val);
        add_to_int(&mut tm.mon, fval * 12.0);
    };
    let months = |tm: &mut TmInterval, val: i32, fval: f64| {
        tm.mon = tm.mon.wrapping_add(val);
        adjust_fract_days_v13(fval, tm, 30);
    };
    let days = |tm: &mut TmInterval, val: i32, fval: f64| {
        tm.mday = tm.mday.wrapping_add(val);
        adjust_fract_seconds_v13(fval, tm, 86_400);
    };
    let hours = |tm: &mut TmInterval, val: i32, fval: f64| {
        tm.hour = tm.hour.wrapping_add(val);
        adjust_fract_seconds_v13(fval, tm, 3600);
    };
    let minutes = |tm: &mut TmInterval, val: i32, fval: f64| {
        tm.min = tm.min.wrapping_add(val);
        adjust_fract_seconds_v13(fval, tm, 60);
    };
    let seconds = |tm: &mut TmInterval, val: i32, fval: f64| {
        tm.sec = tm.sec.wrapping_add(val);
        adjust_fract_seconds_v13(fval, tm, 1);
    };
    while at(s, i) != 0 {
        if s[i] == b'T' {
            datepart = false;
            havefield = false;
            i += 1;
            continue;
        }
        let fieldstart = i;
        let (val, fval, end) = parse_iso8601_number_v13(s, i)?;
        i = end;
        let unit = at(s, i);
        i += 1;
        if datepart {
            match unit {
                b'Y' => years(&mut tm, val, fval),
                b'M' => months(&mut tm, val, fval),
                b'W' => {
                    tm.mday = tm.mday.wrapping_add(val.wrapping_mul(7));
                    adjust_fract_days_v13(fval, &mut tm, 7);
                }
                b'D' => days(&mut tm, val, fval),
                b'T' | 0 | b'-' => {
                    if unit != b'-' && iso8601_integer_width(s, fieldstart) == 8 && !havefield {
                        tm.year = tm.year.wrapping_add(val / 10000);
                        tm.mon = tm.mon.wrapping_add((val / 100) % 100);
                        tm.mday = tm.mday.wrapping_add(val % 100);
                        adjust_fract_seconds_v13(fval, &mut tm, 86_400);
                        if unit == 0 {
                            return Ok(tm);
                        }
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    if havefield {
                        return Err(BAD);
                    }
                    years(&mut tm, val, fval);
                    if unit == 0 {
                        return Ok(tm);
                    }
                    if unit == b'T' {
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    let (val, fval, end) = parse_iso8601_number_v13(s, i)?;
                    i = end;
                    months(&mut tm, val, fval);
                    if at(s, i) == 0 {
                        return Ok(tm);
                    }
                    if at(s, i) == b'T' {
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    if at(s, i) != b'-' {
                        return Err(BAD);
                    }
                    i += 1;
                    let (val, fval, end) = parse_iso8601_number_v13(s, i)?;
                    i = end;
                    days(&mut tm, val, fval);
                    if at(s, i) == 0 {
                        return Ok(tm);
                    }
                    if at(s, i) == b'T' {
                        datepart = false;
                        havefield = false;
                        continue;
                    }
                    return Err(BAD);
                }
                _ => return Err(BAD),
            }
        } else {
            match unit {
                b'H' => hours(&mut tm, val, fval),
                b'M' => minutes(&mut tm, val, fval),
                b'S' => seconds(&mut tm, val, fval),
                0 | b':' => {
                    if unit == 0 && iso8601_integer_width(s, fieldstart) == 6 && !havefield {
                        tm.hour = tm.hour.wrapping_add(val / 10000);
                        tm.min = tm.min.wrapping_add((val / 100) % 100);
                        tm.sec = tm.sec.wrapping_add(val % 100);
                        adjust_fract_seconds_v13(fval, &mut tm, 1);
                        return Ok(tm);
                    }
                    if havefield {
                        return Err(BAD);
                    }
                    hours(&mut tm, val, fval);
                    if unit == 0 {
                        return Ok(tm);
                    }
                    let (val, fval, end) = parse_iso8601_number_v13(s, i)?;
                    i = end;
                    minutes(&mut tm, val, fval);
                    if at(s, i) == 0 {
                        return Ok(tm);
                    }
                    if at(s, i) != b':' {
                        return Err(BAD);
                    }
                    i += 1;
                    let (val, fval, end) = parse_iso8601_number_v13(s, i)?;
                    i = end;
                    seconds(&mut tm, val, fval);
                    if at(s, i) == 0 {
                        return Ok(tm);
                    }
                    return Err(BAD);
                }
                _ => return Err(BAD),
            }
        }
        havefield = true;
    }
    Ok(tm)
}

/// `interval_in` before v15: the month total, read as a `double` from the
/// wrapped fields, is all `tm2interval` checks.
fn interval_accepts_v13(
    s: &[u8],
    fields: Option<&[Field]>,
    sql_standard: bool,
    range: i32,
) -> bool {
    let decoded = match fields {
        Some(fields) => decode_interval_v13(fields, sql_standard, range),
        None => Err(BAD),
    };
    let decoded = match decoded {
        Err(Dterr::BadFormat) => decode_iso8601_interval_v13(s).map(|tm| (tm, DTK_DELTA)),
        other => other,
    };
    match decoded {
        Ok((tm, DTK_DELTA)) => {
            let months = f64::from(tm.year) * 12.0 + f64::from(tm.mon);
            (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&months)
        }
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// **A text is read exactly where some supported major reads it under some
    /// setting** — and refused where none does (I83, I84). Each case was put
    /// to its type's input function in a `postgres:<major>-trixie` container
    /// at 13.23, 14.24, 15.19, 16.15, 17.11 and 18.6, through a literal so
    /// the input function is handed the typmod as `COPY` hands it: a date or
    /// time under each `DateStyle` order with the `Default` and `Australia`
    /// abbreviations and a file naming each word of the text no keyword names
    /// as a fixed zone abbreviation, beside a zone file `Foo/Bar`; an
    /// `interval` under both `IntervalStyle`s and as `interval`, `interval
    /// year`, `month`, `day`, `hour`, `minute` and `minute to second`. The
    /// six digits say which majors read it; `w` marks a timestamp read only
    /// under a session zone a week from UTC, `TimeZone = 'FOO24'` or
    /// `'FOO-167'`, which those servers read. The cases are hand-written ones
    /// and a sample of a differential run of 84,000 generated texts, 252,000
    /// cases with each put to every input it suits, which this build read as
    /// the servers did but for a zone named with punctuation, a server's
    /// zone files deciding it, and a timestamp at its range's edge.
    #[test]
    fn a_text_is_read_exactly_where_some_major_reads_it_under_some_setting() {
        let check = |read: bool, text: &str, majors: &str, what: &str| {
            let want = majors.contains(['1', 'w']);
            assert_eq!(read, want, "{what} {text:?}, read by {majors}");
        };
        let date: &[(&str, &str)] = &[
            ("abc", "000000"),
            ("epoch 2020-01-01", "000000"),
            ("y2001m02d04", "111000"),
            ("2020-01-01 y", "111000"),
            ("1999.008", "111111"),
            ("J2451187", "111111"),
            ("January 8, 1999", "111111"),
            ("2020-01-01 xyz", "111111"),
            ("2020-01-01 foo5", "111111"),
            ("2020-01-01 foo//bar", "000000"),
            ("2020-01-01 foo/.bar", "000000"),
            ("2020-01-01 a/b-99:00", "111111"),
            ("4714-11-23 BC", "000000"),
            ("2020-01-01 mon sat", "111111"),
            ("2020-01-01 mon tue", "000000"),
            ("0000-01-01", "000000"),
            ("2020-13-01", "000000"),
            ("2020-02-30x", "000000"),
            ("today", "111111"),
            ("tomorrow bc", "111111"),
            ("20200101", "111111"),
            ("200101", "111111"),
            ("12/31/99", "111111"),
            ("31/12/99", "111111"),
            ("99/12/31", "111111"),
            ("2020-01-01 +16", "000000"),
            ("5874898-01-01", "000000"),
            ("+infinity", "000111"),
            ("2020-01-01 at on", "111111"),
            ("12:00 pm", "000000"),
            ("13:00 pm", "000000"),
            ("12:", "000000"),
            ("12::30", "000000"),
            ("24:00:01", "000000"),
            ("04:05 PM", "000000"),
            ("allballs", "000000"),
            ("now", "111111"),
            ("040506", "111111"),
            ("12:00 m", "000000"),
            ("z", "000000"),
            ("12:00:00.", "000000"),
            ("12:00 xyz", "000000"),
            ("12:00 sat", "000000"),
            ("1:2:3:4", "000000"),
            ("1999-12-30 995959", "111111"),
            ("2020-01-01 t 12:00", "111111"),
            ("2020-01-01 t abcd-05", "111110"),
            ("epoch", "111111"),
            ("294277-01-01", "111111"),
            ("294276-12-31 23:59:59.999999", "111111"),
            ("2020-01-01 25:00", "000000"),
            ("294277-01-01 +00", "111111"),
            ("294277-01-01 xyz", "111111"),
            ("4714-11-23 23:00:00-02 BC", "000000"),
            ("12:00 dst", "000000"),
            ("2020-01-01 12:00 xyz dst", "111111"),
            ("2020-01-01 12:00 foo5 dst", "000000"),
            ("12:00 +05 dst", "000000"),
            ("2020-01-01 -infinity", "111000"),
            ("infinity 2020-01-01", "000000"),
            ("Sat Jan 01 00:00:00 2000 PST", "111111"),
            ("2003-04-12 04:05:06 America/New_York", "111111"),
            ("20011225T040506.789-07", "111111"),
            ("2001-12-25 t 04:05:06", "111111"),
            ("h04mm05s06", "000000"),
            ("2020-01-01 j", "111000"),
            ("12-jan2020", "111111"),
            ("jan 2020 12", "111111"),
            ("99999999999-01-01", "000000"),
            ("2020-01-01 12:00 2147483648", "000000"),
            ("2020-01-01 12:00:00.5.5", "000000"),
            ("12:00:00.0000005", "000000"),
            ("2020-01-01 12:00:00.", "001111"),
            ("1-1-1", "111111"),
            ("70-1-1", "111111"),
            ("2020-366", "111111"),
            ("2019-366", "111111"),
            ("", "000000"),
            ("+", "000000"),
            ("0epoch", "111000"),
            ("\t1-1-1j", "111000"),
            ("1-1-1/y", "111000"),
            ("70-1-1y", "111000"),
            (" +infinity", "000111"),
            ("2020-1-1", "111111"),
            ("infinity", "111111"),
            ("1/1/70  +1/00:00:00.", "001111"),
            ("2000-01-01  00:00:00.", "001111"),
            ("xyz,today t 00:00:00.", "001111"),
            ("20200101 t foo5-+15:59:59 ", "111110"),
            ("0", "000000"),
            (" now", "111111"),
            ("1/2/3:s", "111000"),
            ("q\"EPOCH", "111000"),
            ("\t7_epoch", "111000"),
            ("J245118M", "111000"),
            ("+infinity at at", "000111"),
            ("2020/01/02T+15:59 12:30.5", "000111"),
            ("20200101 t -infxniry 12:00:00.1234567890123", "000111"),
            ("2019-2-9", "111111"),
            ("tomorrow  00:00:00.", "001111"),
            ("\t", "000000"),
            ("040506m", "111000"),
            ("1.2.3_s", "111000"),
            ("1/1/7h0", "111000"),
            ("epoch3B", "111000"),
            ("\tnow", "111111"),
            ("\t+infinity", "000111"),
            (" 12/05/2020T  +05 12:34:56.789 ", "000111"),
            ("EPOCH\t00:00:00.", "001000"),
            ("100+allballs,12::30.EPOCH\t", "001000"),
            ("2000-01-01 12:00:005.", "001111"),
            ("2020-01-01 12:34:56. B", "001111"),
            ("yesterday\t+0530 00:00:00.", "001111"),
        ];
        for (text, majors) in date {
            check(datetime_reads(DateTimeInput::Date, text), text, majors, "date");
        }
        let time: &[(&str, &str)] = &[
            ("abc", "000000"),
            ("epoch 2020-01-01", "000000"),
            ("y2001m02d04", "000000"),
            ("2020-01-01 y", "000000"),
            ("1999.008", "000000"),
            ("J2451187", "000000"),
            ("January 8, 1999", "000000"),
            ("2020-01-01 xyz", "000000"),
            ("2020-01-01 foo5", "000000"),
            ("2020-01-01 foo//bar", "000000"),
            ("2020-01-01 foo/.bar", "000000"),
            ("2020-01-01 a/b-99:00", "000000"),
            ("4714-11-23 BC", "000000"),
            ("2020-01-01 mon sat", "000000"),
            ("2020-01-01 mon tue", "000000"),
            ("0000-01-01", "000000"),
            ("2020-13-01", "000000"),
            ("2020-02-30x", "000000"),
            ("today", "000000"),
            ("tomorrow bc", "000000"),
            ("20200101", "000000"),
            ("200101", "111111"),
            ("12/31/99", "000000"),
            ("31/12/99", "000000"),
            ("99/12/31", "000000"),
            ("2020-01-01 +16", "000000"),
            ("5874898-01-01", "000000"),
            ("+infinity", "000000"),
            ("2020-01-01 at on", "000000"),
            ("12:00 pm", "111111"),
            ("13:00 pm", "000000"),
            ("12:", "111111"),
            ("12::30", "111111"),
            ("24:00:01", "000000"),
            ("04:05 PM", "111111"),
            ("allballs", "111111"),
            ("now", "111111"),
            ("040506", "111111"),
            ("12:00 m", "111000"),
            ("z", "000000"),
            ("12:00:00.", "001111"),
            ("12:00 xyz", "111111"),
            ("12:00 sat", "111111"),
            ("1:2:3:4", "000000"),
            ("1999-12-30 995959", "000000"),
            ("2020-01-01 t 12:00", "000000"),
            ("2020-01-01 t abcd-05", "000000"),
            ("epoch", "000000"),
            ("294277-01-01", "000000"),
            ("294276-12-31 23:59:59.999999", "111111"),
            ("2020-01-01 25:00", "000000"),
            ("294277-01-01 +00", "000000"),
            ("294277-01-01 xyz", "000000"),
            ("4714-11-23 23:00:00-02 BC", "111111"),
            ("12:00 dst", "000000"),
            ("2020-01-01 12:00 xyz dst", "111111"),
            ("2020-01-01 12:00 foo5 dst", "000000"),
            ("12:00 +05 dst", "111111"),
            ("2020-01-01 -infinity", "000000"),
            ("infinity 2020-01-01", "000000"),
            ("Sat Jan 01 00:00:00 2000 PST", "000000"),
            ("2003-04-12 04:05:06 America/New_York", "111111"),
            ("20011225T040506.789-07", "000000"),
            ("2001-12-25 t 04:05:06", "000000"),
            ("h04mm05s06", "000000"),
            ("2020-01-01 j", "000000"),
            ("12-jan2020", "000000"),
            ("jan 2020 12", "000000"),
            ("99999999999-01-01", "000000"),
            ("2020-01-01 12:00 2147483648", "000000"),
            ("2020-01-01 12:00:00.5.5", "000000"),
            ("12:00:00.0000005", "111111"),
            ("2020-01-01 12:00:00.", "001111"),
            ("1-1-1", "000000"),
            ("70-1-1", "000000"),
            ("2020-366", "000000"),
            ("2019-366", "000000"),
            ("", "000000"),
            ("+", "000000"),
            ("0:0", "111111"),
            ("24:00:00.Z", "001111"),
            ("04:05:06.N-8", "001111"),
            ("24:00:00.BCd", "001111"),
            ("at 00:00:00.\t", "001111"),
            ("m12: ", "111000"),
            ("04:05M", "111000"),
            ("0:W5 M", "111000"),
            ("04:05 M", "111000"),
            ("d12:.-0530", "001000"),
            ("00:00:00.= mm +5:3:2_", "001000"),
            (" -12:00:y2001_00:00:00.", "001000"),
            ("0:0:00", "111111"),
            ("24:0:0", "111111"),
            ("00:00:00.", "001111"),
            ("0", "000000"),
            ("!4:0:0.", "001111"),
            ("00:00:.g", "001111"),
            ("25:00.+05", "001111"),
            ("\t00:00:00.", "001111"),
            ("1:", "111111"),
            ("0:!", "111111"),
            ("y12:", "111000"),
            ("0405D", "111000"),
            ("0:05 M", "111000"),
            ("d  12:", "111000"),
            ("0:0:0", "111111"),
            ("1:4:56", "111111"),
            ("0:00:00.", "001111"),
            ("24:00:00.d04Tat", "001000"),
            ("isodow,00:00:00.", "001000"),
            ("isodow_00:00:00.", "001000"),
            ("\t", "000000"),
            ("7:0.", "001111"),
            ("12:30.", "001111"),
            ("\t12:30.", "001111"),
            ("0:0.-0530", "001111"),
            ("0000d", "111000"),
            ("0406J", "111000"),
            ("0405 M", "111000"),
            ("2020\ty", "111000"),
            ("0:00:0", "111111"),
            ("d04:1.", "001000"),
            ("dow 00:00:00.", "001000"),
            ("dow_00:00:60.xyz", "001000"),
        ];
        for (text, majors) in time {
            check(datetime_reads(DateTimeInput::Time, text), text, majors, "time");
        }
        let timetz: &[(&str, &str)] = &[
            ("abc", "000000"),
            ("epoch 2020-01-01", "000000"),
            ("y2001m02d04", "000000"),
            ("2020-01-01 y", "000000"),
            ("1999.008", "000000"),
            ("J2451187", "000000"),
            ("January 8, 1999", "000000"),
            ("2020-01-01 xyz", "000000"),
            ("2020-01-01 foo5", "000000"),
            ("2020-01-01 foo//bar", "000000"),
            ("2020-01-01 foo/.bar", "000000"),
            ("2020-01-01 a/b-99:00", "000000"),
            ("4714-11-23 BC", "000000"),
            ("2020-01-01 mon sat", "000000"),
            ("2020-01-01 mon tue", "000000"),
            ("0000-01-01", "000000"),
            ("2020-13-01", "000000"),
            ("2020-02-30x", "000000"),
            ("today", "000000"),
            ("tomorrow bc", "000000"),
            ("20200101", "000000"),
            ("200101", "111111"),
            ("12/31/99", "000000"),
            ("31/12/99", "000000"),
            ("99/12/31", "000000"),
            ("2020-01-01 +16", "000000"),
            ("5874898-01-01", "000000"),
            ("+infinity", "000000"),
            ("2020-01-01 at on", "000000"),
            ("12:00 pm", "111111"),
            ("13:00 pm", "000000"),
            ("12:", "111111"),
            ("12::30", "111111"),
            ("24:00:01", "000000"),
            ("04:05 PM", "111111"),
            ("allballs", "111111"),
            ("now", "111111"),
            ("040506", "111111"),
            ("12:00 m", "111000"),
            ("z", "000000"),
            ("12:00:00.", "001111"),
            ("12:00 xyz", "111111"),
            ("12:00 sat", "111111"),
            ("1:2:3:4", "000000"),
            ("1999-12-30 995959", "000000"),
            ("2020-01-01 t 12:00", "000000"),
            ("2020-01-01 t abcd-05", "000000"),
            ("epoch", "000000"),
            ("294277-01-01", "000000"),
            ("294276-12-31 23:59:59.999999", "111111"),
            ("2020-01-01 25:00", "000000"),
            ("294277-01-01 +00", "000000"),
            ("294277-01-01 xyz", "000000"),
            ("4714-11-23 23:00:00-02 BC", "111111"),
            ("12:00 dst", "000000"),
            ("2020-01-01 12:00 xyz dst", "111111"),
            ("2020-01-01 12:00 foo5 dst", "000000"),
            ("12:00 +05 dst", "111111"),
            ("2020-01-01 -infinity", "000000"),
            ("infinity 2020-01-01", "000000"),
            ("Sat Jan 01 00:00:00 2000 PST", "000000"),
            ("2003-04-12 04:05:06 America/New_York", "111111"),
            ("20011225T040506.789-07", "000000"),
            ("2001-12-25 t 04:05:06", "000000"),
            ("h04mm05s06", "000000"),
            ("2020-01-01 j", "000000"),
            ("12-jan2020", "000000"),
            ("jan 2020 12", "000000"),
            ("99999999999-01-01", "000000"),
            ("2020-01-01 12:00 2147483648", "000000"),
            ("2020-01-01 12:00:00.5.5", "000000"),
            ("12:00:00.0000005", "111111"),
            ("2020-01-01 12:00:00.", "001111"),
            ("1-1-1", "000000"),
            ("70-1-1", "000000"),
            ("2020-366", "000000"),
            ("2019-366", "000000"),
            ("", "000000"),
            ("+", "000000"),
            ("0:0", "111111"),
            ("00:00:00.", "001111"),
            ("24:00:00.Z", "001111"),
            ("04:05:06.N-8", "001111"),
            ("24:00:00.BCd", "001111"),
            ("m12: ", "111000"),
            ("04:05M", "111000"),
            ("0:W5 M", "111000"),
            ("04:05 M", "111000"),
            ("d12:.-0530", "001000"),
            ("00:00:00.= mm +5:3:2_", "001000"),
            (" -12:00:y2001_00:00:00.", "001000"),
            ("1:0:0-08", "111111"),
            ("12:0:00-0", "111111"),
            ("0", "000000"),
            ("!4:0:0.", "001111"),
            ("00:00:.g", "001111"),
            ("0:00:00.", "001111"),
            ("1:", "111111"),
            ("0:!", "111111"),
            ("y12:", "111000"),
            ("0405D", "111000"),
            ("0:05 M", "111000"),
            ("d  12:", "111000"),
            ("2:00:00-1", "111111"),
            ("12:00:0-08", "111111"),
            ("24:00:00.d04Tat", "001000"),
            ("isodow,00:00:00.", "001000"),
            ("isodow_00:00:00.", "001000"),
            ("\t", "000000"),
            ("7:0.", "001111"),
            ("12:30.", "001111"),
            ("\t12:30.", "001111"),
            ("0000d", "111000"),
            ("0406J", "111000"),
            ("0405 M", "111000"),
            ("2020\ty", "111000"),
            ("2:00:0-0", "111111"),
            ("12:00:0-0", "111111"),
            ("d04:1.", "001000"),
            ("dow 00:00:00.", "001000"),
            ("dow_00:00:60.xyz", "001000"),
        ];
        for (text, majors) in timetz {
            check(datetime_reads(DateTimeInput::TimeTz, text), text, majors, "timetz");
        }
        let timestamp: &[(&str, &str)] = &[
            ("abc", "000000"),
            ("epoch 2020-01-01", "000000"),
            ("y2001m02d04", "111000"),
            ("2020-01-01 y", "111000"),
            ("1999.008", "111111"),
            ("J2451187", "111111"),
            ("January 8, 1999", "111111"),
            ("2020-01-01 xyz", "111111"),
            ("2020-01-01 foo5", "111111"),
            ("2020-01-01 foo//bar", "000000"),
            ("2020-01-01 foo/.bar", "000000"),
            ("2020-01-01 a/b-99:00", "111111"),
            ("4714-11-23 BC", "000000"),
            ("2020-01-01 mon sat", "111111"),
            ("2020-01-01 mon tue", "000000"),
            ("0000-01-01", "000000"),
            ("2020-13-01", "000000"),
            ("2020-02-30x", "000000"),
            ("today", "111111"),
            ("tomorrow bc", "111111"),
            ("20200101", "111111"),
            ("200101", "111111"),
            ("12/31/99", "111111"),
            ("31/12/99", "111111"),
            ("99/12/31", "111111"),
            ("2020-01-01 +16", "000000"),
            ("5874898-01-01", "000000"),
            ("+infinity", "000111"),
            ("2020-01-01 at on", "111111"),
            ("12:00 pm", "000000"),
            ("13:00 pm", "000000"),
            ("12:", "000000"),
            ("12::30", "000000"),
            ("24:00:01", "000000"),
            ("04:05 PM", "000000"),
            ("allballs", "000000"),
            ("now", "111111"),
            ("040506", "111111"),
            ("12:00 m", "000000"),
            ("z", "000000"),
            ("12:00:00.", "000000"),
            ("12:00 xyz", "000000"),
            ("12:00 sat", "000000"),
            ("1:2:3:4", "000000"),
            ("1999-12-30 995959", "000001"),
            ("2020-01-01 t 12:00", "111111"),
            ("2020-01-01 t abcd-05", "111110"),
            ("epoch", "111111"),
            ("294277-01-01", "000000"),
            ("294276-12-31 23:59:59.999999", "111111"),
            ("2020-01-01 25:00", "000000"),
            ("294277-01-01 +00", "000000"),
            ("294277-01-01 xyz", "000000"),
            ("4714-11-23 23:00:00-02 BC", "000000"),
            ("12:00 dst", "000000"),
            ("2020-01-01 12:00 xyz dst", "111111"),
            ("2020-01-01 12:00 foo5 dst", "000000"),
            ("12:00 +05 dst", "000000"),
            ("2020-01-01 -infinity", "111000"),
            ("infinity 2020-01-01", "000000"),
            ("Sat Jan 01 00:00:00 2000 PST", "111111"),
            ("2003-04-12 04:05:06 America/New_York", "111111"),
            ("20011225T040506.789-07", "111111"),
            ("2001-12-25 t 04:05:06", "111111"),
            ("h04mm05s06", "000000"),
            ("2020-01-01 j", "111000"),
            ("12-jan2020", "111111"),
            ("jan 2020 12", "111111"),
            ("99999999999-01-01", "000000"),
            ("2020-01-01 12:00 2147483648", "000000"),
            ("2020-01-01 12:00:00.5.5", "000000"),
            ("12:00:00.0000005", "000000"),
            ("2020-01-01 12:00:00.", "001111"),
            ("1-1-1", "111111"),
            ("70-1-1", "111111"),
            ("2020-366", "111111"),
            ("2019-366", "111111"),
            ("", "000000"),
            ("+", "000000"),
            ("0epoch", "111000"),
            ("\t1-1-1j", "111000"),
            ("1-1-1/y", "111000"),
            ("70-1-1y", "111000"),
            (" +infinity", "000111"),
            ("infinity", "111111"),
            ("-infinity", "111111"),
            ("1/1/70  +1/00:00:00.", "001111"),
            ("2000-01-01  00:00:00.", "001111"),
            ("xyz,today t 00:00:00.", "001111"),
            ("20200101 t foo5-+15:59:59 ", "111110"),
            ("0", "000000"),
            (" now", "111111"),
            ("1/2/3:s", "111000"),
            ("q\"EPOCH", "111000"),
            ("\t7_epoch", "111000"),
            ("J245118M", "111000"),
            ("+infinity at at", "000111"),
            ("2020/01/02T+15:59 12:30.5", "000111"),
            ("20200101 t -infxniry 12:00:00.1234567890123", "000111"),
            ("tomorrow  00:00:00.", "001111"),
            ("\t", "000000"),
            ("040506m", "111000"),
            ("1.2.3_s", "111000"),
            ("1/1/7h0", "111000"),
            ("epoch3B", "111000"),
            ("\tnow", "111111"),
            ("\t+infinity", "000111"),
            (" 12/05/2020T  +05 12:34:56.789 ", "000111"),
            ("EPOCH\t00:00:00.", "001000"),
            ("100+allballs,12::30.EPOCH\t", "001000"),
            ("2000-01-01 12:00:005.", "001111"),
            ("2020-01-01 12:34:56. B", "001111"),
            ("yesterday\t+0530 00:00:00.", "001111"),
        ];
        for (text, majors) in timestamp {
            check(datetime_reads(DateTimeInput::Timestamp, text), text, majors, "timestamp");
        }
        let timestamptz: &[(&str, &str)] = &[
            ("abc", "000000"),
            ("epoch 2020-01-01", "000000"),
            ("y2001m02d04", "111000"),
            ("2020-01-01 y", "111000"),
            ("1999.008", "111111"),
            ("J2451187", "111111"),
            ("January 8, 1999", "111111"),
            ("2020-01-01 xyz", "111111"),
            ("2020-01-01 foo5", "111111"),
            ("2020-01-01 foo//bar", "000000"),
            ("2020-01-01 foo/.bar", "000000"),
            ("2020-01-01 a/b-99:00", "111111"),
            ("4714-11-23 BC", "w"),
            ("2020-01-01 mon sat", "111111"),
            ("2020-01-01 mon tue", "000000"),
            ("0000-01-01", "000000"),
            ("2020-13-01", "000000"),
            ("2020-02-30x", "000000"),
            ("today", "111111"),
            ("tomorrow bc", "111111"),
            ("20200101", "111111"),
            ("200101", "111111"),
            ("12/31/99", "111111"),
            ("31/12/99", "111111"),
            ("99/12/31", "111111"),
            ("2020-01-01 +16", "000000"),
            ("5874898-01-01", "000000"),
            ("+infinity", "000111"),
            ("2020-01-01 at on", "111111"),
            ("12:00 pm", "000000"),
            ("13:00 pm", "000000"),
            ("12:", "000000"),
            ("12::30", "000000"),
            ("24:00:01", "000000"),
            ("04:05 PM", "000000"),
            ("allballs", "000000"),
            ("now", "111111"),
            ("040506", "111111"),
            ("12:00 m", "000000"),
            ("z", "000000"),
            ("12:00:00.", "000000"),
            ("12:00 xyz", "000000"),
            ("12:00 sat", "000000"),
            ("1:2:3:4", "000000"),
            ("1999-12-30 995959", "000001"),
            ("2020-01-01 t 12:00", "111111"),
            ("2020-01-01 t abcd-05", "111110"),
            ("epoch", "111111"),
            ("294277-01-01", "w"),
            ("294276-12-31 23:59:59.999999", "111111"),
            ("2020-01-01 25:00", "000000"),
            ("294277-01-01 +00", "000000"),
            ("294277-01-01 xyz", "111111"),
            ("4714-11-23 23:00:00-02 BC", "111111"),
            ("12:00 dst", "000000"),
            ("2020-01-01 12:00 xyz dst", "111111"),
            ("2020-01-01 12:00 foo5 dst", "000000"),
            ("12:00 +05 dst", "000000"),
            ("2020-01-01 -infinity", "111000"),
            ("infinity 2020-01-01", "000000"),
            ("Sat Jan 01 00:00:00 2000 PST", "111111"),
            ("2003-04-12 04:05:06 America/New_York", "111111"),
            ("20011225T040506.789-07", "111111"),
            ("2001-12-25 t 04:05:06", "111111"),
            ("h04mm05s06", "000000"),
            ("2020-01-01 j", "111000"),
            ("12-jan2020", "111111"),
            ("jan 2020 12", "111111"),
            ("99999999999-01-01", "000000"),
            ("2020-01-01 12:00 2147483648", "000000"),
            ("2020-01-01 12:00:00.5.5", "000000"),
            ("12:00:00.0000005", "000000"),
            ("2020-01-01 12:00:00.", "001111"),
            ("1-1-1", "111111"),
            ("70-1-1", "111111"),
            ("2020-366", "111111"),
            ("2019-366", "111111"),
            ("", "000000"),
            ("+", "000000"),
            ("0epoch", "111000"),
            ("\t1-1-1j", "111000"),
            ("1-1-1/y", "111000"),
            ("70-1-1y", "111000"),
            (" +infinity", "000111"),
            ("infinity", "111111"),
            ("-infinity", "111111"),
            ("1/1/70  +1/00:00:00.", "001111"),
            ("2000-01-01  00:00:00.", "001111"),
            ("xyz,today t 00:00:00.", "001111"),
            ("20200101 t foo5-+15:59:59 ", "111110"),
            ("0", "000000"),
            (" now", "111111"),
            ("1/2/3:s", "111000"),
            ("q\"EPOCH", "111000"),
            ("\t7_epoch", "111000"),
            ("J245118M", "111000"),
            ("+infinity at at", "000111"),
            ("2020/01/02T+15:59 12:30.5", "000111"),
            ("20200101 t -infxniry 12:00:00.1234567890123", "000111"),
            ("tomorrow  00:00:00.", "001111"),
            ("\t", "000000"),
            ("040506m", "111000"),
            ("1.2.3_s", "111000"),
            ("1/1/7h0", "111000"),
            ("epoch3B", "111000"),
            ("\tnow", "111111"),
            ("\t+infinity", "000111"),
            (" 12/05/2020T  +05 12:34:56.789 ", "000111"),
            ("EPOCH\t00:00:00.", "001000"),
            ("100+allballs,12::30.EPOCH\t", "001000"),
            ("2000-01-01 12:00:005.", "001111"),
            ("2020-01-01 12:34:56. B", "001111"),
            ("yesterday\t+0530 00:00:00.", "001111"),
        ];
        for (text, majors) in timestamptz {
            check(datetime_reads(DateTimeInput::TimestampTz, text), text, majors, "timestamptz");
        }
        let interval: &[(&str, &str)] = &[
            ("abc", "000000"),
            ("1 day", "111111"),
            ("1 day  day", "111100"),
            ("1 year 1 mon 1 day ! year", "111100"),
            ("4294968 millennium", "110000"),
            ("P-nanD", "110000"),
            ("P1Y2M", "111111"),
            ("P1e-310D", "000000"),
            ("P1e-400D", "000000"),
            ("1 day h", "111100"),
            ("1 day ago 1 hour", "111100"),
            ("infinity", "000011"),
            ("infinity ago", "000000"),
            ("-infinity", "000011"),
            ("+infinity", "000011"),
            ("1 day at", "000011"),
            ("3000000000", "001111"),
            ("100:30", "111111"),
            ("2147483648 days", "000000"),
            ("1 2:03", "111111"),
            ("@ 1 day", "111111"),
            ("1-2", "111111"),
            ("1-12", "000000"),
            ("P0001-02-03T04:05:06", "111111"),
            ("PT1.5S", "111111"),
            ("P1.5Y", "111111"),
            ("p1d", "000000"),
            ("P", "000000"),
            ("1 day 1 day", "000000"),
            ("178956970 years 8 mons", "000000"),
            ("2562047788:00:54.775808", "000000"),
            ("-2147483648 days", "111111"),
            ("1.", "001111"),
            ("s 1.", "001100"),
            ("1 h 2", "111111"),
            ("1 ago", "000000"),
            ("ago 1 day", "111100"),
            ("1e5 days", "000000"),
            ("P0x10D", "111111"),
            ("P1e15D", "000000"),
            ("P1e16D", "000000"),
            ("PT010203", "111111"),
            ("P00010203T040506", "111111"),
            ("1 day 2:03:04.5", "111111"),
            ("-1 2:03:04", "111111"),
            ("1 week 2 days", "111111"),
            ("0.5 millennium", "111111"),
            ("1 century 1 decade", "111111"),
            ("10 decades ago", "111111"),
            ("1 microsecond", "111111"),
            ("1.5 microseconds", "111111"),
            ("1 timezone", "000000"),
            ("1 qtr", "000000"),
            ("1 dow", "000000"),
            ("1 jan", "000000"),
            ("1 day today", "000000"),
            ("1 day on", "000011"),
            ("01:02:03:04", "000000"),
            ("1:2.5", "111111"),
            ("1:2:3.", "001111"),
            ("+1:02:03", "111111"),
            ("-1:02:03 1 day", "111111"),
            ("1 day -1:02:03 +1 hour", "000000"),
            ("", "000000"),
            ("(", "000000"),
            ("0", "111111"),
            ("1", "111111"),
            ("m:1.", "001100"),
            ("c 1.\t", "001100"),
            ("s 1.:mil", "001100"),
            (".", "001111"),
            (".<Y", "001111"),
            ("1.\t", "001111"),
            ("on5", "000011"),
            ("1 on", "000011"),
            ("12mm", "000011"),
            ("at,5", "000011"),
            ("H1", "111100"),
            ("M1d", "111100"),
            ("c|0", "111100"),
            ("D11Y", "111100"),
            ("1 mon", "111111"),
            ("-2147483648decade", "110000"),
            ("2562047788:00:54.75807", "001111"),
            ("2562047788:00:54.77507", "001111"),
            ("2562047788:00:54.775807", "001111"),
            ("\t", "000000"),
            ("1on", "000011"),
            ("at 1", "000011"),
            ("at1.", "000011"),
            ("d1", "111100"),
            ("h3", "111100"),
            ("m3", "111100"),
            ("M1s", "111100"),
            (".W", "001111"),
            (">.", "001111"),
            ("d:2:3.", "001100"),
            ("decade 1.", "001100"),
            ("min 1:2.d", "001100"),
            ("minute,1.", "001100"),
            ("P-NanD", "110000"),
            ("-2147483648,decade", "110000"),
            ("-2147483648 century 1:02:03", "110000"),
            ("0 day", "111111"),
            ("2562047788:00:54.77580", "001111"),
            (".Y", "001111"),
            ("0.", "001111"),
            ("[.", "001111"),
            ("12:j", "000011"),
            ("on 1", "000011"),
            ("\t5,at", "000011"),
            ("D5", "111100"),
            ("h8", "111100"),
            ("D1\"", "111100"),
            ("D1]", "111100"),
            ("ago 1.", "001100"),
            ("dec,1.", "001100"),
            ("mil 1.", "001100"),
            ("ms:1:2:3.", "001100"),
            ("2147483647 decade", "110000"),
            ("-2147483648,day ago", "110000"),
            ("1-11 -2147483648 week", "110000"),
        ];
        // The servers' answer is the union over the forms above; this takes
        // it over every qualifier, a wider union these cases do not tell
        // apart from it.
        let any_qualifier = |text| {
            std::iter::once(None)
                .chain(IntervalQualifier::ALL.map(Some))
                .any(|qualifier| interval_reads(text, qualifier))
        };
        for (text, majors) in interval {
            check(any_qualifier(text), text, majors, "interval");
        }
    }

    /// **An `interval` is read exactly where some supported major reads it
    /// under its column's field qualifier** (I84). Each case was put to
    /// `interval_in` at 13.23, 14.24, 15.19, 16.15, 17.11 and 18.6 under both
    /// `IntervalStyle`s, through a literal of each type below, so the input
    /// function is handed the qualifier's typmod; the fourteen digits say
    /// under which some major reads it: none, then `year`, `month`, `day`,
    /// `hour`, `minute`, `second`, `year to month`, `day to hour`, `day to
    /// minute`, `day to second`, `hour to minute`, `hour to second` and
    /// `minute to second`. The cases are up to three of each pattern of
    /// verdicts across the qualifiers, hand-written first and then shortest,
    /// from a differential run of 3,000 texts put to each qualifier and to
    /// `interval(0)` and `interval second(0)`, whose precision decided none,
    /// which this build read as the servers did.
    #[test]
    fn an_interval_is_read_exactly_where_some_major_reads_it_under_its_qualifier() {
        use IntervalQualifier as Q;
        let qualifiers = [
            None,
            Some(Q::Year),
            Some(Q::Month),
            Some(Q::Day),
            Some(Q::Hour),
            Some(Q::Minute),
            Some(Q::Second),
            Some(Q::YearToMonth),
            Some(Q::DayToHour),
            Some(Q::DayToMinute),
            Some(Q::DayToSecond),
            Some(Q::HourToMinute),
            Some(Q::HourToSecond),
            Some(Q::MinuteToSecond),
        ];
        let cases: &[(&str, &str)] = &[
            ("59:60", "00000000000001"),
            ("1:60", "00000000000001"),
            ("\t1:60", "00000000000001"),
            ("1 1", "00001000100000"),
            ("1.5 1", "00001000100000"),
            (".5.", "00001000100000"),
            (" 1-11 178956971:-2147483648", "00010000000000"),
            ("1000000 1:02:03 -2147483648", "00100001000000"),
            ("12:-2147483648", "00110001000000"),
            ("1:2.5-2147483647", "00110001000000"),
            ("+1-2 1:2:3. 12", "01010000000000"),
            ("D1-2 +1:02:>03", "01010000000000"),
            (" -1$2:-5", "01100001000000"),
            (". 1:2_:3", "01100001000000"),
            ("5:+1", "01110001000000"),
            ("0:1/2", "01110001000000"),
            ("4294S~8", "01111101110100"),
            ("1S8956970", "01111101110100"),
            ("9223372036854", "10000010001011"),
            ("214748368347", "10000010001011"),
            ("217174483647", "10000010001011"),
            ("2562047789", "10000110011111"),
            ("3000000000", "10000110011111"),
            ("-9717895697", "10000110011111"),
            ("2147483648", "10001110111111"),
            ("2562047788", "10001110111111"),
            ("2147484648", "10001110111111"),
            ("178956970-1:2", "10011110111111"),
            (" 1-2 -2147483648", "10011110111111"),
            ("-178956971", "10111111111111"),
            ("-2147483648", "10111111111111"),
            ("4y94968", "10111111111111"),
            ("1-2 3", "11011110111111"),
            ("1-:2", "11011110111111"),
            ("1-:1.", "11011110111111"),
            ("1 day 1", "11101111111111"),
            ("1C:6D0", "11101111111111"),
            ("1 dayS 1", "11101111111111"),
            ("1 hour 1", "11110111011111"),
            ("7hour 1", "11110111011111"),
            ("1537228h67", "11110111011111"),
            ("1>m2", "11111011101011"),
            ("17895M970", "11111011101011"),
            ("-2147483648:00", "11111111111110"),
            ("1 100:30", "11111111111110"),
            ("91:2", "11111111111110"),
        ];
        for (text, read) in cases {
            for (qualifier, want) in qualifiers.iter().zip(read.chars()) {
                assert_eq!(
                    interval_reads(text, *qualifier),
                    want == '1',
                    "{text:?} under {qualifier:?}"
                );
            }
        }
    }
}
