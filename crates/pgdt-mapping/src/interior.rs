//! The interior split (`docs/design/decisions.md`, "D52"): an open `COPY`
//! block's interior cut where its source says it may be, handed out a window
//! of pieces at a time, and folded until one piece holds the terminator.
//!
//! A serial leader that has just read `COPY … FROM stdin;` knows every byte
//! until `\.` is line-structured rows, so the interior can be handed out in
//! LF-split ranges to workers that never parse *structure*. [`scan_region`] is
//! the scheduler: it asks the source where a range may be cut, hands out a
//! window of pieces at a time, and folds until one piece holds the
//! terminator. What one piece answers on its own, and how the pieces fold
//! back into the block's totals, is [`piece`]'s.
//!
//! **[`map_forward`](crate::forward::map_forward) is the leader.** Its
//! `CopyStart` arm offers each open region to [`scan_region`] and closes one
//! it took through the same path a serial `CopyEnd` takes, then puts the
//! serial scanner back down past it.

use std::ops::Range;

use pgdt_core::{Error, Result};
use pgdt_dump::{CopyEnd, ScanOptions};
use pgdt_io::{
    ByteRangeSource, Extent, PartitionRead, Partitioning, WaitPolicy,
    internal::{cut, worker_count},
};
use pgdt_model::internal::BlockCensusExt;
use pgdt_model::internal::BlockObserver;
use tokio_util::sync::CancellationToken;

mod piece;

use self::piece::{
    BlockEndings, PieceEntry, PieceObserver, PieceScan, empty_census, merge, scan_piece,
};
pub(crate) use self::piece::{CensusPlan, Interior};

/// What the leader made of one open `COPY` region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegionScan {
    /// The block closed, and these are its totals — what the serial scanner
    /// would have stated at its `CopyEnd`.
    Closed(Interior),
    /// The region was not cut at all, and the serial scanner still owns it:
    /// the caller allows one worker, the source declines to be split here or
    /// does not know its end yet, or what is left of the file is smaller than
    /// one partition.
    Declined,
    /// The pass's cancellation fired between two windows: nothing about the
    /// region is known, and the caller banks what it had before it.
    Cancelled,
}

/// What the leader made of one region, and what it delivered short of what
/// the caller asked for. The second half is here because the two are known at
/// the same instant and nowhere else: [`scan_region`] is the only party that
/// sees the source's advice (`docs/design/decisions.md`, "D64").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegionOutcome {
    /// What happened to the region.
    pub scan: RegionScan,
    /// What the arrangement delivered, where it delivered less than
    /// [`pgdt_io::Parallelism::jobs`] asked for and the reason will still hold at
    /// the next block. `None` where the full count ran, where one reader was
    /// asked for, or where only this region stood in the way — its size, or
    /// the source advising it one partition where the file advises more
    /// ([`Shortfall`]).
    pub shortfall: Option<Shortfall>,
}

/// Which rule cut the delivered reader count below the count asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundBy {
    /// The source advises a single partition **over the whole file**, so it
    /// declines to be split at all — the shape a compressed source whose
    /// largest block the budget cannot hold reaches
    /// ([`pgdt_io::ByteRangeSource::block_decode_bytes`]) — or cannot be cut
    /// at all, its end not being known yet ([`scan_region`]).
    Source,
    /// The source would be split, and the caller's memory budget affords fewer
    /// readers of it than were asked for (`pgdt_io::internal::worker_count`).
    Budget,
}

impl BoundBy {
    /// The token the status line carries: stable, lowercase, and in the
    /// library's own vocabulary rather than any caller's flag names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Budget => "budget",
        }
    }
}

/// A count asked for and not delivered, with the bytes the arrangement it
/// refused would have held.
///
/// **It reports the rules that answer for the arrangement, and not the one
/// that answers for a block.** Both are read off the source's advice over *the
/// rest of the file* — the source's over the whole file too — and the caller's
/// budget, so one line stands for the whole scan. [`scan_region`]'s floor, and a region the source advises one
/// partition of where the file advises more, are the other ways to be left
/// serial and are deliberately not reported: each is an end-of-file
/// condition and would fire on the last block of every file.
///
/// **Silence means the announced count was dispatched, or one of those two
/// refused the block**, never that every
/// worker read at once or that the arrangement was a good one
/// (`KD22`, `docs/design/decisions.md`, "D52").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shortfall {
    /// Readers the caller asked for — [`pgdt_io::Parallelism::jobs`], the number
    /// `scan started` already announced.
    pub asked: usize,
    /// Readers the leader runs instead: one where the source declines to be
    /// split, and what the budget affords otherwise.
    pub delivered: usize,
    /// Which rule cut it.
    pub bound_by: BoundBy,
    /// **What the arrangement that was refused would have held resident** —
    /// the source's own arithmetic in both arms, never re-derived here. Under
    /// [`BoundBy::Budget`] it is what `asked` readers of the advice in force
    /// would hold ([`pgdt_io::Partitioning::worker_memory`]); under
    /// [`BoundBy::Source`], what one reader of the container path costs
    /// ([`pgdt_io::ByteRangeSource::block_decode_bytes`]). `None` for a
    /// source with no such path to name.
    pub would_hold_bytes: Option<u64>,
}

