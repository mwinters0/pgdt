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

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arrow::array::RecordBatch;
use arrow::buffer::Buffer;
use arrow::datatypes::Schema;
use async_stream::try_stream;
use bytes::Bytes;
use futures::Stream;

use crate::batch::{
    QueryOptions, RowBatcher, ScanExtent, SourceChunk, column_names, invalidate_block_cache,
};
use crate::cache::{CacheLoad, CacheMode};
use crate::copy::{CopyHeader, DELIMITER, RawRow, RowSplit, validated_prefix};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::index::{
    ArrayShape, CopyBlock, DumpIndex, scan_preamble, tiling_diagnostics, toc_coverage_diagnostic,
    union_census,
};
use crate::io::ByteRangeSource;
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
/// projection, the filter terms and the schema mode
/// (`docs/design/architecture.md`, "Resume").
///
/// Every field is hashed through an explicit `match` rather than a derived
/// `Hash`, so adding an operator or an option is a compile error here rather
/// than a fingerprint that quietly stops covering it. The hasher's output is
/// not stable across Rust releases, which costs nothing: a token is valid
/// only within the process that produced it.
fn query_fingerprint(table: &str, options: &QueryOptions) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    table.hash(&mut hasher);
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

    // The chunk length this loop repeats to the frontier, announced once
    // (`ByteRangeSource::hint_read_size`).
    source.hint_read_size(scan_options.chunk_size);
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
        // Five reasons, one response today: nothing to resume from, so the map
        // is built from byte 0. Spelled out rather than wildcarded — they are
        // not alike, since what a caller may do to the file at the cache path
        // differs between them (`docs/design/architecture.md`, "The cache").
        CacheLoad::Disabled
        | CacheLoad::Missing
        | CacheLoad::Unreadable
        | CacheLoad::UnsupportedVersion
        | CacheLoad::SourceChanged { .. } => DumpIndex::default(),
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
    let fingerprint = query_fingerprint(&table, &query_options);
    let start_token = resume.clone().unwrap_or_else(|| ResumeToken::start(fingerprint));
    let position = Arc::new(Mutex::new(start_token));
    let position_for_stream = Arc::clone(&position);
    let resolved_schema = Arc::new(Mutex::new(ResolvedSchema::default()));
    let resolved_schema_for_stream = Arc::clone(&resolved_schema);
    let comparison_notes_shared = Arc::new(Mutex::new(Vec::new()));
    let comparison_notes_for_stream = Arc::clone(&comparison_notes_shared);

    let inner = try_stream! {
        // Both checks are on the *request*, so they fire before a byte is
        // read and regardless of whether the table turns up: a projection
        // naming a column twice is wrong whatever the file holds, and a
        // resume token from another query would otherwise be discovered only
        // once its first block resolved.
        if let Some(columns) = query_options.projection.as_deref() {
            for (i, name) in columns.iter().enumerate() {
                if columns[..i].contains(name) {
                    Err(Error::DuplicateProjectionColumn { column: name.clone() })?;
                }
            }
        }
        if resume.as_ref().is_some_and(|t| t.query_fingerprint != fingerprint) {
            Err(Error::ResumeQueryMismatch)?;
        }

        let size = source.size().await?;

        let mut index = match cache.load(source).await? {
            CacheLoad::Index(index) => index,
            // Five reasons, one response today: nothing to resume from, so
            // this query maps from byte 0. Spelled out rather than wildcarded
            // — they are not alike, since what a caller may do to the file at
            // the cache path differs between them
            // (`docs/design/architecture.md`, "The cache").
            CacheLoad::Disabled
            | CacheLoad::Missing
            | CacheLoad::Unreadable
            | CacheLoad::UnsupportedVersion
            | CacheLoad::SourceChanged { .. } => DumpIndex::default(),
        };

        // The first database's preamble always gets captured before
        // anything else runs, regardless of which table this particular
        // call queries or whether it ever reaches the file's first `COPY`
        // block itself (`crate::index::scan_preamble`'s docs) — every
        // `Typed`-mode query needs it for type resolution below, not just a
        // caller that goes on to persist a cache. `CacheMode::Disabled`
        // still runs the scan (`docs/design/architecture.md`, "Bounded
        // preamble-only reads") but `cache.save` below is a no-op for it, so
        // nothing is written.
        // Persisted immediately (not deferred to whenever the mapping pass
        // next saves) so it survives even a caller that polls the stream
        // once and drops it.
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
                scan_preamble(source, &scan_options).await?;
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
            ScanExtent::UntilTargetSettled => Some((table.as_str(), selector)),
            ScanExtent::Full => None,
        };
        // A cancelled mapping pass is an error here rather than a short
        // stream: the blocks it would replay are only the ones it happened to
        // reach, and a caller that asked for a table's rows would be handed a
        // prefix of them with nothing saying so. `pgdq query` never sets the
        // flag; an embedder that does gets told.
        if map_forward(source, &scan_options, &cache, &mut index, target, size).await?
            == MapStop::Interrupted
        {
            Err(Error::ScanCancelled { scanned_through: index.scanned_through })?;
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
            .blocks_for(&table)
            .filter(|b| selector.is_none() || b.database.as_deref() == selector)
            .cloned()
            .collect();
        let mut target: Option<(Option<String>, String)> = None;
        for b in &matches {
            let key = (b.database.clone(), b.header.qualified_name());
            match &target {
                None => target = Some(key),
                Some(t) if *t != key => {
                    Err(Error::AmbiguousTable {
                        name: table.clone(),
                        candidates: vec![render_candidate(t), render_candidate(&key)],
                    })?;
                }
                _ => {}
            }
        }

        // Pass 2: replay each matching block for its rows. A resumed stream
        // picks up inside this same list — every resume point is inside a
        // mapped block by construction, so there is no live-scan fallback and
        // no cache bookkeeping left to do here.
        // **A streamed schema needs no completeness test.** The mapping pass
        // has finished, `matches` is fixed, and every block in it carries a
        // census — so the union below is the evidence for exactly the rows
        // this stream will hand back, on a cold query as much as on a full
        // scan (`docs/design/architecture.md`, "The array shape census").
        let census = union_census(matches.iter());

        let resume_offset = resume.as_ref().map_or(0, |t| t.offset);
        let mut rows_emitted = resume.as_ref().map_or(0, |t| t.rows_emitted);

        // The chunk length every block's replay repeats, announced once for
        // the whole replay rather than per block
        // (`ByteRangeSource::hint_read_size`).
        source.hint_read_size(scan_options.chunk_size);

        // Only the first replayed block can start mid-block (a resumed
        // stream paused between two of its rows); its scanner and in-flight
        // batcher are prebuilt here so `resume_state`'s logic isn't
        // duplicated below.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) if token.in_copy.is_some() => {
                let (scanner, active, resolved) =
                    resume_state(token, &query_options, metadata.as_ref(), &census)?;
                if let Some(r) = resolved {
                    *resolved_schema_for_stream.lock().unwrap() = r;
                }
                if let Some((_, _, _, filter, _)) = &active {
                    *comparison_notes_for_stream.lock().unwrap() = filter.comparison_notes();
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

        for block in matches.iter().filter(|b| b.end_offset > resume_offset) {
            let seg_start = block.header_offset.max(resume_offset);
            let seg_end = block.end_offset;
            let block_database = block.database.clone();

            let mut scanner =
                first_scanner.take().unwrap_or_else(|| CopyScanner::resume(seg_start, None));
            let mut read_pos = seg_start;
            let mut carry = ChunkCarry::new();
            let mut chunks: VecDeque<SourceChunk> = VecDeque::new();

            loop {
                let want = scan_options.chunk_size.min((seg_end - read_pos) as usize);
                let chunk = if want > 0 {
                    let bytes = source.read_range(read_pos, want).await?;
                    chunks.push_back(SourceChunk {
                        start: read_pos,
                        buffer: Buffer::from(bytes.clone()),
                        column_blocks: Vec::new(),
                    });
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
                                    let full = resolve_block(
                                        &start.header,
                                        start.header.columns.len(),
                                        metadata.as_ref(),
                                        block_database.as_deref(),
                                        query_options.schema_mode,
                                        &census,
                                    )?;
                                    // Against the *unprojected* schema: a
                                    // term's index numbers the raw row's
                                    // fields, and a term may name a column the
                                    // projection dropped.
                                    let filter = resolve_expr(
                                        &query_options.filter,
                                        &full,
                                        start.header_offset,
                                    )?;
                                    *comparison_notes_for_stream.lock().unwrap() =
                                        filter.comparison_notes();
                                    let (resolved, field_targets) = project(
                                        &full,
                                        query_options.projection.as_deref(),
                                        start.header_offset,
                                    )?;
                                    let batcher = RowBatcher::new(
                                        &resolved,
                                        start.header.qualified_name(),
                                        query_options.clone(),
                                        field_targets,
                                    );
                                    *resolved_schema_for_stream.lock().unwrap() = resolved;
                                    active = Some((
                                        start.header_offset,
                                        start.header,
                                        batcher,
                                        filter,
                                        block_database.clone(),
                                    ));
                                }
                            }
                            Event::Row(row) => {
                                if let Some((header, header_offset, block_database)) =
                                    pending.take()
                                {
                                    let field_count =
                                        memchr::memchr_iter(DELIMITER, row.raw).count() + 1;
                                    let full = resolve_block(
                                        &header,
                                        field_count,
                                        metadata.as_ref(),
                                        block_database.as_deref(),
                                        query_options.schema_mode,
                                        &census,
                                    )?;
                                    let filter =
                                        resolve_expr(&query_options.filter, &full, header_offset)?;
                                    *comparison_notes_for_stream.lock().unwrap() =
                                        filter.comparison_notes();
                                    let (resolved, field_targets) = project(
                                        &full,
                                        query_options.projection.as_deref(),
                                        header_offset,
                                    )?;
                                    let batcher = RowBatcher::new(
                                        &resolved,
                                        header.qualified_name(),
                                        query_options.clone(),
                                        field_targets,
                                    );
                                    *resolved_schema_for_stream.lock().unwrap() = resolved;
                                    active = Some((
                                        header_offset,
                                        header,
                                        batcher,
                                        filter,
                                        block_database,
                                    ));
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
                                        let batch = batcher.flush()?;
                                        invalidate_block_cache(&mut chunks);
                                        rows_emitted += batch.num_rows() as u64;
                                        *position_for_stream.lock().unwrap() =
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
                                    let batch = batcher.flush()?;
                                    invalidate_block_cache(&mut chunks);
                                    rows_emitted += batch.num_rows() as u64;
                                    *position_for_stream.lock().unwrap() =
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
                    }
                    carry.consumed(pass, &chunk, scanner.take_consumed());
                }

                // Everything before the scanner's new position has already had
                // its chance to be referenced by a zero-copy view (that happens
                // synchronously above, before we get here), so it's safe to drop.
                //
                // **The chunk the scanner just read is retained even so**, as
                // it always was: `end() <= floor` holds only once the scanner
                // has walked past a chunk's last byte, and the row that
                // straddles the next boundary is carried, not scanned, so it
                // cannot reach here needing a chunk that has gone.
                let floor = scanner.position();
                while chunks.front().is_some_and(|c| c.end() <= floor) {
                    chunks.pop_front();
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
        }
    };

    TableStream {
        inner: Box::pin(inner),
        position,
        resolved_schema,
        comparison_notes: comparison_notes_shared,
    }
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
}
