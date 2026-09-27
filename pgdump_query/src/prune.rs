//! The pruning consumer: which row groups of a query's block its filter can
//! skip, read off the statistics a mapping pass stored
//! ([`crate::statistics::BlockStatistics`]) through the truth-set evaluator
//! ([`ResolvedExpr::truths`]).
//!
//! **A group is skipped only when no row of it can make the filter's root
//! `True`**, so a pruned replay returns exactly the rows the unpruned one does;
//! what it gives up is the error a value in a skipped group would have raised
//! (`docs/design/decisions.md`, "D54").
//!
//! **A block sorted on a column a filter bounds is also read no further than
//! its first row past the bound** ([`SortedStop`]), which settles inside a
//! group what the statistics can settle only between groups.
//!
//! **A dynamic filter is read against the same statistics while the replay
//! runs** ([`DynamicPruning`]): a group at a time as the replay reaches it,
//! rather than every group of the block once, since its state can move at
//! any boundary.

use std::ops::Range;
use std::sync::Arc;

use crate::copy::{RawRow, RowSplit};
use crate::gather::{declared_columns, stored_bounds_kinds};
use crate::index::CopyBlock;
use crate::pgtype::{CompareKind, bounds_set_keyed_by};
use crate::preamble::DumpMetadata;
use crate::predicate::{GroupStatistics, PredicateOp, ResolvedExpr, ResolvedTerm, Truth};
use crate::statistics::{BlockStatistics, ColumnBounds, ColumnStatistics, Sortedness};

/// What one block's statistics let a filter skip.
#[derive(Debug, Clone)]
pub(crate) struct BlockPruning {
    /// Every run of kept groups, in file order, as a replay segment's search
    /// bounds (`crate::stream::Segment`): a run's start finds its first row
    /// past the first LF at or after it, and it reads through the line ending
    /// at the first LF at or after its end. Group `k` of a block whose data
    /// starts at `D` is `[D + k·N − 1, D + (k+1)·N − 1)` in those terms — the
    /// rows whose first byte lies in `[D + k·N, D + (k+1)·N)` — so group 0
    /// starts on the header's own LF, and the last group ends at the block's
    /// end, taking its terminator with it.
    ///
    /// **Never empty**: a block every group of which is skipped keeps the
    /// empty run on its header's LF, which reads the header line and no row,
    /// so its schema and comparison notes resolve as an unpruned replay's do.
    pub(crate) kept: Vec<Range<u64>>,
    /// Groups the statistics list.
    pub(crate) groups: u64,
    pub(crate) skipped_groups: u64,
    /// The row bytes of the skipped groups ([`crate::statistics::RowGroup::bytes`]).
    pub(crate) skipped_bytes: u64,
    /// The rows of the kept groups ([`crate::statistics::RowGroup::rows`]):
    /// every row a replay of the block can emit, and more wherever a kept
    /// group's rows fail the filter or a [`SortedStop`] ends the read.
    pub(crate) kept_rows: u64,
    /// Per group the statistics list, whether the filter keeps it.
    pub(crate) kept_groups: Vec<bool>,
    /// Where the block's row order ends its reading early, if anywhere.
    pub(crate) stop: Option<SortedStop>,
}

/// The terms of a filter whose column a block's rows are sorted on in the
/// direction that term's bound closes: `<` and `<=` on an ascending column,
/// `>` and `>=` on a descending one (`docs/design/decisions.md`, "D75").
///
/// **The first row making one of them `False` is past every row the filter
/// keeps**: every non-NULL value after it is on the same side of the bound,
/// and a NULL makes an ordering term `Unknown`, so the term — required of
/// every kept row ([`ResolvedExpr::required_ordering_terms`]) — is `True` of
/// no later row. A replay of the block stops there, inside a group if need
/// be, and nothing past it is parsed, nor read past the chunk holding it.
///
/// *Not taken:* stopping a sub-stream's later segments of a stopped block —
/// pruning already skips a later run, and the rest is the read before a later
/// piece's first row.
#[derive(Debug, Clone)]
pub(crate) struct SortedStop {
    terms: Vec<ResolvedTerm>,
}

impl SortedStop {
    /// Whether `raw_row` is past the bound: some term is `False` of it. A
    /// static filter's stop is asked only of a row that filter rejected, a
    /// kept row having made every term `True`; a dynamic filter's, of every
    /// row of a group it is armed in ([`DynamicPruning::arm`]), that filter
    /// being evaluated nowhere else.
    pub(crate) fn passed(&self, raw_row: RawRow<'_>, split: &mut RowSplit) -> bool {
        self.terms.iter().any(|term| term.is_false(raw_row, split))
    }

