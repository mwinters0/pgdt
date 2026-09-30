//! Per-row-group column statistics: what a mapping pass may be asked to
//! gather for a `COPY` block, the persisted shape it gathers into, and the
//! account of every statistic a pass holds alive ([`StatisticsAccount`]).
//!
//! L1 vocabulary only (`docs/design/decisions.md`, "D74"): column names, the
//! declared type text and `COLLATE` clause a column's statistics were computed
//! under, counts, and bounds as unescaped field text — what `pg_dump` wrote, a
//! `character`'s without its trailing blanks, where the value fits
//! [`DICTIONARY_ENTRY_MAX_BYTES`]; where it does not, a bytewise-ordered
//! column's bound is a prefix or a successor of it ([`Bounds::min_exact`],
//! [`Bounds::max_exact`]) and any other column's group keeps none. Which
//! column gets which statistic, and how a value is ordered, is decided above
//! this layer by whatever implements [`BlockObserver`]; the mapping pass hands
//! it every row and stays type-blind.
//!
//! **A row group is a byte range** (`docs/design/decisions.md`, "D34"): group
//! `k` of a block is the rows whose first byte lies in `[k·N, (k+1)·N)` of the
//! block's data, `N` being [`BlockStatistics::group_size`]. A row longer than
//! `N` leaves groups in which no row starts, which are listed, empty.

use std::mem::size_of;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Deserializer, Serialize};

use crate::copy::CopyHeader;
use crate::index::{CopyBlock, Unrepresentable, UnrepresentableTier};
use crate::instrument;

/// The group size a request that states none gathers at: one mebibyte of a
/// block's data per group.
pub const ROW_GROUP_DEFAULT_SIZE_BYTES: u64 = 1 << 20;

/// The most groups a block's statistics hold under an unstated group size and
/// an unstated maximum: past it, adjacent groups merge pairwise and the
/// block's group size doubles, so what a long block holds grows with its
/// columns rather than its bytes (`docs/design/decisions.md`, "D82"). A stated
/// maximum turns the cap off ([`StatisticsRequest::group_cap`]), so a block
/// gathered under one may hold more groups than this. A judgement, not a
/// reading.
pub const BLOCK_MAX_ROW_GROUPS: usize = 4096;

/// The fewest rows a block's median group — its upper middle one, so that at
/// most half the groups fall short — holds under an unstated group size and an
/// unstated minimum: short of it, the finished block's groups merge
/// pairwise until that group reaches it, the block is one group, or the next
/// size would break a stated maximum, which outranks the minimum
/// (`gather::density_merges`; `docs/design/decisions.md`, "D82").
/// `ROW_GROUP_DEFAULT_SIZE_BYTES` over a row a kibibyte wide; a judgement,
/// not a reading.
pub const ROW_GROUP_DEFAULT_MIN_ROWS: u64 = 1 << 10;

/// The longest text any stored bound or dictionary entry may be, in bytes.
pub const DICTIONARY_ENTRY_MAX_BYTES: usize = 256;

/// The most distinct texts one group's dictionary may hold; a group with more
/// has none on that column.
pub const DICTIONARY_MAX_ENTRIES: usize = 64;

/// What a mapping pass is asked to record: each table's and column's level,
/// at what group size and between what density bounds
/// (`docs/design/decisions.md`, "D77"). An argument of
/// [`crate::stream::map_file`] alone — a query never gathers.
///
/// **The default is the data level everywhere** ([`Self::DATA`]), a parse
/// carrying the intent to do all work a later query could use; recording less
/// is stated, [`Self::METADATA`] recording nothing drawn from any block's
/// rows (`docs/design/roadmap.md`, "A parse does all the work a later query
/// could use").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatisticsRequest {
    /// Each table's and column's level: which blocks are censused, and which
    /// columns statistics are gathered for.
    pub selection: StatisticsSelection,
    /// The group size, `None` when the caller stated none and
    /// [`ROW_GROUP_DEFAULT_SIZE_BYTES`] applies, doubled past
    /// [`BLOCK_MAX_ROW_GROUPS`] groups unless [`Self::max_rows`] is stated,
    /// and short of [`Self::min_rows`]; a stated size is gathered exactly.
    /// Kept apart from the default because a stated size and an unstated one
    /// are different requests to a block already gathered at another. In a
    /// block the request does not track ([`Self::tracked_columns`] answering
    /// `None`, as it always does under [`StatisticsSelection::METADATA`]) it sizes
    /// nothing and is ignored.
    pub group_size: Option<NonZeroU64>,
    /// The fewest rows a block's median group should hold, `None` when the
    /// caller stated none and [`ROW_GROUP_DEFAULT_MIN_ROWS`] applies; `0`
    /// coarsens nothing. Kept apart from the default for the reason
    /// [`Self::group_size`] is. Under a stated group size, which is gathered
    /// exactly, it sizes nothing and is ignored.
    pub min_rows: Option<u64>,
    /// The most rows a block's densest tenth of groups should hold — read at
    /// the 90th-percentile group ([`max_rows_group`]) — and **there is no
    /// default**: a maximum applies only where a caller states one.
    ///
    /// Short of a stated group size, it is the only thing that makes a block
    /// finer than [`ROW_GROUP_DEFAULT_SIZE_BYTES`], so where it is stated it **outranks
    /// both the minimum and [`BLOCK_MAX_ROW_GROUPS`]**, which it turns off
    /// ([`Self::group_cap`]) — a person sensitive to what a query reads asked
    /// for the groups, and neither a default nor a bound on memory quietly
    /// overrules them. Under a stated group size it sizes nothing and is
    /// ignored, as [`Self::min_rows`] is.
    pub max_rows: Option<u64>,
}

