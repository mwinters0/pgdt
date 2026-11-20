//! A block's per-row-group column statistics, and the fields PostgreSQL
//! refuses that an ignoring parse went past, as the cache persists them
//! (`docs/design/decisions.md`, "D34", "D103"). What a group is, and the
//! views a column's statistics are read through, are `crate::statistics`'.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};

use super::block::Unrepresentable;
use pgdt_core::internal as instrument;

/// A field PostgreSQL refuses, where a statistics observer met it
/// ([`BlockGathered::Refused`](crate::BlockGathered::Refused), [`ColumnRefusals::first`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldRefusal {
    /// Where the field's row starts, relative to the block's first data byte.
    pub offset: u64,
    /// The row's line as `COPY` numbers it, `1` being the block's first data
    /// line: the rows the observer meeting it was handed before it, plus
    /// one, and the rows before that observer's first once the block folds
    /// it in ([`crate::internal::BlockObserver::absorb`]).
    pub line: u64,
    /// The field's column, by its position in the block's header.
    pub column: usize,
    /// The column's declared type.
    pub declared_type: String,
    /// The field's text as a restore's error quotes it — its first 100 bytes
    /// and `...` where longer (I94) — or `None` for a NULL its column's `NOT
    /// NULL` refuses ([`pgdt_core::Error::NullRefused`]).
    pub value: Option<String>,
}

/// **The fields PostgreSQL refuses — by its type's `*_in` or its column's
/// `NOT NULL` — that a parse told to ignore them went past in one block** ([`crate::CopyBlock::ignored_refusals`]),
/// **per column**: each column holding one keeps its first in the block's row
/// order, quoted as [`FieldRefusal::value`] is, and how many. A fact about the dump, not the mode that met
/// it, so a parse under [`crate::PostgresInvalidValues::Default`] over a cache
/// holding one fails with it exactly where it tracks the column, re-reading
/// nothing ([`pgdt_core::Error::FieldRefusedRecorded`]).
///
/// **Only the fields gathering read**, as a parse under
/// [`crate::PostgresInvalidValues::Default`] checks only those: a gathered
/// column's scalar values and its NULLs, up to each observer's own stop where the block
/// declined — for a block read in pieces, possibly past the decline. A
/// strict parse, which checks every field, does not read it: it re-reads the
/// block, which no strict parse can have checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredRefusals {
    /// One per column holding such a field, in the header's column order,
    /// never empty.
    pub columns: Vec<ColumnRefusals>,
}

/// One column's part of [`IgnoredRefusals`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnRefusals {
    /// The column's first, in the block's row order; its
    /// [`FieldRefusal::column`] is the column's.
    pub first: FieldRefusal,
    /// Every one the column's gathering keyed, `first` included.
    pub count: u64,
}

/// One block's statistics, held by [`crate::CopyBlock::statistics`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockStatistics {
    /// `N`, the bytes of block data each group covers.
    pub group_size: u64,
    /// The request `group_size` was chosen under, which a back-fill compares
    /// a later request's against (`docs/design/decisions.md`, "D34").
    pub sizing: GroupSizing,
    /// Every group from the first to the one the last row starts in, in order.
    pub groups: Vec<RowGroup>,
    /// One entry per column of the block's header, `None` for a column no
    /// request that gathered the block tracked.
    pub columns: Vec<Option<ColumnStatistics>>,
}

/// How a block's group size was chosen: what [`BlockStatistics::sizing`]
/// records, a size no longer saying which request chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupSizing {
    /// A stated group size, gathered exactly.
    Stated,
    /// An unstated size: [`pgdt_mapping::statistics::ROW_GROUP_DEFAULT_SIZE_BYTES`], or the finer size a
    /// re-read for a broken maximum predicts, merged pairwise
    /// past [`pgdt_mapping::statistics::BLOCK_MAX_ROW_GROUPS`] groups and, once the block is finished,
    /// until its median group holds `min_rows` rows or it is one group.
    ///
    /// `max_rows` is the stated maximum the size was chosen under, if any: it
    /// stops the merging where the next size would put more than that many
    /// rows in the 90th-percentile group, and it turns the cap off, so a block
    /// recorded under one holds whatever its own density asked for.
    Density { min_rows: u64, max_rows: Option<u64> },
}

