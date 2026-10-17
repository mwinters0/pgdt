//! **The order a [`ValueKey`](super::ValueKey) was read in, and which two
//! orders relate**: what `ValueKey::compare` checks before it compares two
//! keys' values. The key the filter, the gatherer and the summary hold
//! carries none of it — every key they compare was read under one plan —
//! so this is the embedder's alone (`docs/design/decisions.md`, "D56").
//!
//! An order names the comparison, never the declared type: `character
//! varying(n)` and `text` share one, a domain is its base's, and `inet` and
//! `cidr` are one. A nested order names each leaf's type besides, the server
//! relating two arrays, ranges or records only where their element, bound
//! or column types are one (`array_cmp`, `range_cmp`, `record_cmp`).

use std::cmp::Ordering;
use std::mem::size_of;
use std::sync::Arc;

use super::order::{FINITE, OrderKey, order_keys, pg_float_cmp};
use crate::compare::{CompareKind, Discrete, NestedCompare};
use crate::declared::{builtin_name, split_typmod};
use crate::value::datetime::POSTGRES_EPOCH_UNIX_MICROS;
use crate::value::numeric::NumericKey;

/// A scalar column's order: its [`CompareKind`] with what moves no key
/// folded away — a text's length, an `interval`'s qualifier, a `numeric`'s
/// precision, `cidr`, whether an enum's labels are all its type holds — and
/// what tells two orders apart kept: an integer's width, a `numeric(p,s)`'s
/// scale, a MAC address's octets, an enum's type and labels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ScalarOrder {
    Bool,
    Int { bytes: u8 },
    Oid,
    Float32,
    Float64,
    Decimal { scale: i8 },
    Numeric,
    Date,
    Time,
    Timestamp { with_tz: bool },
    Uuid,
    Bytea,
    Text,
    PaddedText,
    Enum { name: Arc<str>, labels: Arc<[String]> },
    Interval,
    IntervalFields,
    TimeTz,
    Network,
    MacAddr { octets: usize },
    Jsonb,
}

impl ScalarOrder {
    pub(crate) fn of(kind: &CompareKind) -> Self {
        match kind {
            CompareKind::Bool => Self::Bool,
            CompareKind::Int { bytes } => Self::Int { bytes: *bytes },
            CompareKind::UnsignedInt => Self::Oid,
            CompareKind::Float32 => Self::Float32,
            CompareKind::Float64 => Self::Float64,
            CompareKind::Decimal { scale, .. } => Self::Decimal { scale: *scale },
            CompareKind::Numeric { .. } => Self::Numeric,
            CompareKind::Date => Self::Date,
            CompareKind::Time => Self::Time,
            CompareKind::Timestamp { with_tz } => Self::Timestamp { with_tz: *with_tz },
            CompareKind::Uuid => Self::Uuid,
            CompareKind::Bytea => Self::Bytea,
            CompareKind::Text { .. } => Self::Text,
            CompareKind::PaddedText { .. } => Self::PaddedText,
            CompareKind::Enum { name, labels, .. } => {
                Self::Enum { name: name.clone(), labels: labels.clone() }
            }
            CompareKind::Interval { .. } => Self::Interval,
            CompareKind::IntervalFields { .. } => Self::IntervalFields,
            CompareKind::TimeTz => Self::TimeTz,
            CompareKind::Network { .. } => Self::Network,
            CompareKind::MacAddr { octets } => Self::MacAddr { octets: *octets },
            CompareKind::Jsonb => Self::Jsonb,
        }
    }
}

/// A nested column's order: its tree, each leaf naming its type, typmod
/// aside, beside its order, and a range its canonical function.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NestedOrder {
    Leaf { declared: String, order: ScalarOrder },
    Array(Box<NestedOrder>),
    Record(Vec<NestedOrder>),
    Range { bound: Box<NestedOrder>, discrete: Option<Discrete> },
    Multirange { bound: Box<NestedOrder>, discrete: Option<Discrete> },
    Int2Vector,
}