impl StatisticsRequest {
    /// Every table and column at the data level, at the default group size
    /// and minimum and under no maximum — the default.
    pub const DATA: Self = Self {
        selection: StatisticsSelection::DATA,
        group_size: None,
        min_rows: None,
        max_rows: None,
    };

    /// Every table at the metadata level: no block censused, nothing gathered.
    pub const METADATA: Self = Self {
        selection: StatisticsSelection::METADATA,
        group_size: None,
        min_rows: None,
        max_rows: None,
    };

    /// Whether this request may track a column at all — false where no table
    /// or column is at the data level.
    pub fn gathers(&self) -> bool {
        self.selection.default == StatisticsLevel::Data
            || self.selection.overrides.iter().any(|(_, level)| *level == StatisticsLevel::Data)
    }

    /// The group size this request gathers at, before any merge.
    pub fn group_size(&self) -> u64 {
        self.group_size.map_or(ROW_GROUP_DEFAULT_SIZE_BYTES, NonZeroU64::get)
    }

    /// The most groups a block this request gathers may hold:
    /// [`BLOCK_MAX_ROW_GROUPS`] under an unstated size and an unstated
    /// maximum, `None` under either, both being sizes a caller asked for
    /// exactly.
    pub fn group_cap(&self) -> Option<usize> {
        (self.group_size.is_none() && self.max_rows.is_none()).then_some(BLOCK_MAX_ROW_GROUPS)
    }

    /// The density minimum a block this request gathers is sized by at its
    /// end: the stated or default minimum under an unstated size, `None` under
    /// a stated one.
    pub fn min_rows(&self) -> Option<u64> {
        self.group_size.is_none().then(|| self.min_rows.unwrap_or(ROW_GROUP_DEFAULT_MIN_ROWS))
    }

    /// The density maximum a block this request gathers is sized by at its
    /// end: the stated maximum under an unstated size, and `None` under a
    /// stated size or where no maximum was stated — there being no default.
    pub fn max_rows(&self) -> Option<u64> {
        self.group_size.is_none().then_some(self.max_rows).flatten()
    }

    /// What a block this request gathers from its first row records it was
    /// sized under.
    pub fn sizing(&self) -> GroupSizing {
        match self.min_rows() {
            Some(min_rows) => GroupSizing::Density { min_rows, max_rows: self.max_rows() },
            None => GroupSizing::Stated,
        }
    }

    /// Gathering `columns` from a block's first row as this request sizes it.
    pub(crate) fn gathering(&self, columns: Vec<bool>) -> StatisticsBackfill {
        StatisticsBackfill {
            columns,
            group_size: self.group_size(),
            group_cap: self.group_cap(),
            min_rows: self.min_rows(),
            max_rows: self.max_rows(),
            sizing: self.sizing(),
        }
    }

    /// Which of `header`'s columns this request tracks, positionally — each
    /// column at the data level ([`StatisticsSelection::level`]) — and `None`
    /// when it tracks nothing in the block, which is then at the metadata
    /// level: neither censused nor gathered. A block whose header names no
    /// columns is tracked with no column, its groups still counted, where its
    /// table is at the data level.
    ///
    /// **This is also whether the block is censused**: a table is at the data
    /// level where any of its columns is (`docs/design/decisions.md`, "D35").
    pub fn tracked_columns(&self, header: &CopyHeader) -> Option<Vec<bool>> {
        let data =
            |column: Option<&str>| self.selection.level(header, column) == StatisticsLevel::Data;
        if header.columns.is_empty() {
            return data(None).then(Vec::new);
        }
        let tracked: Vec<bool> = header.columns.iter().map(|c| data(Some(c))).collect();
        tracked.contains(&true).then_some(tracked)
    }

