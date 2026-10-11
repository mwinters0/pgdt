//! A filter the query's consumer refines while the query runs, as one
//! sub-stream's replay reads it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pgdt_codec::ResolvedSchema;
use pgdt_codec::internal::UnrepresentableRead;
use pgdt_codec::pgtype::ComparisonSemantics;
#[cfg(feature = "introspect")]
use pgdt_core::internal::{evaluated_row, timed_span};
use pgdt_core::{RawRow, RowSplit};
use pgdt_core::{
    instrument::EvaluationPart,
    internal::{row_evaluated, timed},
};
use pgdt_model::{BlockStatistics, CopyBlock, PostgresInvalidValues};

use super::plan::{ReplayPlan, query_reads};
use super::resolve::resolve_loosened;
use super::table::resolve_block;
use super::table_partitions::Cut;
use crate::predicate::{Expr, ResolvedExpr};
use crate::prune::{DynamicPruning, SortedStop};

/// A filter a query's consumer refines while the query runs — a hash join's
/// build side, a TopK's heap — handed to a partitioned replay
/// ([`TablePartitions::under`](super::table_partitions::TablePartitions::under)), which cuts its sub-streams over the row
/// groups the state keeps when the first is polled, skips those it rules out
/// as it reaches them, ends a sorted block at the first row past a bound the
/// state requires, and, under [`RowEvaluation::On`], drops each row of the
/// rest the state rejects before decoding it.
///
/// **Its contract is looser than a filter's, and is stated here alone.** A
/// replay may drop any row a state it read rejects, at any moment, and reads
/// the state at its first poll, as it enters a row group and as it takes each
/// chunk: what a stream emits lies between the rows its static filter keeps
/// and those the states it read keep, and depends on when each was read. So a
/// replay is handed one by the one entry point that never resumes, a resume
/// token counting rows being meaningless under it
/// (`docs/design/decisions.md`, "D95"). Where the query does not
/// use statistics it skips no group on its account, and still drops each row
/// a state rejects where it evaluates rows. A consumer needing an exact answer
/// re-checks its own rows.
///
/// **A state is the library's filter tree**, over the table's column names,
/// resolved against each block as a static filter is, in the query's
/// semantics: a term a block cannot resolve stands as whatever keeps every
/// row where it sits, never as the query's refusal.
pub trait DynamicFilter: Send + Sync {
    /// A number that moves whenever the state does. Asked each time the
    /// replay enters a row group or takes a chunk, so it should cost little.
    fn generation(&self) -> u64;

    /// The state now, and the generation it is the state of.
    fn current(&self) -> (u64, Arc<Expr>);
}

/// Whether a replay under a [`DynamicFilter`] evaluates each row its static
/// filter keeps against the state, dropping the rows it rejects before they
/// decode. Either way the state still skips the row groups it rules out, cuts
/// the sub-streams and ends a block sorted past a bound it requires; what
/// `Off` gives up is only the rows its consumer would have dropped itself.
///
/// **`Off` is the default** (`docs/design/decisions.md`, "D93").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowEvaluation {
    /// Each row is left to the filter's consumer.
    #[default]
    Off,
    /// Each row the state rejects is dropped before a column of it decodes.
    On,
}

/// A [`DynamicFilter`] as one sub-stream's replay reads it: the filter, the
/// statistics of the blocks it may prune, and the block being read.
pub(super) struct DynamicRead {
    pub(super) filter: Arc<dyn DynamicFilter>,
    /// Whether each row is evaluated against the state.
    pub(super) evaluation: RowEvaluation,
    pub(super) statistics: Arc<BTreeMap<u64, Arc<BlockStatistics>>>,
    /// The cut the sub-stream was handed, whose verdicts a state it read is
    /// read with again rather than asked of each group anew.
    pub(super) cut: Arc<Cut>,
    /// The block last entered, by `header_offset`, as the filter reads it —
    /// `None` for one whose schema does not resolve.
    pub(super) block: Option<(u64, Option<DynamicBlock>)>,
    /// The groups ruled out so far ([`TableStream::dynamic_filter_pruned_groups`](super::table::TableStream::dynamic_filter_pruned_groups)).
    pub(super) pruned: Arc<AtomicU64>,
    /// The rows dropped so far ([`TableStream::dynamic_filter_pruned_rows`](super::table::TableStream::dynamic_filter_pruned_rows)).
    pub(super) rows: Arc<AtomicU64>,
}

