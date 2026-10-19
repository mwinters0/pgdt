//! Re-reading blocks the map already holds: the statistics back-fill a pass
//! makes after EOF, its reports, and the one block a caller back-fills.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use pgdt_codec::internal::counter_for;
use pgdt_core::internal::StatisticsScope;
use pgdt_core::{Error, Result};
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_dump::internal::announce_read_loop;
use pgdt_dump::{ChunkedScanner, CopyEnd, CopyScanner, Event, ReadOptions, ScanOptions};
use pgdt_io::{Arrangement, ByteRangeSource};
use pgdt_model::internal::{BlockCensusExt, FieldRefusalExt};
use pgdt_model::{
    BlockCensus, BlockGathered, BlockStatistics, CopyBlock, DataBlock, DumpIndex, DumpMetadata,
    IgnoredRefusals, PostgresInvalidValues, Span, SpanBody, internal::BlockObserver,
};
use tokio_util::sync::CancellationToken;

use crate::forward::{SaveThrottle, cancelled_read};
use crate::interior::{CensusPlan, RegionScan, report_shortfall, scan_region};
use crate::statistics::account::{StatisticsAccount, Term};
use crate::statistics::density::BlockStatisticsExt;
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
/// Deficiency register: `deficiency: KD37` — that is true of the token and of a
/// dropped read this pass makes itself ([`observe_rows`]), and not of one the
/// leader dispatched: [`Rereads::reread`]'s [`scan_region`] propagates, so a
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
    cancel: &CancellationToken,
    invalid: PostgresInvalidValues,
    allowance: Option<u64>,
    cache: &CacheMode,
    watch: &SourceWatch,
    index: &mut DumpIndex,
    statistics: &StatisticsRequest,
    account: &Arc<StatisticsAccount>,
    size: u64,
    loaded: HashSet<u64>,
    which: Backfill,
) -> Result<BackfillRun> {
    // Positions into `index.spans`, which nothing below adds to or reorders.
    let metadata = index.metadata.as_deref();
    let strict = invalid == PostgresInvalidValues::Strict;
    let gathers = statistics.gathers() && which == Backfill::Lacking;
    let rereads: Vec<(usize, Option<StatisticsBackfill>, bool)> = index
        .spans
        .iter()
        .enumerate()
        .filter_map(|(at, span)| {
            let SpanBody::Data(DataBlock::Copy(block)) = &span.body else { return None };
            let backfill = gathers
                .then(|| statistics.backfill(block, metadata, span.database.as_deref(), allowance))
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
    announce_read_loop(source, scan_options.read, scan_options.parallelism);
    let mut reader = Rereads {
        source,
        scan_options,
        cancel,
        invalid,
        // There is no cache file to name in a `CachedBlockChanged` refusal
        // unless one is enabled. A block can lack what was asked under any
        // mode — a stated maximum no single read delivers leaves a freshly
        // mapped block lacking, `CacheMode::Disabled` included (see this
        // function's doc).
        cache_path: cache.path(),
        request: statistics,
        allowance,
        account,
        size,
        loaded,
        shortfall_reported: false,
    };
    let mut throttle = SaveThrottle::new();
    let blocks = rereads.len();
    for (done, (at, backfill, unchecked)) in rereads.into_iter().enumerate() {
        let gathering = backfill.is_some();
        let DumpIndex { spans, metadata, .. } = &mut *index;
        let Span { database, body, .. } = &mut spans[at];
        let SpanBody::Data(DataBlock::Copy(block)) = body else {
            unreachable!("the positions were read off copy blocks of this span list");
        };
        if !reader.fill(metadata.as_deref(), database.as_deref(), block, backfill).await? {
            cache.save(watch, source, index).await?;
            run.interrupted = true;
            return Ok(run);
        }
        run.reread += usize::from(gathering);
        run.checked += usize::from(unchecked);
        // Both of `map_forward`'s check points, a block's close being the
        // second (`docs/design/decisions.md`, "D63").
        if cancel.is_cancelled() && done + 1 < blocks {
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
    let Some(max_rows) = statistics.density_max_rows() else { return };
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

/// **Say which blocks declined statistics an allowance could not hold**, one
/// line each, and answer how many — what
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

/// **Back-fill one block the map already holds**: re-read it until it holds
/// what `request` asks of it, and record what each read found in `block` —
/// what [`map_file`](crate::map_file)'s back-fill does to every block lacking
/// what its request asks, through the same per-block step. Its columns are
/// resolved against `metadata`, the map's, whole-file where it can be, in
/// `database`, the one its span records
/// ([`DumpIndex::blocks_with_database`](pgdt_model::DumpIndex::blocks_with_database)).
/// Answers whether it read the block: `false` is a block that holds what was
/// asked already, or one that declined under an allowance no smaller than
/// this one, and is left as it was.
///
/// **What it asks of the block is the pass's**: whatever `request` lacks
/// there, gathered beside everything the block already held so nothing is
/// lost, and read once more at a finer size where the first read still
/// breaks a stated maximum; and under [`PostgresInvalidValues::Strict`] a
/// block no strict parse checked is read to check every field too. What a
/// read found is recorded as a pass records it: the census, which puts a
/// block mapped at the metadata level at the data level; the statistics, or
/// the allowance they declined under ([`CopyBlock::statistics_declined`];
/// `docs/design/decisions.md`, "D85"); the fields a column it does not track
/// went past ([`CopyBlock::ignored_refusals`]); and a clean check
/// ([`CopyBlock::checked_in_full`]).
///
/// **The arrangement is the pass's too**: the block is offered to the leader
/// under its parallelism and read serially where it declines, so what is
/// gathered is what a straight-through pass would have gathered; and its
/// statistics allowance ([`Arrangement::statistics_allowance`]) bounds what
/// the block's statistics hold, those it already holds included, a read past
/// it declining. It takes no cancellation, being no mapping pass: a caller
/// stops it by dropping it (`docs/design/decisions.md`, "D26").
///
/// **A block that no longer ends where the map says is refused**,
/// [`Error::CachedBlockChanged`] naming no cache, this entry point being
/// handed none, rather than given statistics describing other bytes than its
/// map does. **A field its type's `*_in` refuses fails the read**, with
/// [`Error::FieldRefused`], in a column `request` tracks — in every column
/// under [`PostgresInvalidValues::Strict`] — as it fails a mapping pass,
/// unless `invalid` ignores it; in any other column it is recorded.
#[allow(clippy::too_many_arguments)]
pub async fn backfill_block(
    source: &dyn ByteRangeSource,
    read: ReadOptions,
    arrangement: &Arrangement,
    invalid: PostgresInvalidValues,
    request: &StatisticsRequest,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    block: &mut CopyBlock,
) -> Result<bool> {
    let allowance = arrangement.statistics_allowance();
    let plan = request.backfill(block, metadata, database, allowance);
    let unchecked = invalid == PostgresInvalidValues::Strict && !block.checked_in_full;
    if plan.is_none() && !unchecked {
        return Ok(false);
    }
    let scan_options = ScanOptions { read, parallelism: arrangement.parallelism() };
    let size = source.size().await?;
    announce_read_loop(source, read, scan_options.parallelism);
    // What the block holds is alive beside what a read gathers, as a
    // loaded block's statistics are in a pass, until a read replaces them.
    let account = Arc::new(StatisticsAccount::bounded_by(allowance));
    let held = block.statistics.as_deref().map_or(0, BlockStatistics::heap_bytes);
    account.apply(&[(Term::Loaded, held as i64)]);
    let loaded = block.statistics.is_some().then_some(block.header_offset).into_iter().collect();
    let cancel = CancellationToken::new();
    let mut reader = Rereads {
        source,
        scan_options: &scan_options,
        cancel: &cancel,
        invalid,
        cache_path: None,
        request,
        allowance,
        account: &account,
        size,
        loaded,
        shortfall_reported: false,
    };
    let filled = reader.fill(metadata, database, block, plan).await?;
    assert!(filled, "a re-read stops short only where its token fires, and nothing holds this one");
    Ok(true)
}

/// What every re-read of one back-fill shares: the source, how it is read,
/// what a refused field does, the cache a moved block's refusal names, what
/// the request asks and under what allowance, the account the observers
/// charge, which blocks' statistics it carries as loaded, and the flag
/// [`report_shortfall`] keeps once per pass.
struct Rereads<'a> {
    source: &'a dyn ByteRangeSource,
    scan_options: &'a ScanOptions,
    cancel: &'a CancellationToken,
    invalid: PostgresInvalidValues,
    cache_path: Option<&'a Path>,
    request: &'a StatisticsRequest,
    allowance: Option<u64>,
    account: &'a Arc<StatisticsAccount>,
    size: u64,
    /// The header offsets of the blocks whose statistics `account` carries
    /// as [`Term::Loaded`], until a re-read replaces them.
    loaded: HashSet<u64>,
    shortfall_reported: bool,
}

impl Rereads<'_> {
    /// Read `block` for `plan` — gathering what it names, or with `None`
    /// checking every field alone — and, after a gathering read, once more
    /// where the request still finds the block lacking, recording each read
    /// in the block as it closes. `false` is the token stopping a read, the
    /// block keeping what the reads before it recorded.
    async fn fill(
        &mut self,
        metadata: Option<&DumpMetadata>,
        database: Option<&str>,
        block: &mut CopyBlock,
        plan: Option<StatisticsBackfill>,
    ) -> Result<bool> {
        // **At most two reads of a block**, the second being the finer size a
        // stated maximum a block broke at the size it gathered from asks for
        // ([`StatisticsRequest::backfill`]); nothing asks for a third, and the
        // bound here is what says so. `Some(None)` is a read checking the
        // block alone.
        let mut next = Some(plan);
        for _ in 0..2 {
            let Some(plan) = next.take() else { break };
            let Some(reread) = self.reread(metadata, database, block, plan.as_ref()).await? else {
                return Ok(false);
            };
            self.record(block, reread, plan.is_some());
            if plan.is_some() {
                next = self.request.backfill(block, metadata, database, self.allowance).map(Some);
            }
        }
        Ok(true)
    }

    /// One read of `block`, gathering what `plan` names, or with `None`
    /// gathering nothing and checking every field ([`gather::checker`]); a
    /// refused field read as `invalid` says. `None` is the token stopping it.
    async fn reread(
        &mut self,
        metadata: Option<&DumpMetadata>,
        database: Option<&str>,
        block: &CopyBlock,
        plan: Option<&StatisticsBackfill>,
    ) -> Result<Option<BlockReread>> {
        let (header, account) = (&block.header, self.account);
        let mut observer = match plan {
            Some(plan) => {
                gather::observer_tracking(plan, header, metadata, database, account, self.invalid)
            }
            None => gather::checker(header, metadata, database, account),
        };
        let read = reread_rows(
            self.source,
            self.scan_options,
            self.cancel,
            self.cache_path,
            metadata,
            database,
            block,
            self.size,
            Some(observer.as_mut()),
            &mut self.shortfall_reported,
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

    /// Record in `block` what one read of it found: its census where the read
    /// `gathered` — every row was read, so a block gathering is at the data
    /// level whether its statistics fitted or not; a check alone records only
    /// that it checked, as a strict mapping pass records nothing more of a
    /// block it was not asked to — and what it gathered, or declined.
    fn record(&mut self, block: &mut CopyBlock, reread: BlockReread, gathered: bool) {
        let BlockReread { gathered: outcome, census, ignored, checked } = reread;
        if gathered {
            block.census = Some(census);
        }
        block.checked_in_full |= checked;
        let _attributed = StatisticsScope::enter();
        match outcome {
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
                let replaced = block.statistics.as_deref().map_or(0, BlockStatistics::heap_bytes);
                // The re-read's own observer has already credited
                // `Term::Retained`, so what it replaced leaves the term
                // that carried it: `Term::Loaded` for a block the cache
                // supplied, and `Term::Retained` for one this run
                // gathered — which a block re-read for a stated maximum
                // is, its own first read included.
                let term = if self.loaded.remove(&block.header_offset) {
                    Term::Loaded
                } else {
                    Term::Retained
                };
                block.statistics = Some(Arc::new(gathered));
                // A block that holds what was asked declines nothing,
                // so a record from an earlier, tighter allowance goes.
                block.statistics_declined = None;
                merge_ignored(&mut block.ignored_refusals, ignored);
                self.account.apply(&[(term, -(replaced as i64))]);
            }
        }
    }
}

/// What one read of a block the map already holds found.
struct BlockReread {
    /// What the block gathered, or the allowance it declined under.
    gathered: BlockGathered,
    /// The block's census and count, which every re-read takes, having read
    /// every row — a declined one included.
    census: BlockCensus,
    /// The fields its type's `*_in` refuses that the re-read keyed and went
    /// past, per column — under [`PostgresInvalidValues::Ignore`], or in a
    /// column gathered only because the block held it
    /// ([`StatisticsBackfill::requested`]).
    ignored: Option<IgnoredRefusals>,
    /// Whether the re-read checked every field of the block and found none
    /// refused, as it does under [`PostgresInvalidValues::Strict`].
    checked: bool,
}

/// Read every row of `block` again, as a mapping pass reads them — offered
/// to the leader, and serially where it declines — censusing and counting
/// them against the DDL `metadata` states for the block in `database`, handing each to
/// `observer` where there is one, and answer the census and count:
/// what a back-fill's read (`Rereads::reread`) and a query's census of a block mapped
/// at the metadata level (`census_metadata_level`) both read. `None` is
/// `cancel` stopping it first, and nothing else answers it; a block no
/// longer ending where the map says is [`Error::CachedBlockChanged`], naming
/// `cache_path`.
#[allow(clippy::too_many_arguments)]
pub async fn reread_rows(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    cancel: &CancellationToken,
    cache_path: Option<&Path>,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
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
        counter: counter_for(&block.header, metadata, database),
    };
    let outcome = scan_region(
        source,
        scan_options,
        cancel,
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
            match observe_rows(source, scan_options.read, cancel, block, &plan, size, observer)
                .await?
            {
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
/// `cancel` fired first, read once per chunk as
/// [`map_forward`](crate::forward::map_forward) reads it.
async fn observe_rows(
    source: &dyn ByteRangeSource,
    read: ReadOptions,
    cancel: &CancellationToken,
    block: &CopyBlock,
    plan: &CensusPlan,
    size: u64,
    mut observer: Option<&mut (dyn BlockObserver + 'static)>,
) -> Result<Option<(CopyEnd, BlockCensus)>> {
    let scanner = CopyScanner::resume(block.data_offset, Some((block.header_offset, 0)));
    let mut chunked = ChunkedScanner::new(scanner, size, read);
    let mut census = BlockCensus::new(plan.width);
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let mut chunk = match chunked.next_chunk(source).await {
            Ok(chunk) => chunk,
            // The check above, reached through the read rather than
            // through the token ([`cancelled_read`]). This block's re-read
            // is abandoned either way, so the caller hears the same
            // `None`.
            Err(e) if cancelled_read(&e, cancel) => return Ok(None),
            Err(e) => return Err(e),
        };
        while let Some(event) = chunk.next_event()? {
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
        if chunked.at_end() {
            return Err(Error::UnterminatedCopyBlock { header_offset: block.header_offset });
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

#[cfg(test)]
mod tests;
