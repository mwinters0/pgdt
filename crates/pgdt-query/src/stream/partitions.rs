//! A partitioned replay of one [`Table`](crate::Table) — the whole table in
//! file order, a serial resume, and its partitions under one dynamic
//! filter.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_stream::try_stream;
use futures::StreamExt;
use tokio::sync::OnceCell;

use pgdt_codec::ResolvedSchema;
use pgdt_core::{Error, Result};
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::SourceWatch;
use pgdt_io::{ByteRangeSource, Partitioning};
use pgdt_model::{BlockStatistics, CopyBlock, DumpMetadata, Sortedness, TableName};

use super::merge;
use super::note::PlanNote;
use super::replay::dynamic::{Cut, DynamicBlock, DynamicFilter, DynamicRead, RowEvaluation};
use super::replay::partition::cut_blocks;
use super::replay::plan::{PlannedReplay, ReplayPlan, plan_replay};
use super::replay::replay;
use super::replay::segment::Segment;
use super::resume::{ResumeToken, query_fingerprint};
use super::table::{StreamShared, TableStream};
use crate::options::QueryOptions;
use crate::summary::partition_orders;

/// A partitioned replay of one [`Table`](crate::Table), planned once
/// ([`Table::plan`](crate::Table::plan)) and streamed per partition, for an
/// embedder that schedules partitions itself and runs them in any order, or
/// more than once (`docs/design/decisions.md`, "D90"), or as one stream in
/// file order ([`Self::in_file_order`]).
///
/// **The mapping pass is over before it exists**: no cache is loaded or
/// written and no byte is scanned for structure, the table's blocks being
/// every block of it a complete map holds, or a cold query's pass settled
/// ([`Table::settle`](crate::Table::settle)).
///
/// The cut is made where the source said it was willing to be cut
/// ([`ByteRangeSource::partitions`]) and no finer than the query's
/// parallelism allows; **what N sub-streams cost resident is N times one**,
/// N solved against what the source charges concurrent readers
/// (`Partitioning::worker_memory`, a shared pool term included) plus, on a
/// source retaining by chunk alone, what a sub-stream's in-flight batch pins
/// (`QueryOptions::max_source_span`) — see `plan_partitions`
/// (`docs/design/decisions.md`, "D4").
///
/// **It owns its source**, so its sub-streams outlive the call that planned
/// them — `'static`, which is what a scheduler holding a plan across tasks
/// needs. Its watch is the dump's: its baseline is what each sub-stream
/// checks the source against when it ends, taken by the open the map was
/// loaded or built under and shared across every replay of that map.
///
/// **It shares each matched block's statistics with the dump's map**, where
/// the query uses them, for a [`DynamicFilter`] to be read against as its
/// sub-streams run ([`Self::under`]): nothing is copied, so they cost
/// nothing the map does not while the dump is held, and a plan kept after
/// the dump is dropped keeps them.
pub struct TablePartitions {
    source: Arc<dyn ByteRangeSource>,
    watch: Arc<SourceWatch>,
    plan: Arc<ReplayPlan>,
    /// The planned cut: each sub-stream's segments, over the row groups the
    /// static filter keeps.
    groups: Arc<Vec<Vec<Segment>>>,
    plan_notes: Vec<PlanNote>,
    orders: Vec<Sortedness>,
    table: String,
    statistics: Arc<BTreeMap<u64, Arc<BlockStatistics>>>,
    /// Every matched block in file order, its statistics held where the query
    /// uses them, and the advice the planned cut was made by: what a dynamic
    /// filter's cut is made from again ([`Self::cut_under`]).
    matches: Vec<CopyBlock>,
    advice: Vec<Partitioning>,
}

