//! A whole file's map: [`map_file`], resumed from what a cache holds and
//! back-filled where its blocks lack what was asked, and the eager
//! [`build_index`] and [`build_map`] it is held to.

use std::collections::HashSet;
use std::sync::Arc;

use pgdt_codec::internal::counter_for;
#[cfg(feature = "introspect")]
use pgdt_core::internal::statistics_loaded;
use pgdt_core::{Error, Result};
use pgdt_dump::attach_text;
use pgdt_dump::cache::{CacheLoad, CacheMode, SourceWatch};
use pgdt_dump::dump_metadata_from_spans;
use pgdt_dump::internal::Builder;
use pgdt_dump::internal::{
    non_seekable_compression_diagnostic, scan_preamble, tiling_diagnostics, toc_coverage_diagnostic,
};
use pgdt_dump::{Event, ScanOptions, scan as scan_events};
use pgdt_io::ByteRangeSource;
use pgdt_model::internal::IgnoredRefusalsExt;
use pgdt_model::{
    Diagnostic, DiagnosticKind, DumpIndex, DumpMetadata, PostgresInvalidValues, Span,
};

use crate::backfill::{
    Backfill, backfill_statistics, report_density_shortfall, report_statistics_declines,
};
use crate::forward::{MapStop, Pass, cancelled_read, map_forward, splice};
use crate::statistics::account::{StatisticsAccount, StatisticsHeld, Term};
use crate::statistics::request::{GatherOptions, StatisticsRequest};

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
    /// zero under every other mode, and for a run interrupted before the map
    /// reached EOF, whatever it checked and banked before the interrupt.
    pub checked: usize,
    /// **How many blocks of the finished map declined to gather statistics**,
    /// under this run's allowance or an earlier, larger one, or under an
    /// earlier, smaller one where the statistics the block holds already cover
    /// this request, so nothing re-reads it
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
/// only a scan reaching EOF may state; `pgdt_mapping::build_index` stays the
/// eager, cache-blind producer. Rejected: teaching *it* to resume, which
/// duplicates the splice-onto-a-prefix logic here.
///
/// **It opens with the bounded preamble prepass** `pgdt_query::table_stream` runs, under
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
/// same per-block re-read [`gather_block_statistics`](crate::gather_block_statistics) exposes, saving as it
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
    gather: &GatherOptions,
    cache: &CacheMode,
    statistics: &StatisticsRequest,
) -> Result<MapRun> {
    // Taken before anything is read, so every later observation compares
    // against what this run started from (`pgdt_dump::cache::SourceWatch`).
    let watch = SourceWatch::open(source, cache.strict_identity()).await?;
    let run = map_file_watched(source, scan_options, gather, cache, statistics, &watch).await;
    watch.attribute(source, run).await
}

/// [`map_file`] once its watch is open, which then attributes any failure.
async fn map_file_watched(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    gather: &GatherOptions,
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
    refuse_recorded(gather.postgres_invalid_values, cache, statistics, &index)?;
    let account = Arc::new(StatisticsAccount::bounded_by(gather.statistics_allowance_bytes));
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
                d.kind(),
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
            gather,
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
        Pass::Parse(statistics, &account, gather.postgres_invalid_values),
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
        gather,
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
            report_statistics_declines(statistics, gather.statistics_allowance_bytes, &index);
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
    invalid: PostgresInvalidValues,
    cache: &CacheMode,
    statistics: &StatisticsRequest,
    index: &DumpIndex,
) -> Result<()> {
    if invalid != PostgresInvalidValues::Default {
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
