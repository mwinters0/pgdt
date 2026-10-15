//! The order key of a nested value — an array, a record, a range, a
//! multirange — read from its literal, and the range algebra `range_in`
//! and the comparison operators apply to it (I46).

use std::cmp::Ordering;
use std::mem::size_of;

use pgdt_model::PostgresInvalidValues;

use crate::compare::discrete::DiscreteExt;
use crate::compare::{ComparisonSemantics, Discrete, NestedCompare};
use crate::declared::Position;
use crate::key::order::{OrderKey, Side, compare_keys, literal_key, order_keys, read_key};
use crate::value::{first_difference, nested};

/// One side of a **nested** comparison, decoded from a container literal per
/// the column's [`NestedCompare`]. Both sides of any one comparison come from
/// the same plan, so a variant mismatch is unreachable by construction.
///
/// `None` in an element or field position is SQL NULL, a value of the
/// container rather than the absence of one: `{1,NULL}` is a two-element
/// array. The whole field being NULL is a different fact — the `\N` the row
/// carries — and is SQL's `Unknown` as it is for a scalar column.
///
/// Not the codec's API: an embedder orders a nested value through
/// `crate::key`'s `ValueKey`, which hides this representation.
#[derive(Debug, Clone, PartialEq)]
pub enum NestedKey {
    /// A scalar position, through the leaf type's own [`OrderKey`].
    Leaf(OrderKey),
    /// An `array_out` value: its elements flattened row-major, plus the shape
    /// they were written in, because `array_cmp` decides on the shape once
    /// the elements agree (I45).
    Array { elements: Vec<Option<NestedKey>>, dims: Vec<usize>, lower_bounds: Vec<i32> },
    /// A `record_out` value: one entry per declared field, in declaration
    /// order.
    Record(Vec<Option<NestedKey>>),
    /// A range value, already through [`make_range`] — the form the server
    /// would have stored, not the form it was written in. Boxed because a
    /// bound is itself a `NestedKey`; the vector the multirange holds is
    /// indirection enough on its own.
    Range(Box<RangeKey>),
    /// A multirange value, already sorted, coalesced and emptied out
    /// ([`canonical_multirange`]), so its members are never empty and never
    /// touch.
    Multirange(Vec<RangeKey>),
}

/// A range value in the form PostgreSQL itself stores, which is the only form
/// two ranges may be compared in: `range_in` runs every literal through
/// `make_range`, so `int4range '[1,10]'` and `int4range '(0,11)'` are one
/// value the file can only hold as `[1,11)` (I46).
#[derive(Debug, Clone, PartialEq)]
pub struct RangeKey {
    /// The empty range, which `range_cmp` sorts below every other value and
    /// which is *not* the same as a range with two absent bounds — `empty`
    /// and `(,)` are different values (I46).
    empty: bool,
    lower: RangeBoundKey,
    upper: RangeBoundKey,
}

/// One bound of a range: PostgreSQL's `RangeBound`, minus the type it is a
/// bound of.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RangeBoundKey {
    /// `None` is an **unbounded** bound — the server's `infinite` flag, and
    /// not a value at all. A bound holding `infinity` is a different thing and
    /// lives in the [`OrderKey`] beneath: `daterange '[2020-01-01,infinity]'`
    /// has a finite upper bound, which is why `daterange_canonical` leaves it
    /// alone (I34, I46).
    pub(crate) value: Option<NestedKey>,
    pub(crate) inclusive: bool,
    /// Which end this bound is. It decides the answer whenever two bounds
    /// hold the same value, so it travels with the bound rather than being
    /// inferred from the caller — `bounds_adjacent` relabels a pair before
    /// comparing it, as the server does.
    pub(crate) lower: bool,
}

