//! Pull-mode streaming API (`docs/design/roadmap-phase1-mvp.md`, "Streaming
//! API").
//!
//! [`table_stream`] is the primitive: an async `Stream<Item =
//! Result<RecordBatch>>` built directly on [`CopyScanner`]/[`RowBatcher`], the
//! same machinery [`crate::batch::read_table`] (push mode) now drives
//! internally rather than duplicating. [`ResumeToken`] lets a caller stop
//! consuming partway through and pick back up later in the same process — it
//! holds no public fields (`roadmap-phase1-mvp.md` is explicit that it must
//! stay opaque), so its representation is free to change without an API break.
//!
//! **Cache-consulting** (`roadmap-phase1-mvp.md`, "Index / structure
//! cache"): a [`CacheMode::Enabled`] cache is consulted up front and turned
//! into a sequence of [`Segment`]s — one [`Segment::Known`] per already-cached
//! block matching the query table (replayed for its rows, at zero I/O cost
//! for every non-matching block in between) plus one trailing
//! [`Segment::Live`] covering whatever's past the cache's watermark, which
//! is scanned as today, with every block it finds (matching or not) handed
//! to a [`Recorder`] and persisted back after each one completes.
//!
//! **Preamble capture** (Phase 2.2.1,
//! `docs/design/roadmap-phase2.2.1-incremental-preamble-notes.md`): before
//! any of that, [`table_stream`] runs [`crate::index::scan_preamble`] once
//! (skipped once a cache already has it), regardless of which table was
//! queried, whether it ever appears, or how far the live scan gets before a
//! caller stops polling. This runs even under [`CacheMode::Disabled`] as of
//! Phase 2.3 — `--cache-path none` disables *persistence*, not type
//! resolution (`docs/design/roadmap-phase2-typed-columns.md`, "The preamble
//! pass") — but `cache.save` is a no-op there, so nothing is written to
//! disk. Under [`CacheMode::Enabled`], a cache file's mere presence
//! therefore does *not* mean its metadata is complete for every database (a
//! later `\connect`-ed one is still full-scan-only), but it does always mean
//! the first one is.
//!
//! **Type resolution** (Phase 2.2.1's successor): once a query's matching
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

use crate::batch::{BatchOptions, RowBatcher, SourceChunk, invalidate_block_cache, schema_for};
use crate::cache::CacheMode;
use crate::copy::{CopyHeader, DELIMITER};
use crate::index::{CopyBlock, DumpIndex, scan_preamble};
use crate::io::ByteRangeSource;
use crate::preamble::DumpMetadata;
use crate::predicate::Predicate;
use crate::resolve::{ResolvedSchema, resolve_columns};
use crate::scan::{CopyScanner, CopyStart, Event, ScanOptions};
use crate::{Error, Result};

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, the column index a [`Predicate`] was resolved to
/// against this block's own schema (schemas can differ block-to-block, e.g.
/// a headerless block's placeholder names), and the database this block is
/// attributed to (`docs/design/roadmap-phase2-typed-columns.md`, "One target
/// per query").
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

/// One piece of a [`table_stream`] scan: either replaying a block the cache
/// already knew about, or walking unscanned territory live. See the module
/// docs.
enum Segment {
    /// `[start, end)`: an already-cached block matching the query table.
    Known { start: u64, end: u64 },
    /// `[start, size)`: unscanned territory, discovered as it's walked.
    Live { start: u64 },
}

/// Accumulates newly-discovered blocks during a [`Segment::Live`] scan and
/// persists them back to the cache. Seeded from whatever the cache already
/// had, so a persisted index stays complete rather than shrinking to just
/// this call's discoveries.
struct Recorder {
    cache: CacheMode,
    index: DumpIndex,
}

impl Recorder {
    /// Record `block`, ignoring it if a block at the same `header_offset` is
    /// already present. A resumed stream's live segment can re-walk bytes the
    /// base index already covers (see `docs/design/roadmap-phase1-mvp.md`'s
    /// "Known gaps") — without this guard that would duplicate an entry rather
    /// than just redundantly re-read some bytes.
    fn record(&mut self, block: CopyBlock) {
        if !self.index.blocks.iter().any(|b| b.header_offset == block.header_offset) {
            self.index.blocks.push(block);
        }
    }