/// One block as a [`DynamicFilter`] reads it: its unprojected schema, which
/// each state is resolved against, the state last read, and its statistics'
/// pruning where they answer.
pub(super) struct DynamicBlock {
    header_offset: u64,
    full: ResolvedSchema,
    /// How the query reads each column's values its type cannot hold, which
    /// each state's leaves read them by, as the static filter's do.
    unrepresentable: Vec<Option<UnrepresentableRead>>,
    /// How the query reads a field its type's `*_in` refuses, as the static
    /// filter's leaves do.
    invalid: PostgresInvalidValues,
    /// The generation of the state last read, `None` before the first.
    generation: Option<u64>,
    /// The state last read, resolved against this block: what its groups
    /// are judged by.
    state: Arc<ResolvedExpr>,
    /// The same state as its rows are evaluated against
    /// ([`ResolvedExpr::for_rows`]), where they are.
    rows: Option<ResolvedExpr>,
    /// `None` where the block's statistics answer nothing
    /// ([`DynamicPruning::new`]), no group then being ruled out.
    pub(super) pruning: Option<DynamicPruning>,
}

impl DynamicBlock {
    /// `block` of `plan`'s table, pruned by `statistics` where they answer —
    /// or `None` where its schema does not resolve.
    pub(super) fn new(
        block: &CopyBlock,
        statistics: Option<&Arc<BlockStatistics>>,
        plan: &ReplayPlan,
    ) -> Option<Self> {
        let metadata = plan.metadata.as_ref();
        let census = plan.table.census_for(&block.header.columns);
        let text = plan.table.text_for(&block.header.columns);
        let (schema_mode, semantics) =
            (plan.query_options.schema_mode, plan.query_options.semantics);
        let database = block.database.as_deref();
        let full = resolve_block(
            &block.header,
            metadata,
            database,
            schema_mode,
            &census,
            &text,
            semantics,
        )
        .ok()?;
        let counts = block.unrepresentable.as_deref();
        let unrepresentable =
            query_reads(&block.header, metadata, database, &full, counts, &plan.query_options);
        let reading = plan.query_options.statistics_view();
        let pruning = statistics.and_then(|statistics| {
            DynamicPruning::new(block, Arc::clone(statistics), metadata, reading)
        });
        Some(Self {
            header_offset: block.header_offset,
            full,
            unrepresentable,
            invalid: plan.query_options.postgres_invalid_values,
            generation: None,
            state: Arc::new(ResolvedExpr::And(Vec::new())),
            rows: None,
            pruning,
        })
    }

    /// Take `state`, the filter's state at `generation`, resolved against
    /// this block ([`resolve_loosened`]) — whole for its groups, and in its
    /// row form for its rows where `evaluation` evaluates them
    /// (`docs/design/decisions.md`, "D93").
    pub(super) fn read(
        &mut self,
        generation: u64,
        state: &Expr,
        semantics: ComparisonSemantics,
        evaluation: RowEvaluation,
    ) {
        let resolved = resolve_loosened(state, &self.full, self.header_offset, semantics, false)
            .reading(&self.unrepresentable)
            .invalid_values(self.invalid);
        self.rows = (evaluation == RowEvaluation::On).then(|| resolved.for_rows());
        self.state = Arc::new(resolved);
        self.generation = Some(generation);
        if let Some(pruning) = self.pruning.as_mut() {
            pruning.read(Arc::clone(&self.state));
        }
    }
}

/// What a [`DynamicFilter`] says of the row a replay is about to read
/// ([`DynamicRead::at_row`]).
pub(super) enum Skip {
    Read,
    /// The row's group and every one up to the group whose search starts at
    /// this offset are ruled out: the segment ends, and resumes here.
    To(u64),
    /// The row's group and every later one the segment holds are ruled out.
    Rest,
}

impl DynamicRead {
    /// Make `block` the block being read, unless it already is. Its state is
    /// first read at the first group it enters or chunk it takes.
    pub(super) fn enter(&mut self, block: &CopyBlock, plan: &ReplayPlan) {
        if self.block.as_ref().is_some_and(|(at, _)| *at == block.header_offset) {
            return;
        }
        let statistics = self.statistics.get(&block.header_offset);
        self.block = Some((block.header_offset, DynamicBlock::new(block, statistics, plan)));
    }

    /// The block being read, where its schema resolves.
    fn block(&mut self) -> Option<&mut DynamicBlock> {
        self.block.as_mut()?.1.as_mut()
    }

    /// Read the state into the block being read where its generation has
    /// moved since the block last read it, and say whether it had.
    fn refresh(&mut self, semantics: ComparisonSemantics) -> bool {
        let moved = self.filter.generation();
        let (filter, cut) = (Arc::clone(&self.filter), Arc::clone(&self.cut));
        let evaluation = self.evaluation;
        let Some(block) = self.block() else { return false };
        if block.generation == Some(moved) {
            return false;
        }
        let (generation, state) = filter.current();
        block.read(generation, &state, semantics, evaluation);
        // A generation names one state, so the cut's verdicts under it are
        // this state's.
        if generation == cut.generation
            && let Some(verdicts) = cut.verdicts.get(&block.header_offset)
            && let Some(pruning) = block.pruning.as_mut()
        {
            pruning.seed(verdicts);
        }
        true
    }

