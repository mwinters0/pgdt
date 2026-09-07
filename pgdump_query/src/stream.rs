//! Pull-mode streaming API (`docs/design/architecture.md`, "Execution model and API surface").
//!
//! [`table_stream`] is the primitive: an async `Stream<Item =
//! Result<RecordBatch>>` built directly on [`CopyScanner`]/[`RowBatcher`], the
//! same machinery [`crate::batch::read_table`] (push mode) now drives
//! internally rather than duplicating. [`ResumeToken`] lets a caller stop
//! consuming partway through and pick back up later in the same process — it
//! holds no public fields (`docs/design/architecture.md` is explicit that it must
//! stay opaque), so its representation is free to change without an API break.
//!
//! **Mapping and streaming are separate passes**
//! (`docs/design/architecture.md`, the section of that
//! name). A query runs in two phases, never interleaved:
//!
//! 1. [`map_forward`] extends the [`DumpIndex`]'s map from its own
//!    `scanned_through` — recording every `COPY` block it passes and
//!    classifying the DDL between them through a [`crate::map::Builder`] —
//!    and **yields nothing**. It stops as soon as the queried table is
//!    settled ([`ScanExtent::UntilTargetSettled`]), which is what keeps a
//!    query against an early table in a huge dump from costing a full scan.
//! 2. Every block the map holds for that table is then replayed for its
//!    rows, in file order.
//!
//! The queried block's bytes are therefore read twice — once to find its
//! extent, once to emit its rows — and that is the price of the split. What
//! it buys: the map is never behind the rows, so a [`ResumeToken`] can only
//! ever point inside already-mapped territory, and a query-built `DumpIndex`
//! tiles the file exactly the way [`crate::index::build_index`]'s does, with
//! no exemption for resumed streams. **The map advances and a
//! [`CacheMode::Enabled`] cache is persisted at the same points**: completed
//! blocks whose save has earned its cost ([`SaveThrottle`]), the block that
//! settles the query, and every exit — so a caller that stops polling keeps
//! what the map learned;
//! [`CacheMode::Disabled`] runs the same way with `save` a no-op, mapping in
//! memory only.
//!
//! **Preamble capture** (`docs/design/architecture.md`, "Bounded
//! preamble-only reads"): before
//! any of that, [`table_stream`] runs [`crate::index::scan_preamble`] once
//! (skipped once a cache already has it), regardless of which table was
//! queried, whether it ever appears, or how far the live scan gets before a
//! caller stops polling. This runs even under [`CacheMode::Disabled`]:
//! `--dqcache none` disables *persistence*, not type resolution
//! (`docs/design/architecture.md`, "Bounded preamble-only reads") — but
//! `cache.save` is a no-op there, so nothing is written to disk. The prepass
//! covers the *first* database; every later `\connect`ed one is stated by
//! [`map_forward`] when it reaches that database's first `COPY` block, which
//! per I1 is the same kind of boundary the prepass stops at. So a cache's
//! metadata covers exactly the databases whose data the scan reached, and one
//! it did not reach has no blocks in the map to ask about.
//!
//! **Type resolution**: once a query's matching
//! `COPY` block is found, its column list is resolved against that captured
//! metadata into a [`crate::resolve::ResolvedSchema`], retrievable via
//! [`TableStream::resolved_schema`]. This is a preview, not what actually
//! decodes a row — see that method's docs and `resolve.rs`'s module docs for
//! why the `RecordBatch`es this stream yields stay all-`Utf8View` regardless.

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
use crate::index::{
    ArrayShape, CopyBlock, DumpIndex, scan_preamble, tiling_diagnostics, toc_coverage_diagnostic,
    union_census,
};
use crate::io::{ByteRangeSource, Parallelism, PartitionBoundaries, Partitioning, WaitPolicy};
use crate::map::{Builder, Span, SpanBody, attach_text};
use crate::preamble::{DumpMetadata, dump_metadata_from_spans};
use crate::predicate::{ComparisonNote, Expr, PredicateOp, ResolvedExpr, resolve_term};
use crate::resolve::{ResolvedSchema, SchemaMode, resolve_columns};
use crate::scan::{ChunkCarry, CopyScanner, Event, Row, ScanOptions};
use crate::{Error, Result};

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, `QueryOptions::filter` resolved against this
/// block's own schema (schemas can differ block-to-block, e.g. a headerless
/// block's placeholder names), and the database this block is attributed to
/// (`docs/design/architecture.md`, "One target per query").
///
/// The [`ResolvedExpr`] mirrors the caller's [`Expr`] and each of its leaves
/// carries the field index it reads — into the block's **unprojected**
/// column list, because that is what the raw row's fields are numbered by,
/// and a term may name a column the projection does not — plus the typed
/// comparison it makes.
type Active = (u64, CopyHeader, RowBatcher, ResolvedExpr, Option<String>);

