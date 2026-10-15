//! The dispatch building an [`OrderKey`] from a field's or a filter
//! literal's text, over the type families under `crate::value`, each holding
//! its own key type, and the comparison of two keys. Not the codec's API — an
//! ordering over these representations is a surface nobody has designed —
//! so `internal` re-exports what a crate above calls, and an embedder orders
//! a value through the namespace's `ValueKey`.

use std::cmp::Ordering;

use arrow_buffer::i256;
use pgdt_model::PostgresInvalidValues;

use crate::compare::{CompareKind, ComparisonSemantics};
use crate::decode::{Read, Unread};
use crate::value::bool::{bool_unread, decode_bool};
use crate::value::bytea::{bytea_unread, decode_bytea};
use crate::value::datetime::grammar::DateTimeInput;
use crate::value::datetime::{
    date_days, datetime_read, extract_offset, interval_parts, interval_read, interval_span,
    parse_time_of_day, time_of_day_micros, timestamp_postgres_micros,
};
use crate::value::float::{float_field, float_in};
use crate::value::int::{int_in, oid_unread};
use crate::value::json::{Jsonb, parse_jsonb};
use crate::value::network::{NetworkKey, macaddr_octets, macaddr_unread, macaddr8_in, network_in};
use crate::value::numeric::{
    NumericColumn, NumericKey, decimal_unscaled_digits, numeric_in_stores, numeric_unread,
    typmod_unscaled_digits,
};
use crate::value::text::char_typmod_refuses;
use crate::value::uuid::decode_uuid;

/// A `jsonb` comparison key: the document, with a **top-level scalar wrapped
/// in the one-element pseudo-array PostgreSQL stores it in** (I41). Modelling
/// the wrapper is what gives `1 < [1]`, `1 < [1,2]` and `1 > []`.
fn jsonb_key(text: &str) -> Read<OrderKey> {
    Ok(OrderKey::Jsonb(match parse_jsonb(text)? {
        container @ (Jsonb::Array { .. } | Jsonb::Object(_)) => container,
        scalar => Jsonb::Array { raw_scalar: true, items: vec![scalar] },
    }))
}

/// One side of an ordering comparison, decoded from text per the column's
/// [`CompareKind`]. Both sides of any one comparison come from the same kind,
/// so a *finite* variant mismatch is unreachable by construction.
///
/// Three variants are not values of the column's Arrow type at all:
/// `infinity`, `-infinity` and `numeric`'s `NaN` are legal values of their
/// declared PostgreSQL types with a total order (I34), and are carried as
/// their position in that order (`docs/design/decisions.md`, "D56"). A
/// `real`/`double precision` special is not here: IEEE has all three, so the
/// column's own decoder yields them inside `Float` and [`pg_float_cmp`]
/// orders them PostgreSQL's way.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderKey {
    /// PostgreSQL's `-infinity`, below every finite value of its type.
    NegativeInfinity,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(i256),
    /// A bare `numeric`, whose digits do not fit any fixed-width integer.
    Numeric(NumericKey),
    /// An `interval`'s span in microseconds, which is 128 bits wide because
    /// PostgreSQL's own `interval_cmp_value` is (I40).
    Interval(i128),
    /// An `interval`'s three fields as `Interval(MonthDayNano)` holds them —
    /// months, days, microseconds — compared one after another, as Arrow
    /// compares that type.
    IntervalFields(i32, i32, i64),
    /// A `time with time zone`: the UTC-equivalent instant, then the stored
    /// zone as PostgreSQL stores it — seconds *west* of GMT, the negation of
    /// the sign the value displays.
    TimeTz {
        utc: i64,
        zone: i64,
    },
    Network(NetworkKey),
    /// A `jsonb` document, with a top-level scalar already wrapped in the
    /// pseudo-array PostgreSQL stores it in — see [`jsonb_key`].
    Jsonb(Jsonb),
    Bytes(Vec<u8>),
    Text(String),
    /// PostgreSQL's `infinity`, above every finite value of its type.
    PositiveInfinity,
    /// `numeric`'s `NaN`, which orders above `infinity` and equals itself
    /// (I34). Reached through [`CompareKind::Decimal`] and
    /// [`CompareKind::Numeric`].
    NotANumber,
}