    /// `end.end_offset` is always a safe, resumable watermark — the scanner
    /// is back in its `Outside` state there — so every persisted
    /// `scanned_through` is valid for a later query to replay from.
    fn persist(&mut self, scanned_through: u64) -> Result<()> {
        self.index.scanned_through = self.index.scanned_through.max(scanned_through);
        self.cache.save(&self.index)
    }
}

/// Opaque cursor into a [`table_stream`]/[`crate::batch::read_table`]
/// consumption, sufficient to resume from just past the last batch a caller
/// accepted. Valid only within the process that produced it — persisting it
/// across a restart is out of scope for Phase 1 (`roadmap-phase1-mvp.md`).
#[derive(Debug, Clone)]
pub struct ResumeToken {
    offset: u64,
    rows_emitted: u64,
    /// Reserved for the structural cache's generation stamp. The cache
    /// doesn't stamp generations in Phase 1, so this is always 0; carrying
    /// the field now avoids a later breaking change to this already-opaque
    /// type.
    #[allow(dead_code)]
    generation: u64,
    in_copy: Option<InCopyResume>,
    /// The database in scope at `offset`, tracked from `\connect` lines the
    /// live scan passes over (`crate::preamble::parse_connect`). Lets a
    /// resumed stream keep attributing newly-discovered blocks correctly
    /// without re-scanning from the file start — see "One target per query"
    /// in `docs/design/roadmap-phase2-typed-columns.md`.
    database: Option<String>,
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
        Self { offset: 0, rows_emitted: 0, generation: 0, in_copy: None, database: None }
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
    /// (`docs/design/roadmap-phase2-typed-columns.md`, "Schema resolution":
    /// "one schema per stream"). The empty schema (`ResolvedSchema::default`)
    /// until the query's matching `COPY` block has been found — which, for a
    /// table that never appears in the dump, is forever; a caller checking
    /// before consuming any batches only learns that once the whole stream
    /// has been drained.
    pub fn resolved_schema(&self) -> ResolvedSchema {
        self.resolved_schema.lock().unwrap().clone()
    }
}