impl TablePartitions {
    /// Plan the replay of `table` over `blocks`, every block of it in a
    /// complete map of `source`, against `metadata`, the dump's DDL: its
    /// columns, every block resolved, statistics consulted, and the blocks
    /// cut into as many partitions as `query_options.parallelism` and the
    /// source's own advice allow. A request [`QueryOptions::validate`]
    /// refuses is refused here, and so is a source `watch` cannot check
    /// during a read, where the replay's reads are planned
    /// ([`SourceWatch::require_checkable`]).
    pub(crate) fn new(
        source: Arc<dyn ByteRangeSource>,
        watch: Arc<SourceWatch>,
        metadata: Option<Arc<DumpMetadata>>,
        table: &TableName,
        blocks: &[CopyBlock],
        read: ReadOptions,
        query_options: QueryOptions,
    ) -> Result<Self> {
        query_options.validate()?;
        watch.require_checkable(source.as_ref())?;
        let PlannedReplay { plan, groups, plan_notes, advice } = plan_replay(
            source.as_ref(),
            table.database.clone(),
            blocks,
            metadata,
            read,
            query_options,
        )?;
        let resolved = plan
            .blocks
            .values()
            .next()
            .map(|planned| &planned.resolved)
            .cloned()
            .unwrap_or_default();
        let reading = plan.query_options.statistics_view();
        let orders = partition_orders(
            blocks,
            plan.metadata.as_deref(),
            plan.database.as_deref(),
            &resolved,
            &block_runs(&groups),
            reading,
        );
        let use_statistics = plan.query_options.use_statistics;
        let matches: Vec<CopyBlock> = blocks
            .iter()
            .map(|block| match use_statistics {
                true => block.clone(),
                false => CopyBlock { statistics: None, ..block.clone() },
            })
            .collect();
        let statistics = matches
            .iter()
            .filter_map(|block| Some((block.header_offset, Arc::clone(block.statistics.as_ref()?))))
            .collect();
        Ok(Self {
            source,
            watch,
            plan,
            groups: Arc::new(groups),
            plan_notes,
            orders,
            table: table.qualified_name(),
            statistics: Arc::new(statistics),
            matches,
            advice,
        })
    }

    /// How many sub-streams the plan cut: at least one, and never more than
    /// the parallelism it was planned under allows.
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Always `false`: a plan holds at least one partition, the degenerate one
    /// of a table with no blocks included.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The schema every sub-stream's batches carry, known before any is read.
    pub fn resolved_schema(&self) -> ResolvedSchema {
        self.plan.blocks.values().next().map(|planned| planned.resolved.clone()).unwrap_or_default()
    }

    /// What the plan settled before any row is read ([`TableStream::plan_notes`]).
    pub fn plan_notes(&self) -> &[PlanNote] {
        &self.plan_notes
    }

    /// **Never fewer rows than the plan's partitions emit together**, settled
    /// with the plan and reading nothing: the rows of every row group the
    /// filter's pruning kept ([`pgdt_model::RowGroup::rows`]), plus
    /// every row of each block it did not consult — one without statistics,
    /// or a query that turned them off or whose filter reads no field. It is
    /// a bound and not a count, since a kept group's rows may fail the filter
    /// and a block sorted past its bound stops early; with no filter it is
    /// the table's rows.
    pub fn kept_rows(&self) -> u64 {
        self.plan.kept_rows
    }

    /// **Never fewer text bytes than the plan's partitions emit of each
    /// field together**, parallel to [`Self::resolved_schema`]'s fields and
    /// settled as [`Self::kept_rows`] is: each field's values' text bytes over
    /// the groups pruning kept and every group of a block it did not consult
    /// ([`pgdt_model::ColumnStatistics::value_bytes`]). `None` for a
    /// field some block's statistics did not count.
    pub fn kept_value_bytes(&self) -> &[Option<u64>] {
        &self.plan.kept_value_bytes
    }

    /// **The order every partition emits each column in**, parallel to
    /// [`Self::resolved_schema`]'s fields: `Ascending` or `Descending` where
    /// the map's statistics prove that each partition's rows come out in that
    /// order under Arrow's comparator, with no NULL among them, and
    /// `Unsorted` wherever they do not — which says nothing about the rows,
    /// only about the proof. Settled with the plan, reading nothing
    /// (`summary::partition_orders`).
    pub fn orders(&self) -> &[Sortedness] {
        &self.orders
    }