/// The rank of a finite value — the second of the four [`OrderKey::rank`]
/// classes, above `-infinity` and below `infinity` and `NaN`, and the only one whose members are compared by value.
const FINITE: u8 = 1;

impl OrderKey {
    /// Where this key sits in PostgreSQL's total order relative to the finite
    /// values of its own type. Ranks are compared before values are, so a
    /// special value never collides with a finite one a sentinel could have —
    /// `i64::MAX` micros since 1970 is a date PostgreSQL itself accepts
    /// (`docs/design/decisions.md`, "D56").
    fn rank(&self) -> u8 {
        match self {
            Self::NegativeInfinity => 0,
            Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Decimal(_)
            | Self::Numeric(_)
            | Self::Interval(_)
            | Self::IntervalFields(..)
            | Self::TimeTz { .. }
            | Self::Network(_)
            | Self::Jsonb(_)
            | Self::Bytes(_)
            | Self::Text(_) => FINITE,
            Self::PositiveInfinity => 2,
            Self::NotANumber => 3,
        }
    }

    /// The heap this key holds, as the sizes the allocator was asked for.
    pub(crate) fn heap_bytes(&self) -> u64 {
        match self {
            Self::Numeric(numeric) => numeric.heap_bytes(),
            Self::Jsonb(jsonb) => jsonb.heap_bytes(),
            Self::Bytes(bytes) => bytes.capacity() as u64,
            Self::Text(text) => text.capacity() as u64,
            Self::NegativeInfinity
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Decimal(_)
            | Self::Interval(_)
            | Self::IntervalFields(..)
            | Self::TimeTz { .. }
            | Self::Network(_)
            | Self::PositiveInfinity
            | Self::NotANumber => 0,
        }
    }
}

