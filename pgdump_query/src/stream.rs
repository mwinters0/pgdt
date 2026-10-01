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
//! exit past the first read but an error. [`CacheMode::Disabled`] runs the same way with `save` a no-op.
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
//! ([`TableStream::resolved_schema`]), and the batches this stream yields are
//! built to it: typed under `SchemaMode::Typed`, all-`Utf8View` under
//! `SchemaMode::Strings`. A table's blocks may list its columns in different
//! orders, and every batch comes out in the table's one order
//! ([`TableColumns`]).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arrow::array::RecordBatch;
use arrow::datatypes::Schema;
use async_stream::try_stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::sync::OnceCell;

use crate::batch::{QueryOptions, RetainedChunks, RowBatcher, ScanExtent};
#[cfg(test)]
use crate::cache::StrictIdentity;
use crate::cache::{CacheLoad, CacheMode, SourceWatch};
use crate::copy::{CopyHeader, RawRow, RowSplit, validated_prefix};
use crate::diagnostic::{Diagnostic, DiagnosticKind, Finding, Severity};
use crate::gather;
use crate::index::{
    ArrayShape, BlockCensus, CopyBlock, DumpIndex, TableName, Unrepresentable, UnrepresentableTier,
    non_seekable_compression_diagnostic, scan_preamble, tiling_diagnostics,
    toc_coverage_diagnostic, union_census,
};
#[cfg(feature = "introspect")]
use crate::instrument::statistics_loaded;
use crate::instrument::{EvaluationPart, StatisticsScope, row_evaluated, timed};
#[cfg(feature = "introspect")]
use crate::instrument::{evaluated_row, timed_span};
use crate::io::{
    ByteRangeSource, DEFAULT_MEMORY_BUDGET, Parallelism, PartitionBoundaries, Partitioning,
    RetainedUnit, WaitPolicy, WorkerMemory, memory_budget_display,
};
use crate::leader::{self, CensusPlan, RegionScan};
use crate::map::{Builder, DataBlock, Span, SpanBody, attach_text, census_row};
use crate::pgtype::ComparisonSemantics;
use crate::preamble::{DumpMetadata, dump_metadata_from_spans};
use crate::predicate::{
    ComparisonNote, Expr, PredicateOp, ResolvedExpr, resolve_membership, resolve_term,
};
use crate::prune::{DynamicPruning, SortedStop, prune_block};
use crate::resolve::{
    ResolvedSchema, SchemaMode, database_for_name, read_as_text, resolve_columns,
};
use crate::scan::{
    ChunkCarry, CopyEnd, CopyScanner, Event, Row, ScanOptions, announce_cancellation,
    scan as scan_events,
};
use crate::statistics::{
    BlockGathered, BlockObserver, BlockStatistics, Sortedness, StatisticsAccount,
    StatisticsBackfill, StatisticsHeld, StatisticsRequest, Term,
};
use crate::summary::partition_orders;
use crate::unrepresentable::{
    UnrepresentableMode, UnrepresentableRead, counter_for, unrepresentable_reads,
    unrepresentable_tests,
};
use crate::{Error, Result};

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, `QueryOptions::filter` resolved against this block's
/// own schema (a table's blocks can order its columns differently), and the database this block
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
/// operator on a column the register gives no order is
/// `Error::UnorderedPredicateColumn`, any comparing operator on a column whose
/// server comparison is not a text one is `Error::UncomparablePredicateColumn`,
/// and a literal that is not a value of
/// the column's type — `=` included — is `Error::PredicateValueDecode`. All
/// are raised for the first offending term in a left-to-right walk. The plan
/// runs it for every block before any is read, and raises the first refusing
/// block's refusal in file order before any row of the table
/// ([`plan_blocks`]). It takes the whole [`ResolvedSchema`] because the ordering
/// refusal reads `columns` and `plans` too.
fn resolve_expr(
    filter: &Expr,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
) -> Result<ResolvedExpr> {
    let branch = |children: &[Expr]| {
        children
            .iter()
            .map(|child| resolve_expr(child, resolved, header_offset, semantics))
            .collect::<Result<Vec<_>>>()
    };
    let column = |name: &str| {
        column_position(resolved, name).ok_or_else(|| Error::UnknownPredicateColumn {
            header_offset,
            column: name.to_string(),
        })
    };
    Ok(match filter {
        Expr::Term(predicate) => ResolvedExpr::Term(resolve_term(
            predicate,
            column(&predicate.column)?,
            resolved,
            header_offset,
            semantics,
        )?),
        Expr::In(membership) => ResolvedExpr::In(resolve_membership(
            membership,
            column(&membership.column)?,
            resolved,
            header_offset,
            semantics,
        )?),
        Expr::And(children) => ResolvedExpr::And(branch(children)?),
        Expr::Or(children) => ResolvedExpr::Or(branch(children)?),
        Expr::Not(inner) => {
            ResolvedExpr::Not(Box::new(resolve_expr(inner, resolved, header_offset, semantics)?))
        }
    })
}

/// Where the column `name` sits in the block's unprojected schema.
fn column_position(resolved: &ResolvedSchema, name: &str) -> Option<usize> {
    resolved.schema.fields().iter().position(|f| f.name() == name)
}

/// A [`DynamicFilter`]'s state resolved as [`resolve_expr`] resolves a static
/// filter, but **never refused**: a term this block cannot resolve stands as
/// whatever keeps every row where it sits — the empty conjunction beneath an
/// even number of `Not`s, the empty disjunction beneath an odd one — since a
/// dynamic filter only ever licenses dropping rows, and a term it cannot
/// state here licenses nothing.
fn resolve_loosened(
    filter: &Expr,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
    negated: bool,
) -> ResolvedExpr {
    let branch = |children: &[Expr]| {
        children
            .iter()
            .map(|child| resolve_loosened(child, resolved, header_offset, semantics, negated))
            .collect()
    };
    let leaf = match filter {
        // No producer states one, and a block reading a state holds no test
        // for it to read (`resolve_for_query`'s `testing`), so it stands as
        // whatever keeps every row.
        Expr::Term(predicate) if predicate.op.tests_unrepresentable() => None,
        Expr::Term(predicate) => column_position(resolved, &predicate.column).and_then(|index| {
            resolve_term(predicate, index, resolved, header_offset, semantics)
                .ok()
                .map(ResolvedExpr::Term)
        }),
        Expr::In(membership) => column_position(resolved, &membership.column).and_then(|index| {
            resolve_membership(membership, index, resolved, header_offset, semantics)
                .ok()
                .map(ResolvedExpr::In)
        }),
        _ => None,
    };
    match filter {
        Expr::Term(_) | Expr::In(_) => match (leaf, negated) {
            (Some(leaf), _) => leaf,
            (None, false) => ResolvedExpr::And(Vec::new()),
            (None, true) => ResolvedExpr::Or(Vec::new()),
        },
        Expr::And(children) => ResolvedExpr::And(branch(children)),
        Expr::Or(children) => ResolvedExpr::Or(branch(children)),
        Expr::Not(inner) => ResolvedExpr::Not(Box::new(resolve_loosened(
            inner,
            resolved,
            header_offset,
            semantics,
            !negated,
        ))),
    }
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
/// projection, the filter terms and their semantics, the schema mode, how a
/// value its column cannot hold is read and — for a sub-stream of a
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
    match options.semantics {
        ComparisonSemantics::Postgres => 0u8,
        ComparisonSemantics::DataFusion => 1u8,
    }
    .hash(&mut hasher);
    options.unrepresentable.hash(&mut hasher);
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
                PredicateOp::IsUnrepresentable => 10u8,
                PredicateOp::IsNotUnrepresentable => 11u8,
            }
            .hash(hasher);
            term.value.hash(hasher);
        }
        Expr::In(membership) => {
            4u8.hash(hasher);
            membership.column.hash(hasher);
            membership.values.hash(hasher);
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
/// throttle's gate never closes, which is every `--dtcache none` scan
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
    /// spliced watermark and has been persisted; everything past it is
    /// unscanned.
    Interrupted,
}

/// Whether this error is the interrupt arriving by another door.
///
/// A source whose wait is a request rather than a `pread` answers a
/// cancellation by dropping the read in flight, so what a polled check point
/// would have seen as a set flag reaches its caller as a failed read instead
/// (`docs/design/decisions.md`, "D26"). The flag is what tells the two apart:
/// a `ScanCancelled` nobody asked for is a source misreporting itself and
/// stays an error.
fn cancelled_read(error: &Error, scan_options: &ScanOptions) -> bool {
    matches!(error, Error::ScanCancelled { .. }) && scan_options.cancelled()
}

/// What a mapping pass records of each block it maps
/// (`docs/design/decisions.md`, "D77").
#[derive(Clone, Copy)]
enum Pass<'a> {
    /// [`map_file`]'s: each block at the level `.0` gives its table, censused
    /// where that is the data level, and its data-level columns gathered.
    Parse(&'a StatisticsRequest),
    /// A query's: every block censused, nothing gathered, whatever level a
    /// parse would have given it (`docs/design/decisions.md`, "D35").
    Query,
}