    /// Whether some row of `group` could be past the bound, as its statistics
    /// say: none can where no term can be `False` of it.
    fn reachable(&self, group: &impl GroupStatistics) -> bool {
        self.terms.iter().any(|term| term.truths(group).contains(Truth::False))
    }
}

/// Which of `block`'s groups `filter` could keep a row of, and where its row
/// order stops a replay of it — or `None` where the block's statistics answer
/// nothing: it holds none, lists no group, holds statistics that do not fit
/// its extent or its column list, or records a `data_offset` of zero, which
/// the group arithmetic below subtracts one from.
///
/// **A column's bounds, row order and dictionary are believed only under the
/// declared type and collation they were gathered under**
/// (`docs/design/decisions.md`, "D78"), compared against
/// what `metadata` declares now; its NULL counts are read off the text and
/// believed regardless. What the comparison itself believes is settled when
/// the filter resolved ([`ResolvedExpr::truths`]), **and which stored set of
/// bounds a term reads is this block's**: the one gathering stored under the
/// kind the term compares by, gathering's kinds recomputed from the DDL the
/// believed column was gathered under (`docs/design/decisions.md`, "D79").
pub(crate) fn prune_block(
    block: &CopyBlock,
    filter: &ResolvedExpr,
    metadata: Option<&DumpMetadata>,
) -> Option<BlockPruning> {
    let statistics = block.statistics.as_deref()?;
    let last = statistics.groups.len().checked_sub(1)?;
    let (believed, kinds) = believed_columns(block, statistics, metadata)?;
    let view = Believed { statistics, believed: &believed, kinds: &kinds };

    let n = statistics.group_size;
    let mut pruning = BlockPruning {
        kept: Vec::new(),
        groups: statistics.groups.len() as u64,
        skipped_groups: 0,
        skipped_bytes: 0,
        kept_rows: 0,
        kept_groups: vec![false; statistics.groups.len()],
        stop: sorted_stop(filter, &view),
    };
    for (index, group) in statistics.groups.iter().enumerate() {
        if !view.keeps(filter, index) {
            pruning.skipped_groups += 1;
            pruning.skipped_bytes += group.bytes;
            continue;
        }
        pruning.kept_rows += group.rows;
        pruning.kept_groups[index] = true;
        let k = index as u64;
        let start = block.data_offset + k * n - 1;
        let end =
            if index == last { block.end_offset } else { block.data_offset + (k + 1) * n - 1 };
        match pruning.kept.last_mut() {
            Some(run) if run.end == start => run.end = end,
            _ => pruning.kept.push(start..end),
        }
    }
    if pruning.kept.is_empty() {
        let header_lf = block.data_offset - 1;
        pruning.kept.push(header_lf..header_lf);
    }
    Some(pruning)
}

/// Per header column, the kinds its stored sets of bounds are ordered by
/// (`crate::gather::stored_bounds_kinds`).
type StoredKinds = Vec<[Option<CompareKind>; 2]>;

/// Per column of `block`, whether `statistics` — its own — are believed
/// under the DDL `metadata` states now, and the kinds its stored sets of
/// bounds are ordered by ([`prune_block`]); or `None` where the statistics
/// answer nothing, not fitting the block's extent or column list, or the
/// block recording a `data_offset` of zero, which a group's search start
/// subtracts one from.
fn believed_columns(
    block: &CopyBlock,
    statistics: &BlockStatistics,
    metadata: Option<&DumpMetadata>,
) -> Option<(Vec<bool>, StoredKinds)> {
    let data = block.terminator_offset.saturating_sub(block.data_offset);
    // The last group is the one the last row starts in, so no index reaches
    // past the data; a block whose statistics say otherwise describes other
    // bytes, and is read whole.
    if statistics.group_size == 0
        || statistics.groups.is_empty()
        || statistics.groups.len() as u64 > data.div_ceil(statistics.group_size)
        || statistics.columns.len() != block.header.columns.len()
        || block.data_offset == 0
    {
        return None;
    }
    let declared =
        declared_columns(metadata, block.database.as_deref(), &block.header.qualified_name());
    let believed: Vec<bool> = block
        .header
        .columns
        .iter()
        .zip(&statistics.columns)
        .map(|(name, column)| {
            let Some(column) = column else { return false };
            let def = declared.and_then(|columns| columns.iter().find(|c| &c.name == name));
            column.declared_type.as_deref() == def.map(|d| d.declared_type.as_str())
                && column.collation.as_deref() == def.and_then(|d| d.collation.as_deref())
        })
        .collect();
    let kinds = stored_bounds_kinds(&block.header, metadata, block.database.as_deref());
    Some((believed, kinds))
}

