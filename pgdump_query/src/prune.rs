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
//! **A dynamic filter is read against the same statistics** ([`DynamicPruning`]):
//! each group the static filter kept, once, when the first sub-stream is
//! polled, which is where they
//! are cut ([`DynamicPruning::kept_within`]), and after that a group at a time
//! as the replay reaches it, or the group being read where a state moves
//! inside it, rather than every group of the block at each move, since its
//! state can move at any moment (`docs/design/decisions.md`, "D93").

use std::ops::Range;
use std::sync::Arc;

use crate::copy::{RawRow, RowSplit};
use crate::gather::{declared_column, stored_bounds_kinds};
use crate::index::{CopyBlock, Unrepresentable};
use crate::pgtype::{CompareKind, bounds_set_keyed_by};
use crate::preamble::DumpMetadata;
use crate::predicate::{GroupStatistics, PredicateOp, ResolvedExpr, ResolvedTerm, Truth};
use crate::statistics::{BlockStatistics, BoundsSet, ColumnStatistics, Sortedness, StatisticsView};

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
    /// filter's stop is asked only of a row that filter rejected, a kept row
    /// having made every term `True` — a dynamic filter's only in a group it
    /// is armed in ([`DynamicPruning::arm`]), and there of every row the
    /// static filter rejects, which its state is not evaluated on, and of
    /// every kept row where rows are not evaluated at all.
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
/// believed regardless where every value is read, and in a view taking some
/// as NULL only under that same declared type. What the comparison itself believes is settled when
/// the filter resolved ([`ResolvedExpr::truths`]), **and which stored set of
/// bounds a term reads is this block's**: the one gathering stored under the
/// kind the term compares by, gathering's kinds recomputed from the DDL the
/// believed column was gathered under (`docs/design/decisions.md`, "D79").
///
/// **`reading` is how the filter takes a value its column's type cannot
/// hold** ([`StatisticsView`]), and the bounds, row order and NULL count read
/// are that view's — an added NULL believed only where the bounds are, the
/// tier being the declared type's (`docs/design/decisions.md`, "D97").
pub(crate) fn prune_block(
    block: &CopyBlock,
    filter: &ResolvedExpr,
    metadata: Option<&DumpMetadata>,
    reading: StatisticsView,
) -> Option<BlockPruning> {
    let statistics = block.statistics.as_deref()?;
    let last = statistics.groups.len().checked_sub(1)?;
    let (believed, kinds) = believed_columns(block, statistics, metadata)?;
    let view = Believed { statistics, believed: &believed, kinds: &kinds, reading };

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
        let run =
            group_run(block.data_offset..block.end_offset, statistics.group_size, index, last);
        push_run(&mut pruning.kept, run);
    }
    pruning.kept = or_header_run(pruning.kept, block.data_offset);
    Some(pruning)
}

/// Group `index` of the block whose data starts at `extent.start` and which
/// ends at `extent.end`, its statistics grouping by `group_size` and listing
/// `last + 1` groups, as a replay segment's search bounds
/// ([`BlockPruning::kept`]): the last ends at the block's end.
fn group_run(extent: Range<u64>, group_size: u64, index: usize, last: usize) -> Range<u64> {
    let k = index as u64;
    let start = extent.start + k * group_size - 1;
    let end = if index == last { extent.end } else { extent.start + (k + 1) * group_size - 1 };
    start..end
}

/// `run` appended to `runs`, the last of them extended where `run` continues
/// it.
fn push_run(runs: &mut Vec<Range<u64>>, run: Range<u64>) {
    match runs.last_mut() {
        Some(last) if last.end == run.start => last.end = run.end,
        _ => runs.push(run),
    }
}