    /// What re-reading `block`, already mapped, must gather for it to hold
    /// what this request asks of it — `None` when it holds that already, or
    /// when the request tracks nothing in it.
    ///
    /// **A block lacks the requested statistics** where it holds none — every
    /// block mapped at the metadata level that this request puts at the data
    /// level among them, the re-read censusing it too — where a
    /// column the request tracks was not gathered or was gathered without the
    /// bounds `bounded` says gathering keeps for it, positionally to the
    /// block's header (`crate::stream::bounded_columns`;
    /// `docs/design/decisions.md`, "D79"), where the request
    /// **states** a group size other than the one the block was gathered at,
    /// or where it states no size, **states** a bound — minimum or maximum —
    /// and the sizing that makes, the other bound at its default, is not the
    /// block's record ([`BlockStatistics::sizing`]). A
    /// resized block is re-read from its first row as the request sizes it; an
    /// unstated size and bounds lack nothing a gathered block holds, and
    /// re-read a block lacking a column at the size it holds, exactly, keeping
    /// its record (`docs/design/decisions.md`, "D34").
    ///
    /// **A block that declined is re-read only under a larger allowance**
    /// ([`CopyBlock::statistics_declined`]): `allowance` is this pass's, and
    /// `None` — an embedder that stated none — declines nothing and so retries
    /// everything. Without the record a `parse` at the same allocation would
    /// re-read and re-decline the same block every run
    /// (`docs/design/decisions.md`, "D85").
    ///
    /// **A block still at the size it gathered from also lacks a stated
    /// maximum its groups break** ([`BlockStatistics::breaks_max_rows`]): no
    /// merge can make a block finer, so the maximum is reachable only by
    /// re-reading it at [`BlockStatistics::predicted_group_size`]. Once that
    /// re-read has happened the block is finer than the size a gather starts
    /// from, so it is never re-read for that maximum again however its rows
    /// cluster.
    ///
    /// **So a run reads a block at most twice for a maximum, and a block it
    /// mapped itself once.** A block gathered under another sizing is resized
    /// first, which this answers from its first row at the default size, and
    /// only the groups that read leaves are asked whether they break the
    /// maximum. Rejected: predicting the finer size from the groups another
    /// sizing left, which saves that first read by guessing where the read
    /// would have measured.
    pub fn backfill(
        &self,
        block: &CopyBlock,
        bounded: &[bool],
        allowance: Option<u64>,
    ) -> Option<StatisticsBackfill> {
        let requested = self.tracked_columns(&block.header)?;
        if let (Some(declined), Some(allowance)) = (block.statistics_declined, allowance)
            && allowance <= declined
        {
            return None;
        }
        let Some(held) = block.statistics.as_deref() else {
            return Some(self.gathering(requested));
        };
        let stated_bound = self.min_rows.is_some() || self.max_rows.is_some();
        let resized = match self.group_size {
            Some(size) => size.get() != held.group_size,
            None => stated_bound && self.sizing() != held.sizing,
        };
        let unmet_max = !resized
            && held.group_size == self.group_size()
            && self.max_rows().is_some_and(|max_rows| held.breaks_max_rows(max_rows));
        let missing = requested.iter().enumerate().any(|(i, &wanted)| {
            wanted
                && held.columns.get(i).and_then(Option::as_ref).is_none_or(|column| {
                    column.bounds.is_none() && bounded.get(i).copied().unwrap_or(false)
                })
        });
        if !resized && !unmet_max && !missing {
            return None;
        }
        let columns = requested
            .iter()
            .enumerate()
            .map(|(i, &wanted)| wanted || held.columns.get(i).is_some_and(Option::is_some))
            .collect();
        if resized {
            return Some(self.gathering(columns));
        }
        if unmet_max {
            let max_rows = self.max_rows().expect("`unmet_max` read it");
            return Some(StatisticsBackfill {
                group_size: held.predicted_group_size(max_rows),
                ..self.gathering(columns)
            });
        }
        Some(StatisticsBackfill {
            columns,
            group_size: held.group_size,
            group_cap: None,
            min_rows: None,
            max_rows: None,
            sizing: held.sizing,
        })
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
    /// The group size gathered at: for a block re-read from its first row,
    /// the request's stated size, else [`ROW_GROUP_DEFAULT_SIZE_BYTES`]; for
    /// a block whose groups break a stated maximum, the finer size
    /// [`BlockStatistics::predicted_group_size`] predicts; and for one lacking
    /// only a column, the size it already held.
    pub group_size: u64,
    /// The most groups the block may hold, adjacent ones merging pairwise
    /// past it — [`StatisticsRequest::group_cap`] for a block re-read from its
    /// first row — and `None` for a size gathered exactly: a stated one, or
    /// the one the block already held.
    pub group_cap: Option<usize>,
    /// The fewest rows the finished block's median group may hold, its groups
    /// merging pairwise short of it — [`StatisticsRequest::min_rows`] for a
    /// block re-read from its first row — and `None` for a size gathered
    /// exactly.
    pub min_rows: Option<u64>,
    /// The most rows the finished block's 90th-percentile group may hold, no
    /// merge passing it — [`StatisticsRequest::max_rows`] for a block re-read
    /// from its first row — and `None` for a size gathered exactly.
    pub max_rows: Option<u64>,
    /// What the gathered block records it was sized under: the request's for
    /// a block re-read from its first row, the block's own record for one
    /// re-read at the size it held.
    pub sizing: GroupSizing,
}

/// How much a mapping pass records of a table or a column
/// (`docs/design/decisions.md`, "D35").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatisticsLevel {
    /// A block's location and row count, and nothing drawn from its rows: no
    /// census, no statistics, no field split. A query of such a table maps
    /// its blocks again for itself ([`crate::stream::table_stream`]), and a
    /// replay over a caller's map refuses one.
    Metadata,
    /// The census, for a table any of whose columns is at this level, and
    /// statistics, for each column that is — the default.
    #[default]
    Data,
}

/// Each table's and column's [`StatisticsLevel`]: a default for every one,
/// then overrides, **the most specific entry naming a column deciding its
/// level** — a column entry, then a qualified table, then a bare one, then
/// the default; among equally specific entries naming one column, the last.
/// The type's `Default` is the data level everywhere
/// (`docs/design/decisions.md`, "D77").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatisticsSelection {
    /// The level of every table and column no override names.
    pub default: StatisticsLevel,
    /// Tables and columns at a level of their own.
    pub overrides: Vec<(StatisticsTarget, StatisticsLevel)>,
}

impl StatisticsSelection {
    /// Every table and column at the data level.
    pub const DATA: Self = Self { default: StatisticsLevel::Data, overrides: Vec::new() };

    /// Every table at the metadata level.
    pub const METADATA: Self = Self { default: StatisticsLevel::Metadata, overrides: Vec::new() };

    /// The level of `column` of `header`'s table, or with `None` of the table
    /// itself, which no column entry reaches.
    pub fn level(&self, header: &CopyHeader, column: Option<&str>) -> StatisticsLevel {
        let mut level = (0, self.default);
        for (target, stated) in &self.overrides {
            let rank = match target {
                StatisticsTarget::Column { table, column: named }
                    if column == Some(named.as_str()) && header.matches(table) =>
                {
                    3 + u8::from(table.contains('.'))
                }
                StatisticsTarget::Table(table) if header.matches(table) => {
                    1 + u8::from(table.contains('.'))
                }
                _ => continue,
            };
            if rank >= level.0 {
                level = (rank, *stated);
            }
        }
        level.1
    }
}

