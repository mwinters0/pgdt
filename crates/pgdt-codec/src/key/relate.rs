//! **The order a [`ValueKey`](super::ValueKey) was read in, and which two
//! orders relate**: what `ValueKey::compare` checks before it compares two
//! keys' values. The key the filter, the gatherer and the summary hold
//! carries none of it — every key they compare was read under one plan —
//! so this is the embedder's alone (`docs/design/decisions.md`, "D56").
//!
//! A scalar order names the comparison, and the declared type only where
//! the server tells two apart: a domain is its base's, `inet` and `cidr`
//! are one, and `character varying` is `text`'s but against a
//! `character(n)`. A nested order names the types beneath it besides, the
//! server relating two arrays, ranges or records only where their element,
//! range or column types are one (`array_cmp`, `range_cmp`, `record_cmp`,
//! I92), and then relating their leaves as it relates two scalars.

use std::cmp::Ordering;
use std::mem::size_of;
use std::sync::Arc;

use super::order::{FINITE, OrderKey, order_keys, pg_float_cmp};
use crate::compare::{CompareKind, ComparisonSemantics, NestedCompare};
use crate::declared::{array_element, builtin_name, split_typmod};
use crate::value::datetime::POSTGRES_EPOCH_UNIX_MICROS;
use crate::value::nested::key::{NestedKey, array_shape, compare_nested, null_order};
use crate::value::numeric::NumericKey;

/// A scalar column's order: its [`CompareKind`] with what moves no key
/// folded away — a text's length, an `interval`'s qualifier, a `numeric`'s
/// precision, `cidr`, whether an enum's labels are all its type holds — and
/// what tells two orders apart kept: an integer's width, a `numeric(p,s)`'s
/// scale, a MAC address's octets, an enum's type and labels, and in
/// PostgreSQL's semantics whether a text is `character varying`.
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
    VaryingText,
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
    /// `kind`'s order in `semantics`: DataFusion's tells no text type from
    /// another, emitting each as one.
    pub(crate) fn of(kind: &CompareKind, semantics: ComparisonSemantics) -> Self {
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
            CompareKind::Text { varying: true, .. }
                if semantics == ComparisonSemantics::Postgres =>
            {
                Self::VaryingText
            }
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

/// A nested column's order: its tree, each node named where the server
/// tells two apart by name (`docs/design/decisions.md`, "D56").
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NestedOrder {
    /// A leaf's type, typmod aside, beside its order.
    Leaf { name: String, order: ScalarOrder },
    /// `name` is the domain an element or field is declared as; `None` for
    /// an array spelled as one, which its element names.
    Array { name: Option<String>, element: Box<NestedOrder> },
    /// `name` is the composite's below the top; `None` at the top, where
    /// `record_cmp` relates two composite types whose column types are one.
    Record { name: Option<String>, fields: Vec<NestedOrder> },
    /// A range is named at every level, two range types over one subtype
    /// relating by no operator; the name implies the bound.
    Range { name: String, bound: Box<NestedOrder> },
    /// A multirange, named as its range is.
    Multirange { name: String, bound: Box<NestedOrder> },
    /// `name` tells a domain over `int2vector` from the type.
    Int2Vector { name: String },
}

impl NestedOrder {
    /// `None` at a position with no order, which no keyed tree holds
    /// ([`ComparisonPlan::orders`](crate::compare::ComparisonPlan::orders)).
    pub(crate) fn of(tree: &NestedCompare) -> Option<Self> {
        Self::at(tree, true)
    }

    /// [`Self::of`] at one node, `top` where it is the column's own.
    fn at(tree: &NestedCompare, top: bool) -> Option<Self> {
        Some(match tree {
            NestedCompare::Leaf { declared, kind, .. } => {
                let order = ScalarOrder::of(kind, ComparisonSemantics::Postgres);
                Self::Leaf { name: type_name(declared), order }
            }
            NestedCompare::Uncomparable { .. } => return None,
            NestedCompare::Array { declared, element } => Self::Array {
                name: array_element(declared).is_none().then(|| type_name(declared)),
                element: Box::new(Self::at(&element.plan, false)?),
            },
            NestedCompare::Record { declared, fields } => Self::Record {
                name: (!top).then(|| type_name(declared)),
                fields: fields
                    .iter()
                    .map(|(_, field)| Self::at(&field.plan, false))
                    .collect::<Option<_>>()?,
            },
            NestedCompare::Range { declared, bound, .. } => {
                Self::Range { name: type_name(declared), bound: Box::new(Self::at(bound, false)?) }
            }
            NestedCompare::Multirange { declared, bound, .. } => Self::Multirange {
                name: type_name(declared),
                bound: Box::new(Self::at(bound, false)?),
            },
            NestedCompare::Int2Vector { declared } => {
                Self::Int2Vector { name: type_name(declared) }
            }
        })
    }

    /// Whether the server resolves `a < b` between values of the two
    /// orders: one shape, named alike at every node, and leaves of one type
    /// whatever their typmods. A range is related to itself alone.
    fn relates(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Leaf { name: a, .. }, Self::Leaf { name: b, .. }) => a == b,
            (Self::Array { name: n, element: a }, Self::Array { name: m, element: b }) => {
                n == m && a.relates(b)
            }
            (Self::Record { name: n, fields: a }, Self::Record { name: m, fields: b }) => {
                n == m && a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.relates(b))
            }
            _ => self == other,
        }
    }

    /// The heap this tree holds.
    pub(crate) fn heap_bytes(&self) -> u64 {
        let boxed = |inner: &Self| size_of::<Self>() as u64 + inner.heap_bytes();
        let named = |name: &Option<String>| name.as_ref().map_or(0, |n| n.capacity() as u64);
        match self {
            Self::Leaf { name, .. } | Self::Int2Vector { name } => name.capacity() as u64,
            Self::Range { name, bound } | Self::Multirange { name, bound } => {
                name.capacity() as u64 + boxed(bound)
            }
            Self::Array { name, element } => named(name) + boxed(element),
            Self::Record { name, fields } => {
                named(name)
                    + (fields.capacity() * size_of::<Self>()) as u64
                    + fields.iter().map(Self::heap_bytes).sum::<u64>()
            }
        }
    }
}