/// One group's extent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowGroup {
    /// Rows whose first byte lies in the group. Zero for a group no row
    /// starts in.
    pub rows: u64,
    /// From the group's first row's first byte to its last row's line end,
    /// which a row straddling the group's end carries past `N`.
    pub bytes: u64,
}

/// One column's statistics over a block, every per-group vector as long as
/// [`BlockStatistics::groups`].
///
/// **What a [`StatisticsView`](crate::StatisticsView) gives its meaning is
/// read only through it**: the NULL counts, the bounds, the row order and the
/// unrepresentable counts are private to this crate, and [`Self::null_count`], [`Self::group_bounds`] and
/// [`Self::sortedness`] read them under a view, and
/// [`Self::holds_unrepresentable`] by the tier a view names, a raw read
/// having silently picked one. A gatherer builds one, and a test takes one apart, through
/// `internal`'s [`ColumnStatisticsParts`](crate::internal::ColumnStatisticsParts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnStatistics {
    /// The declared type text the statistics were computed under, as the
    /// preamble recorded it; `None` where the column was not declared.
    pub declared_type: Option<String>,
    /// The column's `COLLATE` clause, verbatim; `None` for no clause.
    pub collation: Option<String>,
    pub(crate) null_counts: Vec<u64>,
    pub(crate) bounds: Option<ColumnBounds>,
    pub(crate) datafusion_bounds: Option<ColumnBounds>,
    /// Present for a column its comparison equates exactly.
    pub dictionary: Option<ColumnDictionary>,
    pub(crate) unrepresentable: Option<Vec<Unrepresentable>>,
    /// Per group, the sum of its non-NULL values the type holds as integers —
    /// a decimal's unscaled — wrapped at 128 bits, which reduces exactly to
    /// any narrower wrapping sum; a `NaN` is left out, as a NULL is, and
    /// counted as unrepresentable. Kept for a column the typed read
    /// emits as `Int16`, `Int32`, `Int64`, `UInt32` or `Decimal128` — `int2`,
    /// `int4`, `int8`, `oid` and a `numeric(p,s)` of at most 38 digits — and
    /// `None` for every other, **and for one holding a value that does not
    /// decode as its type**: a sum cannot leave a value out the way a bound
    /// can.
    pub sums: Option<Vec<i128>>,
    /// Per group, the summed length of its non-NULL fields' text once
    /// unescaped — the bytes a `Utf8View` of them holds, and never fewer than
    /// a `bytea`'s decoded bytes or an enum's labels — a field that does not
    /// decode counting its escaped bytes. Kept for every column, whatever the
    /// typed read emits it as: the measure is the text's, so a read in
    /// [`pgdt_codec::SchemaMode::Strings`] sizes every column by it.
    pub value_bytes: Vec<u64>,
}

/// Bounds per group and the block's row order, for one column, **over the
/// values its Arrow type holds**, and beside them the views that differ where
/// a group holds one it cannot ([`StatisticsView`](crate::StatisticsView)):
/// one stored set of a [`ColumnStatistics`], read through its view-taking methods and reached raw
/// only through `internal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnBounds {
    /// The order of the non-NULL values the type holds over the whole block,
    /// row by row.
    pub sortedness: Sortedness,
    /// Per group, over the values the type holds; `None` where the group
    /// holds no such value or one the bounds could not cover.
    pub groups: Vec<Option<Bounds>>,
    /// Over every value, where some group holds one past Arrow's format spec:
    /// [`StatisticsView::Every`](crate::StatisticsView::Every)'s, listed only
    /// for a group that does.
    pub every: Option<BoundsView>,
    /// Over the values within the engine's calendar as well, where some group
    /// holds one past it:
    /// [`StatisticsView::Displayable`](crate::StatisticsView::Displayable)'s,
    /// listed only for a group that does.
    pub displayable: Option<BoundsView>,
}