impl NestedKey {
    /// The heap this key holds, as the sizes the allocator was asked for.
    pub(crate) fn heap_bytes(&self) -> u64 {
        let slots = |slots: &Vec<Option<NestedKey>>| {
            (slots.capacity() * size_of::<Option<NestedKey>>()) as u64
                + slots.iter().flatten().map(NestedKey::heap_bytes).sum::<u64>()
        };
        match self {
            Self::Leaf(key) => key.heap_bytes(),
            Self::Array { elements, dims, lower_bounds } => {
                slots(elements)
                    + (dims.capacity() * size_of::<usize>()) as u64
                    + (lower_bounds.capacity() * size_of::<i32>()) as u64
            }
            Self::Record(fields) => slots(fields),
            Self::Range(range) => size_of::<RangeKey>() as u64 + range.heap_bytes(),
            Self::Multirange(members) => {
                (members.capacity() * size_of::<RangeKey>()) as u64
                    + members.iter().map(RangeKey::heap_bytes).sum::<u64>()
            }
        }
    }
}

impl RangeKey {
    /// The heap this range's bounds hold.
    fn heap_bytes(&self) -> u64 {
        [&self.lower, &self.upper]
            .into_iter()
            .filter_map(|bound| bound.value.as_ref())
            .map(NestedKey::heap_bytes)
            .sum()
    }

    /// The empty range. Its bounds are never read, and are the shape
    /// `range_serialize` leaves behind: absent and exclusive.
    fn empty() -> Self {
        RangeKey {
            empty: true,
            lower: RangeBoundKey { value: None, inclusive: false, lower: true },
            upper: RangeBoundKey { value: None, inclusive: false, lower: false },
        }
    }
}

/// Read one side of a nested comparison out of `text`.
///
/// `side` is which grammar to read it in, and it is the whole of what
/// separates the two sides: a *field* is read with [`crate::nested`]'s strict
/// `decode_*`, a *literal* with the `array_in`/`record_in` supersets
/// (`parse_*`), which take `{a, b}` and `{ 1 , 2 }` (I44).
///
/// The leaf grammar does not widen with it (`docs/design/decisions.md`,
/// "D58"): a leaf is read by its type's own reader, which takes no space its
/// `*_out` form does not write, so `--filter 'p=( 1 , a )'` is refused.
///
/// `None` is "not a value of this type", which is
/// `Error::PredicateValueDecode` for a literal and `Error::FieldDecode` for a
/// field.
pub fn nested_key(plan: &NestedCompare, text: &str, side: Side) -> Option<NestedKey> {
    let input = side == Side::Literal;
    Some(match plan {
        // A nested comparison is PostgreSQL's alone, DataFusion's semantics
        // refusing one.
        NestedCompare::Leaf { kind, .. } => NestedKey::Leaf(match side {
            Side::Literal => literal_key(kind, text, ComparisonSemantics::Postgres)?,
            Side::Field(invalid) => read_key(kind, text, invalid)?,
        }),
        // Refused before any value is read: `resolve_term` never builds a
        // comparison over a tree holding one.
        NestedCompare::Uncomparable { .. } => return None,
        NestedCompare::Array(element) => {
            let literal =
                if input { nested::parse_array(text) } else { nested::decode_array(text) }?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            for value in &literal.elements {
                elements.push(match value {
                    Some(value) => Some(nested_key(&element.plan, value, side)?),
                    None if position_refuses_null(element, side) => return None,
                    None => None,
                });
            }
            NestedKey::Array { elements, dims: literal.dims, lower_bounds: literal.lower_bounds }
        }
        // `int2vector`'s own grammar on both sides; the elements are built
        // here rather than through `order_key`, `int2vectorout` writing an
        // `int16` and nothing else.
        //
        // `dims` and `lower_bounds` are `[n]` and `[0]` for every value, the
        // empty vector included: `int2vectorin` sets `ndim = 1` and
        // `lbound1 = 0` unconditionally, where `array_out`'s `{}` is
        // zero-dimensional, so an empty `int2vector` is not the empty array
        // (I47).
        NestedCompare::Int2Vector => {
            let values = if input {
                nested::parse_int2vector(text)
            } else {
                nested::decode_int2vector(text)
            }?;
            NestedKey::Array {
                elements: values
                    .iter()
                    .map(|v| Some(NestedKey::Leaf(OrderKey::Int(i64::from(*v)))))
                    .collect(),
                dims: vec![values.len()],
                lower_bounds: vec![0],
            }
        }
        NestedCompare::Record(plans) => {
            let mut fields = if input {
                nested::parse_record(text, plans.len())?.fields
            } else {
                let mut fields = nested::decode_record(text)?.fields;
                // A zero-field composite is written `()`, and so is a
                // one-field composite holding NULL — the literal cannot tell
                // them apart, so the declared field list decides (I23).
                // `parse_record` has the arity in hand; `decode_record` does
                // not, so it is asked here.
                if plans.is_empty() && fields == [None] {
                    fields.clear();
                }
                fields
            };
            if fields.len() != plans.len() {
                return None;
            }
            let mut out = Vec::with_capacity(plans.len());
            for ((_, field), value) in plans.iter().zip(fields.drain(..)) {
                out.push(match value {
                    Some(value) => Some(nested_key(&field.plan, &value, side)?),
                    None if position_refuses_null(field, side) => return None,
                    None => None,
                });
            }
            NestedKey::Record(out)
        }
        NestedCompare::Range { bound, discrete } => {
            let literal =
                if input { nested::parse_range(text) } else { nested::decode_range(text) }?;
            NestedKey::Range(Box::new(range_key(bound, &literal, *discrete, side)?))
        }
        NestedCompare::Multirange { bound, discrete } => {
            let literals = if input {
                nested::parse_multirange(text)
            } else {
                nested::decode_multirange(text)
            }?;
            let mut members = Vec::with_capacity(literals.len());
            for literal in &literals {
                members.push(range_key(bound, literal, *discrete, side)?);
            }
            NestedKey::Multirange(canonical_multirange(members, *discrete)?)
        }
    })
}

