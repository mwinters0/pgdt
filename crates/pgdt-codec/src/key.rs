//! A value's place in PostgreSQL's order, read from its text, and the field
//! checks a strict parse reads through the same readers.
//!
//! A key is a function of a column's [`CompareKind`] (or
//! [`NestedCompare`](crate::compare::NestedCompare)) and one unescaped text
//! alone, so a field, a filter's literal, a stored bound and a gathered value
//! are ordered by one comparison: the filter's (`pgdt_query::predicate`), the
//! statistics gatherer's (`pgdt_query::gather`) and the summaries' alike. What
//! a literal of each kind has to look like, for the refusal naming it, is
//! [`crate::grammar`]'s, with the refusal checks a strict parse dispatches
//! through these readers.

pub(crate) mod order;

use std::cmp::Ordering;

use self::order::{OrderKey, compare_keys, field_key, order_key};
use crate::compare::CompareKind;
use crate::decode::Read;

/// A value's place in its column's order, opaque outside this module: the key
/// a filter orders a row by, for a caller ordering values that meet no filter
/// — the statistics gatherer's running bounds and sortedness.
#[derive(Debug, Clone)]
pub struct ValueKey(OrderKey);

impl ValueKey {
    /// The key of one unescaped value, `None` when the text is not a value of
    /// `kind` ([`order_key`]).
    pub fn of(kind: &CompareKind, text: &str) -> Option<Self> {
        order_key(kind, text).map(Self)
    }

    /// [`Self::of`] a field, saying why it has no key: the statistics
    /// gatherer fails the parse on one the type's `*_in` refuses.
    pub fn of_field(kind: &CompareKind, text: &str) -> Read<Self> {
        field_key(kind, text).map(Self)
    }

    /// [`compare_keys`]: a total order over the keys of one kind, the one a
    /// filter compares by, of which [`Self::stored_order`] is a refinement.
    pub fn compare(&self, other: &Self) -> Ordering {
        compare_keys(&self.0, &other.0)
    }

    /// **The order a stored bound and a block's row order are kept in**:
    /// [`compare_keys`], the order a filter compares by, with a float's `-0`
    /// below `0` where it ties them, as IEEE `totalOrder` — the order Arrow
    /// sorts a float by and DataFusion takes its `MIN`/`MAX` in — places
    /// them. A refinement, so an extreme in it is one in the filter's order
    /// and a block sorted in it is sorted in that order too
    /// (`docs/design/decisions.md`, "D79").
    pub fn stored_order(&self, other: &Self) -> Ordering {
        compare_keys(&self.0, &other.0).then_with(|| match (&self.0, &other.0) {
            (OrderKey::Float(x), OrderKey::Float(y)) => x.total_cmp(y),
            _ => Ordering::Equal,
        })
    }

    /// The heap this key holds, as the sizes the allocator was asked for:
    /// what gathering charges a keyed bound or row-order value
    /// (`pgdt_query::statistics::StatisticsAccount`).
    pub fn heap_bytes(&self) -> u64 {
        match &self.0 {
            OrderKey::Numeric(numeric) => numeric.heap_bytes(),
            OrderKey::Jsonb(jsonb) => jsonb.heap_bytes(),
            OrderKey::Bytes(bytes) => bytes.capacity() as u64,
            OrderKey::Text(text) => text.capacity() as u64,
            OrderKey::NegativeInfinity
            | OrderKey::Bool(_)
            | OrderKey::Int(_)
            | OrderKey::Float(_)
            | OrderKey::Decimal(_)
            | OrderKey::Interval(_)
            | OrderKey::IntervalFields(..)
            | OrderKey::TimeTz { .. }
            | OrderKey::Network(_)
            | OrderKey::PositiveInfinity
            | OrderKey::NotANumber => 0,
        }
    }
}