/// One [`StatisticsView`](crate::StatisticsView)'s bounds and row order where
/// it is not
/// [`ColumnBounds`]' own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundsView {
    /// The order of the non-NULL values the view takes, over the whole block.
    pub sortedness: Sortedness,
    /// Per group, `None` for one holding no value the view reads otherwise
    /// than [`ColumnBounds::groups`] does, which it is read from, for one
    /// holding no value it takes, and for one whose bounds could not cover a
    /// value it takes.
    pub groups: Vec<Option<Bounds>>,
}

/// A lower and an upper bound on one group's non-NULL values, as unescaped
/// field text no longer than
/// [`DICTIONARY_ENTRY_MAX_BYTES`](crate::DICTIONARY_ENTRY_MAX_BYTES) — a
/// `bytea`'s in hex, a `character` value's without the trailing blanks its
/// comparison ignores.
///
/// **A float's bounds tell its zeros apart**: where `-0` and `0` tie at an
/// extreme, `min` is `-0` and `max` is `0` if the group holds that zero, as
/// IEEE `totalOrder` — DataFusion's order for a float — places them. Still
/// bounds in PostgreSQL's order, where the two are equal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    /// No value orders below it.
    pub min: String,
    /// No value orders above it.
    pub max: String,
    /// Whether `min` is a value the group holds. A truncated lower bound is a
    /// prefix below every value and is never read as one.
    pub min_exact: bool,
    /// Whether `max` is a value the group holds. A truncated upper bound is
    /// above every value and is never read as one.
    pub max_exact: bool,
}

/// Whether a column's non-NULL values are in order row by row over a block.
/// Equal neighbours are in either order. A value gathering cannot place against its neighbour —
/// one that does not key, a keyed one past
/// [`DICTIONARY_ENTRY_MAX_BYTES`](crate::DICTIONARY_ENTRY_MAX_BYTES), or a
/// bytewise one agreeing with it past what a bound reads — makes the column
/// `Unsorted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sortedness {
    Ascending,
    Descending,
    /// At most one distinct value, every one placed: in both orders, so a
    /// block of it sets no direction for the blocks beside it. Recorded by the
    /// gatherer, never read off equal bounds: a block of one value whose
    /// bounds were truncated holds a prefix and a successor that differ.
    Constant,
    Unsorted,
}

/// Every distinct text per group, interned once per block and column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDictionary {
    /// Each distinct text once, in first-seen order — a `character` value's
    /// without the trailing blanks its comparison ignores, as with [`Bounds`].
    pub entries: Vec<String>,
    /// Per group, indices into `entries`; `None` where the group held more than
    /// [`DICTIONARY_MAX_ENTRIES`](crate::DICTIONARY_MAX_ENTRIES) distinct
    /// texts, a text longer than
    /// [`DICTIONARY_ENTRY_MAX_BYTES`](crate::DICTIONARY_ENTRY_MAX_BYTES), a
    /// field that is not text, or one an ignoring parse went past.
    pub groups: Vec<Option<Vec<u32>>>,
}

/// [`crate::CopyBlock::statistics`] as the cache decodes it, with what the decode
/// allocates attributed to statistics where the instrument is built in
/// (`pgdt_core::instrument`) — and a plain decode where it is not.
pub(crate) fn deserialize_block_statistics<'de, D>(
    deserializer: D,
) -> Result<Option<Arc<BlockStatistics>>, D::Error>
where
    D: Deserializer<'de>,
{
    let _attributed = instrument::StatisticsScope::enter();
    Option::<Arc<BlockStatistics>>::deserialize(deserializer)
}
