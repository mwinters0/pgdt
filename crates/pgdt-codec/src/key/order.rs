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
use crate::value::bool::BoolIn;
use crate::value::bytea::{bytea_unread, decode_bytea};
use crate::value::datetime::grammar::DateTimeInput;
use crate::value::datetime::{
    date_days, datetime_read, extract_offset, interval_parts, interval_read, interval_span,
    parse_time_of_day, time_of_day_micros, timestamp_postgres_micros, within_workbuf,
};
use crate::value::float::{float_field, float_in};
use crate::value::int::{IntIn, OidIn};
use crate::value::json::{Jsonb, parse_jsonb};
use crate::value::network::{NetworkKey, macaddr_octets, macaddr_unread, macaddr8_in, network_in};
use crate::value::numeric::{
    NumericColumn, NumericIn, NumericKey, decimal_unscaled_digits, numeric_in_stores,
};
use crate::value::text::char_typmod_refuses;
use crate::value::uuid::UuidIn;

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
/// [`CompareKind`]. Both sides of a comparison [`compare_keys`] makes come
/// from the same kind, so a *finite* variant mismatch is unreachable there;
/// `order_keys` orders two that may not share one.
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
pub(crate) const FINITE: u8 = 1;

impl OrderKey {
    /// Where this key sits in PostgreSQL's total order relative to the finite
    /// values of its own type. Ranks are compared before values are, so a
    /// special value never collides with a finite one a sentinel could have —
    /// `i64::MAX` micros since 1970 is a date PostgreSQL itself accepts
    /// (`docs/design/decisions.md`, "D56").
    pub(crate) fn rank(&self) -> u8 {
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

    /// The heap this key holds, as the sizes the allocator was asked for:
    /// what gathering charges a keyed bound or row-order value
    /// (`pgdt_mapping`'s `StatisticsAccount`).
    pub fn heap_bytes(&self) -> u64 {
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
    // pg-refuses: I83 — past `timetz_in`'s work buffer.
    within_workbuf(text, DateTimeInput::TimeTz.workbuf())?;
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
/// the text is not a value of that type, or one this build does not read
/// ([`field_key`]'s `Unparsed`) — for a *field* `Error::FieldDecode`,
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

/// [`order_key`], saying why a text has no key: refused by the type's `*_in`,
/// which fails the parse reading it as a field, or not read here.
///
/// A kind whose reader is a push machine — an integer, `oid`, `boolean`, an
/// enum, `uuid`, `numeric` — is read by feeding it the text ([`KeyRead`]),
/// the one grammar a strict check feeds a part's runs to. A literal of an integer in
/// DataFusion's semantics is read as an `i64` instead, by [`literal_key`];
/// one of an `oid` is a `u32`, and the width *is* the refusal: `oidin` reads
/// `-1` as 4294967295 and this build does not implement that wrap, so a
/// negative literal is `Error::PredicateValueDecode`.
pub fn field_key(kind: &CompareKind, text: &str) -> Read<OrderKey> {
    if let Some(mut read) = KeyRead::machine(kind) {
        read.whole(text.as_bytes());
        return read.finish();
    }
    if let Some(special) = special_order_key(kind, text) {
        return Ok(special);
    }
    let unparsed = Unread::Unparsed;
    Ok(match kind {
        CompareKind::Bool
        | CompareKind::Int { .. }
        | CompareKind::UnsignedInt
        | CompareKind::Enum { .. }
        | CompareKind::Uuid
        | CompareKind::Decimal { .. }
        | CompareKind::Numeric { .. } => unreachable!("a push machine reads it"),
        CompareKind::Float32 => OrderKey::Float(f64::from(float_in::<f32>(text)?)),
        CompareKind::Float64 => OrderKey::Float(float_in::<f64>(text)?),
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
        CompareKind::Bytea => {
            OrderKey::Bytes(decode_bytea(text).ok_or_else(|| bytea_unread(text.bytes()))?)
        }
        // A field past its typmod's length is refused, and one within it is
        // keyed as written. A literal is coerced with no typmod
        // ([`literal_key`]).
        CompareKind::Text { length, .. } => {
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
        (_, CompareKind::Text { .. }) => {
            order_key(&CompareKind::Text { length: None, varying: false }, text)
        }
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
pub(crate) fn pg_float_cmp(a: f64, b: f64) -> Ordering {
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
/// both are finite and their representations differ. Which two kinds' keys
/// it may be asked to order at all is `ValueKey::compare`'s to say
/// (`crate::key::relate`).
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

/// **The order a stored bound and a block's row order are kept in**:
/// [`compare_keys`], the order a filter compares by, with a float's `-0`
/// below `0` where it ties them, as IEEE `totalOrder` — the order Arrow sorts
/// a float by and DataFusion takes its `MIN`/`MAX` in — places them. A
/// refinement, so an extreme in it is one in the filter's order and a block
/// sorted in it is sorted in that order too (`docs/design/decisions.md`,
/// "D79"). Over two keys of one column's kind, as [`compare_keys`] is: the
/// order the statistics gatherer keeps its bounds and row order in, and a
/// summary reads them back in.
pub fn compare_keys_stored(a: &OrderKey, b: &OrderKey) -> Ordering {
    compare_keys(a, b).then_with(|| match (a, b) {
        (OrderKey::Float(x), OrderKey::Float(y)) => x.total_cmp(y),
        _ => Ordering::Equal,
    })
}

/// **An order key read off a text fed a byte at a time**
/// (`docs/design/decisions.md`, "D144", "D147", "D148"): [`field_key`]'s
/// answer, a `&str` fed whole being one source, a strict check's part fed its
/// runs another. A kind whose reader is a push machine holds that machine's
/// state, a `numeric`'s the digits its column reads; any other collects its
/// text for `field_key`'s slice reader.
#[derive(Debug, Clone)]
pub(crate) enum KeyRead<'k> {
    Int {
        bytes: u8,
        read: IntIn,
    },
    Oid(OidIn),
    Bool(BoolIn),
    Enum(EnumIn<'k>),
    Uuid(UuidIn),
    /// A `numeric`, read as its column reads one.
    Numeric {
        kind: &'k CompareKind,
        read: Box<NumericIn>,
    },
    /// A kind whose reader takes a slice: its text, no longer than it.
    Collected {
        kind: &'k CompareKind,
        text: Vec<u8>,
    },
}

impl<'k> KeyRead<'k> {
    /// The reader of `kind`.
    pub(crate) fn new(kind: &'k CompareKind) -> Self {
        Self::machine(kind).unwrap_or(KeyRead::Collected { kind, text: Vec::new() })
    }

    /// The push machine reading `kind`, where its reader is one.
    pub(crate) fn machine(kind: &'k CompareKind) -> Option<Self> {
        Some(match kind {
            CompareKind::Int { bytes } => KeyRead::Int { bytes: *bytes, read: IntIn::default() },
            CompareKind::UnsignedInt => KeyRead::Oid(OidIn::default()),
            CompareKind::Bool => KeyRead::Bool(BoolIn::default()),
            CompareKind::Enum { labels, exact, .. } => KeyRead::Enum(EnumIn {
                labels,
                exact: *exact,
                label: [0; ENUM_LABEL_LIMIT],
                len: 0,
            }),
            CompareKind::Uuid => KeyRead::Uuid(UuidIn::default()),
            CompareKind::Decimal { precision, scale } => KeyRead::Numeric {
                kind,
                read: Box::new(NumericIn::new(NumericColumn::Typmod {
                    precision: *precision,
                    scale: i16::from(*scale),
                })),
            },
            CompareKind::Numeric { infinities: bare, typmod } => {
                let column = match typmod {
                    Some(typmod) => {
                        NumericColumn::Typmod { precision: typmod.precision, scale: typmod.scale }
                    }
                    None if *bare => NumericColumn::Bare,
                    None => NumericColumn::UnreadTypmod,
                };
                KeyRead::Numeric { kind, read: Box::new(NumericIn::new(column)) }
            }
            _ => return None,
        })
    }

    /// Ready the reader for another text.
    pub(crate) fn start(&mut self) {
        match self {
            KeyRead::Collected { text, .. } => text.clear(),
            KeyRead::Enum(read) => read.len = 0,
            KeyRead::Int { read, .. } => *read = IntIn::default(),
            KeyRead::Oid(read) => *read = OidIn::default(),
            KeyRead::Bool(read) => *read = BoolIn::default(),
            KeyRead::Uuid(read) => *read = UuidIn::default(),
            KeyRead::Numeric { read, .. } => read.start(),
        }
    }

    /// Read the whole of a text, this reader having read none of it.
    pub(crate) fn whole(&mut self, text: &[u8]) {
        match self {
            KeyRead::Numeric { read, .. } => read.whole_text(text),
            reader => reader.run(text),
        }
    }

    /// Read the text's next run.
    pub(crate) fn run(&mut self, run: &[u8]) {
        match self {
            KeyRead::Collected { text, .. } => text.extend_from_slice(run),
            reader => run.iter().for_each(|&byte| reader.byte(byte)),
        }
    }

    /// Read the text's next byte.
    pub(crate) fn byte(&mut self, byte: u8) {
        match self {
            KeyRead::Int { read, .. } => read.byte(byte),
            KeyRead::Oid(read) => read.byte(byte),
            KeyRead::Bool(read) => read.byte(byte),
            KeyRead::Enum(read) => read.byte(byte),
            KeyRead::Uuid(read) => read.byte(byte),
            KeyRead::Numeric { read, .. } => read.byte(byte),
            KeyRead::Collected { text, .. } => text.push(byte),
        }
    }

    /// The text has ended: its key, or why it has none. A collected text
    /// that is not UTF-8 is no value, which no `*_in` is handed.
    pub(crate) fn finish(&self) -> Read<OrderKey> {
        Ok(match self {
            KeyRead::Int { bytes, read } => OrderKey::Int(read.finish(*bytes)?),
            // Every value the column can hold widens into `i64` unchanged.
            KeyRead::Oid(read) => OrderKey::Int(read.finish()?.into()),
            KeyRead::Bool(read) => OrderKey::Bool(read.finish()?),
            KeyRead::Enum(read) => OrderKey::Int(read.finish()?),
            // Every refusal of the reader is `uuid_in`'s (I64).
            KeyRead::Uuid(read) => OrderKey::Bytes(read.finish().ok_or(Unread::Refused)?.to_vec()),
            KeyRead::Numeric { kind, read } => numeric_key(kind, read)?,
            KeyRead::Collected { kind, text } => {
                let text = std::str::from_utf8(text).map_err(|_| Unread::Refused)?;
                field_key(kind, text)?
            }
        })
    }

    /// What a whitespace run fed from here can move, for
    /// [`KeyRead::rewind`]: a machine's fixed state, a collected text's
    /// length.
    pub(crate) fn mark(&self) -> KeyMark<'k> {
        match self {
            KeyRead::Collected { text, .. } => KeyMark::Collected(text.len()),
            machine => KeyMark::Machine(machine.clone()),
        }
    }

    /// Return to `mark`, taken of this reader.
    pub(crate) fn rewind(&mut self, mark: &KeyMark<'k>) {
        match (self, mark) {
            (KeyRead::Collected { text, .. }, KeyMark::Collected(at)) => text.truncate(*at),
            (reader, KeyMark::Machine(machine)) => reader.clone_from(machine),
            _ => unreachable!("a mark is rewound into the reader it was taken of"),
        }
    }
}

/// A `numeric`'s key, read by `read` as a column compared as `kind` reads
/// one: a special value first ([`special_order_key`]), then a typmod'd
/// column's field put through its typmod, rounded to the scale and refused
/// past the precision as `COPY` refuses it (I51), and a bare one's held to
/// `numeric_in`'s bounds (I63), each spelling `numeric_in` reads and this
/// build does not unparsed. A literal is read by [`literal_key`].
fn numeric_key(kind: &CompareKind, read: &NumericIn) -> Read<OrderKey> {
    if let Some(special) = read.word().and_then(|word| special_order_key(kind, word)) {
        return Ok(special);
    }
    Ok(match kind {
        CompareKind::Decimal { .. } => {
            OrderKey::Decimal(i256::from_string(&read.unscaled()?).ok_or(Unread::Unparsed)?)
        }
        CompareKind::Numeric { typmod: Some(typmod), .. } => {
            OrderKey::Numeric(NumericKey::from_unscaled(&read.unscaled()?, typmod.scale))
        }
        CompareKind::Numeric { typmod: None, .. } => {
            let (negative, int, frac) = read.parts()?;
            OrderKey::Numeric(NumericKey::from_parts(negative, int, frac))
        }
        _ => unreachable!("only a `numeric` is read by `NumericIn`"),
    })
}

/// What [`KeyRead::mark`] saved.
#[derive(Debug)]
pub(crate) enum KeyMark<'k> {
    Machine(KeyRead<'k>),
    Collected(usize),
}

/// `NAMEDATALEN`: `enum_in` refuses a text of this many bytes or more before
/// looking it up, no label being so long (I70).
pub(crate) const ENUM_LABEL_LIMIT: usize = 64;

/// **`enum_in` fed a text a byte at a time**, holding no more of it than the
/// longest label: a label the type does not declare is no value of the
/// column, which `enum_in` refuses where the declared labels are all the type
/// holds, or the text is longer than any label can be; otherwise it may be
/// one this build did not read.
#[derive(Debug, Clone)]
pub(crate) struct EnumIn<'k> {
    labels: &'k [String],
    exact: bool,
    label: [u8; ENUM_LABEL_LIMIT],
    /// The text's length, saturating past the limit.
    len: usize,
}

impl EnumIn<'_> {
    fn byte(&mut self, byte: u8) {
        if let Some(at) = self.label.get_mut(self.len) {
            *at = byte;
        }
        self.len = self.len.saturating_add(1);
    }

    /// The label's position among the type's, which orders it. The linear
    /// scan is over a label list, a handful of entries in practice.
    // pg-refuses: I70
    fn finish(&self) -> Read<i64> {
        let text = self.label.get(..self.len);
        let position = text.and_then(|text| self.labels.iter().position(|l| l.as_bytes() == text));
        let refused = self.exact || self.len >= ENUM_LABEL_LIMIT;
        Ok(position.ok_or(if refused { Unread::Refused } else { Unread::Unparsed })? as i64)
    }
}

#[cfg(test)]
mod differential;
#[cfg(test)]
mod prior_shape;
#[cfg(test)]
mod tests;
