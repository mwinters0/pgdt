//! The pull-mode stream of one table's batches, its resume token, the entry
//! points that build it, and the blocking iterator over it.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use async_stream::try_stream;
use futures::{Stream, StreamExt};

use pgdt_codec::UnrepresentableMode;
use pgdt_codec::compare::ComparisonSemantics;
use pgdt_codec::internal::{database_for_name, read_as_text};
use pgdt_codec::{ResolvedSchema, SchemaMode, resolve_columns};
use pgdt_core::CopyHeader;
use pgdt_core::{Error, Result};
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_dump::{CopyScanner, ScanOptions};
use pgdt_io::{ByteRangeSource, Partitioning};
use pgdt_model::{ArrayShape, CopyBlock, DumpIndex, DumpMetadata, TableName, union_census};

use super::map_query::{MappedTable, map_for_query, validate_request};
use super::note::{EarlyStop, PlanNote};
use super::partition::plan_partitions;
use super::plan::{ReplayPlan, plan_blocks, unrepresentable_values};
use super::replay::replay;
use super::resolve::{Active, Opened, query_fingerprint};
use super::segment::{Segment, StreamShared, activate};
use crate::batch::QueryOptions;
use crate::predicate::ComparisonNote;

/// Opaque cursor into a [`table_stream`]/[`crate::batch::read_table`]
/// consumption, sufficient to resume from just past the last batch a caller
/// accepted. Valid only within the process that produced it; persisting one
/// across a restart is out of scope (`docs/design/roadmap.md`).
#[derive(Debug, Clone)]
pub struct ResumeToken {
    pub(super) offset: u64,
    pub(super) rows_emitted: u64,
    /// Stamp of the query this token came out of — see [`query_fingerprint`].
    /// Resuming a stream whose options hash differently is
    /// `Error::ResumeQueryMismatch`, which is what defends "one schema per
    /// stream, resolved up front".
    pub(super) query_fingerprint: u64,
    /// Reserved for the structural cache's generation stamp; the cache does
    /// not stamp generations, so this is always 0.
    #[allow(dead_code)]
    generation: u64,
    pub(super) in_copy: Option<InCopyResume>,
}

#[derive(Debug, Clone)]
pub(super) struct InCopyResume {
    header: CopyHeader,
    pub(super) header_offset: u64,
    rows_in_block: u64,
    database: Option<String>,
}

impl ResumeToken {
    pub(super) fn start(query_fingerprint: u64) -> Self {
        Self { offset: 0, rows_emitted: 0, query_fingerprint, generation: 0, in_copy: None }
    }
}

/// A pull-mode stream of `RecordBatch`es for one table query.
/// Construct with [`table_stream`].
pub struct TableStream<'a> {
    pub(super) inner: Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send + 'a>>,
    pub(super) position: Arc<Mutex<ResumeToken>>,
    pub(super) resolved_schema: Arc<Mutex<ResolvedSchema>>,
    pub(super) comparison_notes: Arc<Mutex<Vec<ComparisonNote>>>,
    pub(super) batch_offset: Arc<Mutex<u64>>,
    pub(super) plan_notes: Arc<Mutex<Vec<PlanNote>>>,
    pub(super) early_stops: Arc<Mutex<Vec<EarlyStop>>>,
    pub(super) dynamic_pruned: Arc<AtomicU64>,
    pub(super) dynamic_rows: Arc<AtomicU64>,
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
    /// whichever of its blocks a batch came from ([`TableColumns`]). The empty
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
    /// column's values read as NULL ([`PlanNoteKind::ReadAsNull`](super::note::PlanNoteKind::ReadAsNull)). A fourth
    /// channel beside `DumpIndex.diagnostics` (the model), `ResolvedSchema.notes`
    /// (the codec) and [`Self::comparison_notes`] (query).
    ///
    /// **Settled before any block is read**, from the map and the source's own
    /// advice. A
    /// [`table_stream_partitions`] sub-stream holds them when it is handed
    /// back; [`table_stream`]'s serial replay, which plans no partitions and
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
    /// closes, or statistics were not used — unless a [`DynamicFilter`](super::dynamic::DynamicFilter)'s
    /// bound stopped it, which adds the block's entry when it leaves a byte
    /// unread.
    pub fn early_stops(&self) -> Vec<EarlyStop> {
        self.early_stops.lock().unwrap().clone()
    }

    /// The row groups this stream's [`DynamicFilter`](super::dynamic::DynamicFilter) ruled out, so far —
    /// none where it was handed none, and none of the groups
    /// [`PlanNoteKind::StatisticsPruned`](super::note::PlanNoteKind::StatisticsPruned) counts, which the replay never
    /// reaches. **Each group is counted by one sub-stream**: one the cut at
    /// the first poll ruled out, by the first sub-stream to take that cut
    /// ([`DynamicPartitions`](super::table_partitions::DynamicPartitions)); one ruled out as the replay reached it, by the
    /// sub-stream whose piece holds the byte its search starts at
    /// (`D + k·N − 1`, [`crate::prune::BlockPruning::kept`]'s terms), where
    /// that sub-stream skipped it — so a group cut between two sub-streams
    /// counts once at most, and not at all where the first read any of it.
    pub fn dynamic_filter_pruned_groups(&self) -> u64 {
        self.dynamic_pruned.load(Ordering::Relaxed)
    }

    /// The rows this stream's [`DynamicFilter`](super::dynamic::DynamicFilter) dropped, so far, before
    /// decoding them: rows its static filter kept, in any block, that the
    /// state last read rejects — none under [`RowEvaluation::Off`](super::dynamic::RowEvaluation::Off). None of
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

