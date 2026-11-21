//! Per-row-group column statistics as a block holds them, what a field's
//! refusal leaves behind (`docs/design/decisions.md`, "D34", "D103"), the
//! views they are read through, and the contract over a block's rows that
//! answers both ([`BlockObserver`]); and what a read does with a refused field
//! ([`PostgresInvalidValues`]), the mode that record answers to. The shapes
//! are the cache's (`crate::persisted::statistics`).
//! What a mapping pass is asked to gather, and the account of what it holds,
//! are `pgdt_mapping::statistics`'.
//!
//! Model vocabulary only (`docs/design/decisions.md`, "D74"): column names, the
//! declared type text and `COLLATE` clause a column's statistics were computed
//! under, counts, and bounds as unescaped field text — what `pg_dump` wrote, a
//! `bytea`'s in its hex form whichever form it was written in, a
//! `character`'s without its trailing blanks, where the value fits
//! [`DICTIONARY_ENTRY_MAX_BYTES`]; where it does not, a bytewise-ordered
//! column's bound is a prefix or a successor of it ([`Bounds::min_exact`],
//! [`Bounds::max_exact`]), its group keeping none where no prefix has a
//! successor, and any other column's group keeps none.
//!
//! **A row group is a byte range** (`docs/design/decisions.md`, "D34"): group
//! `k` of a block is the rows whose first byte lies in `[k·N, (k+1)·N)` of the
//! block's data, `N` being [`BlockStatistics::group_size`]. A row longer than
//! `N` leaves groups in which no row starts, which are listed, empty.

use std::mem::size_of;

use crate::internal::ColumnBounds;
use crate::{
    BlockStatistics, Bounds, ColumnDictionary, ColumnRefusals, ColumnStatistics, FieldRefusal,
    IgnoredRefusals, Sortedness, Unrepresentable, UnrepresentableTier,
};
use pgdt_core::CopyHeader;
use pgdt_core::Error;

/// What a read does with a field its type's `*_in` refuses
/// ([`pgdt_codec::decode::Unread::Refused`]), stated for a parse as `pgdt_mapping::map_file`'s argument
/// and for a query on [`pgdt_query::QueryOptions`] (`docs/design/decisions.md`,
/// "D103").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PostgresInvalidValues {
    /// Refused as a restore refuses it: a parse fails at the first such field
    /// it decodes, or that its cache records an ignoring parse went past
    /// ([`pgdt_core::Error::FieldRefusedRecorded`]), and a query decoding one fails
    /// there.
    #[default]
    Default,
    /// Read as the decoders read it, with no contract: a parse goes on past
    /// one, its row group then keeping no statistic of its column a read could
    /// contradict and its block recording it
    /// ([`crate::BlockRecord::ignored_refusals`]), and a query reads a float
    /// past its type's range as `pgdt_codec::internal::float_field` reads it, a
    /// `varchar(n)` or `char(n)` value past its length as written and a NULL in
    /// a `NOT NULL` column as NULL, and fails on every other such field. A
    /// filter literal is never opted out.
    Ignore,
    /// **A parse checks every field**, failing at the first its type's `*_in`
    /// refuses as [`Self::Default`] does, but over the fields `Default` leaves
    /// to a query: a column the request leaves at the metadata level, a nested
    /// value's elements, a value too long to key, and the rows of a block whose
    /// statistics declined. A block it checks whole records it
    /// ([`crate::BlockRecord::checked_in_full`]), and one the cache holds
    /// unchecked is re-read and checked, so a clean run has checked every field
    /// whichever runs built the cache. A query reads under it as under
    /// `Default`, and a query front end refuses it ([`Self::queryable`]).
    Strict,
}

impl PostgresInvalidValues {
    /// Every mode, in the order a front end lists them.
    pub const ALL: [Self; 3] = [Self::Default, Self::Ignore, Self::Strict];

    /// **This mode as a query front end may state it**: [`Self::Strict`] is
    /// refused, a parse's alone (`docs/design/decisions.md`, "D103"), in the
    /// one sentence every query front end gives.
    pub fn queryable(self) -> Result<Self, String> {
        match self {
            Self::Strict => Err(format!(
                "`{}` checks every field as a parse maps it, so it is a parse's mode alone; a \
                 query reads under `{}` or `{}`",
                Self::Strict,
                Self::Default,
                Self::Ignore
            )),
            mode => Ok(mode),
        }
    }
}

/// A mode's word, which every front end writes and reads it by.
impl std::fmt::Display for PostgresInvalidValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Default => "default",
            Self::Ignore => "ignore",
            Self::Strict => "strict",
        })
    }
}