/// Whether [`nested_key`] refuses a NULL at `position` read on `side`: a
/// literal's always and a field's but under
/// [`PostgresInvalidValues::Ignore`], where the position's type is a `NOT
/// NULL` domain, as `array_in` and `record_in` refuse it (I76).
fn position_refuses_null(position: &Position<NestedCompare>, side: Side) -> bool {
    position.not_null && side != Side::Field(PostgresInvalidValues::Ignore)
}

/// One range value, read through its bound's plan and then put into the form
/// the server stores it in.
///
/// Both sides go through [`make_range`], not only the literal
/// (`docs/design/decisions.md`, "D58"): it is idempotent on a `range_out`
/// field, the server having already applied it, so one code path serves both
/// grammars.
fn range_key(
    bound: &NestedCompare,
    literal: &nested::RangeLiteral,
    discrete: Option<Discrete>,
    reading: Side,
) -> Option<RangeKey> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        Some(RangeBoundKey {
            value: match text {
                Some(text) => Some(nested_key(bound, text, reading)?),
                None => None,
            },
            inclusive,
            lower,
        })
    };
    make_range(
        side(&literal.lower, literal.lower_inclusive, true)?,
        side(&literal.upper, literal.upper_inclusive, false)?,
        literal.empty,
        discrete,
    )
}