/// Build the [`ResolvedSchema`] for a table-matching block — the actual batch
/// schema a [`RowBatcher`](crate::batch::RowBatcher) built from it carries (see
/// [`TableStream::resolved_schema`]) — scoped to `database`, the block's own
/// attribution, never a guess. Its
/// fields are the header's list, in the order its rows carry them, and
/// `census` is one entry per name, taken
/// from the union over **every block this stream will replay**, a parameter
/// rather than something `resolve_columns` looks up so that no call site can
/// silently disagree (`docs/design/decisions.md`, "D35").
///
/// `Typed` mode against metadata that has no *complete* entry for `database`
/// is `Error::MetadataNotScanned` rather than a silent `NotDeclared`
/// degradation. **No map a mapping pass produced can trip it**: a block
/// enters the map only past its database's first `COPY` header, where the
/// pass marks that database's DDL complete. `table_schema` and
/// `TablePartitions::plan` take a caller's `DumpIndex`, whose fields are
/// public, so a hand-built one can. The check is pinned by a unit test.
///
/// `text` marks, as `census` is taken, each column the untyped mode reads as
/// its text ([`TableColumns::text_for`]), compared in `semantics`
/// ([`read_as_text`]).
pub(super) fn resolve_block(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    schema_mode: SchemaMode,
    census: &[ArrayShape],
    text: &[bool],
    semantics: ComparisonSemantics,
) -> Result<ResolvedSchema> {
    if schema_mode == SchemaMode::Typed
        && let Some(meta) = metadata
        && !meta.databases.iter().any(|db| db.name.as_deref() == database && db.preamble_complete)
    {
        return Err(Error::MetadataNotScanned { database: database.map(str::to_string) });
    }
    let mut resolved = resolve_columns(
        &header.qualified_name(),
        &header.columns,
        metadata,
        database,
        schema_mode,
        census,
    );
    read_as_text(&mut resolved, text, semantics);
    Ok(resolved)
}

/// Refuse a typed plan over a block holding no census — one a map the caller
/// holds records at the metadata level ([`Error::TableAtMetadataLevel`]),
/// whose union ([`union_census`]) would read as a table holding no array
/// deeper than its DDL says (`docs/design/decisions.md`, "D35"). A query that
/// maps for itself has censused every block before it plans
/// (`census_metadata_level`); [`SchemaMode::Strings`] reads no census.
pub(super) fn refuse_metadata_level(
    matches: &[CopyBlock],
    query_options: &QueryOptions,
) -> Result<()> {
    if query_options.schema_mode != SchemaMode::Typed {
        return Ok(());
    }
    match matches.iter().find(|block| block.array_shapes.is_none()) {
        Some(block) => Err(Error::TableAtMetadataLevel { table: block.header.qualified_name() }),
        None => Ok(()),
    }
}