/// What one override names. A table is named as [`CopyHeader::matches`]
/// reads it, qualified or bare.
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
/// what it would have had it been handed every row itself — bar a decline,
/// which a piece's charge alongside its neighbours' can reach where one
/// observer's would not (`docs/design/decisions.md`, "D85").
pub(crate) trait BlockObserver: Send {
    /// One row, `offset` being where its first byte sits relative to the
    /// block's first data byte, and `raw` the still-escaped line without its
    /// terminator.
    fn observe_row(&mut self, offset: u64, raw: &[u8]);

    /// The block's statistics, or [`BlockGathered::Declined`] where the
    /// allowance could not hold them before this call; the close it makes is
    /// kept even past the allowance (`docs/design/decisions.md`, "D85"). `end`
    /// is the terminator line's offset relative to the block's first data byte
    /// — where the last row's line ends.
    fn finish(self: Box<Self>, end: u64) -> BlockGathered;

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

/// What one block's observer answers: its statistics, or the decline that
/// stands in their place (`docs/design/decisions.md`, "D85").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockGathered {
    /// What the block gathered.
    Gathered(BlockStatistics),
    /// **The allowance could not hold this block's statistics**, so it dropped
    /// what it had gathered and what was in flight and gathered no more. The
    /// scan went on: nothing is OOM-killed for want of statistics, and raising
    /// memory is what fits a wide table. Granularity never depends on memory;
    /// whether a block gathers does (`docs/design/decisions.md`, "D85").
    Declined {
        /// The [`crate::scan::ScanOptions::statistics_allowance_bytes`] it declined
        /// under, which the map records
        /// ([`crate::index::CopyBlock::statistics_declined`]) so that a
        /// back-fill retries it only under a larger one.
        allowance: u64,
    },
}

impl BlockGathered {
    /// What the block gathered, and `None` where it declined.
    pub fn gathered(self) -> Option<BlockStatistics> {
        match self {
            Self::Gathered(statistics) => Some(statistics),
            Self::Declined { .. } => None,
        }
    }
}

/// One block's statistics, held by [`crate::index::CopyBlock::statistics`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockStatistics {
    /// `N`, the bytes of block data each group covers.
    pub group_size: u64,
    /// The request `group_size` was chosen under, which a back-fill compares
    /// a later request's against (`docs/design/decisions.md`, "D34").
    pub sizing: GroupSizing,
    /// Every group from the first to the one the last row starts in, in order.
    pub groups: Vec<RowGroup>,
    /// One entry per column of the block's header, `None` for a column the
    /// request did not track.
    pub columns: Vec<Option<ColumnStatistics>>,
}

/// How a block's group size was chosen: what [`BlockStatistics::sizing`]
/// records, a size no longer saying which request chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupSizing {
    /// A stated group size, gathered exactly.
    Stated,
    /// An unstated size: [`ROW_GROUP_DEFAULT_SIZE_BYTES`], or the finer size a
    /// re-read for a broken maximum predicts, merged pairwise
    /// past [`BLOCK_MAX_ROW_GROUPS`] groups and, once the block is finished,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnStatistics {
    /// The declared type text the statistics were computed under, as the
    /// preamble recorded it; `None` where the column was not declared.
    pub declared_type: Option<String>,
    /// The column's `COLLATE` clause, verbatim; `None` for no clause.
    pub collation: Option<String>,
    /// NULLs per group.
    pub null_counts: Vec<u64>,
    /// Present for a scalar column, in the register's order where that is
    /// exact and DataFusion semantics' otherwise — as its text for a column
    /// nothing declared ([`crate::ResolvedSchema::bounds_kinds`]).
    pub bounds: Option<ColumnBounds>,
    /// A second set in DataFusion semantics' order, present only where the
    /// register's order is exact and DataFusion's is another
    /// (`docs/design/decisions.md`, "D79").
    pub datafusion_bounds: Option<ColumnBounds>,
    /// Present for a column its comparison equates exactly.
    pub dictionary: Option<ColumnDictionary>,
    /// Per group, its values the column's Arrow type cannot hold, in each tier
    /// (`docs/design/decisions.md`, "D96") — what a reading taking them as
    /// NULL adds to [`Self::null_counts`] ([`StatisticsView`]). `None` where
    /// no group of the block holds one.
    pub unrepresentable: Option<Vec<Unrepresentable>>,
    /// Per group, the sum of its non-NULL values the type holds as integers —
    /// a decimal's unscaled — wrapped at 128 bits, which reduces exactly to
    /// any narrower wrapping sum; a `NaN` is left out, as a NULL is, and
    /// counted in [`Self::unrepresentable`]. Kept for a column the typed read
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
    /// [`crate::SchemaMode::Strings`] sizes every column by it.
    pub value_bytes: Vec<u64>,
}

/// Which of a column's stored sets of bounds and row order a statistic is read
/// from ([`crate::ComparisonPlan::bounds_kinds`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundsSet {
    /// [`ColumnStatistics::bounds`]: the register's order where it is
    /// exact, DataFusion semantics' otherwise.
    Primary,
    /// [`ColumnStatistics::datafusion_bounds`]: DataFusion semantics' order,
    /// kept beside an exact register order that is not it.
    DataFusion,
}

/// Which values a reading of a column's statistics takes as its values, and
/// which as NULL — a column's type being unable to hold some that PostgreSQL
/// accepts (`docs/design/decisions.md`, "D96", "D97"). A query's is
/// [`crate::QueryOptions::statistics_view`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatisticsView {
    /// Every value, in PostgreSQL's order, a special value ranked
    /// (`docs/design/decisions.md`, "D56") — the reading of a filter that
    /// compares the text and holds no value as NULL.
    Every,
    /// The values Arrow's format spec lets the column's type hold, each other
    /// one read as NULL.
    Representable,
    /// The values the engine displays as well: a `date` or timestamp past
    /// [`crate::calendar_end`] read as NULL too.
    Displayable,
}

