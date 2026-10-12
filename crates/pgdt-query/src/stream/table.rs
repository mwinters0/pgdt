//! The stream's handle: the pull-mode stream of one table's batches, what
//! it publishes to its owner while it runs, the schema a query would resolve,
//! and the blocking iterator over it.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use futures::Stream;

use pgdt_codec::ResolvedSchema;
use pgdt_core::Result;
use pgdt_model::{CopyBlock, DumpIndex, TableName};

use super::note::{EarlyStop, PlanNote};
use super::replay::plan::plan_table;
use super::resume::ResumeToken;
use crate::options::QueryOptions;
use crate::predicate::ComparisonNote;

/// A pull-mode stream of `RecordBatch`es for one table query.
/// Construct with [`table_stream`](crate::stream::table_stream).
pub struct TableStream<'a> {
    inner: Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send + 'a>>,
    position: Arc<Mutex<ResumeToken>>,
    resolved_schema: Arc<Mutex<ResolvedSchema>>,
    comparison_notes: Arc<Mutex<Vec<ComparisonNote>>>,
    batch_offset: Arc<Mutex<u64>>,
    plan_notes: Arc<Mutex<Vec<PlanNote>>>,
    early_stops: Arc<Mutex<Vec<EarlyStop>>>,
    dynamic_pruned: Arc<AtomicU64>,
    dynamic_rows: Arc<AtomicU64>,
}

