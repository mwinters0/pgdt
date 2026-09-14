//! Pull-mode streaming API (`docs/design/decisions.md`, "Batches, streams and
//! the leader").
//!
//! [`table_stream`] is the primitive: an async `Stream<Item =
//! Result<RecordBatch>>` built directly on [`CopyScanner`]/[`RowBatcher`], the
//! same machinery [`crate::batch::read_table`] (push mode) drives internally.
//! [`ResumeToken`] resumes a consumption within the same process; it holds no
//! public fields (`docs/design/decisions.md`, "D50").
//!
//! **Mapping and streaming are separate passes**
//! (`docs/design/decisions.md`, "D48"). A query runs in two phases, never
//! interleaved:
//!
//! 1. [`map_forward`] extends the [`DumpIndex`]'s map from its own
//!    `scanned_through`, recording every `COPY` block it passes and
//!    classifying the DDL between them through a [`crate::map::Builder`], and
//!    **yields nothing**. It stops as soon as the queried table is settled
//!    ([`ScanExtent::UntilTargetSettled`]).
//! 2. Every block the map holds for that table is then replayed for its rows,
//!    in file order.
//!
//! So the queried block's bytes are read twice, and a [`ResumeToken`] can only
//! ever point inside already-mapped territory. A [`CacheMode::Enabled`] cache
//! is persisted where the map advances: completed blocks whose save has earned
//! its cost ([`SaveThrottle`]), the block that settles the query, and every
//! exit. [`CacheMode::Disabled`] runs the same way with `save` a no-op.
//!
//! **Preamble capture** (`docs/design/decisions.md`, "D30"): before any of
//! that, [`table_stream`] runs [`crate::index::scan_preamble`] once (skipped
//! once a cache already has it), whatever table was queried and under
//! [`CacheMode::Disabled`] too. It covers the *first* database; every later
//! `\connect`ed one is stated by [`map_forward`] at that database's first
//! `COPY` block (I1).
//!
//! **Type resolution**: a matching `COPY` block's column list is resolved
//! against that captured metadata into a [`crate::resolve::ResolvedSchema`]
//! ([`TableStream::resolved_schema`]). That is a preview, not what decodes a
//! row: the `RecordBatch`es this stream yields stay all-`Utf8View`.

use std::collections::BTreeMap;
use std::ops::Range;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arrow::array::RecordBatch;
use arrow::datatypes::Schema;
use async_stream::try_stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};

use crate::batch::{QueryOptions, RetainedChunks, RowBatcher, ScanExtent, column_names};
use crate::cache::{CacheLoad, CacheMode};
use crate::copy::{CopyHeader, DELIMITER, RawRow, RowSplit, validated_prefix};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::gather;
use crate::index::{
    ArrayShape, CopyBlock, DumpIndex, scan_preamble, tiling_diagnostics, toc_coverage_diagnostic,
    union_census,
};
use crate::io::{
    ByteRangeSource, DEFAULT_MEMORY_BUDGET, Parallelism, PartitionBoundaries, Partitioning,
    RetainedUnit, WaitPolicy, WorkerMemory, memory_budget_display,
};
use crate::leader::{self, RegionScan};
use crate::map::{Builder, DataBlock, Span, SpanBody, attach_text};
use crate::preamble::{DumpMetadata, dump_metadata_from_spans};
use crate::predicate::{ComparisonNote, Expr, PredicateOp, ResolvedExpr, resolve_term};
use crate::prune::prune_block;
use crate::resolve::{ResolvedSchema, SchemaMode, resolve_columns};
use crate::scan::{ChunkCarry, CopyEnd, CopyScanner, Event, Row, ScanOptions};
use crate::statistics::{BlockObserver, BlockStatistics, StatisticsBackfill, StatisticsRequest};
use crate::{Error, Result};

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, `QueryOptions::filter` resolved against this block's
/// own schema (schemas can differ block-to-block), and the database this block
/// is attributed to (`docs/design/decisions.md`, "D49"). Each
/// [`ResolvedExpr`] leaf carries the field index it reads — into the block's
/// **unprojected** column list — plus the typed comparison it makes. Shared
/// with the block's [`PlannedBlock`], so every piece of a block evaluates one
/// tree.
type Active = (u64, CopyHeader, RowBatcher, Arc<ResolvedExpr>, Option<String>);

/// What [`activate`] hands back: the block's state, and the schema and notes
/// a stream publishes for it.
type Opened = (Active, ResolvedSchema, Vec<ComparisonNote>);

/// Where `row` sits inside `prefix`, the UTF-8-validated leading part of the
/// span it was scanned out of — `None` when the row runs past it, which is
/// every row of a span whose validation failed and none of one whose
/// validation held. `str::get` rather than an index, so a mis-derived offset
/// costs the row its fast path instead of panicking.
fn row_text<'a>(prefix: &'a str, span_base: u64, row: &Row<'_>) -> Option<&'a str> {
    let start = usize::try_from(row.offset.checked_sub(span_base)?).ok()?;
    prefix.get(start..start.checked_add(row.raw.len())?)
}

/// `database`, rendered as `database.schema.table`, or just `schema.table`
/// when the file had no `\connect` at all — the form `Error::AmbiguousTable`
/// names its candidates in.
fn render_candidate((database, qualified_name): &(Option<String>, String)) -> String {
    match database {
        Some(db) => format!("{db}.{qualified_name}"),
        None => qualified_name.clone(),
    }
}

/// Resolve `filter` against `resolved` — the block's own **unprojected**
/// schema — once per block, returning the same tree with every leaf
/// resolved. The empty conjunction resolves to an empty conjunction.
///
/// **This is where a predicate is validated against a block**, and the only
/// place (`docs/design/decisions.md`, "D54"): a term naming a column this
/// block does not carry is `Error::UnknownPredicateColumn`, an ordering
/// operator on a column that is not `Mapped` with a `NestedPlan::Scalar` plan
/// is `Error::UnorderedPredicateColumn`, and a literal that is not a value of
/// the column's type — `=` included — is `Error::PredicateValueDecode`. All
/// are raised for the first offending term in a left-to-right walk. The plan
/// runs it for every block with a column list before any is read, and raises
/// the first refusing block's refusal in file order before any row of the
/// table ([`plan_blocks`]); only a block with no column list refuses where it
/// is reached. It takes the whole [`ResolvedSchema`] because the ordering
/// refusal reads `columns` and `plans` too.
fn resolve_expr(
    filter: &Expr,
    resolved: &ResolvedSchema,
    header_offset: u64,
) -> Result<ResolvedExpr> {
    let branch = |children: &[Expr]| {
        children
            .iter()
            .map(|child| resolve_expr(child, resolved, header_offset))
            .collect::<Result<Vec<_>>>()
    };
    Ok(match filter {
        Expr::Term(predicate) => {
            let index = resolved
                .schema
                .fields()
                .iter()
                .position(|f| f.name() == &predicate.column)
                .ok_or_else(|| Error::UnknownPredicateColumn {
                    header_offset,
                    column: predicate.column.clone(),
                })?;
            ResolvedExpr::Term(resolve_term(predicate, index, resolved, header_offset)?)
        }
        Expr::And(children) => ResolvedExpr::And(branch(children)?),
        Expr::Or(children) => ResolvedExpr::Or(branch(children)?),
        Expr::Not(inner) => {
            ResolvedExpr::Not(Box::new(resolve_expr(inner, resolved, header_offset)?))
        }
    })
}

/// Cut `resolved` down to `projection`, and say which of the block's fields
/// each projected column is fed by. All five of [`ResolvedSchema`]'s vectors
/// are cut together, in the requested order
/// (`docs/design/decisions.md`, "D28").
///
/// Returns the projected schema and `field_targets` — one entry per field of
/// the block, `Some(i)` when that field feeds projected column `i`. `None`
/// projection is every column, in file order. Duplicate names are the caller's
/// to reject: this resolves each requested name independently and would
/// silently accept one twice.
fn project(
    resolved: &ResolvedSchema,
    projection: Option<&[String]>,
    header_offset: u64,
) -> Result<(ResolvedSchema, Vec<Option<usize>>)> {
    let width = resolved.schema.fields().len();
    let Some(names) = projection else {
        return Ok((resolved.clone(), (0..width).map(Some).collect()));
    };
    let mut field_targets = vec![None; width];
    let mut sources = Vec::with_capacity(names.len());
    for name in names {
        let source =
            resolved.schema.fields().iter().position(|f| f.name() == name).ok_or_else(|| {
                Error::UnknownProjectionColumn { header_offset, column: name.clone() }
            })?;
        field_targets[source] = Some(sources.len());
        sources.push(source);
    }
    let projected = ResolvedSchema {
        schema: Arc::new(Schema::new(
            sources.iter().map(|&i| resolved.schema.field(i).clone()).collect::<Vec<_>>(),
        )),
        columns: sources.iter().map(|&i| resolved.columns[i].clone()).collect(),
        notes: sources.iter().map(|&i| resolved.notes[i].clone()).collect(),
        plans: sources.iter().map(|&i| resolved.plans[i].clone()).collect(),
        comparisons: sources.iter().map(|&i| resolved.comparisons[i].clone()).collect(),
    };
    Ok((projected, field_targets))
}

/// A [`ResumeToken`]'s stamp of the query that produced it: the table, the
/// projection, the filter terms, the schema mode and — for a sub-stream of a
/// partitioned replay — which partition of how many it came out of
/// (`docs/design/decisions.md`, "D50"). The hasher's output is not stable
/// across Rust releases: a token is valid only within its own process.
///
/// **`partition` is what keeps a sub-stream's token from resuming as a whole
/// one.** A token carries an offset and nothing about the range its stream was
/// confined to, so stamping the partition turns what would be a silent
/// superset into `Error::ResumeQueryMismatch`
/// (`docs/design/decisions.md`, "D51").
fn query_fingerprint(
    table: &str,
    options: &QueryOptions,
    partition: Option<(usize, usize)>,
) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    table.hash(&mut hasher);
    match partition {
        None => 0u8.hash(&mut hasher),
        Some((index, of)) => {
            1u8.hash(&mut hasher);
            index.hash(&mut hasher);
            of.hash(&mut hasher);
        }
    }
    match options.schema_mode {
        SchemaMode::Typed => 0u8,
        SchemaMode::Strings => 1u8,
    }
    .hash(&mut hasher);
    match &options.projection {
        None => 0u8.hash(&mut hasher),
        Some(columns) => {
            1u8.hash(&mut hasher);
            columns.hash(&mut hasher);
        }
    }
    hash_expr(&options.filter, &mut hasher);
    hasher.finish()
}

/// Fold one filter expression into `hasher`, node kind first, then arity,
/// then each child in order — so two trees of different shape cannot collide
/// by carrying the same terms. Two conjunctions differing only in term order
/// fingerprint differently; canonicalizing instead would make the stamp depend
/// on an ordering rule of its own (`docs/design/decisions.md`, "D50").
fn hash_expr<H: std::hash::Hasher>(expr: &Expr, hasher: &mut H) {
    use std::hash::Hash;

    match expr {
        Expr::Term(term) => {
            0u8.hash(hasher);
            term.column.hash(hasher);
            match term.op {
                PredicateOp::Eq => 0u8,
                PredicateOp::Ne => 1u8,
                PredicateOp::IsNull => 2u8,
                PredicateOp::IsNotNull => 3u8,
                PredicateOp::Lt => 4u8,
                PredicateOp::Le => 5u8,
                PredicateOp::Gt => 6u8,
                PredicateOp::Ge => 7u8,
                PredicateOp::IsDistinctFrom => 8u8,
                PredicateOp::IsNotDistinctFrom => 9u8,
            }
            .hash(hasher);
            term.value.hash(hasher);
        }
        Expr::And(children) => {
            1u8.hash(hasher);
            hash_children(children, hasher);
        }
        Expr::Or(children) => {
            2u8.hash(hasher);
            hash_children(children, hasher);
        }
        Expr::Not(inner) => {
            3u8.hash(hasher);
            hash_expr(inner, hasher);
        }
    }
}

/// Arity, then each child in order.
fn hash_children<H: std::hash::Hasher>(children: &[Expr], hasher: &mut H) {
    use std::hash::Hash;

    children.len().hash(hasher);
    for child in children {
        hash_expr(child, hasher);
    }
}

/// Replace everything a mapping scan covered with what it built: `prefix`
/// (the spans that already tiled `[0, seg_start)`, untouched) followed by
/// `built` (a complete tiling of `[seg_start, watermark)` from
/// [`crate::map::Builder`]), plus the trailing [`SpanBody::Unscanned`] span
/// that makes the result tile the whole file even though the scan stopped
/// early (`docs/design/decisions.md`, "D30").
///
/// Whole-region replacement rather than an incremental merge because
/// `Builder`'s output is already a complete tiling of everything the segment
/// walked, so there is nothing to reconcile — only the seam at `seg_start`
/// has to be closed, and it is closed the same way `Builder::push_span`
/// closes every other boundary: **by extending the preceding span to where
/// the next one starts**.
///
/// That is what keeps interstitial blank lines attributed to the span before
/// them (`docs/design/decisions.md`, "D32") even across a stopping point. A previous scan
/// that stopped on a block's `end_offset` left that block's span ending
/// exactly there; the blank line that follows belongs to it, not to whatever
/// the next segment happens to recognize first.
///
/// Deficiency register: `deficiency: KD5` — the replacement is a whole-list
/// clone, so mapping is O(blocks × splices) and O(blocks²) wherever the save
/// throttle's gate never closes, which is every `--dqcache none` scan
/// (`measurements.md`, `per-block-quadratic`). **(c) unowned**; promoted by a
/// dump of thousands of blocks scanned that way. Closing it means an
/// appendable frontier here rather than a rebuild; a floor under the throttle
/// was refused, since it makes the cache's rate govern the map's cost.
fn splice(
    prefix: &[Span],
    mut built: Vec<Span>,
    seg_start: u64,
    watermark: u64,
    size: u64,
) -> Vec<Span> {
    let mut spans: Vec<Span> = prefix.to_vec();
    match (spans.last_mut(), built.first_mut()) {
        (Some(last), Some(first)) => last.end = first.start,
        // The segment recognized nothing at all, so everything it walked
        // belongs to the span that was already open at the seam.
        (Some(last), None) => last.end = watermark,
        // Nothing precedes this segment (the file's first bytes are already
        // inside it), so its own start is the floor — otherwise leading blank
        // lines, which open no span, would be left unattributed.
        (None, Some(first)) => first.start = seg_start,
        (None, None) => {}
    }
    spans.extend(built);
    if watermark < size {
        spans.push(Span {
            start: watermark,
            end: size,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: SpanBody::Unscanned,
        });
    }
    spans
}

/// Whether `index`'s map now answers the query for good, so the mapping scan
/// can stop short of EOF (`docs/design/decisions.md`, "D49"). Two things can
/// make a further block share the queried name, and both are ruled out:
///
/// - **A partition-root marker on a matching block**, whose `COPY` header
///   names the partition's **root**: other blocks carry the same name and are
///   not adjacent to it (I2), so only reaching EOF enumerates them.
/// - **Any `\connect` at all**, the file then being a `pg_dumpall`, a
///   concatenation or a `--create` dump, where a qualified name can be defined
///   again in a later database (I2). A `query_options.database` selector does
///   not lift this: two `\connect` segments can name the *same* database.
///
/// What neither catches is a file whose *first* segment has no `\connect` —
/// deficiency `KD6`, detailed at its marker
/// ([`crate::batch::ScanExtent::UntilTargetSettled`]).
fn target_settled(index: &DumpIndex, table: &str, selector: Option<&str>) -> bool {
    if index.spans.iter().any(|s| matches!(s.body, SpanBody::Connect { .. })) {
        return false;
    }
    let mut matched = false;
    for block in index.blocks_for(table) {
        if selector.is_some() && block.database.as_deref() != selector {
            continue;
        }
        if block.partition_root.is_some() {
            return false;
        }
        matched = true;
    }
    matched
}