/// Scan `source` end to end and build its full file map, every block
/// censused and counted and none gathering statistics — the eager, serial
/// pass the incremental one ([`map_file`]) is held to. One pass:
/// [`crate::map::Builder`] is fed the whole [`Event`] stream, and
/// `DumpIndex::metadata` is [`dump_metadata_from_spans`] over the result and
/// [`DumpIndex::blocks`] a filter over it (`docs/design/decisions.md`,
/// "D34").
///
/// **Here rather than beside the index**, which is L1: counting a block's
/// values resolves its columns' declared types, so the pass states each
/// database's DDL at its first `COPY` block as [`map_forward`] does
/// (`docs/design/decisions.md`, "D96").
pub async fn build_index(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<DumpIndex> {
    let builder = eager_pass(source, options).await?;
    let size = source.size().await?;
    let roles = builder.roles().clone();
    let tablespaces = builder.tablespaces().clone();
    let mut spans = builder.finish(size);
    let metadata = Some(dump_metadata_from_spans(&spans));
    attach_text(source, &mut spans).await?;
    let mut diagnostics = tiling_diagnostics(&spans, size);
    diagnostics.push(toc_coverage_diagnostic(&spans));
    diagnostics.extend(non_seekable_compression_diagnostic(source.seek_table().as_ref()));
    Ok(DumpIndex { spans, scanned_through: size, metadata, roles, tablespaces, diagnostics })
}

/// Scan `source` end to end and build its full file map as a span list —
/// [`build_index`]'s spans, alone. Always scans to EOF, so
/// [`SpanBody::Unscanned`] never appears in the result.
pub async fn build_map(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<Vec<Span>> {
    let builder = eager_pass(source, options).await?;
    let mut spans = builder.finish(source.size().await?);
    attach_text(source, &mut spans).await?;
    Ok(spans)
}

/// [`build_index`] and [`build_map`]'s one scan: every event to one builder,
/// each block counted against the DDL above it, restated once per database.
async fn eager_pass(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<Builder> {
    let mut builder = Builder::new();
    // The database the metadata in hand was stated for, and that metadata.
    let mut stated: Option<(Option<String>, DumpMetadata)> = None;
    scan_events(source, options, |event| {
        match event {
            Event::CopyStart(start) => {
                let boundary = builder.pending_comment_start().unwrap_or(start.header_offset);
                let header = start.header.clone();
                builder.on_copy_start(start, true);
                let db = builder.database().map(str::to_owned);
                if stated.as_ref().is_none_or(|(covers, _)| *covers != db) {
                    let metadata = dump_metadata_from_spans(&builder.snapshot(boundary));
                    stated = Some((db.clone(), metadata));
                }
                let metadata = stated.as_ref().map(|(_, metadata)| metadata);
                builder.count_block(counter_for(&header, metadata, db.as_deref()));
            }
            Event::Row(row) => builder.on_row(row.offset, row.raw),
            Event::CopyEnd(end) => builder.on_copy_end(end),
            Event::Line(line) => builder.feed_line(line.offset, line.raw),
            Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
            Event::LargeObjectStart(start) => builder.on_large_object_start(start.start_offset),
            Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
        }
        std::ops::ControlFlow::Continue(())
    })
    .await?;
    Ok(builder)
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
/// The [`crate::map::Builder`] is seeded with the database in scope at the
/// frontier, which it cannot infer; [`splice`] places its spans after the
/// frontier (`docs/design/decisions.md`, "D48").
///
/// **Not every completed block is persisted; every exit past the first read
/// but an error is** — see [`SaveThrottle`]; the two that return before one,
/// the map unchanged, save nothing. **`index.metadata` is restated at each `\connect`ed
/// database's first `COPY` block**, which per I1 is one of the two boundaries
/// [`dump_metadata_from_spans`] may be called at, and the only one this loop
/// stands on.
///
/// **`mapping` says what to record of each block** ([`Pass`]): [`map_file`]'s
/// records what its request's levels ask, a query's censuses every block and
/// gathers nothing. A block gathering statistics is observed row by row on
/// this loop, or piece by piece where the leader takes it, every observer
/// charging `account`.
#[allow(clippy::too_many_arguments)]
async fn map_forward(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    watch: &SourceWatch,
    index: &mut DumpIndex,
    target: Option<(&str, Option<&str>)>,
    mapping: Pass<'_>,
    account: &Arc<StatisticsAccount>,
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
        chunk_size = scan_options.chunk_size_bytes,
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
    source.hint_read_size(scan_options.chunk_size_bytes);
    source.hint_parallelism(scan_options.parallelism);
    announce_cancellation(source, scan_options);
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
            cache.save(watch, source, index).await?;
            return Ok(MapStop::Interrupted);
        }
        let want = scan_options.chunk_size_bytes.min((size - read_pos) as usize);
        let chunk = if want > 0 {
            let bytes = match source.read_range(read_pos, want).await {
                Ok(bytes) => bytes,
                // The check point above, reached through the read rather than
                // through the flag ([`cancelled_read`]): the same chunk and
                // the same watermark, so this banks what that would have
                // banked.
                Err(e) if cancelled_read(&e, scan_options) => {
                    cache.save(watch, source, index).await?;
                    return Ok(MapStop::Interrupted);
                }
                Err(e) => return Err(e),
            };
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
                        // The block's level: censused, and gathering where
                        // a statistics request tracks its columns.
                        let (census, request) = match mapping {
                            Pass::Query => (true, None),
                            Pass::Parse(request) => {
                                (request.tracked_columns(&start.header).is_some(), Some(request))
                            }
                        };
                        let header = census.then(|| start.header.clone());
                        builder.on_copy_start(start, census);
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
                        // After the restatement, so the count and the
                        // observer resolve the block against its own
                        // database's DDL.
                        let plan = header.as_ref().map(|header| {
                            let counter =
                                counter_for(header, index.metadata.as_ref(), db.as_deref());
                            builder.count_block(Arc::clone(&counter));
                            CensusPlan { width: header.columns.len(), counter }
                        });
                        let observer =
                            header.as_ref().zip(request).and_then(|(header, request)| {
                                gather::observer_for(
                                    request,
                                    header,
                                    index.metadata.as_ref(),
                                    db.as_deref(),
                                    account,
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
                            plan,
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
                                    watch,
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
                                cache.save(watch, source, index).await?;
                                return Ok(MapStop::Interrupted);
                            }
                        }
                    }
                    // This pass needs only the block's extent; row bytes
                    // become batches in the replay phase. Rows are read here
                    // for a data-level block's census and count
                    // (`docs/design/decisions.md`, "D35", "D96") and a
                    // gathering block's statistics.
                    Event::Row(row) => builder.on_row(row.offset, row.raw),
                    Event::CopyEnd(end) => {
                        let targets = std::mem::take(&mut open_block_targets);
                        let end_offset = end.end_offset;
                        match close_copy_block(
                            source,
                            scan_options,
                            cache,
                            watch,
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
            // The four hints this loop announced still stand: `scan_region`
            // restores `WaitPolicy::NeverWait` on its way out, announces the
            // same `Parallelism`, and never touches the read size or the
            // cancellation token.
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

    // **The text is attached before the index advances**, so a read dropped
    // here ([`cancelled_read`]) leaves the map at the watermark it already
    // held rather than at one whose spans lost their text with nothing that
    // would ever re-attach it — a resumed run past `scanned_through` re-reads
    // nothing.
    let roles: Vec<String> = builder.roles().iter().cloned().collect();
    let tablespaces: Vec<String> = builder.tablespaces().iter().cloned().collect();
    let mut spans = splice(&prefix, builder.finish(size), seg_start, size, size);
    match attach_text(source, &mut spans).await {
        Ok(()) => {}
        Err(e) if cancelled_read(&e, scan_options) => {
            cache.save(watch, source, index).await?;
            return Ok(MapStop::Interrupted);
        }
        Err(e) => return Err(e),
    }
    index.roles.extend(roles);
    index.tablespaces.extend(tablespaces);
    index.spans = spans;
    index.scanned_through = size;
    index.diagnostics = tiling_diagnostics(&index.spans, size);
    index.diagnostics.push(toc_coverage_diagnostic(&index.spans));
    cache.save(watch, source, index).await?;
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
/// **Silence means the leader dispatched the announced count** — and never
/// that every worker read at once. A decline is *not* silent: the shortfall is
/// computed before it and returned with it ([`crate::leader::Shortfall`]), so
/// a region left serial by the budget or by the source still prints. Only
/// `scan_region`'s own floor is unreported, deliberately. `flag` keeps one scan-wide fact from printing
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
/// early-stop check. **One body, two call sites** — the serial scanner's
/// `CopyEnd` and a region [`crate::leader::scan_region`] closed
/// (`docs/design/decisions.md`, "D52") — and
/// the ordering it holds is splice before the settled test, save before the
/// return. `targets` is whether this block's header could be the one that
/// settles `target`, read at `CopyStart`.
#[allow(clippy::too_many_arguments)]
async fn close_copy_block(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    watch: &SourceWatch,
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
    // "D62"). Nothing between gate openings reads `index.spans`: the metadata
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
        throttle.save(cache, watch, source, index).await?;
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
/// save unconditionally once the pass has read anything.
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
        watch: &SourceWatch,
        source: &dyn ByteRangeSource,
        index: &DumpIndex,
    ) -> Result<()> {
        let started = Instant::now();
        cache.save(watch, source, index).await?;
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
    /// false or the interrupt came during the back-fill, and otherwise
    /// everything up to the last *spliced* watermark, which is also the last
    /// save ([`SaveThrottle`]).
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
    /// How many of those were re-read — including a re-read that declined,
    /// which leaves the block lacking still.
    pub backfilled: usize,
    /// **How many blocks of the finished map declined to gather statistics**,
    /// under this run's allowance or an earlier, larger one
    /// ([`crate::index::CopyBlock::statistics_declined`]) — zero for a run the
    /// interrupt reached before the back-fill finished, which is where the
    /// count is taken. A run stating no allowance declines nothing itself, so
    /// it counts only a block an earlier run declined whose held statistics
    /// already cover this request (`docs/design/decisions.md`, "D85").
    pub declined_statistics: usize,
    /// What the run's statistics held when it returned, by term, and the most
    /// they held (`docs/design/decisions.md`, "D81").
    pub statistics: StatisticsHeld,
}

/// Map `source` end to end, **continuing from whatever `cache` already
/// holds** — `pgdt parse`'s scan (`docs/design/decisions.md`, "D61"). This is
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
/// **It records what `statistics`' levels ask for**, over the blocks this run
/// maps — a data-level table's census and its data-level columns' statistics,
/// and of a metadata-level table nothing drawn from its rows — and then
/// **re-reads every block that lacks it** — one the cache already held,
/// a metadata-level one among them, or one this run gathered too coarsely for
/// a stated maximum ([`StatisticsRequest::backfill`]) — never replacing what a
/// block holds with less, one at a time in file order through the
/// same per-block re-read [`gather_block_statistics`] exposes, saving as it
/// goes and charging the run's own account. The back-fill runs once the
/// map has reached EOF, so each block resolves against whole-file metadata,
/// and a run interrupted inside it resumes into it, the blocks still lacking
/// being counted afresh. [`StatisticsRequest::default`] is the data level
/// everywhere.
///
/// **The three finishing steps are this function's.**
///
/// - `metadata` is recomputed over the whole span list. EOF is the other
///   boundary [`dump_metadata_from_spans`] may be called at (I1), covering a
///   trailing database with no `COPY` block of its own, and a file with no
///   blocks at all.
/// - `diagnostics` are recomputed rather than inherited, being
///   `#[serde(skip)]` — [`map_forward`] recomputes them at its own EOF exit
///   too, and what is this function's is keeping whatever
///   [`CacheMode::load`] reported about the cache *file* ahead of them.
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
    // Taken before anything is read, so every later observation compares
    // against what this run started from (`crate::cache::SourceWatch`).
    let watch = SourceWatch::open(source, cache.strict_identity()).await?;
    let run = map_file_watched(source, scan_options, cache, statistics, &watch).await;
    watch.attribute(source, run).await
}

/// [`map_file`] once its watch is open, which then attributes any failure.
async fn map_file_watched(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    statistics: &StatisticsRequest,
    watch: &SourceWatch,
) -> Result<MapRun> {
    let size = source.size().await?;
    let mut index = match cache.load(source).await? {
        CacheLoad::Index(index) => index,
        // Nothing to build forward from, and nothing at the path to keep.
        CacheLoad::Disabled | CacheLoad::Missing => DumpIndex::default(),
        // Refused before a byte of the dump is read, unless the caller said
        // this cache may be replaced (`docs/design/decisions.md`, "D20").
        CacheLoad::Unusable(unusable) => match cache.refusal(&unusable) {
            Some(refusal) => return Err(refusal),
            None => DumpIndex::default(),
        },
    };
    let account = Arc::new(StatisticsAccount::bounded_by(scan_options.statistics_allowance_bytes));
    let loaded = index.statistics_heap_bytes();
    #[cfg(feature = "introspect")]
    statistics_loaded(loaded);
    account.apply(&[(Term::Loaded, loaded as i64)]);
    // Which statistics the *cache* supplied, by the one identity a block keeps
    // across the splice: what a back-fill replaces leaves this term, and what
    // this run gathered leaves `Term::Retained` instead.
    let loaded: HashSet<u64> = index
        .blocks()
        .filter(|block| block.statistics.is_some())
        .map(|b| b.header_offset)
        .collect();
    // The diagnostics about the cache *file* rather than about the map — what
    // the load said about its identity; everything else it computed is
    // recomputed below.
    let carried: Vec<Diagnostic> = index
        .diagnostics
        .drain(..)
        .filter(|d| {
            matches!(
                d.kind,
                DiagnosticKind::CacheMtimeChanged
                    | DiagnosticKind::CacheEntityTagChanged
                    | DiagnosticKind::CacheOriginChanged
            )
        })
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
        // ignores the cancel flag (`docs/design/decisions.md`, "D26") — but a
        // source whose read is dropped in flight answers it anyway, and none
        // of a half-read preamble may be banked, which is that entry's reason.
        // Everything the prepass reads is therefore held aside until it is
        // whole.
        let prepass = async {
            let (metadata, spans, preamble_end, roles, tablespaces) =
                scan_preamble(source, scan_options).await?;
            let mut spans = splice(&[], spans, 0, preamble_end, size);
            attach_text(source, &mut spans).await?;
            Ok::<_, Error>((metadata, spans, preamble_end, roles, tablespaces))
        }
        .await;
        match prepass {
            Ok((metadata, spans, preamble_end, roles, tablespaces)) => {
                index.metadata = Some(metadata);
                index.spans = spans;
                index.roles.extend(roles);
                index.tablespaces.extend(tablespaces);
                index.scanned_through = preamble_end;
                cache.save(watch, source, &index).await?;
            }
            // **Nothing to bank, so nothing is written.** The prepass holds
            // every span aside until it is whole, and this arm is reached
            // only from a cold start, so a save here would write a map
            // describing zero bytes — or strip an existing empty one of the
            // identity diagnostics `carried` has already drained — and either
            // can then refuse the resume it invited through
            // `Error::CacheUnusable`. The source is asked directly,
            // there being no save left to ride ([`crate::cache::SourceWatch`]).
            Err(e) if cancelled_read(&e, scan_options) => {
                watch.check(source).await?;
                return Ok(interrupted_run(index, resumed_from, &account));
            }
            Err(e) => return Err(e),
        }
    }

    let stopped = match map_forward(
        source,
        scan_options,
        cache,
        watch,
        &mut index,
        None,
        Pass::Parse(statistics),
        &account,
        size,
    )
    .await
    {
        Ok(stop) => stop,
        // The interrupt arriving by another door ([`cancelled_read`]). The
        // pass's own check points save before they return and an unwinding
        // read has not, so the bank happens here, at the watermark the map
        // was already consistent at. **The check is this arm's own**: an
        // enabled save makes the same one before it writes, and a disabled
        // cache makes none at all, so leaving it to the save is how a
        // `--dtcache none` run reached the interrupt below unverified. Only a
        // read the leader dispatched unwinds this far; every other one the
        // pass makes is caught at a check point of its own.
        Err(e) if cancelled_read(&e, scan_options) => {
            watch.check(source).await?;
            cache.save(watch, source, &index).await?;
            return Ok(interrupted_run(index, resumed_from, &account));
        }
        Err(e) => return Err(e),
    };
    if stopped == MapStop::Interrupted {
        // The run's own last word, for a run that saved nothing to ride
        // (`crate::cache::SourceWatch`).
        watch.check(source).await?;
        return Ok(interrupted_run(index, resumed_from, &account));
    }

    index.metadata = Some(dump_metadata_from_spans(&index.spans));
    let mut diagnostics = carried;
    diagnostics.extend(tiling_diagnostics(&index.spans, size));
    diagnostics.push(toc_coverage_diagnostic(&index.spans));
    index.diagnostics = diagnostics;
    let backfill = backfill_statistics(
        source,
        scan_options,
        cache,
        watch,
        &mut index,
        statistics,
        &account,
        size,
        loaded,
    )
    .await?;
    let mut declined_statistics = 0;
    if backfill.interrupted {
        watch.check(source).await?;
    } else {
        // Before the save, so a complete map is never written from bytes that
        // fail their own check ([`SourceWatch::finish`]); an interrupted run
        // leaves the block it stopped inside for the resume to re-read.
        watch.finish(source).await?;
        cache.save(watch, source, &index).await?;
        report_density_shortfall(statistics, &index);
        declined_statistics =
            report_statistics_declines(statistics, scan_options.statistics_allowance_bytes, &index);
    }
    Ok(MapRun {
        index,
        resumed_from,
        interrupted: backfill.interrupted,
        lacking_statistics: backfill.lacking,
        backfilled: backfill.reread,
        declined_statistics,
        statistics: announce_statistics_held(&account),
    })
}

/// What [`map_file`] returns for a run that stopped before EOF: the map as far
/// as it is consistent, and **none of the three counts a back-fill states** —
/// an interrupt reached during the mapping pass never ran one, so the caller
/// is told the map is short rather than that nothing was re-read.
///
/// The caller banks first: whether that is the pass's own save at a check
/// point or one made where an unwinding read was caught is this function's
/// business either way.
fn interrupted_run(
    index: DumpIndex,
    resumed_from: u64,
    account: &Arc<StatisticsAccount>,
) -> MapRun {
    MapRun {
        index,
        resumed_from,
        interrupted: true,
        lacking_statistics: 0,
        backfilled: 0,
        declined_statistics: 0,
        statistics: announce_statistics_held(account),
    }
}

/// Read `account` whole as a pass returns, and say what it held on the status
/// output — the total now, the peak and each term's own peak — unless it
/// never held anything: a pass that gathered, loaded or back-filled no
/// statistic prints nothing (`docs/manual/dump-inspection.md`,
/// "`--statistics-level`: what `parse` records for later queries").
fn announce_statistics_held(account: &StatisticsAccount) -> StatisticsHeld {
    let held = account.held();
    if held.peak > 0 {
        let peaks = held.term_peaks;
        tracing::info!(
            bytes = held.now.total(),
            peak_bytes = held.peak,
            retained_peak_bytes = peaks.retained,
            loaded_peak_bytes = peaks.loaded,
            gathering_peak_bytes = peaks.gathering,
            pieces_peak_bytes = peaks.pieces,
            interned_peak_bytes = peaks.interned,
            "statistics held",
        );
    }
    held
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
///
/// Deficiency register: `deficiency: KD37` — that is true of the flag and of a
/// dropped read this pass makes itself ([`observe_rows`]), and not of one the
/// leader dispatched: [`reread_block`]'s `leader::scan_region` propagates, so a
/// source that answers a cancellation by failing its read ends a `parse` as
/// `Error::ScanCancelled`, banking nothing since the throttle's last save, where the same Ctrl-C during the
/// mapping pass is an interrupted run ([`cancelled_read`],
/// `docs/design/decisions.md`, "D26"). The fix is the arm [`map_file`] already
/// carries for the pass's own leader reads, here as well. **(c) unowned**;
/// promoted by a parallel remote `parse` seen to error on Ctrl-C after its map
/// reached EOF, which is the only arrangement that reaches it.
///
/// **A block lacking statistics was loaded, or broke a stated maximum**: a
/// block this pass mapped holds what its own request asked, except for a
/// maximum no single read can deliver. So what a re-read replaces leaves the
/// term that carried it — `loaded`, the header offsets the cache supplied
/// statistics for, telling the two apart.
#[allow(clippy::too_many_arguments)]
async fn backfill_statistics(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    watch: &SourceWatch,
    index: &mut DumpIndex,
    statistics: &StatisticsRequest,
    account: &Arc<StatisticsAccount>,
    size: u64,
    mut loaded: HashSet<u64>,
) -> Result<BackfillRun> {
    // Positions into `index.spans`, which nothing below adds to or reorders.
    let metadata = index.metadata.as_ref();
    let lacking: Vec<(usize, StatisticsBackfill)> = if statistics.gathers() {
        index
            .spans
            .iter()
            .enumerate()
            .filter_map(|(at, span)| match &span.body {
                SpanBody::Data(DataBlock::Copy(block)) => statistics
                    .backfill(
                        block,
                        &bounded_columns(block, metadata),
                        scan_options.statistics_allowance_bytes,
                    )
                    .map(|backfill| (at, backfill)),
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
    // There is no cache file to name in a `CachedBlockChanged` refusal
    // unless one is enabled. A block can lack what was asked under any mode —
    // a stated maximum no single read delivers leaves a freshly mapped block
    // lacking, `CacheMode::Disabled` included (see this function's doc).
    let cache_path = match cache {
        CacheMode::Enabled { path, .. } => Some(path.as_path()),
        CacheMode::Disabled { .. } | CacheMode::Offline(_) => None,
    };
    let mut throttle = SaveThrottle::new();
    let mut shortfall_reported = false;
    for (at, backfill) in lacking {
        // **At most two reads of a block**, the second being the finer size a
        // stated maximum a block broke at the size it gathered from asks for
        // ([`StatisticsRequest::backfill`]); nothing asks for a third, and the
        // bound here is what says so.
        let mut plan = Some(backfill);
        for _ in 0..2 {
            let Some(backfill) = plan.take() else { break };
            let SpanBody::Data(DataBlock::Copy(block)) = &index.spans[at].body else {
                unreachable!("the positions were read off copy blocks of this span list");
            };
            let gathered = reread_block(
                source,
                scan_options,
                cache_path,
                index.metadata.as_ref(),
                block,
                &backfill,
                account,
                size,
                &mut shortfall_reported,
            )
            .await?;
            let Some(BlockReread { gathered, census }) = gathered else {
                cache.save(watch, source, index).await?;
                run.interrupted = true;
                return Ok(run);
            };
            if let SpanBody::Data(DataBlock::Copy(block)) = &mut index.spans[at].body {
                // Every row was read, so the block is at the data level
                // whether its statistics fitted or not.
                block.set_census(census);
                let _attributed = StatisticsScope::enter();
                match gathered {
                    // **A re-read that declined keeps what the block already
                    // held**: the observer freed its own and holds nothing to
                    // replace them with, so the block records the allowance
                    // beside whatever an earlier pass gathered
                    // (`docs/design/decisions.md`, "D85").
                    BlockGathered::Declined { allowance } => {
                        block.statistics_declined = Some(allowance);
                    }
                    BlockGathered::Gathered(gathered) => {
                        let replaced =
                            block.statistics.as_deref().map_or(0, BlockStatistics::heap_bytes);
                        // The re-read's own observer has already credited
                        // `Term::Retained`, so what it replaced leaves the term
                        // that carried it: `Term::Loaded` for a block the cache
                        // supplied, and `Term::Retained` for one this run
                        // gathered — which a block re-read for a stated maximum
                        // is, its own first read included.
                        let term = if loaded.remove(&block.header_offset) {
                            Term::Loaded
                        } else {
                            Term::Retained
                        };
                        block.statistics = Some(Arc::new(gathered));
                        // A block that holds what was asked declines nothing,
                        // so a record from an earlier, tighter allowance goes.
                        block.statistics_declined = None;
                        account.apply(&[(term, -(replaced as i64))]);
                    }
                }
                plan = statistics.backfill(
                    block,
                    &bounded_columns(block, index.metadata.as_ref()),
                    scan_options.statistics_allowance_bytes,
                );
            }
        }
        run.reread += 1;
        // Both of `map_forward`'s check points, a block's close being the
        // second (`docs/design/decisions.md`, "D63").
        if scan_options.cancelled() && run.reread < run.lacking {
            cache.save(watch, source, index).await?;
            run.interrupted = true;
            return Ok(run);
        }
        if throttle.due() {
            throttle.save(cache, watch, source, index).await?;
        }
    }
    tracing::info!(blocks = run.reread, "statistics back-fill complete");
    Ok(run)
}

/// **Say which blocks still hold more rows than a stated maximum asks for**,
/// one line each: a block too dense for it is re-read at the size its own
/// groups predicted, and one whose rows cluster can miss it there. Nothing
/// reads such a block again, so every run under that maximum says what it
/// keeps rather than what it will fix.
fn report_density_shortfall(statistics: &StatisticsRequest, index: &DumpIndex) {
    let Some(max_rows) = statistics.max_rows() else { return };
    for block in index.blocks() {
        let Some(held) = block.statistics.as_deref() else { continue };
        if statistics.tracked_columns(&block.header).is_some() && held.breaks_max_rows(max_rows) {
            tracing::info!(
                table = block.header.table,
                group_size = held.group_size,
                max_rows,
                "row groups still hold more rows than the stated maximum",
            );
        }
    }
}

/// **Say which blocks hold no statistics because the allowance could not hold
/// them**, one line each, and answer how many — what
/// [`MapRun::declined_statistics`] carries.
///
/// **Every run under an allowance says it, not only the run that declined**: a
/// block's decline is recorded in the map ([`CopyBlock::statistics_declined`])
/// and re-read only under a larger allowance, so the run that skips one is the
/// run that has to say what it is leaving and what would fix it
/// (`docs/design/decisions.md`, "D85"). A pass that gathers nothing says
/// nothing: the blocks it did not ask about are none of its business.
fn report_statistics_declines(
    statistics: &StatisticsRequest,
    allowance: Option<u64>,
    index: &DumpIndex,
) -> usize {
    if !statistics.gathers() {
        return 0;
    }
    let mut declined = 0;
    for block in index.blocks() {
        let Some(under) = block.statistics_declined else { continue };
        if statistics.tracked_columns(&block.header).is_none() {
            continue;
        }
        declined += 1;
        tracing::info!(
            table = block.header.table,
            header_offset = block.header_offset,
            declined_under_bytes = under,
            allowance_bytes = %allowance.map_or_else(|| "(none)".to_string(), |a| a.to_string()),
            "statistics declined: this block's do not fit the allowance, and a larger --memory is \
             what re-reads it",
        );
    }
    declined
}

/// The hints every top-level read loop announces before its first read
/// (`crate::scan::scan`, [`map_forward`]): its chunk length, its budget, the
/// caller's cancellation where there is one, and that it grants no wait.
fn announce_read_loop(source: &dyn ByteRangeSource, scan_options: &ScanOptions) {
    source.hint_read_size(scan_options.chunk_size_bytes);
    source.hint_parallelism(scan_options.parallelism);
    announce_cancellation(source, scan_options);
    source.hint_wait_policy(WaitPolicy::NeverWait);
}

/// Which of `block`'s columns gathering keeps bounds and row order for,
/// positionally to its header, its columns resolved against `metadata` as a
/// mapping pass resolves them — what [`StatisticsRequest::backfill`] reads a
/// block's held statistics against, a column held without the bounds this
/// says it gets lacking them (`docs/design/decisions.md`, "D79").
pub fn bounded_columns(block: &CopyBlock, metadata: Option<&DumpMetadata>) -> Vec<bool> {
    gather::bounded_columns(&block.header, metadata, block.database.as_deref())
}

/// What re-reading one block the map already holds yields
/// ([`gather_block_statistics`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockReread {
    /// What the block gathered, or the allowance it declined under.
    pub gathered: BlockGathered,
    /// The block's census and count, which every re-read takes, having read
    /// every row — a declined one included.
    pub census: BlockCensus,
}

/// Re-read one block the map already holds and gather what `backfill` names —
/// the public entry point onto the same per-block re-read [`map_file`]'s
/// back-fill runs over every block lacking what its request asks for, this one
/// charging no account and naming no cache. `metadata` is the map's, whole-file where
/// it can be, which the block's columns are resolved against as a mapping pass
/// resolves them; `backfill` is [`StatisticsRequest::backfill`]'s answer for
/// `block`. The caller stores the result's `gathered` in
/// [`CopyBlock::statistics`], or — for [`BlockGathered::Declined`] — the
/// allowance in [`CopyBlock::statistics_declined`]
/// (`docs/design/decisions.md`, "D85"), and its `census` through
/// [`CopyBlock::set_census`], which puts a block mapped at the metadata
/// level at the data level.
///
/// **The block is scanned as a mapping pass scans it**: offered to the leader
/// under `scan_options`' parallelism, and read serially where it declines, so
/// what is gathered is what a straight-through pass would have gathered.
/// `Ok(None)` is [`ScanOptions::cancel`] stopping it before the block closed.
///
/// **A block that no longer ends where the map says is refused**,
/// [`Error::CachedBlockChanged`], rather than given statistics describing other
/// bytes than its map does. It names no cache, this entry point being handed
/// none; [`map_file`]'s back-fill names the one it loaded.
pub async fn gather_block_statistics(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    metadata: Option<&DumpMetadata>,
    block: &CopyBlock,
    backfill: &StatisticsBackfill,
) -> Result<Option<BlockReread>> {
    let size = source.size().await?;
    announce_read_loop(source, scan_options);
    let mut shortfall_reported = false;
    reread_block(
        source,
        scan_options,
        None,
        metadata,
        block,
        backfill,
        &Arc::default(),
        size,
        &mut shortfall_reported,
    )
    .await
}

/// [`gather_block_statistics`] once the source is announced, `size` known, and
/// with the flag [`report_shortfall`] keeps once per pass. `cache_path` is the
/// cache the map was loaded from, which a moved block's refusal names; the
/// observer charges `account`.
#[allow(clippy::too_many_arguments)]
async fn reread_block(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache_path: Option<&Path>,
    metadata: Option<&DumpMetadata>,
    block: &CopyBlock,
    backfill: &StatisticsBackfill,
    account: &Arc<StatisticsAccount>,
    size: u64,
    shortfall_reported: &mut bool,
) -> Result<Option<BlockReread>> {
    let mut observer = gather::observer_tracking(
        backfill,
        &block.header,
        metadata,
        block.database.as_deref(),
        account,
    );
    let read = reread_rows(
        source,
        scan_options,
        cache_path,
        metadata,
        block,
        size,
        Some(observer.as_mut()),
        shortfall_reported,
    )
    .await?;
    let Some(census) = read else { return Ok(None) };
    // The observer's own allocation is freed as `finish` returns, attributed
    // as it was allocated (`crate::instrument`).
    let _attributed = StatisticsScope::enter();
    let gathered = observer.finish(block.terminator_offset - block.data_offset);
    Ok(Some(BlockReread { gathered, census }))
}

/// Read every row of `block` again, as a mapping pass reads them — offered
/// to the leader, and serially where it declines — censusing and counting
/// them against the DDL `metadata` states for the block, handing each to
/// `observer` where there is one, and answer the census and count:
/// what a back-fill ([`reread_block`]) and a query's census of a block mapped
/// at the metadata level ([`census_metadata_level`]) both read. `None` is
/// [`ScanOptions::cancel`] stopping it first; a block no longer ending where
/// the map says is [`Error::CachedBlockChanged`], naming `cache_path`.
#[allow(clippy::too_many_arguments)]
async fn reread_rows(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache_path: Option<&Path>,
    metadata: Option<&DumpMetadata>,
    block: &CopyBlock,
    size: u64,
    mut observer: Option<&mut (dyn BlockObserver + 'static)>,
    shortfall_reported: &mut bool,
) -> Result<Option<BlockCensus>> {
    let moved = || Error::CachedBlockChanged {
        path: cache_path.map(Path::to_path_buf),
        header_offset: block.header_offset,
    };
    // A source cut short of the block's recorded end cannot end it there, and
    // every read loop below takes its length from `size` less its position.
    if block.end_offset > size {
        return Err(moved());
    }
    let plan = CensusPlan {
        width: block.header.columns.len(),
        counter: counter_for(&block.header, metadata, block.database.as_deref()),
    };
    let outcome = leader::scan_region(
        source,
        scan_options,
        block.header_offset,
        block.data_offset,
        Some(plan.clone()),
        size,
        observer.as_deref_mut(),
    )
    .await?;
    report_shortfall(shortfall_reported, outcome.shortfall);
    let (end, census) = match outcome.scan {
        RegionScan::Closed(interior) => (interior.end, interior.census),
        RegionScan::Cancelled => return Ok(None),
        RegionScan::Declined => {
            match observe_rows(source, scan_options, block, &plan, size, observer).await? {
                Some(read) => read,
                None => return Ok(None),
            }
        }
    };
    let recorded = (block.terminator_offset, block.end_offset, block.row_count);
    if (end.terminator_offset, end.end_offset, end.row_count) != recorded {
        return Err(moved());
    }
    Ok(Some(census))
}

/// Census and count every row of `block` as `plan` says, read serially from
/// its first data byte to its terminator, handing each to `observer` where
/// there is one, and answer the `CopyEnd` the scanner met with the census —
/// `None` where
/// [`ScanOptions::cancel`] was set first, read once per chunk as
/// [`map_forward`] reads it.
async fn observe_rows(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    block: &CopyBlock,
    plan: &CensusPlan,
    size: u64,
    mut observer: Option<&mut (dyn BlockObserver + 'static)>,
) -> Result<Option<(CopyEnd, BlockCensus)>> {
    let mut scanner = CopyScanner::resume(block.data_offset, Some((block.header_offset, 0)));
    let mut carry = ChunkCarry::new();
    let mut read_pos = block.data_offset;
    let mut census = BlockCensus::new(plan.width);
    loop {
        if scan_options.cancelled() {
            return Ok(None);
        }
        let want = scan_options.chunk_size_bytes.min((size - read_pos) as usize);
        let chunk = if want > 0 {
            let bytes = match source.read_range(read_pos, want).await {
                Ok(bytes) => bytes,
                // The check above, reached through the read rather than
                // through the flag ([`cancelled_read`]). This block's re-read
                // is abandoned either way, so the caller hears the same
                // `None`.
                Err(e) if cancelled_read(&e, scan_options) => return Ok(None),
                Err(e) => return Err(e),
            };
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
                        census_row(&mut census, plan.counter.as_ref(), row.raw);
                        if let Some(observer) = observer.as_deref_mut() {
                            observer.observe_row(row.offset - block.data_offset, row.raw);
                        }
                    }
                    Event::CopyEnd(end) => return Ok(Some((end, census))),
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
    database: Option<String>,
}

impl ResumeToken {
    fn start(query_fingerprint: u64) -> Self {
        Self { offset: 0, rows_emitted: 0, query_fingerprint, generation: 0, in_copy: None }
    }
}

/// A pull-mode stream of `RecordBatch`es for one table query.
/// Construct with [`table_stream`].
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
    /// alone (`crate::pgtype::ComparisonDivergence::affects_equality`), so a
    /// `text` column with no `COLLATE` clause earns a note under `<` and none
    /// under `=`. A third channel beside `DumpIndex.diagnostics` (L1) and
    /// `ResolvedSchema.notes` (L2), each a [`crate::diagnostic::Finding`] a
    /// caller drains into one sink with the others. Like
    /// [`Self::resolved_schema`], it describes the **last** block whose schema
    /// resolved.
    pub fn comparison_notes(&self) -> Vec<ComparisonNote> {
        self.comparison_notes.lock().unwrap().clone()
    }

    /// Facts about *this query's plan* rather than about a column or a
    /// predicate ([`PlanNoteKind`]): the memory budget in force declining
    /// something, and the row groups statistics let the replay skip. A fourth
    /// channel beside `DumpIndex.diagnostics` (L1), `ResolvedSchema.notes`
    /// (L2) and [`Self::comparison_notes`] (L4).
    ///
    /// **Settled before any block is read**, from the map and the source's own
    /// advice. A
    /// [`table_stream_partitions`] sub-stream holds them when it is handed
    /// back; [`table_stream`]'s serial replay, which plans no partitions and
    /// so never notes a budget, holds its pruning note once its first item is
    /// polled, and nothing before.
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
    /// closes, or statistics were not used — unless a [`DynamicFilter`]'s
    /// bound stopped it, which adds the block's entry when it leaves a byte
    /// unread.
    pub fn early_stops(&self) -> Vec<EarlyStop> {
        self.early_stops.lock().unwrap().clone()
    }

    /// The row groups this stream's [`DynamicFilter`] ruled out, so far —
    /// none where it was handed none, and none of the groups
    /// [`PlanNoteKind::StatisticsPruned`] counts, which the replay never
    /// reaches. **Each group is counted by one sub-stream**: one the cut at
    /// the first poll ruled out, by the first sub-stream to take that cut
    /// ([`DynamicPartitions`]); one ruled out as the replay reached it, by the
    /// sub-stream whose piece holds the byte its search starts at
    /// (`D + k·N − 1`, [`crate::prune::BlockPruning::kept`]'s terms), where
    /// that sub-stream skipped it — so a group cut between two sub-streams
    /// counts once at most, and not at all where the first read any of it.
    pub fn dynamic_filter_pruned_groups(&self) -> u64 {
        self.dynamic_pruned.load(Ordering::Relaxed)
    }

    /// The rows this stream's [`DynamicFilter`] dropped, so far, before
    /// decoding them: rows its static filter kept, in any block, that the
    /// state last read rejects — none under [`RowEvaluation::Off`]. None of
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
/// schema a [`RowBatcher`] built from it carries (see
/// [`TableStream::resolved_schema`]) — scoped to `database`, the block's own
/// attribution, never a guess (`docs/design/decisions.md`, "D49"). Its
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
fn resolve_block(
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
/// ([`census_metadata_level`]); [`SchemaMode::Strings`] reads no census.
fn refuse_metadata_level(matches: &[CopyBlock], query_options: &QueryOptions) -> Result<()> {
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
struct TableColumns {
    order: Vec<String>,
    /// One entry per name in `order`, its census unioned over every block
    /// ([`union_census`]).
    census: Vec<(String, ArrayShape)>,
    /// **The columns the untyped mode reads as their text**: under
    /// [`UnrepresentableMode::Text`], each whose blocks count, over every one
    /// of them, a value in the tiers the query's front end cannot hold — the
    /// count the refuse mode refuses by ([`materialized_unrepresentable`]),
    /// settled once for the table as its census is, so no two blocks of it
    /// disagree on a column's type (`docs/design/decisions.md`, "D100").
    /// Empty under every other mode.
    text: Vec<String>,
}

impl TableColumns {
    /// The table `matches` are the blocks of — one `(database, table)`
    /// target, already narrowed (`docs/design/decisions.md`, "D49") — as
    /// `query_options` reads it.
    fn settle(
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
            .and_then(|db| db.tables.get(&first.header.qualified_name()))
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
    fn text_for(&self, names: &[String]) -> Vec<bool> {
        names.iter().map(|name| self.text.contains(name)).collect()
    }

    /// `names`' census, one entry per name, in `names`' order.
    fn census_for(&self, names: &[String]) -> Vec<ArrayShape> {
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
/// total) and a fresh [`RowBatcher`] on the original block's schema.
fn resume_state(token: &ResumeToken, plan: &ReplayPlan) -> Result<(CopyScanner, Option<Opened>)> {
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

fn snapshot(
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

/// Everything a replay needs that the mapping pass produced, shared unchanged
/// by every sub-stream of a partitioned replay
/// (`docs/design/decisions.md`, "D51"). Held behind an `Arc`, `metadata` being
/// the whole dump's DDL, and never mutated once shared with a sub-stream —
/// `table_stream_partitions` writes the derived span onto `query_options`
/// after the mapping pass and before the `Arc`: the table's columns
/// in particular — their order, and their census, the union over **every**
/// block the query will replay — are settled once, so two partitions of one
/// table cannot order its columns or resolve its arrays differently
/// (`docs/design/decisions.md`, "D35").
#[derive(Clone)]
struct ReplayPlan {
    scan_options: ScanOptions,
    query_options: QueryOptions,
    metadata: Option<DumpMetadata>,
    table: TableColumns,
    /// Every matched block, resolved and keyed by the
    /// block's `header_offset` — see [`plan_blocks`], which refuses the whole
    /// plan on any block's resolution refusal.
    blocks: BTreeMap<u64, PlannedBlock>,
    /// The runs of row groups each pruned block keeps, keyed as `blocks` is
    /// ([`crate::prune::BlockPruning::kept`]); a block absent here is read
    /// whole.
    kept: BTreeMap<u64, Vec<Range<u64>>>,
    /// Where each block sorted on a column the filter bounds stops being read,
    /// keyed as `blocks` is ([`SortedStop`]).
    stops: BTreeMap<u64, SortedStop>,
    /// What pruning skipped, where any block's statistics were consulted.
    pruned: Option<PlanNote>,
    /// Each materialized column's values read as NULL
    /// ([`PlanNoteKind::ReadAsNull`]).
    read_as_null: Vec<PlanNote>,
    /// At least the rows the replay can emit ([`Pruned::rows`]).
    kept_rows: u64,
    /// At least the text bytes the replay emits of each projected field
    /// ([`Pruned::value_bytes`]).
    kept_value_bytes: Vec<Option<u64>>,
}

impl ReplayPlan {
    /// The plan for replaying `matches`: the table's columns
    /// ([`TableColumns::settle`]), and every one of its blocks resolved
    /// against them and the DDL before any sub-stream exists — or the table's
    /// refusal, or else the first refusing block's ([`plan_blocks`]), or else,
    /// under the refuse mode, its first materialized column holding a value
    /// the query cannot hold ([`materialized_unrepresentable`]).
    fn new(
        scan_options: ScanOptions,
        query_options: QueryOptions,
        matches: &[CopyBlock],
        metadata: Option<DumpMetadata>,
    ) -> Result<Self> {
        refuse_metadata_level(matches, &query_options)?;
        let table = TableColumns::settle(matches, metadata.as_ref(), &query_options)?;
        let blocks = plan_blocks(matches, &query_options, metadata.as_ref(), &table)?;
        let mut unrepresentable = materialized_unrepresentable(matches, &blocks, &query_options);
        if query_options.unrepresentable == UnrepresentableMode::Refuse
            && !unrepresentable.is_empty()
        {
            let MaterializedUnrepresentable { table, column, declared_type, values } =
                unrepresentable.swap_remove(0);
            return Err(Error::Unrepresentable { table, column, declared_type, values });
        }
        let read_as_null = read_as_null(unrepresentable, &query_options);
        let Pruned { kept, stops, note: pruned, rows: kept_rows, value_bytes: kept_value_bytes } =
            prune_blocks(matches, &blocks, &query_options, metadata.as_ref());
        Ok(Self {
            scan_options,
            query_options,
            metadata,
            table,
            blocks,
            kept,
            stops,
            pruned,
            read_as_null,
            kept_rows,
            kept_value_bytes,
        })
    }

    /// What the plan settled before any row is read, as every sub-stream
    /// carries it ([`TableStream::plan_notes`]) bar what cutting it into
    /// sub-streams adds.
    fn notes(&self) -> impl Iterator<Item = PlanNote> + '_ {
        self.read_as_null.iter().chain(&self.pruned).cloned()
    }

    /// The segments that replay `block` whole, or the runs of groups its
    /// statistics keep: the run holding group 0 starts at the block's header,
    /// as a whole block does, and every other starts inside its data.
    fn segments(&self, block: &CopyBlock) -> Vec<Segment> {
        let Some(kept) = self.kept.get(&block.header_offset) else {
            return vec![Segment::new(
                block,
                block.header_offset,
                block.end_offset,
                SegmentEntry::Header,
            )];
        };
        kept.iter().map(|run| Segment::over(block, run.clone())).collect()
    }
}

/// What [`prune_blocks`] settles for a [`ReplayPlan`].
struct Pruned {
    kept: BTreeMap<u64, Vec<Range<u64>>>,
    stops: BTreeMap<u64, SortedStop>,
    note: Option<PlanNote>,
    /// The rows of every group pruning kept, plus the `row_count` of every
    /// block it did not consult: never fewer than the replay emits, and
    /// nothing guessed below that — where a sorted block stops is found only
    /// as its rows are read.
    rows: u64,
    /// Per projected field, the text bytes of its values in the groups `rows`
    /// counts ([`crate::statistics::ColumnStatistics::value_bytes`]) — `None`
    /// wherever a block counted none — bounding what the replay emits of it
    /// as `rows` bounds its rows.
    value_bytes: Vec<Option<u64>>,
}

/// Settle which row groups of `matches` the query's filter skips
/// ([`prune_block`]), where a sorted block's rows stop being read, and the
/// [`PlanNote`] saying what was skipped. Nothing is pruned or stopped where
/// the caller turned statistics off or the filter reads no field — no
/// statistic can rule out a row of a filter that keeps every one.
///
/// **A block whose statistics keep every group is left out of `kept`**, so
/// it is replayed exactly as an unpruned query replays it, bar its stop.
fn prune_blocks(
    matches: &[CopyBlock],
    blocks: &BTreeMap<u64, PlannedBlock>,
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
) -> Pruned {
    let (mut kept, mut stops) = (BTreeMap::new(), BTreeMap::new());
    let mut rows = 0;
    let fields = blocks.values().next().map_or(0, |planned| planned.resolved.schema.fields().len());
    let mut value_bytes = vec![Some(0); fields];
    let (mut consulted, mut groups, mut skipped_groups, mut skipped_bytes) = (false, 0, 0, 0);
    for block in matches {
        let pruning = blocks
            .get(&block.header_offset)
            .filter(|planned| query_options.use_statistics && planned.filter.reads_fields())
            .and_then(|planned| {
                prune_block(block, &planned.filter, metadata, query_options.statistics_view())
            });
        let kept_groups = pruning.as_ref().map(|pruning| pruning.kept_groups.as_slice());
        let block_bytes = field_value_bytes(block, blocks.get(&block.header_offset), kept_groups);
        for (field, held) in value_bytes.iter_mut().enumerate() {
            *held = held.zip(block_bytes.get(field).copied().flatten()).map(|(a, b)| a + b);
        }
        let Some(pruning) = pruning else {
            rows += block.row_count;
            continue;
        };
        consulted = true;
        rows += pruning.kept_rows;
        groups += pruning.groups;
        skipped_groups += pruning.skipped_groups;
        skipped_bytes += pruning.skipped_bytes;
        if let Some(stop) = pruning.stop {
            stops.insert(block.header_offset, stop);
        }
        if pruning.skipped_groups > 0 {
            kept.insert(block.header_offset, pruning.kept);
        }
    }
    let bytes = matches.iter().map(|b| b.terminator_offset.saturating_sub(b.data_offset)).sum();
    let note = consulted.then_some(PlanNote {
        kind: PlanNoteKind::StatisticsPruned { skipped_groups, groups, skipped_bytes, bytes },
        levers: Vec::new(),
    });
    Pruned { kept, stops, note, rows, value_bytes }
}

/// A column the query materializes that holds values its front end cannot
/// hold, with the map's count of them in the tiers the query's semantics
/// cannot hold over every block of the table, not the groups a filter keeps.
struct MaterializedUnrepresentable {
    table: String,
    column: String,
    declared_type: String,
    values: u64,
}

/// **Each column the query materializes that holds a value its front end
/// cannot hold**, in the order the query emits them — typed, and never a
/// column read as `Utf8View`, whose text holds every value. What the refuse
/// mode refuses at planning ([`Error::Unrepresentable`],
/// `docs/design/decisions.md`, "D99") and the null mode warns of
/// ([`read_as_null`]).
fn materialized_unrepresentable(
    matches: &[CopyBlock],
    blocks: &BTreeMap<u64, PlannedBlock>,
    query_options: &QueryOptions,
) -> Vec<MaterializedUnrepresentable> {
    if query_options.schema_mode != SchemaMode::Typed {
        return Vec::new();
    }
    let (Some(first), Some(planned)) = (matches.first(), blocks.values().next()) else {
        return Vec::new();
    };
    let reach = query_options.unrepresentable_reach();
    let table = first.header.qualified_name();
    let fields = planned.resolved.schema.fields();
    let mut found = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        if matches!(field.data_type(), arrow::datatypes::DataType::Utf8View) {
            continue;
        }
        let values = unrepresentable_values(matches, field.name(), reach);
        if values == 0 {
            continue;
        }
        found.push(MaterializedUnrepresentable {
            table: table.clone(),
            column: field.name().clone(),
            declared_type: planned.resolved.notes[i].declared.clone().unwrap_or_default(),
            values,
        });
    }
    found
}

/// The map's count of column `name`'s values past the tiers `reach` names,
/// over every one of `matches`, the blocks of one table.
fn unrepresentable_values(matches: &[CopyBlock], name: &str, reach: UnrepresentableTier) -> u64 {
    matches
        .iter()
        .filter_map(|block| {
            let c = block.header.columns.iter().position(|column| column == name)?;
            let count = block.unrepresentable.as_ref()?.get(c)?;
            Some(count.format + if reach == UnrepresentableTier::Engine { count.engine } else { 0 })
        })
        .sum()
}

/// **Each column the query materializes that holds a value it reads as
/// NULL**, with the map's count ([`PlanNoteKind::ReadAsNull`]) — under
/// [`crate::UnrepresentableMode::Null`] alone.
fn read_as_null(
    columns: Vec<MaterializedUnrepresentable>,
    query_options: &QueryOptions,
) -> Vec<PlanNote> {
    if query_options.unrepresentable != UnrepresentableMode::Null {
        return Vec::new();
    }
    columns
        .into_iter()
        .map(|MaterializedUnrepresentable { table, column, declared_type, values }| PlanNote {
            kind: PlanNoteKind::ReadAsNull {
                table,
                column,
                declared_type,
                values,
                semantics: query_options.semantics,
            },
            levers: Vec::new(),
        })
        .collect()
}

/// Per field of `planned`, the text bytes `block`'s statistics count of its
/// values in the groups `kept` keeps — every group where `kept` is `None` —
/// and `None` for a field the block's statistics did not count
/// ([`crate::statistics::ColumnStatistics::value_bytes_where`]).
fn field_value_bytes(
    block: &CopyBlock,
    planned: Option<&PlannedBlock>,
    kept: Option<&[bool]>,
) -> Vec<Option<u64>> {
    let Some(planned) = planned else { return Vec::new() };
    let mut bytes = vec![None; planned.resolved.schema.fields().len()];
    let Some(statistics) = block.statistics.as_deref() else { return bytes };
    if statistics.columns.len() != block.header.columns.len() {
        return bytes;
    }
    let groups = statistics.groups.len();
    let keep = |g: usize| kept.is_none_or(|kept| kept.get(g).copied().unwrap_or(false));
    for (column, target) in statistics.columns.iter().zip(&planned.field_targets) {
        if let (Some(column), Some(field)) = (column, *target) {
            bytes[field] = column.value_bytes_where(groups, keep);
        }
    }
    bytes
}

/// One block's schema, filter and projection, resolved against a query —
/// everything [`activate`] builds a [`RowBatcher`] from, bar the batcher,
/// which holds rows and so is built per activation.
///
/// It records the two inputs the block itself contributes, so a lookup can
/// confirm it answers the activation asking rather than trusting the offset
/// alone; the rest are the [`ReplayPlan`]'s, and fixed.
#[derive(Debug, Clone)]
struct PlannedBlock {
    header: CopyHeader,
    database: Option<String>,
    /// The **projected** schema, which is what the batches carry.
    resolved: ResolvedSchema,
    field_targets: Vec<Option<usize>>,
    /// Per field of the block, how the query reads its values their type
    /// cannot hold ([`unrepresentable_reads`]) — what the batcher and the
    /// filter both read them by.
    unrepresentable: Vec<Option<UnrepresentableRead>>,
    filter: Arc<ResolvedExpr>,
    notes: Vec<ComparisonNote>,
}

/// Resolve one block for a query: its schema, the filter against
/// the unprojected schema, then the projection — in that order, which is the
/// order their refusals are raised in.
///
/// **An unprojected query projects the table's order** ([`TableColumns`]),
/// so the batches of a block listing its columns in another order come out
/// reordered by name, the same shape as every other block's.
///
/// `counts` is the block's unrepresentable count, where the map holds the
/// block ([`unrepresentable_reads`]).
fn resolve_for_query(
    header: &CopyHeader,
    header_offset: u64,
    database: Option<&str>,
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
    table: &TableColumns,
    counts: Option<&[Unrepresentable]>,
) -> Result<PlannedBlock> {
    let census = table.census_for(&header.columns);
    let text = table.text_for(&header.columns);
    let full = resolve_block(
        header,
        metadata,
        database,
        query_options.schema_mode,
        &census,
        &text,
        query_options.semantics,
    )?;
    let unrepresentable = query_reads(header, metadata, database, &full, counts, query_options);
    let tests = query_tests(header, metadata, database, counts, query_options)?;
    // Against the *unprojected* schema, in the block's own order: a term's
    // index numbers the raw row's fields, and a term may name a column the
    // projection dropped.
    let filter =
        resolve_expr(&query_options.filter, &full, header_offset, query_options.semantics)?
            .reading(&unrepresentable)
            .testing(&tests);
    let notes = filter.comparison_notes();
    let projection = query_options.projection.as_deref().unwrap_or(&table.order);
    let (resolved, field_targets) = project(&full, Some(projection), header_offset)?;
    Ok(PlannedBlock {
        header: header.clone(),
        database: database.map(str::to_string),
        resolved,
        field_targets,
        unrepresentable,
        filter: Arc::new(filter),
        notes,
    })
}

/// [`unrepresentable_reads`] for `query_options`' mode and front end.
fn query_reads(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    full: &ResolvedSchema,
    counts: Option<&[Unrepresentable]>,
    query_options: &QueryOptions,
) -> Vec<Option<UnrepresentableRead>> {
    unrepresentable_reads(
        header,
        metadata,
        database,
        full,
        counts,
        query_options.unrepresentable,
        query_options.unrepresentable_reach(),
    )
}

/// What `query_options`' unrepresentable tests test each column's text by
/// ([`unrepresentable_tests`]), and nothing where its filter holds none — or
/// its refusal, under [`SchemaMode::Strings`], which resolves no declared type
/// for a test to read (`docs/design/decisions.md`, "D101").
fn query_tests(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    counts: Option<&[Unrepresentable]>,
    query_options: &QueryOptions,
) -> Result<Vec<Option<UnrepresentableRead>>> {
    let Some(test) = query_options.filter.first_unrepresentable_test() else {
        return Ok(Vec::new());
    };
    if query_options.schema_mode == SchemaMode::Strings {
        return Err(Error::UnrepresentableTestUntyped {
            column: test.column.clone(),
            op: test.op.symbol(),
        });
    }
    let reach = query_options.unrepresentable_reach();
    Ok(unrepresentable_tests(header, metadata, database, counts, reach))
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
fn plan_blocks(
    matches: &[CopyBlock],
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
    table: &TableColumns,
) -> Result<BTreeMap<u64, PlannedBlock>> {
    matches
        .iter()
        .map(|block| {
            let planned = resolve_for_query(
                &block.header,
                block.header_offset,
                block.database.as_deref(),
                query_options,
                metadata,
                table,
                block.unrepresentable.as_deref(),
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
    /// Inside the block's data: reading begins at the first row boundary
    /// after [`Segment::start`], and the schema comes from the map's own
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
    /// The block this piece is of, **without its statistics**
    /// ([`Segment::new`]).
    block: CopyBlock,
    start: u64,
    limit: u64,
    entry: SegmentEntry,
}

impl Segment {
    /// The piece of `block` from `start` to `limit`, entered as `entry`.
    ///
    /// **The copy drops the block's statistics.** Nothing reading a segment
    /// needs them once [`ReplayPlan::new`] has pruned — a replay under a
    /// [`DynamicFilter`] reads them from the [`TablePartitions`] holding them
    /// — and a copy keeping the `Arc` would hold every matched block's
    /// resident for as long as the replay runs, after the map that loaded
    /// them is gone, which is what lets a caller of [`table_stream`] or
    /// [`table_stream_partitions`] carve a replay with nothing held for them.
    fn new(block: &CopyBlock, start: u64, limit: u64, entry: SegmentEntry) -> Self {
        let block = CopyBlock { statistics: None, ..block.clone() };
        Segment { block, start, limit, entry }
    }

    /// The piece of `block` replaying `run`, a range in segment terms: the
    /// run that begins before the block's first row starts at its header, as
    /// a whole block's first piece does, and any other inside its data.
    fn over(block: &CopyBlock, run: Range<u64>) -> Self {
        let (start, entry) = if run.start < block.data_offset {
            (block.header_offset, SegmentEntry::Header)
        } else {
            (run.start, SegmentEntry::Interior)
        };
        Segment::new(block, start, run.end, entry)
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

    /// The rest of this piece from `start`, the search start of a group
    /// inside its data ([`DynamicRead::at_row`]): entered there, its limit
    /// unchanged.
    fn rest_from(&self, start: u64) -> Self {
        Segment::new(&self.block, start, self.limit, SegmentEntry::Interior)
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
    early_stops: Arc<Mutex<Vec<EarlyStop>>>,
    dynamic_pruned: Arc<AtomicU64>,
    dynamic_rows: Arc<AtomicU64>,
}

impl StreamShared {
    fn new(token: ResumeToken) -> Self {
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
            early_stops: self.early_stops,
            dynamic_pruned: self.dynamic_pruned,
            dynamic_rows: self.dynamic_rows,
        }
    }
}

/// Take one block's resolution from `plan` — resolving it here where the
/// plan holds none ([`plan_blocks`]) — and build the [`RowBatcher`] that will
/// hold its rows.
///
/// The three places a block becomes active — a `COPY` header the scanner
/// read, a partition that started inside a block, and a resumed token —
/// differ only in where the header comes from.
fn activate(
    header: CopyHeader,
    header_offset: u64,
    database: Option<String>,
    plan: &ReplayPlan,
) -> Result<Opened> {
    let resolved_here;
    let block = match plan
        .blocks
        .get(&header_offset)
        .filter(|planned| planned.database == database && planned.header == header)
    {
        Some(planned) => planned,
        None => {
            resolved_here = resolve_for_query(
                &header,
                header_offset,
                database.as_deref(),
                &plan.query_options,
                plan.metadata.as_ref(),
                &plan.table,
                None,
            )?;
            &resolved_here
        }
    };
    let batcher = RowBatcher::new(
        &block.resolved,
        header.qualified_name(),
        plan.query_options.clone(),
        block.field_targets.clone(),
        block.unrepresentable.clone(),
    );
    let (resolved, notes) = (block.resolved.clone(), block.notes.clone());
    Ok(((header_offset, header, batcher, Arc::clone(&block.filter), database), resolved, notes))
}

/// The offset of the first row boundary after `from`, searching no
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
        let want = options.chunk_size_bytes.min((end - pos) as usize);
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
/// and the DDL to type them against.
struct MappedTable {
    matches: Vec<CopyBlock>,
    metadata: Option<DumpMetadata>,
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
/// name-only matches to at most one candidate, whose census the plan takes
/// ([`TableColumns`]). Yields
/// no rows — see the module docs. **Both entry points run exactly this**, so a
/// partitioned replay maps the file once rather than once per sub-stream.
async fn map_for_query(
    source: &dyn ByteRangeSource,
    table: &str,
    scan_options: &ScanOptions,
    query_options: &QueryOptions,
    cache: &CacheMode,
    watch: &SourceWatch,
) -> Result<MappedTable> {
    let size = source.size().await?;

    let mut index = match cache.load(source).await? {
        CacheLoad::Index(index) => index,
        // Nothing to build forward from, and nothing at the path to keep.
        CacheLoad::Disabled | CacheLoad::Missing => DumpIndex::default(),
        // Refused before a byte of the dump is read, unless the caller said
        // this cache may be replaced (`docs/design/decisions.md`, "D20").
        CacheLoad::Unusable(unusable) => match cache.refusal(&unusable) {
            Some(refusal) => return Err(refusal),
            None => DumpIndex::default(),
        },
    };
    // The one statistics term a query has: it keeps no account, so nothing else
    // says what the cache handed it.
    #[cfg(feature = "introspect")]
    statistics_loaded(index.statistics_heap_bytes());

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
        cache.save(watch, source, &index).await?;
    }
    // Pass 1: extend the map until this query's table is settled. No
    // rows come out of this, and nothing is yielded until it returns.
    let selector = query_options.database.as_deref();
    let target = match query_options.scan_extent {
        ScanExtent::UntilTargetSettled => Some((table, selector)),
        ScanExtent::Full => None,
    };
    // A cancelled mapping pass is an error here rather than a short stream
    // (`docs/design/decisions.md`, "D48"). `pgdt query` never sets the flag;
    // an embedder that does gets told.
    // A pass gathering nothing charges nothing, so its account is never read.
    let account = Arc::default();
    if map_forward(
        source,
        scan_options,
        cache,
        watch,
        &mut index,
        target,
        Pass::Query,
        &account,
        size,
    )
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
    // the fact. An ambiguous name among the blocks mapped errors before a
    // single row goes out; one past an early stop is never seen (`KD6`).
    let mut matches: Vec<CopyBlock> = index
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

    census_metadata_level(
        source,
        scan_options,
        query_options,
        cache,
        metadata.as_ref(),
        &mut matches,
        index.scanned_through,
        size,
    )
    .await?;

    // **A streamed schema needs no completeness test**
    // (`docs/design/decisions.md`, "D34"): the mapping pass has finished and
    // `matches` is fixed, so the census the plan unions over them is the
    // evidence for exactly the rows this stream will hand back
    // (`docs/design/decisions.md`, "D35").
    Ok(MappedTable { matches, metadata })
}

/// **A query's cold semantics over a table the map holds at the metadata
/// level** (`docs/design/decisions.md`, "D35"): each of `matches` holding no
/// census is read again for one, as a query's own mapping pass would have
/// censused it, and holds it for this query alone — the map and the cache
/// keep the level `parse` gave them, so the price is a second read of the
/// table on every such query. Under [`SchemaMode::Strings`], which reads no
/// census, nothing is read. A cancelled read is
/// [`Error::ScanCancelled`] at `scanned_through`, as a cancelled mapping pass
/// is.
#[allow(clippy::too_many_arguments)]
async fn census_metadata_level(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    query_options: &QueryOptions,
    cache: &CacheMode,
    metadata: Option<&DumpMetadata>,
    matches: &mut [CopyBlock],
    scanned_through: u64,
    size: u64,
) -> Result<()> {
    if query_options.schema_mode != SchemaMode::Typed {
        return Ok(());
    }
    let lacking: Vec<usize> =
        (0..matches.len()).filter(|&i| matches[i].array_shapes.is_none()).collect();
    let Some(&first) = lacking.first() else { return Ok(()) };
    tracing::info!(
        table = matches[first].header.qualified_name(),
        blocks = lacking.len(),
        "table mapped at the metadata level: reading its rows again for this query's census",
    );
    announce_read_loop(source, scan_options);
    let cache_path = match cache {
        CacheMode::Enabled { path, .. } => Some(path.as_path()),
        CacheMode::Disabled { .. } | CacheMode::Offline(_) => None,
    };
    let mut shortfall_reported = false;
    for at in lacking {
        let read = reread_rows(
            source,
            scan_options,
            cache_path,
            metadata,
            &matches[at],
            size,
            None,
            &mut shortfall_reported,
        )
        .await?;
        let Some(census) = read else {
            return Err(Error::ScanCancelled { scanned_through });
        };
        matches[at].set_census(census);
    }
    Ok(())
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
            // increasing, so `cuts.dedup()` below is a no-op on this arm — as
            // it is on the other, once `want` is clamped to the length.
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
/// any of the other three (`docs/design/decisions.md`, "D19"), and a
/// [`Finding`] like each of them, so one sink drains all four. See
/// [`TableStream::plan_notes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanNote {
    pub kind: PlanNoteKind,
    /// What a caller could change that would move what this note reports,
    /// **for this source and this budget** — empty for a note naming no
    /// budget. Computed where the plan reads the source's cost and the budget
    /// it was carved, so a caller mapping each lever onto its own setting
    /// names none that is inert here without re-deriving either
    /// ([`PlanLever`]).
    ///
    /// **A subset of what the sentence names, never more**: the message
    /// offers a lever wherever it can apply and says where it cannot, as
    /// [`PlanNoteKind::ParallelismBudgetLimited`]'s does of the budget on a
    /// plain source, while this lists only the ones that apply.
    pub levers: Vec<PlanLever>,
}

/// One of the settings a caller holds that a [`PlanNote`] can name, in the
/// library's vocabulary and in the order [`PlanNote::levers`] lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlanLever {
    /// A larger resident allowance, carved as [`Parallelism::within`] carves
    /// one — listed only where it would raise the budget the note quotes
    /// ([`Parallelism::allowance_raises_budget`]), which on a source
    /// recommending no per-reader cost stops at
    /// `crate::io::DEFAULT_MEMORY_BUDGET`.
    LargerAllowance,
    /// A smaller read chunk (`crate::scan::ScanOptions::chunk_size_bytes`) —
    /// listed only where the source sizes a reader from it
    /// ([`crate::io::Partitioning::sized_by_read_chunk`]) or the plan charged
    /// a batch span floored on it ([`derived_source_span`]).
    SmallerReadChunk,
    /// Fewer sub-streams asked for (`Parallelism::jobs`), which leaves each a
    /// larger batch ([`PlanNoteKind::BatchSpanNarrowed`]).
    FewerSubStreams,
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
    ///
    /// **`max_source_span` here is what was charged, not what was stated**:
    /// the span is derived down before the count is cut
    /// ([`derived_source_span`]), so this note beside a
    /// [`PlanNoteKind::BatchSpanNarrowed`] names that smaller number and the
    /// span is not among the levers left.
    ///
    /// **Both levers that are left are named, because neither always works**
    /// (`KD32`, as [`PlanNoteKind::BatchSpanNarrowed`] states it too, and `KD41`):
    /// a source recommending no per-reader cost is left on
    /// `crate::io::DEFAULT_MEMORY_BUDGET` however large an allowance is stated
    /// (`docs/design/decisions.md`, "D83"), so a message offering the budget
    /// alone is inert exactly where this fires on a plain one. The other is
    /// the announced read chunk
    /// (`crate::scan::ScanOptions::chunk_size_bytes`), which a replay plan
    /// never announces (`KD41`), and which a source cutting by it sizes both
    /// terms from — `footprint` a fixed multiple of it
    /// (`crate::io::Partitioning::partition_bytes`) and the span floored on it
    /// ([`derived_source_span`]).
    ///
    /// **What a seat buys is not said here**: whether the sub-streams seated
    /// run concurrently is decided by how the caller drains them, which a plan
    /// cannot see, so it is the front end's to say (`pgdt query`'s merge,
    /// `KD57`).
    ParallelismBudgetLimited {
        requested: usize,
        planned: usize,
        footprint: u64,
        max_source_span: Option<u64>,
        memory_bytes: u64,
    },
    /// The `.xz` source this query reads declined the block-decode path: the
    /// memory budget in force does not afford one block-decoding reader, so it
    /// reads each block **in pieces** — the *streaming decoder* this note and
    /// the manual name, one live `xz_seek::BlockRead` behind a mutex — and
    /// every **backward** read decodes forward from its block's start
    /// (`docs/design/decisions.md`, "D15").
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
    /// mechanism (`crate::io::WorkerMemory::affords`' one-worker floor,
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
    ///
    /// **It names its levers as its siblings do**: a larger memory budget
    /// always, with their caveat that a larger allowance is not one on a
    /// source recommending no per-reader cost (`KD32`), and a smaller read
    /// chunk only where [`PlanNote::levers`] lists one — where the source
    /// sizes a reader from the chunk and the budget is above zero, a budget of
    /// zero being below every reader however small its chunk. On a plain
    /// source the three arrangements differ: a zero budget moves with memory
    /// alone, one between zero and a reader with memory or the chunk, and one
    /// at `crate::io::DEFAULT_MEMORY_BUDGET` under a chunk past it with the
    /// chunk alone.
    AllocationBelowFloor { unit_bytes: u64, memory_bytes: u64 },
    /// The batch span this query stated
    /// (`crate::batch::QueryOptions::max_source_span`) did not leave room for
    /// the workers it asked for, so the plan charged `planned_bytes` instead
    /// of `stated_bytes` and the sub-streams' batches are that much smaller
    /// ([`derived_source_span`]; `docs/design/decisions.md`, "D84").
    ///
    /// **Not a shortfall in itself**: `workers` is what the narrowed span did
    /// buy, and where it still came up short of what was asked for a
    /// [`PlanNoteKind::ParallelismBudgetLimited`] says so beside this. A span
    /// at one announced read chunk
    /// (`crate::scan::ScanOptions::chunk_size_bytes`) is the floor, and what
    /// moves a span off it is what [`derived_source_span`] divides: a larger
    /// `memory_bytes`, a smaller `Parallelism::jobs`, or a smaller charge per
    /// reader — which a source sizing its reader from the read chunk
    /// ([`crate::io::Partitioning::sized_by_read_chunk`]) lowers with the
    /// chunk.
    ///
    /// **The sub-streams and the budget are always named, because only the
    /// first is always the caller's** (`KD32`): a source recommending no
    /// per-reader cost is left on `crate::io::DEFAULT_MEMORY_BUDGET` however
    /// large an allowance is stated (`docs/design/decisions.md`, "D83"), and
    /// which end this run's `memory_bytes` came from is a caller's own fact to
    /// add ([`PlanNote::budget_bytes`]; `docs/design/decisions.md`, "D64").
    /// **The chunk is named only where [`PlanNote::levers`] lists it**, in
    /// [`PlanNoteKind::AllocationBelowFloor`]'s clause.
    ///
    /// **Not a [`crate::diagnostic::DiagnosticKind`]**, for the reason its
    /// siblings are not (`docs/design/decisions.md`, "D19").
    BatchSpanNarrowed { stated_bytes: u64, planned_bytes: u64, workers: usize, memory_bytes: u64 },
    /// The row-group statistics a mapping pass stored proved that
    /// `skipped_groups` of the `groups` listed by the blocks carrying them
    /// hold no row the filter keeps, so their `skipped_bytes` of rows are
    /// never parsed — read at most as the tail of a kept run's last chunk —
    /// out of the `bytes` of rows every matched block holds
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
    /// A column this query materializes holds `values` values PostgreSQL
    /// accepts for its declared type and its type cannot hold, which the
    /// query reads as NULL ([`crate::UnrepresentableMode::Null`];
    /// `docs/design/decisions.md`, "D98"). One per such column.
    ///
    /// **A property of the table, never of the run**: `values` is the map's
    /// count over every block of the table, in the tiers the query's semantics
    /// cannot hold, so it does not move with a `LIMIT`, a pruned group or a
    /// dynamic filter's skip, which decide only how many of them a run met.
    ///
    /// The sentence names the filter term that tells those values from the
    /// NULLs the dump holds, in the spelling of the front end `semantics`
    /// names (`docs/design/decisions.md`, "D101").
    ReadAsNull {
        table: String,
        column: String,
        declared_type: String,
        values: u64,
        semantics: ComparisonSemantics,
    },
}

/// One block a [`TableStream`] replayed under an early stop — the filter, or
/// a [`DynamicFilter`]'s state, requiring a bound the block's stored row
/// order closes — and what the stop left unread ([`TableStream::early_stops`];
/// `docs/design/decisions.md`, "D80").
///
/// **Per block, not a count**, because a partitioned replay can hand one
/// block's pieces to several sub-streams and a piece past the stopping row
/// stops at its own first row: a caller summing over sub-streams counts a
/// block once by its `header_offset`, and adds its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlyStop {
    /// The block, by its [`CopyBlock::header_offset`].
    pub header_offset: u64,
    /// The bytes of rows the stop left unread in the pieces of the block this
    /// stream replayed, summed; `None` where it ended no piece before its last
    /// row — the bound never passed, or passed only there — which saved
    /// nothing.
    ///
    /// **Measured from the end of the stopping row to the piece's limit, that
    /// byte included**, or
    /// to the block's `\.` where the piece runs to it. A piece owns the row
    /// straddling its limit, whose rest past the limit is not counted, so this
    /// is short of the unread bytes by at most that one row's tail per piece
    /// and never counts a byte a skipped group's
    /// [`PlanNoteKind::StatisticsPruned`] `skipped_bytes` does: the two add.
    pub unread_bytes: Option<u64>,
}

impl PlanNote {
    fn compressed_block_path_declined(
        block_count: usize,
        max_block_uncompressed: u64,
        reader_bytes: u64,
        memory_bytes: u64,
        allowance_raises: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::CompressedBlockPathDeclined {
                block_count,
                max_block_uncompressed,
                reader_bytes,
                memory_bytes,
            },
            levers: levers(allowance_raises, false, false),
        }
    }

    fn allocation_below_floor(
        unit_bytes: u64,
        memory_bytes: u64,
        allowance_raises: bool,
        chunk_sized: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::AllocationBelowFloor { unit_bytes, memory_bytes },
            levers: levers(allowance_raises, chunk_sized && memory_bytes > 0, false),
        }
    }

    fn batch_span_narrowed(
        stated_bytes: u64,
        planned_bytes: u64,
        workers: usize,
        memory_bytes: u64,
        allowance_raises: bool,
        chunk_sized: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::BatchSpanNarrowed {
                stated_bytes,
                planned_bytes,
                workers,
                memory_bytes,
            },
            levers: levers(allowance_raises, chunk_sized, true),
        }
    }

    fn parallelism_budget_limited(
        requested: usize,
        planned: usize,
        footprint: u64,
        max_source_span: Option<u64>,
        memory_bytes: u64,
        allowance_raises: bool,
        chunk_sized: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::ParallelismBudgetLimited {
                requested,
                planned,
                footprint,
                max_source_span,
                memory_bytes,
            },
            // The span it charged is floored on the chunk wherever it fired.
            levers: levers(allowance_raises, chunk_sized || max_source_span.is_some(), false),
        }
    }

    /// The read-buffer budget this note's [`PlanNote::message`] quotes, where
    /// it quotes one — every kind but [`PlanNoteKind::StatisticsPruned`] and
    /// [`PlanNoteKind::ReadAsNull`], whose facts are about what the map holds
    /// and name no budget at all.
    ///
    /// **It exists so a caller can say where that number came from.**
    /// Provenance is the caller's fact and never the library's
    /// (`docs/design/decisions.md`, "D64"), so the clause naming it is
    /// appended outside; which notes want one is a property of the note and
    /// not of the severity a caller prints it at — a narrowed span quotes a
    /// budget and is no fault ([`PlanNoteKind::BatchSpanNarrowed`];
    /// `docs/design/decisions.md`, "D84").
    pub fn budget_bytes(&self) -> Option<u64> {
        match &self.kind {
            PlanNoteKind::ParallelismBudgetLimited { memory_bytes, .. }
            | PlanNoteKind::CompressedBlockPathDeclined { memory_bytes, .. }
            | PlanNoteKind::AllocationBelowFloor { memory_bytes, .. }
            | PlanNoteKind::BatchSpanNarrowed { memory_bytes, .. } => Some(*memory_bytes),
            PlanNoteKind::StatisticsPruned { .. } | PlanNoteKind::ReadAsNull { .. } => None,
        }
    }

    /// The clause offering a smaller read chunk, exactly where
    /// [`PlanNote::levers`] lists one, and empty elsewhere — one wording for
    /// every sentence that appends it.
    fn chunk_clause(&self) -> &'static str {
        if self.levers.contains(&PlanLever::SmallerReadChunk) {
            ", or a smaller read chunk, which this source sizes a reader from"
        } else {
            ""
        }
    }
}

impl Finding for PlanNote {
    /// `Info` for a fact that is no fault — a skip, and a span the plan
    /// narrowed to seat the readers asked for (`docs/design/decisions.md`,
    /// "D84"), where the `Warning` beside it is what says a count still came
    /// up short — and `Warning` for every note saying the budget in force
    /// declined something asked of it, and for values read as NULL. Derived from the kind, as
    /// `ColumnNote`'s is from its resolution, so a caller printing it and one
    /// draining it into a sink cannot disagree.
    fn severity(&self) -> Severity {
        match self.kind {
            PlanNoteKind::StatisticsPruned { .. } | PlanNoteKind::BatchSpanNarrowed { .. } => {
                Severity::Info
            }
            PlanNoteKind::ParallelismBudgetLimited { .. }
            | PlanNoteKind::CompressedBlockPathDeclined { .. }
            | PlanNoteKind::AllocationBelowFloor { .. }
            | PlanNoteKind::ReadAsNull { .. } => Severity::Warning,
        }
    }

    /// One sentence naming why the plan fell short of what was asked, and what
    /// to raise to close the gap — in the library's own vocabulary rather than
    /// any caller's flag names, as `ComparisonNote`'s is.
    fn message(&self) -> String {
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
                     byte(s) to decode plus {span} byte(s) held by its own batch — the batch \
                     span is already as small as the plan will make it, so what seats more is \
                     a smaller read chunk, which a source cutting by one sizes both of those \
                     terms from, or a larger memory budget, which is not the same as a larger \
                     allowance on a source that recommends no per-reader cost of its own"
                ),
                None => format!(
                    "asked for up to {requested} sub-stream(s), but a memory budget of \
                     {memory_bytes} byte(s) affords only {planned} at {footprint} byte(s) to \
                     decode each — what seats more is a smaller read chunk, which a source \
                     cutting by one sizes that cost from, or a larger memory budget, which is \
                     not the same as a larger allowance on a source that recommends no \
                     per-reader cost of its own"
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
            PlanNoteKind::AllocationBelowFloor { unit_bytes, memory_bytes } => {
                let chunk = self.chunk_clause();
                format!(
                    "a memory budget of {memory_bytes} byte(s) is less than the {unit_bytes} \
                     byte(s) one reader of this source holds, so this runs at its one-slot floor \
                     whatever concurrency is asked for — what lifts it off the floor is a larger \
                     memory budget, which is not the same as a larger allowance on a source that \
                     recommends no per-reader cost of its own{chunk}"
                )
            }
            PlanNoteKind::BatchSpanNarrowed {
                stated_bytes,
                planned_bytes,
                workers,
                memory_bytes,
            } => {
                let chunk = self.chunk_clause();
                format!(
                    "a memory budget of {memory_bytes} byte(s) cannot seat the sub-streams asked \
                     for beside batches spanning {stated_bytes} byte(s) of the source each, so \
                     each batch spans at most {planned_bytes} byte(s) instead and {workers} \
                     sub-stream(s) were planned — asking for fewer sub-streams leaves each a \
                     larger batch; so does a larger memory budget, which is not the same as a \
                     larger allowance on a source that recommends no per-reader cost of its \
                     own{chunk}"
                )
            }
            PlanNoteKind::StatisticsPruned { skipped_groups, groups, skipped_bytes, bytes } => {
                format!(
                    "row-group statistics rule out {skipped_groups} of {groups} group(s), so \
                     {skipped_bytes} of the {bytes} byte(s) of rows this table holds are not read"
                )
            }
            PlanNoteKind::ReadAsNull { table, column, declared_type, values, semantics } => {
                let term = match semantics {
                    ComparisonSemantics::Postgres => format!("`{column} IS UNREPRESENTABLE`"),
                    ComparisonSemantics::DataFusion => {
                        format!("`pgdump_unrepresentable({column})`")
                    }
                };
                format!(
                    "{table}.{column} holds {values} value(s) its type `{declared_type}` cannot \
                     hold, read as NULL — {term} tells them from the NULLs the dump holds"
                )
            }
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// [`PlanNote::levers`] from which of them apply, in [`PlanLever`]'s order.
fn levers(allowance: bool, chunk: bool, fewer: bool) -> Vec<PlanLever> {
    [
        (allowance, PlanLever::LargerAllowance),
        (chunk, PlanLever::SmallerReadChunk),
        (fewer, PlanLever::FewerSubStreams),
    ]
    .into_iter()
    .filter_map(|(applies, lever)| applies.then_some(lever))
    .collect()
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
    // The budget actually in force, which is what the source compared its
    // reader against: a caller that stated no number leaves every pool on
    // this one.
    let memory_bytes = parallelism.memory_bytes().unwrap_or(DEFAULT_MEMORY_BUDGET);
    Some(PlanNote::compressed_block_path_declined(
        table.block_count(),
        table.max_block_uncompressed(),
        // What a reader of this file would have held on the path it declined.
        // The fallback is unreachable, but nothing in the trait obliges the
        // two answers to agree.
        source.block_decode_bytes().unwrap_or(0),
        memory_bytes,
        Parallelism::allowance_raises_budget(source.default_worker_memory(), memory_bytes),
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
/// **And that second term is derived, not taken** ([`derived_source_span`];
/// `docs/design/decisions.md`, "D84"): the stated span is a ceiling, and a
/// budget that cannot seat `jobs` readers beside it narrows the span before it
/// cuts the count, stopping at `chunk_size` — the length this caller's replay
/// will announce to the source, and so the unit a batch actually pins. The
/// third return value is the span charged — the stated one where none was,
/// every block's advice retaining by partition — which the caller writes back onto
/// the sub-streams' own `QueryOptions` so the batches are the size the plan
/// was solved for.
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
/// ([`compressed_block_path_declined`]), at most one naming a budget that
/// affords less than a single reader ([`PlanNoteKind::AllocationBelowFloor`]),
/// and at most one naming a batch span narrowed below what the caller stated
/// ([`PlanNoteKind::BatchSpanNarrowed`]).**
/// Empty on every path that limits nothing, so a caller need not special-case
/// "nothing to say" — though an empty `matches`, still charged its span, can
/// name a count the budget cut. The fourth is each block's advice, which a cut
/// made again at a dynamic filter's first poll cuts by ([`cut_blocks`]).
///
/// **What these sub-streams buy is the caller's to collect**: they run
/// concurrently only where every one is polled, as a DataFusion scan polls its
/// partitions, and `pgdt query`'s in-order merge reads one at a time past its
/// first round (`KD57`).
///
/// Deficiency register: `deficiency: KD23` — the cut here is over a whole
/// `CopyBlock` rather than through the leader's window, so a piece spans as
/// many units as the region holds over the worker count and a held batch's
/// `max_source_span` can pin several decoded blocks where
/// [`crate::io::RetainedUnit::Partition`] bills one;
/// `BOUNDARIED_PARTITION_UNITS` does not bound this path. **(c) unowned**;
/// promoted by a phase taking up query-path memory, the repair reversing a
/// recorded decision either way.
///
/// Deficiency register: `deficiency: KD41` — `chunk_size` is never announced
/// here, so a plain source's advice is cut from the chunk the last read on it
/// announced, or `POOL_MAX_BYTES` where none has: a `pgdt query` over a
/// complete cache prices its unit at the default chunk whatever
/// `--chunk-size` states, and a provider's first scan after `SET
/// pgdump.chunk_size` at the chunk before it. Only `max_source_span` follows
/// the stated chunk, so the smaller chunk
/// [`PlanNoteKind::ParallelismBudgetLimited`] advises does not seat more on
/// the plan it advises. **(c) unowned**; promoted by a stated chunk seen not
/// to seat what the note promised, the fix being `hint_read_size(chunk_size)`
/// beside `hint_parallelism` — a plan announcing a read size to a source
/// other scans share.
fn plan_partitions(
    source: &dyn ByteRangeSource,
    matches: &[CopyBlock],
    kept: &BTreeMap<u64, Vec<Range<u64>>>,
    parallelism: Parallelism,
    max_source_span: Option<usize>,
    chunk_size: usize,
) -> (Vec<Vec<Segment>>, Vec<PlanNote>, Option<usize>, Vec<Partitioning>) {
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
    let widest = advice.iter().max_by_key(|advice| advice.partition_bytes());
    let footprint = widest.map_or(0, Partitioning::partition_bytes);
    // What the notes below offer, read off the source's cost and the budget
    // it was carved by: both callers carve with this recommendation
    // (`Parallelism::within`, `Parallelism::discover_for`).
    let chunk_sized = widest.is_some_and(Partitioning::sized_by_read_chunk);
    let recommended = source.default_worker_memory();
    let raises = |budget| Parallelism::allowance_raises_budget(recommended, budget);
    // The span is a cost of a chunk-shaped source only. `all` over an empty
    // advice would be vacuously true, so the emptiness is tested.
    let retains_partitions =
        !advice.is_empty() && advice.iter().all(|a| a.retained_unit() == RetainedUnit::Partition);
    let stated_span = if retains_partitions { None } else { max_source_span };
    // The charge the count is solved against: the largest per-worker footprint
    // above, carrying that source's shared pool term, plus this caller's span.
    let charge = advice
        .iter()
        .map(Partitioning::worker_memory)
        .max_by_key(|memory| memory.bytes_per_worker())
        .unwrap_or_default();
    // The span is derived from what the budget leaves once the readers asked
    // for are paid for, so `jobs` readers cost batch size rather than being
    // declined (`docs/design/decisions.md`, "D84").
    let charged_span =
        stated_span.map(|span| derived_source_span(charge, parallelism, span, chunk_size));
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
        notes.push(PlanNote::allocation_below_floor(
            footprint,
            memory_bytes,
            raises(memory_bytes),
            chunk_sized,
        ));
    }
    // **Said whenever the span moved, not only where the count fell short**:
    // the span is a number the caller stated on its own `QueryOptions`, so a
    // plan that charged a smaller one has not delivered what was asked for.
    if let (Some(memory_bytes), Some(stated), Some(planned)) =
        (parallelism.memory_bytes(), stated_span, charged_span)
        && planned < stated
    {
        notes.push(PlanNote::batch_span_narrowed(
            stated as u64,
            planned as u64,
            workers,
            memory_bytes,
            raises(memory_bytes),
            chunk_sized,
        ));
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
            raises(memory_bytes),
            chunk_sized,
        ));
    }

    let groups = cut_blocks(matches, kept, &advice, workers);
    (groups, notes, charged_span.or(max_source_span), advice)
}

/// Cut `matches` — each whole, or the runs of row groups `kept` holds of it —
/// into the pieces each block's `advice` permits, and group them into at most
/// `workers` sub-streams ([`distribute`]), byte-balanced over what they read.
fn cut_blocks(
    matches: &[CopyBlock],
    kept: &BTreeMap<u64, Vec<Range<u64>>>,
    advice: &[Partitioning],
    workers: usize,
) -> Vec<Vec<Segment>> {
    let mut segments = Vec::new();
    for (block, advice) in matches.iter().zip(advice) {
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
                segments.push(Segment::new(block, start, piece.end, entry));
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
    distribute(segments, workers)
}

/// The span a sub-stream's held batch is charged, given what one reader of
/// this source costs and what the caller stated
/// (`crate::batch::QueryOptions::max_source_span`, which is a ceiling).
///
/// **It moves down or not at all** (`docs/design/decisions.md`, "D84"). Where
/// the stated span already affords the count asked for, it is returned
/// unchanged — so a serial replay, whose count is one whatever the charge, is
/// never narrowed. Where it does not, the span becomes what the budget has
/// left once `jobs` readers of the source are paid for:
/// `charge.plus_per_worker(s).at(jobs)` is `charge.at(jobs) + s × jobs`, so
/// the largest `s` that fits is `(budget − charge.at(jobs)) / jobs` — an
/// inversion of [`crate::io::WorkerMemory::at`] rather than a second copy of
/// its arithmetic, which keeps the shared pool term out of this function's
/// hands.
///
/// **The floor is `chunk_size`**, the length the replay loop announces to the
/// source (`crate::scan::ScanOptions::chunk_size_bytes`,
/// [`crate::io::ByteRangeSource::hint_read_size`]), so a budget that affords
/// the count at no span at all narrows to one read chunk and leaves the
/// shortfall to [`worker_count`]; below that the span would cost rows per
/// batch while bounding nothing the retained unit does not already bound.
/// **It is the announced chunk and not the shipped default**: the pin is the
/// span rounded out to what this source actually retains, which is what this
/// caller asked it to read in.
///
/// A caller stating no budget (`Parallelism::default`) derives nothing: there
/// is no number to solve against.
// Deficiency register: `deficiency: KD32` — the budget divided here is the
// read-buffer budget, which a plain source holds at `DEFAULT_MEMORY_BUDGET`
// whatever allowance was stated, so a plain `query`'s span and its sub-stream
// count are both fixed at a number picked to answer a different question.
// **(c) unowned**; closing it means a plain source recommending a per-reader
// cost, which is `KD25`'s reading.
fn derived_source_span(
    charge: WorkerMemory,
    parallelism: Parallelism,
    stated: usize,
    chunk_size: usize,
) -> usize {
    let Some(budget) = parallelism.memory_bytes() else { return stated };
    let jobs = parallelism.jobs();
    if charge.plus_per_worker(stated as u64).affords(budget, jobs) >= jobs {
        return stated;
    }
    let room = budget.saturating_sub(charge.at(jobs)) / jobs.max(1) as u64;
    stated.min(usize::try_from(room.max(chunk_size as u64)).unwrap_or(usize::MAX))
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
/// serial [`table_stream`] is this function over one segment per kept run of
/// each matching block, a partitioned replay it over each group
/// [`plan_partitions`] handed out.
///
/// **Under a [`DynamicFilter`]** (`dynamic`), each row group the replay
/// enters is first asked of the filter's state ([`DynamicRead::at_row`]), and
/// the group being read is asked again where the state has moved by the next
/// chunk the replay takes ([`DynamicRead::at_chunk`]): one it rules out ends
/// the segment there, and the replay re-enters the block at the next group it
/// keeps as a piece starting inside the data, the segment's limit unchanged.
/// Under [`RowEvaluation::On`], a row the static filter keeps is evaluated
/// against the state before a column of the row decodes, in every block, and
/// dropped where it rejects it ([`DynamicRead::rejects`]). A block sorted on a
/// bound the state requires stops at its first row past it, as under a static
/// filter's.
fn replay<'a>(
    source: &'a dyn ByteRangeSource,
    plan: Arc<ReplayPlan>,
    segments: Vec<Segment>,
    shared: StreamShared,
    resume: Option<ResumeToken>,
    fingerprint: u64,
    mut dynamic: Option<DynamicRead>,
) -> impl Stream<Item = Result<RecordBatch>> + Send + 'a {
    try_stream! {
        let scan_options = &plan.scan_options;
        let query_options = &plan.query_options;
        let semantics = query_options.semantics;

        // The chunk length every segment's replay repeats, announced once for
        // the whole sub-stream (`ByteRangeSource::hint_read_size`). The budget
        // comes from `QueryOptions`, not `ScanOptions`: a query states the two
        // passes' parallelism separately because they split differently.
        source.hint_read_size(scan_options.chunk_size_bytes);
        source.hint_parallelism(query_options.parallelism);
        announce_cancellation(source, scan_options);
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
        // One buffer for the whole replay: every row of a block has the same
        // width, so after the first it never grows again.
        let mut split = RowSplit::default();

        // Every block holding a stop gets its entry up front, so a drained
        // stream tells one whose stop never fired from one with none planned.
        // A block's segments are adjacent, being in file order.
        {
            let mut early_stops = shared.early_stops.lock().unwrap();
            for segment in &segments {
                let header_offset = segment.block.header_offset;
                if plan.stops.contains_key(&header_offset)
                    && early_stops.last().is_none_or(|last| last.header_offset != header_offset)
                {
                    early_stops.push(EarlyStop { header_offset, unread_bytes: None });
                }
            }
        }

        // A segment a dynamic filter cut short is followed by the rest of it
        // the filter keeps, re-entered ahead of the segments after it.
        let mut queue = VecDeque::from(segments);
        while let Some(segment) = queue.pop_front() {
            let block = &segment.block;
            let seg_start = segment.start;
            let seg_limit = segment.limit;
            let seg_end = block.end_offset;
            let block_database = block.database.clone();
            let stop = plan.stops.get(&block.header_offset);
            if let Some(dynamic) = dynamic.as_mut() {
                dynamic.enter(block, &plan);
            }

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
                        // the map's own copy of it.
                        let (opened, resolved, notes) = activate(
                            block.header.clone(),
                            block.header_offset,
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
                        let skip = dynamic.as_mut().map_or(Skip::Read, |dynamic| {
                            dynamic.at_row(row_start, seg_start, seg_limit, semantics)
                        });
                        match skip {
                            Skip::Read => {}
                            Skip::To(start) => {
                                queue.push_front(segment.rest_from(start));
                                continue;
                            }
                            Skip::Rest => continue,
                        }
                        active = Some(opened);
                        CopyScanner::resume(row_start, Some((block.header_offset, 0)))
                    }
                },
            };
            let mut read_pos = scanner.position();
            let mut carry = ChunkCarry::new();
            let mut chunks = RetainedChunks::new();
            // Set once this segment has read the line that ends at or past
            // its limit — the last it owns, and the one the next segment's
            // search skips, so the two tile — or a row past its block's
            // sorted bound, after which it owns none the filter keeps, or the
            // start of a group a dynamic filter rules out. A later segment of
            // the same block stops at its own first row, where pruning has not
            // already skipped it.
            let mut past_limit = false;
            // Where the row that stopped this segment ends, if one did.
            let mut stopped_at: Option<u64> = None;
            // Where the rest of this segment is searched from, where a
            // dynamic filter ruled out the group its next row starts.
            let mut rest_from: Option<u64> = None;

            loop {
                // The state is read again at each chunk taken, where it has
                // moved, and a group it now rules out ends the segment before
                // the chunk is read (`docs/design/decisions.md`, "D93").
                if let Some(dynamic) = dynamic.as_mut() {
                    match dynamic.at_chunk(scanner.position(), seg_start, seg_limit, semantics) {
                        Skip::Read => {}
                        Skip::To(start) => {
                            rest_from = Some(start);
                            break;
                        }
                        Skip::Rest => break,
                    }
                }
                let want = scan_options.chunk_size_bytes.min((seg_end - read_pos) as usize);
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
                        // Whether the scanner now stands at a row's start
                        // inside the block's data.
                        let at_row = matches!(event, Event::CopyStart(_) | Event::Row(_));
                        match event {
                            Event::CopyStart(start) => {
                                let (opened, resolved, notes) = activate(
                                    start.header,
                                    start.header_offset,
                                    block_database.clone(),
                                    &plan,
                                )?;
                                *shared.comparison_notes.lock().unwrap() = notes;
                                *shared.resolved_schema.lock().unwrap() = resolved;
                                active = Some(opened);
                            }
                            Event::Row(row) => {
                                if let Some((header_offset, _, batcher, filter, _)) =
                                    active.as_mut()
                                {
                                    let unchecked = RawRow::unchecked(row.raw);
                                    let raw = if batcher.decodes_fields()
                                        || filter.reads_fields()
                                        || dynamic.as_ref().is_some_and(DynamicRead::reads_fields)
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
                                    // A kept row a dynamic filter's state
                                    // rejects is dropped before a column of
                                    // it decodes (`DynamicRead::rejects`).
                                    let rejected = keep
                                        && dynamic.as_ref().is_some_and(|dynamic| {
                                            dynamic.rejects(
                                                raw,
                                                &mut split,
                                                batcher.table(),
                                                row.offset,
                                            )
                                        });
                                    // A row a filter keeps makes every term
                                    // of its stop `True`, so each stop is
                                    // asked only of a row its filter
                                    // rejected — the dynamic one's in a
                                    // group it is armed in
                                    // (`DynamicPruning::arm`), and of every
                                    // row there where its rows are not
                                    // evaluated.
                                    let dynamic_stop = dynamic.as_ref().and_then(DynamicRead::stop);
                                    let unevaluated = dynamic
                                        .as_ref()
                                        .is_some_and(|dynamic| !dynamic.evaluates_rows());
                                    let passed = if keep {
                                        (rejected || unevaluated)
                                            && dynamic_stop
                                                .is_some_and(|stop| stop.passed(raw, &mut split))
                                    } else {
                                        stop.is_some_and(|stop| stop.passed(raw, &mut split))
                                            || dynamic_stop
                                                .is_some_and(|stop| stop.passed(raw, &mut split))
                                    };
                                    if passed {
                                        past_limit = true;
                                        stopped_at = Some(scanner.position());
                                    } else if keep && !rejected {
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
                        if past_limit || scanner.position() > seg_limit {
                            past_limit = true;
                            break;
                        }
                        // The next row is this segment's: where it enters a
                        // group a dynamic filter rules out, the segment ends.
                        if at_row && let Some(dynamic) = dynamic.as_mut() {
                            match dynamic.at_row(scanner.position(), seg_start, seg_limit, semantics)
                            {
                                Skip::Read => {}
                                Skip::To(start) => {
                                    rest_from = Some(start);
                                    past_limit = true;
                                    break;
                                }
                                Skip::Rest => {
                                    past_limit = true;
                                    break;
                                }
                            }
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

            if let Some(stopped_at) = stopped_at {
                let owned_end = seg_limit.saturating_add(1).min(block.terminator_offset);
                let unread = owned_end.saturating_sub(stopped_at);
                if unread > 0 {
                    let mut early_stops = shared.early_stops.lock().unwrap();
                    record_unread(&mut early_stops, block.header_offset, unread);
                }
            }
            if let Some(start) = rest_from {
                queue.push_front(segment.rest_from(start));
            }

            // A segment that stopped at its limit rather than at the block's
            // `\.` has no `CopyEnd` to flush it, so it flushes here — and
            // clears the block state either way, the next segment possibly
            // being a different block with a different schema.
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

/// Add `unread` to the entry of `early_stops` for the block at
/// `header_offset`, adding the entry in file order where there is none: a
/// dynamic filter's stop is found as the replay runs, not planned.
fn record_unread(early_stops: &mut Vec<EarlyStop>, header_offset: u64, unread: u64) {
    match early_stops.iter().position(|e| e.header_offset >= header_offset) {
        Some(at) if early_stops[at].header_offset == header_offset => {
            *early_stops[at].unread_bytes.get_or_insert(0) += unread;
        }
        at => early_stops.insert(
            at.unwrap_or(early_stops.len()),
            EarlyStop { header_offset, unread_bytes: Some(unread) },
        ),
    }
}

/// A filter a query's consumer refines while the query runs — a hash join's
/// build side, a TopK's heap — handed to a partitioned replay
/// ([`TablePartitions::under`]), which cuts its sub-streams over the row
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
struct DynamicRead {
    filter: Arc<dyn DynamicFilter>,
    /// Whether each row is evaluated against the state.
    evaluation: RowEvaluation,
    statistics: Arc<BTreeMap<u64, Arc<BlockStatistics>>>,
    /// The cut the sub-stream was handed, whose verdicts a state it read is
    /// read with again rather than asked of each group anew.
    cut: Arc<Cut>,
    /// The block last entered, by `header_offset`, as the filter reads it —
    /// `None` for one whose schema does not resolve.
    block: Option<(u64, Option<DynamicBlock>)>,
    /// The groups ruled out so far ([`TableStream::dynamic_filter_pruned_groups`]).
    pruned: Arc<AtomicU64>,
    /// The rows dropped so far ([`TableStream::dynamic_filter_pruned_rows`]).
    rows: Arc<AtomicU64>,
}

/// One block as a [`DynamicFilter`] reads it: its unprojected schema, which
/// each state is resolved against, the state last read, and its statistics'
/// pruning where they answer.
struct DynamicBlock {
    header_offset: u64,
    full: ResolvedSchema,
    /// How the query reads each column's values its type cannot hold, which
    /// each state's leaves read them by, as the static filter's do.
    unrepresentable: Vec<Option<UnrepresentableRead>>,
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
    pruning: Option<DynamicPruning>,
}

impl DynamicBlock {
    /// `block` of `plan`'s table, pruned by `statistics` where they answer —
    /// or `None` where its schema does not resolve.
    fn new(
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
    fn read(
        &mut self,
        generation: u64,
        state: &Expr,
        semantics: ComparisonSemantics,
        evaluation: RowEvaluation,
    ) {
        let resolved = resolve_loosened(state, &self.full, self.header_offset, semantics, false)
            .reading(&self.unrepresentable);
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
enum Skip {
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
    fn enter(&mut self, block: &CopyBlock, plan: &ReplayPlan) {
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
    fn at_row(
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
    fn at_chunk(
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
    fn stop(&self) -> Option<&SortedStop> {
        self.block.as_ref()?.1.as_ref()?.pruning.as_ref()?.stop()
    }

    /// The state last read, resolved against the block being read as its
    /// rows are evaluated against it — `None` where rows are not evaluated,
    /// before the first read, or where the block's schema does not resolve.
    fn state(&self) -> Option<&ResolvedExpr> {
        self.block.as_ref()?.1.as_ref()?.rows.as_ref()
    }

    /// Whether each row is evaluated against the state.
    fn evaluates_rows(&self) -> bool {
        self.evaluation == RowEvaluation::On
    }

    /// Whether evaluating the state last read reads a field of a row.
    fn reads_fields(&self) -> bool {
        self.state().is_some_and(ResolvedExpr::reads_fields)
    }

    /// Whether the state last read rejects `raw_row`, a row the static filter
    /// kept, which the replay then drops before decoding a column of it,
    /// counting it ([`TableStream::dynamic_filter_pruned_rows`]) — never
    /// where rows are not evaluated ([`RowEvaluation`]).
    ///
    /// **Evaluated in every block**, statistics or not, the state being read
    /// at each chunk as well as at each group entered
    /// (`docs/design/decisions.md`, "D93"). **A field that does not decode
    /// keeps the row**: a dynamic filter only licenses dropping a row, and
    /// the error is raised where evaluation or the row's own decoding
    /// reaches it (`docs/design/decisions.md`, "D54").
    fn rejects(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> bool {
        let Some(state) = self.state() else { return false };
        // Timed whole where the instrument is built in
        // (`crate::instrument::row_evaluated!`).
        row_evaluated!({
            let rejected = matches!(state.matches(raw_row, split, table, row_offset), Ok(false));
            if rejected {
                self.rows.fetch_add(1, Ordering::Relaxed);
            }
            rejected
        })
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
/// and otherwise a row is kept only where the tree is `True`, tested after that
/// row has been fully unescaped. A term referencing a column absent from a
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
        // Taken before the map is read, so a warm query — which reaches no
        // save at all — still has something to compare against when it
        // finishes (`crate::cache::SourceWatch`).
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
struct PlannedReplay {
    plan: Arc<ReplayPlan>,
    /// The blocks cut into sub-streams ([`plan_partitions`]).
    groups: Vec<Vec<Segment>>,
    /// What every sub-stream carries ([`TableStream::plan_notes`]).
    plan_notes: Vec<PlanNote>,
    /// Each block's advice, in file order, as the cut was made by.
    advice: Vec<Partitioning>,
}

/// The plan and the sub-streams' segments for replaying `matches`, shared by
/// [`table_stream_partitions`] and [`TablePartitions::plan`].
fn plan_replay(
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
    // (`docs/design/decisions.md`, "D50"), so writing it back moves no token.
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
/// blocks name different column sets, a projected name no block carries. A
/// table the map holds no block for has the empty schema, as its stream has
/// no rows. **Believe it only over a complete map**: a table's later blocks
/// are part of its census and of its column-set check
/// ([`crate::index::DumpIndex::is_complete`]).
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

/// A partitioned replay of one table over **a complete map the caller holds**,
/// planned once and streamed per partition, for an embedder that schedules
/// partitions itself and runs them in any order, or more than once
/// (`docs/design/decisions.md`, "D90").
///
/// [`table_stream_partitions`] with the mapping pass taken out: no cache is
/// loaded or written and no byte is scanned for structure, so the map is
/// believed exactly as handed over, which is why a map that stops short of
/// the source's end is refused ([`Error::MapIncomplete`]). **The table is
/// named exactly** ([`TableName`]), so nothing is matched and no ambiguity can
/// arise.
///
/// **It owns its source**, so its sub-streams outlive the call that planned
/// them — `'static`, which is what a scheduler holding a plan across tasks
/// needs. `watch` is the caller's: its baseline is what each sub-stream
/// checks the source against when it ends, so a caller opens it before
/// trusting the map it loaded and shares it across every replay of that map.
///
/// **It shares each matched block's statistics with the caller's map**, where
/// the query uses them, for a [`DynamicFilter`] to be read against as its
/// sub-streams run ([`Self::under`]): nothing is copied, so they cost
/// nothing the map does not while the caller holds it, and a plan kept after
/// the map is dropped keeps them.
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
    /// Plan the replay of `table` over `index`: its columns, every block
    /// resolved, statistics consulted, and the blocks cut into as many
    /// partitions as `query_options.parallelism` and the source's own advice
    /// allow — the same plan [`table_stream_partitions`] makes after its
    /// mapping pass.
    pub async fn plan(
        source: Arc<dyn ByteRangeSource>,
        index: &DumpIndex,
        watch: Arc<SourceWatch>,
        table: &TableName,
        scan_options: ScanOptions,
        query_options: QueryOptions,
    ) -> Result<Self> {
        validate_request(&query_options, None, 0)?;
        let size = source.size().await?;
        if !index.is_complete(size) {
            return Err(Error::MapIncomplete { scanned_through: index.scanned_through, size });
        }
        let matches: Vec<CopyBlock> = index.blocks_of(table).cloned().collect();
        let PlannedReplay { plan, groups, plan_notes, advice } = plan_replay(
            source.as_ref(),
            &matches,
            index.metadata.clone(),
            scan_options,
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
            &matches,
            index.metadata.as_ref(),
            &resolved,
            &block_runs(&groups),
            reading,
        );
        let use_statistics = plan.query_options.use_statistics;
        let matches: Vec<CopyBlock> = matches
            .into_iter()
            .map(|block| match use_statistics {
                true => block,
                false => CopyBlock { statistics: None, ..block },
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
            table: table.qualified(),
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
    /// filter's pruning kept ([`crate::statistics::RowGroup::rows`]), plus
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
    /// ([`crate::statistics::ColumnStatistics::value_bytes`]). `None` for a
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
    /// runs the partition, and outside the plan it was cut under
    /// (`docs/design/decisions.md`, "D50"). Each call starts the partition from
    /// its beginning; a partition past [`Self::len`] is an empty stream.
    pub fn stream(&self, partition: usize, max_rows: usize) -> TableStream<'static> {
        self.sub_stream(partition, max_rows, None)
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
        let metadata = self.plan.metadata.as_ref();
        let resolved = self.resolved_schema();
        let reading = self.plan.query_options.statistics_view();
        let proved =
            partition_orders(&self.matches, metadata, &resolved, &block_runs(groups), reading);
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
        let plan = if self.plan.query_options.max_rows == max_rows {
            Arc::clone(&self.plan)
        } else {
            let mut plan = ReplayPlan::clone(&self.plan);
            plan.query_options.max_rows = max_rows;
            Arc::new(plan)
        };
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
                    let block = None;
                    let read =
                        DynamicRead { filter, evaluation, statistics, cut, block, pruned, rows };
                    (groups, Some(read))
                }
            };
            let segments = groups.get(partition).cloned().unwrap_or_default();
            let mut rows = Box::pin(replay(
                source.as_ref(),
                plan,
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

/// The byte cut a [`DynamicPartitions`] made at its first poll, beside what
/// it read to make it.
struct Cut {
    groups: Arc<Vec<Vec<Segment>>>,
    /// The row groups the cut ruled out, until the first sub-stream to take
    /// it counts them.
    ruled_out: AtomicU64,
    /// The generation of the state the cut read, and per block by header
    /// offset the verdict it reached on each group under that state: a
    /// sub-stream reading the same state takes them rather than asking each
    /// group again ([`DynamicPruning::seed`]).
    generation: u64,
    verdicts: BTreeMap<u64, Vec<Option<bool>>>,
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
    use crate::scan::SCAN_CHUNK_DEFAULT_SIZE_BYTES;

    /// **The span is spent before the count is cut, and it stops at the
    /// floor** (`docs/design/decisions.md`, "D84"). Every arm of
    /// [`derived_source_span`], against a plain source's shape — an 8 MiB
    /// per-worker charge with no pool term — the shipped 64 MiB ceiling, and
    /// the shipped 1 MiB read chunk as the floor.
    ///
    /// The exact-inversion arm is the load-bearing one: what comes back must
    /// be the largest span at which the count asked for is still afforded, so
    /// each case asserts the returned span *and* that `worker_count` then
    /// hands out every worker (or, at the floor, how many it could).
    #[test]
    fn a_stated_span_is_spent_down_to_seat_the_readers_and_no_further() {
        let footprint = 8 << 20;
        let charge = WorkerMemory::per_worker(footprint);
        let stated = 64 << 20;
        let chunk = SCAN_CHUNK_DEFAULT_SIZE_BYTES;
        let derived = |parallelism: Parallelism, stated: usize| {
            derived_source_span(charge, parallelism, stated, chunk)
        };
        let seated = |parallelism: Parallelism, span: usize| {
            worker_count(parallelism, charge.plus_per_worker(span as u64))
        };

        // Nothing stated: nothing to solve against.
        assert_eq!(derived(Parallelism::default(), stated), stated);

        // One worker is what the floors deliver whatever the charge, so the
        // stated span is already affordable and is left alone — which is what
        // keeps a serial replay's batches the size the caller asked for.
        let serial = Parallelism::workers(1, 64 << 20);
        assert_eq!(derived(serial, stated), stated);

        // A budget with room to spare leaves the ceiling in place.
        let roomy = Parallelism::workers(2, 1 << 30);
        assert_eq!(derived(roomy, stated), stated);

        // Two readers of 8 MiB inside 64 MiB leave 24 MiB apiece, and 24 MiB
        // is exactly what comes back: one byte more would seat only one.
        let two = Parallelism::workers(2, 64 << 20);
        assert_eq!(derived(two, stated), 24 << 20);
        assert_eq!(seated(two, 24 << 20), 2);
        assert_eq!(seated(two, (24 << 20) + 1), 1);

        // Four readers leave 8 MiB apiece, on the same arithmetic.
        let four = Parallelism::workers(4, 64 << 20);
        assert_eq!(derived(four, stated), 8 << 20);
        assert_eq!(seated(four, 8 << 20), 4);

        // Eight readers leave nothing, so the span stops at the floor and the
        // shortfall is the count's: 9 MiB apiece affords seven of the eight.
        let eight = Parallelism::workers(8, 64 << 20);
        assert_eq!(derived(eight, stated), chunk);
        assert_eq!(seated(eight, chunk), 7);

        // A budget under one reader's own charge cannot go below the floor
        // either — `AllocationBelowFloor` is what names that arrangement.
        let starved = Parallelism::workers(8, 1 << 20);
        assert_eq!(derived(starved, stated), chunk);

        // **It only ever moves down.** A caller stating less than the floor is
        // stating a batch size, and gets it.
        assert_eq!(derived(eight, 4096), 4096);
        assert_eq!(derived(two, chunk), chunk);
    }

    /// **The floor is the chunk this caller announced, not the shipped one**
    /// (`docs/design/decisions.md`, "D84"): the pin is the span rounded out to
    /// what the source retains, and what it retains is what the replay loop
    /// asked it to read in (`crate::scan::ScanOptions::chunk_size_bytes`).
    ///
    /// The shape is `--chunk-size 64k` on a plain source, whose per-worker
    /// footprint is eight of whatever chunk it was told to read
    /// (`crate::io::PLAIN_PARTITION_CHUNKS`), against a budget of exactly what
    /// the readers asked for cost — so the room left for a span is nothing and
    /// the floor is the whole answer. The two floors differ by a factor of
    /// sixteen there, and so does what the budget then seats.
    #[test]
    fn the_floor_is_one_announced_read_chunk_and_a_small_one_buys_readers() {
        let chunk = 64 << 10;
        let unit = 8 * chunk as u64;
        let charge = WorkerMemory::per_worker(unit);
        let stated = 64 << 20;
        // Eight readers of `unit` inside eight units: the count is afforded at
        // no span at all, which is where the floor decides.
        let eight = Parallelism::workers(8, 8 * unit);
        let seated = |span: usize| worker_count(eight, charge.plus_per_worker(span as u64));

        let span = derived_source_span(charge, eight, stated, chunk);
        assert_eq!(span, chunk, "the floor is the announced chunk");
        assert_eq!(seated(span), 7, "which is what the budget then seats");

        // What the shipped constant would have floored at, and what it cost:
        // a span sixteen times the unit the source actually retains, and five
        // of the seven readers declined for bytes no batch pins.
        let shipped = derived_source_span(charge, eight, stated, SCAN_CHUNK_DEFAULT_SIZE_BYTES);
        assert_eq!(shipped, SCAN_CHUNK_DEFAULT_SIZE_BYTES);
        assert_eq!(seated(shipped), 2);

        // **A chunk larger than the shipped one floors above it**, the other
        // direction of the same defect: the span is rounded out to the chunk
        // whatever the chunk says, so narrowing below it buys nothing.
        let big = 4 << 20;
        let charge = WorkerMemory::per_worker(8 * big as u64);
        let eight = Parallelism::workers(8, 8 * 8 * big as u64);
        assert_eq!(derived_source_span(charge, eight, stated, big), big);
    }

    /// **The count note names a lever a plain source actually has** (`KD32`):
    /// a source recommending no per-reader cost keeps
    /// `crate::io::DEFAULT_MEMORY_BUDGET` however large an allowance is
    /// stated (`docs/design/decisions.md`, "D83"), so a remedy offering the
    /// budget alone is inert exactly where this note fires on one. Both arms
    /// are checked, the span-carrying one and the bare, and the second half
    /// is the premise itself — without it the first half is a string
    /// asserting its own wording.
    #[test]
    fn the_count_note_names_a_lever_a_plain_source_has() {
        for span in [Some(1 << 20), None] {
            let note =
                PlanNote::parallelism_budget_limited(8, 2, 8 << 20, span, 64 << 20, false, true);
            let message = note.message();
            assert!(message.contains("a smaller read chunk"), "{message}");
        }

        // What makes the budget clause alone inert: twice the allowance is
        // the same budget, both of them `DEFAULT_MEMORY_BUDGET`.
        let budget = |allowance| Parallelism::within(8, None, allowance).memory_bytes();
        assert_eq!(budget(2 << 30), Some(crate::io::DEFAULT_MEMORY_BUDGET));
        assert_eq!(budget(4 << 30), budget(2 << 30));
    }

    /// **[`PlanNote::budget_bytes`] answers for exactly the notes whose
    /// sentence quotes a budget**, which is what a caller appends its
    /// provenance clause to (`docs/design/decisions.md`, "D64"). Asserted
    /// against the message rather than against a second list of kinds: a note
    /// added later that names a budget and forgets the accessor fails here,
    /// where a list would agree with itself.
    #[test]
    fn a_note_quoting_a_budget_is_the_one_that_reports_it() {
        let memory_bytes = 64 << 20;
        let notes = [
            PlanNote::compressed_block_path_declined(9, 1 << 20, 2 << 20, memory_bytes, true),
            PlanNote::allocation_below_floor(8 << 20, memory_bytes, false, true),
            PlanNote::batch_span_narrowed(64 << 20, 1 << 20, 7, memory_bytes, false, true),
            PlanNote::parallelism_budget_limited(
                8,
                7,
                8 << 20,
                Some(1 << 20),
                memory_bytes,
                false,
                true,
            ),
            PlanNote {
                kind: PlanNoteKind::StatisticsPruned {
                    skipped_groups: 1,
                    groups: 2,
                    skipped_bytes: 3,
                    bytes: 4,
                },
                levers: Vec::new(),
            },
        ];
        for note in notes {
            let quoted =
                note.message().contains(&format!("memory budget of {memory_bytes} byte(s)"));
            assert_eq!(note.budget_bytes().is_some(), quoted, "{}", note.message());
            assert!(note.budget_bytes().is_none_or(|bytes| bytes == memory_bytes));
        }
    }

    /// **A plan note lists only the levers that move it, for this source and
    /// this budget**, and `AllocationBelowFloor`'s sentence offers the chunk
    /// exactly where its levers list one. Over a plain file, whose budget a
    /// larger allowance raises only below `DEFAULT_MEMORY_BUDGET` and whose
    /// reader the chunk sizes, the floor note has three arrangements: a zero
    /// budget moved by memory alone, one between zero and a reader by memory
    /// or the chunk, and one at that cap under a chunk past it by the chunk
    /// alone. At the cap the count note keeps only the chunk, and the span
    /// note the chunk and fewer sub-streams — a smaller chunk being a cheaper
    /// reader, which leaves each seated sub-stream a wider batch.
    #[test]
    fn a_plan_note_lists_only_the_levers_that_move_it() {
        use crate::io::LocalFileSource;
        use PlanLever::{FewerSubStreams, LargerAllowance, SmallerReadChunk};

        let dir = tempfile::tempdir().unwrap();
        let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
        let block = CopyBlock {
            header: crate::copy::parse_copy_header(b"COPY public.t (a, b) FROM stdin;").unwrap(),
            database: None,
            header_offset: 0,
            data_offset: 33,
            terminator_offset: 42,
            end_offset: 45,
            row_count: 2,
            partition_root: None,
            statistics: None,
            statistics_declined: None,
            array_shapes: Some(Vec::new()),
            unrepresentable: Some(Vec::new()),
        };
        // Nothing is read: the plan prices the chunk the source was last told.
        let planned = |chunk: usize, parallelism, span| {
            source.hint_read_size(chunk);
            let (_, notes, span, _) = plan_partitions(
                &source,
                std::slice::from_ref(&block),
                &BTreeMap::new(),
                parallelism,
                span,
                chunk,
            );
            (notes, span)
        };
        let plan = |chunk, parallelism, span| planned(chunk, parallelism, span).0;
        let floor = |chunk, budget| {
            let notes = plan(chunk, Parallelism::workers(1, budget), None);
            let [note] = notes.as_slice() else { panic!("{notes:?}") };
            assert!(matches!(note.kind, PlanNoteKind::AllocationBelowFloor { .. }), "{note:?}");
            let offers_chunk = note.message().contains("a smaller read chunk");
            assert_eq!(offers_chunk, note.levers.contains(&SmallerReadChunk), "{}", note.message());
            assert!(note.message().contains("a larger memory budget"), "{}", note.message());
            note.levers.clone()
        };
        assert_eq!(floor(1 << 20, 0), vec![LargerAllowance]);
        assert_eq!(floor(1 << 20, 4 << 20), vec![LargerAllowance, SmallerReadChunk]);
        let cap = crate::io::DEFAULT_MEMORY_BUDGET;
        assert_eq!(floor(2 * cap as usize, cap), vec![SmallerReadChunk]);
        // The cap is what an allowance of any size carves for this source.
        assert_eq!(Parallelism::within(1, None, 64 << 30).memory_bytes(), Some(cap));

        let notes = plan(1 << 20, Parallelism::workers(8, cap), Some(64 << 20));
        let levers: Vec<_> = notes.iter().map(|note| (&note.kind, note.levers.clone())).collect();
        assert!(
            matches!(
                levers.as_slice(),
                [
                    (PlanNoteKind::BatchSpanNarrowed { .. }, narrowed),
                    (PlanNoteKind::ParallelismBudgetLimited { .. }, limited),
                ] if narrowed == &[SmallerReadChunk, FewerSubStreams]
                    && limited == &[SmallerReadChunk]
            ),
            "{notes:?}"
        );

        // Four readers at the cap seat every one on a narrowed span, so the
        // span note is alone, and the chunk it offers is what widens it: each
        // reader's charge is a multiple of the chunk, and the span is what
        // the budget leaves over the readers.
        let (notes, narrowed) = planned(1 << 20, Parallelism::workers(4, cap), Some(64 << 20));
        let [note] = notes.as_slice() else { panic!("{notes:?}") };
        assert!(matches!(note.kind, PlanNoteKind::BatchSpanNarrowed { .. }), "{note:?}");
        assert_eq!(note.levers, [SmallerReadChunk, FewerSubStreams]);
        assert!(note.message().contains("a smaller read chunk"), "{}", note.message());
        let (_, widened) = planned(256 << 10, Parallelism::workers(4, cap), Some(64 << 20));
        assert!(widened > narrowed, "{widened:?} against {narrowed:?}");
    }

    /// **A note is a `Warning` exactly where the budget declined something
    /// asked of it**: a skip and a narrowed span are `Info`, as `pgdt` has
    /// always printed them `note:`. A sink recovers the note itself.
    #[test]
    fn a_plan_note_warns_where_the_budget_declined_what_was_asked() {
        let memory_bytes = 64 << 20;
        let cases = [
            (
                PlanNote::compressed_block_path_declined(9, 1, 2, memory_bytes, true),
                Severity::Warning,
            ),
            (
                PlanNote::allocation_below_floor(8 << 20, memory_bytes, false, true),
                Severity::Warning,
            ),
            (
                PlanNote::parallelism_budget_limited(8, 7, 1, None, memory_bytes, false, true),
                Severity::Warning,
            ),
            (
                PlanNote::batch_span_narrowed(64 << 20, 1 << 20, 7, memory_bytes, false, true),
                Severity::Info,
            ),
            (
                PlanNote {
                    kind: PlanNoteKind::StatisticsPruned {
                        skipped_groups: 1,
                        groups: 2,
                        skipped_bytes: 3,
                        bytes: 4,
                    },
                    levers: Vec::new(),
                },
                Severity::Info,
            ),
        ];
        for (note, severity) in cases {
            let finding: &dyn Finding = &note;
            assert_eq!(finding.severity(), severity, "{}", finding.message());
            assert_eq!(finding.as_any().downcast_ref::<PlanNote>(), Some(&note));
        }
    }

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
    /// of panicking. Unreachable in practice — it takes a save costing a
    /// twentieth of `Duration::MAX` — but the multiplication is on the hot
    /// path of every block.
    #[test]
    fn an_absurd_save_cost_saturates_rather_than_panicking() {
        assert!(!SaveThrottle::due_after(Duration::MAX / 2, Duration::MAX));
    }

    /// **The guard no caller can trip, pinned so it stays that way.**
    /// [`resolve_block`] refuses `Typed` resolution against metadata with no
    /// complete entry for the block's database, which every call site
    /// satisfies by construction — so `Error::MetadataNotScanned` is
    /// unreachable but through a hand-built `DumpIndex`, and this is what stands
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

        let err = resolve_block(
            &header,
            Some(&metadata),
            Some("second"),
            SchemaMode::Typed,
            &[],
            &[],
            ComparisonSemantics::Postgres,
        )
        .expect_err("the metadata has no entry for `second`");
        assert!(
            matches!(&err, Error::MetadataNotScanned { database } if database.as_deref() == Some("second")),
            "{err:?}"
        );

        // The same block in a database the metadata covers resolves, so the
        // refusal is about coverage and not about the lookup failing.
        assert!(
            resolve_block(
                &header,
                Some(&metadata),
                Some("first"),
                SchemaMode::Typed,
                &[],
                &[],
                ComparisonSemantics::Postgres
            )
            .is_ok()
        );

        // `Strings` never looks, so it is never refused.
        assert!(
            resolve_block(
                &header,
                Some(&metadata),
                Some("second"),
                SchemaMode::Strings,
                &[],
                &[],
                ComparisonSemantics::Postgres
            )
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
            statistics_declined: None,
            array_shapes: Some(Vec::new()),
            unrepresentable: Some(Vec::new()),
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

        // One piece carrying most of the bytes: two groups, two pieces each.
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
            statistics_declined: None,
            array_shapes: Some(Vec::new()),
            unrepresentable: Some(Vec::new()),
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

    /// One table in two blocks naming its columns in different orders.
    fn two_blocks(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("two_blocks.sql");
        std::fs::write(
            &path,
            "COPY public.t (a, b) FROM stdin;\n1\tx\n2\t\\N\n\\.\n\n\
             COPY public.t (b, a) FROM stdin;\ny\t3\n\\.\n",
        )
        .unwrap();
        path
    }

    /// **A replay planned for `table_stream` or `table_stream_partitions`
    /// keeps no block's statistics**: once its plan and segments
    /// exist, the matched blocks the map handed over are the only holders of
    /// each block's `Arc`, so dropping them frees the statistics before a row
    /// is read, on the partitioned path and the serial one alike.
    #[tokio::test]
    async fn a_replay_holds_no_block_s_statistics() {
        use crate::io::LocalFileSource;
        use crate::statistics::{BlockStatistics, GroupSizing};

        let dir = tempfile::tempdir().unwrap();
        let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
        let query_options = QueryOptions {
            scan_extent: ScanExtent::Full,
            parallelism: Parallelism::workers(4, crate::io::DEFAULT_MEMORY_BUDGET),
            ..QueryOptions::default()
        };
        let mapped = map_for_query(
            &source,
            "public.t",
            &ScanOptions::default(),
            &query_options,
            &CacheMode::DISABLED,
            &SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap(),
        )
        .await
        .unwrap();
        let mut matches = mapped.matches;
        assert_eq!(matches.len(), 2);
        for block in &mut matches {
            block.statistics = Some(Arc::new(BlockStatistics {
                group_size: 1 << 20,
                sizing: GroupSizing::Stated,
                groups: Vec::new(),
                columns: vec![None; block.header.columns.len()],
            }));
        }
        let held: Vec<_> =
            matches.iter().map(|block| Arc::clone(block.statistics.as_ref().unwrap())).collect();

        let PlannedReplay { plan, groups, .. } =
            plan_replay(&source, &matches, mapped.metadata, ScanOptions::default(), query_options)
                .unwrap();
        let serial: Vec<Segment> = matches.iter().flat_map(|block| plan.segments(block)).collect();
        assert!(groups.iter().flatten().chain(&serial).all(|s| s.block.statistics.is_none()));
        drop(matches);
        for statistics in &held {
            assert_eq!(Arc::strong_count(statistics), 1, "only the test's own copy remains");
        }
    }

    /// **A dynamic filter's stop is armed only in a group a row of which can
    /// pass it**: under `id < 150` over ascending ids, of the groups the
    /// state keeps only the one whose maximum reaches the bound is asked
    /// row by row, so the groups before it pay nothing for the stop.
    #[tokio::test]
    async fn a_dynamic_stop_is_armed_only_where_a_row_can_pass_it() {
        use crate::io::LocalFileSource;
        use crate::predicate::Predicate;
        use crate::statistics::{StatisticsRequest, StatisticsSelection};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ascending.sql");
        let rows: String = (1..=200).map(|id| format!("{id}\n")).collect();
        std::fs::write(
            &path,
            format!(
                "CREATE TABLE public.t (\n    id integer\n);\n\n\
                 COPY public.t (id) FROM stdin;\n{rows}\\.\n"
            ),
        )
        .unwrap();
        let source = LocalFileSource::open(&path).unwrap();
        let request = StatisticsRequest {
            selection: StatisticsSelection::DATA,
            group_size: Some(std::num::NonZeroU64::new(64).unwrap()),
            ..StatisticsRequest::DATA
        };
        let run = map_file(&source, &ScanOptions::default(), &CacheMode::DISABLED, &request)
            .await
            .unwrap();
        let block = run.index.blocks_for("public.t").next().unwrap();
        let statistics = Arc::clone(block.statistics.as_ref().unwrap());
        let metadata = run.index.metadata.as_ref();
        let census = vec![ArrayShape::default(); block.header.columns.len()];
        let full = resolve_block(
            &block.header,
            metadata,
            None,
            SchemaMode::Typed,
            &census,
            &[],
            ComparisonSemantics::Postgres,
        )
        .unwrap();
        let below = Expr::all([Predicate {
            column: "id".into(),
            op: PredicateOp::Lt,
            value: Some("150".into()),
        }]);
        let filter =
            resolve_expr(&below, &full, block.header_offset, ComparisonSemantics::Postgres)
                .unwrap();
        let reading = crate::statistics::StatisticsView::Every;
        let mut pruning =
            DynamicPruning::new(block, Arc::clone(&statistics), metadata, reading).unwrap();
        pruning.read(Arc::new(filter));

        let bounds = statistics.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
        let (mut armed, mut reaching) = (Vec::new(), Vec::new());
        for group in 0..pruning.groups() {
            if !pruning.keeps(group) {
                continue;
            }
            pruning.arm(group);
            armed.push(pruning.stop().is_some());
            let max: i64 = bounds.groups[group].as_ref().unwrap().max.parse().unwrap();
            reaching.push(max >= 150);
        }
        assert!(armed.len() > 3, "{armed:?}");
        assert_eq!(armed, reaching);
        assert_eq!(armed.iter().filter(|&&armed| armed).count(), 1, "{armed:?}");
    }

    /// **A block prunes in the view its query reads its statistics in**: over
    /// a `date` column, one value a group, a value past the format spec is
    /// its rank in every value's view and a NULL in the representable one, and
    /// one past the calendar is a NULL in the displayable one too — so a
    /// bound rules each out, or `IS NULL` keeps it, by the view — and a column
    /// sorted but for an infinity is sorted, and stops a read, only where the
    /// infinity is a NULL.
    #[tokio::test]
    async fn a_block_prunes_in_the_view_its_query_reads() {
        use crate::io::LocalFileSource;
        use crate::predicate::Predicate;
        use crate::statistics::{StatisticsRequest, StatisticsView};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dates.sql");
        std::fs::write(
            &path,
            "CREATE TABLE public.t (\n    d date\n);\n\n\
             CREATE TABLE public.u (\n    d date\n);\n\n\
             COPY public.t (d) FROM stdin;\n\
             2024-01-01\ninfinity\n-infinity\n262143-01-01\n\\N\n\\.\n\n\
             COPY public.u (d) FROM stdin;\n\
             2020-01-01\n2021-01-01\n2022-01-01\n-infinity\n\\.\n",
        )
        .unwrap();
        let source = LocalFileSource::open(&path).unwrap();
        // One row a group: a row's first byte is all a group holds.
        let request = StatisticsRequest {
            group_size: Some(std::num::NonZeroU64::new(1).unwrap()),
            ..StatisticsRequest::DATA
        };
        let run = map_file(&source, &ScanOptions::default(), &CacheMode::DISABLED, &request)
            .await
            .unwrap();
        let metadata = run.index.metadata.as_ref();
        let filter = |table: &str, op: PredicateOp, value: Option<&str>| {
            let block = run.index.blocks_for(table).next().unwrap();
            let census = vec![ArrayShape::default(); block.header.columns.len()];
            let full = resolve_block(
                &block.header,
                metadata,
                None,
                SchemaMode::Typed,
                &census,
                &[],
                ComparisonSemantics::Postgres,
            )
            .unwrap();
            let term = Predicate { column: "d".into(), op, value: value.map(str::to_string) };
            let expr = Expr::all([term]);
            (
                block,
                resolve_expr(&expr, &full, block.header_offset, ComparisonSemantics::Postgres)
                    .unwrap(),
            )
        };
        // The values each view's pruning keeps a group of, in file order.
        let kept = |table: &str, op: PredicateOp, value: Option<&str>, view: StatisticsView| {
            let (block, filter) = filter(table, op, value);
            let pruning = prune_block(block, &filter, metadata, view).unwrap();
            let statistics = block.statistics.as_ref().unwrap();
            let values = ["2024-01-01", "infinity", "-infinity", "262143-01-01", "NULL"];
            let holding: Vec<usize> =
                (0..statistics.groups.len()).filter(|&g| statistics.groups[g].rows > 0).collect();
            assert_eq!(holding.len(), values.len(), "one row a group");
            holding
                .iter()
                .zip(values)
                .filter(|&(&g, _)| pruning.kept_groups[g])
                .map(|(_, value)| value)
                .collect::<Vec<_>>()
        };
        use StatisticsView::{Displayable, Every, Representable};
        let later = |view| kept("public.t", PredicateOp::Gt, Some("2030-01-01"), view);
        assert_eq!(later(Every), ["infinity", "262143-01-01"]);
        assert_eq!(later(Representable), ["262143-01-01"]);
        assert_eq!(later(Displayable), Vec::<&str>::new());
        let null = |view| kept("public.t", PredicateOp::IsNull, None, view);
        assert_eq!(null(Every), ["NULL"]);
        assert_eq!(null(Representable), ["infinity", "-infinity", "NULL"]);
        assert_eq!(null(Displayable), ["infinity", "-infinity", "262143-01-01", "NULL"]);

        let stops = |view| {
            let (block, filter) = filter("public.u", PredicateOp::Lt, Some("2021-06-01"));
            prune_block(block, &filter, metadata, view).unwrap().stop.is_some()
        };
        assert!(!stops(Every), "-infinity closes the column out of order");
        assert!(stops(Representable) && stops(Displayable));
    }

    /// **The plan resolves every block before any is read, and activation
    /// takes that resolution rather than making its own.** A block's filter
    /// or projection refusal refuses the plan.
    #[tokio::test]
    async fn the_filter_is_resolved_once_per_block_at_plan_time() {
        use crate::io::LocalFileSource;
        use crate::predicate::Predicate;

        let dir = tempfile::tempdir().unwrap();
        let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
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
            &CacheMode::DISABLED,
            &SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap(),
        )
        .await
        .unwrap();
        let [first, second] = &mapped.matches[..] else {
            panic!("two blocks: {:?}", mapped.matches)
        };
        let plan_for = |query_options: &QueryOptions| {
            ReplayPlan::new(
                ScanOptions::default(),
                query_options.clone(),
                &mapped.matches,
                mapped.metadata.clone(),
            )
        };

        // A filter no block can answer refuses the plan, naming the first.
        let refusing = QueryOptions { filter: not_null("c"), ..query_options.clone() };
        assert!(matches!(
            plan_for(&refusing).map(|_| ()),
            Err(Error::UnknownPredicateColumn { header_offset, .. })
                if header_offset == first.header_offset
        ));

        // So does a projection naming a column no block carries.
        let refusing = QueryOptions { projection: Some(vec!["c".into()]), ..query_options.clone() };
        assert!(matches!(
            plan_for(&refusing).map(|_| ()),
            Err(Error::UnknownProjectionColumn { header_offset, .. })
                if header_offset == first.header_offset
        ));

        let plan = plan_for(&query_options).unwrap();
        assert_eq!(
            plan.blocks.keys().copied().collect::<Vec<_>>(),
            [first.header_offset, second.header_offset]
        );

        // The planned entry is what resolving the block where it is reached
        // would have produced.
        let live = |block: &CopyBlock| {
            resolve_for_query(
                &block.header,
                block.header_offset,
                block.database.as_deref(),
                &query_options,
                mapped.metadata.as_ref(),
                &plan.table,
                block.unrepresentable.as_deref(),
            )
        };
        let planned = &plan.blocks[&first.header_offset];
        assert_eq!(format!("{planned:?}"), format!("{:?}", live(first).unwrap()));

        // The second block's `a` is its second field, and it is `a` that
        // feeds the projection.
        assert_eq!(plan.blocks[&second.header_offset].field_targets, [None, Some(0)]);

        // Activating the planned block hands out the plan's own tree.
        let open = |header: &CopyHeader, block: &CopyBlock| {
            activate(header.clone(), block.header_offset, block.database.clone(), &plan)
        };
        let ((_, _, _, filter, _), resolved, notes) = open(&first.header, first).unwrap();
        assert!(Arc::ptr_eq(&filter, &planned.filter));
        assert_eq!((resolved, notes), (planned.resolved.clone(), planned.notes.clone()));

        // An activation the entry does not describe resolves for itself.
        let ((_, _, _, other, _), _, _) =
            open(&second.header, first).unwrap_or_else(|e| panic!("{e}"));
        assert!(!Arc::ptr_eq(&other, &planned.filter));
    }
}
