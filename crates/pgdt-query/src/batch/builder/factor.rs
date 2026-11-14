//! **A decode factor**: the most a column's builders, and the walk feeding
//! them, hold beside a field's text per byte of it while the field is
//! appended — known from the column's builder before a row is read, and
//! folded with a filter's key factors into a query's one factor
//! (`docs/design/decisions.md`, "D154").
//!
//! What it counts:
//!
//! - **what a nested value's builders are reserved to** past its chunk term
//!   (`reserved_column_builder`): each repeated part's entry, at every depth,
//!   over the fewest bytes the part is written in, its delimiter included,
//!   and a text's or a `bytea`'s own bytes one for one — every buffer at its
//!   own density, since each is reserved so, and so never grown by doubling;
//! - **what a part's append holds besides**, one part at a time: an escaped
//!   field's token and its unquoted copy, and a `bytea` part's decoded bytes —
//!   the most a tier test's walk holds too, reading the same parts before the
//!   decode does (`decisions.md`, "D153").
//!
//! Not counted: what a value holds whatever its text's length — a scalar's
//! entry, a record's and a range's own, and a list's at the top — which the
//! column's type bounds per row; a scalar's own text, which never grows long;
//! and a buffer's slack and copy as a batch grows within its span, the
//! span's (`decisions.md`, "D141").

use arrow::datatypes::DataType;

use pgdt_codec::internal::KeyFactor;
use pgdt_codec::pgtype::NestedPlan;

use super::{ColumnBuilder, is_walked, new_column_builder, shortest};

/// At most `held` bytes for every `per` bytes of a text, beside the text:
/// what a value's decode or an ordering key over it holds while it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Factor {
    held: u64,
    per: u64,
}

impl Factor {
    const ZERO: Self = Self { held: 0, per: 1 };
    const ONE: Self = Self { held: 1, per: 1 };

    /// `held` for every `per`, in lowest terms.
    pub(crate) fn new(held: u64, per: u64) -> Self {
        assert!(per > 0, "a factor is per some text");
        let (mut a, mut b) = (held, per);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Self { held: held / a.max(1), per: per / a.max(1) }
    }

    /// `bits` held for every `bytes` of text.
    fn bits(bits: u64, bytes: u64) -> Self {
        Self::new(bits, 8 * bytes)
    }

    /// `held` and `per`, in lowest terms: what an instrumented binary reports.
    #[cfg(feature = "introspect")]
    pub(crate) fn parts(self) -> (u64, u64) {
        (self.held, self.per)
    }

    /// The most held beside a text of `len` bytes.
    pub(crate) fn of(self, len: u64) -> u64 {
        let held = (u128::from(len) * u128::from(self.held)).div_ceil(u128::from(self.per));
        u64::try_from(held).unwrap_or(u64::MAX)
    }

    /// The larger of the two.
    pub(crate) fn max(self, other: Self) -> Self {
        let mine = u128::from(self.held) * u128::from(other.per);
        if mine >= u128::from(other.held) * u128::from(self.per) { self } else { other }
    }

    fn plus(self, other: Self) -> Self {
        Self::new(self.held * other.per + other.held * self.per, self.per * other.per)
    }

    fn is_zero(self) -> bool {
        self.held == 0
    }
}

impl From<KeyFactor> for Factor {
    fn from(key: KeyFactor) -> Self {
        Self::new(key.held, key.per)
    }
}

/// **The decode factor of a column of `data_type` filled as `plan` says**:
/// `None` where its decode holds nothing that grows with a value's text.
pub(crate) fn column_factor(data_type: &DataType, plan: &NestedPlan) -> Option<Factor> {
    let builder = new_column_builder(data_type, plan);
    let factor = match &builder {
        // A text read in place is a view into its held line, an escaped one
        // its own block, and a `bytea` decoded into its column: one copy at
        // most, every decode shortening a value (D141).
        ColumnBuilder::Utf8View(..) | ColumnBuilder::Binary(_) => Factor::ONE,
        b if is_walked(b) => reserved(b).plus(stage(b)),
        _ => Factor::ZERO,
    };
    (!factor.is_zero()).then_some(factor)
}

/// A list's entry: its `i32` offset and its validity, a byte until finished.
const LIST_ENTRY_BITS: u64 = 32 + 8;
/// A struct's validity, a byte until finished.
const STRUCT_ENTRY_BITS: u64 = 8;

