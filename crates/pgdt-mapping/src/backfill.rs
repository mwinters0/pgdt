//! Re-reading blocks the map already holds: the statistics back-fill a pass
//! makes after EOF, its reports, and the one block a caller back-fills.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use pgdt_codec::internal::counter_for;
use pgdt_core::internal::StatisticsScope;
use pgdt_core::{Error, Result};
use pgdt_dump::internal::{Drawing, FieldFold, announce_read_loop, finish_block};
use pgdt_dump::{ChunkedScanner, CopyEnd, CopyScanner, Event, ReadOptions, ScanOptions};
use pgdt_io::{Arrangement, ByteRangeSource};
use pgdt_model::{
    BlockId, BlockRecord, BlockStatistics, CopyBlock, DataBlock, DumpIndex, DumpMetadata,
    IgnoredRefusals, PostgresInvalidValues, Span, SpanBody,
    internal::{BlockObserver, Recorder},
};
use tokio_util::sync::CancellationToken;

use crate::context::{Gathering, PassContext, ReadContext};
use crate::forward::{SaveThrottle, cancelled_read};
use crate::interior::{Region, RegionScan, report_shortfall, scan_region};
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
/// `gathering`'s request asks for ([`Backfill::Lacking`]), or of a map short of it
/// only those no strict parse checked ([`Backfill::Unchecked`]), in file order, storing each block's statistics as it
/// closes and saving through a [`SaveThrottle`] as [`map_forward`](crate::forward::map_forward) does; an
/// interrupt saves and stops. Announces how many blocks lacked them and, once
/// every one is re-read, how many were — and nothing at all when none did. A
/// read dropped by the interrupt is the interrupt, whether this pass made it or
/// the leader dispatched it ([`reread_rows`], [`cancelled_read`]).
///
/// **A block lacking statistics was loaded, or broke a stated maximum**: a
/// block this pass mapped holds what its own request asked, except for a
/// maximum no single read can deliver. So what a re-read replaces leaves the
/// term that carried it — `loaded`, the header offsets the cache supplied
/// statistics for, telling the two apart. What a re-read gathers charges
/// `gathering`'s account, under its allowance.
///
/// **Under [`PostgresInvalidValues::Strict`] a block no strict parse checked
/// is re-read too**, checking every field and recording that it did
/// ([`BlockRecord::checked_in_full`](pgdt_model::BlockRecord::checked_in_full))
/// — in the same read where it lacks statistics as well — so a clean strict run
/// has checked every field whichever runs built the map. `which` says whether
/// this is that pass alone ([`Backfill::Unchecked`]).
pub(crate) async fn backfill_statistics(
    ctx: &PassContext<'_>,
    gathering: Gathering<'_>,
    index: &mut DumpIndex,
    loaded: HashSet<BlockId>,
    which: Backfill,
) -> Result<BackfillRun> {
    let (statistics, allowance) = (gathering.request, gathering.allowance());
    // Positions into `index.spans`, which nothing below adds to or reorders.
    let metadata = index.metadata.as_deref();
    let strict = gathering.strict();
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
            let unchecked = strict && !block.record.checked_in_full;
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
    announce_read_loop(ctx.source(), ctx.read.scan.read, ctx.read.scan.parallelism);
    let mut reader = Rereads {
        read: ctx.read,
        // There is no cache file to name in a `CachedBlockChanged` refusal
        // unless one is enabled. A block can lack what was asked under any
        // mode — a stated maximum no single read delivers leaves a freshly
        // mapped block lacking, `CacheMode::Disabled` included (see this
        // function's doc).
        cache_path: ctx.cache.path(),
        gathering,
        loaded,
        shortfall_reported: false,
    };
    // **This pass's saves check the watch and do not finish it**, though a
    // map at EOF is complete: every unit it re-reads was compared against its
    // own check by the forward pass that completed the map, under the
    // identity the watch still holds ([`SourceWatch::finish`]) — but a map
    // `KD178` saves, whose last unit nothing compared — and finishing
    // before each save would decode a unit again per save and slow an
    // interrupt. `map_file`'s last save finishes it.
    let mut throttle = SaveThrottle::new();
    let blocks = rereads.len();
    for (done, (at, backfill, unchecked)) in rereads.into_iter().enumerate() {
        let regathers = backfill.is_some();
        let DumpIndex { spans, metadata, .. } = &mut *index;
        let Span { database, body, .. } = &mut spans[at];
        let SpanBody::Data(DataBlock::Copy(block)) = body else {
            unreachable!("the positions were read off copy blocks of this span list");
        };
        if !reader.fill(metadata.as_deref(), database.as_deref(), block, backfill).await? {
            ctx.save(index).await?;
            run.interrupted = true;
            return Ok(run);
        }
        run.reread += usize::from(regathers);
        run.checked += usize::from(unchecked);
        // Both of `map_forward`'s check points, a block's close being the
        // second (`docs/design/decisions.md`, "D63").
        if ctx.cancel().is_cancelled() && done + 1 < blocks {
            ctx.save(index).await?;
            run.interrupted = true;
            return Ok(run);
        }
        if throttle.due() {
            throttle.save(ctx, index).await?;
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
/// re-reads such a block for the maximum but one whose finer read declined, which a run
/// under a larger allowance re-reads, so every run under that maximum says
/// what it keeps rather than what it will fix.
pub(crate) fn report_density_shortfall(statistics: &StatisticsRequest, index: &DumpIndex) {
    let Some(max_rows) = statistics.density_max_rows() else { return };
    for block in index.blocks() {
        let Some(held) = block.record.statistics.as_deref() else { continue };
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
/// block's decline is recorded in the map
/// ([`BlockRecord::statistics_declined`](pgdt_model::BlockRecord::statistics_declined))
/// and re-read only under a larger allowance, or none, so the run that skips
/// one is the run that has to say what it is leaving and what would fix it
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
        let Some(under) = block.record.statistics_declined else { continue };
        if statistics.tracked_columns(&block.header).is_none() {
            continue;
        }
        declined += 1;
        tracing::info!(
            table = block.header.table,
            header_offset = block.address.header_offset,
            declined_under_bytes = under,
            allowance_bytes = %allowance.map_or_else(|| "(none)".to_string(), |a| a.to_string()),
            "statistics declined: this block's do not fit the statistics allowance, and a larger \
             one is what re-reads it",
        );
    }
    declined
}

/// **Back-fill one block the map already holds**: re-read it for what
/// `request` asks of it, and record what each read found in `block` —
/// what [`map_file`](crate::map_file)'s back-fill does to every block lacking
/// what its request asks, through the same per-block step. Its columns are
/// resolved against `metadata`, the map's, whole-file where it can be, in
/// `database`, the one its span records
/// ([`DumpIndex::blocks_with_database`](pgdt_model::DumpIndex::blocks_with_database)).
/// Answers whether it read the block: `false` is a block that holds what was
/// asked already, or one that declined under an allowance no smaller than
/// this one, and that no strict parse is owed a check of, and is left as it
/// was.
///
/// **What it asks of the block is the pass's**: whatever `request` lacks
/// there, gathered beside everything the block already held so nothing is
/// lost, and read once more at a finer size where the first read still
/// breaks a stated maximum; and under [`PostgresInvalidValues::Strict`] a
/// block no strict parse checked is read to check every field too. What a
/// read found is recorded as a pass records it: the census, which puts a
/// block mapped at the metadata level at the data level; the statistics, or
/// the allowance they declined under ([`BlockRecord::statistics_declined`](pgdt_model::BlockRecord::statistics_declined);
/// `docs/design/decisions.md`, "D85"); the fields a column it does not track
/// went past ([`BlockRecord::ignored_refusals`](pgdt_model::BlockRecord::ignored_refusals)); and a clean check
/// ([`BlockRecord::checked_in_full`](pgdt_model::BlockRecord::checked_in_full)).
///
/// **The arrangement is the pass's too**: the block is offered to the leader
/// under its parallelism and read serially where it declines, so what is
/// gathered is what a straight-through pass would have gathered; and its
/// statistics allowance ([`Arrangement::statistics_allowance`]) bounds what
/// the block's statistics hold, those it already holds included, a read past
/// it declining. Its readers draw what they hold past their chunk from its
/// ledger, which the block's statistics draw on first, the arrangement's
/// `held` being what the block holds already, which its account loads
/// (`docs/design/decisions.md`, "D157"). It takes no cancellation, being no
/// mapping pass: a caller stops it by dropping it (`docs/design/decisions.md`,
/// "D26").
///
/// **A block that no longer ends where the map says is refused**,
/// [`Error::CachedBlockChanged`] naming no cache, this entry point being
/// handed none, rather than given statistics describing other bytes than its
/// map does. **A field its type's `*_in` refuses fails the read**, with
/// [`Error::FieldRefused`], in a column `request` tracks — in every column
/// under [`PostgresInvalidValues::Strict`] — as it fails a mapping pass,
/// unless `invalid` ignores it; in any other column it is recorded.
///
/// **Its parameters are what an embedder states**, as [`map_file`](crate::map_file)'s are:
/// it opens no pass, so has no [`PassContext`] to take.
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
    let unchecked = invalid == PostgresInvalidValues::Strict && !block.record.checked_in_full;
    if plan.is_none() && !unchecked {
        return Ok(false);
    }
    let scan_options = ScanOptions { read, parallelism: arrangement.parallelism() };
    let extent = source.extent().await?;
    announce_read_loop(source, read, scan_options.parallelism);
    // What the block holds is alive beside what a read gathers, as a
    // loaded block's statistics are in a pass, until a read replaces them.
    let account = Arc::new(StatisticsAccount::charging(allowance, arrangement.ledger().clone()));
    let held = block.record.statistics.as_deref().map_or(0, BlockStatistics::heap_bytes);
    account.apply(&[(Term::Loaded, held as i64)]);
    let loaded =
        block.record.statistics.is_some().then_some(block.address.id()).into_iter().collect();
    let cancel = CancellationToken::new();
    let read = ReadContext {
        source,
        scan: scan_options,
        ledger: arrangement.ledger(),
        cancel: &cancel,
        extent,
    };
    let mut reader = Rereads {
        read,
        cache_path: None,
        gathering: Gathering { request, account: &account, invalid },
        loaded,
        shortfall_reported: false,
    };
    let filled = reader.fill(metadata, database, block, plan).await?;
    assert!(filled, "a re-read stops short only where its token fires, and nothing holds this one");
    Ok(true)
}

/// What every re-read of one back-fill shares: what it reads under, the
/// cache a moved block's refusal names, what it gathers under, which blocks'
/// statistics it carries as loaded, and the flag [`report_shortfall`] keeps
/// once per pass.
struct Rereads<'a> {
    read: ReadContext<'a>,
    cache_path: Option<&'a Path>,
    gathering: Gathering<'a>,
    /// The blocks whose statistics the gathering's account carries as
    /// [`Term::Loaded`], until a re-read replaces them.
    loaded: HashSet<BlockId>,
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
                let (request, allowance) = (self.gathering.request, self.gathering.allowance());
                next = request.backfill(block, metadata, database, allowance).map(Some);
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
    ) -> Result<Option<BlockRecord>> {
        let (header, account, invalid) =
            (&block.header, self.gathering.account, self.gathering.invalid);
        let observer = match plan {
            Some(plan) => {
                gather::observer_tracking(plan, header, metadata, database, account, invalid)
            }
            None => gather::checker(header, metadata, database, account),
        };
        reread_rows(
            &self.read,
            self.cache_path,
            metadata,
            database,
            block,
            Some(observer),
            &mut self.shortfall_reported,
        )
        .await
    }

    /// Record in `block` what one read of it found, `read` being the record
    /// that read finished: its census where the read `gathered` — every row
    /// was read, so a block gathering is at the data level whether its
    /// statistics fitted or not; a check alone records only that it checked,
    /// as a strict mapping pass records nothing more of a block it was not
    /// asked to — and what it gathered, or declined, with the fields its
    /// type's `*_in` refuses that the read keyed and went past, per column —
    /// under [`PostgresInvalidValues::Ignore`], or in a column gathered only
    /// because the block held it ([`StatisticsBackfill::requested`]).
    fn record(&mut self, block: &mut CopyBlock, read: BlockRecord, gathered: bool) {
        let id = block.address.id();
        // Copied on write where another holder shares it (`docs/design/decisions.md`, "D168").
        let record = Arc::make_mut(&mut block.record);
        if gathered {
            record.census = read.census;
        }
        record.checked_in_full |= read.checked_in_full;
        let ignored = read.ignored_refusals.map(|ignored| *ignored);
        let _attributed = StatisticsScope::enter();
        if let Some(allowance) = read.statistics_declined {
            // **A re-read that declined keeps what the block already
            // held**: the observer freed its own and holds nothing to
            // replace them with, so the block records the allowance
            // beside whatever an earlier pass gathered
            // (`docs/design/decisions.md`, "D85").
            record.statistics_declined = Some(allowance);
            merge_ignored(&mut record.ignored_refusals, ignored);
        } else if let Some(gathered) = read.statistics {
            let replaced = record.statistics.as_deref().map_or(0, BlockStatistics::heap_bytes);
            // The re-read's own observer has already credited
            // `Term::Retained`, so what it replaced leaves the term
            // that carried it: `Term::Loaded` for a block the cache
            // supplied, and `Term::Retained` for one this run
            // gathered — which a block re-read for a stated maximum
            // is, its own first read included.
            let term = if self.loaded.remove(&id) { Term::Loaded } else { Term::Retained };
            record.statistics = Some(gathered);
            // A block that holds what was asked declines nothing,
            // so a record from an earlier, tighter allowance goes.
            record.statistics_declined = None;
            merge_ignored(&mut record.ignored_refusals, ignored);
            self.gathering.account.apply(&[(term, -(replaced as i64))]);
        }
        // A check alone gathered nothing and declined nothing: what it
        // checked is all it records.
    }
}