/// PostgreSQL's special values, for the kinds whose columns can hold one and
/// in the exact spelling that type's own `*_out` writes (I34): `date`,
/// `timestamp`, `timestamptz` and `interval` write `infinity`/`-infinity`,
/// and a `numeric` writes `NaN`, a bare one its infinities too (below).
/// Nothing else is accepted — a `date` field or
/// literal reading `Infinity` is a decode failure
/// (`docs/design/decisions.md`, "D55").
///
/// `interval`'s two are read on every file: they are v17 values no older
/// server could have written, so accepting the spelling unconditionally is
/// the union rule (I35) rather than a claim about the file's own major.
/// `real`/`double precision` are absent because IEEE represents all three and
/// [`decode_f64`](crate::value::float::decode_f64) already returns them.
///
/// The two `numeric` kinds differ only about the infinities (I34): a
/// [`CompareKind::Decimal`] column, and a `numeric(p,s)` past 76 digits, can
/// hold a `NaN` and never an infinity, while a *bare* `numeric` has all three
/// in `numeric_out`'s own spellings, which capitalize where `date_out`'s do
/// not.
pub fn special_order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    match kind {
        CompareKind::Date
        | CompareKind::Timestamp { .. }
        | CompareKind::Interval { .. }
        | CompareKind::IntervalFields { .. } => match text {
            "infinity" => Some(OrderKey::PositiveInfinity),
            "-infinity" => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        CompareKind::Decimal { .. } => (text == "NaN").then_some(OrderKey::NotANumber),
        CompareKind::Numeric { infinities, .. } => match text {
            "NaN" => Some(OrderKey::NotANumber),
            "Infinity" if *infinities => Some(OrderKey::PositiveInfinity),
            "-Infinity" if *infinities => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        _ => None,
    }
}

/// A `time with time zone`, split into the UTC-equivalent instant and the
/// zone PostgreSQL stores — seconds *west* of GMT, the negation of the offset
/// the value displays. `timetz_cmp_internal` sorts by the first and breaks
/// ties with the second (I40), so `00:00:00+00` and `01:00:00+01` are the
/// same instant and still not equal.
fn timetz_key(text: &str) -> Read<OrderKey> {
    let (time_only, displayed) = extract_offset(text)?;
    let (seconds, micros) = parse_time_of_day(time_only)?;
    let zone = -displayed;
    // Both parts are bounded above, so neither sum can overflow.
    let utc = seconds * 1_000_000 + micros + zone * 1_000_000;
    Ok(OrderKey::TimeTz { utc, zone })
}

/// An `inet` or `cidr` value, as `network_in` reads it
/// ([`network_in`](crate::value::network::network_in)): `pg_inet_net_ntop`'s
/// `<address>[/<bits>]`, with `cidr_out` always appending the netmask and
/// `inet_out` omitting it when it is the family's full width (I40), and every
/// other spelling the server reads, a `cidr`'s abbreviated network among them.
/// Every refusal is the server's (I67), a `cidr`'s bit set below its netmask
/// included: the *only* thing separating the two types, their comparison being
/// identical.
fn network_key(text: &str, cidr: bool) -> Read<OrderKey> {
    let (v6, bits, addr) = network_in(text, cidr).ok_or(Unread::Refused)?;
    Ok(OrderKey::Network(NetworkKey { v6, bits, addr }))
}

/// Decode one already-COPY-unescaped value into a comparable key. `None` when
/// the text is not a value of that type — for a *field* `Error::FieldDecode`,
/// for the filter's own literal `Error::PredicateValueDecode`, raised before
/// a row is read.
///
/// A special value is answered by [`special_order_key`] first: it is a legal
/// value of the declared type that the *Arrow* type cannot hold, a separate
/// population from text that is malformed for the column.
///
/// **A key is a function of the kind and the text alone**, and nothing here
/// or in [`compare_keys`] reads a row: two values that never shared one — a
/// field and a filter's literal, a stored bound and a literal, two neighbours
/// being gathered — are ordered by the one key the filter uses.
pub fn order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    field_key(kind, text).ok()
}

/// A field's key as a query reads the field: [`order_key`]'s, but for a float
/// its type's `*_in` refuses, keyed under [`PostgresInvalidValues::Ignore`] as
/// the typed read emits it ([`float_field`](crate::value::float::float_field)), so a filter and the
/// batch agree on it.
pub fn read_key(
    kind: &CompareKind,
    text: &str,
    invalid: PostgresInvalidValues,
) -> Option<OrderKey> {
    match (field_key(kind, text), kind) {
        (Ok(key), _) => Some(key),
        (Err(Unread::Refused), CompareKind::Float32) => {
            Some(OrderKey::Float(f64::from(float_field::<f32>(text, invalid)?)))
        }
        (Err(Unread::Refused), CompareKind::Float64) => {
            Some(OrderKey::Float(float_field::<f64>(text, invalid)?))
        }
        (Err(_), _) => None,
    }
}

/// Which side of a comparison
/// [`nested_key`](crate::value::nested::key::nested_key) reads: a filter's
/// literal, or a field, read as the query reads one its type's `*_in` refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Literal,
    Field(PostgresInvalidValues),
}

/// `NAMEDATALEN`: `enum_in` refuses a text of this many bytes or more before
/// looking it up, no label being so long (I70).
const ENUM_LABEL_LIMIT: usize = 64;