/// Bounds per group and the block's row order, for one column, **over the
/// values its Arrow type holds**, and beside them the views that differ where
/// a group holds one it cannot ([`StatisticsView`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnBounds {
    /// The order of the non-NULL values the type holds over the whole block,
    /// row by row.
    pub sortedness: Sortedness,
    /// Per group, over the values the type holds; `None` where the group
    /// holds no such value or one the bounds could not cover.
    pub groups: Vec<Option<Bounds>>,
    /// Over every value, where some group holds one past Arrow's format spec:
    /// [`StatisticsView::Every`]'s, listed only for a group that does.
    pub every: Option<BoundsView>,
    /// Over the values within the engine's calendar as well, where some group
    /// holds one past it: [`StatisticsView::Displayable`]'s, listed only for a
    /// group that does.
    pub displayable: Option<BoundsView>,
}

/// One [`StatisticsView`]'s bounds and row order where it is not
/// [`ColumnBounds`]' own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundsView {
    /// The order of the non-NULL values the view takes, over the whole block.
    pub sortedness: Sortedness,
    /// Per group, `None` for one holding no value the view reads otherwise
    /// than [`ColumnBounds::groups`] does, which it is read from, and for one
    /// whose bounds could not cover a value it takes.
    pub groups: Vec<Option<Bounds>>,
}

/// A lower and an upper bound on one group's non-NULL values, as unescaped
/// field text no longer than [`DICTIONARY_ENTRY_MAX_BYTES`] — a `character` value's
/// without the trailing blanks its comparison ignores.
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
/// Equal neighbours are in either order; a column with at most one distinct
/// value, every one placed, is `Ascending`. A value gathering cannot place against its neighbour —
/// one that does not key, a keyed one past [`DICTIONARY_ENTRY_MAX_BYTES`], or a bytewise
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
    /// [`DICTIONARY_MAX_ENTRIES`] distinct texts, a text longer than
    /// [`DICTIONARY_ENTRY_MAX_BYTES`], or a field that is not text.
    pub groups: Vec<Option<Vec<u32>>>,
}

/// Bytes of heap a `Vec` holds: its capacity, not its length, since no
/// gathering vector is shrunk.
pub(crate) fn vec_heap<T>(v: &Vec<T>) -> u64 {
    (v.capacity() * size_of::<T>()) as u64
}

/// Bytes of heap a `String` holds.
pub(crate) fn text_heap(text: &String) -> u64 {
    text.capacity() as u64
}

/// The `rank`-th smallest of `rows`, counting from zero.
fn nth_smallest(rows: &[u64], rank: usize) -> u64 {
    let mut sorted = rows.to_vec();
    *sorted.select_nth_unstable(rank).1
}

/// **The group a density minimum is read at**: the upper middle one, the
/// `⌊G/2⌋+1`-th smallest of `G` groups' rows, so that at most half the groups
/// fall short of the minimum. Reading the upper middle group rather than the
/// nearest-rank `⌈G/2⌉`-th is what makes the predicate monotone in size
/// (`gather::density_merges`). Panics on no groups.
pub(crate) fn min_rows_group(rows: &[u64]) -> u64 {
    nth_smallest(rows, rows.len() / 2)
}

/// **The group a density maximum is read at**: the 90th-percentile one by
/// nearest rank, the `⌈9G/10⌉`-th smallest of `G` groups' rows, so that at most
/// a tenth of the groups hold more than the maximum. Near enough a bound for a
/// person who asked for one, without one dense stretch multiplying a whole
/// block's groups. Panics on no groups.
pub(crate) fn max_rows_group(rows: &[u64]) -> u64 {
    nth_smallest(rows, (9 * rows.len()).div_ceil(10) - 1)
}

impl BlockStatistics {
    /// Whether this block's groups break `max_rows`: its 90th-percentile group
    /// holds more rows than that. A block with no group breaks nothing.
    pub(crate) fn breaks_max_rows(&self, max_rows: u64) -> bool {
        let rows: Vec<u64> = self.groups.iter().map(|group| group.rows).collect();
        !rows.is_empty() && max_rows_group(&rows) > max_rows
    }

    /// **The group size a re-read of this block should gather at to meet
    /// `max_rows`**: halved, from the size it holds, until the rows its
    /// 90th-percentile group would then hold are within the maximum — a
    /// prediction, since halving a group does not halve every group's rows,
    /// and one the re-read is free to miss where a block's rows cluster.
    /// Asked only of a block [`Self::breaks_max_rows`] answered for, so it
    /// holds at least one group.
    pub(crate) fn predicted_group_size(&self, max_rows: u64) -> u64 {
        let rows: Vec<u64> = self.groups.iter().map(|group| group.rows).collect();
        let (mut size, mut densest) = (self.group_size, max_rows_group(&rows));
        while densest > max_rows && size > 1 {
            (size, densest) = (size / 2, densest.div_ceil(2));
        }
        size
    }

    /// The heap one block's statistics hold, the allocation of the `Arc` that
    /// shares them included: what [`StatisticsAccount`] charges a retained or a
    /// loaded block. Each term is the size the allocator was asked for — a
    /// vector's capacity, not its length — which
    /// `tests/statistics_heap.rs` holds to what freeing a block gives back.
    pub fn heap_bytes(&self) -> u64 {
        let shared = size_of::<(usize, usize, Self)>() as u64;
        shared
            + vec_heap(&self.groups)
            + vec_heap(&self.columns)
            + self.columns.iter().flatten().map(ColumnStatistics::heap_bytes).sum::<u64>()
    }
}