/// Read every row of `block` again, as a mapping pass reads them — offered
/// to the leader, and serially where it declines — recording them as a
/// mapping pass records them ([`Recorder`]): censused and counted against the
/// DDL `metadata` states for the block in `database`, each handed to
/// `observer` where there is one, and answer the record the read finished:
/// what a back-fill's read (`Rereads::reread`) and a query's census of a block mapped
/// at the metadata level (`pgdt_query::Table::settle`) both read, a field the
/// observer refuses failing it. `None` is
/// `ctx`'s token stopping it first, by its flag or by a read it dropped, whichever
/// reader made that read, and nothing else answers it; a block no
/// longer ending where the map says is [`Error::CachedBlockChanged`], naming
/// `cache_path`. Each reader draws what it holds past its chunk from
/// `ctx`'s ledger (`docs/design/decisions.md`, "D157").
pub async fn reread_rows(
    ctx: &ReadContext<'_>,
    cache_path: Option<&Path>,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    block: &CopyBlock,
    observer: Option<Box<dyn BlockObserver>>,
    shortfall_reported: &mut bool,
) -> Result<Option<BlockRecord>> {
    let moved = || Error::CachedBlockChanged {
        path: cache_path.map(Path::to_path_buf),
        header_offset: block.address.header_offset,
    };
    // A source cut short of the block's recorded end cannot end it there, and
    // every read loop below takes its length from the end less its position.
    // One whose end is not known yet is read until it ends.
    if ctx.extent.exact().is_some_and(|size| block.address.end_offset > size) {
        return Err(moved());
    }
    let counter = counter_for(&block.header, metadata, database);
    let mut recorder =
        Recorder::new(block.address.data_offset).with_census(block.header.columns.len(), counter);
    if let Some(observer) = observer {
        recorder = recorder.with_observer(observer);
    }
    let region = Region {
        table: &block.header.qualified_name(),
        header_offset: block.address.header_offset,
        data_offset: block.address.data_offset,
        recorder: &mut recorder,
    };
    let outcome = match scan_region(ctx, region).await {
        Ok(outcome) => outcome,
        // A read the leader dispatched, dropped by the interrupt
        // ([`cancelled_read`]): the same `None` the token answers, as
        // [`map_file`](crate::map_file) banks the mapping pass's.
        Err(e) if cancelled_read(&e, ctx.cancel) => return Ok(None),
        Err(e) => return Err(e),
    };
    report_shortfall(shortfall_reported, outcome.shortfall);
    let end = match outcome.scan {
        RegionScan::Closed(end) => end,
        RegionScan::Cancelled => return Ok(None),
        RegionScan::Declined => match record_rows(ctx, block, &mut recorder).await? {
            Some(end) => end,
            None => return Ok(None),
        },
    };
    let recorded = (
        block.address.terminator_offset,
        block.address.end_offset,
        block.record.row_count,
        block.record.longest_line,
    );
    let longest_line = end.longest_line.map(|line| line.in_block(block.address.data_offset));
    if (end.terminator_offset, end.end_offset, end.row_count, longest_line) != recorded {
        return Err(moved());
    }
    finish_block(recorder, &block.header, &end).map(Some)
}