/// [`order_key`], saying why a text has no key: refused by the type's `*_in`,
/// which fails the parse reading it as a field, or not read here.
pub(crate) fn field_key(kind: &CompareKind, text: &str) -> Read<OrderKey> {
    if let Some(special) = special_order_key(kind, text) {
        return Ok(special);
    }
    let unparsed = Unread::Unparsed;
    Ok(match kind {
        CompareKind::Bool => OrderKey::Bool(decode_bool(text).ok_or_else(|| bool_unread(text))?),
        // A literal in DataFusion's semantics is read as an `i64` instead, by
        // [`literal_key`].
        CompareKind::Int { bytes } => OrderKey::Int(int_in(text, *bytes)?),
        // `u32`, and the width *is* the refusal: `oidin` reads `-1` as
        // 4294967295 and this build does not implement that wrap, so a
        // negative literal is `Error::PredicateValueDecode`. Every value the
        // column can hold widens into `i64` unchanged.
        CompareKind::UnsignedInt => {
            OrderKey::Int(text.parse::<u32>().map_err(|_| oid_unread(text))?.into())
        }
        CompareKind::Float32 => OrderKey::Float(f64::from(float_in::<f32>(text)?)),
        CompareKind::Float64 => OrderKey::Float(float_in::<f64>(text)?),
        // A field is put through its typmod, rounded to the scale and refused
        // past the precision as `COPY` refuses it (I51). A literal is read
        // exactly, by [`literal_key`].
        CompareKind::Decimal { precision, scale } => OrderKey::Decimal(
            i256::from_string(&typmod_unscaled_digits(text, *precision, i16::from(*scale))?)
                .ok_or(unparsed)?,
        ),
        CompareKind::Numeric { typmod: Some(typmod), .. } => {
            OrderKey::Numeric(NumericKey::from_unscaled(
                &typmod_unscaled_digits(text, typmod.precision, typmod.scale)?,
                typmod.scale,
            ))
        }
        // A bare column's field is bounded by `numeric_in` (I63) once read:
        // the bounds count the digits a grammar with no exponent reads, and
        // `numeric_in` stores a text whose exponent brings its scale back
        // within them. A typmod'd one never reaches the bounds, the typmod
        // rounding first. A literal in PostgreSQL's semantics is bounded by
        // [`literal_key`].
        CompareKind::Numeric { infinities: bare, typmod: None } => {
            let key = NumericKey::parse(text).ok_or_else(|| {
                let column = if *bare { NumericColumn::Bare } else { NumericColumn::UnreadTypmod };
                numeric_unread(text, column)
            })?;
            if *bare && !numeric_in_stores(text) {
                return Err(Unread::Refused);
            }
            OrderKey::Numeric(key)
        }
        // A label the type does not declare is no value of the column, which
        // `enum_in` refuses: where the declared labels are all the type
        // holds, or the text is longer than any label can be, and otherwise
        // it may be one this build did not read. The linear scan is over a
        // label list, a handful of entries in practice.
        // pg-refuses: I70
        CompareKind::Enum { labels, exact } => {
            let position = labels.iter().position(|l| l == text);
            let refused = *exact || text.len() >= ENUM_LABEL_LIMIT;
            OrderKey::Int(position.ok_or(if refused { Unread::Refused } else { unparsed })? as i64)
        }
        CompareKind::Date => {
            OrderKey::Int(datetime_read(date_days(text), DateTimeInput::Date, text)?.into())
        }
        CompareKind::Time => {
            OrderKey::Int(datetime_read(time_of_day_micros(text), DateTimeInput::Time, text)?)
        }
        // From PostgreSQL's epoch, not Arrow's: every value the server admits
        // keys, those past what `i64` counts from 1970 among them (I49).
        CompareKind::Timestamp { with_tz } => {
            let input =
                if *with_tz { DateTimeInput::TimestampTz } else { DateTimeInput::Timestamp };
            OrderKey::Int(datetime_read(timestamp_postgres_micros(text, *with_tz), input, text)?)
        }
        // A field the reader does not read is refused where `interval_in`
        // refuses it under the column's qualifier. A literal is read under
        // none ([`literal_key`]).
        CompareKind::Interval { qualifier } => {
            OrderKey::Interval(interval_read(interval_span(text), text, *qualifier)?)
        }
        CompareKind::IntervalFields { qualifier } => {
            let (months, days, time) = interval_read(interval_parts(text), text, *qualifier)?;
            OrderKey::IntervalFields(months, days, time)
        }
        CompareKind::TimeTz => datetime_read(timetz_key(text), DateTimeInput::TimeTz, text)?,
        CompareKind::Network { cidr } => network_key(text, *cidr)?,
        CompareKind::MacAddr { octets } => {
            OrderKey::Bytes(macaddr_octets(text, *octets).ok_or_else(|| match octets {
                8 if macaddr8_in(text).is_none() => Unread::Refused,
                8 => unparsed,
                _ => macaddr_unread(text),
            })?)
        }
        CompareKind::Jsonb => jsonb_key(text)?,
        // Every refusal of the reader is `uuid_in`'s (I64).
        CompareKind::Uuid => OrderKey::Bytes(decode_uuid(text).ok_or(Unread::Refused)?.to_vec()),
        CompareKind::Bytea => {
            OrderKey::Bytes(decode_bytea(text).ok_or_else(|| bytea_unread(text))?)
        }
        // A field past its typmod's length is refused, and one within it is
        // keyed as written. A literal is coerced with no typmod
        // ([`literal_key`]).
        CompareKind::Text { length } => {
            if length.is_some_and(|length| char_typmod_refuses(text, length)) {
                return Err(Unread::Refused);
            }
            OrderKey::Text(text.to_string())
        }
        // `bcTruelen` on both sides, which is what makes this the server's
        // comparison rather than one over the padding (I38). The blank is
        // ASCII `0x20` and nothing else; a tab is a value byte.
        CompareKind::PaddedText { length } => {
            if length.is_some_and(|length| char_typmod_refuses(text, length)) {
                return Err(Unread::Refused);
            }
            OrderKey::Text(text.trim_end_matches(' ').to_string())
        }
    })
}