/// How far [`map_forward`] got.
///
/// Two variants, not three: reaching EOF and stopping at a settled target are
/// the same fact to every caller. Interruption is the one outcome a caller
/// must not mistake for either, the map being short of the file through no
/// decision of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapStop {
    /// EOF, or the queried table settled.
    Reached,
    /// [`ScanOptions::cancel`] was set. The index is consistent at the last
    /// completed block and has been persisted; everything past it is
    /// unscanned.
    Interrupted,
}

/// Extend `index`'s map forward from its own `scanned_through`, persisting as
/// it goes, until the queried table is settled, EOF is reached, or the scan is
/// cancelled. Emits no rows — see the module docs. `target` is the
/// `(table, database selector)` a query may stop early for once
/// [`target_settled`] says so; **`None` means "run to EOF"**, what
/// [`ScanExtent::Full`] asks for and what [`map_file`] always wants. Rejected:
/// a `ScanExtent` beside a sentinel table name, which is dead data any later
/// reader has to prove is unused.
///
/// The [`crate::map::Builder`] is seeded with the segment's start offset, so
/// its first span begins at the frontier rather than at the first non-blank
/// line past it, and with the database in scope there, which it cannot infer.
///
/// **Not every completed block is persisted; every *exit* is** — see
/// [`SaveThrottle`]. **`index.metadata` is restated at each `\connect`ed
/// database's first `COPY` block**, which per I1 is one of the two boundaries
/// [`dump_metadata_from_spans`] may be called at, and the only one this loop
/// stands on.
///
/// **`statistics` is what to gather, and only [`map_file`] passes one that
/// gathers**: a query's pass is [`StatisticsRequest::NONE`]. A block it tracks
/// is observed row by row on this loop, or piece by piece where the leader
/// takes it.
async fn map_forward(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    index: &mut DumpIndex,
    target: Option<(&str, Option<&str>)>,
    statistics: &StatisticsRequest,
    size: u64,
) -> Result<MapStop> {
    if index.scanned_through >= size {
        return Ok(MapStop::Reached);
    }
    if target.is_some_and(|(table, selector)| target_settled(index, table, selector)) {
        return Ok(MapStop::Reached);
    }

    // The two checks above are "nothing to do" and earn no line
    // (`docs/manual/dump-inspection.md`, "`parse`: reading the dump"). Past
    // here a real scan runs, at whatever arrangement `--jobs` and the stated
    // budget resolved to (`docs/design/decisions.md`, "D64").
    tracing::info!(
        bytes = size,
        resumed_from = index.scanned_through,
        chunk_size = scan_options.chunk_size,
        jobs = scan_options.parallelism.jobs(),
        memory_bytes = %memory_budget_display(scan_options.parallelism),
        "scan started",
    );

    let seg_start = index.scanned_through;
    let prefix: Vec<Span> = index.spans.iter().filter(|s| s.end <= seg_start).cloned().collect();
    // The prefix tiles `[0, seg_start)`, so its last span ends exactly at the
    // frontier and its `database` is the one in scope there. With no prefix,
    // the preamble prepass has just run and I1 puts the frontier inside the
    // first database it captured.
    let database = prefix.last().and_then(|s| s.database.clone()).or_else(|| {
        index.metadata.as_ref().and_then(|m| m.databases.first()).and_then(|db| db.name.clone())
    });
    let mut builder = Builder::with_database(database);

    // The database whose first `COPY` block the metadata in hand was computed
    // at — the outer `None` meaning "no metadata at all", the inner one a
    // database with no `\connect` to name it. `dump_metadata_from_spans`
    // finalizes the *last* database it walks, so the last entry is it.
    let mut metadata_covers: Option<Option<String>> =
        index.metadata.as_ref().and_then(|m| m.databases.last()).map(|db| db.name.clone());

    // The chunk length this loop repeats to the frontier, and the budget it
    // may keep buffers inside (`ByteRangeSource::hint_read_size`).
    source.hint_read_size(scan_options.chunk_size);
    source.hint_parallelism(scan_options.parallelism);
    // **This loop grants no wait** (`docs/design/decisions.md`, "D5"): the
    // leader's fused worker grants it for itself and restores this policy on
    // the way out (`crate::leader::scan_region`).
    source.hint_wait_policy(WaitPolicy::NeverWait);
    let mut scanner = CopyScanner::resume(seg_start, None);
    let mut read_pos = seg_start;
    let mut carry = ChunkCarry::new();
    let mut throttle = SaveThrottle::new();
    // Whether the open `COPY` block is one `target_settled` would count — see
    // the `CopyEnd` arm, its only reader.
    let mut open_block_targets = false;
    // Whether `scan started`'s `jobs=` has already been corrected for this
    // scan — see [`report_shortfall`], its only reader and writer.
    let mut shortfall_reported = false;

    loop {
        // Once per chunk, the `CopyEnd` arm carrying the other check
        // (`docs/design/decisions.md`, "D63").
        // `index` is consistent at the last *spliced* watermark whatever the
        // buffer holds, so the save needs no snapshot logic of its own.
        if scan_options.cancelled() {
            cache.save(source, index).await?;
            return Ok(MapStop::Interrupted);
        }
        let want = scan_options.chunk_size.min((size - read_pos) as usize);
        let chunk = if want > 0 {
            let bytes = source.read_range(read_pos, want).await?;
            read_pos += bytes.len() as u64;
            bytes
        } else {
            Bytes::new()
        };
        let eof = read_pos >= size;

        carry.absorb(&chunk);
        // Where the leader closed a block the workers scanned, and so where
        // the serial scanner has to be put back down. `None` for a chunk no
        // region was taken out of.
        let mut resume_at: Option<u64> = None;
        for pass in ChunkCarry::PASSES {
            let (span, span_eof) = carry.span(pass, &chunk, eof);
            while let Some(event) = scanner.next_event(span, span_eof)? {
                match event {
                    Event::CopyStart(start) => {
                        // The current database's first `COPY` block is the
                        // recurring one of the two boundaries
                        // `dump_metadata_from_spans` may be called at (I1).
                        // Retreat to a pending TOC comment's own start as
                        // `crate::index::scan_preamble` does, so the span list
                        // ends where the `Data` span is about to begin.
                        let boundary =
                            builder.pending_comment_start().unwrap_or(start.header_offset);
                        // Whether this block can be the one that settles
                        // `target` — the `CopyEnd` arm's reason to splice for
                        // a block the throttle would have skipped.
                        // **Deliberately the header alone**, a superset:
                        // repeating `target_settled`'s selector test here
                        // would tie the gate's width to that function's body.
                        open_block_targets =
                            target.is_some_and(|(table, _)| start.header.matches(table));
                        // Read off the header, for the offer below.
                        let header_offset = start.header_offset;
                        let data_offset = start.data_offset;
                        let columns = start.header.columns.len();
                        let header = statistics.gathers().then(|| start.header.clone());
                        builder.on_copy_start(start);
                        // **Once per database, not once per block**:
                        // recomputing at every `CopyStart` would put a third
                        // whole-index-sized cost in this loop.
                        let db = builder.database().map(str::to_owned);
                        if metadata_covers.as_ref() != Some(&db) {
                            let spans = splice(
                                &prefix,
                                builder.snapshot(boundary),
                                seg_start,
                                boundary,
                                size,
                            );
                            index.metadata = Some(dump_metadata_from_spans(&spans));
                            metadata_covers = Some(db.clone());
                        }
                        // After the restatement, so the observer resolves the
                        // block against its own database's DDL.
                        let observer = header.as_ref().and_then(|header| {
                            gather::observer_for(
                                statistics,
                                header,
                                index.metadata.as_ref(),
                                db.as_deref(),
                            )
                        });
                        if let Some(observer) = observer {
                            builder.observe_block(observer);
                        }
                        // **The offer, and this loop is the leader making it**
                        // (`crate::leader::scan_region`): everything from
                        // `data_offset` until `\.` is line-structured rows, so
                        // the region may be handed to workers that never parse
                        // structure. It answers the block's totals as the
                        // serial scanner would have, or declines — and a
                        // block it closes holds the statistics its pieces
                        // observed, as a declined one does the rows this loop
                        // hands on.
                        let outcome = leader::scan_region(
                            source,
                            scan_options,
                            header_offset,
                            data_offset,
                            columns,
                            size,
                            builder.block_observer(),
                        )
                        .await?;
                        report_shortfall(&mut shortfall_reported, outcome.shortfall);
                        match outcome.scan {
                            RegionScan::Closed(interior) => {
                                // The workers counted the rows, so their
                                // census stands in for the `on_row` calls this
                                // loop never made.
                                builder.absorb_census(&interior.census);
                                let watermark = interior.end.end_offset;
                                let targets = std::mem::take(&mut open_block_targets);
                                match close_copy_block(
                                    source,
                                    scan_options,
                                    cache,
                                    index,
                                    &mut builder,
                                    &mut throttle,
                                    &prefix,
                                    seg_start,
                                    size,
                                    target,
                                    targets,
                                    interior.end,
                                )
                                .await?
                                {
                                    // The serial scanner is still at this
                                    // block's `data_offset` and the chunk in
                                    // hand is bytes the workers already read,
                                    // so both are dropped — after the event
                                    // loop, where they can be moved at all.
                                    BlockClose::Continue => {
                                        resume_at = Some(watermark);
                                        break;
                                    }
                                    BlockClose::Settled => {
                                        // A query stopping at its own target,
                                        // not at EOF.
                                        tracing::info!(
                                            bytes = watermark,
                                            reached_eof = false,
                                            "scan complete",
                                        );
                                        return Ok(MapStop::Reached);
                                    }
                                    BlockClose::Interrupted => return Ok(MapStop::Interrupted),
                                }
                            }
                            // Nothing happened: the serial scanner owns the
                            // region and reads on into it.
                            RegionScan::Declined => {}
                            // Cancelled between two windows, so nothing about
                            // this block is known. `index` is consistent at
                            // the last spliced watermark, which is before it.
                            RegionScan::Cancelled => {
                                cache.save(source, index).await?;
                                return Ok(MapStop::Interrupted);
                            }
                        }
                    }
                    // This pass needs only the block's extent; row bytes
                    // become batches in the replay phase. Rows are read here
                    // for the array-shape census alone
                    // (`docs/design/decisions.md`, "D35").
                    Event::Row(row) => builder.on_row(row.offset, row.raw),
                    Event::CopyEnd(end) => {
                        let targets = std::mem::take(&mut open_block_targets);
                        let end_offset = end.end_offset;
                        match close_copy_block(
                            source,
                            scan_options,
                            cache,
                            index,
                            &mut builder,
                            &mut throttle,
                            &prefix,
                            seg_start,
                            size,
                            target,
                            targets,
                            end,
                        )
                        .await?
                        {
                            BlockClose::Continue => {}
                            BlockClose::Settled => {
                                tracing::info!(
                                    bytes = end_offset,
                                    reached_eof = false,
                                    "scan complete",
                                );
                                return Ok(MapStop::Reached);
                            }
                            BlockClose::Interrupted => return Ok(MapStop::Interrupted),
                        }
                    }
                    Event::Line(line) => builder.feed_line(line.offset, line.raw),
                    Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
                    Event::LargeObjectStart(start) => {
                        builder.on_large_object_start(start.start_offset)
                    }
                    Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
                }
            }
            if resume_at.is_some() {
                break;
            }
            carry.consumed(pass, &chunk, scanner.take_consumed());
        }

        // **The leader took a region, so the serial scanner is put back down
        // past it.** The carry is discarded rather than fixed up — whatever it
        // held is inside bytes the workers have since read whole, and
        // `end_offset` is a line start — and `read_pos` follows it.
        if let Some(at) = resume_at {
            scanner = CopyScanner::resume(at, None);
            carry = ChunkCarry::new();
            read_pos = at;
            // The three hints this loop announced still stand: `scan_region`
            // restores `WaitPolicy::NeverWait` on its way out, announces the
            // same `Parallelism`, and never touches the read size.
            continue;
        }

        if eof {
            break;
        }
        if carry.len() > scan_options.max_line_bytes {
            Err(Error::LineTooLong {
                offset: scanner.position(),
                limit: scan_options.max_line_bytes,
            })?;
        }
    }

    index.roles.extend(builder.roles().iter().cloned());
    index.tablespaces.extend(builder.tablespaces().iter().cloned());
    index.spans = splice(&prefix, builder.finish(size), seg_start, size, size);
    index.scanned_through = size;
    attach_text(source, &mut index.spans).await?;
    index.diagnostics = tiling_diagnostics(&index.spans, size);
    index.diagnostics.push(toc_coverage_diagnostic(&index.spans));
    cache.save(source, index).await?;
    // The true end of the file, as opposed to the two early
    // `Ok(MapStop::Reached)`s above that stop a query at its settled target.
    tracing::info!(bytes = size, reached_eof = true, "scan complete");
    Ok(MapStop::Reached)
}

/// Correct `scan started`'s `jobs=` where the leader delivered fewer readers
/// than the caller asked for, **once per scan**, and say what would buy the
/// arrangement back (`docs/design/decisions.md`, "D64"). `scan started` names
/// what was asked for, the source's advice not having been read when it fires;
/// on a query the same fact is a [`PlanNote`] on a `TableStream`.
///
/// **Silence means the leader dispatched the announced count**, not that every
/// one of them read at once. `flag` keeps one scan-wide fact from printing
/// once per block, which is also why [`crate::leader::Shortfall`] reports no
/// reason a later block could answer differently.
fn report_shortfall(flag: &mut bool, shortfall: Option<leader::Shortfall>) {
    let Some(shortfall) = shortfall.filter(|_| !*flag) else {
        return;
    };
    *flag = true;
    tracing::info!(
        jobs = shortfall.delivered,
        asked = shortfall.asked,
        bound_by = shortfall.bound_by.as_str(),
        would_hold_bytes = shortfall.would_hold_bytes,
        "scan arrangement",
    );
}

/// What closing one `COPY` block asks of the loop that closed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockClose {
    /// Nothing: carry on scanning.
    Continue,
    /// The queried table is settled, and the map has been persisted at this
    /// block's watermark.
    Settled,
    /// [`ScanOptions::cancel`] was set, and the map has been persisted at this
    /// block's watermark.
    Interrupted,
}

/// Close a `COPY` block on the [`crate::map::Builder`] and do everything
/// [`map_forward`] owes at a `CopyEnd`: the splice, the throttled save, and the
/// early-stop check. **One body, two callers** — the serial scanner and
/// [`crate::leader::scan_region`] (`docs/design/decisions.md`, "D52") — and
/// the ordering it holds is splice before the settled test, save before the
/// return. `targets` is whether this block's header could be the one that
/// settles `target`, read at `CopyStart`.
#[allow(clippy::too_many_arguments)]
async fn close_copy_block(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    index: &mut DumpIndex,
    builder: &mut Builder,
    throttle: &mut SaveThrottle,
    prefix: &[Span],
    seg_start: u64,
    size: u64,
    target: Option<(&str, Option<&str>)>,
    targets: bool,
    end: CopyEnd,
) -> Result<BlockClose> {
    // `end_offset` is always a safe, resumable watermark — the scanner is back
    // in its `Outside` state there — and `on_copy_end` leaves the builder
    // `Idle`, where `snapshot` is sound.
    let watermark = end.end_offset;
    builder.on_copy_end(end);
    // The second of the guard's two check points (`docs/design/decisions.md`,
    // "D63"): the two together bound the response by the shorter of a chunk
    // and a block.
    let cancelled = scan_options.cancelled();
    let due = throttle.due();
    // **The splice rides the throttle's gate** (`docs/design/decisions.md`,
    // "D62"). Nothing between gate openings reads `index`: the metadata
    // recompute in the `CopyStart` arm splices its own copy, and
    // `target_settled` is the one reader that would — which is why a block
    // whose header could satisfy it opens the gate too.
    if targets || cancelled || due {
        index.spans = splice(prefix, builder.snapshot(watermark), seg_start, watermark, size);
        index.roles.extend(builder.roles().iter().cloned());
        index.tablespaces.extend(builder.tablespaces().iter().cloned());
        index.scanned_through = index.scanned_through.max(watermark);
    }
    // Only a block `target_settled` counts can turn it from false to true, and
    // `index` has just been spliced for exactly those — so this always reads a
    // map current through `watermark`.
    let settled =
        targets && target.is_some_and(|(table, selector)| target_settled(index, table, selector));
    // The save at the *last* watermark before an early stop is what persists
    // the map for the next query, so a settled target and an interrupt save
    // whether or not the throttle would have.
    if settled || cancelled || due {
        throttle.save(cache, source, index).await?;
    }
    if settled {
        return Ok(BlockClose::Settled);
    }
    if cancelled {
        return Ok(BlockClose::Interrupted);
    }
    Ok(BlockClose::Continue)
}