    /// The group of the row starting at `offset` in the block being read,
    /// where its statistics answer, and whether the replay enters it there.
    fn group_at(&mut self, offset: u64) -> Option<(usize, bool)> {
        let pruning = self.block()?.pruning.as_mut()?;
        let group = pruning.group_of(offset)?;
        Some((group, pruning.enters(group)))
    }

    /// What the filter says of the row starting at `offset`, the next a
    /// segment searched from `start` to `limit` owns, **as the row enters a
    /// group**: the state is re-read there where its generation has moved,
    /// and a group it rules out is skipped with every one after it that it
    /// also rules out, up to the segment's limit ([`Self::judge`]).
    pub(super) fn at_row(
        &mut self,
        offset: u64,
        start: u64,
        limit: u64,
        semantics: ComparisonSemantics,
    ) -> Skip {
        match self.group_at(offset) {
            Some((group, true)) => {
                self.refresh(semantics);
                self.judge(group, true, start, limit)
            }
            _ => Skip::Read,
        }
    }

    /// What the filter says as the replay takes a chunk, its scanner at
    /// `offset`, in a segment searched from `start` to `limit`: the state is
    /// re-read where its generation has moved, in every block, and **a state
    /// read inside a group acts as one read at its entry does** — the group
    /// is judged again, the rest of it skipped with every later group the
    /// state rules out, and one still kept re-armed ([`Self::judge`];
    /// `docs/design/decisions.md`, "D93").
    pub(super) fn at_chunk(
        &mut self,
        offset: u64,
        start: u64,
        limit: u64,
        semantics: ComparisonSemantics,
    ) -> Skip {
        timed!(EvaluationPart::Chunk, {
            if !self.refresh(semantics) {
                return Skip::Read;
            }
            match self.group_at(offset) {
                Some((group, entering)) => self.judge(group, entering, start, limit),
                None => Skip::Read,
            }
        })
    }

    /// Whether the state last read keeps `group` of the block being read,
    /// arming its stop there where it does, and where it does not, the
    /// groups up to the next it keeps skipped, counting each whose search
    /// start the segment searched from `start` holds and of which nothing
    /// was read — `group` only where the replay is `entering` it.
    fn judge(&mut self, group: usize, entering: bool, start: u64, limit: u64) -> Skip {
        let Some(pruning) = self.block().and_then(|block| block.pruning.as_mut()) else {
            return Skip::Read;
        };
        if pruning.keeps(group) {
            pruning.arm(group);
            return Skip::Read;
        }
        let mut skipped = u64::from(entering && start <= pruning.search_start(group));
        let mut next = group + 1;
        let skip = loop {
            if next >= pruning.groups() || pruning.search_start(next) >= limit {
                break Skip::Rest;
            }
            if pruning.keeps(next) {
                break Skip::To(pruning.search_start(next));
            }
            skipped += 1;
            next += 1;
        };
        self.pruned.fetch_add(skipped, Ordering::Relaxed);
        skip
    }

    /// Where the state last read stops the block being read, if anywhere.
    pub(super) fn stop(&self) -> Option<&SortedStop> {
        self.block.as_ref()?.1.as_ref()?.pruning.as_ref()?.stop()
    }

    /// The state last read, resolved against the block being read as its
    /// rows are evaluated against it — `None` where rows are not evaluated,
    /// before the first read, or where the block's schema does not resolve.
    fn state(&self) -> Option<&ResolvedExpr> {
        self.block.as_ref()?.1.as_ref()?.rows.as_ref()
    }

    /// Whether each row is evaluated against the state.
    pub(super) fn evaluates_rows(&self) -> bool {
        self.evaluation == RowEvaluation::On
    }

    /// Whether evaluating the state last read reads a field of a row.
    pub(super) fn reads_fields(&self) -> bool {
        self.state().is_some_and(ResolvedExpr::reads_fields)
    }

    /// Whether the state last read rejects `raw_row`, a row the static filter
    /// kept, which the replay then drops before decoding a column of it,
    /// counting it ([`TableStream::dynamic_filter_pruned_rows`](super::table::TableStream::dynamic_filter_pruned_rows)) — never
    /// where rows are not evaluated ([`RowEvaluation`]).
    ///
    /// **Evaluated in every block**, statistics or not, the state being read
    /// at each chunk as well as at each group entered
    /// (`docs/design/decisions.md`, "D93"). **A field that does not decode
    /// keeps the row**: a dynamic filter only licenses dropping a row, and
    /// the error is raised where evaluation or the row's own decoding
    /// reaches it (`docs/design/decisions.md`, "D54").
    pub(super) fn rejects(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> bool {
        let Some(state) = self.state() else { return false };
        // Timed whole where the instrument is built in
        // (`pgdt_core::row_evaluated!`).
        row_evaluated!({
            let rejected = matches!(state.matches(raw_row, split, table, row_offset), Ok(false));
            if rejected {
                self.rows.fetch_add(1, Ordering::Relaxed);
            }
            rejected
        })
    }
}