impl NestedOrder {
    /// `None` at a position with no order, which no keyed tree holds
    /// ([`ComparisonPlan::orders`](crate::compare::ComparisonPlan::orders)).
    pub(crate) fn of(tree: &NestedCompare) -> Option<Self> {
        Some(match tree {
            NestedCompare::Leaf { declared, kind, .. } => {
                Self::Leaf { declared: type_name(declared), order: ScalarOrder::of(kind) }
            }
            NestedCompare::Uncomparable { .. } => return None,
            NestedCompare::Array(element) => Self::Array(Box::new(Self::of(&element.plan)?)),
            NestedCompare::Record(fields) => Self::Record(
                fields.iter().map(|(_, field)| Self::of(&field.plan)).collect::<Option<_>>()?,
            ),
            NestedCompare::Range { bound, discrete } => {
                Self::Range { bound: Box::new(Self::of(bound)?), discrete: *discrete }
            }
            NestedCompare::Multirange { bound, discrete } => {
                Self::Multirange { bound: Box::new(Self::of(bound)?), discrete: *discrete }
            }
            NestedCompare::Int2Vector => Self::Int2Vector,
        })
    }

    /// The heap this tree holds.
    pub(crate) fn heap_bytes(&self) -> u64 {
        let boxed = |inner: &Self| size_of::<Self>() as u64 + inner.heap_bytes();
        match self {
            Self::Leaf { declared, .. } => declared.capacity() as u64,
            Self::Array(inner)
            | Self::Range { bound: inner, .. }
            | Self::Multirange { bound: inner, .. } => boxed(inner),
            Self::Record(fields) => {
                (fields.capacity() * size_of::<Self>()) as u64
                    + fields.iter().map(Self::heap_bytes).sum::<u64>()
            }
            Self::Int2Vector => 0,
        }
    }
}

/// A leaf's type as the server tells two apart: the declared spelling with
/// its typmod cut out, a built-in under its one name.
fn type_name(declared: &str) -> String {
    let declared = declared.trim();
    if split_typmod(declared).0.contains('.') {
        split_typmod(declared).0.to_owned()
    } else {
        builtin_name(declared).0.into_owned()
    }
}

/// Two scalar keys in PostgreSQL's semantics, `x` read in order `a` and `y`
/// in `b`: the server's answer to `x < y` where it resolves the operator
/// without consulting a setting, `None` where it resolves none, resolves one
/// only under `TimeZone` — a `timestamp` against a `timestamptz`, a `date`
/// against a `timestamptz`, a `time` against a `timetz` — or refuses the
/// cast it chose, as `i8tooid` refuses a `bigint` past an `oid`'s range and
/// `float8in` a `numeric` past a double's.
///
/// deficiency: KD120 — `None` where the server answers, because a key's
/// order does not carry what its answer turns on. A `text` key ending in a
/// blank against a `character(n)`'s: the server casts the `character(n)` to
/// `text` and compares the blank, or casts a `character varying` to
/// `bpchar` and trims it, and [`CompareKind::Text`] holds both types. Two
/// arrays or ranges of `numeric` at two scales, whose leaves are read at
/// each one's own. And the reverse, a nested pair answered where the server
/// resolves no operator: two range types over one subtype, neither
/// discrete, which the tree does not name, and two records whose columns
/// are two composite types of one shape. **(b) owned**, by the ledger row
/// the index names: the plan naming a text's type, a range's and a nested
/// composite's, and a nested comparison relating leaves of one type as
/// scalars relate.
pub(crate) fn postgres(
    a: &ScalarOrder,
    x: &OrderKey,
    b: &ScalarOrder,
    y: &OrderKey,
) -> Option<Ordering> {
    if a == b {
        return order_keys(x, y);
    }
    directed(a, x, b, y).or_else(|| directed(b, y, a, x).map(Ordering::reverse))
}