/// **One table, one schema**: the column order every batch of a table
/// carries, whichever of its blocks the rows came from, and the census keyed
/// to it.
///
/// A table can own several blocks (I2), each listing its columns in its own
/// leaf's order (I5), so a block's batches are reordered by name into the
/// table's order — the table's DDL order where the dump declares the table,
/// the first block's order where it does not — and a table whose blocks name
/// different *sets* of columns is refused, naming two of them. Neither the
/// order nor the refusal reads [`SchemaMode`], so a table's columns come out
/// the same with typing on or off (`docs/design/decisions.md`, "D66").
///
/// **A block naming no columns copies none** (I5: `pg_dump` lists none
/// exactly when every column is dropped or generated), so its set is the
/// empty one: a table whose blocks list nothing has an empty order whatever
/// its DDL declares, and each of its rows is an empty line read as a row of
/// zero fields ([`crate::batch::RowBatcher::push_row`]).
///
/// *Rejected: the first block's order, every other refused*, which refuses
/// ordinary partitioned dumps; *the union of the names, an absent column
/// filled with `NULL`*, which states values PostgreSQL never held; *and a
/// list-less block taking the declared names where its width matches*, which
/// reads an empty line as one field and gives a table of generated columns
/// `''` for every value.
#[derive(Debug, Clone)]
pub(super) struct TableColumns {
    pub(super) order: Vec<String>,
    /// One entry per name in `order`, its census unioned over every block
    /// ([`union_census`]).
    census: Vec<(String, ArrayShape)>,
    /// **The columns the untyped mode reads as their text**: under
    /// [`UnrepresentableMode::Text`], each whose blocks count, over every one
    /// of them, a value in the tiers the query's front end cannot hold — the
    /// count the refuse mode refuses by (`materialized_unrepresentable`),
    /// settled once for the table as its census is, so no two blocks of it
    /// disagree on a column's type (`docs/design/decisions.md`, "D100").
    /// Empty under every other mode.
    text: Vec<String>,
}

impl TableColumns {
    /// The table `matches` are the blocks of — one `(database, table)`
    /// target, already narrowed (`docs/design/decisions.md`, "D49") — as
    /// `query_options` reads it.
    pub(super) fn settle(
        matches: &[CopyBlock],
        metadata: Option<&DumpMetadata>,
        query_options: &QueryOptions,
    ) -> Result<Self> {
        let Some(first) = matches.first() else {
            return Ok(Self { order: Vec::new(), census: Vec::new(), text: Vec::new() });
        };
        fn set(b: &CopyBlock) -> Vec<&String> {
            let mut names: Vec<&String> = b.header.columns.iter().collect();
            names.sort();
            names
        }
        let first_set = set(first);
        if let Some(other) = matches.iter().find(|b| set(b) != first_set) {
            return Err(Error::TableColumnsDisagree {
                table: first.header.qualified_name(),
                header_offset: first.header_offset,
                other_offset: other.header_offset,
            });
        }
        let declared: Option<Vec<&str>> = metadata
            .and_then(|metadata| database_for_name(metadata, first.database.as_deref()))
            .map(|db| db.declared_columns(&first.header.qualified_name()))
            .map(|columns| columns.iter().map(|c| c.name.as_str()).collect());
        // Stable, so a name the DDL does not declare keeps the first block's
        // place for it, after every declared one.
        let mut order = first.header.columns.clone();
        let rank = |name: &String| declared.as_ref().and_then(|d| d.iter().position(|c| c == name));
        order.sort_by_key(|name| rank(name).unwrap_or(usize::MAX));
        let census = order.iter().cloned().zip(union_census(&order, matches)).collect();
        let text = match (query_options.schema_mode, query_options.unrepresentable) {
            (SchemaMode::Typed, UnrepresentableMode::Text) => {
                let reach = query_options.unrepresentable_reach();
                order
                    .iter()
                    .filter(|name| unrepresentable_values(matches, name, reach) > 0)
                    .cloned()
                    .collect()
            }
            _ => Vec::new(),
        };
        Ok(Self { order, census, text })
    }

    /// Which of `names` the untyped mode reads as text, in `names`' order.
    pub(super) fn text_for(&self, names: &[String]) -> Vec<bool> {
        names.iter().map(|name| self.text.contains(name)).collect()
    }

