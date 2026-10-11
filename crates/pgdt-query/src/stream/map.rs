//! The mapping pass (`docs/design/decisions.md`, "D48"): extending a map
//! forward from its frontier, closing each `COPY` block, saving the cache when
//! a save has earned its cost, and the whole-file runs `map_file`,
//! `build_index` and `build_map`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;

use pgdt_codec::internal::counter_for;
#[cfg(feature = "introspect")]
use pgdt_core::internal::statistics_loaded;
use pgdt_core::{Error, Result};
use pgdt_dump::attach_text;
use pgdt_dump::cache::{CacheLoad, CacheMode, SourceWatch};
use pgdt_dump::dump_metadata_from_spans;
use pgdt_dump::internal::Builder;
use pgdt_dump::internal::announce_cancellation;
use pgdt_dump::internal::{CensusPlan, RegionScan, Shortfall, scan_region};
use pgdt_dump::internal::{
    non_seekable_compression_diagnostic, scan_preamble, tiling_diagnostics, toc_coverage_diagnostic,
};
use pgdt_dump::{ChunkCarry, CopyEnd, CopyScanner, Event, ScanOptions, scan as scan_events};
use pgdt_io::{ByteRangeSource, WaitPolicy, internal::memory_budget_display};
use pgdt_model::internal::IgnoredRefusalsExt;
use pgdt_model::{
    Diagnostic, DiagnosticKind, DumpIndex, DumpMetadata, IgnoredRefusals, PostgresInvalidValues,
    Span, SpanBody,
};

use super::backfill::{
    Backfill, backfill_statistics, report_density_shortfall, report_statistics_declines,
};
use crate::gather;
use crate::statistics::{StatisticsAccount, StatisticsHeld, StatisticsRequest, Term};

/// Replace everything a mapping scan covered with what it built: `prefix`
/// (the spans that already tiled `[0, seg_start)`, untouched) followed by
/// `built` (a complete tiling of `[seg_start, watermark)` from
/// [`pgdt_dump::internal::Builder`]), plus the trailing [`SpanBody::Unscanned`] span
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
/// them (`docs/design/decisions.md`, "D48") even across a stopping point. A previous scan
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
pub(super) fn splice(
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
pub(super) enum MapStop {
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
pub(super) fn cancelled_read(error: &Error, scan_options: &ScanOptions) -> bool {
    matches!(error, Error::ScanCancelled { .. }) && scan_options.cancelled()
}

/// What a mapping pass records of each block it maps
/// (`docs/design/decisions.md`, "D77").
#[derive(Clone, Copy)]
pub(super) enum Pass<'a> {
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
/// [`pgdt_dump::internal::Builder`] is fed the whole [`Event`] stream, and
/// `DumpIndex::metadata` is [`dump_metadata_from_spans`] over the result and
/// [`DumpIndex::blocks`] a filter over it (`docs/design/decisions.md`,
/// "D34").
///
/// **Here rather than beside the index**, which is dump's: counting a block's
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
    // What stopped the scan, a block's close failing it. None can: this pass
    // gathers no statistics, so no block meets a refused field.
    let mut failed: Option<Error> = None;
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
            Event::CopyEnd(end) => {
                if let Err(e) = builder.on_copy_end(end) {
                    failed = Some(e);
                    return std::ops::ControlFlow::Break(());
                }
            }
            Event::Line(line) => builder.feed_line(line.offset, line.raw),
            Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
            Event::LargeObjectStart(start) => builder.on_large_object_start(start.start_offset),
            Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
        }
        std::ops::ControlFlow::Continue(())
    })
    .await?;
    match failed {
        Some(e) => Err(e),
        None => Ok(builder),
    }
}