/// Where `view`'s block, read under `filter`, stops: at the first row making
/// one of the filter's required ordering terms `False`, for each such term
/// on a column the block's rows are sorted on the way its bound closes.
fn sorted_stop(filter: &ResolvedExpr, view: &Believed<'_>) -> Option<SortedStop> {
    let terms: Vec<ResolvedTerm> = filter
        .required_ordering_terms()
        .into_iter()
        .filter(|term| {
            let Some(kind) = term.bounds_kind() else { return false };
            let Some(bounds) = view.bounds(term.index(), kind) else { return false };
            matches!(
                (bounds.sortedness, term.op()),
                (Sortedness::Ascending, PredicateOp::Lt | PredicateOp::Le)
                    | (Sortedness::Descending, PredicateOp::Gt | PredicateOp::Ge)
            )
        })
        .cloned()
        .collect();
    (!terms.is_empty()).then_some(SortedStop { terms })
}

/// One block's statistics, held by a replay reading a **dynamic filter**
/// against them (`crate::stream::DynamicFilter`): which of the block's groups
/// the filter's state keeps, and where the block's row order stops a read
/// under it.
///
/// **A group's verdict is reached only when the replay reaches the group**,
/// and kept until the state moves. A state can move at every group boundary —
/// a TopK tightens while the scan streams — so re-pruning the whole block on
/// each move would evaluate its every group at each of its boundaries; asked
/// in order, each group is evaluated once per state the replay reads there.
///
/// **Its stop is asked only in a group whose statistics say a row there can
/// pass it** ([`Self::stop`]). The state is evaluated nowhere else, so the
/// stop is asked of every row it is asked of at all; asked in every kept
/// group, it would evaluate its terms on each row of each group before the
/// one holding the bound, where none can pass it and what it could save is
/// the rest of that one group, pruning skipping every group after it.
///
/// `generation` and `filter` are the state last read, resolved against this
/// block ([`Self::read`]); until the first read the filter is the empty
/// conjunction, keeping every group.
pub(crate) struct DynamicPruning {
    statistics: Arc<BlockStatistics>,
    believed: Vec<bool>,
    kinds: StoredKinds,
    data_offset: u64,
    generation: Option<u64>,
    filter: ResolvedExpr,
    /// Per group, whether `filter` keeps it, where asked since it was read.
    verdicts: Vec<Option<bool>>,
    stop: Option<SortedStop>,
    /// The group the replay last entered ([`Self::enters`]), and whether a
    /// row of it can pass `stop` ([`Self::arm`]).
    entered: Option<usize>,
    armed: bool,
}

impl DynamicPruning {
    /// `block`'s `statistics`, as far as they are believed under `metadata`
    /// — or `None` where they answer nothing ([`prune_block`]'s conditions).
    pub(crate) fn new(
        block: &CopyBlock,
        statistics: Arc<BlockStatistics>,
        metadata: Option<&DumpMetadata>,
    ) -> Option<Self> {
        let (believed, kinds) = believed_columns(block, &statistics, metadata)?;
        let groups = statistics.groups.len();
        Some(Self {
            statistics,
            believed,
            kinds,
            data_offset: block.data_offset,
            generation: None,
            filter: ResolvedExpr::And(Vec::new()),
            verdicts: vec![None; groups],
            stop: None,
            entered: None,
            armed: false,
        })
    }

    /// The generation of the state last read, `None` before the first.
    pub(crate) fn generation(&self) -> Option<u64> {
        self.generation
    }

    /// Take `filter`, the state at `generation` resolved against this block:
    /// every verdict reached under the state before is forgotten.
    pub(crate) fn read(&mut self, generation: u64, filter: ResolvedExpr) {
        let view =
            Believed { statistics: &self.statistics, believed: &self.believed, kinds: &self.kinds };
        self.stop = sorted_stop(&filter, &view);
        self.filter = filter;
        self.generation = Some(generation);
        self.verdicts.fill(None);
    }