/// `runs`, or the empty run on the header's LF where there are none, which
/// reads the header line and no row ([`BlockPruning::kept`]).
fn or_header_run(mut runs: Vec<Range<u64>>, data_offset: u64) -> Vec<Range<u64>> {
    if runs.is_empty() {
        let header_lf = data_offset - 1;
        runs.push(header_lf..header_lf);
    }
    runs
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
    let (database, qualified) = (block.database.as_deref(), block.header.qualified_name());
    let believed: Vec<bool> = block
        .header
        .columns
        .iter()
        .zip(&statistics.columns)
        .map(|(name, column)| {
            let Some(column) = column else { return false };
            let def = declared_column(metadata, database, &qualified, name);
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
            let Some(sortedness) = view.sortedness(term.index(), kind) else { return false };
            matches!(
                (sortedness, term.op()),
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
/// **Past the cut, a group's verdict is reached only when the replay reaches
/// the group**, and kept until the state moves. The cut asks each group the
/// static filter kept once, under the state it reads ([`Self::kept_within`]),
/// and a replay reading that state takes its verdicts ([`Self::seed`]). A
/// state can move at any moment — a TopK tightens while the scan streams — so
/// re-pruning the whole block on each move would evaluate its every group at
/// each read; asked in order, each group is evaluated once per state the
/// replay reads there.
///
/// **Its stop is asked only in a group whose statistics say a row there can
/// pass it** ([`Self::stop`]). The state is evaluated, where rows are, on the
/// rows the static filter keeps and on no others (`crate::stream`'s
/// `DynamicRead::rejects`), so the stop is asked of every row the static
/// filter rejects that it is asked of at all, and of each kept row the state
/// rejects — of every kept row, where rows are not evaluated; asked in every
/// kept group, it would evaluate its terms on such rows of each group before
/// the one holding the bound, where none can pass it and what it could save
/// is the rest of that one group, pruning skipping every group after it.
///
/// `filter` is the state last read, resolved against this block
/// ([`Self::read`]); until the first read it is the empty conjunction,
/// keeping every group.
pub(crate) struct DynamicPruning {
    statistics: Arc<BlockStatistics>,
    believed: Vec<bool>,
    kinds: StoredKinds,
    /// How the state takes a value its column's type cannot hold
    /// ([`prune_block`]).
    reading: StatisticsView,
    /// The block's data and its end, as [`group_run`] reads them.
    extent: Range<u64>,
    filter: Arc<ResolvedExpr>,
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
    /// and read in `reading` — or `None` where they answer nothing
    /// ([`prune_block`]'s conditions).
    pub(crate) fn new(
        block: &CopyBlock,
        statistics: Arc<BlockStatistics>,
        metadata: Option<&DumpMetadata>,
        reading: StatisticsView,
    ) -> Option<Self> {
        let (believed, kinds) = believed_columns(block, &statistics, metadata)?;
        let groups = statistics.groups.len();
        Some(Self {
            statistics,
            believed,
            kinds,
            reading,
            extent: block.data_offset..block.end_offset,
            filter: Arc::new(ResolvedExpr::And(Vec::new())),
            verdicts: vec![None; groups],
            stop: None,
            entered: None,
            armed: false,
        })
    }

    fn believed_view(&self) -> Believed<'_> {
        let (statistics, believed, kinds) = (&self.statistics, &self.believed, &self.kinds);
        Believed { statistics, believed, kinds, reading: self.reading }
    }

    /// Take `filter`, a state resolved against this block: every verdict
    /// reached under the state before is forgotten.
    pub(crate) fn read(&mut self, filter: Arc<ResolvedExpr>) {
        let view = self.believed_view();
        self.stop = sorted_stop(&filter, &view);
        self.filter = filter;
        self.verdicts.fill(None);
    }

    /// The verdict reached on each group under the state last read, `None`
    /// for one not yet asked.
    pub(crate) fn verdicts(&self) -> &[Option<bool>] {
        &self.verdicts
    }

    /// Take `verdicts`, reached on this block's groups under the state last
    /// read by another reading of it, in place of asking those groups again.
    /// Ones of another length describe other groups, and are not taken.
    pub(crate) fn seed(&mut self, verdicts: &[Option<bool>]) {
        if verdicts.len() == self.verdicts.len() {
            self.verdicts.copy_from_slice(verdicts);
        }
    }

    /// The group whose rows include the one starting at `offset`, or `None`
    /// before the block's data or past its last group's `group_size` bytes.
    pub(crate) fn group_of(&self, offset: u64) -> Option<usize> {
        let group = offset.checked_sub(self.extent.start)? / self.statistics.group_size;
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
        self.run(group).start
    }

    /// `group` as a replay segment's search bounds ([`group_run`]).
    fn run(&self, group: usize) -> Range<u64> {
        let last = self.groups() - 1;
        group_run(self.extent.clone(), self.statistics.group_size, group, last)
    }

    /// The runs of this block's groups the state last read keeps, stated as
    /// [`BlockPruning::kept`] states them, **of those `within` holds** — the
    /// runs a static filter's pruning kept, or every group where `None` — and
    /// how many groups `within` holds that the state rules out. Every group
    /// `within` holds is asked of the state, which is what a cut made before a
    /// row is read costs.
    pub(crate) fn kept_within(&mut self, within: Option<&[Range<u64>]>) -> (Vec<Range<u64>>, u64) {
        let mut within = within.map(|runs| runs.iter().peekable());
        let (mut runs, mut ruled_out) = (Vec::new(), 0);
        for group in 0..self.groups() {
            let run = self.run(group);
            // Both lists are in file order, so the static runs are walked
            // once: a group is in one where the first run not ending at or
            // before its start begins at or before it.
            let statically = within.as_mut().is_none_or(|within| {
                while within.next_if(|kept| kept.end <= run.start).is_some() {}
                within.peek().is_some_and(|kept| kept.start <= run.start)
            });
            if !statically {
                continue;
            }
            if self.keeps(group) {
                push_run(&mut runs, run);
            } else {
                ruled_out += 1;
            }
        }
        (or_header_run(runs, self.extent.start), ruled_out)
    }

    /// Whether the state last read could keep a row of `group`.
    pub(crate) fn keeps(&mut self, group: usize) -> bool {
        if let Some(verdict) = self.verdicts[group] {
            return verdict;
        }
        let view = self.believed_view();
        let verdict = view.keeps(&self.filter, group);
        self.verdicts[group] = Some(verdict);
        verdict
    }

    /// Ask [`Self::stop`] of `group`'s rows where a row of it can pass the
    /// stop of the state last read, and of none where no row can. Asked in
    /// every kept group, the stop cost a row's evaluation over every group
    /// before the bound's, to save at most the rest of one.
    pub(crate) fn arm(&mut self, group: usize) {
        let view = self.believed_view();
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
    /// The view of each column's values read ([`prune_block`]).
    reading: StatisticsView,
}

impl Believed<'_> {
    fn believed(&self, column: usize) -> Option<&ColumnStatistics> {
        if !self.believed.get(column).copied().unwrap_or(false) {
            return None;
        }
        self.statistics.columns.get(column)?.as_ref()
    }

    /// The believed column and its set of bounds ordered as `kind` orders.
    fn set(&self, column: usize, kind: &CompareKind) -> Option<(&ColumnStatistics, BoundsSet)> {
        let set = bounds_set_keyed_by(self.kinds.get(column)?, kind)?;
        Some((self.believed(column)?, set))
    }

    /// The row order of the believed set of `column`'s bounds ordered as
    /// `kind` orders, in the view read.
    fn sortedness(&self, column: usize, kind: &CompareKind) -> Option<Sortedness> {
        let (statistics, set) = self.set(column, kind)?;
        statistics.sortedness(set, self.reading)
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

    /// Read off the text and believed regardless in every value, and in a
    /// view taking some as NULL only where the column's declared type is the
    /// one its tiers were counted under.
    fn null_count(&self, column: usize) -> Option<u64> {
        let statistics = match self.view.reading {
            StatisticsView::Every => self.view.statistics.columns.get(column)?.as_ref()?,
            _ => self.view.believed(column)?,
        };
        statistics.null_count(self.index, self.view.reading)
    }

    fn bounds(&self, column: usize, kind: &CompareKind) -> Option<(&str, &str)> {
        let (statistics, set) = self.view.set(column, kind)?;
        let bounds = statistics.group_bounds(set, self.view.reading, self.index)?;
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

    /// Believed only where the column's declared type is the one its tiers
    /// were counted under, as a view's NULL count is. The test is answered off
    /// this count, never the null mode's NULL count, in either direction: a
    /// group of NULLs under that view may hold nothing but such values.
    fn unrepresentable(&self, column: usize) -> Option<Unrepresentable> {
        Some(self.view.believed(column)?.unrepresentable_in(self.index))
    }
}