/// Extend `index`'s map forward from its own `scanned_through`, persisting as
/// it goes, until the queried table is settled, EOF is reached, or the scan is
/// cancelled. Emits no rows — see the module docs. `target` is the
/// `(table, database selector)` a query may stop early for once
/// [`target_settled`] says so; **`None` means "run to EOF"**, what
/// [`ScanExtent::Full`](crate::batch::ScanExtent::Full) asks for and what [`map_file`] always wants. Rejected:
/// a `ScanExtent` beside a sentinel table name, which is dead data any later
/// reader has to prove is unused.
///
/// The [`pgdt_dump::internal::Builder`] is seeded with the database in scope at the
/// frontier, which it cannot infer; [`splice`] places its spans after the
/// frontier (`docs/design/decisions.md`, "D48").
///
/// **Not every completed block is persisted; every exit from the loop but
/// an error is** — see [`SaveThrottle`], an interrupt before the first read
/// included; the two that return before the loop, the map unchanged, save
/// nothing. **`index.metadata` is restated at each `\connect`ed
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
pub(super) async fn map_forward(
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
    // the way out (`pgdt_dump::leader::scan_region`).
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
                        // `pgdt_dump::internal::scan_preamble` does, so the span list
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
                        let header = start.header.clone();
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
                        let plan = census.then(|| {
                            let counter =
                                counter_for(&header, index.metadata.as_ref(), db.as_deref());
                            builder.count_block(Arc::clone(&counter));
                            CensusPlan { width: header.columns.len(), counter }
                        });
                        // A block no request tracks is observed too under
                        // `PostgresInvalidValues::Strict`, to check its fields.
                        let observer = request.and_then(|request| {
                            gather::observer_for(
                                request,
                                &header,
                                index.metadata.as_ref(),
                                db.as_deref(),
                                account,
                                scan_options.postgres_invalid_values,
                            )
                        });
                        if let Some(observer) = observer {
                            builder.observe_block(observer);
                        }
                        // **The offer, and this loop is the leader making it**
                        // (`pgdt_dump::leader::scan_region`): everything from
                        // `data_offset` until `\.` is line-structured rows, so
                        // the region may be handed to workers that never parse
                        // structure. It answers the block's totals as the
                        // serial scanner would have, or declines — and a
                        // block it closes holds the statistics its pieces
                        // observed, as a declined one does the rows this loop
                        // hands on.
                        let outcome = scan_region(
                            source,
                            scan_options,
                            &header.qualified_name(),
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
            scanner =
                CopyScanner::resume(at, None).with_standard_strings(scanner.standard_strings());
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
/// on a query the same fact is a [`PlanNote`](super::note::PlanNote) on a `TableStream`.
///
/// **Silence means the leader dispatched the announced count, or refused the
/// block on its own account** — and never
/// that every worker read at once. A decline is *not* silent: the shortfall is
/// computed before it and returned with it ([`pgdt_dump::leader::Shortfall`]), so
/// a region left serial by the budget or by the source still prints. Only
/// `scan_region`'s refusals about its own block are unreported, deliberately:
/// its floor, and a region the source advises one partition of where the
/// whole file advises more. `flag` keeps one scan-wide fact from printing
/// once per block, which is also why [`pgdt_dump::leader::Shortfall`] reports no
/// reason a later block could answer differently.
pub(super) fn report_shortfall(flag: &mut bool, shortfall: Option<Shortfall>) {
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

/// Close a `COPY` block on the [`pgdt_dump::internal::Builder`] and do everything
/// [`map_forward`] owes at a `CopyEnd`: the splice, the throttled save, and the
/// early-stop check. **One body, two call sites** — the serial scanner's
/// `CopyEnd` and a region [`pgdt_dump::leader::scan_region`] closed
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
    // A refused field fails the pass here, before the splice and the save:
    // the map stands at the last watermark it was spliced at.
    builder.on_copy_end(end)?;
    // The second of the guard's two check points (`docs/design/decisions.md`,
    // "D63"): the two together bound the response by the shorter of a chunk
    // and a block.
    let cancelled = scan_options.cancelled();
    let due = throttle.due();
    // **The splice rides the throttle's gate** (`docs/design/decisions.md`,
    // "D62"). Nothing between gate openings needs `index.spans` current: an
    // interrupt's save persists the last spliced map, consistent as it
    // stands, the metadata recompute in the `CopyStart` arm splices its own
    // copy, and
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
/// save unconditionally, an interrupt before the first read included.
///
/// **The gate also decides when the map is rebuilt.** `stream::splice` fires
/// at the openings of this gate rather than at every `CopyEnd` (see
/// [`map_forward`]'s `CopyEnd` arm). With a disabled cache `save` is ~free, so
/// the gate always clears and the map is rebuilt per block — the residual
/// `KD5` names.
pub(super) struct SaveThrottle {
    last_save: Instant,
    last_cost: Duration,
}

impl SaveThrottle {
    /// Starts due: `last_cost` is zero, so the first block of a segment always
    /// banks, rather than leaving a resumable frontier never written down.
    pub(super) fn new() -> Self {
        Self { last_save: Instant::now(), last_cost: Duration::ZERO }
    }

    /// The rule itself, over measured quantities rather than clocks.
    pub(super) fn due_after(elapsed: Duration, last_cost: Duration) -> bool {
        elapsed >= last_cost.saturating_mul(SAVE_THROTTLE_K)
    }

    pub(super) fn due(&self) -> bool {
        Self::due_after(self.last_save.elapsed(), self.last_cost)
    }

    /// Save, and time the save — that duration is the whole input to the next
    /// decision. A disabled cache makes this ~free and so never throttles.
    pub(super) async fn save(
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
    /// index and the cache agree either way, but after an interrupt inside the
    /// preamble prepass, which writes nothing; what differs is whether the
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
    /// **How many blocks the map already held that no strict parse had
    /// checked were re-read to check them**, under
    /// [`PostgresInvalidValues::Strict`] ([`CopyBlock::checked_in_full`](pgdt_model::CopyBlock::checked_in_full)) —
    /// zero under every other mode, and for an interrupted run, whatever it
    /// checked and banked before the interrupt.
    pub checked: usize,
    /// **How many blocks of the finished map declined to gather statistics**,
    /// under this run's allowance or an earlier, larger one
    /// ([`pgdt_model::CopyBlock::statistics_declined`]) — zero for a run the
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
/// only a scan reaching EOF may state; `pgdt_query::build_index` stays the
/// eager, cache-blind producer. Rejected: teaching *it* to resume, which
/// duplicates the splice-onto-a-prefix logic here.
///
/// **It opens with the bounded preamble prepass** [`table_stream`](super::table::table_stream) runs, under
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
/// same per-block re-read [`gather_block_statistics`](super::backfill::gather_block_statistics) exposes, saving as it
/// goes and charging the run's own account. The back-fill runs once the
/// map has reached EOF, so each block resolves against whole-file metadata,
/// and a run interrupted inside it resumes into it, the blocks still lacking
/// being counted afresh. [`StatisticsRequest::default`] is the data level
/// everywhere. **Under [`PostgresInvalidValues::Strict`] it checks every field
/// it maps, and re-reads every block it holds that no strict parse checked**
/// ([`CopyBlock::checked_in_full`](pgdt_model::CopyBlock::checked_in_full)) — in the back-fill, or before mapping on
/// where it resumes short of EOF.
///
/// **The three finishing steps are this function's.**
///
/// - `metadata` is recomputed over the whole span list. EOF is the other
///   boundary [`dump_metadata_from_spans`] may be called at (I1), covering a
///   trailing database with no `COPY` block of its own, and a file with no
///   blocks at all.
/// - `diagnostics` are the tiling and TOC-coverage figures, recomputed
///   rather than inherited, being `#[serde(skip)]` — [`map_forward`]
///   recomputes them at its own EOF exit too — and what is this function's
///   is keeping what [`CacheMode::load`] reported about the cache file's
///   identity ahead of them.
/// - The cache is saved once more at the end, persisting the finished index.
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
    // against what this run started from (`pgdt_dump::cache::SourceWatch`).
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
        // Refused before the dump is read past its magic, unless the caller said
        // this cache may be replaced (`docs/design/decisions.md`, "D20").
        CacheLoad::Unusable(unusable) => match cache.refusal(&unusable) {
            Some(refusal) => return Err(refusal),
            None => DumpIndex::default(),
        },
    };
    refuse_recorded(scan_options, cache, statistics, &index)?;
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
    // the load said about its identity; of the rest, tiling and TOC coverage
    // are recomputed below.
    //
    // Deficiency register: `deficiency: KD58` — the non-seekable warning the
    // load adds is dropped here and never recomputed, so this function's index
    // lacks the `NonSeekableCompressedSource` [`build_index`] and a loaded
    // cache carry, and `pgdt parse` of a one-block `.xz` does not list it
    // where `pgdt info` does. **(c) unowned**; promoted by a user who parses
    // such a file and is not told, the fix being one more recomputed figure.
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
            // there being no save left to ride ([`pgdt_dump::cache::SourceWatch`]).
            Err(e) if cancelled_read(&e, scan_options) => {
                watch.check(source).await?;
                return Ok(interrupted_run(index, resumed_from, &account));
            }
            Err(e) => return Err(e),
        }
    }

    // **A strict parse resuming a map short of EOF checks the blocks it holds
    // first**, so the first field it refuses is the first in file order; over
    // a map at EOF the back-fill below checks them, in the read gathering any
    // statistics they lack.
    let mut checked_before = 0;
    if resumed_from < size {
        let checked = backfill_statistics(
            source,
            scan_options,
            cache,
            watch,
            &mut index,
            statistics,
            &account,
            size,
            HashSet::new(),
            Backfill::Unchecked,
        )
        .await?;
        if checked.interrupted {
            watch.check(source).await?;
            return Ok(interrupted_run(index, resumed_from, &account));
        }
        // Banked before mapping on, which may stop at a refusal.
        if checked.checked > 0 {
            cache.save(watch, source, &index).await?;
        }
        checked_before = checked.checked;
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
        // (`pgdt_dump::cache::SourceWatch`).
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
        Backfill::Lacking,
    )
    .await?;
    let checked = checked_before + backfill.checked;
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
        checked,
        declined_statistics,
        statistics: announce_statistics_held(&account),
    })
}

