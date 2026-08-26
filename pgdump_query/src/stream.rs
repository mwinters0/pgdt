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
//! no exemption for resumed streams. A [`CacheMode::Enabled`] cache is
//! persisted after every completed block, so a caller that stops polling
//! keeps what the map learned; [`CacheMode::Disabled`] runs the same way with
//! `save` a no-op, mapping in memory only.
//!
//! **Preamble capture** (`docs/design/architecture.md`, "Bounded
//! preamble-only reads"): before
//! any of that, [`table_stream`] runs [`crate::index::scan_preamble`] once
//! (skipped once a cache already has it), regardless of which table was
//! queried, whether it ever appears, or how far the live scan gets before a
//! caller stops polling. This runs even under [`CacheMode::Disabled`]:
//! `--dqcache none` disables *persistence*, not type resolution
//! (`docs/design/architecture.md`, "Bounded preamble-only reads") — but
//! `cache.save` is a no-op there, so nothing is written to disk. Under [`CacheMode::Enabled`], a cache file's mere presence
//! therefore does *not* mean its metadata is complete for every database (a
//! later `\connect`-ed one is still full-scan-only), but it does always mean
//! the first one is.
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

use arrow::array::RecordBatch;
use arrow::buffer::Buffer;
use async_stream::try_stream;
use futures::Stream;

use crate::batch::{
    BatchOptions, RowBatcher, ScanExtent, SourceChunk, column_names, invalidate_block_cache,
};
use crate::cache::CacheMode;
use crate::copy::{CopyHeader, DELIMITER};
use crate::index::{ArrayShape, CopyBlock, DumpIndex, scan_preamble, union_census};
use crate::io::ByteRangeSource;
use crate::map::{Span, SpanBody};
use crate::preamble::DumpMetadata;
use crate::predicate::Predicate;
use crate::resolve::{ResolvedSchema, resolve_columns};
use crate::scan::{CopyScanner, Event, ScanOptions};
use crate::{Error, Result};

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, the column index a [`Predicate`] was resolved to
/// against this block's own schema (schemas can differ block-to-block, e.g.
/// a headerless block's placeholder names), and the database this block is
/// attributed to (`docs/design/architecture.md`, "One target per query").
type Active = (u64, CopyHeader, RowBatcher, Option<usize>, Option<String>);

/// `database`, rendered as `database.schema.table`, or just `schema.table`
/// when the file had no `\connect` at all — the form `Error::AmbiguousTable`
/// names its candidates in.
fn render_candidate((database, qualified_name): &(Option<String>, String)) -> String {
    match database {
        Some(db) => format!("{db}.{qualified_name}"),
        None => qualified_name.clone(),
    }
}

