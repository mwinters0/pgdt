//! **A nested field ordered against a filter's bound as the output grammars'
//! walk reads it**: what [`nested_key`] over the field and
//! [`compare_nested`] against the bound answer, with no key of the field
//! built — each part keyed, compared against the bound's at its position and
//! dropped as the walk hands it over, and every part read whatever an
//! earlier one decided, so a field `nested_key` refuses is refused alike
//! (`docs/design/decisions.md`, "D151").

use std::cmp::Ordering;

use pgdt_model::PostgresInvalidValues;

use super::multirange::order_multirange;
use super::{
    NestedKey, RangeBoundKey, array_shape, compare_nested, compare_range, compare_slot, make_range,
    nested_key, null_order, position_refuses_null,
};
use crate::compare::{CompareKind, Discrete, NestedCompare};
use crate::declared::Position;
use crate::key::order::{OrderKey, Side, compare_keys, jsonb_order, read_key};
use crate::key::runs::FieldOrder;
use crate::value::nested::{FieldText, walk_array, walk_int2vector, walk_range, walk_record};

/// How a nested field, read off `text` as the query reads it under
/// `invalid`, orders against `bound`, a key of `plan`'s own; `None` where
/// the field is no value of the type, as [`nested_key`] refuses it.
///
/// A multirange's members are held as their bounds' positions, sorted and
/// merged before any is compared.
pub fn nested_field_order(
    plan: &NestedCompare,
    mut text: FieldText<'_>,
    bound: &NestedKey,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    order(plan, &mut text, Some(bound), invalid)
}

/// How the value `text` holds orders against `bound`, or `Equal` where
/// there is none, the value read all the same.
fn order(
    plan: &NestedCompare,
    text: &mut FieldText<'_>,
    bound: Option<&NestedKey>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    match plan {
        NestedCompare::Array { element, .. } => order_array(element, text, bound, invalid),
        NestedCompare::Record { fields, .. } => order_record(fields, text, bound, invalid),
        NestedCompare::Range { bound: subtype, discrete, .. } => {
            order_range(subtype, *discrete, text, bound, invalid)
        }
        NestedCompare::Int2Vector { .. } => order_int2vector(text, bound),
        NestedCompare::Multirange { bound: subtype, discrete, .. } => {
            order_multirange(subtype, *discrete, text, bound, invalid)
        }
        NestedCompare::Leaf { .. } | NestedCompare::Uncomparable { .. } => {
            let whole = text.rest()?;
            let key = nested_key(plan, &whole, Side::Field(invalid))?;
            Some(bound.map_or(Ordering::Equal, |bound| compare_nested(&key, bound)))
        }
    }
}

/// [`order`] over one part's text, a scalar position read by its key's
/// reader.
fn order_part(
    plan: &NestedCompare,
    text: &str,
    bound: Option<&NestedKey>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    match plan {
        NestedCompare::Leaf { kind, .. } => order_leaf(kind, text, bound, invalid),
        _ => order(plan, &mut FieldText::plain(text), bound, invalid),
    }
}

/// A scalar position's text against `bound`, a leaf key: a bytewise kind's
/// ordered with no key of it built ([`FieldOrder`]), a `jsonb`'s by its
/// pairs' positions ([`jsonb_order`]), any other's keyed.
fn order_leaf(
    kind: &CompareKind,
    text: &str,
    bound: Option<&NestedKey>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    let bound = bound.map(|bound| match bound {
        NestedKey::Leaf(key) => key,
        _ => unreachable!("a scalar position's bound is a leaf key"),
    });
    // A text read where nothing is compared is ordered against the empty
    // one, which allocates nothing.
    let unbounded = match kind {
        CompareKind::Bytea => OrderKey::Bytes(Vec::new()),
        _ => OrderKey::Text(String::new()),
    };
    if matches!(kind, CompareKind::Jsonb) {
        return jsonb_order(FieldText::plain(text), bound);
    }
    if let Some(order) = FieldOrder::new(kind, bound.unwrap_or(&unbounded)) {
        let order = order.of(text)?;
        return Some(if bound.is_some() { order } else { Ordering::Equal });
    }
    let key = read_key(kind, text, invalid)?;
    Some(bound.map_or(Ordering::Equal, |bound| compare_keys(&key, bound)))
}

/// One position's value, `None` being SQL NULL, against `bound`, the
/// bound's slot at the same position where one is compared: what
/// [`compare_slot`] answers, the value read whether or not one is.
fn order_slot(
    position: &Position<NestedCompare>,
    value: Option<&str>,
    bound: Option<Option<&NestedKey>>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    let order = match value {
        None if position_refuses_null(position, Side::Field(invalid)) => return None,
        None => Ordering::Equal,
        Some(text) => order_part(&position.plan, text, bound.flatten(), invalid)?,
    };
    Some(match bound {
        None => Ordering::Equal,
        Some(Some(_)) if value.is_some() => order,
        Some(slot) => null_order(value.is_some(), slot.is_some()),
    })
}

