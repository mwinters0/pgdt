//! A map extended from its frontier (`docs/design/decisions.md`, "D48"):
//! each `COPY` block closed, its open region offered to the interior split,
//! and the cache saved when a save has earned its cost.

use std::sync::Arc;

use pgdt_codec::internal::counter_for;
use pgdt_core::{CopyHeader, Error, Result};
use pgdt_dump::attach_text;
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_dump::dump_metadata_from_spans;
use pgdt_dump::internal::Builder;
use pgdt_dump::internal::announce_read_loop;
use pgdt_dump::internal::derived_diagnostics;
use pgdt_dump::{ChunkedScanner, CopyEnd, CopyScanner, Event, ScanOptions};
use pgdt_io::{ByteRangeSource, Extent, internal::memory_budget_display};
use pgdt_model::{DumpIndex, PostgresInvalidValues, Span, SpanBody};
use tokio_util::sync::CancellationToken;

use crate::interior::{CensusPlan, RegionScan, report_shortfall, scan_region};
use crate::statistics::account::StatisticsAccount;
use crate::statistics::gather;
use crate::statistics::request::StatisticsRequest;

mod save;

pub(crate) use self::save::SaveThrottle;

/// Replace everything a mapping scan covered with what it built: `prefix`
/// (the spans that already tiled `[0, seg_start)`, untouched) followed by
/// `built` (a complete tiling of `[seg_start, watermark)` from
/// [`pgdt_dump::internal::Builder`]), plus the trailing [`SpanBody::Unscanned`] span
/// that makes the result tile the whole file even though the scan stopped
/// early (`docs/design/decisions.md`, "D30") — where the file's end, `end`,
/// is known: a map of a source whose end is not yet read ends at `watermark`.
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
/// throttle's gate seldom closes, which is every `--dtcache none` scan, a
/// no-op save closing it only for blocks that end within
/// `forward::save`'s `SAVE_THROTTLE_K` times its own measured duration
/// (`measurements.md`, `per-block-quadratic`). **(c) unowned**; promoted by a
/// dump of thousands of blocks scanned that way. Closing it means an
/// appendable frontier here rather than a rebuild; a floor under the throttle
/// was refused, since it makes the cache's rate govern the map's cost.
pub fn splice(
    prefix: &[Span],
    mut built: Vec<Span>,
    seg_start: u64,
    watermark: u64,
    end: Option<u64>,
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
    if let Some(end) = end.filter(|&end| watermark < end) {
        spans.push(Span {
            start: watermark,
            end,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: SpanBody::Unscanned,
        });
    }
    spans
}

/// The table a query's pass may stop early for, as the query states it: whether
/// a block, in the database its span records, holds rows for it. A test of
/// the caller's, so the pass names no selector of its own.
pub type Target<'a> = &'a (dyn Fn(Option<&str>, &CopyHeader) -> bool + Sync);