/// `make_range`: `range_serialize`'s type-independent checks, then the range
/// type's canonical function where it has one, then those checks again — the
/// order the server applies them in, and the reason `int4range '(1,2)'` is
/// `empty` (I46).
///
/// `None` is the server's `22000` — a lower bound above its upper, a *semantic*
/// refusal the container grammar cannot see (I44), so it is raised here where
/// the bounds have been decoded — or a discrete bound whose successor leaves
/// the subtype, past [`Discrete::largest`], the canonical function's
/// "out of range".
pub(crate) fn make_range(
    lower: RangeBoundKey,
    upper: RangeBoundKey,
    empty: bool,
    discrete: Option<Discrete>,
) -> Option<RangeKey> {
    let serialized = serialize_range(lower, upper, empty)?;
    let Some(discrete) = discrete.filter(|_| !serialized.empty) else {
        return Some(serialized);
    };
    // `int4range_canonical` and its two siblings, which differ only in the
    // value past which they refuse a successor: an exclusive lower bound
    // becomes inclusive at the successor, an inclusive upper becomes
    // exclusive at the successor.
    //
    // The `Int` pattern is `daterange_canonical`'s `DATE_NOT_FINITE` guard: a
    // date `infinity` decodes to `OrderKey::PositiveInfinity` rather than to
    // a day count, so it matches no arm here and is left as written, which is
    // what makes `[2020-01-01,infinity]` keep its inclusive upper (I34, I46).
    // pg-refuses: I46 — a successor past the subtype's largest value.
    let successor = |bound: &RangeBoundKey| match &bound.value {
        Some(NestedKey::Leaf(OrderKey::Int(n))) => (*n < discrete.largest()).then(|| {
            Some(RangeBoundKey {
                value: Some(NestedKey::Leaf(OrderKey::Int(n + 1))),
                inclusive: !bound.inclusive,
                lower: bound.lower,
            })
        }),
        // Not a finite integer bound, so the canonical function skips it.
        _ => Some(None),
    };
    let mut lower = serialized.lower;
    let mut upper = serialized.upper;
    if !lower.inclusive
        && let Some(shifted) = successor(&lower)?
    {
        lower = shifted;
    }
    if upper.inclusive
        && let Some(shifted) = successor(&upper)?
    {
        upper = shifted;
    }
    serialize_range(lower, upper, false)
}

/// `range_serialize`'s type-independent half: the out-of-order refusal, the
/// collapse to `empty`, and "an infinite boundary is never inclusive".
fn serialize_range(
    mut lower: RangeBoundKey,
    mut upper: RangeBoundKey,
    empty: bool,
) -> Option<RangeKey> {
    if empty {
        return Some(RangeKey::empty());
    }
    match compare_bound_values(&lower, &upper) {
        // pg-refuses: I46 — `22000`, a lower bound above its upper.
        Ordering::Greater => return None,
        // Equal bounds are a value only when both ends include it: `[1,1]` is
        // one point and `[1,1)`, `(1,1]` and `(1,1)` are all `empty`. This
        // runs before canonicalization *and* after it, which is what makes
        // `int4range '(1,2)'` empty.
        Ordering::Equal if !(lower.inclusive && upper.inclusive) => {
            return Some(RangeKey::empty());
        }
        _ => {}
    }
    if lower.value.is_none() {
        lower.inclusive = false;
    }
    if upper.value.is_none() {
        upper.inclusive = false;
    }
    Some(RangeKey { empty: false, lower, upper })
}