/// How many times the elapsed scan has to cover the last save's own cost
/// before another save is worth taking, which bounds save overhead at roughly
/// `1/K` of scan time.
const SAVE_THROTTLE_K: u32 = 20;

/// Decides whether a mid-scan cache save has earned its cost
/// (`docs/design/decisions.md`, "D62").
///
/// **The rule is self-tuning, not an interval**: skip a block's save unless at
/// least [`SAVE_THROTTLE_K`] times the last save's own duration has elapsed
/// since it. **Exits are exempt** — EOF, a settled target and an interrupt all
/// save unconditionally.
///
/// **The gate also decides when the map is rebuilt.** `stream::splice` fires
/// at the openings of this gate rather than at every `CopyEnd` (see
/// [`map_forward`]'s `CopyEnd` arm). With a disabled cache `save` is ~free, so
/// the gate always clears and the map is rebuilt per block — the residual
/// `KD5` names.
struct SaveThrottle {
    last_save: Instant,
    last_cost: Duration,
}

impl SaveThrottle {
    /// Starts due: `last_cost` is zero, so the first block of a segment always
    /// banks, rather than leaving a resumable frontier never written down.
    fn new() -> Self {
        Self { last_save: Instant::now(), last_cost: Duration::ZERO }
    }

    /// The rule itself, over measured quantities rather than clocks.
    fn due_after(elapsed: Duration, last_cost: Duration) -> bool {
        elapsed >= last_cost.saturating_mul(SAVE_THROTTLE_K)
    }

    fn due(&self) -> bool {
        Self::due_after(self.last_save.elapsed(), self.last_cost)
    }

    /// Save, and time the save — that duration is the whole input to the next
    /// decision. A disabled cache makes this ~free and so never throttles.
    async fn save(
        &mut self,
        cache: &CacheMode,
        source: &dyn ByteRangeSource,
        index: &DumpIndex,
    ) -> Result<()> {
        let started = Instant::now();
        cache.save(source, index).await?;
        self.last_cost = started.elapsed();
        self.last_save = Instant::now();
        Ok(())
    }
}

/// What one [`map_file`] run did. `resumed_from` is the frontier the run
/// *started* at: `0` for a scan that began at byte 0, the cache's
/// `scanned_through` for one that resumed.
#[derive(Debug)]
pub struct MapRun {
    /// The map as it stands after the run: whole-file when `interrupted` is
    /// false, everything up to the last *spliced* watermark when it is true,
    /// which is also the last save ([`SaveThrottle`]).
    pub index: DumpIndex,
    /// The frontier this run started from.
    pub resumed_from: u64,
    /// Whether [`ScanOptions::cancel`] stopped the run short of EOF, or short
    /// of re-reading every block that lacked the requested statistics. The
    /// index and the cache agree either way; what differs is whether the
    /// index describes the whole file, which [`DumpIndex::is_complete`] tells
    /// apart from a stop inside the back-fill.
    pub interrupted: bool,
    /// How many blocks the scan had already mapped lacked the statistics this
    /// run asked for ([`StatisticsRequest::backfill`]) — zero for a run
    /// interrupted before its map reached EOF, which is where the count is
    /// taken.
    pub lacking_statistics: usize,
    /// How many of those were re-read and now hold them.
    pub backfilled: usize,
}

/// Map `source` end to end, **continuing from whatever `cache` already
/// holds** — `pgdq parse`'s scan (`docs/design/decisions.md`, "D61"). This is
/// [`map_forward`] with no stop target, plus the three whole-file facts that
/// only a scan reaching EOF may state; `crate::index::build_index` stays the
/// eager, cache-blind producer. Rejected: teaching *it* to resume, which
/// duplicates the splice-onto-a-prefix logic here.
///
/// **It opens with the bounded preamble prepass** [`table_stream`] runs, under
/// the same "unless the first database's preamble is already complete" guard
/// (`docs/design/decisions.md`, "D30"), and only for a scan starting at byte 0
/// — its spans *are* the prefix, so running it over a resumed map would
/// discard one.
///
/// **It gathers what `statistics` asks for**, over the blocks this run maps,
/// and then **re-reads every block the cache already held that lacks it**
/// ([`StatisticsRequest::backfill`]), one at a time in file order through
/// [`gather_block_statistics`], saving as it goes. The back-fill runs once the
/// map has reached EOF, so each block resolves against whole-file metadata,
/// and a run interrupted inside it resumes into it, the blocks still lacking
/// being counted afresh. [`StatisticsRequest::default`] gathers every
/// statistic.
///
/// **The three finishing steps are this function's, not `map_forward`'s.**
///
/// - `metadata` is recomputed over the whole span list. EOF is the other
///   boundary [`dump_metadata_from_spans`] may be called at (I1), covering a
///   trailing database with no `COPY` block of its own, and a file with no
///   blocks at all.
/// - `diagnostics` are recomputed rather than inherited, being
///   `#[serde(skip)]`; whatever [`CacheMode::load`] reported about the cache
///   *file* is kept ahead of them.
/// - The cache is saved once more at the end, persisting the finished index;
///   it is also the only save when nothing was scanned at all.
///
/// **A run interrupted before EOF states none of the three**, and returns
/// [`MapRun::interrupted`]: a span list cut at a `CopyEnd` watermark is not a
/// boundary `dump_metadata_from_spans` may be called at.
pub async fn map_file(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    statistics: &StatisticsRequest,
) -> Result<MapRun> {
    let size = source.size().await?;
    let mut index = match cache.load(source).await? {
        CacheLoad::Index(index) => index,
        // Four reasons to start cold, spelled out rather than wildcarded
        // (`docs/design/decisions.md`, "D22").
        CacheLoad::Disabled
        | CacheLoad::Missing
        | CacheLoad::Unreadable
        | CacheLoad::UnsupportedVersion => DumpIndex::default(),
        // The fifth is a refusal, before a byte of the dump is read: this
        // cache describes another file.
        CacheLoad::SourceChanged { cached_stored_size, live_stored_size } => {
            return Err(cache.source_mismatch(cached_stored_size, live_stored_size));
        }
    };
    // The one diagnostic about the cache *file* rather than about the map;
    // everything else the load computed is recomputed below.
    let carried: Vec<Diagnostic> = index
        .diagnostics
        .drain(..)
        .filter(|d| d.kind == DiagnosticKind::CacheMtimeChanged)
        .collect();
    let resumed_from = index.scanned_through.min(size);

    let first_db_preamble_known = index
        .metadata
        .as_ref()
        .and_then(|m| m.databases.first())
        .is_some_and(|db| db.preamble_complete);
    if resumed_from == 0 && !first_db_preamble_known {
        // Persisted before `map_forward` runs, so an interrupt in the very
        // first chunk still finds banked metadata. `scan_preamble` itself
        // ignores the cancel flag (`docs/design/decisions.md`, "D26").
        let (metadata, spans, preamble_end, roles, tablespaces) =
            scan_preamble(source, scan_options).await?;
        index.metadata = Some(metadata);
        index.spans = splice(&[], spans, 0, preamble_end, size);
        index.roles.extend(roles);
        index.tablespaces.extend(tablespaces);
        index.scanned_through = preamble_end;
        attach_text(source, &mut index.spans).await?;
        cache.save(source, &index).await?;
    }

    if map_forward(source, scan_options, cache, &mut index, None, statistics, size).await?
        == MapStop::Interrupted
    {
        return Ok(MapRun {
            index,
            resumed_from,
            interrupted: true,
            lacking_statistics: 0,
            backfilled: 0,
        });
    }

    index.metadata = Some(dump_metadata_from_spans(&index.spans));
    let mut diagnostics = carried;
    diagnostics.extend(tiling_diagnostics(&index.spans, size));
    diagnostics.push(toc_coverage_diagnostic(&index.spans));
    index.diagnostics = diagnostics;
    let backfill =
        backfill_statistics(source, scan_options, cache, &mut index, statistics, size).await?;
    if !backfill.interrupted {
        cache.save(source, &index).await?;
    }
    Ok(MapRun {
        index,
        resumed_from,
        interrupted: backfill.interrupted,
        lacking_statistics: backfill.lacking,
        backfilled: backfill.reread,
    })
}

/// What [`backfill_statistics`] did.
struct BackfillRun {
    lacking: usize,
    reread: usize,
    interrupted: bool,
}

