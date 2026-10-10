//! A `time`'s and a timestamp's value as PostgreSQL stores it, which its
//! comparison orders, where `decode`'s readers yield what Arrow holds: not
//! `decode`'s API, so `internal` re-exports it.

use super::{
    POSTGRES_EPOCH_UNIX_MICROS, Read, parse_time_of_day, push_timestamp, timestamp_micros_wide,
};

/// A `time`'s microseconds since midnight as PostgreSQL holds it, its inclusive
/// upper bound `24:00:00` included — what its comparison orders (`crate::key`),
/// where [`decode_time64_micros`](super::decode_time64_micros) is what Arrow
/// holds.
pub fn time_of_day_micros(s: &str) -> Read<i64> {
    let (seconds, micros) = parse_time_of_day(s)?;
    Ok(seconds * 1_000_000 + micros)
}

/// A finite timestamp's microseconds from PostgreSQL's epoch, 2000-01-01 UTC —
/// the `int64` PostgreSQL stores it as (I49), so every value it admits fits,
/// the three decades
/// [`decode_timestamp_micros`](super::decode_timestamp_micros) cannot count
/// from 1970 included. What its comparison orders (`crate::key`), where
/// [`decode_timestamp_micros`](super::decode_timestamp_micros) is what Arrow
/// holds; [`Unread::Unparsed`](super::Unread::Unparsed) for the infinities, and
/// an [`Unread`](super::Unread) for text that is not a timestamp.
pub fn timestamp_postgres_micros(s: &str, with_tz: bool) -> Read<i64> {
    let postgres = timestamp_micros_wide(s, with_tz)? - POSTGRES_EPOCH_UNIX_MICROS;
    Ok(i64::try_from(postgres).expect("PostgreSQL's range is an i64 of microseconds"))
}

/// [`render_timestamp_micros`](super::render_timestamp_micros) of a
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