/// `range_cmp_bound_values`: the bounds' held values alone, with an absent
/// bound settled by which end it is. Inclusivity is not consulted, which is
/// what separates this from [`compare_bounds`] — the emptiness and adjacency
/// tests both need the values without it.
fn compare_bound_values(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    match (&a.value, &b.value) {
        (None, None) if a.lower == b.lower => Ordering::Equal,
        (None, None) | (None, Some(_)) => {
            if a.lower {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (Some(_), None) => {
            if b.lower {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `range_cmp_bounds`: infinity, then the held value, then inclusivity — and
/// inclusivity is where which *end* a bound is starts to matter. An exclusive
/// **lower** bound is above an inclusive one at the same value ("just
/// after"), an exclusive **upper** below it ("just before") (I46).
fn compare_bounds(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    let by_value = compare_bound_values(a, b);
    if a.value.is_none() || b.value.is_none() || by_value.is_ne() {
        return by_value;
    }
    match (a.inclusive, b.inclusive) {
        (true, true) => Ordering::Equal,
        (false, false) if a.lower == b.lower => Ordering::Equal,
        (false, _) if a.lower => Ordering::Greater,
        (false, _) => Ordering::Less,
        (true, false) if b.lower => Ordering::Less,
        (true, false) => Ordering::Greater,
    }
}

/// `range_cmp`: `empty` below everything, then lower bound, then upper.
fn compare_range(a: &RangeKey, b: &RangeKey) -> Ordering {
    match (a.empty, b.empty) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => {
            compare_bounds(&a.lower, &b.lower).then_with(|| compare_bounds(&a.upper, &b.upper))
        }
    }
}

/// `multirange_canonicalize`: sort the members, drop the empty ones, and
/// merge any two that overlap or touch. After it no member is empty and no
/// two members meet, so the sequence is the value and the comparison is a
/// plain sequence walk (I46).
///
/// `None` propagates a bound the union could not re-serialize, which the
/// shapes reaching here cannot produce; it is carried rather than unwrapped.
pub(crate) fn canonical_multirange(
    mut members: Vec<RangeKey>,
    discrete: Option<Discrete>,
) -> Option<Vec<RangeKey>> {
    members.sort_by(compare_range);
    let mut out: Vec<RangeKey> = Vec::with_capacity(members.len());
    for current in members {
        if current.empty {
            continue;
        }
        let Some(last) = out.last() else {
            out.push(current);
            continue;
        };
        // The server's own order. The first test needs the sort:
        // `range_adjacent_internal` answers true for "either meets the
        // other", and only sorting rules out the second direction.
        if ranges_adjacent(last, &current, discrete) {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        } else if range_before(last, &current) {
            out.push(current);
        } else {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        }
    }
    Some(out)
}

/// `range_before_internal`: every point of `a` is below every point of `b`,
/// with an empty range neither before nor after anything.
fn range_before(a: &RangeKey, b: &RangeKey) -> bool {
    !a.empty && !b.empty && compare_bounds(&a.upper, &b.lower).is_lt()
}

/// `range_adjacent_internal`: the two ranges touch without overlapping,
/// in either direction.
fn ranges_adjacent(a: &RangeKey, b: &RangeKey, discrete: Option<Discrete>) -> bool {
    !a.empty
        && !b.empty
        && (bounds_adjacent(&a.upper, &b.lower, discrete)
            || bounds_adjacent(&b.upper, &a.lower, discrete))
}

/// `bounds_adjacent`: whether an upper bound and a lower bound meet with no
/// point between them.
///
/// Equal values are adjacent exactly when one end includes the point and the
/// other does not. Values that differ are adjacent only in a discrete range,
/// which the server decides by building the range *between* them with both
/// inclusivities flipped and asking whether it came out empty.
fn bounds_adjacent(
    upper: &RangeBoundKey,
    lower: &RangeBoundKey,
    discrete: Option<Discrete>,
) -> bool {
    match compare_bound_values(upper, lower) {
        Ordering::Equal => upper.inclusive != lower.inclusive,
        Ordering::Greater => false,
        Ordering::Less => {
            discrete.is_some()
                && make_range(
                    RangeBoundKey {
                        value: upper.value.clone(),
                        inclusive: !upper.inclusive,
                        lower: true,
                    },
                    RangeBoundKey {
                        value: lower.value.clone(),
                        inclusive: !lower.inclusive,
                        lower: false,
                    },
                    false,
                    discrete,
                )
                .is_some_and(|between| between.empty)
        }
    }
}

/// `range_union_internal` for two ranges already known to overlap or touch:
/// the lower of the two lower bounds, the upper of the two uppers, back
/// through [`make_range`].
fn range_union(a: &RangeKey, b: &RangeKey, discrete: Option<Discrete>) -> Option<RangeKey> {
    let lower =
        if compare_bounds(&a.lower, &b.lower).is_lt() { a.lower.clone() } else { b.lower.clone() };
    let upper =
        if compare_bounds(&a.upper, &b.upper).is_gt() { a.upper.clone() } else { b.upper.clone() };
    make_range(lower, upper, false, discrete)
}

/// One NULL rule, at every level: two NULLs are equal, and NULL sorts above
/// not-NULL (I45). It covers equality and ordering alike, which is what makes
/// a nested comparison two-valued throughout, never SQL's `Unknown`.
fn compare_slot(a: Option<&NestedKey>, b: Option<&NestedKey>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `array_cmp`/`record_cmp`, structurally.
///
/// An array compares its elements first and its shape only afterwards (I45):
/// up to the shorter array's length, then element count, then dimension
/// count, then the dimensions, then the lower bounds — not the order
/// `array_eq` uses. So `{1,2}` is above `[0:1]={1,2}` and below
/// `{{1,2},{3,4}}`, whose extra elements are never reached.
///
/// A record's two sides always have the same arity: [`nested_key`] checks it
/// against the composite's own declared field list before building either.
pub fn compare_nested(a: &NestedKey, b: &NestedKey) -> Ordering {
    match (a, b) {
        (NestedKey::Leaf(a), NestedKey::Leaf(b)) => compare_keys(a, b),
        (
            NestedKey::Array { elements: ae, dims: ad, lower_bounds: al },
            NestedKey::Array { elements: be, dims: bd, lower_bounds: bl },
        ) => first_difference(
            ae.iter()
                .zip(be.iter())
                .map(|(a, b)| compare_slot(a.as_ref(), b.as_ref()))
                .chain(std::iter::once(ae.len().cmp(&be.len())))
                .chain(std::iter::once(ad.len().cmp(&bd.len())))
                .chain(ad.iter().zip(bd.iter()).map(|(a, b)| a.cmp(b)))
                .chain(al.iter().zip(bl.iter()).map(|(a, b)| a.cmp(b))),
        ),
        (NestedKey::Record(a), NestedKey::Record(b)) => first_difference(
            a.iter().zip(b.iter()).map(|(a, b)| compare_slot(a.as_ref(), b.as_ref())),
        ),
        (NestedKey::Range(a), NestedKey::Range(b)) => compare_range(a, b),
        // `multirange_cmp`: member-wise, and the shorter one first where the
        // members it has all agree — the server treating a missing member as
        // an empty range and `empty` as the lowest value there is.
        (NestedKey::Multirange(a), NestedKey::Multirange(b)) => first_difference(
            a.iter()
                .zip(b.iter())
                .map(|(a, b)| compare_range(a, b))
                .chain(std::iter::once(a.len().cmp(&b.len()))),
        ),
        _ => unreachable!("both sides of a comparison are read through one column's plan"),
    }
}

/// Whether [`compare_nested`] orders `a` against `b`: one shape at every
/// pair of positions it can reach, and one arity for two records. Two keys
/// read through one plan always are; two read through different plans may
/// not be, and comparing them would reach an unreachable arm.
pub(crate) fn comparable(a: &NestedKey, b: &NestedKey) -> bool {
    let slots = |a: &[Option<NestedKey>], b: &[Option<NestedKey>]| {
        a.iter().zip(b).all(|pair| match pair {
            (Some(a), Some(b)) => comparable(a, b),
            _ => true,
        })
    };
    match (a, b) {
        (NestedKey::Leaf(a), NestedKey::Leaf(b)) => order_keys(a, b).is_some(),
        (NestedKey::Array { elements: a, .. }, NestedKey::Array { elements: b, .. }) => slots(a, b),
        (NestedKey::Record(a), NestedKey::Record(b)) => a.len() == b.len() && slots(a, b),
        (NestedKey::Range(a), NestedKey::Range(b)) => ranges_comparable(a, b),
        (NestedKey::Multirange(a), NestedKey::Multirange(b)) => {
            a.iter().zip(b).all(|(a, b)| ranges_comparable(a, b))
        }
        _ => false,
    }
}

/// [`comparable`] for two ranges: [`compare_range`] reads a lower bound only
/// against a lower, and an upper only against an upper.
fn ranges_comparable(a: &RangeKey, b: &RangeKey) -> bool {
    let bounds = |a: &RangeBoundKey, b: &RangeBoundKey| match (&a.value, &b.value) {
        (Some(a), Some(b)) => comparable(a, b),
        _ => true,
    };
    bounds(&a.lower, &b.lower) && bounds(&a.upper, &b.upper)
}
