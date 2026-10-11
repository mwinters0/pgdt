//! Re-reading blocks the map already holds: the statistics back-fill a pass
//! makes after EOF, its reports, and the one-block re-read a caller asks for.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;

use pgdt_codec::internal::counter_for;
use pgdt_core::internal::StatisticsScope;
use pgdt_core::{Error, Result};
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_dump::internal::announce_cancellation;
use pgdt_dump::{ChunkCarry, CopyEnd, CopyScanner, Event, ScanOptions};
use pgdt_io::{ByteRangeSource, WaitPolicy};
use pgdt_model::internal::{BlockCensusExt, BlockStatisticsExt, FieldRefusalExt};
use pgdt_model::{
    BlockCensus, BlockGathered, BlockStatistics, CopyBlock, DataBlock, DumpIndex, DumpMetadata,
    IgnoredRefusals, PostgresInvalidValues, SpanBody, internal::BlockObserver,
};

use crate::forward::{SaveThrottle, cancelled_read, report_shortfall};
use crate::interior::{CensusPlan, RegionScan, scan_region};
use crate::statistics::account::{StatisticsAccount, Term};
use crate::statistics::gather;
use crate::statistics::request::{StatisticsBackfill, StatisticsRequest};

/// What [`backfill_statistics`] did.
pub(crate) struct BackfillRun {
    pub(crate) lacking: usize,
    pub(crate) reread: usize,
    /// The blocks re-read only because no strict parse had checked them, or
    /// also for that.
    pub(crate) checked: usize,
    pub(crate) interrupted: bool,
}

/// Which blocks [`backfill_statistics`] re-reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backfill {
    /// Every block lacking what the request asks for, and under
    /// [`PostgresInvalidValues::Strict`] every block no strict parse checked:
    /// the pass once the map has reached EOF.
    Lacking,
    /// Only the blocks no strict parse checked, and only to check them: the
    /// pass a strict parse resuming a map short of EOF runs over the blocks
    /// it holds before mapping on, so the first field it refuses is the first
    /// in file order.
    Unchecked,
}