/// A mode by its word ([`Display`](std::fmt::Display)'s), in any case.
impl std::str::FromStr for PostgresInvalidValues {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|mode| mode.to_string().eq_ignore_ascii_case(text)).ok_or_else(
            || {
                let words: Vec<String> = Self::ALL.iter().map(|mode| format!("`{mode}`")).collect();
                format!("`{text}` is not one of {}", words.join(", "))
            },
        )
    }
}

/// The longest text any stored bound or dictionary entry may be, in bytes.
pub const DICTIONARY_ENTRY_MAX_BYTES: usize = 256;

/// The most distinct texts one group's dictionary may hold; a group with more
/// has none on that column.
pub const DICTIONARY_MAX_ENTRIES: usize = 64;

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
        /// The statistics allowance it declined under
        /// (`pgdt_io::Arrangement::statistics_allowance`), which the map records
        /// ([`crate::BlockRecord::statistics_declined`]) so that a
        /// back-fill retries it only under a larger one.
        allowance: u64,
    },
    /// **A field the observer read is one PostgreSQL refuses** — its type's
    /// `*_in`, its column's `NOT NULL`, or a strict parse's check — so
    /// the pass reading the block fails there, as a restore under
    /// `ON_ERROR_STOP` fails the table's `COPY` (`roadmap.md`, "A literal is
    /// guaranteed in `*_out`'s form and never read past `*_in`'s"). The first
    /// in the block's row order: an observer stops at one, and a piece's is
    /// kept only where no earlier row's was.
    Refused(FieldRefusal),
    /// **The observer gathered nothing, only checking the block's fields** as
    /// a strict parse does ([`crate::PostgresInvalidValues::Strict`]), and
    /// found none refused: what a block whose columns the request tracks none
    /// of answers, and a re-read checking a block alone.
    Checked,
}

impl BlockGathered {
    /// What the block gathered, and `None` where it declined, refused or
    /// gathered nothing.
    pub fn gathered(self) -> Option<BlockStatistics> {
        match self {
            Self::Gathered(statistics) => Some(statistics),
            Self::Declined { .. } | Self::Refused(_) | Self::Checked => None,
        }
    }
}

/// What a crate above calls on [`FieldRefusal`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait FieldRefusalExt {
    fn into_error(self, header: &CopyHeader, data_offset: u64) -> Error;
}

impl FieldRefusalExt for FieldRefusal {
    /// The error a pass fails with, naming the table and the column off
    /// `header` and the row's line by its number and by its offset in the
    /// file, the block's data starting at `data_offset`.
    fn into_error(self, header: &CopyHeader, data_offset: u64) -> Error {
        let table = header.qualified_name();
        let column = header.columns.get(self.column).cloned().unwrap_or_default();
        let line_offset = data_offset + self.offset;
        match self.value {
            Some(value) => Error::FieldRefused {
                table,
                column,
                declared_type: self.declared_type,
                line: self.line,
                line_offset,
                value,
            },
            None => Error::NullRefused { table, column, line: Some(self.line), line_offset },
        }
    }
}

impl IgnoredRefusals {
    /// `held`'s columns, made where there were none, and where `column`'s
    /// entry is or would be inserted.
    fn slot(
        held: &mut Option<Self>,
        column: usize,
    ) -> (&mut Vec<ColumnRefusals>, Result<usize, usize>) {
        let columns = &mut held.get_or_insert_with(|| Self { columns: Vec::new() }).columns;
        let at = columns.binary_search_by_key(&column, |c| c.first.column);
        (columns, at)
    }

    /// Merge `other`'s, read over the same block from its first row as
    /// `held`'s was, either perhaps stopping at a decline: a column's first is
    /// `held`'s, and its count the larger, the read covering more rows having
    /// met at least as many; a column only one read gathered keeps that read's.
    pub fn merge(held: &mut Option<Self>, other: Option<Self>) {
        // deficiency: KD107 — `held`'s first is the block's only where both
        // reads met it. A read in pieces that declined can have gone past its
        // first in the declining piece's unread tail and recorded a later one
        // from a piece made before the decline, so a refusing parse over the
        // cache names a later line than a restore fails at.
        for other in other.into_iter().flat_map(|other| other.columns) {
            match Self::slot(held, other.first.column) {
                (columns, Ok(at)) => columns[at].count = columns[at].count.max(other.count),
                (columns, Err(at)) => columns.insert(at, other),
            }
        }
    }

