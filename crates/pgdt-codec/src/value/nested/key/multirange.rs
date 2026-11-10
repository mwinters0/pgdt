//! **A multirange field ordered as its members' bounds hold it**: each
//! member held as where in the field its bounds are written, sorted and
//! merged as `multirange_canonicalize` sorts and merges the ranges, a
//! member's range keyed from its text each time it is compared and dropped,
//! so its order holds two positions a member and no key of the field
//! (`docs/design/decisions.md`, "D152").

use std::cmp::Ordering;
use std::mem::size_of;

use pgdt_model::PostgresInvalidValues;

use super::{
    NestedKey, RangeBoundKey, RangeKey, compare_bounds, compare_range, make_range, nested_key,
    range_before, ranges_adjacent,
};
use crate::compare::{Discrete, NestedCompare};
use crate::key::order::Side;
use crate::value::factor::KeyFactor;
use crate::value::first_difference;
use crate::value::nested::{FieldText, walk_bracketed, walk_multirange};

/// What a multirange's order holds per byte of its field's text, at most:
/// two positions a member, the shortest member and its comma being `(,),`.
pub(crate) const MULTIRANGE_KEY_FACTOR: KeyFactor =
    KeyFactor { held: 2 * size_of::<u32>() as u64, per: "(,),".len() as u64 };

/// A member as the order holds it: where the member its lower bound is read
/// from opens, and where the one its upper bound is read from, the two one
/// member until a merge joins two.
#[derive(Debug, Clone, Copy)]
struct Member {
    lower: u32,
    upper: u32,
}

/// What reads a member's range again from where it opens.
struct Members<'p, 'a> {
    field: FieldText<'a>,
    bound: &'p NestedCompare,
    discrete: Option<Discrete>,
    invalid: PostgresInvalidValues,
}

impl Members<'_, '_> {
    /// The range the member opening at `text` holds, its bounds keyed and
    /// put into the form the server stores it in, or `None` where the field's
    /// key refuses it.
    fn read(&self, text: &mut FieldText<'_>) -> Option<RangeKey> {
        let (mut lower, mut upper) = (None, None);
        let shape = walk_bracketed(text, |is_lower, value| {
            let key = match value {
                Some(text) => Some(nested_key(self.bound, text, Side::Field(self.invalid))?),
                None => None,
            };
            *(if is_lower { &mut lower } else { &mut upper }) = key;
            Some(())
        })?;
        make_range(
            RangeBoundKey { value: lower, inclusive: shape.lower_inclusive, lower: true },
            RangeBoundKey { value: upper, inclusive: shape.upper_inclusive, lower: false },
            false,
            self.discrete,
        )
    }

    /// The range of the member opening at `offset`.
    fn at(&self, offset: u32) -> RangeKey {
        let mut text = self.field.clone();
        text.seek(offset as usize);
        self.read(&mut text).expect("a member read once reads again")
    }

    /// `member`'s range: its lower bound's member's lower bound and its upper
    /// bound's member's upper, as `range_union_internal` joins two.
    fn range(&self, member: Member) -> RangeKey {
        let lower = self.at(member.lower);
        if member.upper == member.lower {
            return lower;
        }
        let upper = self.at(member.upper);
        make_range(lower.lower, upper.upper, false, self.discrete)
            .expect("two bounds a merge joined make a range")
    }
}

/// A multirange's field against `bound`, a multirange key of the same plan,
/// what [`nested_key`] over it and [`compare_nested`](super::compare_nested)
/// answer, or `Equal` where there is no bound, the field read all the same.
pub(super) fn order_multirange(
    plan: &NestedCompare,
    discrete: Option<Discrete>,
    text: &mut FieldText<'_>,
    bound: Option<&NestedKey>,
    invalid: PostgresInvalidValues,
) -> Option<Ordering> {
    let members = Members { field: text.clone(), bound: plan, discrete, invalid };
    // Every member read as the field's key reads it, before any is held.
    let mut count = 0usize;
    walk_multirange(text, |text| {
        count += usize::from(!members.read(text)?.empty);
        Some(())
    })?;
    let Some(bound) = bound else { return Some(Ordering::Equal) };
    let bound = match bound {
        NestedKey::Multirange(bound) => bound,
        _ => unreachable!("a multirange's bound is a multirange key"),
    };
    let mut held = Vec::with_capacity(count);
    let mut text = members.field.clone();
    walk_multirange(&mut text, |text| {
        let offset = u32::try_from(text.offset()).ok()?;
        if !members.read(text)?.empty {
            held.push(Member { lower: offset, upper: offset });
        }
        Some(())
    })?;
    held.sort_unstable_by(|&a, &b| compare_range(&members.range(a), &members.range(b)));
    let held = canonical(&members, held);
    Some(first_difference(
        held.iter()
            .zip(bound)
            .map(|(&member, bound)| compare_range(&members.range(member), bound))
            .chain(std::iter::once(held.len().cmp(&bound.len()))),
    ))
}

/// `multirange_canonicalize` over sorted members, none empty, in place:
/// [`canonical_multirange`](super::canonical_multirange)'s merge, a union
/// held as the positions of the bounds it keeps.
fn canonical(members: &Members<'_, '_>, mut held: Vec<Member>) -> Vec<Member> {
    let mut kept = 0usize;
    for at in 0..held.len() {
        let current = held[at];
        let Some(last) = kept.checked_sub(1).map(|last| held[last]) else {
            kept = 1;
            continue;
        };
        let (a, b) = (members.range(last), members.range(current));
        if ranges_adjacent(&a, &b, members.discrete) || !range_before(&a, &b) {
            held[kept - 1] = Member {
                lower: if compare_bounds(&a.lower, &b.lower).is_lt() {
                    last.lower
                } else {
                    current.lower
                },
                upper: if compare_bounds(&a.upper, &b.upper).is_gt() {
                    last.upper
                } else {
                    current.upper
                },
            };
        } else {
            held[kept] = current;
            kept += 1;
        }
    }
    held.truncate(kept);
    held
}