/// Where `row` sits inside `prefix`, the UTF-8-validated leading part of the
/// span it was scanned out of — `None` when the row runs past it, which is
/// every row of a span whose validation failed and none of a span whose
/// validation held.
///
/// `str::get` rather than an index: it answers `None` for a range that is out
/// of bounds or off a character boundary, so a mis-derived offset costs the
/// row its fast path instead of panicking. Neither can happen here — a row
/// starts just past a line terminator and ends just before one, and both are
/// ASCII.
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
/// resolved. The empty conjunction resolves to an empty conjunction, which
/// is what makes "no filter" need no case of its own on the row path.
///
/// **This is where a predicate is validated against a block**, and the only
/// place: a term naming a column this block does not carry is
/// `Error::UnknownPredicateColumn`, an ordering operator on a column that
/// is not `Mapped` with a `NestedPlan::Scalar` plan is
/// `Error::UnorderedPredicateColumn`, and a literal that is not a value of
/// the column's type — under any comparing operator, `=` included — is
/// `Error::PredicateValueDecode`. All are raised for the first offending
/// term in a left-to-right walk of the tree, before a row of this block
/// flows — so every leaf is validated whatever the evaluator would
/// short-circuit past. A table whose blocks carry different schemas can
/// therefore refuse at the third block after rows from the first two were
/// emitted; that is already true of `UnknownPredicateColumn` and adds no new
/// shape of failure.
///
/// It takes the whole [`ResolvedSchema`] rather than its `schema` because the
/// ordering refusal reads `columns` and `plans` as well — the three are
/// positional and parallel, and splitting them across two lookups is how they
/// would come to disagree.
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
/// each projected column is fed by.
///
/// All five of [`ResolvedSchema`]'s vectors are cut together, in the
/// requested order: they are positional and parallel by construction, and
/// `RecordBatch::try_new` checks the built arrays against `schema` exactly,
/// so a stream advertising the full table while emitting narrow batches
/// would put those two out of agreement
/// (`docs/design/architecture.md`, "Projection").
///
/// Returns the projected schema and `field_targets` — one entry per field of
/// the block, `Some(i)` when that field feeds projected column `i`. `None`
/// projection is every column, in file order.
///
/// Duplicate names are the caller's to reject before the scan starts; this
/// resolves each requested name independently and would silently accept one
/// twice.
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
/// (`docs/design/architecture.md`, "Resume").
///
/// Every field is hashed through an explicit `match` rather than a derived
/// `Hash`, so adding an operator or an option is a compile error here rather
/// than a fingerprint that quietly stops covering it. The hasher's output is
/// not stable across Rust releases, which costs nothing: a token is valid
/// only within the process that produced it.
///
/// **`partition` is what keeps a sub-stream's token from resuming as a whole
/// one.** A token carries an offset and nothing about the range its stream was
/// confined to, so feeding partition *k*'s token to [`table_stream`] would
/// replay every matching row from that offset onward — a superset of what the
/// partition had left, silently. Stamping the partition makes that
/// `Error::ResumeQueryMismatch` instead, which is the whole of the support a
/// partitioned replay offers for resume
/// (`docs/design/architecture.md`, "Partitioned replay").
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
/// by carrying the same terms.
///
/// Two conjunctions that differ only in the order of their terms are
/// semantically the same query and fingerprint differently; that costs a
/// `ResumeQueryMismatch` on a resume nobody would write, and the alternative
/// — canonicalizing the tree — would make the stamp depend on an ordering
/// rule of its own.
///
/// Every variant and every operator is written out rather than derived, so
/// adding one is a compile error here rather than a fingerprint that quietly
/// stops covering it.
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
/// early (`docs/design/architecture.md`, "The file map").
///
/// Whole-region replacement rather than an incremental merge because
/// `Builder`'s output is already a complete tiling of everything the segment
/// walked, so there is nothing to reconcile — only the seam at `seg_start`
/// has to be closed, and it is closed the same way `Builder::push_span`
/// closes every other boundary: **by extending the preceding span to where
/// the next one starts**.
///
/// That is what keeps interstitial blank lines attributed to the span before
/// them (`docs/design/architecture.md`, "Three things close a statement") even across a stopping point. A previous scan
/// that stopped on a block's `end_offset` left that block's span ending
/// exactly there; the blank line that follows belongs to it, not to whatever
/// the next segment happens to recognize first.
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
/// can stop short of EOF. Two things can make a further block share the
/// queried name, and both have to be ruled out.
///
/// **A partition-root marker on a matching block.** Its `COPY` header names
/// the partition's **root**, so other blocks in the same dump carry the same
/// name — and they are *not* adjacent to it, since `TABLE DATA` entries sort
/// by the partition's own name (I2). Only reaching EOF enumerates them.
///
/// **Any `\connect` at all.** The file is then a `pg_dumpall`, a
/// concatenation, or a `--create` dump, and a qualified name can be defined
/// again in a later database — the other route I2 names. Stopping early there
/// would hand back one candidate's rows where
/// `docs/design/architecture.md`'s "One target per query"
/// requires `Error::AmbiguousTable`, which is a wrong answer with no signal,
/// exactly what that decision exists to prevent. A `query_options.database`
/// selector does not lift this: two `\connect` segments can name the *same*
/// database. So any `Connect` span means map the whole file.
///
/// What neither test catches is a file whose *first* segment has no
/// `\connect` — a plain dump with something concatenated after it. Nothing in
/// the prefix announces that.
///
/// Deficiency register: `deficiency: KD6` — the detail is
/// `docs/design/architecture.md`'s "One target per query".
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
/// the same fact to every caller — the loop ran until it had nothing left to
/// do — and only [`map_file`], which never passes a target, has to tell them
/// apart, which it does by construction. Interruption is the one outcome a
/// caller must not mistake for either, because the map is short of the file
/// through no decision of its own.
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
/// cancelled. Emits no rows — see the module docs.
///
/// `target` is the `(table, database selector)` a query may stop early for
/// once [`target_settled`] says so. **`None` means "run to EOF"** — what
/// [`ScanExtent::Full`] asks for, and what [`map_file`] always wants. It is an
/// `Option` rather than a `ScanExtent` beside an ignored table name because a
/// stop rule with no target is not a rule: a sentinel table name would be dead
/// data that any later reader has to prove is unused.
///
/// The [`crate::map::Builder`] is seeded with the segment's start offset (so
/// its first span begins at the frontier rather than at the first non-blank
/// line past it, which would leave the blank lines in between unattributed)
/// and with the database in scope there, which it cannot infer: it never
/// reads the `\connect` lines earlier in the file.
///
/// **Not every completed block is persisted; every *exit* is.** See
/// [`SaveThrottle`] for the rule and why the exits are exempt from it.
///
/// **`index.metadata` is restated at each `\connect`ed database's first
/// `COPY` block**, which per I1 is one of the two boundaries
/// [`dump_metadata_from_spans`] may be called at — and the
/// only one this loop ever stands on, since a `CopyEnd` watermark is not one.
/// It fires when the block's governing database differs from the one the
/// metadata in hand was computed at, so a single-database dump (koji included)
/// pays nothing: the preamble prepass already stood at that same offset.
async fn map_forward(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    index: &mut DumpIndex,
    target: Option<(&str, Option<&str>)>,
    size: u64,
) -> Result<MapStop> {
    if index.scanned_through >= size {
        return Ok(MapStop::Reached);
    }
    if target.is_some_and(|(table, selector)| target_settled(index, table, selector)) {
        return Ok(MapStop::Reached);
    }

    let seg_start = index.scanned_through;
    let prefix: Vec<Span> = index.spans.iter().filter(|s| s.end <= seg_start).cloned().collect();
    // The prefix tiles `[0, seg_start)`, so its last span is the one ending
    // exactly at the frontier and its `database` is the one in scope there.
    // With no prefix at all, the preamble prepass has just run and I1 puts
    // the frontier inside the first database it captured.
    let database = prefix.last().and_then(|s| s.database.clone()).or_else(|| {
        index.metadata.as_ref().and_then(|m| m.databases.first()).and_then(|db| db.name.clone())
    });
    let mut builder = Builder::with_database(database);

    // The database whose first `COPY` block the metadata in hand was computed
    // at — the outer `None` meaning "no metadata at all", the inner one a
    // database with no `\connect` to name it. `dump_metadata_from_spans`
    // finalizes the *last* database it walks, so the last entry is the one
    // whose boundary the computation stood on.
    let mut metadata_covers: Option<Option<String>> =
        index.metadata.as_ref().and_then(|m| m.databases.last()).map(|db| db.name.clone());

    // The chunk length this loop repeats to the frontier, and the budget it
    // may keep buffers inside, announced once
    // (`ByteRangeSource::hint_read_size`, `hint_parallelism`).
    source.hint_read_size(scan_options.chunk_size);
    source.hint_parallelism(scan_options.parallelism);
    // **This loop grants no wait** (`ByteRangeSource::hint_wait_policy`). It
    // builds spans, not batches, so every chunk is consumed and dropped inside
    // the iteration that read it and a wait would be safe — but no shipped
    // loop arms the bound until `16.10.1`'s fused worker needs it
    // (`docs/design/architecture.md`, "Execution model and API surface").
    source.hint_wait_policy(WaitPolicy::NeverWait);
    let mut scanner = CopyScanner::resume(seg_start, None);
    let mut read_pos = seg_start;
    let mut carry = ChunkCarry::new();
    let mut throttle = SaveThrottle::new();
    // Whether the `COPY` block currently open is one `target_settled` would
    // count — see the `CopyEnd` arm, which is the only reader.
    let mut open_block_targets = false;

    loop {
        // Once per chunk, before anything is read: this is the check that
        // gets a scan out of a block big enough that its `CopyEnd` is an hour
        // away (`ScanOptions::cancel`); the `CopyEnd` arm carries the other
        // one, for the file whose whole scan fits in two chunks. `index` is
        // consistent at the last *spliced* watermark whatever the buffer holds
        // — nothing between splices touches it — so the save needs no snapshot
        // logic of its own, and it is unconditional: everything since that
        // watermark is what an interrupt costs, which the `CopyEnd` arm
        // bounds.
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
        for pass in ChunkCarry::PASSES {
            let (span, span_eof) = carry.span(pass, &chunk, eof);
            while let Some(event) = scanner.next_event(span, span_eof)? {
                match event {
                    Event::CopyStart(start) => {
                        // The start of the current database's first `COPY` block
                        // is the second of the two boundaries
                        // `dump_metadata_from_spans` may be called at (I1), and
                        // the one that *recurs* — once per `\connect`ed database.
                        // Stating the metadata here is what makes a `parse`
                        // interrupted in database 3 typed for the two segments it
                        // finished instead of for database 1 alone, and what makes
                        // a cold query and a warm one type a `pg_dumpall` alike.
                        //
                        // Retreat to a pending TOC comment's own start the way
                        // `crate::index::scan_preamble` does, so the span list
                        // handed over ends where the `Data` span is about to
                        // begin rather than swallowing the comment.
                        let boundary =
                            builder.pending_comment_start().unwrap_or(start.header_offset);
                        // Whether this block can be the one that settles `target`,
                        // read off the header before it moves into the builder —
                        // the `CopyEnd` arm's reason to splice for a block the
                        // throttle would have skipped. **Deliberately the header
                        // alone**, which is a superset: a block whose database the
                        // selector excludes cannot settle the target either, but
                        // repeating that test here would tie the gate's width to
                        // `target_settled`'s body, where a later narrowing there
                        // would silently make the gate too narrow. Over-splicing
                        // costs a clone on a block whose name is the queried one;
                        // under-splicing loses the early stop.
                        open_block_targets =
                            target.is_some_and(|(table, _)| start.header.matches(table));
                        builder.on_copy_start(start);
                        // **Once per database, not once per block.** Recomputing
                        // at every `CopyStart` and leaning on idempotence would
                        // put a third whole-index-sized cost in this loop, beside
                        // the two `docs/design/measurements.md` already prices.
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
                            metadata_covers = Some(db);
                        }
                    }
                    // This pass needs only the block's extent, which the
                    // scanner finds from the `\.` terminator — row bytes become
                    // batches in the replay phase. The one thing rows are read
                    // for here is the array-shape census, which every mapping
                    // pass records (`crate::index::CopyBlock::array_shapes`).
                    Event::Row(row) => builder.on_row(row.raw),
                    Event::CopyEnd(end) => {
                        // `end_offset` is always a safe, resumable watermark —
                        // the scanner is back in its `Outside` state there — and
                        // `on_copy_end` leaves the builder `Idle`, which is
                        // exactly where `snapshot` is sound.
                        let watermark = end.end_offset;
                        builder.on_copy_end(end);
                        let targets = std::mem::take(&mut open_block_targets);
                        // The second of the guard's two check points, and the one
                        // that covers the opposite extreme from the chunk check
                        // above: a block-rich file can spend tens of seconds
                        // inside a *single* chunk, where the chunk check runs
                        // twice in the whole scan. The two together bound the
                        // response by the shorter of a chunk and a block.
                        let cancelled = scan_options.cancelled();
                        let due = throttle.due();
                        // **The splice rides the throttle's gate.** Rebuilding
                        // `index.spans` clones the whole list, so doing it per
                        // block is O(blocks²) — the half of that quadratic the
                        // throttle did not reach (`architecture.md`, "`parse`
                        // resumes, and saves as it goes"). Nothing between gate
                        // openings reads `index`: the metadata recompute above
                        // splices its own copy, and `target_settled` is the one
                        // reader that would — which is why a block whose header
                        // could satisfy it opens the gate too. What this costs is
                        // the interrupt's promise, bounded in *time* by the
                        // throttle rather than in blocks.
                        if targets || cancelled || due {
                            index.spans = splice(
                                &prefix,
                                builder.snapshot(watermark),
                                seg_start,
                                watermark,
                                size,
                            );
                            index.roles.extend(builder.roles().iter().cloned());
                            index.tablespaces.extend(builder.tablespaces().iter().cloned());
                            index.scanned_through = index.scanned_through.max(watermark);
                        }
                        // Only a block `target_settled` counts can turn it from
                        // false to true, and `index` has just been spliced for
                        // exactly those — so this reads a map that is current
                        // through `watermark` every time it is consulted.
                        let settled = targets
                            && target.is_some_and(|(table, selector)| {
                                target_settled(index, table, selector)
                            });
                        // The save at the *last* watermark before an early stop is
                        // what persists the map for the next query, so a settled
                        // target saves whether or not the throttle would have —
                        // and so does an interrupt.
                        if settled || cancelled || due {
                            throttle.save(cache, source, index).await?;
                        }
                        if settled {
                            return Ok(MapStop::Reached);
                        }
                        if cancelled {
                            return Ok(MapStop::Interrupted);
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
            carry.consumed(pass, &chunk, scanner.take_consumed());
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
    Ok(MapStop::Reached)
}

/// How many times the elapsed scan has to cover the last save's own cost
/// before another save is worth taking. `20` puts the ceiling on save
/// overhead at ~5% of scan time.
const SAVE_THROTTLE_K: u32 = 20;

/// Decides whether a mid-scan cache save has earned its cost
/// (`docs/design/architecture.md`, "`parse` resumes, and saves as it goes").
///
/// Every save serializes the **whole** index, and the index grows with the
/// block count, so saving at every `CopyEnd` is O(blocks²): koji's 74 blocks
/// cost +1.5% wall, while 4000 small blocks cost 44 s against a scan of
/// milliseconds (`docs/design/measurements.md`, "Per-block cache saving").
///
/// **The rule is self-tuning, not an interval**: skip a block's save unless at
/// least [`SAVE_THROTTLE_K`] times the last save's own duration has elapsed
/// since it. That bounds the overhead at roughly `1/K` of scan time in every
/// regime with no constant that has to be right in two of them — a cheap cache
/// saves often, an expensive one saves rarely, and koji (blocks ~45 s apart,
/// saves well under a second) is untouched. *Rejected:* "every N seconds" and
/// "every N bytes"; both pick a number against one dump shape, and the cost
/// tracks block count rather than bytes read.
///
/// **Exits are exempt.** EOF, a settled target and an interrupt all save
/// unconditionally — the whole risk the throttle adds is the window between
/// saves, and those three are where that window would cost something real.
///
/// **The gate also decides when the map is rebuilt.** `stream::splice` is the
/// other half of the same quadratic, and it fires at the openings of this gate
/// rather than at every `CopyEnd` (see [`map_forward`]'s `CopyEnd` arm). The
/// consequence for this type is that its rule now prices two costs at once:
/// with a disabled cache `save` is ~free, so the gate always clears and the
/// map is rebuilt per block exactly as it was — the residual `KD5` names.
struct SaveThrottle {
    last_save: Instant,
    last_cost: Duration,
}

impl SaveThrottle {
    /// Starts due: `last_cost` is zero, so the first block of a segment always
    /// banks. A scan that dies before ever saving would otherwise leave a
    /// resumable frontier it never wrote down.
    fn new() -> Self {
        Self { last_save: Instant::now(), last_cost: Duration::ZERO }
    }

    /// The rule itself, over measured quantities rather than clocks, so it is
    /// testable without one.
    fn due_after(elapsed: Duration, last_cost: Duration) -> bool {
        elapsed >= last_cost.saturating_mul(SAVE_THROTTLE_K)
    }

    fn due(&self) -> bool {
        Self::due_after(self.last_save.elapsed(), self.last_cost)
    }

    /// Save, and time the save — that duration is the whole input to the next
    /// decision. A disabled cache makes this ~free and so never throttles,
    /// which is right: there is nothing to amortize.
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

/// What one [`map_file`] run did.
///
/// `resumed_from` is the frontier the run *started* at — `0` for a scan that
/// began at byte 0, the cache's `scanned_through` for one that resumed — which
/// is what `pgdq parse` prints about the invocation before the listing that
/// describes the file.
#[derive(Debug)]
pub struct MapRun {
    /// The map as it stands after the run: whole-file when `interrupted` is
    /// false, everything up to the last *spliced* watermark when it is true —
    /// which is the last save, since both ride one gate ([`SaveThrottle`]).
    pub index: DumpIndex,
    /// The frontier this run started from.
    pub resumed_from: u64,
    /// Whether [`ScanOptions::cancel`] stopped the run short of EOF. The
    /// index and the cache agree either way; what differs is whether the
    /// index describes the whole file.
    pub interrupted: bool,
}

/// Map `source` end to end, **continuing from whatever `cache` already
/// holds** — `pgdq parse`'s scan (`docs/design/architecture.md`, "CLI
/// surface").
///
/// This is [`map_forward`] with no stop target, plus the three whole-file
/// facts that only a scan reaching EOF may state. It is a second caller for
/// the incremental loop, not a second implementation of it: `crate::index::build_index`
/// stays the eager, cache-blind producer, and teaching *it* to resume would
/// duplicate the splice-onto-a-prefix logic here with a different set of bugs.
///
/// **It opens with the bounded preamble prepass** [`table_stream`] has always
/// run, under the same "unless the first database's preamble is already
/// complete" guard. Without it an interrupted `parse` leaves a cache with no
/// `DumpMetadata` at all, and `crate::resolve::resolve_columns` turns that
/// into `NotDeclared` for every column of every block — the *final* "the dump
/// never explained this column" answer, where the truth is
/// [`crate::resolve::ColumnResolution::MetadataNotScanned`], "finish the parse
/// and ask again". It costs one read of the preamble rather than two:
/// `scan_preamble` stops at the first `COPY` header and leaves
/// `scanned_through` there, which is exactly where `map_forward` picks up.
///
/// The prepass runs only for a scan starting at byte 0. Its spans *are* the
/// prefix — it produces a tiling of `[0, preamble_end)`, not something to
/// splice onto one — so running it over a resumed map would discard it. A
/// resumed cache carries whatever metadata its own scan stated, and
/// `map_forward`'s per-database recompute repairs one that carries none.
///
/// **The three finishing steps are this function's, not `map_forward`'s.**
///
/// - `metadata` is recomputed over the whole span list. `map_forward` states
///   it at each database's first `COPY` block (I1's recurring boundary), which
///   already covers every database whose data the scan reached; EOF is the
///   other boundary [`dump_metadata_from_spans`] may be
///   called at, and it is what covers a trailing database with no `COPY` block
///   of its own — and a file with no blocks at all, where the recurring
///   boundary is never reached.
/// - `diagnostics` are recomputed rather than inherited: they are
///   `#[serde(skip)]`, so an index that came wholly from the cache carries
///   none, and whatever [`CacheMode::load`] reported about the cache *file*
///   (an mtime mismatch) is kept ahead of them rather than overwritten.
/// - The cache is saved once more at the end. `map_forward` already saved at
///   EOF, but with the pre-EOF metadata; this is the save that persists the
///   finished index, and it is also the only save when the cache already
///   covered the file and nothing was scanned at all.
///
/// **An interrupted run states none of the three**, and returns
/// [`MapRun::interrupted`] rather than an index that would claim to describe
/// the whole file: a span list cut at a `CopyEnd` watermark is not a boundary
/// `dump_metadata_from_spans` may be called at, and a coverage figure computed
/// over a partial map would read as a finished one. `map_forward` has already
/// persisted what it holds by then — including the metadata it stated at the
/// legal boundaries it *did* stand on, which is what makes an interrupted
/// cache typed rather than merely labelled.
pub async fn map_file(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
) -> Result<MapRun> {
    let size = source.size().await?;
    let mut index = match cache.load(source).await? {
        CacheLoad::Index(index) => index,
        // Four reasons to start cold: nothing to resume from, so the map is
        // built from byte 0, and nothing at that path is worth keeping.
        // Spelled out rather than wildcarded (`docs/design/architecture.md`,
        // "The cache").
        CacheLoad::Disabled
        | CacheLoad::Missing
        | CacheLoad::Unreadable
        | CacheLoad::UnsupportedVersion => DumpIndex::default(),
        // The fifth is a refusal, before a byte of the dump is read: this
        // cache describes another file, and the scan would overwrite it at
        // its first throttled save.
        CacheLoad::SourceChanged { cached_stored_size, live_stored_size } => {
            return Err(cache.source_mismatch(cached_stored_size, live_stored_size));
        }
    };
    // The one diagnostic about the cache *file* rather than about the map:
    // everything else the load computed is recomputed below over the finished
    // spans, and `map_forward` assigns `diagnostics` wholesale at EOF anyway.
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
        // Persisted before `map_forward` runs, so an interrupt arriving during
        // the very first chunk still finds banked metadata. `scan_preamble`
        // itself ignores the cancel flag on purpose (`ScanOptions::cancel`):
        // the preamble is an uncancellable region bounded by its own length.
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

    if map_forward(source, scan_options, cache, &mut index, None, size).await?
        == MapStop::Interrupted
    {
        return Ok(MapRun { index, resumed_from, interrupted: true });
    }

    index.metadata = Some(dump_metadata_from_spans(&index.spans));
    let mut diagnostics = carried;
    diagnostics.extend(tiling_diagnostics(&index.spans, size));
    diagnostics.push(toc_coverage_diagnostic(&index.spans));
    index.diagnostics = diagnostics;
    cache.save(source, &index).await?;
    Ok(MapRun { index, resumed_from, interrupted: false })
}

/// Opaque cursor into a [`table_stream`]/[`crate::batch::read_table`]
/// consumption, sufficient to resume from just past the last batch a caller
/// accepted. Valid only within the process that produced it — persisting it
/// across a restart is out of scope (`docs/design/roadmap.md`).
#[derive(Debug, Clone)]
pub struct ResumeToken {
    offset: u64,
    rows_emitted: u64,
    /// Stamp of the query this token came out of — see [`query_fingerprint`].
    /// Resuming a stream whose options hash differently is
    /// `Error::ResumeQueryMismatch`, which is what defends "one schema per
    /// stream, resolved up front" now that a projection can change the
    /// schema without changing the table.
    query_fingerprint: u64,
    /// Reserved for the structural cache's generation stamp. The cache
    /// doesn't stamp generations, so this is always 0; carrying
    /// the field now avoids a later breaking change to this already-opaque
    /// type.
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

    /// This query's resolved schema and diagnostics
    /// (`docs/design/architecture.md`, "Type resolution":
    /// "one schema per stream"). The empty schema (`ResolvedSchema::default`)
    /// until the query's matching `COPY` block has been found — which, for a
    /// table that never appears in the dump, is forever; a caller checking
    /// before consuming any batches only learns that once the whole stream
    /// has been drained.
    pub fn resolved_schema(&self) -> ResolvedSchema {
        self.resolved_schema.lock().unwrap().clone()
    }

    /// The terms of this query whose comparison does not answer what
    /// PostgreSQL's own operator for that column's type would — one
    /// [`ComparisonNote`] per such term, in term order, empty until this
    /// query's matching `COPY` block has resolved (and forever, for a table
    /// that never appears).
    ///
    /// **Per term, because a divergence is operator-conditional.** Most of
    /// them are divergences of *order* alone
    /// (`crate::pgtype::ComparisonDivergence::affects_equality`), so a `text`
    /// column with no `COLLATE` clause earns a note under `<` and none under
    /// `=`.
    ///
    /// A **third channel, and deliberately not a fourth thing to unify**:
    /// `DumpIndex.diagnostics` is L1 and `ResolvedSchema.notes` is L2, while
    /// this signal is per-column *and* conditional on a predicate — L4 — so
    /// writing it into either inverts the layering
    /// (`docs/design/layering.md`). `pgdq query` announces these once on
    /// stderr; what an embedder should be handed instead is filed in
    /// `docs/design/roadmap-P6-embeddable-engine-inbox.md`.
    ///
    /// Like [`Self::resolved_schema`], it describes the **last** block whose
    /// schema resolved: a table whose blocks carry different schemas can
    /// diverge on one block and not on another.
    pub fn comparison_notes(&self) -> Vec<ComparisonNote> {
        self.comparison_notes.lock().unwrap().clone()
    }

    /// Where in the source the batch [`futures::StreamExt::next`] last
    /// returned begins: the offset of its first row, and 0 before anything
    /// has been polled.
    ///
    /// **This is the key a caller merges partitions on.** A `RecordBatch`
    /// carries no position, and the sub-streams of a partitioned replay
    /// ([`table_stream_partitions`]) each run in file order over a contiguous
    /// run of the file — so a caller holding one batch per sub-stream and
    /// always emitting the lowest of these offsets re-assembles the serial
    /// order at N × batch, which is what `pgdq query` does
    /// (`docs/design/architecture.md`, "Partitioned replay").
    ///
    /// It is a *start*, not the end [`Self::resume_token`] reports: the two
    /// order identically here, batches of one replay never overlapping, and a
    /// start is the one that stays a merge key if that ever stops holding.
    /// A batch carrying no rows — reachable only through a `max_rows` of 0,
    /// which makes every row event a flush — reports the scanner's position
    /// instead, so the value is monotone within a sub-stream either way.
    pub fn batch_source_offset(&self) -> u64 {
        *self.batch_offset.lock().unwrap()
    }
}

/// Build the [`ResolvedSchema`] for a table-matching block — the actual
/// batch schema a [`RowBatcher`] built from it carries (see
/// [`TableStream::resolved_schema`]'s docs) — scoped to `database`, the
/// block's own attribution, never a guess
/// (`docs/design/architecture.md`, "One target per query").
///
/// `census` is the union of the array-shape censuses of **every block this
/// stream will replay**, which is what lets a top-level array column commit
/// to the shape the file actually holds rather than to an optimistic
/// `List<T>` (`docs/design/architecture.md`, "The array shape census"). It is
/// a parameter rather than something `resolve_columns` looks up so that all
/// three call sites below — the resumed one included — cannot silently
/// disagree about a stream's schema.
///
/// `Typed` mode against metadata that doesn't (yet) have a *complete* entry
/// for `database` is `Error::MetadataNotScanned` rather than a silent
/// `NotDeclared` degradation.
///
/// **No caller can trip it today**, and that is deliberate rather than
/// accidental: all three call sites are in [`table_stream`], which reads
/// `index.metadata` *after* its mapping pass, and the mapping pass states a
/// database's DDL at that database's first `COPY` block — strictly before any
/// of its blocks can be replayed. The resumed path ([`resume_state`]) shares
/// that same clone, so a [`ResumeToken`] does not reach it either. The check
/// stays because it is what stands between a future reordering — moving that
/// clone back above `map_forward` — and a silently wrongly-typed row, and it
/// is pinned by a unit test rather than left as untested defence.
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

/// Reconstruct the in-progress block state a [`ResumeToken`] captured, if
/// any: the scanner's row counter (so a later `CopyEnd` reports the block's
/// true total, not just rows-since-resume) and a fresh [`RowBatcher`] built
/// from the same schema the original block used.
fn resume_state(
    token: &ResumeToken,
    query_options: &QueryOptions,
    metadata: Option<&DumpMetadata>,
    census: &[ArrayShape],
) -> Result<(CopyScanner, Option<Active>, Option<ResolvedSchema>)> {
    let scanner = CopyScanner::resume(
        token.offset,
        token.in_copy.as_ref().map(|ic| (ic.header_offset, ic.rows_in_block)),
    );
    let mut resolved = None;
    let active = token
        .in_copy
        .as_ref()
        .map(|ic| {
            let full = resolve_block(
                &ic.header,
                ic.field_count,
                metadata,
                ic.database.as_deref(),
                query_options.schema_mode,
                census,
            )?;
            let filter = resolve_expr(&query_options.filter, &full, ic.header_offset)?;
            let (r, field_targets) =
                project(&full, query_options.projection.as_deref(), ic.header_offset)?;
            let batcher = RowBatcher::new(
                &r,
                ic.header.qualified_name(),
                query_options.clone(),
                field_targets,
            );
            resolved = Some(r);
            Ok::<_, Error>((
                ic.header_offset,
                ic.header.clone(),
                batcher,
                filter,
                ic.database.clone(),
            ))
        })
        .transpose()?;
    Ok((scanner, active, resolved))
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
/// (`docs/design/architecture.md`, "Partitioned replay").
///
/// Held behind an `Arc` because `metadata` is the whole dump's DDL and N
/// sub-streams would otherwise each clone it. Nothing in here is mutated
/// after the mapping pass, which is what makes one copy correct for all of
/// them — the census in particular is the union over **every** block the
/// query will replay, so two partitions of one table cannot resolve its
/// arrays differently (`docs/design/architecture.md`, "The array shape
/// census").
struct ReplayPlan {
    scan_options: ScanOptions,
    query_options: QueryOptions,
    metadata: Option<DumpMetadata>,
    census: Vec<ArrayShape>,
}

/// What state a [`Segment`]'s scanner starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentEntry {
    /// Outside a block, at [`Segment::start`] — which is the `COPY` header
    /// line for a whole block, so the header the scanner reads is what
    /// resolves the schema. Also how a [`ResumeToken`] that paused *between*
    /// blocks re-enters.
    Header,
    /// Inside the block's data: reading begins at the first row boundary at
    /// or after [`Segment::start`], and the schema comes from the map's own
    /// copy of the header, no header line being in range.
    Interior,
}

/// One contiguous piece of one `COPY` block that a sub-stream replays.
///
/// **The two offsets are not a byte range, and the difference is what makes
/// the pieces tile.** `start` is where the *search* for this piece's first row
/// begins; the piece's first row is the one starting just past the first LF at
/// or after it. `limit` is not where reading stops either: the piece runs
/// through the line that *ends* at the first LF at or after `limit`, which is
/// exactly the row the next piece's search then skips. So a row that straddles
/// a cut belongs to the piece before it, once, and no cut has to land on a row
/// boundary — which is what lets a source advise cuts (block starts, or
/// anywhere at all) that know nothing about rows
/// (`docs/design/architecture.md`, "Partitioned replay").
#[derive(Debug, Clone)]
struct Segment {
    block: CopyBlock,
    start: u64,
    limit: u64,
    entry: SegmentEntry,
}

impl Segment {
    /// The bytes this piece is responsible for, for balancing sub-streams
    /// against each other. Approximate at both ends by exactly one row, which
    /// is why nothing reads it as an extent.
    ///
    /// Measured from the block's **data**, so the first piece is not charged
    /// for the header line it also covers: the header is not work, and on a
    /// small block charging for it is enough to push the piece after it into
    /// a neighbour's group.
    fn weight(&self) -> u64 {
        self.limit.saturating_sub(self.start.max(self.block.data_offset))
    }
}

/// The four values a [`TableStream`] publishes to its owner while it runs.
#[derive(Clone)]
struct StreamShared {
    position: Arc<Mutex<ResumeToken>>,
    resolved_schema: Arc<Mutex<ResolvedSchema>>,
    comparison_notes: Arc<Mutex<Vec<ComparisonNote>>>,
    batch_offset: Arc<Mutex<u64>>,
}

impl StreamShared {
    fn new(token: ResumeToken) -> Self {
        Self {
            position: Arc::new(Mutex::new(token)),
            resolved_schema: Arc::new(Mutex::new(ResolvedSchema::default())),
            comparison_notes: Arc::new(Mutex::new(Vec::new())),
            batch_offset: Arc::new(Mutex::new(0)),
        }
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
        }
    }
}

/// Resolve one block's schema against `plan`, validate the filter and the
/// projection against it, and build the [`RowBatcher`] that will hold its
/// rows.
///
/// The three places a block becomes active — a `COPY` header the scanner
/// read, the first row of a headerless block, and a partition that started
/// inside a block and took the header off the map — differ only in where the
/// header and the field count come from, so they share this rather than
/// carrying three copies of the same six steps that would drift apart.
fn activate(
    header: CopyHeader,
    header_offset: u64,
    field_count: usize,
    database: Option<String>,
    plan: &ReplayPlan,
) -> Result<(Active, ResolvedSchema, Vec<ComparisonNote>)> {
    let full = resolve_block(
        &header,
        field_count,
        plan.metadata.as_ref(),
        database.as_deref(),
        plan.query_options.schema_mode,
        &plan.census,
    )?;
    // Against the *unprojected* schema: a term's index numbers the raw row's
    // fields, and a term may name a column the projection dropped.
    let filter = resolve_expr(&plan.query_options.filter, &full, header_offset)?;
    let notes = filter.comparison_notes();
    let (resolved, field_targets) =
        project(&full, plan.query_options.projection.as_deref(), header_offset)?;
    let batcher = RowBatcher::new(
        &resolved,
        header.qualified_name(),
        plan.query_options.clone(),
        field_targets,
    );
    Ok(((header_offset, header, batcher, filter, database), resolved, notes))
}

/// The offset of the first row boundary at or after `from`, searching no
/// further than `end` — the byte just past the first LF in `[from, end)`, or
/// `None` when there is none.
///
/// **A partition never resyncs by handing the scanner a mid-row byte.** The
/// scanner in its `InCopy` state treats every line as a row, and the tail of a
/// row can be the two bytes `\.` — a value ending in an escaped backslash, cut
/// between the two — which it would read as the block's terminator. I7's
/// guarantee that `\.` cannot open a data line is about a *line start*, so it
/// covers a scanner started here and does not cover one started mid-row.
///
/// The read is one chunk in the common case and is re-read by the segment's
/// own loop immediately after; on a block-decoding source it lands inside the
/// block that segment was going to decode anyway.
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
/// they fire before a byte is read and regardless of whether the table turns
/// up: a projection naming a column twice is wrong whatever the file holds,
/// and a resume token from another query would otherwise be discovered only
/// once its first block resolved.
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
/// no rows — see the module docs.
///
/// **Both entry points run exactly this**, which is what makes a partitioned
/// replay map the file once rather than once per sub-stream, and what makes
/// the two agree about which blocks a query covers.
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
        // Four reasons to start cold: nothing to resume from, so this
        // query maps from byte 0, and nothing at that path is worth
        // keeping. Spelled out rather than wildcarded
        // (`docs/design/architecture.md`, "The cache").
        CacheLoad::Disabled
        | CacheLoad::Missing
        | CacheLoad::Unreadable
        | CacheLoad::UnsupportedVersion => DumpIndex::default(),
        // The fifth is a refusal, before a byte of the dump is read: this
        // cache describes another file, and this query's own mapping pass
        // would overwrite it.
        CacheLoad::SourceChanged { cached_stored_size, live_stored_size } => {
            return Err(cache.source_mismatch(cached_stored_size, live_stored_size));
        }
    };

    // The first database's preamble always gets captured before anything else
    // runs, regardless of which table this particular call queries or whether
    // it ever reaches the file's first `COPY` block itself
    // (`crate::index::scan_preamble`'s docs) — every `Typed`-mode query needs
    // it for type resolution below, not just a caller that goes on to persist
    // a cache. `CacheMode::Disabled` still runs the scan
    // (`docs/design/architecture.md`, "Bounded preamble-only reads") but
    // `cache.save` below is a no-op for it, so nothing is written. Persisted
    // immediately (not deferred to whenever the mapping pass next saves) so it
    // survives even a caller that polls the stream once and drops it.
    let first_db_preamble_known = index
        .metadata
        .as_ref()
        .and_then(|m| m.databases.first())
        .is_some_and(|db| db.preamble_complete);
    if !first_db_preamble_known {
        // The prepass's spans are kept, not discarded: they tile
        // `[0, preamble_end)`, which is exactly the prefix `map_forward`
        // splices its own output onto. Without them the map would start
        // at the frontier with nothing beneath it and could not tile.
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
    // A cancelled mapping pass is an error here rather than a short
    // stream: the blocks it would replay are only the ones it happened to
    // reach, and a caller that asked for a table's rows would be handed a
    // prefix of them with nothing saying so. `pgdq query` never sets the
    // flag; an embedder that does gets told.
    if map_forward(source, scan_options, cache, &mut index, target, size).await?
        == MapStop::Interrupted
    {
        return Err(Error::ScanCancelled { scanned_through: index.scanned_through });
    }

    // Read *after* the mapping pass, not before it: `map_forward` states
    // the metadata at each `\connect`ed database's first `COPY` block, so
    // a cold query on a `pg_dumpall` types a later database's blocks
    // exactly as a query after `pgdq parse` does. The two passes are still
    // strictly ordered (see the module docs), so the schema depends on the
    // map, never on how far the *row* replay has got.
    let metadata = index.metadata.clone();

    // One target per query (`docs/design/architecture.md`,
    // "One target per query"): narrow the name-only matches down to at
    // most one `(database, qualified name)` candidate before reading any
    // of them, so a would-be silent union across schemas or databases
    // errors instead. `query_options.database`, when given, is the way
    // out of an otherwise-ambiguous bare or cross-database name — it
    // filters candidates first, exactly like a `WHERE` clause narrowing
    // matches rather than picking among them after the fact.
    //
    // Because the map is now complete before any row is emitted, this
    // check runs over every candidate the scan reached rather than
    // incrementally as blocks turn up — so an ambiguous name errors
    // before a single row goes out, not partway through one candidate's.
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

    // **A streamed schema needs no completeness test.** The mapping pass
    // has finished, `matches` is fixed, and every block in it carries a
    // census — so the union below is the evidence for exactly the rows
    // this stream will hand back, on a cold query as much as on a full
    // scan (`docs/design/architecture.md`, "The array shape census").
    let census = union_census(matches.iter());
    Ok(MappedTable { matches, metadata, census })
}

/// How many sub-streams a caller's [`Parallelism`] and a source's per-partition
/// footprint allow between them.
///
/// **Both numbers bind, and the bytes bind on the *stream* count rather than
/// per block**, because the sub-streams are what run at once: capping each
/// block's cut at the memory allowance and then handing out one sub-stream per
/// piece would multiply the allowance by the block count. A source that states
/// no footprint (the declining default, and every source before this method
/// existed) is bounded by `jobs` alone.
fn worker_count(parallelism: Parallelism, partition_bytes: u64) -> usize {
    let jobs = parallelism.jobs();
    match parallelism.memory_bytes() {
        Some(budget) if partition_bytes > 0 => {
            jobs.min(usize::try_from(budget / partition_bytes).unwrap_or(usize::MAX).max(1))
        }
        _ => jobs,
    }
}

/// Cut `range` into at most `want` pieces where `advice` permits, in ascending
/// order and tiling it exactly.
///
/// `Anywhere` is cut evenly, which is the plain file's answer and the only one
/// that can balance exactly. `At(offsets)` is cut at the source's own
/// boundaries — thinned to `want - 1` of them, evenly spaced through the list,
/// when it offers more than the caller can use — because a cut anywhere else
/// makes two readers decode one block twice
/// (`docs/design/architecture.md`, "The compressed source"). An empty `At` is
/// the source declining to be split, and it yields the range whole.
fn cut(range: Range<u64>, advice: &Partitioning, want: usize) -> Vec<Range<u64>> {
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
            // `n` offers describe `n + 1` pieces, and what is being spread
            // evenly is the **pieces**, not the offers: with four offers and
            // three wanted groups the cuts fall after the second and fourth
            // piece, not after the second and third offer. `ceil` is what
            // rounds that the right way, and it keeps the picks strictly
            // increasing (the step is at least one whole piece, since
            // `take <= len`), so they need no dedup pass and collapse to "all
            // of them" when the source offers no more than the caller wants.
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

/// Split `matches` into the pieces `parallelism` and the source between them
/// allow, then group those pieces into sub-streams — each internally in file
/// order, and the groups themselves in file order, so concatenating them is
/// the serial replay (`docs/design/architecture.md`, "Partitioned replay").
///
/// Never empty: a table with no blocks at all is one sub-stream that yields
/// nothing, which is what [`table_stream`] does with the same map.
fn plan_partitions(
    source: &dyn ByteRangeSource,
    matches: &[CopyBlock],
    parallelism: Parallelism,
) -> Vec<Vec<Segment>> {
    // **Announced before the advice is asked for, not when the first
    // sub-stream runs.** A compressed source decides from the stated budget
    // whether it can decode a whole block at all, and that decision is what
    // its answer here is read off (`ByteRangeSource::partitions`) — so asking
    // under the mapping pass's budget would plan against a read path the
    // replay is not going to take. Each sub-stream re-announces the same
    // value, which is idempotent.
    source.hint_parallelism(parallelism);
    // A block's own advice, and its footprint, are read once per block; the
    // footprint that decides the sub-stream count is the largest of them,
    // since one sub-stream may read any of the blocks.
    let advice: Vec<Partitioning> =
        matches.iter().map(|b| source.partitions(b.data_offset..b.end_offset)).collect();
    let footprint = advice.iter().map(Partitioning::partition_bytes).max().unwrap_or(0);
    let workers = worker_count(parallelism, footprint).max(1);

    let mut segments = Vec::new();
    for (block, advice) in matches.iter().zip(&advice) {
        let want = match advice.max_partitions() {
            Some(max) => workers.min(max),
            None => workers,
        };
        // The **data** range is what is cut, not `[header_offset, …)`: a cut
        // inside the header line would give the first piece no rows and the
        // second all of them. The first piece is then extended back over the
        // header, which is where its schema comes from.
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
    }
    distribute(segments, workers)
}

/// Group `segments` into at most `streams` contiguous, byte-balanced runs,
/// dropping the empty ones.
///
/// **Contiguous rather than round-robin**, so a sub-stream reads a run of the
/// file rather than every *n*th piece of it: on a block-decoding source that
/// is what keeps one worker's blocks its own, and it is what makes
/// concatenating the sub-streams in order equal the serial replay.
///
/// The group is chosen from a segment's **midpoint** in the running total, so
/// one huge piece beside many small ones does not push everything after it
/// into the last group.
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
/// batches `plan.query_options` asks for.
///
/// This is the whole of the row path, and there is one of it: a serial
/// [`table_stream`] is this function over one segment per matching block, and
/// a partitioned replay is this function over each group
/// [`plan_partitions`] handed out.
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
        // the whole sub-stream rather than per segment
        // (`ByteRangeSource::hint_read_size`). The budget comes from
        // `QueryOptions`, not `ScanOptions`: the mapping pass has finished, and
        // a query states the two passes' parallelism separately because they
        // split differently. Every sub-stream of a partitioned replay announces
        // the same three values, so the announcements are idempotent whatever
        // order the caller polls them in.
        source.hint_read_size(scan_options.chunk_size);
        source.hint_parallelism(query_options.parallelism);
        // **The replay loop could not grant a wait even if the shipped loops
        // armed the bound**, which is what makes stating it here different
        // from the two mapping ones: `RetainedChunks` pins every chunk a batch
        // has taken a `Utf8View` into until that batch flushes, and the batch
        // then goes to the caller, so this loop holds many buffers at once and
        // can never be the task that frees one it would be waiting on
        // (`ByteRangeSource::hint_wait_policy`). Stated here rather than left
        // to the default, so that whatever the mapping pass granted is
        // un-stated on the same source.
        source.hint_wait_policy(WaitPolicy::NeverWait);

        let mut rows_emitted = resume.as_ref().map_or(0, |t| t.rows_emitted);

        // Only the first segment can start mid-row (a resumed stream paused
        // between two of one block's rows); its scanner and in-flight batcher
        // are prebuilt here so `resume_state`'s logic isn't duplicated below.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) if token.in_copy.is_some() => {
                let (scanner, active, resolved) =
                    resume_state(token, query_options, plan.metadata.as_ref(), &plan.census)?;
                if let Some(r) = resolved {
                    *shared.resolved_schema.lock().unwrap() = r;
                }
                if let Some((_, _, _, filter, _)) = &active {
                    *shared.comparison_notes.lock().unwrap() = filter.comparison_notes();
                }
                (active, Some(scanner))
            }
            _ => (None, None),
        };
        // A matching header with no column list, waiting on its first row to
        // learn the field count. Never non-empty across a resume point: a
        // stream only yields right after a flush, and by then any pending
        // headerless block has already seen its first row (see `active`).
        let mut pending: Option<(CopyHeader, u64, Option<String>)> = None;
        // One buffer for the whole replay: every row of a block has the same
        // width, so after the first it never grows again.
        let mut split = RowSplit::default();

        for segment in segments {
            let block = &segment.block;
            let seg_limit = segment.limit;
            let seg_end = block.end_offset;
            let block_database = block.database.clone();

            let mut scanner = match first_scanner.take() {
                Some(scanner) => scanner,
                None => match segment.entry {
                    SegmentEntry::Header => CopyScanner::resume(segment.start, None),
                    SegmentEntry::Interior => {
                        // Where this piece's first row starts — and whether it
                        // has one at all. A piece whose search lands past its
                        // own limit is a piece two cuts fell inside one row
                        // of: the row belongs to the piece before it, and this
                        // one is empty rather than a duplicate.
                        let Some(row_start) =
                            first_row_start(source, segment.start, seg_end, scan_options).await?
                        else {
                            continue;
                        };
                        if row_start > seg_limit || row_start >= seg_end {
                            continue;
                        }
                        // No header line is in range, so the schema comes off
                        // the map's own copy of it — resolved now when the
                        // header named its columns, and deferred to the first
                        // row when it did not, exactly as the live paths below
                        // do.
                        if block.header.columns.is_empty() {
                            pending = Some((
                                block.header.clone(),
                                block.header_offset,
                                block_database.clone(),
                            ));
                        } else {
                            let (opened, resolved, notes) = activate(
                                block.header.clone(),
                                block.header_offset,
                                block.header.columns.len(),
                                block_database.clone(),
                                &plan,
                            )?;
                            *shared.comparison_notes.lock().unwrap() = notes;
                            *shared.resolved_schema.lock().unwrap() = resolved;
                            active = Some(opened);
                        }
                        CopyScanner::resume(row_start, Some((block.header_offset, 0)))
                    }
                },
            };
            let mut read_pos = scanner.position();
            let mut carry = ChunkCarry::new();
            let mut chunks = RetainedChunks::new();
            // Set once this segment has read the line that ends at or past its
            // limit — the last line it owns. The next segment's search skips
            // exactly that line, so the two tile.
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
                    // which is what turns a row's file offset into a position
                    // inside `validated` below.
                    let span_base = scanner.position();
                    // The span's rows, validated as UTF-8 in one SIMD pass
                    // rather than one `from_utf8` per field. **Taken on the
                    // first row that will decode something**, so a query that
                    // decodes nothing pays nothing
                    // (`docs/design/architecture.md`, "A row's bytes are
                    // validated once, in bulk").
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
                                    // One split per row, shared: the terms
                                    // find the boundaries they read and
                                    // `push_row` finds the rest.
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
                            // A replay segment covers exactly one block, so the
                            // only non-row line in range is the `COPY` header
                            // itself, which arrives as `CopyStart`. Nothing
                            // outside a block — a dollar-quoted region or a
                            // large-object region included — can fall inside one.
                            Event::Line(_) | Event::DollarQuoteEnd(_) => {}
                            Event::LargeObjectStart(_) | Event::LargeObjectEnd(_) => {}
                        }
                        // The line just consumed ended at or past this
                        // segment's limit, so it was the last one this segment
                        // owns. Checked after the event rather than before it,
                        // because the straddling row is *this* segment's.
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
                // its chance to be referenced by a zero-copy view (that happens
                // synchronously above, before we get here), so it's safe to
                // drop. Which chunks that actually releases is
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
            // clears the block state either way, since the next segment may be
            // a different block with a different schema.
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
/// produced; `None` starts from the beginning of `source`. A token whose
/// query fingerprint disagrees with `query_options` is
/// `Error::ResumeQueryMismatch`.
///
/// `query_options.filters` applies `docs/design/architecture.md`'s post-parse
/// row filter (`docs/design/architecture.md`, "Predicates") as a
/// conjunction: an empty list yields every row, and otherwise a row is kept
/// only if **every** term matches, tested after that row has been fully
/// unescaped. A term referencing a column absent from a matching block's own
/// schema is `Error::UnknownPredicateColumn`.
///
/// `query_options.projection` decides which columns are materialized
/// (`docs/design/architecture.md`, "Projection"). It cuts the schema
/// [`TableStream::resolved_schema`] reports as well as the batches, may
/// reorder, and may be empty — a zero-column projection yields batches
/// carrying a row count and nothing else. A filter term may name a column
/// the projection does not.
///
/// `cache` controls structure-cache consulting
/// (`docs/design/architecture.md`, "The cache").
/// `CacheMode::Enabled` persists the map at completed blocks as the mapping
/// pass advances — and always at the block it stops on — so a later query
/// against the same dump starts from a nearer
/// frontier; `CacheMode::Disabled` runs identically but writes nothing,
/// mapping in memory for this call only. Either way rows come from replaying
/// mapped blocks, never from the mapping pass itself — see the module docs.
///
/// `query_options.scan_extent` decides how much of the file the mapping pass
/// walks before any row comes back; see [`ScanExtent`].
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
        // apiece. A resumed stream picks up inside this same list — every
        // resume point is inside a mapped block by construction, so there is
        // no live-scan fallback and no cache bookkeeping left to do here.
        let resume_offset = resume.as_ref().map_or(0, |t| t.offset);
        let segments: Vec<Segment> = mapped
            .matches
            .iter()
            .filter(|b| b.end_offset > resume_offset)
            .map(|b| Segment {
                block: b.clone(),
                start: b.header_offset.max(resume_offset),
                limit: b.end_offset,
                entry: SegmentEntry::Header,
            })
            .collect();
        let plan = Arc::new(ReplayPlan {
            scan_options,
            query_options,
            metadata: mapped.metadata,
            census: mapped.census,
        });
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
/// (`docs/design/architecture.md`, "Partitioned replay").
///
/// The mapping pass runs once, here, before any sub-stream exists; each
/// sub-stream then replays a contiguous run of the blocks that pass settled,
/// cut where the source said it was willing to be cut
/// ([`ByteRangeSource::partitions`]) and no finer than
/// `query_options.parallelism` allows. **The caller runs them**, concurrently
/// or not: running them in order and concatenating is exactly what
/// [`table_stream`] yields, so the serial path is not a second implementation.
///
/// The returned `Vec` is never empty and never holds an empty sub-stream
/// beyond the degenerate one a table with no rows produces.
/// `Parallelism::Serial` is one sub-stream, which is the serial replay.
///
/// **Each sub-stream carries its own schema, notes and position.**
/// [`TableStream::resolved_schema`] is empty on a sub-stream until that
/// sub-stream's first block resolves, so a caller wanting the schema before
/// consuming anything reads it off the *first* sub-stream, whose first segment
/// starts at a `COPY` header. [`TableStream::resume_token`] is stamped with the
/// partition it came from, so feeding one back to [`table_stream`] is
/// `Error::ResumeQueryMismatch` rather than a silent superset of the rows that
/// partition had left — resuming a partitioned replay is not supported.
///
/// **What N sub-streams cost resident is N times one.** Each holds its own
/// read chunks for as long as its in-flight batch pins them
/// (`QueryOptions::max_source_span`), and the source holds
/// `Partitioning::partition_bytes` per concurrent reader on top — which is the
/// number `query_options.parallelism`'s byte half is spent against here.
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
    let groups = plan_partitions(source, &mapped.matches, query_options.parallelism);
    let plan = Arc::new(ReplayPlan {
        scan_options,
        query_options,
        metadata: mapped.metadata,
        census: mapped.census,
    });
    let of = groups.len();
    Ok(groups
        .into_iter()
        .enumerate()
        .map(|(index, segments)| {
            let fingerprint = query_fingerprint(&table, &plan.query_options, Some((index, of)));
            let shared = StreamShared::new(ResumeToken::start(fingerprint));
            let inner =
                replay(source, Arc::clone(&plan), segments, shared.clone(), None, fingerprint);
            shared.into_stream(Box::pin(inner))
        })
        .collect())
}

/// Blocking [`Iterator`] wrapper over a [`TableStream`], for sync callers
/// (the CLI) with no ambient `tokio` runtime — nesting this inside one
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
    /// clock. What it has to get right at the edges: a save that cost nothing
    /// never blocks another (the first save of a segment, and every save under
    /// `CacheMode::Disabled`), and a save that cost something blocks the next
    /// one until the scan has done `K` times that much work.
    #[test]
    fn a_save_is_due_once_the_scan_has_outrun_the_last_ones_cost() {
        let ms = Duration::from_millis;

        assert!(SaveThrottle::due_after(Duration::ZERO, Duration::ZERO), "a free save never waits");
        assert!(SaveThrottle::due_after(ms(1), Duration::ZERO));

        // K = 20: 10ms of saving buys 200ms of silence.
        assert!(!SaveThrottle::due_after(ms(199), ms(10)));
        assert!(SaveThrottle::due_after(ms(200), ms(10)));
        assert!(SaveThrottle::due_after(ms(1000), ms(10)));

        // koji's shape — blocks ~45s apart, saves well under a second — is
        // untouched, which is the regime the measurement said not to change.
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

    /// **The guard no caller can trip any more, pinned so it stays that way.**
    /// [`resolve_block`] refuses `Typed` resolution against metadata that has
    /// no complete entry for the block's database, and every one of its three
    /// call sites now satisfies that by construction: `table_stream` reads
    /// `index.metadata` *after* its mapping pass, and the mapping pass states a
    /// database's DDL at that database's first `COPY` block, strictly before
    /// any of its blocks can be replayed.
    ///
    /// That makes `Error::MetadataNotScanned` unreachable through every public
    /// entry point — and makes this check the thing standing between a future
    /// reordering (moving the clone back above `map_forward`, say) and a
    /// silently wrongly-typed row. An untested guard is one that gets deleted
    /// as dead code, so it is exercised directly here rather than through an
    /// integration test that can no longer construct the state.
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

        // The same block in the database the metadata *does* cover resolves,
        // so the refusal is about coverage and not about the lookup failing.
        assert!(
            resolve_block(&header, 1, Some(&metadata), Some("first"), SchemaMode::Typed, &[])
                .is_ok()
        );

        // And `Strings` never looks, so it is never refused — the documented
        // way out of the error.
        assert!(
            resolve_block(&header, 1, Some(&metadata), Some("second"), SchemaMode::Strings, &[])
                .is_ok()
        );
    }

    /// The two properties every cut has to have, whatever the source advised:
    /// the pieces **tile** the range exactly, in ascending order, and there
    /// are never more of them than the caller asked for. Everything above
    /// this — which rows a piece owns — rests on the tiling.
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
    /// finer, since a piece with no bytes in it can own no rows and would
    /// only cost a sub-stream a resync read.
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

    /// `At` is cut only where the source offered, and thinned evenly when it
    /// offers more boundaries than the caller can use — an offer of exactly
    /// as many as are wanted is taken whole.
    #[test]
    fn an_at_source_is_cut_only_where_it_offered() {
        let advice = Partitioning::at(vec![20, 40, 60, 80], 1 << 20);
        assert_eq!(cut(0..100, &advice, 5), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
        assert_eq!(cut(0..100, &advice, 9), vec![0..20, 20..40, 40..60, 60..80, 80..100]);
        assert_eq!(cut(0..100, &advice, 3), vec![0..40, 40..80, 80..100]);
        assert_eq!(cut(0..100, &advice, 2), vec![0..60, 60..100]);
        // Boundaries outside the range are not cuts, and the range's own ends
        // are not either — `n` offers inside describe `n + 1` pieces.
        assert_eq!(cut(40..80, &advice, 4), vec![40..60, 60..80]);
        for want in [1usize, 2, 3, 4, 5, 9] {
            assert_tiles(0..100, &cut(0..100, &advice, want), want);
        }
    }

    /// A source that declines to be split is not split, however many workers
    /// the caller has — the empty `At` is a policy, not an absence
    /// (`docs/design/architecture.md`, "Execution model and API surface").
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
        assert_eq!(worker_count(eight, 32 << 20), 2);
        assert_eq!(worker_count(eight, 4 << 20), 8);
        // A footprint larger than the whole budget still leaves one worker:
        // the serial path is what a caller with no room for two gets.
        assert_eq!(worker_count(eight, 128 << 20), 1);
        // A source that states no footprint is bounded by `jobs` alone.
        assert_eq!(worker_count(eight, 0), 8);
        // `Serial` states no budget and is one worker, not a pool of one.
        assert_eq!(worker_count(Parallelism::Serial, 32 << 20), 1);
    }

    /// Grouping keeps the pieces in file order and contiguous, which is what
    /// makes concatenating the sub-streams equal the serial replay — and it
    /// balances by bytes, so one long piece beside many short ones does not
    /// land in the same group as all of them.
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
            sparse_index: None,
            column_stats: None,
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

        // Contiguity and order: flattening the groups is the original list.
        for streams in [1usize, 2, 3, 4, 5, 9] {
            let groups = distribute(segments.clone(), streams);
            assert!(groups.len() <= streams.max(1));
            assert!(groups.iter().all(|g| !g.is_empty()));
            let flat: Vec<u64> = groups.iter().flatten().map(|s| s.start).collect();
            assert_eq!(flat, vec![0, 10, 20, 30, 40], "streams {streams}");
        }

        // One piece carrying most of the bytes gets a group of its own rather
        // than dragging its neighbours in with it.
        let lopsided = vec![piece(0, 1), piece(1, 2), piece(2, 1002), piece(1002, 1003)];
        let groups = distribute(lopsided, 2);
        assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2]);
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
    /// of, so it cannot be mistaken for a whole stream's — which is what
    /// turns "resuming a partition is unsupported" into an error rather than
    /// into a silent superset of the rows that partition had left.
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
}
