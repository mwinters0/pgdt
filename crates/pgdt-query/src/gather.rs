//! The statistics gatherer: [`pgdt_model::BlockObserver`] implemented
//! in query, where a column's comparison is known
//! (`docs/design/decisions.md`, "D74").
//!
//! [`observer_for`] builds one per `COPY` block from the block's resolved
//! schema. Every tracked column counts its NULLs per group; a scalar column
//! also keeps per-group bounds and the block's row order in each order some
//! semantics compares it by exactly, under the key a filter orders by, a
//! float's zeros told apart ([`ValueKey::stored_order`](pgdt_codec::key::ValueKey::stored_order)) — one set where the
//! orders coincide, two where PostgreSQL's is exact and Arrow's is another
//! (`pgdt_codec::ResolvedSchema::bounds_kinds`);
//! a column its comparison equates exactly keeps a dictionary per group; and a
//! column the typed read emits as an integer or a `Decimal128` keeps a sum per
//! group; and every column keeps its values' text bytes per group. **A column
//! whose type cannot hold every value counts those per group**, and its
//! bounds and row order are over the values it holds, each other view kept
//! apart only where a group holds one it reads otherwise
//! (`docs/design/decisions.md`, "D97"; [`KeyedGroup`](self::bounds::KeyedGroup), [`ViewOrders`](self::order::ViewOrders)).
//!
//! **A leader piece gathers into an observer of its own, and the pieces join
//! in file order into exactly what one observer handed every row gathers**
//! ([`Gatherer::join`](self::gatherer::Gatherer::join)), unless the block declines, which a stated allowance
//! decides over every piece charging at once (`docs/design/decisions.md`,
//! "D85"). Two things cross a join: the group a cut falls
//! inside, which a piece holds open rather than closing ([`Gatherer::head`](self::gatherer::Gatherer::head)),
//! and each ordered column's first value, which the rows before the piece
//! place their last value against ([`RowOrder`](self::order::RowOrder)).
//!
//! **Every observer charges what it holds to the pass's
//! [`StatisticsAccount`](crate::statistics::StatisticsAccount)** (`docs/design/decisions.md`, "D81"): a block's
//! observer as [`Term::Gathering`](crate::statistics::Term::Gathering), a piece's as [`Term::Pieces`](crate::statistics::Term::Pieces), each
//! dictionary's interning map as [`Term::Interned`](crate::statistics::Term::Interned). What it holds is
//! recomputed, O(1) a column, whenever its rows' growth passes
//! [`STATISTICS_ACCOUNT_CHARGE_STEP`](crate::statistics::STATISTICS_ACCOUNT_CHARGE_STEP), as each column closes a group and through a piece's fold;
//! a vector or a map is charged ahead of the allocation growing it.
//!
//! **A block past its cap merges its closed groups pairwise into exactly what
//! gathering at twice the size gathers** (`docs/design/decisions.md`, "D82"),
//! never mid-scan while a piece it made is alive; at the block's end it merges
//! whatever is outstanding, a piece alive there joining nothing more
//! ([`Gatherer::fit_cap`](self::gatherer::Gatherer::fit_cap)). **A finished
//! block short of its density minimum merges the same way**
//! (`docs/design/decisions.md`, "D82"; [`Gatherer::fit_density`](self::gatherer::Gatherer::fit_density)).
//!
//! **A block whose gathering passes the pass's statistics allowance declines**
//! ([`Gatherer::decline`](self::gatherer::Gatherer::decline)): it frees what it holds, reads its remaining rows
//! for the census alone — and a strict parse's check ([`checker`]) — and
//! answers [`BlockGathered::Declined`](pgdt_model::BlockGathered::Declined) with the
//! allowance, which the map records (`docs/design/decisions.md`, "D85"). The
//! close `Gatherer::finish` makes is kept even where it passes the
//! allowance; the next block declines at its first charge.

mod bounds;
mod charge;
mod column;
mod dictionary;
mod gatherer;
mod order;
mod strict;
mod tracking;

#[cfg(test)]
pub(crate) use self::bounds::one_group_bounds;
pub(crate) use self::strict::checker;
pub use self::strict::{
    StrictUnchecked, UncheckedBound, UncheckedCheck, UncheckedColumn, strict_unchecked,
};
pub(crate) use self::tracking::{
    bounded_columns, declared_column, dictionary_holds_field_text, keeps_sums, observer_for,
    observer_tracking, stored_bounds_kinds, stored_resolution,
};

#[cfg(test)]
mod tests;