/// Re-read every block of a map that has reached EOF which lacks what
/// `statistics` asks for, in file order, storing each block's statistics as it
/// closes and saving through a [`SaveThrottle`] as [`map_forward`](crate::forward::map_forward) does; an
/// interrupt saves and stops. Announces how many blocks lacked them and, once
/// every one is re-read, how many were — and nothing at all when none did.
///
/// Deficiency register: `deficiency: KD37` — that is true of the flag and of a
/// dropped read this pass makes itself ([`observe_rows`]), and not of one the
/// leader dispatched: [`reread_block`]'s [`scan_region`] propagates, so a
/// source that answers a cancellation by failing its read ends a `parse` as
/// `Error::ScanCancelled`, banking nothing since the throttle's last save, where the same Ctrl-C during the
/// mapping pass is an interrupted run ([`cancelled_read`],
/// `docs/design/decisions.md`, "D26"). The fix is the arm [`map_file`](crate::map_file) already
/// carries for the pass's own leader reads, here as well. **(c) unowned**;
/// promoted by a parallel remote `parse` seen to error on Ctrl-C after its map
/// reached EOF, or in a strict one's check of a map short of it, which
/// [`map_file`](crate::map_file) runs through here before mapping on.
///
/// **A block lacking statistics was loaded, or broke a stated maximum**: a
/// block this pass mapped holds what its own request asked, except for a
/// maximum no single read can deliver. So what a re-read replaces leaves the
/// term that carried it — `loaded`, the header offsets the cache supplied
/// statistics for, telling the two apart.
///
/// **Under [`PostgresInvalidValues::Strict`] a block no strict parse checked
/// is re-read too**, checking every field and recording that it did
/// ([`CopyBlock::checked_in_full`]) — in the same read where it lacks
/// statistics as well — so a clean strict run has checked every field
/// whichever runs built the map. `which` says whether this is that pass alone
/// ([`Backfill::Unchecked`]).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn backfill_statistics(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache: &CacheMode,
    watch: &SourceWatch,
    index: &mut DumpIndex,
    statistics: &StatisticsRequest,
    account: &Arc<StatisticsAccount>,
    size: u64,
    mut loaded: HashSet<u64>,
    which: Backfill,
) -> Result<BackfillRun> {
    // Positions into `index.spans`, which nothing below adds to or reorders.
    let metadata = index.metadata.as_ref();
    let strict = scan_options.postgres_invalid_values == PostgresInvalidValues::Strict;
    let gathers = statistics.gathers() && which == Backfill::Lacking;
    let rereads: Vec<(usize, Option<StatisticsBackfill>, bool)> = index
        .spans
        .iter()
        .enumerate()
        .filter_map(|(at, span)| {
            let SpanBody::Data(DataBlock::Copy(block)) = &span.body else { return None };
            let backfill = gathers
                .then(|| {
                    statistics.backfill(
                        block,
                        &bounded_columns(block, metadata),
                        scan_options.statistics_allowance_bytes,
                    )
                })
                .flatten();
            let unchecked = strict && !block.checked_in_full;
            (backfill.is_some() || unchecked).then_some((at, backfill, unchecked))
        })
        .collect();
    let lacking = rereads.iter().filter(|(_, backfill, _)| backfill.is_some()).count();
    let unchecked = rereads.iter().filter(|(.., unchecked)| *unchecked).count();
    let mut run = BackfillRun { lacking, reread: 0, checked: 0, interrupted: false };
    if rereads.is_empty() {
        return Ok(run);
    }
    if lacking > 0 {
        tracing::info!(blocks = lacking, "statistics back-fill started");
    }
    if unchecked > 0 {
        tracing::info!(blocks = unchecked, "check of blocks no strict parse checked started");
    }
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
    let blocks = rereads.len();
    for (done, (at, backfill, unchecked)) in rereads.into_iter().enumerate() {
        // **At most two reads of a block**, the second being the finer size a
        // stated maximum a block broke at the size it gathered from asks for
        // ([`StatisticsRequest::backfill`]); nothing asks for a third, and the
        // bound here is what says so. `Some(None)` is a read checking the
        // block alone.
        let gathering = backfill.is_some();
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
                backfill.as_ref(),
                account,
                size,
                &mut shortfall_reported,
            )
            .await?;
            let Some(BlockReread { gathered, census, ignored, checked }) = gathered else {
                cache.save(watch, source, index).await?;
                run.interrupted = true;
                return Ok(run);
            };
            if let SpanBody::Data(DataBlock::Copy(block)) = &mut index.spans[at].body {
                // Every row was read, so a block gathering is at the data
                // level whether its statistics fitted or not; a check alone
                // records only that it checked, as a strict mapping pass
                // records nothing more of a block it was not asked to.
                if backfill.is_some() {
                    block.set_census(census);
                }
                block.checked_in_full |= checked;
                let _attributed = StatisticsScope::enter();
                match gathered {
                    // **A re-read that declined keeps what the block already
                    // held**: the observer freed its own and holds nothing to
                    // replace them with, so the block records the allowance
                    // beside whatever an earlier pass gathered
                    // (`docs/design/decisions.md`, "D85").
                    BlockGathered::Declined { allowance } => {
                        block.statistics_declined = Some(allowance);
                        merge_ignored(&mut block.ignored_refusals, ignored);
                    }
                    BlockGathered::Refused(_) => unreachable!("a re-read fails on a refusal"),
                    BlockGathered::Checked => {}
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
                        merge_ignored(&mut block.ignored_refusals, ignored);
                        account.apply(&[(term, -(replaced as i64))]);
                    }
                }
                if backfill.is_some() {
                    plan = statistics
                        .backfill(
                            block,
                            &bounded_columns(block, index.metadata.as_ref()),
                            scan_options.statistics_allowance_bytes,
                        )
                        .map(Some);
                }
            }
        }
        run.reread += usize::from(gathering);
        run.checked += usize::from(unchecked);
        // Both of `map_forward`'s check points, a block's close being the
        // second (`docs/design/decisions.md`, "D63").
        if scan_options.cancelled() && done + 1 < blocks {
            cache.save(watch, source, index).await?;
            run.interrupted = true;
            return Ok(run);
        }
        if throttle.due() {
            throttle.save(cache, watch, source, index).await?;
        }
    }
    if lacking > 0 {
        tracing::info!(blocks = run.reread, "statistics back-fill complete");
    }
    if unchecked > 0 {
        tracing::info!(blocks = run.checked, "check of blocks no strict parse checked complete");
    }
    Ok(run)
}