    /// The first, in row and then column order, among the columns `tracked`
    /// marks, positionally to the block's header: the field a parse keying
    /// those columns meets first.
    pub fn first_among(&self, tracked: &[bool]) -> Option<&FieldRefusal> {
        self.columns
            .iter()
            .map(|c| &c.first)
            .filter(|first| tracked.get(first.column).copied().unwrap_or(false))
            .min_by_key(|first| (first.line, first.column))
    }
}

/// What a crate above calls on [`IgnoredRefusals`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait IgnoredRefusalsExt: Sized {
    fn add(held: &mut Option<Self>, refusal: FieldRefusal);
    fn fold(held: &mut Option<Self>, later: Option<Self>);
    fn error_for(
        &self,
        tracked: &[bool],
        header: &CopyHeader,
        data_offset: u64,
        cache: &std::path::Path,
    ) -> Option<Error>;
}

impl IgnoredRefusalsExt for IgnoredRefusals {
    /// Count `refusal`, which follows every one already counted in its
    /// column, keeping it as the column's first where nothing was.
    fn add(held: &mut Option<Self>, refusal: FieldRefusal) {
        match Self::slot(held, refusal.column) {
            (columns, Ok(at)) => columns[at].count += 1,
            (columns, Err(at)) => columns.insert(at, ColumnRefusals { first: refusal, count: 1 }),
        }
    }

    /// Fold `later`'s, whose rows all follow those `held` counted.
    fn fold(held: &mut Option<Self>, later: Option<Self>) {
        for later in later.into_iter().flat_map(|later| later.columns) {
            match Self::slot(held, later.first.column) {
                (columns, Ok(at)) => columns[at].count += later.count,
                (columns, Err(at)) => columns.insert(at, later),
            }
        }
    }

    /// The error a parse under [`crate::PostgresInvalidValues::Default`]
    /// keying `tracked` fails with over a cache, at `cache`, holding this
    /// block's record — `None` where no tracked column holds one: the first
    /// among them ([`Self::first_among`]) worded as
    /// [`FieldRefusal::into_error`] words it, saying where it was read from.
    fn error_for(
        &self,
        tracked: &[bool],
        header: &CopyHeader,
        data_offset: u64,
        cache: &std::path::Path,
    ) -> Option<Error> {
        let first = self.first_among(tracked)?.clone();
        Some(Error::FieldRefusedRecorded {
            refused: Box::new(first.into_error(header, data_offset)),
            cache: cache.into(),
        })
    }
}

/// Receives every row of one `COPY` block from the mapping pass and answers
/// the block's statistics once it closes. Defined here and implemented in mapping,
/// the shape `docs/design/decisions.md`, "D74" gives a contract over a block's rows.
///
/// **A row is folded field by field, a run of each field's bytes at a time**,
/// as a reader streaming the row hands them over, with no field held but one
/// [`Self::holds`] names; a row held whole is that fold fed once
/// ([`Self::observe_row`]), so a held row and a streamed one are read alike
/// (`docs/design/decisions.md`, "D138").
///
/// **A block the leader splits is observed piece by piece**: each piece's rows
/// go to an observer [`Self::piece`] made, and the pieces are handed back in
/// file order to [`Self::absorb`], after which the block's observer answers
/// what it would have had it been handed every row itself — bar a decline,
/// which a piece's charge alongside its neighbours' can reach where one
/// observer's would not (`docs/design/decisions.md`, "D85").
pub trait BlockObserver: Send {
    /// Whether column `column`'s field is wanted whole: its reader then holds
    /// it and hands it over once, as its last bytes. Fixed for the block, so
    /// a field is never handed over partly held.
    fn holds(&self, column: usize) -> bool;

    /// A row begins, `offset` being where its first byte sits relative to
    /// the block's first data byte.
    fn begin_row(&mut self, offset: u64);

    /// The next `bytes` of field `column` of the row begun, still escaped, a
    /// delimiter never among them; `ends` where they are its last. Fields are
    /// handed over in order.
    fn fold(&mut self, column: usize, bytes: &[u8], ends: bool);

    /// The row ends, `fields` wide, its last field's end folded already.
    fn end_row(&mut self, fields: usize);

    /// One row held whole, `offset` placed as [`Self::begin_row`] places it
    /// and `raw` the still-escaped line without its terminator: the fold fed
    /// each field once.
    fn observe_row(&mut self, offset: u64, raw: &[u8]) {
        self.begin_row(offset);
        let mut fields = 0;
        for (column, field) in pgdt_core::split_fields(raw).enumerate() {
            self.fold(column, field, true);
            fields = column + 1;
        }
        self.end_row(fields);
    }