/// The bits one entry of `builder` holds however long its text, its repeated
/// parts aside: a leaf's Arrow slot and validity bit, a view's sixteen bytes
/// holding up to twelve of a text's; a list's entry, and an `int2vector`'s
/// first element with it; a struct's entry and each child's.
fn entry_bits(builder: &ColumnBuilder) -> u64 {
    use ColumnBuilder as B;
    let slot = match builder {
        B::Bool(_) => 1,
        B::Int16(_) => 16,
        // An enum's key: its label is its column's, held once.
        B::Int32(_) | B::UInt32(_) | B::Float32(_) | B::Date32(_) | B::Dictionary(_) => 32,
        // A `bytea`'s offset; its bytes are its value's own.
        B::Binary(_) => 32,
        B::Int64(_) | B::Float64(_) | B::TimestampMicro { .. } | B::Time64Micro(_) => 64,
        B::Utf8View(..)
        | B::IntervalMonthDayNano(_)
        | B::Decimal128 { .. }
        | B::FixedSizeBinary16(_) => 128,
        B::Decimal256 { .. } => 256,
        B::Array(_) | B::Multirange(_) => return LIST_ENTRY_BITS,
        B::Int2Vector(parts) => return LIST_ENTRY_BITS + entry_bits(&parts.child),
        B::Record(parts) | B::Range(parts) => {
            return STRUCT_ENTRY_BITS + parts.children.iter().map(entry_bits).sum::<u64>();
        }
    };
    slot + 1
}

/// **What a nested value's builders are reserved to**, per byte of its text
/// (`reserved_column_builder`): each repeated part's entry over its fewest
/// bytes, summed over every depth, and one where a text's or a `bytea`'s own
/// bytes are held. Every buffer is reserved at its own density, so this is
/// their sum where [`form`] is their largest.
fn reserved(builder: &ColumnBuilder) -> Factor {
    match builder {
        // A text's bytes past its view are blocks of their own, never grown
        // by doubling; a `bytea`'s are reserved to its text's length.
        ColumnBuilder::Utf8View(..) | ColumnBuilder::Binary(_) => Factor::ONE,
        ColumnBuilder::Array(parts)
        | ColumnBuilder::Multirange(parts)
        | ColumnBuilder::Int2Vector(parts) => {
            let child = &*parts.child;
            Factor::bits(entry_bits(child), shortest(child) + 1).plus(reserved(child))
        }
        ColumnBuilder::Record(parts) | ColumnBuilder::Range(parts) => {
            parts.children.iter().map(reserved).fold(Factor::ZERO, Factor::plus)
        }
        _ => Factor::ZERO,
    }
}

/// **The Arrow form a value's text can grow**, per byte of it: the largest of
/// each repeated part's entry over the fewest bytes it is written in with its
/// delimiter — a `bytea` part's one byte held beside its offset — and one
/// where a text's or a `bytea`'s own bytes are held. What the tests hold a
/// column's finished buffers to.
#[cfg(test)]
fn form(builder: &ColumnBuilder) -> Factor {
    match builder {
        ColumnBuilder::Utf8View(..) | ColumnBuilder::Binary(_) => Factor::ONE,
        ColumnBuilder::Array(parts)
        | ColumnBuilder::Multirange(parts)
        | ColumnBuilder::Int2Vector(parts) => {
            let child = &*parts.child;
            let byte = if matches!(child, ColumnBuilder::Binary(_)) { 8 } else { 0 };
            Factor::bits(entry_bits(child) + byte, shortest(child) + 1).max(form(child))
        }
        ColumnBuilder::Record(parts) | ColumnBuilder::Range(parts) => {
            parts.children.iter().map(form).fold(Factor::ZERO, Factor::max)
        }
        _ => Factor::ZERO,
    }
}

/// **What a part's append holds besides**, per byte of the part, one part at
/// a time: an escaped field's token and the part unquoted, and a `bytea`'s
/// bytes decoded before they are appended. A scalar part's are its own text.
fn stage(builder: &ColumnBuilder) -> Factor {
    match builder {
        ColumnBuilder::Utf8View(..) => Factor::new(2, 1),
        ColumnBuilder::Binary(_) => Factor::new(3, 1),
        ColumnBuilder::Array(parts)
        | ColumnBuilder::Multirange(parts)
        | ColumnBuilder::Int2Vector(parts) => stage(&parts.child),
        ColumnBuilder::Record(parts) | ColumnBuilder::Range(parts) => {
            parts.children.iter().map(stage).fold(Factor::ZERO, Factor::max)
        }
        _ => Factor::ZERO,
    }
}

#[cfg(test)]
mod tests;