    /// `names`' census, one entry per name, in `names`' order.
    pub(super) fn census_for(&self, names: &[String]) -> Vec<ArrayShape> {
        names
            .iter()
            .map(|name| {
                self.census.iter().find(|(n, _)| n == name).map(|(_, s)| *s).unwrap_or_default()
            })
            .collect()
    }
}

/// Reconstruct the in-progress block state a [`ResumeToken`] captured, if any:
/// the scanner's row counter (so a later `CopyEnd` reports the block's true
/// total) and a fresh [`RowBatcher`](crate::batch::RowBatcher) on the original block's schema.
pub(super) fn resume_state(
    token: &ResumeToken,
    plan: &ReplayPlan,
) -> Result<(CopyScanner, Option<Opened>)> {
    let scanner = CopyScanner::resume(
        token.offset,
        token.in_copy.as_ref().map(|ic| (ic.header_offset, ic.rows_in_block)),
    );
    let active = token
        .in_copy
        .as_ref()
        .map(|ic| activate(ic.header.clone(), ic.header_offset, ic.database.clone(), plan))
        .transpose()?;
    Ok((scanner, active))
}

pub(super) fn snapshot(
    scanner: &CopyScanner,
    active: &Option<Active>,
    rows_emitted: u64,
    query_fingerprint: u64,
) -> ResumeToken {
    let in_copy = active.as_ref().map(|(header_offset, header, _, _, database)| InCopyResume {
        header: header.clone(),
        header_offset: *header_offset,
        rows_in_block: scanner.in_copy_rows().unwrap_or(0),
        database: database.clone(),
    });
    ResumeToken {
        offset: scanner.position(),
        rows_emitted,
        query_fingerprint,
        generation: 0,
        in_copy,
    }
}

/// Pull-mode entry point: stream `RecordBatch`es for every row of
/// every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`]). A table with zero rows yields no batches.
///
/// `resume` continues a previous consumption from a [`ResumeToken`] it
/// produced; `None` starts from the beginning of `source`. A token whose query
/// fingerprint disagrees with `query_options` is `Error::ResumeQueryMismatch`.
///
/// `query_options.filter` is the post-parse row filter, one `Expr` tree
/// (`docs/design/decisions.md`, "D54"): the empty conjunction yields every row,
/// and otherwise a row is kept only where the tree is `True`, tested on the raw
/// row, each term unescaping only the field it reads (D28). A term referencing a column absent from a
/// matching block's own schema is `Error::UnknownPredicateColumn`. Every
/// refusal resolving a block raises — schema, filter or projection — is
/// yielded before any row of the table, for the first refusing block in file
/// order (`docs/design/decisions.md`, "D54"), and so is
/// `Error::TableColumnsDisagree`, for a table whose blocks name different
/// columns ([`TableColumns`]).
///
/// `query_options.projection` decides which columns are materialized
/// (`docs/design/decisions.md`, "D28"). It cuts the schema
/// [`TableStream::resolved_schema`] reports as well as the batches, may
/// reorder, and may be empty — a zero-column projection yields batches
/// carrying a row count and nothing else. A filter term may name a column the
/// projection does not.
///
/// `cache` controls structure-cache consulting
/// (`docs/design/decisions.md`, "D22"): `CacheMode::Enabled` persists the map
/// as the mapping pass advances, `CacheMode::Disabled` runs identically but
/// writes nothing. Either way rows come from replaying mapped blocks, never
/// from the mapping pass itself. `query_options.scan_extent` decides how much
/// of the file that pass walks before any row comes back; see [`ScanExtent`](crate::batch::ScanExtent).
pub fn table_stream<'a>(
    source: &'a dyn ByteRangeSource,
    table: &str,
    scan_options: ScanOptions,
    query_options: QueryOptions,
    resume: Option<ResumeToken>,
    cache: CacheMode,
) -> TableStream<'a> {
    let table = table.to_string();
    let fingerprint = query_fingerprint(&table, &query_options, None);
    let start_token = resume.clone().unwrap_or_else(|| ResumeToken::start(fingerprint));
    let shared = StreamShared::new(start_token);
    let shared_for_stream = shared.clone();

    let inner = try_stream! {
        validate_request(&query_options, resume.as_ref(), fingerprint)?;
        // Taken before the map is read, so a warm query — which reaches no
        // save at all — still has something to compare against when it
        // finishes (`pgdt_dump::cache::SourceWatch`).
        let watch = SourceWatch::open(source, cache.strict_identity()).await?;
        let mapped =
            map_for_query(source, &table, &scan_options, &query_options, &cache, &watch).await;
        let mapped = watch.attribute(source, mapped).await?;

        // Pass 2: replay each matching block for its rows, as one segment
        // per kept run. A resumed stream picks up inside this same list, every
        // resume point being inside a mapped block by construction.
        let MappedTable { matches, metadata } = mapped;
        let plan = Arc::new(ReplayPlan::new(scan_options, query_options, &matches, metadata)?);
        *shared_for_stream.plan_notes.lock().unwrap() = plan.notes().collect();
        let resume_offset = resume.as_ref().map_or(0, |t| t.offset);
        let segments: Vec<Segment> = matches
            .iter()
            .filter(|b| b.end_offset > resume_offset)
            .flat_map(|b| plan.segments(b))
            .filter_map(|segment| segment.resumed_at(resume_offset))
            .collect();
        // Dropped before the replay rather than at the stream's end: the
        // matches hold the blocks' statistics, which the segments do not
        // ([`Segment::new`]).
        drop(matches);
        let mut rows =
            Box::pin(replay(source, plan, segments, shared_for_stream, resume, fingerprint, None));
        while let Some(batch) = rows.next().await {
            yield watch.attribute(source, batch).await?;
        }
        // The run's last word: the rows are out, so what this recovers is a
        // failure naming the cause in place of a silent wrong answer.
        watch.finish(source).await?;
    };

    shared.into_stream(Box::pin(inner))
}