/// Under the instrument, a block's statistics are freed inside a statistics
/// scope wherever they are dropped — a cache decoded only for its envelope
/// included — so what the decode attributed is given back the same way
/// (`crate::instrument`). Only the `Arc`'s own allocation is freed after.
#[cfg(feature = "introspect")]
impl Drop for BlockStatistics {
    fn drop(&mut self) {
        let _attributed = instrument::StatisticsScope::enter();
        drop(std::mem::take(&mut self.columns));
        drop(std::mem::take(&mut self.groups));
    }
}

impl StatisticsView {
    /// Whether this view takes a value past `tier` — `None` for one the type
    /// holds — as NULL.
    pub fn reads_as_null(self, tier: Option<UnrepresentableTier>) -> bool {
        match (self, tier) {
            (_, None) | (StatisticsView::Every, _) => false,
            (StatisticsView::Representable, Some(tier)) => tier == UnrepresentableTier::Format,
            (StatisticsView::Displayable, Some(_)) => true,
        }
    }
}

impl ColumnStatistics {
    /// The stored set of bounds `set` names, if it was kept.
    pub fn bounds_in(&self, set: BoundsSet) -> Option<&ColumnBounds> {
        match set {
            BoundsSet::Primary => self.bounds.as_ref(),
            BoundsSet::DataFusion => self.datafusion_bounds.as_ref(),
        }
    }

    /// The bounds group `group` of the stored set `set` holds under `view`,
    /// where it holds any: a group holding no value the view reads otherwise
    /// is read from [`ColumnBounds::groups`], and one holding such a value
    /// from the view's own list — none where the view was not kept.
    pub fn group_bounds(
        &self,
        set: BoundsSet,
        view: StatisticsView,
        group: usize,
    ) -> Option<&Bounds> {
        let bounds = self.bounds_in(set)?;
        let count = self.unrepresentable_in(group);
        let groups = match view {
            StatisticsView::Every if count.format > 0 => &bounds.every.as_ref()?.groups,
            StatisticsView::Displayable if count.engine > 0 => &bounds.displayable.as_ref()?.groups,
            _ => &bounds.groups,
        };
        groups.get(group)?.as_ref()
    }

    /// The stored set `set`'s row order under `view`.
    pub fn sortedness(&self, set: BoundsSet, view: StatisticsView) -> Option<Sortedness> {
        let bounds = self.bounds_in(set)?;
        let differing = match view {
            StatisticsView::Every => bounds.every.as_ref(),
            StatisticsView::Displayable => bounds.displayable.as_ref(),
            StatisticsView::Representable => None,
        };
        Some(differing.map_or(bounds.sortedness, |view| view.sortedness))
    }

    /// Group `group`'s NULLs under `view`: its [`Self::null_counts`], and each
    /// value the view reads as NULL.
    pub fn null_count(&self, group: usize, view: StatisticsView) -> Option<u64> {
        let nulls = *self.null_counts.get(group)?;
        let count = self.unrepresentable_in(group);
        Some(match view {
            StatisticsView::Every => nulls,
            StatisticsView::Representable => nulls + count.format,
            StatisticsView::Displayable => nulls + count.format + count.engine,
        })
    }

    /// Group `group`'s count, zero where the block holds no such value.
    fn unrepresentable_in(&self, group: usize) -> Unrepresentable {
        self.unrepresentable
            .as_ref()
            .and_then(|counts| counts.get(group))
            .copied()
            .unwrap_or_default()
    }

    fn heap_bytes(&self) -> u64 {
        let named = self.declared_type.iter().chain(&self.collation).map(text_heap).sum::<u64>();
        let listed = |groups: &Vec<Option<Bounds>>| {
            vec_heap(groups)
                + groups
                    .iter()
                    .flatten()
                    .map(|b| text_heap(&b.min) + text_heap(&b.max))
                    .sum::<u64>()
        };
        let bounds = [&self.bounds, &self.datafusion_bounds].into_iter().flatten().map(|bounds| {
            let views = [&bounds.every, &bounds.displayable].into_iter().flatten();
            listed(&bounds.groups) + views.map(|view| listed(&view.groups)).sum::<u64>()
        });
        let bounds = bounds.sum::<u64>();
        let dictionary = self.dictionary.as_ref().map_or(0, |dictionary| {
            vec_heap(&dictionary.entries)
                + dictionary.entries.iter().map(text_heap).sum::<u64>()
                + vec_heap(&dictionary.groups)
                + dictionary.groups.iter().flatten().map(vec_heap).sum::<u64>()
        });
        let sums = self.sums.as_ref().map_or(0, vec_heap);
        let value_bytes = vec_heap(&self.value_bytes);
        let unrepresentable = self.unrepresentable.as_ref().map_or(0, vec_heap);
        named
            + vec_heap(&self.null_counts)
            + bounds
            + dictionary
            + sums
            + value_bytes
            + unrepresentable
    }

    /// The summed [`Self::value_bytes`] of the groups `keep` answers `true`
    /// for, of a block holding `groups` groups — `None` where this column
    /// did not count one per group.
    pub(crate) fn value_bytes_where(
        &self,
        groups: usize,
        keep: impl Fn(usize) -> bool,
    ) -> Option<u64> {
        let counted = Some(&self.value_bytes).filter(|counted| counted.len() == groups)?;
        Some(counted.iter().enumerate().filter(|&(g, _)| keep(g)).map(|(_, bytes)| bytes).sum())
    }
}

/// [`CopyBlock::statistics`] as the cache decodes it, with what the decode
/// allocates attributed to statistics where the instrument is built in
/// (`crate::instrument`) — and a plain decode where it is not.
pub(crate) fn deserialize_block_statistics<'de, D>(
    deserializer: D,
) -> Result<Option<Arc<BlockStatistics>>, D::Error>
where
    D: Deserializer<'de>,
{
    let _attributed = instrument::StatisticsScope::enter();
    Option::<Arc<BlockStatistics>>::deserialize(deserializer)
}