/// Re-read every block of a map that has reached EOF which lacks what
/// `statistics` asks for, in file order, storing each block's statistics as it
/// closes and saving through a [`SaveThrottle`] as [`map_forward`] does; an
/// interrupt saves and stops. Announces how many blocks lacked them and, once
/// every one is re-read, how many were — and nothing at all when none did.
async fn backfill_statistics(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    index: &mut DumpIndex,
    statistics: &StatisticsRequest,
    size: u64,
) -> Result<BackfillRun> {
    // Positions into `index.spans`, which nothing below adds to or reorders.
    let lacking: Vec<(usize, StatisticsBackfill)> = if statistics.gathers() {
        index
            .spans
            .iter()
            .enumerate()
            .filter_map(|(at, span)| match &span.body {
                SpanBody::Data(DataBlock::Copy(block)) => {
                    statistics.backfill(block).map(|backfill| (at, backfill))
                }
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut run = BackfillRun { lacking: lacking.len(), reread: 0, interrupted: false };
    if lacking.is_empty() {
        return Ok(run);
    }
    tracing::info!(blocks = run.lacking, "statistics back-fill started");
    announce_read_loop(source, scan_options);
    let mut throttle = SaveThrottle::new();
    let mut shortfall_reported = false;
    for (at, backfill) in lacking {
        let SpanBody::Data(DataBlock::Copy(block)) = &index.spans[at].body else {
            unreachable!("the positions were read off copy blocks of this span list");
        };
        let gathered = reread_block(
            source,
            scan_options,
            index.metadata.as_ref(),
            block,
            &backfill,
            size,
            &mut shortfall_reported,
        )
        .await?;
        let Some(gathered) = gathered else {
            cache.save(source, index).await?;
            run.interrupted = true;
            return Ok(run);
        };
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut index.spans[at].body {
            block.statistics = Some(Arc::new(gathered));
        }
        run.reread += 1;
        // Both of `map_forward`'s check points, a block's close being the
        // second (`docs/design/decisions.md`, "D63").
        if scan_options.cancelled() && run.reread < run.lacking {
            cache.save(source, index).await?;
            run.interrupted = true;
            return Ok(run);
        }
        if throttle.due() {
            throttle.save(cache, source, index).await?;
        }
    }
    tracing::info!(blocks = run.reread, "statistics back-fill complete");
    Ok(run)
}

/// The three hints every top-level read loop announces before its first read
/// (`crate::scan::scan`, [`map_forward`]).
fn announce_read_loop(source: &dyn ByteRangeSource, scan_options: &ScanOptions) {
    source.hint_read_size(scan_options.chunk_size);
    source.hint_parallelism(scan_options.parallelism);
    source.hint_wait_policy(WaitPolicy::NeverWait);
}

/// Re-read one block the map already holds and gather what `backfill` names —
/// the library's per-block back-fill, which [`map_file`] runs over every block
/// lacking what its request asks for. `metadata` is the map's, whole-file where
/// it can be, which the block's columns are resolved against as a mapping pass
/// resolves them; `backfill` is [`StatisticsRequest::backfill`]'s answer for
/// `block`. The caller stores the result in [`CopyBlock::statistics`].
///
/// **The block is scanned as a mapping pass scans it**: offered to the leader
/// under `scan_options`' parallelism, and read serially where it declines, so
/// what is gathered is what a straight-through pass would have gathered.
/// `Ok(None)` is [`ScanOptions::cancel`] stopping it before the block closed.
///
/// **A block that no longer ends where the map says is refused**,
/// [`Error::CachedBlockChanged`], rather than given statistics describing other
/// bytes than its map does.
pub async fn gather_block_statistics(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    metadata: Option<&DumpMetadata>,
    block: &CopyBlock,
    backfill: &StatisticsBackfill,
) -> Result<Option<BlockStatistics>> {
    let size = source.size().await?;
    announce_read_loop(source, scan_options);
    let mut shortfall_reported = false;
    reread_block(source, scan_options, metadata, block, backfill, size, &mut shortfall_reported)
        .await
}

/// [`gather_block_statistics`] once the source is announced, `size` known, and
/// with the flag [`report_shortfall`] keeps once per pass.
async fn reread_block(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    metadata: Option<&DumpMetadata>,
    block: &CopyBlock,
    backfill: &StatisticsBackfill,
    size: u64,
    shortfall_reported: &mut bool,
) -> Result<Option<BlockStatistics>> {
    let mut observer = gather::observer_tracking(
        &backfill.columns,
        backfill.group_size,
        &block.header,
        metadata,
        block.database.as_deref(),
    );
    let outcome = leader::scan_region(
        source,
        scan_options,
        block.header_offset,
        block.data_offset,
        block.header.columns.len(),
        size,
        Some(observer.as_mut()),
    )
    .await?;
    report_shortfall(shortfall_reported, outcome.shortfall);
    let end = match outcome.scan {
        RegionScan::Closed(interior) => interior.end,
        RegionScan::Cancelled => return Ok(None),
        RegionScan::Declined => {
            match observe_rows(source, scan_options, block, size, observer.as_mut()).await? {
                Some(end) => end,
                None => return Ok(None),
            }
        }
    };
    let recorded = (block.terminator_offset, block.end_offset, block.row_count);
    if (end.terminator_offset, end.end_offset, end.row_count) != recorded {
        return Err(Error::CachedBlockChanged { header_offset: block.header_offset });
    }
    Ok(Some(observer.finish(block.terminator_offset - block.data_offset)))
}

/// Hand `observer` every row of `block`, read serially from its first data
/// byte to its terminator, and answer the `CopyEnd` the scanner met — `None`
/// where [`ScanOptions::cancel`] was set first, read once per chunk as
/// [`map_forward`] reads it.
async fn observe_rows(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    block: &CopyBlock,
    size: u64,
    observer: &mut dyn BlockObserver,
) -> Result<Option<CopyEnd>> {
    let mut scanner = CopyScanner::resume(block.data_offset, Some((block.header_offset, 0)));
    let mut carry = ChunkCarry::new();
    let mut read_pos = block.data_offset;
    loop {
        if scan_options.cancelled() {
            return Ok(None);
        }
        let want = scan_options.chunk_size.min((size - read_pos) as usize);
        let chunk = if want > 0 {
            let bytes = source.read_range(read_pos, want).await?;
            read_pos += bytes.len() as u64;
            bytes
        } else {
            Bytes::new()
        };
        let eof = read_pos >= size;
        carry.absorb(&chunk);
        for pass in ChunkCarry::PASSES {
            let (span, span_eof) = carry.span(pass, &chunk, eof);
            while let Some(event) = scanner.next_event(span, span_eof)? {
                match event {
                    Event::Row(row) => {
                        observer.observe_row(row.offset - block.data_offset, row.raw)
                    }
                    Event::CopyEnd(end) => return Ok(Some(end)),
                    // A scanner inside a block's rows emits nothing else
                    // before its `CopyEnd`.
                    _ => {}
                }
            }
            carry.consumed(pass, &chunk, scanner.take_consumed());
        }
        if eof {
            return Err(Error::UnterminatedCopyBlock { header_offset: block.header_offset });
        }
        if carry.len() > scan_options.max_line_bytes {
            return Err(Error::LineTooLong {
                offset: scanner.position(),
                limit: scan_options.max_line_bytes,
            });
        }
    }
}

/// Opaque cursor into a [`table_stream`]/[`crate::batch::read_table`]
/// consumption, sufficient to resume from just past the last batch a caller
/// accepted. Valid only within the process that produced it; persisting one
/// across a restart is out of scope (`docs/design/roadmap.md`).
#[derive(Debug, Clone)]
pub struct ResumeToken {
    offset: u64,
    rows_emitted: u64,
    /// Stamp of the query this token came out of — see [`query_fingerprint`].
    /// Resuming a stream whose options hash differently is
    /// `Error::ResumeQueryMismatch`, which is what defends "one schema per
    /// stream, resolved up front".
    query_fingerprint: u64,
    /// Reserved for the structural cache's generation stamp; the cache does
    /// not stamp generations, so this is always 0.
    #[allow(dead_code)]
    generation: u64,
    in_copy: Option<InCopyResume>,
}

#[derive(Debug, Clone)]
struct InCopyResume {
    header: CopyHeader,
    header_offset: u64,
    rows_in_block: u64,
    field_count: usize,
    database: Option<String>,
}

impl ResumeToken {
    fn start(query_fingerprint: u64) -> Self {
        Self { offset: 0, rows_emitted: 0, query_fingerprint, generation: 0, in_copy: None }
    }
}

/// A pull-mode stream of `Utf8View` `RecordBatch`es for one table query.
/// Construct with [`table_stream`].
pub struct TableStream<'a> {
    inner: Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send + 'a>>,
    position: Arc<Mutex<ResumeToken>>,
    resolved_schema: Arc<Mutex<ResolvedSchema>>,
    comparison_notes: Arc<Mutex<Vec<ComparisonNote>>>,
    batch_offset: Arc<Mutex<u64>>,
    plan_notes: Arc<Mutex<Vec<PlanNote>>>,
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

    /// This query's resolved schema and diagnostics — one schema per stream
    /// (`docs/design/decisions.md`, "Type resolution and decoders"). The empty
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
    /// alone (`crate::pgtype::ComparisonDivergence::affects_equality`), so a
    /// `text` column with no `COLLATE` clause earns a note under `<` and none
    /// under `=`. A third channel beside `DumpIndex.diagnostics` (L1) and
    /// `ResolvedSchema.notes` (L2). Like [`Self::resolved_schema`], it
    /// describes the **last** block whose schema resolved.
    pub fn comparison_notes(&self) -> Vec<ComparisonNote> {
        self.comparison_notes.lock().unwrap().clone()
    }

    /// Facts about *this query's plan* rather than about a column or a
    /// predicate ([`PlanNoteKind`]): the memory budget in force declining
    /// something, and the row groups statistics let the replay skip. A fourth
    /// channel beside `DumpIndex.diagnostics` (L1), `ResolvedSchema.notes`
    /// (L2) and [`Self::comparison_notes`] (L4).
    ///
    /// **Settled before any block is read**, from the map alone. A
    /// [`table_stream_partitions`] sub-stream holds them when it is handed
    /// back; [`table_stream`]'s serial replay, which plans no partitions and
    /// so never notes a budget, holds its pruning note once its first item is
    /// polled, and nothing before.
    pub fn plan_notes(&self) -> Vec<PlanNote> {
        self.plan_notes.lock().unwrap().clone()
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
    /// of 0 — reports the scanner's position instead, so the value is monotone
    /// within a sub-stream either way.
    pub fn batch_source_offset(&self) -> u64 {
        *self.batch_offset.lock().unwrap()
    }
}

/// Build the [`ResolvedSchema`] for a table-matching block — the actual batch
/// schema a [`RowBatcher`] built from it carries (see
/// [`TableStream::resolved_schema`]) — scoped to `database`, the block's own
/// attribution, never a guess (`docs/design/decisions.md`, "D49"). `census` is
/// the union of the array-shape censuses of **every block this stream will
/// replay**, a parameter rather than something `resolve_columns` looks up so
/// that no call site can silently disagree
/// (`docs/design/decisions.md`, "D35").
///
/// `Typed` mode against metadata that has no *complete* entry for `database`
/// is `Error::MetadataNotScanned` rather than a silent `NotDeclared`
/// degradation. **No caller can trip that today**: every call site reads
/// `index.metadata` after the mapping pass, which states a database's DDL at
/// its first `COPY` block. The check is pinned by a unit test.
fn resolve_block(
    header: &CopyHeader,
    field_count: usize,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    schema_mode: SchemaMode,
    census: &[ArrayShape],
) -> Result<ResolvedSchema> {
    if schema_mode == SchemaMode::Typed
        && let Some(meta) = metadata
        && !meta.databases.iter().any(|db| db.name.as_deref() == database && db.preamble_complete)
    {
        return Err(Error::MetadataNotScanned { database: database.map(str::to_string) });
    }
    let names = column_names(header, field_count);
    Ok(resolve_columns(&header.qualified_name(), &names, metadata, database, schema_mode, census))
}

/// Reconstruct the in-progress block state a [`ResumeToken`] captured, if any:
/// the scanner's row counter (so a later `CopyEnd` reports the block's true
/// total) and a fresh [`RowBatcher`] on the original block's schema.
fn resume_state(token: &ResumeToken, plan: &ReplayPlan) -> Result<(CopyScanner, Option<Opened>)> {
    let scanner = CopyScanner::resume(
        token.offset,
        token.in_copy.as_ref().map(|ic| (ic.header_offset, ic.rows_in_block)),
    );
    let active = token
        .in_copy
        .as_ref()
        .map(|ic| {
            activate(ic.header.clone(), ic.header_offset, ic.field_count, ic.database.clone(), plan)
        })
        .transpose()?;
    Ok((scanner, active))
}

fn snapshot(
    scanner: &CopyScanner,
    active: &Option<Active>,
    rows_emitted: u64,
    query_fingerprint: u64,
) -> ResumeToken {
    let in_copy =
        active.as_ref().map(|(header_offset, header, batcher, _, database)| InCopyResume {
            header: header.clone(),
            header_offset: *header_offset,
            rows_in_block: scanner.in_copy_rows().unwrap_or(0),
            field_count: batcher.field_count(),
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

/// Everything a replay needs that the mapping pass produced, shared unchanged
/// by every sub-stream of a partitioned replay
/// (`docs/design/decisions.md`, "D51"). Held behind an `Arc`, `metadata` being
/// the whole dump's DDL, and never mutated after the mapping pass: the census
/// in particular is the union over **every** block the query will replay, so
/// two partitions of one table cannot resolve its arrays differently
/// (`docs/design/decisions.md`, "D35").
struct ReplayPlan {
    scan_options: ScanOptions,
    query_options: QueryOptions,
    metadata: Option<DumpMetadata>,
    census: Vec<ArrayShape>,
    /// Every matched block with a column list, resolved and keyed by the
    /// block's `header_offset` — see [`plan_blocks`], which refuses the whole
    /// plan on any block's resolution refusal.
    blocks: BTreeMap<u64, PlannedBlock>,
    /// The runs of row groups each pruned block keeps, keyed as `blocks` is
    /// ([`crate::prune::BlockPruning::kept`]); a block absent here is read
    /// whole.
    kept: BTreeMap<u64, Vec<Range<u64>>>,
    /// What pruning skipped, where any block's statistics were consulted.
    pruned: Option<PlanNote>,
}

impl ReplayPlan {
    /// The plan for replaying `mapped`: its DDL and census, and every one of
    /// its blocks resolved against them before any sub-stream exists — or
    /// the first refusing block's refusal ([`plan_blocks`]).
    fn new(
        scan_options: ScanOptions,
        query_options: QueryOptions,
        matches: &[CopyBlock],
        metadata: Option<DumpMetadata>,
        census: Vec<ArrayShape>,
    ) -> Result<Self> {
        let blocks = plan_blocks(matches, &query_options, metadata.as_ref(), &census)?;
        let (kept, pruned) = prune_blocks(matches, &blocks, &query_options, metadata.as_ref());
        Ok(Self { scan_options, query_options, metadata, census, blocks, kept, pruned })
    }

    /// The segments that replay `block` whole, or the runs of groups its
    /// statistics keep: the run holding group 0 starts at the block's header,
    /// as a whole block does, and every other starts inside its data.
    fn segments(&self, block: &CopyBlock) -> Vec<Segment> {
        let Some(kept) = self.kept.get(&block.header_offset) else {
            return vec![Segment {
                block: block.clone(),
                start: block.header_offset,
                limit: block.end_offset,
                entry: SegmentEntry::Header,
            }];
        };
        kept.iter().map(|run| Segment::over(block, run.clone())).collect()
    }
}

/// Settle which row groups of `matches` the query's filter skips
/// ([`prune_block`]), and the [`PlanNote`] saying so. Nothing is pruned where
/// the caller turned statistics off or the filter reads no field — no
/// statistic can rule out a row of a filter that keeps every one — nor in a
/// block with no [`PlannedBlock`], whose field count only its first row says.
///
/// **A block whose statistics keep every group is left out of the map**, so
/// it is replayed exactly as an unpruned query replays it.
fn prune_blocks(
    matches: &[CopyBlock],
    blocks: &BTreeMap<u64, PlannedBlock>,
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
) -> (BTreeMap<u64, Vec<Range<u64>>>, Option<PlanNote>) {
    let mut kept = BTreeMap::new();
    if !query_options.use_statistics {
        return (kept, None);
    }
    let (mut consulted, mut groups, mut skipped_groups, mut skipped_bytes) = (false, 0, 0, 0);
    for block in matches {
        let Some(planned) = blocks.get(&block.header_offset) else { continue };
        if !planned.filter.reads_fields() {
            continue;
        }
        let Some(pruning) = prune_block(block, &planned.filter, metadata) else { continue };
        consulted = true;
        groups += pruning.groups;
        skipped_groups += pruning.skipped_groups;
        skipped_bytes += pruning.skipped_bytes;
        if pruning.skipped_groups > 0 {
            kept.insert(block.header_offset, pruning.kept);
        }
    }
    let bytes = matches.iter().map(|b| b.terminator_offset.saturating_sub(b.data_offset)).sum();
    let note = consulted.then_some(PlanNote {
        kind: PlanNoteKind::StatisticsPruned { skipped_groups, groups, skipped_bytes, bytes },
    });
    (kept, note)
}

/// One block's schema, filter and projection, resolved against a query —
/// everything [`activate`] builds a [`RowBatcher`] from, bar the batcher,
/// which holds rows and so is built per activation.
///
/// It records the three inputs the block itself contributes, so a lookup can
/// confirm it answers the activation asking rather than trusting the offset
/// alone; the rest are the [`ReplayPlan`]'s, and fixed.
#[derive(Debug)]
struct PlannedBlock {
    header: CopyHeader,
    field_count: usize,
    database: Option<String>,
    /// The **projected** schema, which is what the batches carry.
    resolved: ResolvedSchema,
    field_targets: Vec<Option<usize>>,
    filter: Arc<ResolvedExpr>,
    notes: Vec<ComparisonNote>,
}

/// Resolve one block for a query: its schema, the filter against the
/// unprojected schema, then the projection — in that order, which is the
/// order their refusals are raised in.
fn resolve_for_query(
    header: &CopyHeader,
    header_offset: u64,
    field_count: usize,
    database: Option<&str>,
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
    census: &[ArrayShape],
) -> Result<PlannedBlock> {
    let full =
        resolve_block(header, field_count, metadata, database, query_options.schema_mode, census)?;
    // Against the *unprojected* schema: a term's index numbers the raw row's
    // fields, and a term may name a column the projection dropped.
    let filter = resolve_expr(&query_options.filter, &full, header_offset)?;
    let notes = filter.comparison_notes();
    let (resolved, field_targets) =
        project(&full, query_options.projection.as_deref(), header_offset)?;
    Ok(PlannedBlock {
        header: header.clone(),
        field_count,
        database: database.map(str::to_string),
        resolved,
        field_targets,
        filter: Arc::new(filter),
        notes,
    })
}

/// **The filter is resolved at plan time**, once per matched block and
/// before any byte of a block is replayed, rather than by each piece of each
/// block as a sub-stream reaches it — so the whole plan's resolved trees are
/// in hand before a segment is cut, and a block split into many pieces
/// resolves once.
///
/// **A resolution refusal is a plan fact**: the first block in file order
/// whose [`resolve_for_query`] refuses — its schema, its filter or its
/// projection, in that order — is the error, before any row of any block
/// (`docs/design/decisions.md`, "D54").
///
/// **A block whose header names no columns** is left out without refusing
/// the plan, its field count being what only its first row says; [`activate`]
/// resolves it where it is reached, and raises its refusal there.
fn plan_blocks(
    matches: &[CopyBlock],
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
    census: &[ArrayShape],
) -> Result<BTreeMap<u64, PlannedBlock>> {
    matches
        .iter()
        .filter(|block| !block.header.columns.is_empty())
        .map(|block| {
            let planned = resolve_for_query(
                &block.header,
                block.header_offset,
                block.header.columns.len(),
                block.database.as_deref(),
                query_options,
                metadata,
                census,
            )?;
            Ok((block.header_offset, planned))
        })
        .collect()
}

/// What state a [`Segment`]'s scanner starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentEntry {
    /// Outside a block, at [`Segment::start`] — the `COPY` header line for a
    /// whole block, so the header the scanner reads is what resolves the
    /// schema. Also how a [`ResumeToken`] that paused *between* blocks
    /// re-enters.
    Header,
    /// Inside the block's data: reading begins at the first row boundary at
    /// or after [`Segment::start`], and the schema comes from the map's own
    /// copy of the header, no header line being in range.
    Interior,
}

/// One contiguous piece of one `COPY` block that a sub-stream replays.
///
/// **The two offsets are not a byte range, and the difference is what makes
/// the pieces tile** (`docs/design/decisions.md`, "D51"). `start` is where the
/// *search* for this piece's first row begins: its first row is the one just
/// past the first LF at or after it. `limit` is not where reading stops: the
/// piece runs through the line that *ends* at the first LF at or after
/// `limit`, which is the row the next piece's search then skips. So a row
/// straddling a cut belongs to the piece before it, once.
#[derive(Debug, Clone)]
struct Segment {
    block: CopyBlock,
    start: u64,
    limit: u64,
    entry: SegmentEntry,
}

impl Segment {
    /// The piece of `block` replaying `run`, a range in segment terms: the
    /// run that begins before the block's first row starts at its header, as
    /// a whole block's first piece does, and any other inside its data.
    fn over(block: &CopyBlock, run: Range<u64>) -> Self {
        let (start, entry) = if run.start < block.data_offset {
            (block.header_offset, SegmentEntry::Header)
        } else {
            (run.start, SegmentEntry::Interior)
        };
        Segment { block: block.clone(), start, limit: run.end, entry }
    }

    /// This piece as a stream resuming at `offset` — a row's start, or 0 —
    /// replays it: `None` where every row it owns lies before `offset`, and
    /// starting at `offset`'s row where the piece began before it. A piece
    /// begun at its block's header is then entered inside the data.
    fn resumed_at(mut self, offset: u64) -> Option<Self> {
        let Some(search) = offset.checked_sub(1) else { return Some(self) };
        if self.limit < offset {
            return None;
        }
        if self.start < search {
            self.start = search;
            self.entry = SegmentEntry::Interior;
        }
        Some(self)
    }

    /// The bytes this piece is responsible for, for balancing sub-streams
    /// against each other. Approximate at both ends by exactly one row, which
    /// is why nothing reads it as an extent. Measured from the block's
    /// **data**, so the first piece is not charged for the header line it also
    /// covers.
    fn weight(&self) -> u64 {
        self.limit.saturating_sub(self.start.max(self.block.data_offset))
    }
}

/// The values a [`TableStream`] publishes to its owner while it runs, plus
/// `plan_notes` — settled once, with the plan and before any row, unlike the
/// rest.
#[derive(Clone)]
struct StreamShared {
    position: Arc<Mutex<ResumeToken>>,
    resolved_schema: Arc<Mutex<ResolvedSchema>>,
    comparison_notes: Arc<Mutex<Vec<ComparisonNote>>>,
    batch_offset: Arc<Mutex<u64>>,
    plan_notes: Arc<Mutex<Vec<PlanNote>>>,
}

impl StreamShared {
    fn new(token: ResumeToken) -> Self {
        Self {
            position: Arc::new(Mutex::new(token)),
            resolved_schema: Arc::new(Mutex::new(ResolvedSchema::default())),
            comparison_notes: Arc::new(Mutex::new(Vec::new())),
            batch_offset: Arc::new(Mutex::new(0)),
            plan_notes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Attach the plan-level notes, known in full before any row is read and
    /// never changed after.
    fn with_plan_notes(self, plan_notes: Vec<PlanNote>) -> Self {
        *self.plan_notes.lock().unwrap() = plan_notes;
        self
    }

    fn into_stream<'a>(
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
        }
    }
}

/// Take one block's resolution from `plan` — resolving it here where the
/// plan holds none ([`plan_blocks`]) — and build the [`RowBatcher`] that will
/// hold its rows.
///
/// The four places a block becomes active — a `COPY` header the scanner
/// read, the first row of a headerless block, a partition that started inside
/// a block, and a resumed token — differ only in where the header and field
/// count come from.
fn activate(
    header: CopyHeader,
    header_offset: u64,
    field_count: usize,
    database: Option<String>,
    plan: &ReplayPlan,
) -> Result<Opened> {
    let resolved_here;
    let block = match plan.blocks.get(&header_offset).filter(|planned| {
        planned.field_count == field_count
            && planned.database == database
            && planned.header == header
    }) {
        Some(planned) => planned,
        None => {
            resolved_here = resolve_for_query(
                &header,
                header_offset,
                field_count,
                database.as_deref(),
                &plan.query_options,
                plan.metadata.as_ref(),
                &plan.census,
            )?;
            &resolved_here
        }
    };
    let batcher = RowBatcher::new(
        &block.resolved,
        header.qualified_name(),
        plan.query_options.clone(),
        block.field_targets.clone(),
    );
    let (resolved, notes) = (block.resolved.clone(), block.notes.clone());
    Ok(((header_offset, header, batcher, Arc::clone(&block.filter), database), resolved, notes))
}

/// The offset of the first row boundary at or after `from`, searching no
/// further than `end` — the byte just past the first LF in `[from, end)`, or
/// `None` when there is none.
///
/// **A partition never resyncs by handing the scanner a mid-row byte**
/// (`docs/design/decisions.md`, "D51"). The scanner in its `InCopy` state
/// treats every line as a row, and a row's tail can be the two bytes `\.`,
/// which it would read as the block's terminator; I7's guarantee is about a
/// *line start*, so it covers a scanner started here and not one started
/// mid-row.
async fn first_row_start(
    source: &dyn ByteRangeSource,
    from: u64,
    end: u64,
    options: &ScanOptions,
) -> Result<Option<u64>> {
    let mut pos = from;
    while pos < end {
        let want = options.chunk_size.min((end - pos) as usize);
        let bytes = source.read_range(pos, want).await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        if let Some(nl) = memchr::memchr(b'\n', &bytes) {
            return Ok(Some(pos + nl as u64 + 1));
        }
        pos += bytes.len() as u64;
        if pos - from > options.max_line_bytes as u64 {
            return Err(Error::LineTooLong { offset: from, limit: options.max_line_bytes });
        }
    }
    Ok(None)
}

/// What a query's mapping pass settled: the blocks this query will replay,
/// the DDL to type them against, and their combined array-shape census.
struct MappedTable {
    matches: Vec<CopyBlock>,
    metadata: Option<DumpMetadata>,
    census: Vec<ArrayShape>,
}

/// The two checks that are about the *request* rather than about the file, so
/// they fire before a byte is read and whether or not the table turns up: a
/// duplicate projection column, and a resume token from another query.
fn validate_request(
    query_options: &QueryOptions,
    resume: Option<&ResumeToken>,
    fingerprint: u64,
) -> Result<()> {
    if let Some(columns) = query_options.projection.as_deref() {
        for (i, name) in columns.iter().enumerate() {
            if columns[..i].contains(name) {
                return Err(Error::DuplicateProjectionColumn { column: name.clone() });
            }
        }
    }
    if resume.is_some_and(|t| t.query_fingerprint != fingerprint) {
        return Err(Error::ResumeQueryMismatch);
    }
    Ok(())
}

/// Pass 1 of a query, whole: load or start the map, capture the preamble,
/// extend the map until this query's table is settled, then narrow the
/// name-only matches to at most one candidate and take their census. Yields
/// no rows — see the module docs. **Both entry points run exactly this**, so a
/// partitioned replay maps the file once rather than once per sub-stream.
async fn map_for_query(
    source: &dyn ByteRangeSource,
    table: &str,
    scan_options: &ScanOptions,
    query_options: &QueryOptions,
    cache: &CacheMode,
) -> Result<MappedTable> {
    let size = source.size().await?;

    let mut index = match cache.load(source).await? {
        CacheLoad::Index(index) => index,
        // Four reasons to start cold, spelled out rather than wildcarded
        // (`docs/design/decisions.md`, "D22").
        CacheLoad::Disabled
        | CacheLoad::Missing
        | CacheLoad::Unreadable
        | CacheLoad::UnsupportedVersion => DumpIndex::default(),
        // The fifth is a refusal, before a byte of the dump is read: this
        // cache describes another file.
        CacheLoad::SourceChanged { cached_stored_size, live_stored_size } => {
            return Err(cache.source_mismatch(cached_stored_size, live_stored_size));
        }
    };

    // The first database's preamble is captured before anything else runs,
    // whatever table this call queries (`docs/design/decisions.md`, "D30").
    // `CacheMode::Disabled` still runs the scan, with `cache.save` a no-op.
    // Persisted immediately, so it survives a caller that polls the stream
    // once and drops it.
    let first_db_preamble_known = index
        .metadata
        .as_ref()
        .and_then(|m| m.databases.first())
        .is_some_and(|db| db.preamble_complete);
    if !first_db_preamble_known {
        // The prepass's spans are kept, not discarded: they tile
        // `[0, preamble_end)`, which is the prefix `map_forward` splices its
        // own output onto.
        let (metadata, spans, preamble_end, roles, tablespaces) =
            scan_preamble(source, scan_options).await?;
        index.metadata = Some(metadata);
        index.spans = splice(&[], spans, 0, preamble_end, size);
        index.roles.extend(roles);
        index.tablespaces.extend(tablespaces);
        index.scanned_through = index.scanned_through.max(preamble_end);
        attach_text(source, &mut index.spans).await?;
        cache.save(source, &index).await?;
    }
    // Pass 1: extend the map until this query's table is settled. No
    // rows come out of this, and nothing is yielded until it returns.
    let selector = query_options.database.as_deref();
    let target = match query_options.scan_extent {
        ScanExtent::UntilTargetSettled => Some((table, selector)),
        ScanExtent::Full => None,
    };
    // A cancelled mapping pass is an error here rather than a short stream
    // (`docs/design/decisions.md`, "D48"). `pgdq query` never sets the flag;
    // an embedder that does gets told.
    if map_forward(source, scan_options, cache, &mut index, target, &StatisticsRequest::NONE, size)
        .await?
        == MapStop::Interrupted
    {
        return Err(Error::ScanCancelled { scanned_through: index.scanned_through });
    }

    // Read *after* the mapping pass, not before it: `map_forward` states the
    // metadata at each `\connect`ed database's first `COPY` block, so the
    // schema depends on the map and never on how far the row replay has got.
    let metadata = index.metadata.clone();

    // One target per query (`docs/design/decisions.md`, "D49"): narrow the
    // name-only matches down to at most one `(database, qualified name)`
    // candidate before reading any of them, so a would-be silent union across
    // schemas or databases errors instead. `query_options.database`, when
    // given, filters candidates first rather than picking among them after
    // the fact. The map being complete already, an ambiguous name errors
    // before a single row goes out.
    let matches: Vec<CopyBlock> = index
        .blocks_for(table)
        .filter(|b| selector.is_none() || b.database.as_deref() == selector)
        .cloned()
        .collect();
    let mut target: Option<(Option<String>, String)> = None;
    for b in &matches {
        let key = (b.database.clone(), b.header.qualified_name());
        match &target {
            None => target = Some(key),
            Some(t) if *t != key => {
                return Err(Error::AmbiguousTable {
                    name: table.to_string(),
                    candidates: vec![render_candidate(t), render_candidate(&key)],
                });
            }
            _ => {}
        }
    }

    // **A streamed schema needs no completeness test**
    // (`docs/design/decisions.md`, "D34"): the mapping pass has finished and
    // `matches` is fixed, so the union below is the evidence for exactly the
    // rows this stream will hand back (`docs/design/decisions.md`, "D35").
    let census = union_census(matches.iter());
    Ok(MappedTable { matches, metadata, census })
}

/// How many sub-streams a caller's [`Parallelism`] and a source's per-partition
/// footprint allow between them.
///
/// **Both numbers bind, and the bytes bind on the *stream* count rather than
/// per block**, the sub-streams being what run at once. A source that states
/// no footprint (the declining default) is bounded by `jobs` alone, and the
/// budget is *solved* against the source's cost, never divided by it
/// ([`crate::io::WorkerMemory::affords`], and
/// `docs/design/decisions.md`, "D4"). **Shared with the leader**, which asks
/// the same question of an open `COPY` block's interior
/// (`crate::leader::scan_region`, and `docs/design/decisions.md`, "D48").
pub(crate) fn worker_count(parallelism: Parallelism, memory: WorkerMemory) -> usize {
    let jobs = parallelism.jobs();
    match parallelism.memory_bytes() {
        Some(budget) if !memory.is_zero() => memory.affords(budget, jobs),
        _ => jobs,
    }
}

/// Cut `range` into at most `want` pieces where `advice` permits, in ascending
/// order and tiling it exactly. `Anywhere` is cut evenly; `At(offsets)` at the
/// source's own boundaries, thinned to `want - 1` of them and evenly spaced
/// through the list when it offers more than the caller can use
/// (`docs/design/decisions.md`, "D15"). An empty `At` is the source declining
/// to be split, and it yields the range whole.
///
/// **Shared with the leader** (`crate::leader::scan_region`), which cuts an
/// open block's interior window with it: one cut rule for the two
/// arrangements (`docs/design/decisions.md`, "D52").
pub(crate) fn cut(range: Range<u64>, advice: &Partitioning, want: usize) -> Vec<Range<u64>> {
    if want <= 1 || range.start >= range.end {
        return vec![range];
    }
    let mut cuts: Vec<u64> = match advice.boundaries() {
        PartitionBoundaries::Anywhere => {
            let len = range.end - range.start;
            let want = want.min(usize::try_from(len).unwrap_or(usize::MAX)).max(1);
            (1..want as u64).map(|i| range.start + len * i / want as u64).collect()
        }
        PartitionBoundaries::At(offsets) => {
            let inside: Vec<u64> =
                offsets.iter().copied().filter(|&o| o > range.start && o < range.end).collect();
            let take = (want - 1).min(inside.len());
            // `n` offers describe `n + 1` pieces, and what is spread evenly
            // is the **pieces**, not the offers: with four offers and three
            // wanted groups the cuts fall after the second and fourth piece.
            // `ceil` rounds that the right way and keeps the picks strictly
            // increasing, so they need no dedup pass.
            (1..=take).map(|j| inside[(j * (inside.len() + 1)).div_ceil(take + 1) - 1]).collect()
        }
    };
    cuts.dedup();
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut start = range.start;
    for cut in cuts {
        out.push(start..cut);
        start = cut;
    }
    out.push(start..range.end);
    out
}

/// One fact about *this query's plan*, as opposed to a fact about the file
/// ([`crate::diagnostic::DiagnosticKind`], L1), about one column
/// (`crate::resolve::ResolvedSchema::notes`, L2) or about one predicate term
/// ([`ComparisonNote`], L4) — a fourth channel, deliberately not a widening of
/// any of the other three (`docs/design/decisions.md`, "D19"). See
/// [`TableStream::plan_notes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanNote {
    pub kind: PlanNoteKind,
}

/// What a [`PlanNote`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanNoteKind {
    /// `--jobs` asked for more concurrent sub-streams than the stated memory
    /// budget affords, counting both terms of the charge [`worker_count`]
    /// solves against: `footprint`, the widest touched block's decode cost
    /// (`crate::io::Partitioning::partition_bytes`), and `max_source_span`
    /// (`crate::batch::QueryOptions::max_source_span`) — `None` where the
    /// caller left the span unbounded or the source retains by the partition
    /// (`crate::io::RetainedUnit`). Never a reason to refuse the query:
    /// `planned` sub-streams run regardless.
    ParallelismBudgetLimited {
        requested: usize,
        planned: usize,
        footprint: u64,
        max_source_span: Option<u64>,
        memory_bytes: u64,
    },
    /// The `.xz` source this query reads declined the block-decode path: the
    /// memory budget in force does not afford one block-decoding reader, so it
    /// reads through the streaming decoder and every **backward** read decodes
    /// forward from its block's start (`docs/design/decisions.md`, "D15").
    /// Never a reason to refuse the query, though the forward mapping pass
    /// pays too: a declined source advises a single partition and so reads
    /// serially whatever `--jobs` says.
    ///
    /// `max_block_uncompressed` is the file's largest block, `block_count`
    /// says how much seeking the file would otherwise offer, and
    /// `reader_bytes` is what the budget was compared against — stated by the
    /// source rather than derived here
    /// ([`crate::io::ByteRangeSource::block_decode_bytes`]).
    ///
    /// **Not a [`crate::diagnostic::DiagnosticKind`]**, for the reason its
    /// sibling above is not one (`docs/design/decisions.md`, "D19").
    CompressedBlockPathDeclined {
        block_count: usize,
        max_block_uncompressed: u64,
        reader_bytes: u64,
        memory_bytes: u64,
    },
    /// The budget in force affords less than a **single** reader of this
    /// source, so the plan runs at the one-slot floors already inside the
    /// mechanism (`crate::stream::worker_count`'s `.max(1)`,
    /// `crate::io::BufferPool`'s clamp to one slot, and a compressed source's
    /// refusal to decode a whole block).
    ///
    /// **It is the arrangement below the reserve, named rather than left to
    /// emerge** (`docs/design/decisions.md`, "D3"): a memory limit at or under
    /// `crate::io::MEMORY_RESERVE` leaves the budget at zero, and
    /// [`PlanNoteKind::ParallelismBudgetLimited`] fires only where `requested`
    /// exceeds what was planned, which at one worker is never.
    ///
    /// `unit_bytes` is `crate::io::Partitioning::partition_bytes` and
    /// `memory_bytes` the budget it was compared against. The **span** term is
    /// deliberately not added: a plain `query` at the default budget already
    /// exceeds it once a batch's pin is counted. Never a reason to refuse
    /// anything.
    AllocationBelowFloor { unit_bytes: u64, memory_bytes: u64 },
    /// The row-group statistics a mapping pass stored proved that
    /// `skipped_groups` of the `groups` listed by the blocks carrying them
    /// hold no row the filter keeps, so their `skipped_bytes` of rows are
    /// never read out of the `bytes` of rows every matched block holds
    /// (`crate::batch::QueryOptions::use_statistics`). Stated wherever a
    /// block's statistics were consulted, a skip of nothing included; never
    /// where none were, nor where the filter keeps every row
    /// (`docs/design/decisions.md`, "D19").
    ///
    /// **A block is consulted by holding statistics that fit it**, not by
    /// holding a usable one for a column the filter reads: a zero also covers
    /// a filtered column left out of the gathered selection, one whose recorded
    /// declared type or collation no longer matches, and one holding only NULL
    /// counts under an operator they cannot answer.
    StatisticsPruned { skipped_groups: u64, groups: u64, skipped_bytes: u64, bytes: u64 },
}

impl PlanNote {
    fn compressed_block_path_declined(
        block_count: usize,
        max_block_uncompressed: u64,
        reader_bytes: u64,
        memory_bytes: u64,
    ) -> Self {
        Self {
            kind: PlanNoteKind::CompressedBlockPathDeclined {
                block_count,
                max_block_uncompressed,
                reader_bytes,
                memory_bytes,
            },
        }
    }

    fn allocation_below_floor(unit_bytes: u64, memory_bytes: u64) -> Self {
        Self { kind: PlanNoteKind::AllocationBelowFloor { unit_bytes, memory_bytes } }
    }

    fn parallelism_budget_limited(
        requested: usize,
        planned: usize,
        footprint: u64,
        max_source_span: Option<u64>,
        memory_bytes: u64,
    ) -> Self {
        Self {
            kind: PlanNoteKind::ParallelismBudgetLimited {
                requested,
                planned,
                footprint,
                max_source_span,
                memory_bytes,
            },
        }
    }

    /// One sentence naming why the plan fell short of what was asked, and what
    /// to raise to close the gap — in the library's own vocabulary rather than
    /// any caller's flag names, as [`ComparisonNote::message`] is.
    pub fn message(&self) -> String {
        match &self.kind {
            PlanNoteKind::ParallelismBudgetLimited {
                requested,
                planned,
                footprint,
                max_source_span,
                memory_bytes,
            } => match max_source_span {
                Some(span) => format!(
                    "asked for up to {requested} sub-stream(s), but a memory budget of \
                     {memory_bytes} byte(s) affords only {planned}: each costs {footprint} \
                     byte(s) to decode plus {span} byte(s) held by its own batch — raise the \
                     memory budget, or lower the batch span, to get more"
                ),
                None => format!(
                    "asked for up to {requested} sub-stream(s), but a memory budget of \
                     {memory_bytes} byte(s) affords only {planned} at {footprint} byte(s) to \
                     decode each — raise the memory budget to get more"
                ),
            },
            PlanNoteKind::CompressedBlockPathDeclined {
                block_count,
                max_block_uncompressed,
                reader_bytes,
                memory_bytes,
            } => format!(
                "this .xz source has {block_count} block(s) to seek by, but its largest is \
                 {max_block_uncompressed} byte(s) and a memory budget of {memory_bytes} byte(s) \
                 leaves no room for one reader of it — so it is read through the streaming \
                 decoder and every backward read decodes forward from its block's start; raise \
                 the memory budget to {reader_bytes} byte(s) or more to read it a block at a time"
            ),
            PlanNoteKind::AllocationBelowFloor { unit_bytes, memory_bytes } => format!(
                "a memory budget of {memory_bytes} byte(s) is less than the {unit_bytes} byte(s) \
                 one reader of this source holds, so this runs at its one-slot floor whatever \
                 concurrency is asked for — the budget in force is what bound it, and where \
                 nothing stated one it is the memory limit this process is running under"
            ),
            PlanNoteKind::StatisticsPruned { skipped_groups, groups, skipped_bytes, bytes } => {
                format!(
                    "row-group statistics rule out {skipped_groups} of {groups} group(s), so \
                     {skipped_bytes} of the {bytes} byte(s) of rows this table holds are not read"
                )
            }
        }
    }
}

/// Whether this source is a compressed one that *could* be read a
/// block at a time and is not, because the budget in force leaves no room to
/// hold a whole block ([`crate::io::Partitioning`], and
/// `docs/design/decisions.md`, "D15").
///
/// **Read off `partitions()`, not off a budget the caller would have to hand
/// down**: a source holding a seek table with more than one block and still
/// advising a *single* partition over the whole of it has declined. What the
/// source *is* asked for is the recourse the message names
/// ([`crate::io::ByteRangeSource::block_decode_bytes`]). **Asked over the
/// whole file**, since a query whose rows all sit inside one compressed block
/// would be advised one partition on the block path too. `None` for every
/// plain source and for a file with no more than one block, which is already
/// `DiagnosticKind::NonSeekableCompressedSource`.
fn compressed_block_path_declined(
    source: &dyn ByteRangeSource,
    parallelism: Parallelism,
) -> Option<PlanNote> {
    let table = source.seek_table()?;
    if !table.is_seekable() {
        return None;
    }
    if source.partitions(0..table.uncompressed_size()).max_partitions() != Some(1) {
        return None;
    }
    Some(PlanNote::compressed_block_path_declined(
        table.block_count(),
        table.max_block_uncompressed(),
        // What a reader of this file would have held on the path it declined.
        // The fallback is unreachable, but nothing in the trait obliges the
        // two answers to agree.
        source.block_decode_bytes().unwrap_or(0),
        // The budget actually in force, which is what the source compared its
        // reader against: a caller that stated no number leaves every pool on
        // this one.
        parallelism.memory_bytes().unwrap_or(DEFAULT_MEMORY_BUDGET),
    ))
}

/// Split `matches` into the pieces `parallelism` and the source between them
/// allow, then group those pieces into sub-streams — each internally in file
/// order, and the groups themselves in file order, so concatenating them is
/// the serial replay (`docs/design/decisions.md`, "D51").
///
/// Never empty: a table with no blocks at all is one sub-stream that yields
/// nothing, which is what [`table_stream`] does with the same map.
///
/// **`max_source_span` is the second term the stated budget is solved against,
/// not a separate cap of its own** (`docs/design/decisions.md`, "D4"). What one
/// sub-stream costs the caller is its held
/// batch's pin (`max_source_span`, rounded out to the retained unit) *on top
/// of* what the source charges a concurrent reader for decoding
/// (`partition_bytes`) — a discovery worker pays only the second, but a
/// query's sub-stream is handed its batch and pays both, which is why this
/// charge is not `worker_count`'s own to know and is computed here rather
/// than folded into that function. A caller who left the span unbounded
/// (`None`) has already opted out of a batch-size bound, so the charge falls
/// back to the decode footprint alone — the same answer a discovery worker's
/// call gets.
///
/// **The span is charged per source, not universally**, because the second
/// term is honest for one source shape and double-counts for the other: a
/// source retaining by the read chunk pins bytes `partition_bytes` never
/// charged for, and one retaining by the partition pins bytes it did
/// (`crate::io::RetainedUnit`). So the term is added only where the advice
/// says [`crate::io::RetainedUnit::ReadChunk`], and where it is not, the note
/// below reads back `None` rather than a number that was never charged. An
/// **empty** `matches` has no advice to read, so it keeps the charge: nothing
/// runs either way, and the arm that says nothing is the one that charges
/// more.
///
/// **The second return value is the plan's own notes: at most one naming why
/// `workers` came up short of `parallelism.jobs()`, at most one naming a
/// compressed source that declined the block-decode path under this budget
/// ([`compressed_block_path_declined`]), and at most one naming a budget that
/// affords less than a single reader ([`PlanNoteKind::AllocationBelowFloor`]).**
/// Empty on every path that limits nothing — an empty `matches` included,
/// since a footprint of zero never trips the budget — so a caller need not
/// special-case "nothing to say".
///
/// Deficiency register: `deficiency: KD17` — the sub-streams planned here
/// never run concurrently on a plain typed `query`: throughput is flat across
/// the whole `--jobs` axis and total CPU stays under one core
/// (`measurements.md`, `parallel-scan-throughput`). The named suspect, `POOL_DEPTH`
/// clamping the chunk pool, is spent — a probe build lifting it moved no cell
/// materially — so what serializes them is unidentified. **(c) unowned**;
/// promoted by a phase taking up plain-source extraction throughput.
///
/// Deficiency register: `deficiency: KD23` — the cut here is over a whole
/// `CopyBlock` rather than through the leader's window, so a piece spans as
/// many units as the region holds over the worker count and a held batch's
/// `max_source_span` can pin several decoded blocks where
/// [`crate::io::RetainedUnit::Partition`] bills one;
/// `BOUNDARIED_PARTITION_UNITS` does not bound this path. **(c) unowned**;
/// promoted by a phase taking up query-path memory, the repair reversing a
/// recorded decision either way.
fn plan_partitions(
    source: &dyn ByteRangeSource,
    matches: &[CopyBlock],
    kept: &BTreeMap<u64, Vec<Range<u64>>>,
    parallelism: Parallelism,
    max_source_span: Option<usize>,
) -> (Vec<Vec<Segment>>, Vec<PlanNote>) {
    // **Announced before the advice is asked for**, since a compressed source
    // decides from the stated budget whether it can decode a whole block at
    // all (`ByteRangeSource::partitions`): asking under the mapping pass's
    // budget would plan against a read path the replay will not take. Each
    // sub-stream re-announces the same value, idempotently.
    source.hint_parallelism(parallelism);
    // A block's own advice, and its footprint, are read once per block; the
    // footprint that decides the sub-stream count is the largest of them.
    let advice: Vec<Partitioning> =
        matches.iter().map(|b| source.partitions(b.data_offset..b.end_offset)).collect();
    let footprint = advice.iter().map(Partitioning::partition_bytes).max().unwrap_or(0);
    // The span is a cost of a chunk-shaped source only. `all` over an empty
    // advice would be vacuously true, so the emptiness is tested.
    let retains_partitions =
        !advice.is_empty() && advice.iter().all(|a| a.retained_unit() == RetainedUnit::Partition);
    let charged_span = if retains_partitions { None } else { max_source_span };
    // The charge the count is solved against: the largest per-worker footprint
    // above, carrying that source's shared pool term, plus this caller's span.
    let charge = advice
        .iter()
        .map(Partitioning::worker_memory)
        .max_by_key(|memory| memory.bytes_per_worker())
        .unwrap_or_default();
    let charge = match charged_span {
        Some(span) => charge.plus_per_worker(span as u64),
        None => charge,
    };
    let workers = worker_count(parallelism, charge).max(1);
    let requested = parallelism.jobs();
    // The decline comes first because it is the wider fact: how every read of
    // this file is served, where the count below is how many readers there are.
    let mut notes: Vec<PlanNote> =
        compressed_block_path_declined(source, parallelism).into_iter().collect();
    // **Below one reader's worth the floors decide, and they are silent**, the
    // count note below firing only where `requested` exceeds what was planned.
    // Charged against the footprint alone rather than the whole charge: the
    // span term puts an ordinary plain `query` at the default budget over the
    // line (`docs/design/decisions.md`, "D3").
    if let Some(memory_bytes) = parallelism.memory_bytes()
        && footprint > 0
        && memory_bytes < footprint
    {
        notes.push(PlanNote::allocation_below_floor(footprint, memory_bytes));
    }
    if let Some(memory_bytes) = parallelism.memory_bytes()
        && workers < requested
    {
        notes.push(PlanNote::parallelism_budget_limited(
            requested,
            workers,
            footprint,
            charged_span.map(|span| span as u64),
            memory_bytes,
        ));
    }

    let mut segments = Vec::new();
    for (block, advice) in matches.iter().zip(&advice) {
        let want = match advice.max_partitions() {
            Some(max) => workers.min(max),
            None => workers,
        };
        // A pruned block's runs are cut instead of its data, each into a
        // share of `want` proportional to its bytes, so the readers are
        // balanced over what remains to be read.
        let Some(runs) = kept.get(&block.header_offset) else {
            // The **data** range is what is cut, not `[header_offset, …)`: a
            // cut inside the header line would give the first piece no rows.
            // The first piece is then extended back over the header, its
            // schema's source.
            for (i, piece) in
                cut(block.data_offset..block.end_offset, advice, want).into_iter().enumerate()
            {
                let (start, entry) = if i == 0 {
                    (block.header_offset, SegmentEntry::Header)
                } else {
                    (piece.start, SegmentEntry::Interior)
                };
                segments.push(Segment { block: block.clone(), start, limit: piece.end, entry });
            }
            continue;
        };
        let remaining: u64 = runs.iter().map(|run| run.end - run.start).sum();
        for run in runs {
            let share = (want as u64 * (run.end - run.start)).div_ceil(remaining.max(1));
            let share = usize::try_from(share).unwrap_or(want).clamp(1, want);
            // A run starting on the header's LF is cut from the first data
            // byte, for the reason the whole block's data range is.
            let from = run.start.max(block.data_offset);
            for piece in cut(from..run.end, advice, share) {
                let start = if piece.start == from { run.start } else { piece.start };
                segments.push(Segment::over(block, start..piece.end));
            }
        }
    }
    (distribute(segments, workers), notes)
}

/// Group `segments` into at most `streams` contiguous, byte-balanced runs,
/// dropping the empty ones. **Contiguous rather than round-robin**, so
/// concatenating the sub-streams in order equals the serial replay
/// (`docs/design/decisions.md`, "D51"). The group is chosen from a segment's
/// **midpoint** in the running total, so one huge piece beside many small ones
/// does not push everything after it into the last group.
fn distribute(segments: Vec<Segment>, streams: usize) -> Vec<Vec<Segment>> {
    let streams = streams.max(1).min(segments.len().max(1));
    if streams == 1 {
        return vec![segments];
    }
    let total: u64 = segments.iter().map(Segment::weight).sum();
    let mut groups: Vec<Vec<Segment>> = vec![Vec::new(); streams];
    let mut walked = 0u64;
    for segment in segments {
        let weight = segment.weight();
        // Zero total is every piece empty, which is one group's worth of work
        // however many pieces there are.
        let group = ((walked + weight / 2) * streams as u64)
            .checked_div(total)
            .and_then(|g| usize::try_from(g).ok())
            .unwrap_or(0);
        groups[group.min(streams - 1)].push(segment);
        walked += weight;
    }
    groups.retain(|group| !group.is_empty());
    if groups.is_empty() {
        groups.push(Vec::new());
    }
    groups
}

/// Pass 2, for one sub-stream: replay `segments` in file order, in whatever
/// batches `plan.query_options` asks for. This is the whole of the row path: a
/// serial [`table_stream`] is this function over one segment per matching
/// block, a partitioned replay it over each group [`plan_partitions`] handed
/// out.
fn replay<'a>(
    source: &'a dyn ByteRangeSource,
    plan: Arc<ReplayPlan>,
    segments: Vec<Segment>,
    shared: StreamShared,
    resume: Option<ResumeToken>,
    fingerprint: u64,
) -> impl Stream<Item = Result<RecordBatch>> + Send + 'a {
    try_stream! {
        let scan_options = &plan.scan_options;
        let query_options = &plan.query_options;

        // The chunk length every segment's replay repeats, announced once for
        // the whole sub-stream (`ByteRangeSource::hint_read_size`). The budget
        // comes from `QueryOptions`, not `ScanOptions`: a query states the two
        // passes' parallelism separately because they split differently.
        source.hint_read_size(scan_options.chunk_size);
        source.hint_parallelism(query_options.parallelism);
        // **The replay loop could not grant a wait**
        // (`docs/design/decisions.md`, "D5"): `RetainedChunks` pins every
        // chunk a batch has taken a `Utf8View` into until that batch flushes,
        // so this loop can never be the task that frees one it waits on.
        // Stated rather than left to the default, so whatever the mapping pass
        // granted is un-stated on the same source.
        source.hint_wait_policy(WaitPolicy::NeverWait);

        let mut rows_emitted = resume.as_ref().map_or(0, |t| t.rows_emitted);
        // Where a token paused inside a block, and which block: its scanner
        // resumes the first segment only where that segment holds the pause.
        let paused_in = resume
            .as_ref()
            .and_then(|t| t.in_copy.as_ref().map(|in_copy| (t.offset, in_copy.header_offset)));

        // Only the first segment can start mid-row (a resumed stream paused
        // between two of one block's rows); its scanner and in-flight batcher
        // are prebuilt here.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) if token.in_copy.is_some() => {
                let (scanner, opened) = resume_state(token, &plan)?;
                let active = opened.map(|(active, resolved, notes)| {
                    *shared.resolved_schema.lock().unwrap() = resolved;
                    *shared.comparison_notes.lock().unwrap() = notes;
                    active
                });
                (active, Some(scanner))
            }
            _ => (None, None),
        };
        // A matching header with no column list, waiting on its first row to
        // learn the field count. Never non-empty across a resume point: a
        // stream only yields right after a flush, by which time a pending
        // headerless block has seen its first row.
        let mut pending: Option<(CopyHeader, u64, Option<String>)> = None;
        // One buffer for the whole replay: every row of a block has the same
        // width, so after the first it never grows again.
        let mut split = RowSplit::default();

        for segment in segments {
            let block = &segment.block;
            let seg_limit = segment.limit;
            let seg_end = block.end_offset;
            let block_database = block.database.clone();

            let resumes_here = paused_in.is_some_and(|(offset, header_offset)| {
                header_offset == block.header_offset
                    && segment.start < offset
                    && offset <= seg_limit
            });
            let resumed = first_scanner.take();
            if resumed.is_some() && !resumes_here {
                // The pause lies before this segment, and every row between
                // the two is in a group pruning skipped: the paused block
                // state holds nothing and is dropped.
                active = None;
            }
            let mut scanner = match resumed.filter(|_| resumes_here) {
                Some(scanner) => scanner,
                None => match segment.entry {
                    SegmentEntry::Header => CopyScanner::resume(segment.start, None),
                    SegmentEntry::Interior => {
                        // Where this piece's first row starts — and whether
                        // it has one at all. A piece whose search lands past
                        // its own limit had two cuts fall inside one row: the
                        // row belongs to the piece before it.
                        let row_start =
                            first_row_start(source, segment.start, seg_end, scan_options)
                                .await?
                                .filter(|&start| start <= seg_limit && start < seg_end);
                        // No header line is in range, so the schema comes off
                        // the map's own copy of it — resolved now when the
                        // header named its columns, deferred to the first row
                        // when it did not, as the live paths below do.
                        if block.header.columns.is_empty() {
                            let Some(row_start) = row_start else { continue };
                            pending = Some((
                                block.header.clone(),
                                block.header_offset,
                                block_database.clone(),
                            ));
                            CopyScanner::resume(row_start, Some((block.header_offset, 0)))
                        } else {
                            let (opened, resolved, notes) = activate(
                                block.header.clone(),
                                block.header_offset,
                                block.header.columns.len(),
                                block_database.clone(),
                                &plan,
                            )?;
                            // Published even for a piece holding no row, so a
                            // sub-stream handed only such pieces — cuts inside
                            // one row, or the runs pruning left — reports the
                            // block's schema as one entered at its header does.
                            *shared.comparison_notes.lock().unwrap() = notes;
                            *shared.resolved_schema.lock().unwrap() = resolved;
                            let Some(row_start) = row_start else { continue };
                            active = Some(opened);
                            CopyScanner::resume(row_start, Some((block.header_offset, 0)))
                        }
                    }
                },
            };
            let mut read_pos = scanner.position();
            let mut carry = ChunkCarry::new();
            let mut chunks = RetainedChunks::new();
            // Set once this segment has read the line that ends at or past
            // its limit — the last it owns, and the one the next segment's
            // search skips, so the two tile.
            let mut past_limit = false;

            loop {
                let want = scan_options.chunk_size.min((seg_end - read_pos) as usize);
                let chunk = if want > 0 {
                    let bytes = source.read_range(read_pos, want).await?;
                    chunks.retain(read_pos, &bytes);
                    read_pos += bytes.len() as u64;
                    bytes
                } else {
                    Bytes::new()
                };
                let eof = read_pos >= seg_end;

                carry.absorb(&chunk);
                for pass in ChunkCarry::PASSES {
                    let (span, span_eof) = carry.span(pass, &chunk, eof);
                    // `pos` is 0 at the top of every pass — `take_consumed`
                    // resets it — so this is the absolute offset of `span[0]`,
                    // which turns a row's file offset into a position in
                    // `validated` below.
                    let span_base = scanner.position();
                    // The span's rows, validated as UTF-8 in one pass rather
                    // than one `from_utf8` per field. **Taken on the first row
                    // that will decode something**, so a query that decodes
                    // nothing pays nothing (`docs/design/decisions.md`, "D27").
                    let mut validated: Option<&str> = None;
                    while let Some(event) = scanner.next_event(span, span_eof)? {
                        match event {
                            Event::CopyStart(start) => {
                                if start.header.columns.is_empty() {
                                    pending = Some((
                                        start.header,
                                        start.header_offset,
                                        block_database.clone(),
                                    ));
                                } else {
                                    let field_count = start.header.columns.len();
                                    let (opened, resolved, notes) = activate(
                                        start.header,
                                        start.header_offset,
                                        field_count,
                                        block_database.clone(),
                                        &plan,
                                    )?;
                                    *shared.comparison_notes.lock().unwrap() = notes;
                                    *shared.resolved_schema.lock().unwrap() = resolved;
                                    active = Some(opened);
                                }
                            }
                            Event::Row(row) => {
                                if let Some((header, header_offset, block_database)) =
                                    pending.take()
                                {
                                    let field_count = if header.columns.is_empty() {
                                        memchr::memchr_iter(DELIMITER, row.raw).count() + 1
                                    } else {
                                        header.columns.len()
                                    };
                                    let (opened, resolved, notes) = activate(
                                        header,
                                        header_offset,
                                        field_count,
                                        block_database,
                                        &plan,
                                    )?;
                                    *shared.comparison_notes.lock().unwrap() = notes;
                                    *shared.resolved_schema.lock().unwrap() = resolved;
                                    active = Some(opened);
                                }
                                if let Some((header_offset, _, batcher, filter, _)) =
                                    active.as_mut()
                                {
                                    let unchecked = RawRow::unchecked(row.raw);
                                    let raw = if batcher.decodes_fields()
                                        || filter.reads_fields()
                                    {
                                        let prefix = *validated
                                            .get_or_insert_with(|| validated_prefix(span));
                                        row_text(prefix, span_base, &row)
                                            .map_or(unchecked, RawRow::validated)
                                    } else {
                                        unchecked
                                    };
                                    // One split per row, shared
                                    // (`docs/design/decisions.md`, "D28").
                                    split.restart();
                                    let keep = filter.matches(
                                        raw,
                                        &mut split,
                                        batcher.table(),
                                        row.offset,
                                    )?;
                                    if keep {
                                        batcher.push_row(
                                            *header_offset,
                                            row.offset,
                                            raw,
                                            &mut split,
                                            &mut chunks,
                                        )?;
                                    }
                                    if batcher.should_flush() {
                                        let begins_at = batcher
                                            .batch_start()
                                            .unwrap_or_else(|| scanner.position());
                                        let batch = batcher.flush()?;
                                        chunks.invalidate_block_cache();
                                        rows_emitted += batch.num_rows() as u64;
                                        *shared.batch_offset.lock().unwrap() = begins_at;
                                        *shared.position.lock().unwrap() =
                                            snapshot(&scanner, &active, rows_emitted, fingerprint);
                                        yield batch;
                                    }
                                }
                            }
                            Event::CopyEnd(_) => {
                                pending = None;
                                if let Some((_, _, mut batcher, _, _)) = active.take()
                                    && !batcher.is_empty()
                                {
                                    let begins_at =
                                        batcher.batch_start().unwrap_or_else(|| scanner.position());
                                    let batch = batcher.flush()?;
                                    chunks.invalidate_block_cache();
                                    rows_emitted += batch.num_rows() as u64;
                                    *shared.batch_offset.lock().unwrap() = begins_at;
                                    *shared.position.lock().unwrap() =
                                        snapshot(&scanner, &active, rows_emitted, fingerprint);
                                    yield batch;
                                }
                            }
                            // A replay segment covers exactly one block, so
                            // the only non-row line in range is the `COPY`
                            // header itself, which arrives as `CopyStart`.
                            Event::Line(_) | Event::DollarQuoteEnd(_) => {}
                            Event::LargeObjectStart(_) | Event::LargeObjectEnd(_) => {}
                        }
                        // The line just consumed ended at or past this
                        // segment's limit, so it was its last. Checked after
                        // the event, the straddling row being *this*
                        // segment's.
                        if scanner.position() > seg_limit {
                            past_limit = true;
                            break;
                        }
                    }
                    carry.consumed(pass, &chunk, scanner.take_consumed());
                    if past_limit {
                        break;
                    }
                }

                // Everything before the scanner's new position has already had
                // its chance to be referenced by a zero-copy view, so it is
                // safe to drop. Which chunks that releases is
                // `RetainedChunks`' rule, not this loop's.
                chunks.release_through(scanner.position());

                if past_limit || eof {
                    break;
                }
                if carry.len() > scan_options.max_line_bytes {
                    Err(Error::LineTooLong {
                        offset: scanner.position(),
                        limit: scan_options.max_line_bytes,
                    })?;
                }
            }

            // A segment that stopped at its limit rather than at the block's
            // `\.` has no `CopyEnd` to flush it, so it flushes here — and
            // clears the block state either way, the next segment possibly
            // being a different block with a different schema.
            pending = None;
            if let Some((_, _, mut batcher, _, _)) = active.take()
                && !batcher.is_empty()
            {
                let begins_at = batcher.batch_start().unwrap_or_else(|| scanner.position());
                let batch = batcher.flush()?;
                rows_emitted += batch.num_rows() as u64;
                *shared.batch_offset.lock().unwrap() = begins_at;
                *shared.position.lock().unwrap() =
                    snapshot(&scanner, &active, rows_emitted, fingerprint);
                yield batch;
            }
        }
    }
}