/// Hand every row of `block` to `recorder`, read serially from its first
/// data byte to its terminator — a row outgrowing its chunk streamed, its
/// fields folded as they go by (`docs/design/decisions.md`, "D138") — and
/// answer the `CopyEnd` the scanner met — `None` where `ctx`'s token fired
/// first, read once per chunk as [`map_forward`](crate::forward::map_forward)
/// reads it.
async fn record_rows(
    ctx: &ReadContext<'_>,
    block: &CopyBlock,
    recorder: &mut Recorder,
) -> Result<Option<CopyEnd>> {
    let (source, cancel) = (ctx.source, ctx.cancel);
    let scanner =
        CopyScanner::resume(block.address.data_offset, Some((block.address.header_offset, 0)));
    let mut chunked = ChunkedScanner::to_source_end(scanner, ctx.extent, ctx.scan.read);
    let mut holder = ctx.ledger.holder();
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let fold = Some(&mut *recorder as &mut dyn FieldFold);
        let read = chunked
            .next_chunk_folding(source, fold, Some(Drawing::held(&mut holder).parking()))
            .await;
        let mut chunk = match read {
            Ok(chunk) => chunk,
            // The check above, reached through the read rather than
            // through the token ([`cancelled_read`]). This block's re-read
            // is abandoned either way, so the caller hears the same
            // `None`.
            Err(e) if cancelled_read(&e, cancel) => return Ok(None),
            Err(e) => return Err(e.in_copy_of(|| block.header.qualified_name())),
        };
        chunk.stream_rows(true);
        while let Some(event) = chunk.next_event()? {
            match event {
                Event::Row(row) => recorder.row(row.offset, row.raw),
                Event::CopyEnd(end) => return Ok(Some(*end)),
                // A streamed row was folded into the recorder as it went by; a
                // scanner inside a block's rows emits nothing else before
                // its `CopyEnd`.
                _ => {}
            }
        }
        if chunked.at_end() {
            return Err(Error::UnterminatedCopyBlock {
                header_offset: block.address.header_offset,
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

#[cfg(test)]
mod tests;