/// Resolve `predicate`'s column name against `schema`, once per block. `Ok(None)`
/// when there is no predicate to apply.
fn resolve_predicate_index(
    predicate: Option<&Predicate>,
    schema: &arrow::datatypes::SchemaRef,
    header_offset: u64,
) -> Result<Option<usize>> {
    let Some(predicate) = predicate else { return Ok(None) };
    schema
        .fields()
        .iter()
        .position(|f| f.name() == &predicate.column)
        .map(Some)
        .ok_or(Error::UnknownPredicateColumn { header_offset, column: predicate.column.clone() })
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
/// exactly what that decision exists to prevent. A `batch_options.database`
/// selector does not lift this: two `\connect` segments can name the *same*
/// database. So any `Connect` span means map the whole file.
///
/// What neither test catches is a file whose *first* segment has no
/// `\connect` — a plain dump with something concatenated after it. Nothing in
/// the prefix announces that; see `STATUS.md`'s "Known gaps".
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

/// Extend `index`'s map forward from its own `scanned_through`, persisting
/// after every completed block, until the queried table is settled or EOF is
/// reached. Emits no rows — see the module docs.
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
async fn map_forward<S: ByteRangeSource>(
    source: &S,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    index: &mut DumpIndex,
    target: Option<(&str, Option<&str>)>,
    size: u64,
) -> Result<()> {
    if index.scanned_through >= size {
        return Ok(());
    }
    if target.is_some_and(|(table, selector)| target_settled(index, table, selector)) {
        return Ok(());
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
    let mut builder = crate::map::Builder::with_database(database);

    let mut scanner = CopyScanner::resume(seg_start, None);
    let mut read_pos = seg_start;
    let mut buf: Vec<u8> = Vec::with_capacity(scan_options.chunk_size);

    loop {
        let want = scan_options.chunk_size.min((size - read_pos) as usize);
        if want > 0 {
            let bytes = source.read_range(read_pos, want).await?;
            read_pos += bytes.len() as u64;
            buf.extend_from_slice(&bytes);
        }
        let eof = read_pos >= size;

        while let Some(event) = scanner.next_event(&buf, eof)? {
            match event {
                Event::CopyStart(start) => builder.on_copy_start(start),
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
                    index.spans =
                        splice(&prefix, builder.snapshot(watermark), seg_start, watermark, size);
                    index.roles.extend(builder.roles().iter().cloned());
                    index.tablespaces.extend(builder.tablespaces().iter().cloned());
                    index.scanned_through = index.scanned_through.max(watermark);
                    cache.save(source, index).await?;
                    if target
                        .is_some_and(|(table, selector)| target_settled(index, table, selector))
                    {
                        return Ok(());
                    }
                }
                Event::Line(line) => builder.feed_line(line.offset, line.raw),
                Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
                Event::LargeObjectStart(start) => builder.on_large_object_start(start.start_offset),
                Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
            }
        }

        let used = scanner.take_consumed();
        buf.drain(..used);

        if eof {
            break;
        }
        if buf.len() > scan_options.max_line_bytes {
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
    crate::map::attach_text(source, &mut index.spans).await?;
    index.diagnostics = crate::index::tiling_diagnostics(&index.spans, size);
    index.diagnostics.push(crate::index::toc_coverage_diagnostic(&index.spans));
    cache.save(source, index).await
}

/// Map `source` end to end, **continuing from whatever `cache` already
/// holds** — `pgdq parse`'s scan (`docs/design/architecture.md`, "CLI
/// surface"). Returns the finished [`DumpIndex`] and the frontier this run
/// started from: `0` for a scan that began at byte 0, and the cache's
/// `scanned_through` for one that resumed.
///
/// This is [`map_forward`] with no stop target, plus the three whole-file
/// facts that only a scan reaching EOF may state. It is a second caller for
/// the incremental loop, not a second implementation of it: `crate::index::build_index`
/// stays the eager, cache-blind producer, and teaching *it* to resume would
/// duplicate the splice-onto-a-prefix logic here with a different set of bugs.
///
/// **The three finishing steps are this function's, not `map_forward`'s.**
///
/// - `metadata` is recomputed over the whole span list. A query only ever has
///   `crate::index::scan_preamble`'s first-database capture, because that is
///   all a scan stopping at its target may honestly claim
///   ([`crate::preamble::dump_metadata_from_spans`] names the two boundaries
///   it may be called at, and a `CopyEnd` watermark is not one of them). A
///   scan that reached EOF is at the other boundary, so every `\connect`ed
///   database's DDL is recovered here — which is what makes "a full `pgdq
///   parse` leaves every database `preamble_complete`" true.
/// - `diagnostics` are recomputed rather than inherited: they are
///   `#[serde(skip)]`, so an index that came wholly from the cache carries
///   none, and whatever [`CacheMode::load`] reported about the cache *file*
///   (an mtime mismatch) is kept ahead of them rather than overwritten.
/// - The cache is saved once more at the end. `map_forward` already saved at
///   EOF, but with the pre-EOF metadata; this is the save that persists the
///   finished index, and it is also the only save when the cache already
///   covered the file and nothing was scanned at all.
pub async fn map_file<S: ByteRangeSource>(
    source: &S,
    scan_options: &ScanOptions,
    cache: &CacheMode,
) -> Result<(DumpIndex, u64)> {
    let size = source.size().await?;
    let mut index = cache.load(source).await?.unwrap_or_default();
    // The one diagnostic about the cache *file* rather than about the map:
    // everything else the load computed is recomputed below over the finished
    // spans, and `map_forward` assigns `diagnostics` wholesale at EOF anyway.
    let carried: Vec<crate::diagnostic::Diagnostic> = index
        .diagnostics
        .drain(..)
        .filter(|d| d.kind == crate::diagnostic::DiagnosticKind::CacheMtimeChanged)
        .collect();
    let resumed_from = index.scanned_through.min(size);

    map_forward(source, scan_options, cache, &mut index, None, size).await?;

    index.metadata = Some(crate::preamble::dump_metadata_from_spans(&index.spans));
    let mut diagnostics = carried;
    diagnostics.extend(crate::index::tiling_diagnostics(&index.spans, size));
    diagnostics.push(crate::index::toc_coverage_diagnostic(&index.spans));
    index.diagnostics = diagnostics;
    cache.save(source, &index).await?;
    Ok((index, resumed_from))
}

/// Opaque cursor into a [`table_stream`]/[`crate::batch::read_table`]
/// consumption, sufficient to resume from just past the last batch a caller
/// accepted. Valid only within the process that produced it — persisting it
/// across a restart is out of scope (`docs/design/roadmap.md`).
#[derive(Debug, Clone)]
pub struct ResumeToken {
    offset: u64,
    rows_emitted: u64,
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
    fn start() -> Self {
        Self { offset: 0, rows_emitted: 0, generation: 0, in_copy: None }
    }
}

/// A pull-mode stream of `Utf8View` `RecordBatch`es for one table query.
/// Construct with [`table_stream`].
pub struct TableStream<'a> {
    inner: Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send + 'a>>,
    position: Arc<Mutex<ResumeToken>>,
    resolved_schema: Arc<Mutex<ResolvedSchema>>,
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
/// `NotDeclared` degradation — reachable only via `table_stream`'s
/// incremental scan, which never learns a later `\connect`ed database's DDL
/// (see "The preamble pass" in the phase doc); a full `pgdq parse` scan
/// always leaves every database it found `preamble_complete`.
fn resolve_block(
    header: &CopyHeader,
    field_count: usize,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    schema_mode: crate::resolve::SchemaMode,
    census: &[ArrayShape],
) -> Result<ResolvedSchema> {
    if schema_mode == crate::resolve::SchemaMode::Typed
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
    batch_options: &BatchOptions,
    predicate: Option<&Predicate>,
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
            let r = resolve_block(
                &ic.header,
                ic.field_count,
                metadata,
                ic.database.as_deref(),
                batch_options.schema_mode,
                census,
            )?;
            let predicate_index = resolve_predicate_index(predicate, &r.schema, ic.header_offset)?;
            let batcher = RowBatcher::new(&r, ic.header.qualified_name(), batch_options.clone());
            resolved = Some(r);
            Ok::<_, Error>((
                ic.header_offset,
                ic.header.clone(),
                batcher,
                predicate_index,
                ic.database.clone(),
            ))
        })
        .transpose()?;
    Ok((scanner, active, resolved))
}

fn snapshot(scanner: &CopyScanner, active: &Option<Active>, rows_emitted: u64) -> ResumeToken {
    let in_copy =
        active.as_ref().map(|(header_offset, header, batcher, _, database)| InCopyResume {
            header: header.clone(),
            header_offset: *header_offset,
            rows_in_block: scanner.in_copy_rows().unwrap_or(0),
            field_count: batcher.field_count(),
            database: database.clone(),
        });
    ResumeToken { offset: scanner.position(), rows_emitted, generation: 0, in_copy }
}

/// Pull-mode entry point: stream `Utf8View` `RecordBatch`es for every row of
/// every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`]). A table with zero rows yields no batches.
///
/// `resume` continues a previous consumption from a [`ResumeToken`] it
/// produced; `None` starts from the beginning of `source`.
///
/// `predicate` applies `docs/design/architecture.md`'s post-parse row
/// filter (`docs/design/architecture.md`, "Predicates"): `None`
/// yields every row, as before; `Some` drops any row whose named column
/// doesn't satisfy it, after that row has been fully unescaped. Referencing a
/// column absent from a matching block's own schema is
/// `Error::UnknownPredicateColumn`.
///
/// `cache` controls structure-cache consulting
/// (`docs/design/architecture.md`, "The cache").
/// `CacheMode::Enabled` persists the map after every block the mapping pass
/// completes, so a later query against the same dump starts from a nearer
/// frontier; `CacheMode::Disabled` runs identically but writes nothing,
/// mapping in memory for this call only. Either way rows come from replaying
/// mapped blocks, never from the mapping pass itself — see the module docs.
///
/// `batch_options.scan_extent` decides how much of the file the mapping pass
/// walks before any row comes back; see [`ScanExtent`].
pub fn table_stream<'a, S>(
    source: &'a S,
    table: &str,
    scan_options: ScanOptions,
    batch_options: BatchOptions,
    predicate: Option<Predicate>,
    resume: Option<ResumeToken>,
    cache: CacheMode,
) -> TableStream<'a>
where
    S: ByteRangeSource + 'a,
{
    let table = table.to_string();
    let start_token = resume.clone().unwrap_or_else(ResumeToken::start);
    let position = Arc::new(Mutex::new(start_token));
    let position_for_stream = Arc::clone(&position);
    let resolved_schema = Arc::new(Mutex::new(ResolvedSchema::default()));
    let resolved_schema_for_stream = Arc::clone(&resolved_schema);

    let inner = try_stream! {
        let size = source.size().await?;

        let mut index = cache.load(source).await?.unwrap_or_default();

        // The first database's preamble always gets captured before
        // anything else runs, regardless of which table this particular
        // call queries or whether it ever reaches the file's first `COPY`
        // block itself (`crate::index::scan_preamble`'s docs) — every
        // `Typed`-mode query needs it for type resolution below, not just a
        // caller that goes on to persist a cache. `CacheMode::Disabled`
        // still runs the scan (`docs/design/architecture.md`,
        // "`--dqcache none` disables persistence, not typing") but
        // `cache.save` below is a no-op for it, so nothing is written.
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
            crate::map::attach_text(source, &mut index.spans).await?;
            cache.save(source, &index).await?;
        }
        let metadata = index.metadata.clone();

        // Pass 1: extend the map until this query's table is settled. No
        // rows come out of this, and nothing is yielded until it returns.
        let selector = batch_options.database.as_deref();
        let target = match batch_options.scan_extent {
            ScanExtent::UntilTargetSettled => Some((table.as_str(), selector)),
            ScanExtent::Full => None,
        };
        map_forward(source, &scan_options, &cache, &mut index, target, size).await?;

        // One target per query (`docs/design/architecture.md`,
        // "One target per query"): narrow the name-only matches down to at
        // most one `(database, qualified name)` candidate before reading any
        // of them, so a would-be silent union across schemas or databases
        // errors instead. `batch_options.database`, when given, is the way
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

        // Only the first replayed block can start mid-block (a resumed
        // stream paused between two of its rows); its scanner and in-flight
        // batcher are prebuilt here so `resume_state`'s logic isn't
        // duplicated below.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) if token.in_copy.is_some() => {
                let (scanner, active, resolved) = resume_state(
                    token,
                    &batch_options,
                    predicate.as_ref(),
                    metadata.as_ref(),
                    &census,
                )?;
                if let Some(r) = resolved {
                    *resolved_schema_for_stream.lock().unwrap() = r;
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

        for block in matches.iter().filter(|b| b.end_offset > resume_offset) {
            let seg_start = block.header_offset.max(resume_offset);
            let seg_end = block.end_offset;
            let block_database = block.database.clone();

            let mut scanner =
                first_scanner.take().unwrap_or_else(|| CopyScanner::resume(seg_start, None));
            let mut read_pos = seg_start;
            let mut buf: Vec<u8> = Vec::with_capacity(scan_options.chunk_size);
            let mut chunks: VecDeque<SourceChunk> = VecDeque::new();

            loop {
                let want = scan_options.chunk_size.min((seg_end - read_pos) as usize);
                if want > 0 {
                    let bytes = source.read_range(read_pos, want).await?;
                    chunks.push_back(SourceChunk {
                        start: read_pos,
                        buffer: Buffer::from(bytes.clone()),
                        column_blocks: Vec::new(),
                    });
                    read_pos += bytes.len() as u64;
                    buf.extend_from_slice(&bytes);
                }
                let eof = read_pos >= seg_end;

                while let Some(event) = scanner.next_event(&buf, eof)? {
                    match event {
                        Event::CopyStart(start) => {
                            if start.header.columns.is_empty() {
                                pending =
                                    Some((start.header, start.header_offset, block_database.clone()));
                            } else {
                                let resolved = resolve_block(
                                    &start.header,
                                    start.header.columns.len(),
                                    metadata.as_ref(),
                                    block_database.as_deref(),
                                    batch_options.schema_mode,
                                    &census,
                                )?;
                                let predicate_index = resolve_predicate_index(
                                    predicate.as_ref(),
                                    &resolved.schema,
                                    start.header_offset,
                                )?;
                                let batcher = RowBatcher::new(
                                    &resolved,
                                    start.header.qualified_name(),
                                    batch_options.clone(),
                                );
                                *resolved_schema_for_stream.lock().unwrap() = resolved;
                                active = Some((
                                    start.header_offset,
                                    start.header,
                                    batcher,
                                    predicate_index,
                                    block_database.clone(),
                                ));
                            }
                        }
                        Event::Row(row) => {
                            if let Some((header, header_offset, block_database)) = pending.take() {
                                let field_count =
                                    memchr::memchr_iter(DELIMITER, row.raw).count() + 1;
                                let resolved = resolve_block(
                                    &header,
                                    field_count,
                                    metadata.as_ref(),
                                    block_database.as_deref(),
                                    batch_options.schema_mode,
                                    &census,
                                )?;
                                let predicate_index = resolve_predicate_index(
                                    predicate.as_ref(),
                                    &resolved.schema,
                                    header_offset,
                                )?;
                                let batcher = RowBatcher::new(
                                    &resolved,
                                    header.qualified_name(),
                                    batch_options.clone(),
                                );
                                *resolved_schema_for_stream.lock().unwrap() = resolved;
                                active = Some((
                                    header_offset,
                                    header,
                                    batcher,
                                    predicate_index,
                                    block_database,
                                ));
                            }
                            if let Some((header_offset, _, batcher, predicate_index, _)) =
                                active.as_mut()
                            {
                                let keep = match (predicate.as_ref(), *predicate_index) {
                                    (Some(pred), Some(col)) => pred.matches(row.raw, col)?,
                                    _ => true,
                                };
                                if keep {
                                    batcher.push_row(
                                        *header_offset,
                                        row.offset,
                                        row.raw,
                                        &mut chunks,
                                    )?;
                                }
                                if batcher.should_flush() {
                                    let batch = batcher.flush()?;
                                    invalidate_block_cache(&mut chunks);
                                    rows_emitted += batch.num_rows() as u64;
                                    *position_for_stream.lock().unwrap() =
                                        snapshot(&scanner, &active, rows_emitted);
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
                                    snapshot(&scanner, &active, rows_emitted);
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

                let used = scanner.take_consumed();
                buf.drain(..used);

                // Everything before the scanner's new position has already had
                // its chance to be referenced by a zero-copy view (that happens
                // synchronously above, before we get here), so it's safe to drop.
                let floor = scanner.position();
                while chunks.front().is_some_and(|c| c.end() <= floor) {
                    chunks.pop_front();
                }

                if eof {
                    break;
                }
                if buf.len() > scan_options.max_line_bytes {
                    Err(Error::LineTooLong {
                        offset: scanner.position(),
                        limit: scan_options.max_line_bytes,
                    })?;
                }
            }
        }
    };

    TableStream { inner: Box::pin(inner), position, resolved_schema }
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