    /// A fresh stream over partition `partition`, cutting batches at
    /// `max_rows` rows — the one batching knob a scheduler states only when it
    /// runs the partition, and outside the plan it was cut under. Each call
    /// starts the partition from its beginning; a partition past [`Self::len`] is an empty stream.
    pub fn stream(&self, partition: usize, max_rows: usize) -> TableStream<'static> {
        self.sub_stream(partition, max_rows, None)
    }

    /// **The whole table in file order, as one stream**: every partition run
    /// at once, cutting batches at `max_rows` rows, and merged on each
    /// batch's source offset, so it yields exactly the rows and the order a
    /// serial replay does, holding at most one batch per partition
    /// ([`TableStream::batch_source_offset`]). A failure is the lowest-indexed
    /// failing partition's, raised after every row before it
    /// (`docs/design/decisions.md`, "D52").
    ///
    /// Its schema and comparison notes are the plan's, held before the first
    /// poll; its early stops and dynamic-filter counts are its partitions'
    /// together, a block read by two counted once. **Its resume token is the
    /// whole table's**, which [`Self::resume`] takes, where a partition's
    /// resumes nothing (`docs/design/decisions.md`, "D51").
    pub fn in_file_order(&self, max_rows: usize) -> TableStream<'static> {
        let streams = (0..self.len()).map(|partition| self.stream(partition, max_rows)).collect();
        let fingerprint = self.whole_fingerprint();
        let shared = StreamShared::new(ResumeToken::start(fingerprint))
            .with_plan_notes(self.plan_notes.clone());
        self.publish_plan(&shared);
        merge::in_file_order(streams, shared, fingerprint)
    }

    /// The table in file order from just past where `token`'s stream stopped,
    /// **serially**, cutting batches at `max_rows` rows: one stream over the
    /// rest of the plan's segments, whatever cut it was planned under, its
    /// own tokens resuming as [`Self::in_file_order`]'s do. Nothing is mapped,
    /// loaded or saved (`docs/design/decisions.md`, "D49").
    ///
    /// Refused, [`Error::ResumeQueryMismatch`](pgdt_core::Error::ResumeQueryMismatch),
    /// for a token another query produced — another table, filter,
    /// projection, schema mode, semantics or unrepresentable mode — or one
    /// partition's ([`Self::stream`]), which knows nothing of the range it
    /// was confined to.
    pub fn resume(&self, token: ResumeToken, max_rows: usize) -> Result<TableStream<'static>> {
        let fingerprint = self.whole_fingerprint();
        if token.query_fingerprint != fingerprint {
            return Err(Error::ResumeQueryMismatch);
        }
        let plan = Arc::clone(&self.plan);
        let segments: Vec<Segment> = self
            .matches
            .iter()
            .filter(|block| block.end_offset > token.offset)
            .flat_map(|block| plan.segments(block))
            .filter_map(|segment| segment.resumed_at(token.offset))
            .collect();
        // The plan's own notes, not the cut's: one stream reads them.
        let shared = StreamShared::new(token.clone()).with_plan_notes(plan.notes().collect());
        let shared_for_stream = shared.clone();
        let (source, watch) = (Arc::clone(&self.source), Arc::clone(&self.watch));
        let inner = try_stream! {
            let mut rows = Box::pin(replay(
                source.as_ref(),
                plan,
                max_rows,
                segments,
                shared_for_stream,
                Some(token),
                fingerprint,
                None,
            ));
            while let Some(batch) = rows.next().await {
                yield watch.attribute(source.as_ref(), batch).await?;
            }
            drop(rows);
            watch.finish(source.as_ref()).await?;
        };
        Ok(shared.into_stream(Box::pin(inner)))
    }

    /// The stamp of a stream over the whole table under this plan's query,
    /// as no partition's is ([`query_fingerprint`]).
    fn whole_fingerprint(&self) -> u64 {
        query_fingerprint(&self.table, &self.plan.query_options, None)
    }

    /// Publish the plan's schema and comparison notes to `shared` up front,
    /// as a stream over the whole table holds them before its first poll —
    /// every block of a table resolving the same ([`ReplayPlan::new`]).
    fn publish_plan(&self, shared: &StreamShared) {
        if let Some(planned) = self.plan.blocks.values().next() {
            *shared.resolved_schema.lock().unwrap() = planned.resolved.clone();
            *shared.comparison_notes.lock().unwrap() = planned.notes.clone();
        }
    }

    /// These partitions under `filter`, which each of their sub-streams reads
    /// as it runs, against the statistics the plan holds, evaluating each row
    /// against it as `evaluation` says, and whose state when the first of
    /// them is polled decides their byte cut ([`DynamicPartitions`]). Where
    /// the plan does not use statistics it skips no group, and still drops
    /// each row a state rejects where it evaluates rows.
    pub fn under(
        self: &Arc<Self>,
        filter: Arc<dyn DynamicFilter>,
        evaluation: RowEvaluation,
    ) -> DynamicPartitions {
        DynamicPartitions {
            partitions: Arc::clone(self),
            filter,
            evaluation,
            cut: Arc::new(OnceCell::new()),
        }
    }

    /// The cut a [`DynamicPartitions`] makes at its first poll: the matched
    /// blocks cut again into [`Self::len`] sub-streams over the row groups the
    /// static filter and `filter`'s state now both keep, beside how many of
    /// the groups the static filter kept that state rules out.
    ///
    /// **The planned cut stands, ruling nothing out,** where the state rules
    /// out no group — a TopK's or an aggregate's, the empty conjunction until
    /// the scan streams — and where the new cut would lose an order
    /// [`Self::orders`] declared ([`Self::keeps_orders`]).
    fn cut_under(&self, filter: &dyn DynamicFilter) -> Cut {
        let (generation, state) = filter.current();
        let semantics = self.plan.query_options.semantics;
        let mut kept = self.plan.kept.clone();
        let (mut ruled_out, mut verdicts) = (0, BTreeMap::new());
        for block in &self.matches {
            let Some(statistics) = block.statistics.as_ref() else { continue };
            let Some(mut dynamic) = DynamicBlock::new(block, Some(statistics), &self.plan) else {
                continue;
            };
            dynamic.read(generation, &state, semantics, RowEvaluation::Off);
            let Some(pruning) = dynamic.pruning.as_mut() else { continue };
            let within = self.plan.kept.get(&block.header_offset).map(Vec::as_slice);
            let (runs, out) = pruning.kept_within(within);
            verdicts.insert(block.header_offset, pruning.verdicts().to_vec());
            if out > 0 {
                ruled_out += out;
                kept.insert(block.header_offset, runs);
            }
        }
        let cut = (ruled_out > 0)
            .then(|| cut_blocks(&self.matches, &kept, &self.advice, self.groups.len()))
            .filter(|groups| self.keeps_orders(groups));
        let (groups, ruled_out) = match cut {
            Some(groups) => (Arc::new(groups), ruled_out),
            None => (Arc::clone(&self.groups), 0),
        };
        Cut { groups, ruled_out: AtomicU64::new(ruled_out), generation, verdicts }
    }

    /// Whether every sub-stream of `groups` is proved to emit each column
    /// [`Self::orders`] declares ordered in that order — which the planned
    /// cut was, and the caller planned on. **A cut made again can lose it
    /// only across a block boundary**, where it puts two blocks in one
    /// sub-stream that the planned cut kept apart, and nothing proves the
    /// later one's rows follow the earlier one's.
    ///
    /// Deficiency register: `deficiency: KD53` — where it would, the planned
    /// cut stands whole, so a selective join into a table of several blocks
    /// declared in order is read balanced over what its static filter keeps,
    /// not over what the join's filter does. A cut breaking at every block
    /// boundary nothing proves, and balanced between them, would keep both.
    /// **(c) unowned**; promoted by such a table's probe side seen unbalanced.
    fn keeps_orders(&self, groups: &[Vec<Segment>]) -> bool {
        if self.orders.iter().all(|order| *order == Sortedness::Unsorted) {
            return true;
        }
        let (metadata, database) = (self.plan.metadata.as_deref(), self.plan.database.as_deref());
        let resolved = self.resolved_schema();
        let reading = self.plan.query_options.statistics_view();
        let runs = block_runs(groups);
        let proved = partition_orders(&self.matches, metadata, database, &resolved, &runs, reading);
        self.orders
            .iter()
            .zip(&proved)
            .all(|(declared, proved)| *declared == Sortedness::Unsorted || declared == proved)
    }

    /// Partition `partition`'s stream, cutting batches at `max_rows` rows,
    /// over the planned cut — or under `dynamic`, over the cut it makes when
    /// the first of its sub-streams is polled, the groups that cut ruled out
    /// counted as this stream's where it is the first to take it
    /// ([`TableStream::dynamic_filter_pruned_groups`]), and its filter read
    /// as the stream runs, against the statistics the plan holds.
    fn sub_stream(
        &self,
        partition: usize,
        max_rows: usize,
        dynamic: Option<DynamicPartitions>,
    ) -> TableStream<'static> {
        let plan = Arc::clone(&self.plan);
        let fingerprint = query_fingerprint(
            &self.table,
            &plan.query_options,
            Some((partition, self.groups.len())),
        );
        let shared = StreamShared::new(ResumeToken::start(fingerprint))
            .with_plan_notes(self.plan_notes.clone());
        let planned = Arc::clone(&self.groups);
        let statistics = Arc::clone(&self.statistics);
        let shared_for_stream = shared.clone();
        let source = Arc::clone(&self.source);
        let watch = Arc::clone(&self.watch);
        let inner = try_stream! {
            let (groups, dynamic) = match dynamic {
                None => (planned, None),
                Some(under) => {
                    let cut = under.cut().await;
                    let pruned = Arc::clone(&shared_for_stream.dynamic_pruned);
                    pruned.fetch_add(cut.ruled_out.swap(0, Ordering::Relaxed), Ordering::Relaxed);
                    let groups = Arc::clone(&cut.groups);
                    let (filter, evaluation) = (under.filter, under.evaluation);
                    let rows = Arc::clone(&shared_for_stream.dynamic_rows);
                    let read = DynamicRead::new(filter, evaluation, statistics, cut, pruned, rows);
                    (groups, Some(read))
                }
            };
            let segments = groups.get(partition).cloned().unwrap_or_default();
            let mut rows = Box::pin(replay(
                source.as_ref(),
                plan,
                max_rows,
                segments,
                shared_for_stream,
                None,
                fingerprint,
                dynamic,
            ));
            while let Some(batch) = rows.next().await {
                yield watch.attribute(source.as_ref(), batch).await?;
            }
            drop(rows);
            watch.finish(source.as_ref()).await?;
        };
        shared.into_stream(Box::pin(inner))
    }
}