/// An array's elements, then its shape, as [`compare_nested`] orders two.
fn order_array(
    element: &Position<NestedCompare>,
    text: &mut FieldText<'_>,
    bound: Option<&NestedKey>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    let bound = bound.map(|bound| match bound {
        NestedKey::Array { elements, dims, lower_bounds } => (elements, dims, lower_bounds),
        _ => unreachable!("an array's bound is an array key"),
    });
    let mut order = Ordering::Equal;
    let mut count = 0usize;
    let shape = walk_array(text, |value| {
        let slot = bound
            .filter(|_| order.is_eq())
            .and_then(|(elements, ..)| elements.get(count))
            .map(Option::as_ref);
        let compared = slot.is_some();
        let slot_order = order_slot(element, value, slot, invalid)?;
        if compared {
            order = slot_order;
        }
        count += 1;
        Some(())
    })?;
    let Some((elements, dims, lower_bounds)) = bound else { return Some(Ordering::Equal) };
    Some(order.then_with(|| {
        array_shape((count, &shape.dims, &shape.lower_bounds), (elements.len(), dims, lower_bounds))
    }))
}

/// A record's fields in declaration order, its arity the composite's own.
fn order_record(
    fields: &[(String, Position<NestedCompare>)],
    text: &mut FieldText<'_>,
    bound: Option<&NestedKey>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    let bound = bound.map(|bound| match bound {
        NestedKey::Record(fields) => fields,
        _ => unreachable!("a record's bound is a record key"),
    });
    let mut order = Ordering::Equal;
    let mut count = 0usize;
    let read = walk_record(text, |value| {
        let Some((_, position)) = fields.get(count) else {
            // A zero-field composite is written `()`, as a one-field one
            // holding NULL is: the declared field list decides (I23).
            count += 1;
            return (fields.is_empty() && count == 1 && value.is_none()).then_some(());
        };
        let slot =
            bound.filter(|_| order.is_eq()).and_then(|bound| bound.get(count)).map(Option::as_ref);
        let compared = slot.is_some();
        let slot_order = order_slot(position, value, slot, invalid)?;
        if compared {
            order = slot_order;
        }
        count += 1;
        Some(())
    })?;
    (read == fields.len().max(1)).then_some(order)
}

/// A range in the form the server stores it in ([`make_range`]), against a
/// bound already in it.
fn order_range(
    subtype: &NestedCompare,
    discrete: Option<Discrete>,
    text: &mut FieldText<'_>,
    bound: Option<&NestedKey>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    let (mut lower, mut upper) = (None, None);
    let shape = walk_range(text, |is_lower, value| {
        let key = match value {
            Some(text) => Some(nested_key(subtype, text, Side::Field(invalid))?),
            None => None,
        };
        *(if is_lower { &mut lower } else { &mut upper }) = key;
        Some(())
    })?;
    let range = make_range(
        RangeBoundKey { value: lower, inclusive: shape.lower_inclusive, lower: true },
        RangeBoundKey { value: upper, inclusive: shape.upper_inclusive, lower: false },
        shape.empty,
        discrete,
    )?;
    Some(bound.map_or(Ordering::Equal, |bound| match bound {
        NestedKey::Range(bound) => compare_range(&range, bound),
        _ => unreachable!("a range's bound is a range key"),
    }))
}

/// An `int2vector`'s elements, then its shape: `[n]` and `[0]` for every
/// value (I47).
fn order_int2vector(text: &mut FieldText<'_>, bound: Option<&NestedKey>) -> Option<Ordering> {
    let bound = bound.map(|bound| match bound {
        NestedKey::Array { elements, dims, lower_bounds } => (elements, dims, lower_bounds),
        _ => unreachable!("an int2vector's bound is an array key"),
    });
    let mut order = Ordering::Equal;
    let mut count = 0usize;
    let read = walk_int2vector(text, |value| {
        if let Some(slot) =
            bound.filter(|_| order.is_eq()).and_then(|(elements, ..)| elements.get(count))
        {
            let key = NestedKey::Leaf(OrderKey::Int(i64::from(value)));
            order = compare_slot(Some(&key), slot.as_ref());
        }
        count += 1;
        Some(())
    })?;
    let Some((elements, dims, lower_bounds)) = bound else { return Some(Ordering::Equal) };
    Some(
        order
            .then_with(|| array_shape((read, &[read], &[0]), (elements.len(), dims, lower_bounds))),
    )
}

#[cfg(test)]
mod tests;