/// The growth an observer holds uncharged before it updates the pass's
/// [`StatisticsAccount`], which it does, once past it, before it observes
/// another row — so one wide row can carry it further. A
/// judgement setting only how often a wide observer updates.
pub(crate) const STATISTICS_ACCOUNT_CHARGE_STEP: u64 = 64 << 10;

/// Every statistic a mapping pass holds alive, in bytes of heap, by term
/// (`docs/design/decisions.md`, "D81").
///
/// **An account, not a bound**: each term is the sizes the allocator was asked
/// for, summed as the structures change — a finished block once, as it is
/// retained or loaded; an observer still gathering whenever its uncharged
/// growth passes [`STATISTICS_ACCOUNT_CHARGE_STEP`], at every column it closes a group on and
/// through every piece it folds in; and a vector or an interning map ahead of
/// the allocation it grows into. Shared by every observer of one pass, the
/// leader's pieces on the blocking pool included, so it is one lock over its
/// terms: an update, its peak and the instrument's check are read whole, and
/// no reader sees another thread's update half applied.
///
/// **What it does not see** is bounded by an observer rather than by the dump:
/// each open observer's growth until it passes [`STATISTICS_ACCOUNT_CHARGE_STEP`], a row's own
/// decode scratch while a column observes it, a merge's scratch for the pair
/// of groups it is merging, a finished block's rows per group while its size is
/// chosen, what `finish` builds beside what it replaces — the block's column
/// list, and a clipped group's bounds — and the observer's own allocation.
///
/// **It is also the bound a decline reads** (`docs/design/decisions.md`,
/// "D85"): where [`Self::allowance`] is stated and the terms pass it, the
/// observer whose update saw that declines, bar the close `finish` makes, so
/// what the account sums is what the margin is left against.
#[derive(Debug, Default)]
pub(crate) struct StatisticsAccount {
    state: Mutex<AccountState>,
    /// The bytes every statistic alive may hold between them
    /// ([`crate::scan::ScanOptions::statistics_allowance_bytes`]); `None` bounds
    /// nothing and declines nothing.
    allowance: Option<u64>,
}

#[derive(Debug, Default)]
struct AccountState {
    terms: [u64; TERMS],
    term_peaks: [u64; TERMS],
    peak: u64,
    /// What the terms carry for allocations charged ahead and not yet made
    /// ([`Charge::ahead`]).
    announced: u64,
    /// Every [`Charge`] alive: the observers whose uncharged growth the
    /// account may be short of.
    observers: u64,
}

const TERMS: usize = 5;

/// One of [`StatisticsTerms`]' fields, as [`StatisticsAccount`] indexes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Term {
    Retained,
    Loaded,
    Gathering,
    Pieces,
    Interned,
}

impl StatisticsAccount {
    /// An account bounded by `allowance` — `None` bounding nothing, which is
    /// [`Self::default`] and every caller that states no allowance.
    pub(crate) fn bounded_by(allowance: Option<u64>) -> Self {
        Self { state: Mutex::default(), allowance }
    }

    /// The bytes every statistic alive may hold between them, where one was
    /// stated.
    pub(crate) fn allowance(&self) -> Option<u64> {
        self.allowance
    }

    /// Apply every change in `changes` as one update: the peak and the
    /// instrument's check read the account once all of them are in, so a
    /// charge moving from one term to another never reads as both or neither.
    ///
    /// Answers whether the terms now stand **over** [`Self::allowance`], read
    /// inside the same lock as the update that moved them.
    pub(crate) fn apply(&self, changes: &[(Term, i64)]) -> bool {
        self.update(changes, 0, 0)
    }

    /// [`Self::apply`], with `announced` moving what the terms carry for
    /// allocations not yet made and `observers` the count of charges alive,
    /// in the same update.
    fn update(&self, changes: &[(Term, i64)], announced: i64, observers: i64) -> bool {
        let mut state = self.lock();
        for &(term, delta) in changes {
            let term = term as usize;
            state.terms[term] = state.terms[term].wrapping_add(delta as u64);
            state.term_peaks[term] = state.term_peaks[term].max(state.terms[term]);
        }
        state.announced = state.announced.wrapping_add(announced as u64);
        // An observer closing is still open for its own update: what it has
        // not freed yet is freed after.
        let open = state.observers.max(state.observers.wrapping_add(observers as u64));
        state.observers = state.observers.wrapping_add(observers as u64);
        let total = state.terms.iter().sum::<u64>();
        state.peak = state.peak.max(total);
        let tolerance = open * STATISTICS_ACCOUNT_CHARGE_STEP;
        instrument::statistics_account_updated(total, state.announced, tolerance);
        self.allowance.is_some_and(|allowance| total > allowance)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AccountState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the account holds now, and the most it has held.
    pub(crate) fn held(&self) -> StatisticsHeld {
        let state = self.lock();
        let read = |values: &[u64; TERMS]| {
            let [retained, loaded, gathering, pieces, interned] = *values;
            StatisticsTerms { retained, loaded, gathering, pieces, interned }
        };
        StatisticsHeld {
            now: read(&state.terms),
            term_peaks: read(&state.term_peaks),
            peak: state.peak,
        }
    }
}

/// An in-flight observer's share of a [`StatisticsAccount`], charged to one
/// term and to [`Term::Interned`], and released when it is dropped.
///
/// **Declared last in whatever holds it**, so the release follows the drop of
/// everything it charged for: an account released ahead of its memory reads
/// short of the heap for as long as the drop takes.
pub(crate) struct Charge {
    account: Arc<StatisticsAccount>,
    term: Term,
    structure: u64,
    interned: u64,
}

impl Charge {
    pub(crate) fn new(account: Arc<StatisticsAccount>, term: Term) -> Self {
        account.update(&[], 0, 1);
        Self { account, term, structure: 0, interned: 0 }
    }