/// Each sub-stream's blocks, by header offset, in the order it reads them.
fn block_runs(groups: &[Vec<Segment>]) -> Vec<Vec<u64>> {
    groups
        .iter()
        .map(|segments| segments.iter().map(|segment| segment.block.header_offset).collect())
        .collect()
}

/// A plan's partitions under one [`DynamicFilter`]
/// ([`TablePartitions::under`]): each sub-stream streamed from it skips the
/// row groups the filter's state rules out as the replay reaches them, ends a
/// block sorted past a bound the state requires, and under
/// [`RowEvaluation::On`] drops each row of the rest the state rejects before
/// decoding it.
///
/// **Their byte cut is made once, when the first of them is polled**, over
/// the row groups the plan's static filter and the filter's state at that
/// moment both keep, so they are byte-balanced over what they read rather
/// than over what the plan kept (`docs/design/decisions.md`, "D51"): a hash
/// join's filter is complete by its probe side's first poll. The count stays
/// the plan's, which the caller planned on, so a sub-stream the new cut
/// leaves nothing to is empty; and the planned cut stands where the state
/// then rules out no group, or where the new cut would put two blocks in one
/// sub-stream that nothing proves in an order [`TablePartitions::orders`]
/// declared. The groups the cut rules out are counted by the first sub-stream
/// to take it.
///
/// Why one handle, and the cut at the first poll: `docs/design/decisions.md`,
/// "D95".
///
/// **The cut lasts as long as the handle, not one run**: a sub-stream
/// streamed again takes the cut already made, which the filter's contract
/// allows — a replay may drop any row a state it read rejects — so a caller
/// whose filter's producer has started over hands it to
/// [`TablePartitions::under`] again.
#[derive(Clone)]
pub struct DynamicPartitions {
    partitions: Arc<TablePartitions>,
    filter: Arc<dyn DynamicFilter>,
    evaluation: RowEvaluation,
    cut: Arc<OnceCell<Arc<Cut>>>,
}

impl DynamicPartitions {
    /// A fresh stream over partition `partition`, as
    /// [`TablePartitions::stream`] makes one, reading the filter as it runs.
    pub fn stream(&self, partition: usize, max_rows: usize) -> TableStream<'static> {
        self.partitions.sub_stream(partition, max_rows, Some(self.clone()))
    }

    /// The cut, made by the first caller.
    async fn cut(&self) -> Arc<Cut> {
        let made = self
            .cut
            .get_or_init(|| async { Arc::new(self.partitions.cut_under(self.filter.as_ref())) });
        Arc::clone(made.await)
    }
}