    /// The group whose rows include the one starting at `offset`, or `None`
    /// past the last — the block's `\.` line, if nothing else.
    pub(crate) fn group_of(&self, offset: u64) -> Option<usize> {
        let group = offset.checked_sub(self.data_offset)? / self.statistics.group_size;
        usize::try_from(group).ok().filter(|&group| group < self.groups())
    }

    /// Whether the replay, now at a row of `group`, has just entered it:
    /// the first row it reads there since entering another.
    pub(crate) fn enters(&mut self, group: usize) -> bool {
        self.entered.replace(group) != Some(group)
    }

    /// The groups the statistics list.
    pub(crate) fn groups(&self) -> usize {
        self.statistics.groups.len()
    }

    /// Where a replay segment reading `group` from its first row starts its
    /// search (`crate::stream::Segment`), as [`BlockPruning::kept`] states a
    /// run's.
    pub(crate) fn search_start(&self, group: usize) -> u64 {
        self.data_offset + group as u64 * self.statistics.group_size - 1
    }

    /// Whether the state last read could keep a row of `group`.
    pub(crate) fn keeps(&mut self, group: usize) -> bool {
        if let Some(verdict) = self.verdicts[group] {
            return verdict;
        }
        let view =
            Believed { statistics: &self.statistics, believed: &self.believed, kinds: &self.kinds };
        let verdict = view.keeps(&self.filter, group);
        self.verdicts[group] = Some(verdict);
        verdict
    }

    /// Ask [`Self::stop`] of `group`'s rows where a row of it can pass the
    /// stop of the state last read, and of none where no row can.
    pub(crate) fn arm(&mut self, group: usize) {
        let view =
            Believed { statistics: &self.statistics, believed: &self.believed, kinds: &self.kinds };
        let group = Group { view: &view, index: group };
        self.armed = self.stop.as_ref().is_some_and(|stop| stop.reachable(&group));
    }

    /// Where the state last read stops a read of the block, if a row of the
    /// group last armed can reach it ([`Self::arm`]).
    pub(crate) fn stop(&self) -> Option<&SortedStop> {
        self.stop.as_ref().filter(|_| self.armed)
    }
}

/// A block's statistics as far as they are believed.
struct Believed<'a> {
    statistics: &'a BlockStatistics,
    /// Per header column, whether its bounds and dictionary are believed.
    believed: &'a [bool],
    /// Per header column, the kinds its stored sets of bounds are ordered by.
    kinds: &'a [[Option<CompareKind>; 2]],
}

impl Believed<'_> {
    fn believed(&self, column: usize) -> Option<&ColumnStatistics> {
        if !self.believed.get(column).copied().unwrap_or(false) {
            return None;
        }
        self.statistics.columns.get(column)?.as_ref()
    }

    /// The believed set of `column`'s bounds ordered as `kind` orders.
    fn bounds(&self, column: usize, kind: &CompareKind) -> Option<&ColumnBounds> {
        let set = bounds_set_keyed_by(self.kinds.get(column)?, kind)?;
        self.believed(column)?.bounds_in(set)
    }

    /// Whether some row of group `index` could make `filter`'s root `True`.
    fn keeps(&self, filter: &ResolvedExpr, index: usize) -> bool {
        filter.truths(&Group { view: self, index }).contains(Truth::True)
    }
}

/// One group of a block's statistics, as the evaluator reads it.
struct Group<'a> {
    view: &'a Believed<'a>,
    index: usize,
}

impl GroupStatistics for Group<'_> {
    fn rows(&self) -> u64 {
        self.view.statistics.groups[self.index].rows
    }

    fn null_count(&self, column: usize) -> Option<u64> {
        self.view.statistics.columns.get(column)?.as_ref()?.null_counts.get(self.index).copied()
    }

    fn bounds(&self, column: usize, kind: &CompareKind) -> Option<(&str, &str)> {
        let bounds = self.view.bounds(column, kind)?.groups.get(self.index)?.as_ref()?;
        Some((&bounds.min, &bounds.max))
    }

    fn dictionary(&self, column: usize) -> Option<impl Iterator<Item = &str>> {
        let dictionary = self.view.believed(column)?.dictionary.as_ref()?;
        let indices = dictionary.groups.get(self.index)?.as_ref()?;
        // An index past the entries leaves the dictionary incomplete, and an
        // incomplete dictionary is no dictionary.
        indices
            .iter()
            .all(|&i| (i as usize) < dictionary.entries.len())
            .then(|| indices.iter().map(|&i| dictionary.entries[i as usize].as_str()))
    }
}