/// The same query as [`table_stream`], handed back as **N sub-streams over one
/// map** — the partitioned replay
/// (`docs/design/decisions.md`, "D51").
///
/// The mapping pass runs once, here, before any sub-stream exists, and so
/// does the plan, whose resolution refusal this returns rather than a
/// sub-stream ([`table_stream`]'s filter paragraph); each
/// sub-stream then replays a contiguous run of the blocks that pass settled,
/// cut where the source said it was willing to be cut
/// ([`ByteRangeSource::partitions`]) and no finer than
/// `query_options.parallelism` allows. **The caller runs them**, concurrently
/// or not: running them in order and concatenating is exactly what
/// [`table_stream`] yields. The returned `Vec` is never empty and never holds
/// an empty sub-stream beyond the degenerate one a table with no blocks
/// produces; the serial state is one sub-stream.
///
/// **Each sub-stream carries its own schema, notes and position.**
/// [`TableStream::resolved_schema`] is empty on a sub-stream until that
/// sub-stream's first block resolves, so a caller wanting the schema before
/// consuming much reads it off the *first* sub-stream after its first poll:
/// that sub-stream's first segment is the first block's and publishes its
/// schema on entry, holding a row or not. [`TableStream::resume_token`] is stamped with the
/// partition it came from, so feeding one back to [`table_stream`] is
/// `Error::ResumeQueryMismatch`: resuming a partitioned replay is not
/// supported.
///
/// **What N sub-streams cost resident is N times one**, and N is solved
/// against what the source charges concurrent readers
/// (`Partitioning::worker_memory`, a shared pool term included) plus, on a
/// source retaining by chunk alone, what a sub-stream's in-flight batch pins
/// (`QueryOptions::max_source_span`) — see [`plan_partitions`]
/// (`docs/design/decisions.md`, "D4").
pub async fn table_stream_partitions<'a>(
    source: &'a dyn ByteRangeSource,
    table: &str,
    scan_options: ScanOptions,
    query_options: QueryOptions,
    cache: CacheMode,
) -> Result<Vec<TableStream<'a>>> {
    validate_request(&query_options, None, 0)?;
    let table = table.to_string();
    let watch = Arc::new(SourceWatch::open(source, cache.strict_identity()).await?);
    let mapped = map_for_query(source, &table, &scan_options, &query_options, &cache, &watch).await;
    let mapped = watch.attribute(source, mapped).await?;
    let MappedTable { matches, metadata } = mapped;
    let PlannedReplay { plan, groups, plan_notes, .. } =
        plan_replay(source, &matches, metadata, scan_options, query_options)?;
    let of = groups.len();
    Ok(groups
        .into_iter()
        .enumerate()
        .map(|(index, segments)| {
            let fingerprint = query_fingerprint(&table, &plan.query_options, Some((index, of)));
            let shared = StreamShared::new(ResumeToken::start(fingerprint))
                .with_plan_notes(plan_notes.clone());
            let plan = Arc::clone(&plan);
            let shared_for_stream = shared.clone();
            // **Once per sub-stream, not once per replay.** Each one is a
            // reader that ends, and the first to notice aborts; a check that
            // fired only on the last would let the others report rows read
            // from a file that had already moved.
            let watch = Arc::clone(&watch);
            let inner = try_stream! {
                let mut rows = Box::pin(replay(
                    source,
                    plan,
                    segments,
                    shared_for_stream,
                    None,
                    fingerprint,
                    None,
                ));
                while let Some(batch) = rows.next().await {
                    yield watch.attribute(source, batch).await?;
                }
                watch.finish(source).await?;
            };
            shared.into_stream(Box::pin(inner))
        })
        .collect())
}

