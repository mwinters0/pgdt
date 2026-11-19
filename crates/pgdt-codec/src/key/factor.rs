//! **A key factor**: what an ordering key over a value the server normalizes
//! holds per byte of the value's text — a `jsonb` document's pair positions
//! and a multirange's member positions, each stated beside the key that
//! holds them — known from a column's comparison before a row is read
//! (`docs/design/decisions.md`, "D152").
//!
//! What it costs in time is unpriced: a `jsonb` term reads its field twice,
//! a sort keys two pairs again per comparison and a multirange each member
//! once per comparison, and no `--count-pair` row or figure reads such a term.

use crate::compare::{CompareKind, NestedCompare};
pub use crate::value::factor::KeyFactor;
use crate::value::json::JSONB_KEY_FACTOR;
use crate::value::nested::key::MULTIRANGE_KEY_FACTOR;

/// The key factor of a term ordering or equating a scalar column of `kind`,
/// `None` where its key holds nothing that grows with the value beside its
/// text.
pub fn key_factor(kind: &CompareKind) -> Option<KeyFactor> {
    matches!(kind, CompareKind::Jsonb).then_some(JSONB_KEY_FACTOR)
}

/// The key factor of a term ordering a nested column by `plan`: the largest
/// of its positions', a part being ordered and dropped before the next.
pub fn nested_key_factor(plan: &NestedCompare) -> Option<KeyFactor> {
    match plan {
        NestedCompare::Leaf { kind, .. } => key_factor(kind),
        NestedCompare::Uncomparable { .. } | NestedCompare::Int2Vector { .. } => None,
        NestedCompare::Array { element, .. } => nested_key_factor(&element.plan),
        NestedCompare::Record { fields, .. } => fields
            .iter()
            .map(|(_, position)| nested_key_factor(&position.plan))
            .fold(None, KeyFactor::max),
        NestedCompare::Range { bound, .. } => nested_key_factor(bound),
        NestedCompare::Multirange { bound, .. } => {
            KeyFactor::max(Some(MULTIRANGE_KEY_FACTOR), nested_key_factor(bound))
        }
    }
}

#[cfg(test)]
mod tests;