    /// **Close the block**, `end` being the terminator line's offset relative
    /// to the block's first data byte — where the last row's line ends — and
    /// answer what its record takes of this observer
    /// ([`BlockObserved`]). The close it makes is kept even past the
    /// allowance (`docs/design/decisions.md`, "D85").
    fn close(self: Box<Self>, end: u64) -> BlockObserved;

    /// An observer for a piece of this block: rows starting anywhere after the
    /// ones this observer has been handed, to be given back to
    /// [`Self::absorb`] rather than closed.
    fn piece(&self) -> Box<dyn BlockObserver>;

    /// Fold `later`, made by [`Self::piece`] and handed rows that all follow
    /// every row this observer has seen, into this one.
    fn absorb(&mut self, later: Box<dyn BlockObserver>);

    /// This observer as [`Any`](std::any::Any), which is how [`Self::absorb`]
    /// recovers the concrete piece it made.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;
}

/// What a block's observer answers as it closes ([`BlockObserver::close`]):
/// what the block's record takes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockObserved {
    /// The block's statistics, or [`BlockGathered::Declined`] where the
    /// allowance could not hold them.
    pub gathered: BlockGathered,
    /// The fields PostgreSQL refuses, by type or `NOT NULL`, that the
    /// observer keyed and went past, being told to ignore them
    /// ([`crate::PostgresInvalidValues::Ignore`]):
    /// [`crate::BlockRecord::ignored_refusals`].
    pub ignored_refusals: Option<IgnoredRefusals>,
    /// Whether the observer checked every field of every row it was handed
    /// as a strict parse does ([`crate::PostgresInvalidValues::Strict`]), so
    /// a block it closed unrefused is checked in full:
    /// [`crate::BlockRecord::checked_in_full`].
    pub checked_in_full: bool,
}

/// A [`ColumnStatistics`]' every field, as it stores them: what a gatherer
/// builds one from and a test takes one apart into
/// ([`ColumnStatisticsExt::from_parts`], [`ColumnStatisticsExt::into_parts`]).
/// The fields a view interprets are read here raw, which no reader past
/// `internal` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnStatisticsParts {
    /// [`ColumnStatistics::declared_type`].
    pub declared_type: Option<String>,
    /// [`ColumnStatistics::collation`].
    pub collation: Option<String>,
    /// NULLs per group, the values the dump writes as NULL alone.
    pub null_counts: Vec<u64>,
    /// Present for a scalar column, in the register's order where that is
    /// exact and DataFusion semantics' otherwise — as its text for a column
    /// nothing declared ([`pgdt_codec::ResolvedColumn::bounds_kinds`]):
    /// [`BoundsSet::Primary`].
    pub bounds: Option<ColumnBounds>,
    /// A second set in DataFusion semantics' order, present only where the
    /// register's order is exact and DataFusion's is another
    /// (`docs/design/decisions.md`, "D79"): [`BoundsSet::DataFusion`].
    pub datafusion_bounds: Option<ColumnBounds>,
    /// [`ColumnStatistics::dictionary`].
    pub dictionary: Option<ColumnDictionary>,
    /// Per group, its values the column's Arrow type cannot hold, in each tier
    /// (`docs/design/decisions.md`, "D96") — what a reading taking them as
    /// NULL adds to `null_counts` ([`StatisticsView`]). `None` where no group
    /// of the block holds one.
    pub unrepresentable: Option<Vec<Unrepresentable>>,
    /// [`ColumnStatistics::sums`].
    pub sums: Option<Vec<i128>>,
    /// [`ColumnStatistics::value_bytes`].
    pub value_bytes: Vec<u64>,
}

/// Which of a column's stored sets of bounds and row order a statistic is read
/// from ([`pgdt_codec::compare::ComparisonPlan::bounds_kinds`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundsSet {
    /// The register's order where it is exact, DataFusion semantics'
    /// otherwise.
    Primary,
    /// DataFusion semantics' order, kept beside an exact register order that
    /// is not it.
    DataFusion,
}

/// Which values a reading of a column's statistics takes as its values, and
/// which as NULL — a column's type being unable to hold some that PostgreSQL
/// accepts (`docs/design/decisions.md`, "D96", "D97"). A query's is
/// [`pgdt_query::QueryOptions::statistics_view`].
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

/// Bytes of heap a `Vec` holds: its capacity, not its length, since no
/// gathering vector is shrunk.
pub fn vec_heap<T>(v: &Vec<T>) -> u64 {
    (v.capacity() * size_of::<T>()) as u64
}

/// Bytes of heap a `String` holds.
pub fn text_heap(text: &String) -> u64 {
    text.capacity() as u64
}