/// Whether `index`'s map now answers the query for good, so the mapping scan
/// can stop short of EOF (`docs/design/decisions.md`, "D49"). Two things can
/// make a further block share the queried name, and both are ruled out:
///
/// - **A partition-root marker on a matching block**, whose `COPY` header
///   names the partition's **root**: other blocks carry the same name and are
///   not adjacent to it (I2), so only reaching EOF enumerates them.
/// - **Any `\connect` at all**, the file then being a `pg_dumpall`, a
///   concatenation or a `--create` dump, where a qualified name can be defined
///   again in a later database (I2). A target naming its database does not
///   lift this: two `\connect` segments can name the *same* database.
///
/// What neither catches is a file whose *first* segment has no `\connect` —
/// deficiency `KD6`, detailed at its marker
/// (`pgdt_query::ScanExtent::UntilTargetSettled`).
fn target_settled(index: &DumpIndex, target: Target<'_>) -> bool {
    if index.spans.iter().any(|s| matches!(s.body, SpanBody::Connect { .. })) {
        return false;
    }
    let mut matched = false;
    for (database, block) in index.blocks_with_database() {
        if !target(database, &block.header) {
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
pub enum MapStop {
    /// EOF, or the queried table settled.
    Reached,
    /// The pass's cancellation fired, which is the only thing that answers
    /// this. The index's spans are consistent at the last spliced watermark,
    /// its metadata perhaps restated at a database's first block past it, and
    /// it has been persisted; everything past the watermark is unscanned.
    Interrupted,
}

/// Whether this error is the interrupt arriving by another door.
///
/// A source whose wait is a request rather than a `pread` answers a
/// cancellation by dropping the read in flight, so what a polled check point
/// would have seen as a set flag reaches its caller as a failed read instead
/// (`docs/design/decisions.md`, "D26"). The token is what tells the two
/// apart: a `ScanCancelled` nobody asked for is a source misreporting itself
/// and stays an error.
pub(crate) fn cancelled_read(error: &Error, cancel: &CancellationToken) -> bool {
    matches!(error, Error::ScanCancelled { .. }) && cancel.is_cancelled()
}

/// What a mapping pass records of each block it maps
/// (`docs/design/decisions.md`, "D77").
#[derive(Clone, Copy)]
pub enum Pass<'a> {
    /// [`map_file`](crate::map_file)'s: each block at the level `.0` gives its table, censused
    /// where that is the data level, and its data-level columns gathered,
    /// every observer charging `.1` and treating a field its type's `*_in`
    /// refuses as `.2` says ([`PostgresInvalidValues`]).
    Parse(&'a StatisticsRequest, &'a Arc<StatisticsAccount>, PostgresInvalidValues),
    /// A query's: every block censused, nothing gathered, whatever level a
    /// parse would have given it (`docs/design/decisions.md`, "D35").
    Query,
}

/// Extend `index`'s map forward from its own `scanned_through`, persisting as
/// it goes, until the queried table is settled, EOF is reached, or `cancel`
/// fires. Emits no rows. `target` is the
/// table a query may stop early for once
/// [`target_settled`] says so; **`None` means "run to EOF"**, what
/// `pgdt_query::ScanExtent::Full` asks for and what [`map_file`](crate::map_file) always wants. Rejected:
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
/// database's first `COPY` block** — the first after a change of name, a
/// segment naming the database before it restating nothing (`KD133`) — which per I1 is one of the two boundaries
/// [`dump_metadata_from_spans`] may be called at, and the only one this loop
/// stands on.
///
/// **`mapping` says what to record of each block** ([`Pass`]): [`map_file`](crate::map_file)'s
/// records what its request's levels ask, a query's censuses every block and
/// gathers nothing. A block gathering statistics is observed row by row on
/// this loop, or piece by piece where the leader takes it, every observer
/// charging the account [`Pass::Parse`] carries.
#[allow(clippy::too_many_arguments)]
pub async fn map_forward(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cancel: &CancellationToken,
    cache: &CacheMode,
    watch: &SourceWatch,
    index: &mut DumpIndex,
    target: Option<Target<'_>>,
    mapping: Pass<'_>,
    extent: Extent,
) -> Result<MapStop> {
    if extent.reached_by(index.scanned_through) {
        return Ok(MapStop::Reached);
    }
    if target.is_some_and(|target| target_settled(index, target)) {
        return Ok(MapStop::Reached);
    }

    // The two checks above are "nothing to do" and earn no line
    // (`docs/manual/dump-inspection.md`, "`parse`: reading the dump"). Past
    // here a real scan runs, at whatever arrangement `--jobs` and the stated
    // budget resolved to (`docs/design/decisions.md`, "D64").
    tracing::info!(
        bytes = %extent,
        resumed_from = index.scanned_through,
        chunk_size = scan_options.read.chunk_size_bytes,
        jobs = scan_options.parallelism.jobs(),
        memory_bytes = %memory_budget_display(scan_options.parallelism),
        "scan started",
    );

    let seg_start = index.scanned_through;
    let prefix: Vec<Span> = index.spans.iter().filter(|s| s.end <= seg_start).cloned().collect();
    // The prefix tiles `[0, seg_start)`, so its last span ends exactly at the
    // frontier and its `database` is the one in scope there. With no prefix
    // the frontier is byte zero — a preamble of no bytes, or a prepass
    // skipped for a first preamble already known — and I1 puts it inside the
    // first database the metadata captured.
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

    // **This loop grants no wait** (`docs/design/decisions.md`, "D5"):
    // [`scan_region`] grants it around an interior region's workers and
    // restores this policy on the way out.
    announce_read_loop(source, scan_options.read, scan_options.parallelism);
    let mut chunked = ChunkedScanner::to_source_end(
        CopyScanner::resume(seg_start, None),
        extent,
        scan_options.read,
    );
    // Where the file ends, wherever the source has said so: one whose end is
    // not known yet has its map end at each watermark ([`splice`]), and is
    // asked again after each chunk, a read being where it can meet the end —
    // so a map saved after it carries the tail the saved extent states.
    let mut file_end = extent.exact();
    let mut throttle = SaveThrottle::new();
    // Whether the open `COPY` block is one `target_settled` would count — see
    // the `CopyEnd` arm and the `CopyStart` arm's closed region, its readers.
    let mut open_block_targets = false;
    // Whether `scan started`'s `jobs=` has already been corrected for this
    // scan — see [`report_shortfall`], its only reader and writer.
    let mut shortfall_reported = false;

    loop {
        // Once per chunk, the `CopyEnd` arm carrying the other check
        // (`docs/design/decisions.md`, "D63").
        // `index`'s spans are consistent at the last *spliced* watermark
        // whatever the buffer holds, so the save needs no snapshot logic of
        // its own.
        if cancel.is_cancelled() {
            cache.save(watch, source, index).await?;
            return Ok(MapStop::Interrupted);
        }
        let mut chunk = match chunked.next_chunk(source).await {
            Ok(chunk) => chunk,
            // The check point above, reached through the read rather than
            // through the token ([`cancelled_read`]): the same chunk and the
            // same watermark, so this banks what that would have banked.
            Err(e) if cancelled_read(&e, cancel) => {
                cache.save(watch, source, index).await?;
                return Ok(MapStop::Interrupted);
            }
            Err(e) => return Err(e),
        };
        if file_end.is_none() {
            file_end = source.extent().await?.exact();
        }

        // Where the leader closed a block the workers scanned, and so where
        // the serial scanner has to be put back down. `None` for a chunk no
        // region was taken out of.
        let mut resume_at: Option<u64> = None;
        while let Some(event) = chunk.next_event()? {
            match event {
                Event::CopyStart(start) => {
                    // The current database's first `COPY` block is the
                    // recurring one of the two boundaries
                    // `dump_metadata_from_spans` may be called at (I1).
                    // Retreat to a pending TOC comment's own start as
                    // `pgdt_dump::internal::scan_preamble` does, so the span list
                    // ends where the `Data` span is about to begin.
                    let boundary = builder.pending_comment_start().unwrap_or(start.header_offset);
                    // Read off the header, for the offer below.
                    let header_offset = start.header_offset;
                    let data_offset = start.data_offset;
                    // The block's level: censused, and gathering where
                    // a statistics request tracks its columns.
                    let (census, request) = match mapping {
                        Pass::Query => (true, None),
                        Pass::Parse(request, account, invalid) => (
                            request.tracked_columns(&start.header).is_some(),
                            Some((request, account, invalid)),
                        ),
                    };
                    let header = start.header.clone();
                    builder.on_copy_start(*start, census);
                    // **Once per database, not once per block**:
                    // recomputing at every `CopyStart` would put a third
                    // whole-index-sized cost in this loop.
                    let db = builder.database().map(str::to_owned);
                    // Whether this block can be the one that settles
                    // `target` — the `CopyEnd` arm's reason to splice for
                    // a block the throttle would have skipped. **The
                    // target's own test alone**, a superset of
                    // `target_settled`'s, whose partition-root and
                    // `\connect` vetoes need the map.
                    open_block_targets =
                        target.is_some_and(|target| target(db.as_deref(), &header));
                    if metadata_covers.as_ref() != Some(&db) {
                        let spans = splice(
                            &prefix,
                            builder.snapshot(boundary),
                            seg_start,
                            boundary,
                            file_end,
                        );
                        index.metadata = Some(Arc::new(dump_metadata_from_spans(&spans)));
                        metadata_covers = Some(db.clone());
                    }
                    // After the restatement, so the count and the
                    // observer resolve the block against its database's
                    // DDL — the first segment of its name's (`KD133`).
                    let plan = census.then(|| {
                        let counter =
                            counter_for(&header, index.metadata.as_deref(), db.as_deref());
                        builder.count_block(Arc::clone(&counter));
                        CensusPlan { width: header.columns.len(), counter }
                    });
                    // A block no request tracks is observed too under
                    // `PostgresInvalidValues::Strict`, to check its fields.
                    let observer = request.and_then(|(request, account, invalid)| {
                        gather::observer_for(
                            request,
                            &header,
                            index.metadata.as_deref(),
                            db.as_deref(),
                            account,
                            invalid,
                        )
                    });
                    if let Some(observer) = observer {
                        builder.observe_block(observer);
                    }
                    // **The offer, and this loop is the leader making it**
                    // ([`scan_region`]): everything from
                    // `data_offset` until `\.` is line-structured rows, so
                    // the region may be handed to workers that never parse
                    // structure. It answers the block's totals as the
                    // serial scanner would have, or declines, or fails on
                    // what follows the block (`KD134`) — and a
                    // block it closes holds the statistics its pieces
                    // observed, as a declined one does the rows this loop
                    // hands on.
                    let outcome = scan_region(
                        source,
                        scan_options,
                        cancel,
                        &header.qualified_name(),
                        header_offset,
                        data_offset,
                        plan,
                        extent,
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
                                cancel,
                                cache,
                                watch,
                                index,
                                &mut builder,
                                &mut throttle,
                                &prefix,
                                seg_start,
                                file_end,
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
                        cancel,
                        cache,
                        watch,
                        index,
                        &mut builder,
                        &mut throttle,
                        &prefix,
                        seg_start,
                        file_end,
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
                Event::LargeObjectStart(start) => builder.on_large_object_start(start.start_offset),
                Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
            }
        }

        // **The leader took a region, so the serial scanner is put back down
        // past it**, `end_offset` being a line start past every byte the
        // carry held.
        if let Some(at) = resume_at {
            chunked.skip_to(at);
            // The three hints this loop announced still stand: `scan_region`
            // restores `WaitPolicy::NeverWait` on its way out, announces the
            // same `Parallelism`, and never touches the read size.
            continue;
        }

        if chunked.at_end() {
            break;
        }
    }

    // **The text is attached before the index advances**, so a read dropped
    // here ([`cancelled_read`]) leaves the map at the watermark it already
    // held rather than at one whose spans lost their text with nothing that
    // would ever re-attach it — a resumed run past `scanned_through` re-reads
    // nothing.
    let roles: Vec<String> = builder.roles().iter().cloned().collect();
    let tablespaces: Vec<String> = builder.tablespaces().iter().cloned().collect();
    let size = chunked.end().expect("a scan out of its loop reached the end");
    let mut spans = splice(&prefix, builder.finish(size), seg_start, size, Some(size));
    match attach_text(source, &mut spans).await {
        Ok(()) => {}
        Err(e) if cancelled_read(&e, cancel) => {
            cache.save(watch, source, index).await?;
            return Ok(MapStop::Interrupted);
        }
        Err(e) => return Err(e),
    }
    index.roles.extend(roles);
    index.tablespaces.extend(tablespaces);
    index.spans = spans;
    index.scanned_through = size;
    index.diagnostics =
        derived_diagnostics(&index.spans, size, source.compression_index().as_ref());
    // deficiency: KD131 — this saves a complete map, its `scanned_through` the
    // file's end, behind `SourceWatch::check` alone: the reads' own checks
    // (`SourceWatch::finish`, a source's `complete_reads`) come only before
    // `map_file`'s last save, after the back-fill's saves of this same map. So
    // a block whose check a later completion would fail can stand in a
    // complete map on disk; the fix is a save of a complete map that finishes
    // the watch first. **(c) unowned**; promoted by a source whose
    // `complete_reads` refuses what the scan already handed out.
    cache.save(watch, source, index).await?;
    // The true end of the file, as opposed to the early
    // `Ok(MapStop::Reached)`s above: one for a map already there, three
    // stopping a query at its settled target.
    tracing::info!(bytes = size, reached_eof = true, "scan complete");
    Ok(MapStop::Reached)
}

/// What closing one `COPY` block asks of the loop that closed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockClose {
    /// Nothing: carry on scanning.
    Continue,
    /// The queried table is settled, and the map has been persisted at this
    /// block's watermark.
    Settled,
    /// The pass's cancellation fired, and the map has been persisted at this
    /// block's watermark.
    Interrupted,
}

/// Close a `COPY` block on the [`pgdt_dump::internal::Builder`] and do everything
/// [`map_forward`] owes at a `CopyEnd`: the splice, the throttled save, and the
/// early-stop check. **One body, two call sites** — the serial scanner's
/// `CopyEnd` and a region [`scan_region`] closed
/// (`docs/design/decisions.md`, "D52") — and
/// the ordering it holds is splice before the settled test, save before the
/// return. `targets` is whether this block's header could be the one that
/// settles `target`, read at `CopyStart`.
#[allow(clippy::too_many_arguments)]
async fn close_copy_block(
    source: &dyn ByteRangeSource,
    cancel: &CancellationToken,
    cache: &CacheMode,
    watch: &SourceWatch,
    index: &mut DumpIndex,
    builder: &mut Builder,
    throttle: &mut SaveThrottle,
    prefix: &[Span],
    seg_start: u64,
    file_end: Option<u64>,
    target: Option<Target<'_>>,
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
    let cancelled = cancel.is_cancelled();
    let due = throttle.due();
    // **The splice rides the throttle's gate** (`docs/design/decisions.md`,
    // "D62"). Nothing between gate openings needs `index.spans` current: an
    // interrupt's save persists the last spliced map, consistent as it
    // stands, the metadata recompute in the `CopyStart` arm splices its own
    // copy, and
    // `target_settled` is the one reader that would — which is why a block
    // whose header could satisfy it opens the gate too.
    if targets || cancelled || due {
        index.spans = splice(prefix, builder.snapshot(watermark), seg_start, watermark, file_end);
        index.roles.extend(builder.roles().iter().cloned());
        index.tablespaces.extend(builder.tablespaces().iter().cloned());
        index.scanned_through = index.scanned_through.max(watermark);
    }
    // Only a block `target_settled` counts can turn it from false to true, and
    // `index` has just been spliced for exactly those — so this always reads a
    // map current through `watermark`.
    let settled = targets && target.is_some_and(|target| target_settled(index, target));
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