    /// The account this charge is part of, for a piece of the same pass.
    pub(crate) fn account(&self) -> &Arc<StatisticsAccount> {
        &self.account
    }

    /// What this charge holds now, as its structure and its interned bytes.
    pub(crate) fn charged(&self) -> (u64, u64) {
        (self.structure, self.interned)
    }

    /// The allowance this charge's account bounds every statistic alive by,
    /// where one was stated — what a declining observer records.
    pub(crate) fn allowance(&self) -> Option<u64> {
        self.account.allowance()
    }

    /// Charge `structure` bytes to this charge's term and `interned` to
    /// [`Term::Interned`], in place of what was charged before, and answer
    /// whether the account now stands over its allowance
    /// ([`StatisticsAccount::apply`]).
    pub(crate) fn set(&mut self, structure: u64, interned: u64) -> bool {
        let changes = [
            (self.term, structure as i64 - self.structure as i64),
            (Term::Interned, interned as i64 - self.interned as i64),
        ];
        (self.structure, self.interned) = (structure, interned);
        self.account.apply(&changes)
    }

    /// **Charge `grows` — structure and interned bytes — ahead of the
    /// allocation `allocate` makes, and release `frees` once it has freed
    /// them**: a vector or a table grown while the smaller one is live, or
    /// realloc'd, is charged before any other update can read the heap
    /// holding it. Until the second update, the instrument's check reads the
    /// allocation as made where it judges the account short and as not yet
    /// made where it judges it over (`crate::instrument`).
    pub(crate) fn ahead<R>(
        &mut self,
        grows: (u64, u64),
        frees: (u64, u64),
        allocate: impl FnOnce() -> R,
    ) -> R {
        self.ahead_freeing(grows, || (allocate(), frees))
    }

    /// [`Self::ahead`], for work that learns only as it runs what it frees:
    /// `allocate` answers it, and it is released in the same update as the
    /// allocation is judged made, so the account never reads freed bytes as
    /// held.
    pub(crate) fn ahead_freeing<R>(
        &mut self,
        grows: (u64, u64),
        allocate: impl FnOnce() -> (R, (u64, u64)),
    ) -> R {
        let announced = (grows.0 + grows.1) as i64;
        self.structure += grows.0;
        self.interned += grows.1;
        let changes = [(self.term, grows.0 as i64), (Term::Interned, grows.1 as i64)];
        self.account.update(&changes, announced, 0);
        let (made, frees) = allocate();
        self.structure -= frees.0;
        self.interned -= frees.1;
        let changes = [(self.term, -(frees.0 as i64)), (Term::Interned, -(frees.1 as i64))];
        self.account.update(&changes, -announced, 0);
        made
    }

    /// Take `other`'s charge onto this one's term in one update, leaving
    /// `other` charging nothing: a piece whose structures its block is about
    /// to hold.
    pub(crate) fn take_over(&mut self, other: &mut Charge) {
        let changes =
            [(other.term, -(other.structure as i64)), (self.term, other.structure as i64)];
        self.structure += other.structure;
        self.interned += other.interned;
        (other.structure, other.interned) = (0, 0);
        self.account.apply(&changes);
    }

    /// Hand `held` — structure and interned bytes of what this charge carries
    /// — back to `other` in one update: what is left of a folded piece, which
    /// its own charge then releases as the piece is freed.
    pub(crate) fn hand_back(&mut self, other: &mut Charge, held: (u64, u64)) {
        let changes = [(self.term, -(held.0 as i64)), (other.term, held.0 as i64)];
        self.structure -= held.0;
        self.interned -= held.1;
        other.structure += held.0;
        other.interned += held.1;
        self.account.apply(&changes);
    }

    /// Release this charge and credit `retained` bytes to [`Term::Retained`]
    /// in one update: an observer that has become its block's statistics.
    pub(crate) fn retain(&mut self, retained: u64) {
        let changes = [
            (self.term, -(self.structure as i64)),
            (Term::Interned, -(self.interned as i64)),
            (Term::Retained, retained as i64),
        ];
        (self.structure, self.interned) = (0, 0);
        self.account.apply(&changes);
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        let changes =
            [(self.term, -(self.structure as i64)), (Term::Interned, -(self.interned as i64))];
        self.account.update(&changes, 0, -1);
    }
}

/// What a [`StatisticsAccount`] held, by term, in bytes of heap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatisticsTerms {
    /// Finished blocks this pass gathered, which the map holds until it is
    /// dropped.
    pub retained: u64,
    /// Blocks' statistics decoded from the cache the pass loaded, less any a
    /// back-fill has replaced.
    pub loaded: u64,
    /// Every block observer still gathering: its closed groups and its open
    /// ones, and a piece it is folding in.
    pub gathering: u64,
    /// Every observer a parallel window made for a piece and has not folded
    /// into its block — a piece past the block's terminator until it is
    /// dropped — charged as a block observer is.
    pub pieces: u64,
    /// Every dictionary's interning map while it gathers, block or piece: the
    /// second copy of each distinct text, and the table holding it.
    pub interned: u64,
}

impl StatisticsTerms {
    /// Every term summed.
    pub fn total(&self) -> u64 {
        self.retained + self.loaded + self.gathering + self.pieces + self.interned
    }
}

/// A [`StatisticsAccount`] read whole: what one mapping pass's statistics
/// held when it returned, and the most they held
/// ([`crate::stream::MapRun::statistics`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatisticsHeld {
    /// Each term when the account was read.
    pub now: StatisticsTerms,
    /// The largest each term reached, each at its own moment.
    pub term_peaks: StatisticsTerms,
    /// The largest the sum of the terms reached, read at an update.
    pub peak: u64,
}