/// **Say which blocks still hold more rows than a stated maximum asks for**,
/// one line each: a block too dense for it is re-read at the size its own
/// groups predicted, and one whose rows cluster can miss it there. Nothing
/// reads such a block again, so every run under that maximum says what it
/// keeps rather than what it will fix.
pub(crate) fn report_density_shortfall(statistics: &StatisticsRequest, index: &DumpIndex) {
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
/// [`MapRun::declined_statistics`](crate::MapRun::declined_statistics) carries.
///
/// **Every run under an allowance says it, not only the run that declined**: a
/// block's decline is recorded in the map ([`CopyBlock::statistics_declined`])
/// and re-read only under a larger allowance, so the run that skips one is the
/// run that has to say what it is leaving and what would fix it
/// (`docs/design/decisions.md`, "D85"). A pass that gathers nothing says
/// nothing: the blocks it did not ask about are none of its business.
pub(crate) fn report_statistics_declines(
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
/// (`pgdt_dump::scan`, [`map_forward`](crate::forward::map_forward)): its chunk length, its budget, the
/// caller's cancellation where there is one, and that it grants no wait.
pub fn announce_read_loop(source: &dyn ByteRangeSource, scan_options: &ScanOptions) {
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
    /// The fields its type's `*_in` refuses that the re-read keyed and went
    /// past, per column — under [`pgdt_model::PostgresInvalidValues::Ignore`], or
    /// in a column gathered only because the block held it
    /// ([`StatisticsBackfill::requested`]) — which a caller merges into
    /// [`CopyBlock::ignored_refusals`] ([`IgnoredRefusals::merge`]).
    pub ignored: Option<IgnoredRefusals>,
    /// Whether the re-read checked every field of the block and found none
    /// refused, as it does under [`pgdt_model::PostgresInvalidValues::Strict`] —
    /// which a caller records as [`CopyBlock::checked_in_full`].
    pub checked: bool,
}

/// Re-read one block the map already holds and gather what `backfill` names —
/// the public entry point onto the same per-block re-read [`map_file`](crate::map_file)'s
/// back-fill runs over every block lacking what its request asks for, this one
/// charging no account and naming no cache. `metadata` is the map's, whole-file where
/// it can be, which the block's columns are resolved against as a mapping pass
/// resolves them; `backfill` is [`StatisticsRequest::backfill`]'s answer for
/// `block`. The caller stores the result's `gathered` in
/// [`CopyBlock::statistics`], or — for [`BlockGathered::Declined`] — the
/// allowance in [`CopyBlock::statistics_declined`]
/// (`docs/design/decisions.md`, "D85"), and its `census` through
/// [`CopyBlock::set_census`], which puts a block mapped at the metadata
/// level at the data level, and merges its `ignored` into
/// [`CopyBlock::ignored_refusals`] ([`IgnoredRefusals::merge`]).
///
/// **The block is scanned as a mapping pass scans it**: offered to the leader
/// under `scan_options`' parallelism, and read serially where it declines, so
/// what is gathered is what a straight-through pass would have gathered.
/// `Ok(None)` is [`ScanOptions::cancel`] stopping it before the block closed.
///
/// **A block that no longer ends where the map says is refused**,
/// [`Error::CachedBlockChanged`], rather than given statistics describing other
/// bytes than its map does. It names no cache, this entry point being handed
/// none; [`map_file`](crate::map_file)'s back-fill names the one it loaded. **A field its
/// type's `*_in` refuses fails the re-read** in a column `backfill`'s request
/// tracks — in every column under [`pgdt_model::PostgresInvalidValues::Strict`],
/// which checks every field ([`BlockReread::checked`]) — with
/// [`Error::FieldRefused`], as it fails a mapping pass, so the result never
/// holds [`BlockGathered::Refused`]; in any other column it is recorded
/// ([`BlockReread::ignored`]).
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
        Some(backfill),
        &Arc::default(),
        size,
        &mut shortfall_reported,
    )
    .await
}