/// Correct `scan started`'s `jobs=` where the leader delivered fewer readers
/// than the caller asked for, **once per scan**, and say what would buy the
/// arrangement back (`docs/design/decisions.md`, "D64"). `scan started` names
/// what was asked for, the source's advice not having been read when it fires;
/// on a query the same fact is a `pgdt_query::PlanNote` on a `TableStream`.
///
/// What its silence means is [`Shortfall`]'s. A decline is *not* silent: the
/// shortfall is computed before it and returned with it, so a region left
/// serial by the budget or by the source still prints. `flag` keeps one
/// scan-wide fact from printing once per block, which is also why
/// [`Shortfall`] reports no reason a later block could answer differently.
pub(crate) fn report_shortfall(flag: &mut bool, shortfall: Option<Shortfall>) {
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

/// Scan the interior of the `COPY` block that opens at `data_offset` with
/// concurrent fused workers, and answer the block's totals.
///
/// The caller has just read `COPY … FROM stdin;`, so it knows every byte from
/// `data_offset` until `\.` is line-structured rows — which is the whole
/// licence for handing them out to workers that never parse structure
/// (`docs/design/decisions.md`, "D52").
///
/// **Where the file ends is `extent`, and `census` is how the block is
/// censused and counted**, `None` where it is not ([`scan_piece`]). A source
/// whose end is not known yet is never cut: what is left of the file is
/// unknown, and every cut is a range up to it — so it is left to the serial
/// scanner as a source declining to be split is.
/// `header_offset` is carried only to name the block in
/// [`Error::UnterminatedCopyBlock`], which is this function's to raise: a
/// [`scan_piece`] never claims end of file, and the leader is the only party
/// that knows there is none left.
///
/// **`observer` is the block's statistics observer, where it gathers any.**
/// Each partition observes its rows into one [`BlockObserver::piece`] made,
/// and every partition up to the one holding the terminator is absorbed back
/// into it in file order, so a block closed here has observed what the serial
/// scanner would have handed it. A declined region leaves it untouched.
///
/// **It reads `partitions()` for the shape of the cut and never for whether
/// cutting pays**, beyond a source stating one partition at most. That is the
/// caller's economics, stated as
/// `ScanOptions::parallelism` — which is why a plain file is cut here exactly
/// as a compressed one is, and why the refusal of parallel plain-file
/// discovery lives in the plain source's own worker recommendation
/// (`ByteRangeSource::default_workers`), reaching an unstated `--jobs`, rather
/// than in a branch
/// (`docs/design/decisions.md`, "D25").
///
/// **The one rule of its own it applies is a floor**, and it is derived rather than
/// chosen: cutting spends a whole reader's worth of the caller's budget on
/// each piece and charges the scheduling besides, so what is cut must be worth
/// at least one `partition_bytes()`. The region's extent is not known here —
/// finding it *is* the work — so the bound available is what is left of the
/// file, which the region cannot exceed: the floor leaves a file's tail
/// shorter than one reader's charge to the serial scanner, and a small region
/// earlier in the file is cut and over-read.
///
/// Deficiency register: `deficiency: KD22` — every piece of that window is
/// read and parsed before `merge` folds them and discards everything past the
/// first terminator, so a dump whose blocks are far smaller than the window is
/// read orders of magnitude over, worse at every worker added and reached with
/// no flag typed (a compressed source's default worker count is one per core).
/// **(c) unowned.** Neither the cut width nor this floor is the fix: it is for
/// the leader to learn a region's extent before committing a window.
///
/// **That floor is a memory charge and not a span**, which is visible on a
/// block-decoding source, where the charge covers a block unit, a chunk and the
/// decoder: such a tail is left serial until it is worth more than one reader
/// costs. Erring towards the serial scanner is the direction this bound is
/// wanted in — the alternative admits readers the budget never granted.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scan_region(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    cancel: &CancellationToken,
    table: &str,
    header_offset: u64,
    data_offset: u64,
    census: Option<CensusPlan>,
    extent: Extent,
    observer: Option<&mut (dyn BlockObserver + 'static)>,
) -> Result<RegionOutcome> {
    let Some(size) = extent.exact() else {
        let shortfall = (options.parallelism.jobs() > 1).then(|| Shortfall {
            asked: options.parallelism.jobs(),
            delivered: 1,
            bound_by: BoundBy::Source,
            would_hold_bytes: source.block_decode_bytes(),
        });
        return Ok(RegionOutcome { scan: RegionScan::Declined, shortfall });
    };
    let advice = source.partitions(data_offset..size);
    let partition_bytes = advice.partition_bytes();
    let workers = worker_count(options.parallelism, advice.worker_memory());
    // Whether this region could hold one of the source's own partitions — the
    // floor above, and the one refusal about *this* block rather than about
    // the arrangement. A source stating no cost fails it always, and is never
    // cut — which every such source's `max_partitions() == Some(1)` also says.
    let region_fits = partition_bytes > 0 && size.saturating_sub(data_offset) >= partition_bytes;
    let shortfall = shortfall(source, options, &advice, workers, size);
    if workers <= 1 || !region_fits || advice.max_partitions() == Some(1) {
        return Ok(RegionOutcome { scan: RegionScan::Declined, shortfall });
    }

    // **This loop grants the wait, and it is the only one that does**
    // (`docs/design/decisions.md`, "D5"). Each worker holds exactly one read
    // at a time, so a worker blocked for a slot is always waiting on a sibling
    // that will finish. **It is restored on the way out**: this loop runs
    // inside a top-level one and must leave the source as it found it.
    source.hint_parallelism(options.parallelism);
    source.hint_wait_policy(WaitPolicy::MayWait);
    let scanned = run_region(
        source,
        options,
        cancel,
        table,
        &advice,
        workers,
        data_offset,
        census,
        size,
        observer,
    )
    .await;
    source.hint_wait_policy(WaitPolicy::NeverWait);

    let scan = match scanned? {
        Some(interior) => RegionScan::Closed(interior),
        None if cancel.is_cancelled() => RegionScan::Cancelled,
        // Every window ran to the end of the file and none held the
        // terminator: `UnterminatedCopyBlock`, reached the parallel way.
        None => return Err(Error::UnterminatedCopyBlock { header_offset }),
    };
    Ok(RegionOutcome { scan, shortfall })
}

/// What this arrangement delivers short of what was asked, for the two
/// reasons that outlive the region — the fact `scan started` cannot carry,
/// the source's advice not being read until the leader stands on an open
/// block (`docs/design/decisions.md`, "D64").
///
/// **The source arm is asked about the whole file**, not about the region the
/// leader stands on: a block-decoding source past its last boundary advises a
/// single partition too. **Both numbers come off the source**
/// ([`pgdt_io::ByteRangeSource::block_decode_bytes`]), so a second copy of
/// that arithmetic cannot part company with the first.
fn shortfall(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    advice: &Partitioning,
    workers: usize,
    size: u64,
) -> Option<Shortfall> {
    let asked = options.parallelism.jobs();
    if asked <= 1 {
        return None;
    }
    // The source's refusal is reported ahead of the budget's, and the two are
    // routinely both true: a budget too small for one block of a compressed
    // file also affords one reader of the streaming advice it falls back to.
    if advice.max_partitions() == Some(1) && source.partitions(0..size).max_partitions() == Some(1)
    {
        return Some(Shortfall {
            asked,
            delivered: 1,
            bound_by: BoundBy::Source,
            would_hold_bytes: source.block_decode_bytes(),
        });
    }
    (workers < asked).then(|| Shortfall {
        asked,
        delivered: workers.max(1),
        bound_by: BoundBy::Budget,
        would_hold_bytes: Some(advice.worker_memory().at(asked)),
    })
}

/// The window loop: hand out `workers` pieces at a time until one of them
/// holds the terminator. `None` is either cancellation or end of file with no
/// terminator, which [`scan_region`] tells apart.
///
/// **A window is `workers` partitions wide**, so the memory the region holds
/// at its peak is the worker count times what the source said one costs — the
/// per-worker term `Parallelism::memory_bytes` was solved against to reach
/// that count — while no line outgrows its chunk, a worker's read growing up
/// to `max_line_bytes` uncharged where one does. **On a boundaried source, wide in the source's partitions, not
/// in its charge** (`Partitioning::window_end`, and
/// `docs/design/decisions.md`, "D8"): the charge covers a block unit, a chunk
/// and a decoder, so sizing the window by it would offer `cut` more boundaries
/// than there are workers. A source cut anywhere has no boundary to count, and
/// its window is `workers` charges wide.
#[allow(clippy::too_many_arguments)]
async fn run_region(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    cancel: &CancellationToken,
    table: &str,
    advice: &Partitioning,
    workers: usize,
    data_offset: u64,
    plan: Option<CensusPlan>,
    size: u64,
    mut observer: Option<&mut (dyn BlockObserver + 'static)>,
) -> Result<Option<Interior>> {
    let mut rows = 0u64;
    let mut census = empty_census(plan.as_ref());
    let mut frontier = data_offset;
    let mut entry = PieceEntry::RowStart;
    let mut endings = BlockEndings::default();

    while frontier < size {
        // Once per window, on the same argument the mapping loop checks once
        // per chunk (`docs/design/decisions.md`, "D26"): the window is the
        // shortest boundary inside a block a stop can be answered at.
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let ranges = cut(frontier..advice.window_end(frontier, workers, size), advice, workers);
        let mut scans: Vec<PieceScan> = Vec::with_capacity(ranges.len());
        // One future per piece, each a read followed by the parse of what it
        // read, on the blocking pool: the fused worker
        // (`docs/design/decisions.md`, "D52").
        let mut dispatched: futures::stream::FuturesOrdered<_> = ranges
            .into_iter()
            .enumerate()
            .map(|(i, range)| {
                let entry = if i == 0 { entry } else { PieceEntry::Resync };
                let piece =
                    observer.as_deref().map(|block| PieceObserver::new(block.piece(), data_offset));
                scan_partition(
                    source,
                    options,
                    advice.partition_read(),
                    entry,
                    range,
                    plan.clone(),
                    size,
                    piece,
                )
            })
            .collect();
        // Each partition's observer, and whether it read a terminator.
        let mut pieces: Vec<(Option<PieceObserver>, bool)> = Vec::with_capacity(workers);
        // **The lowest-offset error is the one raised, and this is what
        // arranges it** (`docs/design/decisions.md`, "D52"). A window's pieces
        // tile the region in ascending order, so partition order *is* file
        // order and [`futures::stream::FuturesOrdered`] hands the results back
        // in it: the `?` below fires on the earliest failing piece, where
        // `try_join_all` would return the first error it *observes*.
        // deficiency: KD134 — every partition of the window is taken through
        // the `?` below, those past the one holding the terminator included,
        // so a piece failing on what follows the block — a line of
        // `max_line_bytes` or more, a corrupt unit, a failed fetch — fails
        // the region where the serial scanner would have stopped at `\.`: a
        // cold `query` at `--jobs` above one refuses a table it answers
        // serially. The fix is a partition's error raised only where no
        // earlier one held the terminator. **(c) unowned**; promoted by such
        // a refusal reaching a user.
        while let Some(partition) = futures::StreamExt::next(&mut dispatched).await {
            let (partition_scans, piece) = partition?;
            // Held here, in file order, so a refusal is raised ahead of any
            // later partition's error.
            for scan in &partition_scans {
                if let Some((line, line_offset, refusal)) = endings.hold(scan) {
                    let table = table.to_owned();
                    return Err(Error::RowEndingRefused { table, line, line_offset, refusal });
                }
            }
            let terminates = partition_scans.iter().any(|scan| scan.terminator.is_some());
            scans.extend(partition_scans);
            pieces.push((piece, terminates));
        }
        // **Statistics fold as [`merge`] does, and stop where it stops**: the
        // partition holding the earliest terminator is the last in the block,
        // and what a later one observed is past the block's end.
        if let Some(block) = observer.as_deref_mut() {
            for (piece, terminates) in pieces {
                if let Some(piece) = piece {
                    block.absorb(piece.into_observer());
                }
                if terminates {
                    break;
                }
            }
        }

        if let Some(interior) = merge(&scans) {
            census.merge(&interior.census);
            return Ok(Some(Interior {
                census,
                end: CopyEnd { row_count: rows + interior.end.row_count, ..interior.end },
            }));
        }
        for piece in &scans {
            rows += piece.rows;
            census.merge(&piece.census);
        }
        // The last piece's `through` is where the window's last complete line
        // ended, so the next window begins there rather than at the window's
        // own end: the straddling row is counted exactly once.
        let Some(last) = scans.last() else { return Ok(None) };
        // A window that consumed nothing is a file that ended inside a line:
        // no terminator to find and no more bytes to find it in, so the loop
        // stops rather than reading the same window forever — the only thing
        // standing between a truncated dump and a hang.
        if last.through <= frontier {
            return Ok(None);
        }
        frontier = last.through;
        entry = last.next;
    }
    Ok(None)
}

/// One worker: read a piece and parse it, repeating from where the parse
/// stopped until the piece has consumed the line that ends at or past its
/// limit.
///
/// **How the piece is read is the source's own statement**
/// ([`pgdt_io::PartitionRead`]). A
/// plain file is read [`pgdt_io::PartitionRead::Chunked`], so
/// `pgdt_io`'s `BufferPool` pools every chunk-long buffer a worker takes — a
/// read grown past a chunk to finish a longer line is not pooled; a
/// block-decoding file is read [`pgdt_io::PartitionRead::Whole`], whose
/// *first* read of a piece is one of that source's own units; each
/// continuation read is a chunk, so the unit bounds the whole piece only
/// while a chunk is no larger, which nothing here enforces. Both halves are
/// asserted rather than argued: [`no_read_a_worker_makes_exceeds_the_chunk_size`] and
/// [`no_read_a_worker_makes_exceeds_the_stated_unit`].
///
/// **`Whole` is `min(piece, unit)`, not the piece**, so a cut wider than one
/// unit caps the body read at a unit rather than growing it with the width.
/// That bounds the buffer; it does not make a wider cut safe, which wants a
/// read clipped to the next boundary (`docs/design/decisions.md`, "D8").
///
/// **The reads repeat until the piece has passed its limit**, which no read
/// inside it can do: the line ending at or past `range.end` needs bytes past
/// it, so the last read of a piece straddles the boundary and is scanned as a
/// piece of its own — chunk-sized unless a longer line grows it, which is why a partition's stated
/// footprint carries a chunk buffer beside whatever the source retains
/// (`ByteRangeSource::partitions`). Handing back several [`PieceScan`]s keeps
/// [`merge`]'s fold the only fold. **Exactly one read is alive at a time**,
/// which is what makes [`WaitPolicy::MayWait`] safe here
/// (`docs/design/decisions.md`, "D5").
#[allow(clippy::too_many_arguments)]
async fn scan_partition(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    read: PartitionRead,
    entry: PieceEntry,
    range: Range<u64>,
    census: Option<CensusPlan>,
    size: u64,
    mut piece: Option<PieceObserver>,
) -> Result<(Vec<PieceScan>, Option<PieceObserver>)> {
    let limit = range.end;
    let mut out = Vec::new();
    let mut entry = entry;
    let mut start = range.start;
    let mut want = match read {
        PartitionRead::Whole { unit } => range.end.saturating_sub(range.start).min(unit).max(1),
        PartitionRead::Chunked => (options.read.chunk_size_bytes as u64).max(1),
    };

    while start < size {
        let end = (start + want).min(size);
        let len = usize::try_from(end - start).unwrap_or(usize::MAX);
        let bytes = source.read_range(start, len).await?;
        // The observer goes to the blocking pool with the bytes and comes back
        // with the scan, one read of the piece at a time.
        let plan = census.clone();
        let (scan, returned) = tokio::task::spawn_blocking(move || {
            let scan = scan_piece(&bytes, start, entry, limit, plan.as_ref(), piece.as_mut());
            (scan, piece)
        })
        .await
        .map_err(Error::join)?;
        piece = returned;
        let scan = scan?;
        // Finished when it has read the line ending at or past its limit,
        // found the terminator, or run out of file — the last being the
        // leader's to turn into an error.
        let done = scan.terminator.is_some() || scan.through > limit || end >= size;
        let advanced = scan.through > start;
        let (next_start, next_entry) = (scan.through, scan.next);
        if advanced || done {
            out.push(scan);
        }
        if done {
            break;
        }
        if advanced {
            start = next_start;
            entry = next_entry;
            want = options.read.chunk_size_bytes as u64;
        } else {
            // Not one line boundary in `want` bytes. Growing rather than
            // failing lets a legitimately long row through; `max_line_bytes`
            // is the same ceiling the serial loop enforces.
            if want >= options.read.max_line_bytes as u64 {
                return Err(Error::LineTooLong {
                    offset: start,
                    limit: options.read.max_line_bytes,
                });
            }
            want = (want * 2).min(options.read.max_line_bytes as u64);
        }
    }
    Ok((out, piece))
}

#[cfg(test)]
mod tests;