/// [`postgres`] for a pair of two orders, matched in one direction only:
/// the caller tries both.
fn directed(a: &ScalarOrder, x: &OrderKey, b: &ScalarOrder, y: &OrderKey) -> Option<Ordering> {
    use ScalarOrder as O;
    match (a, b) {
        // `int2`, `int4` and `int8` compare through cross-type operators.
        (O::Int { .. }, O::Int { .. }) | (O::Float32, O::Float64) => order_keys(x, y),
        // Cast to `oid`: `int4`'s binary coercion wraps a negative, and
        // `i8tooid` refuses what `oid` cannot hold.
        (O::Int { bytes }, O::Oid) => {
            let (OrderKey::Int(int), OrderKey::Int(oid)) = (x, y) else { return None };
            let wrapped = match bytes {
                8 => u32::try_from(*int).ok()?,
                _ => *int as i32 as u32,
            };
            Some(i64::from(wrapped).cmp(oid))
        }
        // Cast to `double precision`, a `real` widened exactly beside it.
        (O::Int { .. }, O::Float32 | O::Float64) => {
            let (OrderKey::Int(int), OrderKey::Float(float)) = (x, y) else { return None };
            Some(pg_float_cmp(*int as f64, *float))
        }
        // `numeric_float8`: an infinity and `NaN` are the double's own.
        (O::Float32 | O::Float64, O::Decimal { .. } | O::Numeric) => {
            let OrderKey::Float(float) = x else { return None };
            Some(pg_float_cmp(*float, numeric_double(b, y)?))
        }
        // Cast to `numeric`, exactly; a special value is its rank.
        (O::Int { .. } | O::Decimal { .. } | O::Numeric, O::Decimal { .. } | O::Numeric) => {
            ranked(x, y, || Some(numeric(a, x)?.cmp(&numeric(b, y)?)))
        }
        // `date2timestamp`: midnight, with no zone to read.
        (O::Date, O::Timestamp { with_tz: false }) => {
            let (OrderKey::Int(days), OrderKey::Int(micros)) = (x, y) else {
                return ranked(x, y, || None);
            };
            Some(date_micros(*days).cmp(&i128::from(*micros)))
        }
        // `time_interval`: the time of day as a span.
        (O::Time, O::Interval) => {
            let (OrderKey::Int(micros), OrderKey::Interval(span)) = (x, y) else {
                return ranked(x, y, || None);
            };
            Some(i128::from(*micros).cmp(span))
        }
        // A `character(n)` cast to `text`, its padding given up, where the
        // key already gave it up; a `character varying` is cast the other
        // way, which agrees but on a trailing blank (KD120).
        (O::PaddedText, O::Text) => match (x, y) {
            (OrderKey::Text(padded), OrderKey::Text(text)) if !text.ends_with(' ') => {
                Some(padded.as_bytes().cmp(text.as_bytes()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// A special value on either side is its rank; two finite keys are `finite`'s.
fn ranked(
    x: &OrderKey,
    y: &OrderKey,
    finite: impl FnOnce() -> Option<Ordering>,
) -> Option<Ordering> {
    match (x.rank(), y.rank()) {
        (FINITE, FINITE) => finite(),
        (rank_x, rank_y) => Some(rank_x.cmp(&rank_y)),
    }
}

/// A finite key of the integer or `numeric` family, read in `order`, as one
/// `numeric` value whatever its scale.
fn numeric(order: &ScalarOrder, key: &OrderKey) -> Option<NumericKey> {
    match (order, key) {
        (ScalarOrder::Int { .. }, OrderKey::Int(int)) => {
            Some(NumericKey::from_unscaled(&int.to_string(), 0))
        }
        (ScalarOrder::Decimal { scale }, OrderKey::Decimal(unscaled)) => {
            Some(NumericKey::from_unscaled(&unscaled.to_string(), i16::from(*scale)))
        }
        (_, OrderKey::Numeric(numeric)) => Some(numeric.clone()),
        _ => None,
    }
}

/// A `numeric` key as `numeric_float8` casts it: `None` where `float8in`
/// refuses the value as out of a double's range.
fn numeric_double(order: &ScalarOrder, key: &OrderKey) -> Option<f64> {
    match key {
        OrderKey::NotANumber => Some(f64::NAN),
        OrderKey::PositiveInfinity => Some(f64::INFINITY),
        OrderKey::NegativeInfinity => Some(f64::NEG_INFINITY),
        _ => numeric(order, key)?.to_f64(),
    }
}

/// A `date`'s midnight in microseconds from PostgreSQL's epoch, a
/// `timestamp`'s unit.
fn date_micros(days: i64) -> i128 {
    i128::from(days) * 86_400_000_000 - POSTGRES_EPOCH_UNIX_MICROS
}