/// What [`map_file`] returns for a run that stopped before EOF: the map as far
/// as it is consistent, and **none of the three counts a back-fill states** —
/// a strict parse's check of a map short of EOF may have re-read and banked
/// blocks before the interrupt, and the caller is told the map is short
/// rather than what was re-read.
///
/// The caller banks first where there is anything to bank — the prepass's
/// interrupt has nothing — whether that is the pass's own save at a check
/// point or one made where an unwinding read was caught.
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
        checked: 0,
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

/// **Fail a parse refusing the fields PostgreSQL refuses at the first such
/// field `index` records an earlier parse went past**
/// ([`CopyBlock::ignored_refusals`](pgdt_model::CopyBlock::ignored_refusals)), in file order, in a column `statistics`
/// tracks — before anything is read, the cache being as much the dump's
/// account as a re-read of the block would be, so the verdict does not depend
/// on which run gathered it. A column the request leaves at the metadata
/// level is one a refusing parse would not key, and is passed over; among the
/// rest, the field quoted is the one a read would have met first.
///
/// **A strict parse does not read the record**: it keys every column, where
/// the record holds only what an ignoring parse's gathering keyed, so it
/// re-reads the block instead, as it re-reads every block no strict parse
/// checked, and fails at the first field there ([`backfill_statistics`]).
fn refuse_recorded(
    scan_options: &ScanOptions,
    cache: &CacheMode,
    statistics: &StatisticsRequest,
    index: &DumpIndex,
) -> Result<()> {
    if scan_options.postgres_invalid_values != PostgresInvalidValues::Default {
        return Ok(());
    }
    // Only a cache read from a file can hold a record.
    let CacheMode::Enabled { path, .. } = cache else { return Ok(()) };
    let refused = index.blocks().find_map(|block| {
        let recorded = block.ignored_refusals.as_deref()?;
        let tracked = statistics.tracked_columns(&block.header)?;
        recorded.error_for(&tracked, &block.header, block.data_offset, path)
    });
    refused.map_or(Ok(()), Err)
}

/// Merge what a re-read of a block went past into the block's record
/// ([`IgnoredRefusals::merge`]): a column the re-read gathered whole holds
/// its full count either way, and one it did not, or gathered only up to a
/// decline, keeps what the fuller read met.
pub(super) fn merge_ignored(
    record: &mut Option<Box<IgnoredRefusals>>,
    reread: Option<IgnoredRefusals>,
) {
    let mut merged = record.take().map(|record| *record);
    IgnoredRefusals::merge(&mut merged, reread);
    *record = merged.map(Box::new);
}