/// [`gather_block_statistics`] once the source is announced, `size` known, and
/// with the flag [`report_shortfall`] keeps once per pass. `cache_path` is the
/// cache the map was loaded from, which a moved block's refusal names; the
/// observer charges `account`. `None` for `backfill` gathers nothing and
/// checks every field ([`gather::checker`]).
#[allow(clippy::too_many_arguments)]
async fn reread_block(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cache_path: Option<&Path>,
    metadata: Option<&DumpMetadata>,
    block: &CopyBlock,
    backfill: Option<&StatisticsBackfill>,
    account: &Arc<StatisticsAccount>,
    size: u64,
    shortfall_reported: &mut bool,
) -> Result<Option<BlockReread>> {
    let database = block.database.as_deref();
    let mut observer = match backfill {
        Some(backfill) => gather::observer_tracking(
            backfill,
            &block.header,
            metadata,
            database,
            account,
            scan_options.postgres_invalid_values,
        ),
        None => gather::checker(&block.header, metadata, database, account),
    };
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
    // as it was allocated (`pgdt_core::instrument`).
    let _attributed = StatisticsScope::enter();
    let ignored = observer.take_ignored();
    let checked = observer.checks_every_field();
    let gathered = match observer.finish(block.terminator_offset - block.data_offset) {
        BlockGathered::Refused(refusal) => {
            return Err(refusal.into_error(&block.header, block.data_offset));
        }
        gathered => gathered,
    };
    Ok(Some(BlockReread { gathered, census, ignored, checked }))
}

/// Read every row of `block` again, as a mapping pass reads them — offered
/// to the leader, and serially where it declines — censusing and counting
/// them against the DDL `metadata` states for the block, handing each to
/// `observer` where there is one, and answer the census and count:
/// what a back-fill ([`reread_block`]) and a query's census of a block mapped
/// at the metadata level (`census_metadata_level`) both read. `None` is
/// [`ScanOptions::cancel`] stopping it first; a block no longer ending where
/// the map says is [`Error::CachedBlockChanged`], naming `cache_path`.
#[allow(clippy::too_many_arguments)]
pub async fn reread_rows(
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
    let outcome = scan_region(
        source,
        scan_options,
        &block.header.qualified_name(),
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
/// [`map_forward`](crate::forward::map_forward) reads it.
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
                        census.observe_row(plan.counter.as_ref(), row.raw);
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

/// Merge what a re-read of a block went past into the block's record
/// ([`IgnoredRefusals::merge`]): a column the re-read gathered whole holds
/// its full count either way, and one it did not, or gathered only up to a
/// decline, keeps what the fuller read met.
fn merge_ignored(record: &mut Option<Box<IgnoredRefusals>>, reread: Option<IgnoredRefusals>) {
    let mut merged = record.take().map(|record| *record);
    IgnoredRefusals::merge(&mut merged, reread);
    *record = merged.map(Box::new);
}