impl<'a> Stream for TableStream<'a> {
    type Item = Result<RecordBatch>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<'a> TableStream<'a> {
    /// A token that resumes this stream from just past the last batch
    /// [`futures::StreamExt::next`] returned (or from the start, if nothing
    /// has been polled yet).
    pub fn resume_token(&self) -> ResumeToken {
        self.position.lock().unwrap().clone()
    }

    /// This query's resolved schema and diagnostics — one schema per table,
    /// whichever of its blocks a batch came from
    /// ([`TableColumns`](super::replay::plan::TableColumns)). The empty
    /// schema (`ResolvedSchema::default`) until the query's matching `COPY`
    /// block has been found, which for a table that never appears in the dump
    /// is forever.
    pub fn resolved_schema(&self) -> ResolvedSchema {
        self.resolved_schema.lock().unwrap().clone()
    }

    /// The terms of this query whose comparison does not answer what
    /// PostgreSQL's own operator for that column's type would — one
    /// [`ComparisonNote`] per such term, in term order, empty until this
    /// query's matching `COPY` block has resolved.
    ///
    /// **Per term, because a divergence is operator-conditional**
    /// (`docs/design/decisions.md`, "D59"): most are divergences of *order*
    /// alone (`pgdt_codec::compare::ComparisonDivergence::affects_equality`), so a
    /// `text` column with no `COLLATE` clause earns a note under `<` and none
    /// under `=`. A third channel beside `DumpIndex.diagnostics` (the model)
    /// and `ResolvedSchema.notes` (the codec), each a [`pgdt_model::Finding`] a
    /// caller drains into one sink with the others. Like
    /// [`Self::resolved_schema`], it describes the **last** block whose schema
    /// resolved.
    pub fn comparison_notes(&self) -> Vec<ComparisonNote> {
        self.comparison_notes.lock().unwrap().clone()
    }

    /// Facts about *this query's plan* rather than about a column or a
    /// predicate ([`PlanNoteKind`](super::note::PlanNoteKind)): the memory budget in force declining
    /// something, the row groups statistics let the replay skip, and each
    /// column's values read as NULL ([`PlanNoteKind::ReadAsNull`](crate::stream::note::PlanNoteKind::ReadAsNull)). A fourth
    /// channel beside `DumpIndex.diagnostics` (the model), `ResolvedSchema.notes`
    /// (the codec) and [`Self::comparison_notes`] (query).
    ///
    /// **Settled before any block is read**, from the map and the source's own
    /// advice. A
    /// [`table_stream_partitions`](crate::stream::partitions::table_stream_partitions) sub-stream holds them when it is handed
    /// back; [`table_stream`](crate::stream::table_stream)'s serial replay, which plans no partitions and
    /// so never notes a budget, holds its pruning and read-as-NULL notes once
    /// its first item is polled, and nothing before.
    pub fn plan_notes(&self) -> Vec<PlanNote> {
        self.plan_notes.lock().unwrap().clone()
    }

    /// What the early stop did in each block this stream replays with one
    /// planned ([`EarlyStop`]), in file order — **found while rows are read,
    /// so reported after the fact** where [`Self::plan_notes`] is settled
    /// before. Complete once the stream is drained; from the first poll every
    /// block with a stop planned has an entry, one not yet read holding `None`
    /// as one whose stop saved nothing does, and before it nothing. A block with no stop
    /// planned has no entry: the filter requires no bound its sort order
    /// closes, or statistics were not used — unless a [`DynamicFilter`](crate::stream::replay::dynamic::DynamicFilter)'s
    /// bound stopped it, which adds the block's entry when it leaves a byte
    /// unread.
    pub fn early_stops(&self) -> Vec<EarlyStop> {
        self.early_stops.lock().unwrap().clone()
    }

    /// The row groups this stream's [`DynamicFilter`](crate::stream::replay::dynamic::DynamicFilter) ruled out, so far —
    /// none where it was handed none, and none of the groups
    /// [`PlanNoteKind::StatisticsPruned`](crate::stream::note::PlanNoteKind::StatisticsPruned) counts, which the replay never
    /// reaches. **Each group is counted by one sub-stream**: one the cut at
    /// the first poll ruled out, by the first sub-stream to take that cut
    /// ([`DynamicPartitions`](crate::stream::partitions::DynamicPartitions)); one ruled out as the replay reached it, by the
    /// sub-stream whose piece holds the byte its search starts at
    /// (`D + k·N − 1`, [`crate::prune::BlockPruning::kept`]'s terms), where
    /// that sub-stream skipped it — so a group cut between two sub-streams
    /// counts once at most, and not at all where the first read any of it.
    pub fn dynamic_filter_pruned_groups(&self) -> u64 {
        self.dynamic_pruned.load(Ordering::Relaxed)
    }

    /// The rows this stream's [`DynamicFilter`](crate::stream::replay::dynamic::DynamicFilter) dropped, so far, before
    /// decoding them: rows its static filter kept, in any block, that the
    /// state last read rejects — none under [`RowEvaluation::Off`](crate::stream::replay::dynamic::RowEvaluation::Off). None of
    /// the rows
    /// [`Self::dynamic_filter_pruned_groups`] counts the groups of, which
    /// the replay never reads, nor those past an early stop or in the rest
    /// of a group a state read inside it ruled out.
    pub fn dynamic_filter_pruned_rows(&self) -> u64 {
        self.dynamic_rows.load(Ordering::Relaxed)
    }

    /// Where in the source the batch [`futures::StreamExt::next`] last
    /// returned begins: the offset of its first row, and 0 before anything
    /// has been polled.
    ///
    /// **This is the key a caller merges partitions on**
    /// (`docs/design/decisions.md`, "D51"): a caller holding one batch per
    /// sub-stream and always emitting the lowest of these offsets re-assembles
    /// the serial order. It is a *start*, not the end [`Self::resume_token`]
    /// reports. A batch carrying no rows — reachable only through a `max_rows`
    /// or `max_bytes` of 0 — reports the scanner's position instead, so the value is monotone
    /// within a sub-stream either way.
    pub fn batch_source_offset(&self) -> u64 {
        *self.batch_offset.lock().unwrap()
    }
}

/// The values a [`TableStream`] publishes to its owner while it runs, plus
/// `plan_notes` — settled once, with the plan and before any row, unlike the
/// rest.
#[derive(Clone)]
pub(super) struct StreamShared {
    pub(super) position: Arc<Mutex<ResumeToken>>,
    pub(super) resolved_schema: Arc<Mutex<ResolvedSchema>>,
    pub(super) comparison_notes: Arc<Mutex<Vec<ComparisonNote>>>,
    pub(super) batch_offset: Arc<Mutex<u64>>,
    pub(super) plan_notes: Arc<Mutex<Vec<PlanNote>>>,
    pub(super) early_stops: Arc<Mutex<Vec<EarlyStop>>>,
    pub(super) dynamic_pruned: Arc<AtomicU64>,
    pub(super) dynamic_rows: Arc<AtomicU64>,
}

impl StreamShared {
    pub(super) fn new(token: ResumeToken) -> Self {
        Self {
            position: Arc::new(Mutex::new(token)),
            resolved_schema: Arc::new(Mutex::new(ResolvedSchema::default())),
            comparison_notes: Arc::new(Mutex::new(Vec::new())),
            batch_offset: Arc::new(Mutex::new(0)),
            plan_notes: Arc::new(Mutex::new(Vec::new())),
            early_stops: Arc::new(Mutex::new(Vec::new())),
            dynamic_pruned: Arc::new(AtomicU64::new(0)),
            dynamic_rows: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Attach the plan-level notes, known in full before any row is read and
    /// never changed after.
    pub(super) fn with_plan_notes(self, plan_notes: Vec<PlanNote>) -> Self {
        *self.plan_notes.lock().unwrap() = plan_notes;
        self
    }

    pub(super) fn into_stream<'a>(
        self,
        inner: Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send + 'a>>,
    ) -> TableStream<'a> {
        TableStream {
            inner,
            position: self.position,
            resolved_schema: self.resolved_schema,
            comparison_notes: self.comparison_notes,
            batch_offset: self.batch_offset,
            plan_notes: self.plan_notes,
            early_stops: self.early_stops,
            dynamic_pruned: self.dynamic_pruned,
            dynamic_rows: self.dynamic_rows,
        }
    }
}

/// The schema a query of `table` would resolve over `index`, read off the map
/// and the DDL alone: no byte of the dump is read, and nothing is asked of a
/// source.
///
/// It is the schema every batch of that query carries, projection included
/// ([`TableColumns`](super::replay::plan::TableColumns)), read off the
/// opening every plan of the table shares (`plan_table`), or the refusal its
/// plan would raise — a table whose blocks name different column sets, a
/// projected name no block carries — but for the refuse mode's, which only
/// planning the replay raises, and a projection naming a column twice, which
/// only the query's request check refuses: here the column comes back twice.
/// A table the map holds no block for has the empty schema, as its stream
/// has no rows. **Believe it only over a complete map**: a table's later blocks
/// are part of its census and of its column-set check
/// ([`pgdt_model::DumpIndex::is_complete`]).
pub fn table_schema(
    index: &DumpIndex,
    table: &TableName,
    query_options: &QueryOptions,
) -> Result<ResolvedSchema> {
    let matches: Vec<CopyBlock> = index.blocks_of(table).cloned().collect();
    let (_, blocks) = plan_table(&matches, query_options, index.metadata.as_ref())?;
    Ok(blocks.into_values().next().map(|planned| planned.resolved).unwrap_or_default())
}

/// Blocking [`Iterator`] wrapper over a [`TableStream`], for sync callers
/// with no ambient `tokio` runtime — nesting this inside one
/// (e.g. a `#[tokio::main]` function) panics, same as any other
/// `Runtime::block_on` call.
pub struct BlockingTableIter<'a> {
    stream: TableStream<'a>,
    rt: tokio::runtime::Runtime,
}

impl<'a> BlockingTableIter<'a> {
    pub fn new(stream: TableStream<'a>) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().build()?;
        Ok(Self { stream, rt })
    }

    /// A token that resumes from just past the last batch [`Iterator::next`]
    /// returned.
    pub fn resume_token(&self) -> ResumeToken {
        self.stream.resume_token()
    }
}

impl<'a> Iterator for BlockingTableIter<'a> {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        use futures::StreamExt;
        self.rt.block_on(self.stream.next())
    }
}