/// Build both the batch schema (always all-`Utf8View` in Phase 2.3 — see
/// [`TableStream::resolved_schema`]'s docs) and the preview
/// [`ResolvedSchema`] for a table-matching block, from the same column
/// names, scoped to `database` — the block's own attribution, never a guess
/// (`docs/design/roadmap-phase2-typed-columns.md`, "One target per query").
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
) -> Result<(arrow::datatypes::SchemaRef, ResolvedSchema)> {
    let batch_schema = schema_for(header, field_count);
    if schema_mode == crate::resolve::SchemaMode::Typed
        && let Some(meta) = metadata
        && !meta.databases.iter().any(|db| db.name.as_deref() == database && db.preamble_complete)
    {
        return Err(Error::MetadataNotScanned { database: database.map(str::to_string) });
    }
    let names: Vec<String> = batch_schema.fields().iter().map(|f| f.name().clone()).collect();
    let resolved =
        resolve_columns(&header.qualified_name(), &names, metadata, database, schema_mode);
    Ok((batch_schema, resolved))
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
            let (schema, r) = resolve_block(
                &ic.header,
                ic.field_count,
                metadata,
                ic.database.as_deref(),
                batch_options.schema_mode,
            )?;
            resolved = Some(r);
            let predicate_index = resolve_predicate_index(predicate, &schema, ic.header_offset)?;
            Ok::<_, Error>((
                ic.header_offset,
                ic.header.clone(),
                RowBatcher::new(schema, batch_options.clone()),
                predicate_index,
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
    current_database: Option<String>,
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
        generation: 0,
        in_copy,
        database: current_database,
    }
}

/// Pull-mode entry point: stream `Utf8View` `RecordBatch`es for every row of
/// every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`]). A table with zero rows yields no batches.
///
/// `resume` continues a previous consumption from a [`ResumeToken`] it
/// produced; `None` starts from the beginning of `source`.
///
/// `predicate` applies `docs/design/roadmap-phase1-mvp.md`'s post-parse row
/// filter (`docs/design/roadmap-phase1-mvp.md`, "Predicate filtering"): `None`
/// yields every row, as before; `Some` drops any row whose named column
/// doesn't satisfy it, after that row has been fully unescaped. Referencing a
/// column absent from a matching block's own schema is
/// `Error::UnknownPredicateColumn`.
///
/// `cache` controls structure-cache consulting
/// (`docs/design/roadmap-phase1-mvp.md`, "Index / structure cache"):
/// `CacheMode::Disabled` is pure streaming with no side effects;
/// `CacheMode::Enabled` replays already-cached blocks matching `table` at zero
/// I/O cost for everything in between, then scans live from the cache's
/// watermark, persisting each newly-discovered block (matching or not) as it
/// completes — so a later query against the same dump gets progressively
/// cheaper. `resume` takes priority over cache replay: it always starts a
/// single live segment at the token's offset (see
/// `docs/design/roadmap-phase1-mvp.md`'s "Known gaps" for what that costs in
/// the rare case of resuming from inside a would-be replay).
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

        let mut base_index = cache.load()?.unwrap_or_default();

        // The first database's preamble always gets captured before
        // anything else runs, regardless of which table this particular
        // call queries or whether it ever reaches the file's first `COPY`
        // block itself (`crate::index::scan_preamble`'s docs) — every
        // `Typed`-mode query needs it for type resolution below, not just a
        // caller that goes on to persist a cache. `CacheMode::Disabled`
        // still runs the scan (`docs/design/roadmap-phase2-typed-columns.md`,
        // "`--cache-path none` disables persistence, not typing") but
        // `cache.save` below is a no-op for it, so nothing is written.
        // Persisted immediately (not deferred to whenever a segment below
        // next saves) so it survives even a caller that polls the stream
        // once and drops it.
        let first_db_preamble_known = base_index
            .metadata
            .as_ref()
            .and_then(|m| m.databases.first())
            .is_some_and(|db| db.preamble_complete);
        if !first_db_preamble_known {
            let (metadata, preamble_end) = scan_preamble(source, &scan_options).await?;
            base_index.metadata = Some(metadata);
            base_index.scanned_through = base_index.scanned_through.max(preamble_end);
            cache.save(&base_index)?;
        }
        // Captured before `base_index` is moved into `recorder` below —
        // every call site that resolves a matching block's schema needs it.
        let metadata = base_index.metadata.clone();

        // One target per query (`docs/design/roadmap-phase2-typed-columns.md`,
        // "One target per query"): narrow the name-only matches already
        // known from the cache down to at most one `(database, qualified
        // name)` candidate before touching any of them, so a would-be
        // silent union across schemas or databases errors instead.
        // `batch_options.database`, when given, is the way out of an
        // otherwise-ambiguous bare or cross-database name — it filters
        // candidates first, exactly like a `WHERE` clause narrowing matches
        // rather than picking among them after the fact.
        let selector = batch_options.database.as_deref();
        let known_matches: Vec<CopyBlock> = base_index
            .blocks_for(&table)
            .filter(|b| selector.is_none() || b.database.as_deref() == selector)
            .cloned()
            .collect();
        let mut target: Option<(Option<String>, String)> = None;
        for b in &known_matches {
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

        let segments: Vec<Segment> = match &resume {
            Some(token) => vec![Segment::Live { start: token.offset }],
            None => {
                let mut segs: Vec<Segment> = known_matches
                    .iter()
                    .map(|b| Segment::Known { start: b.header_offset, end: b.end_offset })
                    .collect();
                segs.push(Segment::Live { start: base_index.scanned_through });
                segs
            }
        };
        // The database in scope right now, tracked from `\connect` lines a
        // live segment passes over (`crate::preamble::parse_connect`) so a
        // newly-discovered block can be attributed correctly without
        // "reading preamble as it goes" — see "One target per query".
        // Resuming carries it over from the token directly (same reasoning
        // as `InCopyResume`); starting fresh from a cache seeds it from
        // whichever already-known block ends closest to `scanned_through`
        // (always exactly the block immediately preceding it, since the
        // watermark only ever advances to a block's own `end_offset` or to
        // EOF) — correct at that exact byte, and self-correcting the moment
        // this scan's own live segment passes a `\connect` beyond it. When no
        // block is known yet — a cold start, or `CacheMode::Disabled`, where
        // `scanned_through` only ever reflects the preamble prepass's own
        // watermark (exactly the first `COPY` block's offset, or EOF) — the
        // truth at that point is instead whichever database the prepass
        // just finished capturing, `metadata.databases`' first entry (I1:
        // nothing of interest precedes any database's first block, so the
        // very first such stopping point in the file is necessarily still
        // within that first database).
        let mut current_database: Option<String> = match &resume {
            Some(token) => token.database.clone(),
            None => base_index
                .blocks
                .iter()
                .max_by_key(|b| b.end_offset)
                .map(|b| b.database.clone())
                .unwrap_or_else(|| {
                    metadata.as_ref().and_then(|m| m.databases.first()).and_then(|db| db.name.clone())
                }),
        };
        let mut recorder: Option<Recorder> = (!matches!(cache, CacheMode::Disabled))
            .then(|| Recorder { cache: cache.clone(), index: base_index });

        // Only the first segment can start mid-block (a resumed stream); a
        // fresh `CopyScanner` for it is prebuilt here so `resume_state`'s
        // logic isn't duplicated below.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) => {
                let (scanner, active, resolved) =
                    resume_state(token, &batch_options, predicate.as_ref(), metadata.as_ref())?;
                if let Some(r) = resolved {
                    *resolved_schema_for_stream.lock().unwrap() = r;
                }
                (active, Some(scanner))
            }
            None => (None, None),
        };
        // A matching header with no column list, waiting on its first row to
        // learn the field count. Never non-empty across a resume point: a
        // stream only yields right after a flush, and by then any pending
        // headerless block has already seen its first row (see `active`).
        let mut pending: Option<(CopyHeader, u64, Option<String>)> = None;
        let mut rows_emitted = resume.as_ref().map_or(0, |t| t.rows_emitted);

        for (segment_index, segment) in segments.into_iter().enumerate() {
            let is_live = matches!(segment, Segment::Live { .. });
            let (seg_start, seg_end) = match &segment {
                Segment::Known { start, end } => (*start, *end),
                Segment::Live { start } => (*start, size),
            };

            let mut scanner = if segment_index == 0 {
                first_scanner.take().unwrap_or_else(|| CopyScanner::resume(seg_start, None))
            } else {
                CopyScanner::resume(seg_start, None)
            };
            let mut read_pos = seg_start;
            let mut buf: Vec<u8> = Vec::with_capacity(scan_options.chunk_size);
            let mut chunks: VecDeque<SourceChunk> = VecDeque::new();
            // The most recent `CopyStart` in this (live) segment, regardless
            // of table match — mirrors `build_index`'s `pending`, tracked
            // separately from the table-matching `pending`/`active` above
            // since a live segment must record every block, not just ones
            // the query cares about.
            let mut block_start: Option<CopyStart> = None;

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
                            // A known-segment replay is always exactly the
                            // one target this call already confirmed before
                            // the loop started; a live segment's attribution
                            // instead tracks whatever `\connect` it has
                            // itself passed over so far.
                            let block_database = if is_live {
                                current_database.clone()
                            } else {
                                target.as_ref().and_then(|(db, _)| db.clone())
                            };
                            if is_live && recorder.is_some() {
                                block_start = Some(start.clone());
                            }
                            let selector = batch_options.database.as_deref();
                            let matches_table = start.header.matches(&table)
                                && (selector.is_none() || block_database.as_deref() == selector);
                            if matches_table && is_live {
                                // A known-segment match was already checked
                                // for ambiguity before the loop started; a
                                // live discovery needs the same check applied
                                // incrementally, since the full match set
                                // isn't known until the scan reaches EOF.
                                let key = (block_database.clone(), start.header.qualified_name());
                                match &target {
                                    None => target = Some(key),
                                    Some(t) if *t != key => {
                                        Err(Error::AmbiguousTable {
                                            name: table.clone(),
                                            candidates: vec![
                                                render_candidate(t),
                                                render_candidate(&key),
                                            ],
                                        })?;
                                    }
                                    _ => {}
                                }
                            }
                            if matches_table {
                                if start.header.columns.is_empty() {
                                    pending = Some((start.header, start.header_offset, block_database));
                                } else {
                                    let (schema, resolved) = resolve_block(
                                        &start.header,
                                        start.header.columns.len(),
                                        metadata.as_ref(),
                                        block_database.as_deref(),
                                        batch_options.schema_mode,
                                    )?;
                                    *resolved_schema_for_stream.lock().unwrap() = resolved;
                                    let predicate_index = resolve_predicate_index(
                                        predicate.as_ref(),
                                        &schema,
                                        start.header_offset,
                                    )?;
                                    active = Some((
                                        start.header_offset,
                                        start.header,
                                        RowBatcher::new(schema, batch_options.clone()),
                                        predicate_index,
                                        block_database,
                                    ));
                                }
                            }
                        }
                        Event::Row(row) => {
                            if let Some((header, header_offset, block_database)) = pending.take() {
                                let field_count =
                                    memchr::memchr_iter(DELIMITER, row.raw).count() + 1;
                                let (schema, resolved) = resolve_block(
                                    &header,
                                    field_count,
                                    metadata.as_ref(),
                                    block_database.as_deref(),
                                    batch_options.schema_mode,
                                )?;
                                *resolved_schema_for_stream.lock().unwrap() = resolved;
                                let predicate_index = resolve_predicate_index(
                                    predicate.as_ref(),
                                    &schema,
                                    header_offset,
                                )?;
                                active = Some((
                                    header_offset,
                                    header,
                                    RowBatcher::new(schema, batch_options.clone()),
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
                                        snapshot(&scanner, &active, rows_emitted, current_database.clone());
                                    yield batch;
                                }
                            }
                        }
                        Event::CopyEnd(end) => {
                            pending = None;
                            if let Some((_, _, mut batcher, _, _)) = active.take()
                                && !batcher.is_empty()
                            {
                                let batch = batcher.flush()?;
                                invalidate_block_cache(&mut chunks);
                                rows_emitted += batch.num_rows() as u64;
                                *position_for_stream.lock().unwrap() =
                                    snapshot(&scanner, &active, rows_emitted, current_database.clone());
                                yield batch;
                            }
                            if let (Some(start), Some(rec)) =
                                (block_start.take(), recorder.as_mut())
                            {
                                rec.record(CopyBlock {
                                    header: start.header,
                                    // `block_start` is only ever set while
                                    // `is_live`, so `current_database` here
                                    // is this same segment's live tracking,
                                    // unchanged since the matching `CopyStart`
                                    // (a `\connect` cannot occur mid-block).
                                    database: current_database.clone(),
                                    header_offset: start.header_offset,
                                    data_offset: start.data_offset,
                                    terminator_offset: end.terminator_offset,
                                    end_offset: end.end_offset,
                                    row_count: end.row_count,
                                    sparse_index: None,
                                    column_stats: None,
                                });
                                rec.persist(end.end_offset)?;
                            }
                        }
                        // The prepass above already captured the first
                        // database's preamble (I1: nothing before its first
                        // `COPY` block matters to `crate::preamble`), so a
                        // `Line` reaching here is always past that boundary
                        // — DDL for a later `\connect`ed database in a
                        // multi-database dump included, which no incremental
                        // path captures yet (only `build_index`'s full scan
                        // does). A `\connect` on it is still worth tracking,
                        // though — it's a database *name*, never a column
                        // type, so reading it here isn't "reading preamble
                        // as it goes" in the sense that section rules out;
                        // it's what lets a live-discovered block still be
                        // attributed to the right database.
                        Event::Line(line) => {
                            if let Some(name) =
                                crate::preamble::parse_connect(&String::from_utf8_lossy(line.raw))
                            {
                                current_database = Some(name);
                            }
                        }
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

            if is_live && let Some(rec) = recorder.as_mut() {
                rec.persist(size)?;
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