/// A filter literal's key, which is [`order_key`]'s — a field's — but for an
/// integer in DataFusion's semantics, read as an `i64` whatever the column's
/// width, a float in DataFusion's semantics, read as the parse rounds it, an
/// infinity past the type's range, and a `numeric` in either: read within `numeric_in`'s bounds in
/// PostgreSQL's, and exactly, neither rounded to the scale nor refused past
/// the precision, the server coercing a literal with no typmod where it puts a
/// field through the typmod (I63) — and a text of any length and an `interval`
/// under no field qualifier for the same reason. In DataFusion's semantics a literal is
/// DataFusion's value, not the server's (`roadmap.md`, "A literal is
/// guaranteed in `*_out`'s form and never read past `*_in`'s").
pub fn literal_key(
    kind: &CompareKind,
    text: &str,
    semantics: ComparisonSemantics,
) -> Option<OrderKey> {
    match (semantics, kind) {
        (ComparisonSemantics::DataFusion, CompareKind::Int { .. }) => {
            Some(OrderKey::Int(text.parse::<i64>().ok()?))
        }
        (ComparisonSemantics::DataFusion, CompareKind::Float32) => {
            Some(OrderKey::Float(f64::from(text.parse::<f32>().ok()?)))
        }
        (ComparisonSemantics::DataFusion, CompareKind::Float64) => {
            Some(OrderKey::Float(text.parse::<f64>().ok()?))
        }
        (
            ComparisonSemantics::Postgres,
            CompareKind::Decimal { .. } | CompareKind::Numeric { .. },
        ) if !numeric_in_stores(text) => None,
        (_, CompareKind::Decimal { scale, .. }) => special_order_key(kind, text).or_else(|| {
            let unscaled = decimal_unscaled_digits(text, *scale)?;
            Some(OrderKey::Decimal(i256::from_string(&unscaled)?))
        }),
        (_, CompareKind::Numeric { typmod: Some(_), .. }) => special_order_key(kind, text)
            .or_else(|| Some(OrderKey::Numeric(NumericKey::parse(text)?))),
        (_, CompareKind::Text { .. }) => order_key(&CompareKind::Text { length: None }, text),
        (_, CompareKind::PaddedText { .. }) => {
            order_key(&CompareKind::PaddedText { length: None }, text)
        }
        (_, CompareKind::Interval { .. }) => {
            order_key(&CompareKind::Interval { qualifier: None }, text)
        }
        (_, CompareKind::IntervalFields { .. }) => {
            order_key(&CompareKind::IntervalFields { qualifier: None }, text)
        }
        _ => order_key(kind, text),
    }
}