/// Pull-mode entry point: stream `Utf8View` `RecordBatch`es for every row of
/// every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`]). A table with zero rows yields no batches.
///
/// `resume` continues a previous consumption from a [`ResumeToken`] it
/// produced; `None` starts from the beginning of `source`. A token whose query
/// fingerprint disagrees with `query_options` is `Error::ResumeQueryMismatch`.
///
/// `query_options.filters` applies the post-parse row filter as a conjunction
/// (`docs/design/decisions.md`, "D54"): an empty list yields every row, and
/// otherwise a row is kept only if **every** term matches, tested after that
/// row has been fully unescaped. A term referencing a column absent from a
/// matching block's own schema is `Error::UnknownPredicateColumn`. Every
/// refusal resolving a block raises — schema, filter or projection — is
/// yielded before any row of the table, for the first refusing block in file
/// order (`docs/design/decisions.md`, "D54").
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
/// of the file that pass walks before any row comes back; see [`ScanExtent`].
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
        let mapped =
            map_for_query(source, &table, &scan_options, &query_options, &cache).await?;

        // Pass 2: replay each matching block for its rows, as one segment
        // apiece. A resumed stream picks up inside this same list, every
        // resume point being inside a mapped block by construction.
        let MappedTable { matches, metadata, census } = mapped;
        let plan =
            Arc::new(ReplayPlan::new(scan_options, query_options, &matches, metadata, census)?);
        *shared_for_stream.plan_notes.lock().unwrap() = plan.pruned.iter().cloned().collect();
        let resume_offset = resume.as_ref().map_or(0, |t| t.offset);
        let segments: Vec<Segment> = matches
            .iter()
            .filter(|b| b.end_offset > resume_offset)
            .flat_map(|b| plan.segments(b))
            .filter_map(|segment| segment.resumed_at(resume_offset))
            .collect();
        let mut rows =
            Box::pin(replay(source, plan, segments, shared_for_stream, resume, fingerprint));
        while let Some(batch) = rows.next().await {
            yield batch?;
        }
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
/// an empty sub-stream beyond the degenerate one a table with no rows
/// produces; the serial state is one sub-stream.
///
/// **Each sub-stream carries its own schema, notes and position.**
/// [`TableStream::resolved_schema`] is empty on a sub-stream until that
/// sub-stream's first block resolves, so a caller wanting the schema before
/// consuming anything reads it off the *first* sub-stream, whose first segment
/// is the first block's and publishes its schema on entry, holding a row or
/// not. [`TableStream::resume_token`] is stamped with the
/// partition it came from, so feeding one back to [`table_stream`] is
/// `Error::ResumeQueryMismatch`: resuming a partitioned replay is not
/// supported.
///
/// **What N sub-streams cost resident is N times one**, and N is solved
/// against the sum of what a sub-stream's in-flight batch pins
/// (`QueryOptions::max_source_span`) and what the source charges a concurrent
/// reader (`Partitioning::partition_bytes`)
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
    let mapped = map_for_query(source, &table, &scan_options, &query_options, &cache).await?;
    let MappedTable { matches, metadata, census } = mapped;
    let plan = Arc::new(ReplayPlan::new(scan_options, query_options, &matches, metadata, census)?);
    let (groups, mut plan_notes) = plan_partitions(
        source,
        &matches,
        &plan.kept,
        plan.query_options.parallelism,
        plan.query_options.max_source_span,
    );
    plan_notes.extend(plan.pruned.iter().cloned());
    let of = groups.len();
    Ok(groups
        .into_iter()
        .enumerate()
        .map(|(index, segments)| {
            let fingerprint = query_fingerprint(&table, &plan.query_options, Some((index, of)));
            let shared = StreamShared::new(ResumeToken::start(fingerprint))
                .with_plan_notes(plan_notes.clone());
            let inner =
                replay(source, Arc::clone(&plan), segments, shared.clone(), None, fingerprint);
            shared.into_stream(Box::pin(inner))
        })
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The throttle's whole rule, over measured quantities rather than a
    /// clock: a save that cost nothing never blocks another, and a save that
    /// cost something blocks the next one until the scan has done `K` times
    /// that much work.
    #[test]
    fn a_save_is_due_once_the_scan_has_outrun_the_last_ones_cost() {
        let ms = Duration::from_millis;

        assert!(SaveThrottle::due_after(Duration::ZERO, Duration::ZERO), "a free save never waits");
        assert!(SaveThrottle::due_after(ms(1), Duration::ZERO));

        // K = 20: 10ms of saving buys 200ms of silence.
        assert!(!SaveThrottle::due_after(ms(199), ms(10)));
        assert!(SaveThrottle::due_after(ms(200), ms(10)));
        assert!(SaveThrottle::due_after(ms(1000), ms(10)));

        // Blocks far apart and saves far cheaper than they are: untouched.
        assert!(SaveThrottle::due_after(Duration::from_secs(45), ms(300)));

        // And the pathological one: a cache expensive enough that saving it
        // every block is the scan.
        assert!(!SaveThrottle::due_after(ms(11), ms(10)));
    }

    /// A `last_cost` big enough to overflow `Duration * u32` saturates instead
    /// of panicking. Unreachable in practice — it takes a save of over seven
    /// years — but the multiplication is on the hot path of every block.
    #[test]
    fn an_absurd_save_cost_saturates_rather_than_panicking() {
        assert!(!SaveThrottle::due_after(Duration::MAX / 2, Duration::MAX));
    }

    /// **The guard no caller can trip, pinned so it stays that way.**
    /// [`resolve_block`] refuses `Typed` resolution against metadata with no
    /// complete entry for the block's database, which every call site
    /// satisfies by construction — so `Error::MetadataNotScanned` is
    /// unreachable through every public entry point, and this is what stands
    /// between a future reordering and a silently wrongly-typed row.
    #[test]
    fn resolving_a_block_against_a_database_the_metadata_lacks_refuses() {
        use crate::preamble::DatabaseMetadata;

        let header = crate::copy::parse_copy_header(b"COPY public.t (id) FROM stdin;")
            .expect("a well-formed header");
        let first = DatabaseMetadata {
            name: Some("first".to_string()),
            preamble_complete: true,
            server_version: None,
            pg_dump_version: None,
            extensions: Vec::new(),
            types: Vec::new(),
            collations: Vec::new(),
            tables: Default::default(),
        };
        let metadata = DumpMetadata { databases: vec![first] };

        let err =
            resolve_block(&header, 1, Some(&metadata), Some("second"), SchemaMode::Typed, &[])
                .expect_err("the metadata has no entry for `second`");
        assert!(
            matches!(&err, Error::MetadataNotScanned { database } if database.as_deref() == Some("second")),
            "{err:?}"
        );

        // The same block in a database the metadata covers resolves, so the
        // refusal is about coverage and not about the lookup failing.
        assert!(
            resolve_block(&header, 1, Some(&metadata), Some("first"), SchemaMode::Typed, &[])
                .is_ok()
        );

        // `Strings` never looks, so it is never refused.
        assert!(
            resolve_block(&header, 1, Some(&metadata), Some("second"), SchemaMode::Strings, &[])
                .is_ok()
        );
    }

    /// The two properties every cut has to have, whatever the source advised:
    /// the pieces **tile** the range exactly, in ascending order, and there
    /// are never more of them than the caller asked for.
    fn assert_tiles(range: Range<u64>, pieces: &[Range<u64>], want: usize) {
        assert!(!pieces.is_empty(), "a cut always yields at least the range itself");
        assert!(pieces.len() <= want.max(1), "{} pieces for want {want}", pieces.len());
        assert_eq!(pieces[0].start, range.start);
        assert_eq!(pieces[pieces.len() - 1].end, range.end);
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "{pieces:?}");
        }
    }

    /// `Anywhere` is cut evenly, and never into pieces of zero bytes: a range
    /// shorter than the worker count is cut into one piece per byte and no
    /// finer, a piece with no bytes being able to own no rows.
    #[test]
    fn an_anywhere_source_is_cut_evenly_and_never_below_a_byte() {
        let advice = Partitioning::anywhere(1 << 20);
        for want in [1usize, 2, 3, 4, 7, 8, 64] {
            let pieces = cut(100..900, &advice, want);
            assert_tiles(100..900, &pieces, want);
            assert_eq!(pieces.len(), want.max(1));
        }
        let pieces = cut(10..13, &advice, 8);
        assert_eq!(pieces, vec![10..11, 11..12, 12..13]);
    }

    /// `At` is cut only where the source offered, thinned evenly when it
    /// offers more boundaries than the caller can use, and taken whole when it
    /// offers exactly as many as are wanted.
    #[test]
    fn an_at_source_is_cut_only_where_it_offered() {
        let advice = Partitioning::at(vec![20, 40, 60, 80], 1 << 20);
        assert_eq!(cut(0..100, &advice, 5), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
        assert_eq!(cut(0..100, &advice, 9), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
        assert_eq!(cut(0..100, &advice, 3), vec![0..40, 40..80, 80..100]);
        assert_eq!(cut(0..100, &advice, 2), vec![0..60, 60..100]);
        // Boundaries outside the range are not cuts, nor are its own ends.
        assert_eq!(cut(40..80, &advice, 4), vec![40..60, 60..80]);
        for want in [1usize, 2, 3, 4, 5, 9] {
            assert_tiles(0..100, &cut(0..100, &advice, want), want);
        }
    }

    /// A source that declines to be split is not split, however many workers
    /// the caller has — the empty `At` is a policy, not an absence
    /// (`docs/design/decisions.md`, "D7").
    #[test]
    fn a_source_that_declines_to_be_split_is_not() {
        assert_eq!(cut(0..1000, &Partitioning::single(0), 16), vec![0..1000]);
    }

    /// The bytes bind as well as the count, and they bind on the number of
    /// **sub-streams**: eight workers against a budget that affords two
    /// partitions is two.
    #[test]
    fn a_stated_budget_caps_the_worker_count_below_the_stated_jobs() {
        let eight = Parallelism::workers(8, 64 << 20);
        let per = WorkerMemory::per_worker;
        assert_eq!(worker_count(eight, per(32 << 20)), 2);
        assert_eq!(worker_count(eight, per(4 << 20)), 8);
        // A footprint larger than the whole budget still leaves one worker.
        assert_eq!(worker_count(eight, per(128 << 20)), 1);
        // A source that states no footprint is bounded by `jobs` alone.
        assert_eq!(worker_count(eight, per(0)), 8);
        // The serial state is one worker, whatever budget it carries.
        assert_eq!(worker_count(Parallelism::default(), per(32 << 20)), 1);
        assert_eq!(worker_count(Parallelism::workers(1, 1 << 30), per(32 << 20)), 1);
    }

    /// **A shared pool is billed, so the budget is solved rather than
    /// divided.** The block pool retains a unit for every slot but the one a
    /// reader is filling — `POOL_DEPTH - 1` of them below four readers and
    /// `jobs - 1` above — which a per-worker charge does not carry.
    #[test]
    fn a_shared_pool_is_billed_before_the_count_is_handed_back() {
        let eight = Parallelism::workers(8, 64 << 20);
        let charge = WorkerMemory::per_worker(16 << 20).pooling(16 << 20, 4);
        // One reader costs 16 + 3 x 16 = 64 MiB, two cost 32 + 48 = 80 — so
        // this budget affords exactly one where the division said four.
        assert_eq!(worker_count(eight, charge), 1);
        // One byte short, and the serial floor is what is left: never zero.
        assert_eq!(worker_count(Parallelism::workers(8, (64 << 20) - 1), charge), 1);
        // Above the pool's own depth every reader takes a slot with it, so the
        // cost is `n x 32 - 16` MiB: 240 MiB affords eight, and 224 seven.
        assert_eq!(worker_count(Parallelism::workers(8, 240 << 20), charge), 8);
        assert_eq!(worker_count(Parallelism::workers(8, 224 << 20), charge), 7);
    }

    /// Grouping keeps the pieces in file order and contiguous, which is what
    /// makes concatenating the sub-streams equal the serial replay, and it
    /// balances by bytes.
    #[test]
    fn sub_streams_get_contiguous_runs_balanced_by_bytes() {
        let block = CopyBlock {
            header: crate::copy::parse_copy_header(b"COPY public.t (id) FROM stdin;").unwrap(),
            database: None,
            header_offset: 0,
            data_offset: 0,
            terminator_offset: 0,
            end_offset: 0,
            row_count: 0,
            partition_root: None,
            statistics: None,
            array_shapes: Vec::new(),
        };
        let piece = |start: u64, limit: u64| Segment {
            block: block.clone(),
            start,
            limit,
            entry: SegmentEntry::Interior,
        };
        let segments =
            vec![piece(0, 10), piece(10, 20), piece(20, 30), piece(30, 40), piece(40, 50)];

        let groups = distribute(segments.clone(), 5);
        assert_eq!(groups.len(), 5);
        assert!(groups.iter().all(|g| g.len() == 1));

        // Contiguity and order: flattening the groups gives the input list.
        for streams in [1usize, 2, 3, 4, 5, 9] {
            let groups = distribute(segments.clone(), streams);
            assert!(groups.len() <= streams.max(1));
            assert!(groups.iter().all(|g| !g.is_empty()));
            let flat: Vec<u64> = groups.iter().flatten().map(|s| s.start).collect();
            assert_eq!(flat, vec![0, 10, 20, 30, 40], "streams {streams}");
        }

        // One piece carrying most of the bytes gets a group of its own.
        let lopsided = vec![piece(0, 1), piece(1, 2), piece(2, 1002), piece(1002, 1003)];
        let groups = distribute(lopsided, 2);
        assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2]);
    }

    /// A resumed stream keeps a segment while it owns a row at or past the
    /// pause — the row starting exactly at its limit included — and enters
    /// one begun before the pause on the LF ending the paused row.
    #[test]
    fn a_segment_resumes_from_the_row_its_pause_left() {
        let block = CopyBlock {
            header: crate::copy::parse_copy_header(b"COPY public.t (id) FROM stdin;").unwrap(),
            database: None,
            header_offset: 10,
            data_offset: 40,
            terminator_offset: 400,
            end_offset: 403,
            row_count: 0,
            partition_root: None,
            statistics: None,
            array_shapes: Vec::new(),
        };
        let whole = Segment::over(&block, 39..403);
        assert_eq!((whole.start, whole.entry), (10, SegmentEntry::Header));
        let run = Segment::over(&block, 99..199);
        assert_eq!((run.start, run.entry), (99, SegmentEntry::Interior));
        let at = |segment: &Segment, offset| {
            segment.clone().resumed_at(offset).map(|s| (s.start, s.limit, s.entry))
        };

        assert_eq!(at(&run, 0), Some((99, 199, SegmentEntry::Interior)));
        assert_eq!(at(&run, 60), Some((99, 199, SegmentEntry::Interior)));
        assert_eq!(at(&run, 100), Some((99, 199, SegmentEntry::Interior)));
        assert_eq!(at(&run, 150), Some((149, 199, SegmentEntry::Interior)));
        assert_eq!(at(&run, 199), Some((198, 199, SegmentEntry::Interior)));
        assert_eq!(at(&run, 200), None);
        assert_eq!(at(&whole, 10), Some((10, 403, SegmentEntry::Header)));
        assert_eq!(at(&whole, 120), Some((119, 403, SegmentEntry::Interior)));
    }

    /// No blocks at all is one sub-stream that yields nothing, so a caller
    /// never has to distinguish "no partitions" from "no rows".
    #[test]
    fn a_table_with_no_blocks_is_still_one_sub_stream() {
        let groups = distribute(Vec::new(), 8);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].is_empty());
    }

    /// A sub-stream's resume token is stamped with the partition it came out
    /// of, so "resuming a partition is unsupported" is an error rather than a
    /// silent superset of the rows that partition had left.
    #[test]
    fn a_partitions_fingerprint_differs_from_the_whole_streams() {
        let options = QueryOptions::default();
        let whole = query_fingerprint("public.t", &options, None);
        let first = query_fingerprint("public.t", &options, Some((0, 4)));
        let second = query_fingerprint("public.t", &options, Some((1, 4)));
        let of_two = query_fingerprint("public.t", &options, Some((0, 2)));
        assert_ne!(whole, first);
        assert_ne!(first, second);
        assert_ne!(first, of_two);
    }

    /// One table in three blocks: two naming their columns differently and one
    /// naming none, so a term or a projected name on `b` resolves against the
    /// first, refuses the second, and cannot be tried on the third until a row
    /// is read.
    fn three_schemas(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("three_schemas.sql");
        std::fs::write(
            &path,
            "COPY public.t (a, b) FROM stdin;\n1\tx\n2\t\\N\n\\.\n\n\
             COPY public.t (a) FROM stdin;\n3\n\\.\n\n\
             COPY public.t FROM stdin;\n4\ty\n\\.\n",
        )
        .unwrap();
        path
    }

    /// **The plan resolves every block with a column list before any is read,
    /// and activation takes that resolution rather than making its own.** A
    /// block's filter or projection refusal refuses the plan; a block with no
    /// column list is left out, to be resolved where it is reached.
    #[tokio::test]
    async fn the_filter_is_resolved_once_per_block_at_plan_time() {
        use crate::io::LocalFileSource;
        use crate::predicate::Predicate;

        let dir = tempfile::tempdir().unwrap();
        let source = LocalFileSource::open(three_schemas(dir.path())).unwrap();
        let not_null = |column: &str| {
            Expr::all([Predicate {
                column: column.into(),
                op: PredicateOp::IsNotNull,
                value: None,
            }])
        };
        let query_options = QueryOptions {
            filter: not_null("a"),
            projection: Some(vec!["a".into()]),
            // Past the first block, which would otherwise settle the table.
            scan_extent: ScanExtent::Full,
            ..QueryOptions::default()
        };
        let mapped = map_for_query(
            &source,
            "public.t",
            &ScanOptions::default(),
            &query_options,
            &CacheMode::Disabled,
        )
        .await
        .unwrap();
        let [first, second, third] = &mapped.matches[..] else {
            panic!("three blocks: {:?}", mapped.matches)
        };
        let plan_for = |query_options: &QueryOptions| {
            ReplayPlan::new(
                ScanOptions::default(),
                query_options.clone(),
                &mapped.matches,
                mapped.metadata.clone(),
                mapped.census.clone(),
            )
        };

        // A filter the second block refuses refuses the plan, naming it.
        let refusing = QueryOptions { filter: not_null("b"), ..query_options.clone() };
        assert!(matches!(
            plan_for(&refusing).map(|_| ()),
            Err(Error::UnknownPredicateColumn { header_offset, .. })
                if header_offset == second.header_offset
        ));

        // So does a projection the second block refuses.
        let refusing = QueryOptions { projection: Some(vec!["b".into()]), ..query_options.clone() };
        assert!(matches!(
            plan_for(&refusing).map(|_| ()),
            Err(Error::UnknownProjectionColumn { header_offset, .. })
                if header_offset == second.header_offset
        ));

        let plan = plan_for(&query_options).unwrap();
        assert_eq!(
            plan.blocks.keys().copied().collect::<Vec<_>>(),
            [first.header_offset, second.header_offset]
        );
        assert!(third.header.columns.is_empty());

        // The planned entry is what resolving the block where it is reached
        // would have produced.
        let live = |block: &CopyBlock, field_count| {
            resolve_for_query(
                &block.header,
                block.header_offset,
                field_count,
                block.database.as_deref(),
                &query_options,
                mapped.metadata.as_ref(),
                &mapped.census,
            )
        };
        let planned = &plan.blocks[&first.header_offset];
        assert_eq!(format!("{planned:?}"), format!("{:?}", live(first, 2).unwrap()));

        // Activating the planned block hands out the plan's own tree.
        let open = |block: &CopyBlock, field_count| {
            activate(
                block.header.clone(),
                block.header_offset,
                field_count,
                block.database.clone(),
                &plan,
            )
        };
        let ((_, _, _, filter, _), resolved, notes) = open(first, 2).unwrap();
        assert!(Arc::ptr_eq(&filter, &planned.filter));
        assert_eq!((resolved, notes), (planned.resolved.clone(), planned.notes.clone()));

        // An activation the entry does not describe resolves for itself.
        let ((_, _, _, other, _), _, _) = open(first, 3).unwrap_or_else(|e| panic!("{e}"));
        assert!(!Arc::ptr_eq(&other, &planned.filter));
    }
}
