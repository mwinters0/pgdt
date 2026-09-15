//! Per-row-group column statistics: what a mapping pass may be asked to
//! gather for a `COPY` block, and the persisted shape it gathers into.
//!
//! L1 vocabulary only (`docs/design/decisions.md`, "D74"): column names, the
//! declared type text and `COLLATE` clause a column's statistics were computed
//! under, counts, and bounds as unescaped field text — what `pg_dump` wrote
//! where the value fits [`STORED_VALUE_CAP`], and a prefix or a successor of it
//! where it does not ([`Bounds::max_exact`]). Which
//! column gets which statistic, and how a value is ordered, is decided above
//! this layer by whatever implements [`BlockObserver`]; the mapping pass hands
//! it every row and stays type-blind.
//!
//! **A row group is a byte range** (`docs/design/decisions.md`, "D34"): group
//! `k` of a block is the rows whose first byte lies in `[k·N, (k+1)·N)` of the
//! block's data, `N` being [`BlockStatistics::group_size`]. A row longer than
//! `N` leaves groups in which no row starts, which are listed, empty.

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use crate::copy::CopyHeader;
use crate::index::CopyBlock;

/// The group size a request that states none gathers at: one mebibyte of a
/// block's data per group.
pub const DEFAULT_STATISTICS_GROUP_SIZE: u64 = 1 << 20;

/// The longest text any stored bound or dictionary entry may be, in bytes.
pub const STORED_VALUE_CAP: usize = 256;

/// The most distinct texts one group's dictionary may hold; a group with more
/// has none on that column.
pub const DICTIONARY_CAP: usize = 64;

/// What a mapping pass is asked to gather: which columns, at what group size
/// (`docs/design/decisions.md`, "D77").
/// An argument of [`crate::stream::map_file`] alone — a query never gathers.
///
/// **The default gathers every statistic** ([`Self::ALL`]), a parse carrying
/// the intent to do all work a later query could use; gathering less is stated,
/// [`Self::NONE`] gathering nothing (`docs/design/roadmap.md`, "A parse does
/// all the work a later query could use").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatisticsRequest {
    /// The columns statistics are gathered for.
    pub selection: StatisticsSelection,
    /// The group size, `None` when the caller stated none and
    /// [`DEFAULT_STATISTICS_GROUP_SIZE`] applies. Kept apart from the default
    /// because a stated size and an unstated one are different requests to a
    /// block already gathered at another. In a block the request does not
    /// track ([`Self::tracked_columns`] answering `None`, as it always does
    /// for [`StatisticsSelection::None`]) it sizes nothing and is ignored.
    pub group_size: Option<NonZeroU64>,
}

impl StatisticsRequest {
    /// Every column of every table, at the default group size — the default.
    pub const ALL: Self = Self { selection: StatisticsSelection::All, group_size: None };

    /// Nothing gathered: what a query's mapping pass always asks.
    pub const NONE: Self = Self { selection: StatisticsSelection::None, group_size: None };

    /// Whether this request may track a column at all — false for
    /// [`StatisticsSelection::None`] alone.
    pub fn gathers(&self) -> bool {
        self.selection != StatisticsSelection::None
    }

    /// The group size this request gathers at.
    pub fn group_size(&self) -> u64 {
        self.group_size.map_or(DEFAULT_STATISTICS_GROUP_SIZE, NonZeroU64::get)
    }

    /// Which of `header`'s columns this request tracks, positionally — `None`
    /// when it tracks nothing in the block, which then gathers no statistics
    /// at all. A block whose header names no columns is tracked with no
    /// column, its groups still counted.
    pub fn tracked_columns(&self, header: &CopyHeader) -> Option<Vec<bool>> {
        let targets = match &self.selection {
            StatisticsSelection::All => return Some(vec![true; header.columns.len()]),
            StatisticsSelection::None => return None,
            StatisticsSelection::Only(targets) => targets,
        };
        let mut whole_table = false;
        let mut tracked = vec![false; header.columns.len()];
        for target in targets {
            match target {
                StatisticsTarget::Table(table) if header.matches(table) => whole_table = true,
                StatisticsTarget::Column { table, column } if header.matches(table) => {
                    if let Some(i) = header.columns.iter().position(|c| c == column) {
                        tracked[i] = true;
                    }
                }
                _ => {}
            }
        }
        if whole_table {
            return Some(vec![true; header.columns.len()]);
        }
        tracked.contains(&true).then_some(tracked)
    }

    /// What re-reading `block`, already mapped, must gather for it to hold
    /// what this request asks of it — `None` when it holds that already, or
    /// when the request tracks nothing in it.
    ///
    /// **A block lacks the requested statistics** where it holds none, where a
    /// column the request tracks was not gathered, or where the request
    /// **states** a group size other than the one the block was gathered at;
    /// an unstated size lacks nothing a gathered block holds
    /// (`docs/design/decisions.md`, "D34").
    pub fn backfill(&self, block: &CopyBlock) -> Option<StatisticsBackfill> {
        let requested = self.tracked_columns(&block.header)?;
        let Some(held) = block.statistics.as_deref() else {
            return Some(StatisticsBackfill { columns: requested, group_size: self.group_size() });
        };
        let resized = self.group_size.is_some_and(|size| size.get() != held.group_size);
        let missing = requested
            .iter()
            .enumerate()
            .any(|(i, &wanted)| wanted && held.columns.get(i).is_none_or(Option::is_none));
        if !resized && !missing {
            return None;
        }
        let columns = requested
            .iter()
            .enumerate()
            .map(|(i, &wanted)| wanted || held.columns.get(i).is_some_and(Option::is_some))
            .collect();
        let group_size = self.group_size.map_or(held.group_size, NonZeroU64::get);
        Some(StatisticsBackfill { columns, group_size })
    }
}

