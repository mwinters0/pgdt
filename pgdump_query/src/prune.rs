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

use std::ops::Range;

use crate::copy::{RawRow, RowSplit};
use crate::gather::declared_columns;
use crate::index::CopyBlock;
use crate::preamble::DumpMetadata;
use crate::predicate::{GroupStatistics, PredicateOp, ResolvedExpr, ResolvedTerm, Truth};
use crate::statistics::{BlockStatistics, ColumnStatistics, Sortedness};

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
/// be, and nothing past it is read.
#[derive(Debug, Clone)]
pub(crate) struct SortedStop {
    terms: Vec<ResolvedTerm>,
}

impl SortedStop {
    /// Whether `raw_row`, a row `filter` did **not** keep, is past the bound.
    /// Asked only of a rejected row: a kept row made every term `True`.
    pub(crate) fn passed(&self, raw_row: RawRow<'_>, split: &mut RowSplit) -> bool {
        self.terms.iter().any(|term| term.is_false(raw_row, split))
    }
}

/// Which of `block`'s groups `filter` could keep a row of, and where its row
/// order stops a replay of it — or `None` where the block's statistics answer
/// nothing: it holds none, lists no group, or holds statistics that do not fit
/// its extent or its column list.
///
/// **A column's bounds, row order and dictionary are believed only under the
/// declared type and collation they were gathered under**, compared against
/// what `metadata` declares now; its NULL counts are read off the text and
/// believed regardless. What the comparison itself believes is settled when
/// the filter resolved ([`ResolvedExpr::truths`]).
pub(crate) fn prune_block(
    block: &CopyBlock,
    filter: &ResolvedExpr,
    metadata: Option<&DumpMetadata>,
) -> Option<BlockPruning> {
    let statistics = block.statistics.as_deref()?;
    let last = statistics.groups.len().checked_sub(1)?;
    let data = block.terminator_offset.saturating_sub(block.data_offset);
    // The last group is the one the last row starts in, so no index reaches
    // past the data; a block whose statistics say otherwise describes other
    // bytes, and is read whole.
    if statistics.group_size == 0
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

    let terms: Vec<ResolvedTerm> = filter
        .required_ordering_terms()
        .into_iter()
        .filter(|term| {
            let column = term.index();
            let bounds = statistics.columns.get(column).and_then(|c| c.as_ref()?.bounds.as_ref());
            let Some(bounds) = bounds.filter(|_| believed.get(column) == Some(&true)) else {
                return false;
            };
            matches!(
                (bounds.sortedness, term.op()),
                (Sortedness::Ascending, PredicateOp::Lt | PredicateOp::Le)
                    | (Sortedness::Descending, PredicateOp::Gt | PredicateOp::Ge)
            )
        })
        .cloned()
        .collect();

    let n = statistics.group_size;
    let mut pruning = BlockPruning {
        kept: Vec::new(),
        groups: statistics.groups.len() as u64,
        skipped_groups: 0,
        skipped_bytes: 0,
        stop: (!terms.is_empty()).then_some(SortedStop { terms }),
    };
    for (index, group) in statistics.groups.iter().enumerate() {
        let view = Group { statistics, believed: &believed, index };
        if !filter.truths(&view).contains(Truth::True) {
            pruning.skipped_groups += 1;
            pruning.skipped_bytes += group.bytes;
            continue;
        }
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

/// One group of a block's statistics, as the evaluator reads it.
struct Group<'a> {
    statistics: &'a BlockStatistics,
    /// Per header column, whether its bounds and dictionary are believed.
    believed: &'a [bool],
    index: usize,
}

impl Group<'_> {
    fn believed(&self, column: usize) -> Option<&ColumnStatistics> {
        if !self.believed.get(column).copied().unwrap_or(false) {
            return None;
        }
        self.statistics.columns.get(column)?.as_ref()
    }
}

impl GroupStatistics for Group<'_> {
    fn rows(&self) -> u64 {
        self.statistics.groups[self.index].rows
    }

    fn null_count(&self, column: usize) -> Option<u64> {
        self.statistics.columns.get(column)?.as_ref()?.null_counts.get(self.index).copied()
    }

    fn bounds(&self, column: usize) -> Option<(&str, &str)> {
        let bounds = self.believed(column)?.bounds.as_ref()?.groups.get(self.index)?.as_ref()?;
        Some((&bounds.min, &bounds.max))
    }

    fn dictionary(&self, column: usize) -> Option<impl Iterator<Item = &str>> {
        let dictionary = self.believed(column)?.dictionary.as_ref()?;
        let indices = dictionary.groups.get(self.index)?.as_ref()?;
        // An index past the entries leaves the dictionary incomplete, and an
        // incomplete dictionary is no dictionary.
        indices
            .iter()
            .all(|&i| (i as usize) < dictionary.entries.len())
            .then(|| indices.iter().map(|&i| dictionary.entries[i as usize].as_str()))
    }
}