/// PostgreSQL's float order, not Rust's: `NaN` is greater than every other
/// value, infinities included, and `NaN = NaN` is true (I33), where Rust's
/// `partial_cmp` answers `None`. `real`/`double precision` are the only
/// columns whose key is a float `NaN`: a `NaN` in a `numeric(p,s)` column is
/// keyed as a special value ([`special_order_key`]) before any float is read,
/// having no `Decimal128` representation (I4).
fn pg_float_cmp(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).expect("neither side is NaN"),
    }
}

/// Rank first, value second. A special value on either side is decided by
/// [`OrderKey::rank`] alone, so two of the same special are equal —
/// `-infinity = -infinity`, `infinity = infinity`, `NaN = NaN` (I34) — and a
/// special against a finite never has to name a number.
///
/// Over the keys of one kind this is a **total order** — reflexive,
/// antisymmetric and transitive — which a running minimum, maximum or
/// sortedness flag depends on and a single comparison does not; the oracle
/// test `the_key_is_a_total_order_over_every_committed_value` pins it.
///
/// Both sides come from one column's [`CompareKind`], so a finite pair of
/// two representations is unreachable here; [`order_keys`] is the comparison
/// that answers one.
pub fn compare_keys(a: &OrderKey, b: &OrderKey) -> Ordering {
    order_keys(a, b).unwrap_or_else(|| {
        unreachable!("both sides of a comparison decode through one column's `CompareKind`")
    })
}

/// [`compare_keys`] over two keys that may not share a kind: `None` where
/// both are finite and their representations differ, which no order relates.
#[inline]
pub(crate) fn order_keys(a: &OrderKey, b: &OrderKey) -> Option<Ordering> {
    let (rank_a, rank_b) = (a.rank(), b.rank());
    if rank_a != rank_b {
        return Some(rank_a.cmp(&rank_b));
    }
    Some(match (a, b) {
        (OrderKey::Bool(x), OrderKey::Bool(y)) => x.cmp(y),
        (OrderKey::Int(x), OrderKey::Int(y)) => x.cmp(y),
        (OrderKey::Float(x), OrderKey::Float(y)) => pg_float_cmp(*x, *y),
        (OrderKey::Decimal(x), OrderKey::Decimal(y)) => x.cmp(y),
        (OrderKey::Numeric(x), OrderKey::Numeric(y)) => x.cmp(y),
        (OrderKey::Interval(x), OrderKey::Interval(y)) => x.cmp(y),
        (OrderKey::IntervalFields(m, d, t), OrderKey::IntervalFields(n, e, u)) => {
            (m, d, t).cmp(&(n, e, u))
        }
        (
            OrderKey::TimeTz { utc: utc_x, zone: zone_x },
            OrderKey::TimeTz { utc: utc_y, zone: zone_y },
        ) => utc_x.cmp(utc_y).then_with(|| zone_x.cmp(zone_y)),
        (OrderKey::Network(x), OrderKey::Network(y)) => x.cmp(y),
        (OrderKey::Jsonb(x), OrderKey::Jsonb(y)) => x.cmp(y),
        (OrderKey::Bytes(x), OrderKey::Bytes(y)) => x.cmp(y),
        (OrderKey::Text(x), OrderKey::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ if rank_a != FINITE => Ordering::Equal,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