/// Two nested keys in PostgreSQL's semantics, `x` read in order `a` and `y`
/// in `b`: the server's answer to `x < y` where its orders relate
/// ([`NestedOrder::relates`]), each pair of leaves related as [`postgres`]
/// relates two scalars, and `None` where they do not.
pub(crate) fn nested(
    a: &NestedOrder,
    x: &NestedKey,
    b: &NestedOrder,
    y: &NestedKey,
) -> Option<Ordering> {
    if a == b {
        return Some(compare_nested(x, y));
    }
    a.relates(b).then(|| related(a, x, b, y))?
}

/// [`nested`] over two orders that relate, `array_cmp`'s and `record_cmp`'s
/// walk with each leaf pair related by its own two orders: `None` only
/// where [`postgres`] answers none for a leaf pair it reaches.
fn related(a: &NestedOrder, x: &NestedKey, b: &NestedOrder, y: &NestedKey) -> Option<Ordering> {
    use NestedOrder as O;
    if a == b {
        return Some(compare_nested(x, y));
    }
    match (a, x, b, y) {
        (
            O::Leaf { order: a, .. },
            NestedKey::Leaf(x),
            O::Leaf { order: b, .. },
            NestedKey::Leaf(y),
        ) => postgres(a, x, b, y),
        (
            O::Array { element: a, .. },
            NestedKey::Array { elements: xe, dims: xd, lower_bounds: xl },
            O::Array { element: b, .. },
            NestedKey::Array { elements: ye, dims: yd, lower_bounds: yl },
        ) => {
            let order = slots(xe.iter().zip(ye).map(|(x, y)| (&**a, x, &**b, y)))?;
            Some(order.then_with(|| array_shape((xe.len(), xd, xl), (ye.len(), yd, yl))))
        }
        (
            O::Record { fields: a, .. },
            NestedKey::Record(xs),
            O::Record { fields: b, .. },
            NestedKey::Record(ys),
        ) => slots(a.iter().zip(xs).zip(b.iter().zip(ys)).map(|((a, x), (b, y))| (a, x, b, y))),
        _ => None,
    }
}

/// Slot pairs of two containers, each `(a, x, b, y)` as [`related`] takes
/// them, under I45's NULL rule: the first that differs decides.
fn slots<'k>(
    pairs: impl Iterator<
        Item = (&'k NestedOrder, &'k Option<NestedKey>, &'k NestedOrder, &'k Option<NestedKey>),
    >,
) -> Option<Ordering> {
    for (a, x, b, y) in pairs {
        let order = match (x, y) {
            (Some(x), Some(y)) => related(a, x, b, y)?,
            _ => null_order(x.is_some(), y.is_some()),
        };
        if order.is_ne() {
            return Some(order);
        }
    }
    Some(Ordering::Equal)
}

/// A node's type as the server tells two apart: the declared spelling with
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
        // `text` operators, `character varying` binary-coerced to `text`.
        (O::Text, O::VaryingText) => order_keys(x, y),
        // Against `text` or `name`, a `character(n)` is cast to `text`,
        // `rtrim1` giving up its padding as the key already did, and the
        // other side's trailing blanks count; a `character varying` is
        // binary-coerced to `bpchar`, whose comparison gives up both sides'
        // (I92).
        (O::PaddedText, O::Text | O::VaryingText) => {
            let (OrderKey::Text(padded), OrderKey::Text(text)) = (x, y) else { return None };
            let text = match b {
                O::VaryingText => text.trim_end_matches(' '),
                _ => text,
            };
            Some(padded.as_bytes().cmp(text.as_bytes()))
        }
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