/// What one mapped block is re-read to gather, from
/// [`StatisticsRequest::backfill`]: **everything the block already held as
/// well as everything asked for**, so a back-fill never loses a statistic an
/// earlier pass gathered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatisticsBackfill {
    /// The columns gathered, positionally to the block's header.
    pub columns: Vec<bool>,
    /// The group size gathered at: the request's stated size, or else the
    /// size the block already held, or else [`DEFAULT_STATISTICS_GROUP_SIZE`].
    pub group_size: u64,
}

/// The columns a [`StatisticsRequest`] names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StatisticsSelection {
    /// Every column of every table.
    #[default]
    All,
    /// Only these tables and columns.
    Only(Vec<StatisticsTarget>),
    /// No column of any table.
    None,
}

/// One entry of a narrowed selection. A table is named as
/// [`CopyHeader::matches`] reads it, qualified or bare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatisticsTarget {
    /// Every column of the table.
    Table(String),
    /// One column of the table.
    Column { table: String, column: String },
}

/// Receives every row of one `COPY` block from the mapping pass and answers
/// the block's statistics once it closes. Defined here and implemented at L4,
/// the shape `docs/design/decisions.md`, "D74" gives a cross-layer trait.
///
/// **A block the leader splits is observed piece by piece**: each piece's rows
/// go to an observer [`Self::piece`] made, and the pieces are handed back in
/// file order to [`Self::absorb`], after which the block's observer answers
/// what it would have had it been handed every row itself.
pub(crate) trait BlockObserver: Send {
    /// One row, `offset` being where its first byte sits relative to the
    /// block's first data byte, and `raw` the still-escaped line without its
    /// terminator.
    fn observe_row(&mut self, offset: u64, raw: &[u8]);

    /// The block's statistics. `end` is the terminator line's offset relative
    /// to the block's first data byte — where the last row's line ends.
    fn finish(self: Box<Self>, end: u64) -> BlockStatistics;

    /// An observer for a piece of this block: rows starting anywhere after the
    /// ones this observer has been handed, to be given back to
    /// [`Self::absorb`] rather than finished.
    fn piece(&self) -> Box<dyn BlockObserver>;

    /// Fold `later`, made by [`Self::piece`] and handed rows that all follow
    /// every row this observer has seen, into this one.
    fn absorb(&mut self, later: Box<dyn BlockObserver>);

    /// This observer as [`Any`](std::any::Any), which is how [`Self::absorb`]
    /// recovers the concrete piece it made.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;
}

/// One block's statistics, held by [`crate::index::CopyBlock::statistics`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockStatistics {
    /// `N`, the bytes of block data each group covers.
    pub group_size: u64,
    /// Every group from the first to the one the last row starts in, in order.
    pub groups: Vec<RowGroup>,
    /// One entry per column of the block's header, `None` for a column the
    /// request did not track.
    pub columns: Vec<Option<ColumnStatistics>>,
}

/// One group's extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnStatistics {
    /// The declared type text the statistics were computed under, as the
    /// preamble recorded it; `None` where the column was not declared.
    pub declared_type: Option<String>,
    /// The column's `COLLATE` clause, verbatim; `None` for no clause.
    pub collation: Option<String>,
    /// NULLs per group.
    pub null_counts: Vec<u64>,
    /// Present for a column its comparison orders exactly.
    pub bounds: Option<ColumnBounds>,
    /// Present for a column its comparison equates exactly.
    pub dictionary: Option<ColumnDictionary>,
}

/// Bounds per group and the block's row order, for one column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnBounds {
    /// The non-NULL values' order over the whole block, row by row.
    pub sortedness: Sortedness,
    /// Per group; `None` where the group holds no non-NULL value or a value
    /// the bounds could not cover.
    pub groups: Vec<Option<Bounds>>,
}

/// A lower and an upper bound on one group's non-NULL values, as unescaped
/// field text no longer than [`STORED_VALUE_CAP`] — a `character` value's
/// without the trailing blanks its comparison ignores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    /// No value orders below it.
    pub min: String,
    /// No value orders above it.
    pub max: String,
    /// Whether `max` is a value the group holds. A truncated upper bound is
    /// above every value and is never read as one.
    pub max_exact: bool,
}

/// Whether a column's non-NULL values are in order row by row over a block.
/// Equal neighbours are in either order; a column with at most one distinct
/// value is `Ascending`. A value gathering cannot place against its neighbour —
/// one that does not key, a keyed one past [`STORED_VALUE_CAP`], or a bytewise
/// one agreeing with it past what a bound reads — makes the column `Unsorted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sortedness {
    Ascending,
    Descending,
    Unsorted,
}

/// Every distinct text per group, interned once per block and column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDictionary {
    /// Each distinct text once, in first-seen order — a `character` value's
    /// without the trailing blanks its comparison ignores, as with [`Bounds`].
    pub entries: Vec<String>,
    /// Per group, indices into `entries`; `None` where the group held more than
    /// [`DICTIONARY_CAP`] distinct texts, a text longer than
    /// [`STORED_VALUE_CAP`], or a field that is not text.
    pub groups: Vec<Option<Vec<u32>>>,
}