/// The plan and the sub-streams' segments for replaying a table's blocks
/// ([`plan_replay`]).
pub(super) struct PlannedReplay {
    pub(super) plan: Arc<ReplayPlan>,
    /// The blocks cut into sub-streams ([`plan_partitions`]).
    pub(super) groups: Vec<Vec<Segment>>,
    /// What every sub-stream carries ([`TableStream::plan_notes`]).
    pub(super) plan_notes: Vec<PlanNote>,
    /// Each block's advice, in file order, as the cut was made by.
    pub(super) advice: Vec<Partitioning>,
}

/// The plan and the sub-streams' segments for replaying `matches`, shared by
/// [`table_stream_partitions`] and [`TablePartitions::plan`](super::table_partitions::TablePartitions::plan).
pub(super) fn plan_replay(
    source: &dyn ByteRangeSource,
    matches: &[CopyBlock],
    metadata: Option<DumpMetadata>,
    scan_options: ScanOptions,
    query_options: QueryOptions,
) -> Result<PlannedReplay> {
    let mut plan = ReplayPlan::new(scan_options, query_options, matches, metadata)?;
    let (groups, mut plan_notes, span, advice) = plan_partitions(
        source,
        matches,
        &plan.kept,
        plan.query_options.parallelism,
        plan.query_options.max_source_span,
        plan.scan_options.chunk_size_bytes,
    );
    // **The span the plan was solved against is the span the batches use**, or
    // the charge bounds nothing (`docs/design/decisions.md`, "D84"). It is
    // outside the resume fingerprint, being a batching knob
    // (`docs/design/decisions.md`, "D49"), so writing it back moves no token.
    plan.query_options.max_source_span = span;
    plan_notes.extend(plan.notes());
    Ok(PlannedReplay { plan: Arc::new(plan), groups, plan_notes, advice })
}

/// The schema a query of `table` would resolve over `index`, read off the map
/// and the DDL alone: no byte of the dump is read, and nothing is asked of a
/// source.
///
/// It is the schema every batch of that query carries, projection included
/// ([`TableColumns`]), or the refusal its plan would raise — a table whose
/// blocks name different column sets, a projected name no block carries —
/// but for the refuse mode's, which only planning the replay raises. A
/// table the map holds no block for has the empty schema, as its stream has
/// no rows. **Believe it only over a complete map**: a table's later blocks
/// are part of its census and of its column-set check
/// ([`pgdt_model::DumpIndex::is_complete`]).
pub fn table_schema(
    index: &DumpIndex,
    table: &TableName,
    query_options: &QueryOptions,
) -> Result<ResolvedSchema> {
    let matches: Vec<CopyBlock> = index.blocks_of(table).cloned().collect();
    refuse_metadata_level(&matches, query_options)?;
    let metadata = index.metadata.as_ref();
    let columns = TableColumns::settle(&matches, metadata, query_options)?;
    let blocks = plan_blocks(&matches, query_options, metadata, &columns)?;
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