impl BlockStatistics {
    /// The heap one block's statistics hold, the allocation of the `Arc` that
    /// shares them included: what `pgdt_mapping`'s `StatisticsAccount` charges a retained or a
    /// loaded block. Each term is the size the allocator was asked for — a
    /// vector's capacity, not its length — which
    /// `pgdt-mapping`'s `tests/statistics_heap.rs` holds to what freeing a block gives back.
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
/// (`pgdt_core::instrument`). Only the `Arc`'s own allocation is freed after.
#[cfg(feature = "introspect")]
impl Drop for BlockStatistics {
    fn drop(&mut self) {
        let _attributed = pgdt_core::internal::StatisticsScope::enter();
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
    fn bounds_in(&self, set: BoundsSet) -> Option<&ColumnBounds> {
        match set {
            BoundsSet::Primary => self.bounds.as_ref(),
            BoundsSet::DataFusion => self.datafusion_bounds.as_ref(),
        }
    }

    /// How many groups the stored set `set` lists, `None` where it was not
    /// kept — [`BlockStatistics::groups`]' length for a set gathered over
    /// the block.
    pub fn listed_groups(&self, set: BoundsSet) -> Option<usize> {
        self.bounds_in(set).map(|bounds| bounds.groups.len())
    }

    /// How many groups its NULLs are counted for — [`BlockStatistics::groups`]'
    /// length for a column gathered over the block.
    pub fn counted_groups(&self) -> usize {
        self.null_counts.len()
    }

    /// Whether some group holds a value past `tier`, which a view taking it
    /// as NULL reads as NULL ([`StatisticsView::reads_as_null`]) and which
    /// [`Self::sums`] leave out.
    pub fn holds_unrepresentable(&self, tier: UnrepresentableTier) -> bool {
        self.unrepresentable.iter().flatten().any(|count| match tier {
            UnrepresentableTier::Format => count.format() > 0,
            UnrepresentableTier::Engine => count.engine() > 0,
        })
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
            StatisticsView::Every if count.format() > 0 => &bounds.every.as_ref()?.groups,
            StatisticsView::Displayable if count.engine() > 0 => {
                &bounds.displayable.as_ref()?.groups
            }
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

    /// Group `group`'s NULLs under `view`: those the dump writes, and each
    /// value the view reads as NULL; `None` past [`Self::counted_groups`].
    pub fn null_count(&self, group: usize, view: StatisticsView) -> Option<u64> {
        let nulls = *self.null_counts.get(group)?;
        let count = self.unrepresentable_in(group);
        Some(match view {
            StatisticsView::Every => nulls,
            StatisticsView::Representable => nulls + count.format(),
            StatisticsView::Displayable => nulls + count.format() + count.engine(),
        })
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
    pub fn value_bytes_where(&self, groups: usize, keep: impl Fn(usize) -> bool) -> Option<u64> {
        let counted = Some(&self.value_bytes).filter(|counted| counted.len() == groups)?;
        Some(counted.iter().enumerate().filter(|&(g, _)| keep(g)).map(|(_, bytes)| bytes).sum())
    }
}

/// What a crate above calls on [`ColumnStatistics`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait ColumnStatisticsExt {
    fn from_parts(parts: ColumnStatisticsParts) -> Self;
    fn into_parts(self) -> ColumnStatisticsParts;
    fn unrepresentable_in(&self, group: usize) -> Unrepresentable;
}

impl ColumnStatisticsExt for ColumnStatistics {
    /// One built from its fields as it stores them.
    fn from_parts(parts: ColumnStatisticsParts) -> Self {
        let ColumnStatisticsParts {
            declared_type,
            collation,
            null_counts,
            bounds,
            datafusion_bounds,
            dictionary,
            unrepresentable,
            sums,
            value_bytes,
        } = parts;
        Self {
            declared_type,
            collation,
            null_counts,
            bounds,
            datafusion_bounds,
            dictionary,
            unrepresentable,
            sums,
            value_bytes,
        }
    }

    /// Its fields as it stores them, the ones a view interprets raw.
    fn into_parts(self) -> ColumnStatisticsParts {
        let Self {
            declared_type,
            collation,
            null_counts,
            bounds,
            datafusion_bounds,
            dictionary,
            unrepresentable,
            sums,
            value_bytes,
        } = self;
        ColumnStatisticsParts {
            declared_type,
            collation,
            null_counts,
            bounds,
            datafusion_bounds,
            dictionary,
            unrepresentable,
            sums,
            value_bytes,
        }
    }

    /// Group `group`'s count, zero where the block holds no such value.
    fn unrepresentable_in(&self, group: usize) -> Unrepresentable {
        self.unrepresentable
            .as_ref()
            .and_then(|counts| counts.get(group))
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
