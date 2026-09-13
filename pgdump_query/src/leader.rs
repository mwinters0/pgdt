//! The interior split: what one piece of an open `COPY` block's interior
//! answers on its own, and how the pieces fold back into the block's totals
//! (`docs/design/decisions.md`, "D52").
//!
//! A serial leader that has just read `COPY … FROM stdin;` knows every byte
//! until `\.` is line-structured rows, so the interior can be handed out in
//! LF-split ranges to workers that never parse *structure*. [`scan_piece`] is
//! what one worker runs and [`merge`] what the leader does with the answers;
//! both are pure and synchronous, taking a `&[u8]` rather than a source.
//! [`scan_region`] is the scheduler: it asks the source where a range may be
//! cut, hands out a window of pieces at a time, and folds until one piece
//! holds the terminator.
//!
//! **Two invariants make the split sound, and no third is needed.** I15 puts a
//! literal LF among the escaped things in COPY TEXT output, so every LF byte
//! inside a data region is a row boundary; I7 says `LF 5C 2E` cannot open a
//! data line, so the first line of the interior that *is* `\.` is the
//! terminator. Both are about a **line start**, which is why a piece resyncs
//! to one rather than being handed a mid-row byte ([`PieceEntry`]).
//!
//! **A piece is not a byte range, exactly as a replay segment is not**
//! (`docs/design/decisions.md`, "D51"): its `limit` is not where reading
//! stops, the piece running through the line that *ends* at the first LF at or
//! after `limit` — the row the next piece's resync then skips. So a row
//! straddling a cut belongs to the piece before it, once.
//!
//! **`stream::map_forward` is the leader.** Its `CopyStart` arm offers each
//! open region to [`scan_region`] and closes one it took through the same path
//! a serial `CopyEnd` takes, then puts the serial scanner back down past it.

use std::ops::Range;

use crate::index::ArrayShape;
use crate::io::{ByteRangeSource, PartitionRead, Partitioning, WaitPolicy};
use crate::map::census_row;
use crate::scan::{CopyEnd, CopyScanner, Event, ScanOptions};
use crate::stream::{cut, worker_count};
use crate::{Error, Result};

/// Where a piece's first byte sits relative to the rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PieceEntry {
    /// The piece begins exactly on a row start — the block's own
    /// `data_offset`, just past the header line's LF. The first piece of a
    /// block, and the only one that needs no resync.
    RowStart,
    /// The piece begins at a cut that knows nothing about rows: its first row
    /// is the one starting just past the first LF at or after the cut.
    Resync,
}

/// What one worker found in its piece of a `COPY` block's interior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PieceScan {
    /// Data rows this piece owns: every line it consumed before the
    /// terminator, or every line it consumed at all.
    pub rows: u64,
    /// This piece's own array-shape census, one [`ArrayShape`] per column,
    /// merged with its siblings' by [`merge`]: nothing may read it before the
    /// block closes (`docs/design/decisions.md`, "D34").
    pub census: Vec<ArrayShape>,
    /// `(terminator_offset, end_offset)` where this piece held the `\.` line.
    ///
    /// **A piece past the block's end can report one too**, having scanned DDL
    /// as if it were rows. [`merge`] takes the **earliest** piece that reports
    /// one, and every piece before the true terminator is inside the block,
    /// where I7 makes a `\.` line unambiguous.
    pub terminator: Option<(u64, u64)>,
    /// Absolute offset just past the last line this piece consumed: where a
    /// contiguous next piece's resync lands, and what a caller checks the
    /// tiling against.
    pub through: u64,
    /// How a piece continuing from [`PieceScan::through`] must enter.
    ///
    /// **[`PieceEntry::RowStart`] almost always, and the exception is why this
    /// is reported rather than inferred.** A piece that consumed a line
    /// boundary leaves `through` just past an LF, a row start; a
    /// [`PieceEntry::Resync`] piece that found no LF leaves it mid-row, and the
    /// two can produce the same `(rows, through)` pair.
    pub next: PieceEntry,
}

/// The block's totals, as the leader states them at `CopyEnd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interior {
    /// The union of the pieces' censuses, through the one that held the
    /// terminator.
    pub census: Vec<ArrayShape>,
    /// The `CopyEnd` the serial scanner would have emitted, byte for byte
    /// (`docs/design/decisions.md`, "D52").
    pub end: CopyEnd,
}

/// Scan one piece of an open `COPY` block's interior.
///
/// `bytes` are the file's bytes starting at absolute offset `base`, and they
/// must run **past** `limit` far enough to complete the line that straddles it
/// — the caller's job. A piece handed exactly `[base, limit)` is one whose
/// last line was cut short and therefore not counted.
///
/// `columns` sizes the census the way `map::Builder::on_copy_start` does, from
/// the header's column list; a header-less block states zero and the rows grow
/// it. **Pure and synchronous**: this is the body a `spawn_blocking` worker
/// runs, so it takes a slice rather than a source
/// (`docs/design/decisions.md`, "D52").
pub(crate) fn scan_piece(
    bytes: &[u8],
    base: u64,
    entry: PieceEntry,
    limit: u64,
    columns: usize,
) -> Result<PieceScan> {
    let empty = |through: u64, next: PieceEntry| PieceScan {
        rows: 0,
        census: vec![ArrayShape::default(); columns],
        terminator: None,
        through,
        next,
    };

    // The resync, a real forward read rather than a scanner started one byte
    // early: I7's guarantee is about a *line start*, and a row's own tail can
    // be `\.`, which a scanner entered mid-row would read as the terminator.
    let (start, skip) = match entry {
        PieceEntry::RowStart => (base, 0usize),
        PieceEntry::Resync => match memchr::memchr(b'\n', bytes) {
            Some(nl) => (base + nl as u64 + 1, nl + 1),
            // No line boundary anywhere in this piece: one row spans the whole
            // of it and belongs to the piece before this one. The resync is
            // unfinished, so a continuation must go on looking for the LF.
            None => return Ok(empty(base + bytes.len() as u64, PieceEntry::Resync)),
        },
    };
    // Two cuts fell inside one row. The row belongs to the piece before this
    // one, so this piece is empty rather than a duplicate.
    if start > limit {
        return Ok(empty(start, PieceEntry::RowStart));
    }

    let span = &bytes[skip..];
    // `header_offset` is only read back out of `Error::UnterminatedCopyBlock`,
    // which this loop cannot raise: it never claims end of file, so a piece
    // out of bytes reports no terminator rather than an unterminated block.
    // Deciding that is the leader's, the only party that knows.
    let mut scanner = CopyScanner::resume(start, Some((0, 0)));
    let mut census = vec![ArrayShape::default(); columns];
    let mut rows = 0u64;
    let mut terminator = None;

    while let Some(event) = scanner.next_event(span, false)? {
        match event {
            Event::Row(row) => {
                rows += 1;
                census_row(&mut census, row.raw);
            }
            Event::CopyEnd(end) => {
                terminator = Some((end.terminator_offset, end.end_offset));
                break;
            }
            // Unreachable: the scanner is in its `InCopy` state from the first
            // byte and leaves it only on the terminator, which breaks above.
            Event::CopyStart(_)
            | Event::Line(_)
            | Event::DollarQuoteEnd(_)
            | Event::LargeObjectStart(_)
            | Event::LargeObjectEnd(_) => {}
        }
        // The line just consumed ended at or past this piece's limit, so it
        // was its last. Checked *after* the event, the straddling row being
        // this piece's.
        if scanner.position() > limit {
            break;
        }
    }

    // The scanner only ever stops just past an LF, so every offset it can
    // report here is a row start; the `Resync`-found-no-LF arm above is the
    // sole exception.
    Ok(PieceScan {
        rows,
        census,
        terminator,
        through: scanner.position(),
        next: PieceEntry::RowStart,
    })
}

/// Fold `pieces` — one open `COPY` block's interior, in file order — into the
/// block's totals, stopping at the first piece that held the terminator.
/// `None` where none of them did: the leader's window did not reach the end of
/// the block and must hand out more of the interior.
///
/// **The fold is order-dependent only in where it stops.** Row counts sum and
/// censuses union, `ArrayShape`'s merge being a bounded semilattice. What the
/// order decides is which pieces are *in* the fold, everything past the
/// terminator not being part of the block at all.
pub(crate) fn merge(pieces: &[PieceScan]) -> Option<Interior> {
    let mut census: Vec<ArrayShape> = Vec::new();
    let mut row_count = 0u64;
    for piece in pieces {
        absorb_census(&mut census, &piece.census);
        row_count += piece.rows;
        if let Some((terminator_offset, end_offset)) = piece.terminator {
            return Some(Interior {
                census,
                end: CopyEnd { terminator_offset, end_offset, row_count },
            });
        }
    }
    None
}

/// Union `from` into `into`, growing `into` where the piece saw more columns
/// than anything before it — the census fold, in one place because [`merge`]
/// and the window accumulator in [`scan_region`] must not be two rules.
/// Length-tolerant for the same reason `crate::index::union_census` is: a
/// header-less block states no width, so two pieces of one block can
/// legitimately report different lengths.
fn absorb_census(into: &mut Vec<ArrayShape>, from: &[ArrayShape]) {
    if from.len() > into.len() {
        into.resize(from.len(), ArrayShape::default());
    }
    for (slot, shape) in into.iter_mut().zip(from) {
        slot.merge(shape);
    }
}

/// What the leader made of one open `COPY` region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegionScan {
    /// The block closed, and these are its totals — what the serial scanner
    /// would have stated at its `CopyEnd`.
    Closed(Interior),
    /// The region was not cut at all, and the serial scanner still owns it:
    /// the caller allows one worker, the source declines to be split here, or
    /// what is left of the file is smaller than one partition.
    Declined,
    /// [`ScanOptions::cancel`] was set between two windows: nothing about the
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
    pub(crate) scan: RegionScan,
    /// What the arrangement delivered, where it delivered less than
    /// [`crate::Parallelism::jobs`] asked for and the reason will still hold at
    /// the next block. `None` where the full count ran, where one reader was
    /// asked for, or where only this region's own size stood in the way
    /// ([`Shortfall`]).
    pub(crate) shortfall: Option<Shortfall>,
}

/// Which rule cut the delivered reader count below the count asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundBy {
    /// The source advises a single partition **over the whole file**, so it
    /// declines to be split at all — the shape a compressed source whose
    /// largest block the budget cannot hold reaches
    /// ([`crate::io::ByteRangeSource::block_decode_bytes`]).
    Source,
    /// The source would be split, and the caller's memory budget affords fewer
    /// readers of it than were asked for (`crate::stream::worker_count`).
    Budget,
}

impl BoundBy {
    /// The token the status line carries: stable, lowercase, and in the
    /// library's own vocabulary rather than any caller's flag names.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Budget => "budget",
        }
    }
}

/// A count asked for and not delivered, with the bytes the arrangement it
/// refused would have held.
///
/// **It reports the two rules that answer for the arrangement, and not the one
/// that answers for a block.** Both are read off the source's advice over *the
/// rest of the file* and the caller's budget, so one line stands for the whole
/// scan. [`scan_region`]'s floor is the third way to be left serial
/// and is deliberately not reported: it is an end-of-file condition and would
/// fire on the last block of every file.
///
/// **Silence means the announced count was dispatched**, never that every
/// worker read at once or that the arrangement was a good one
/// (`KD22`, `docs/design/decisions.md`, "D52").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shortfall {
    /// Readers the caller asked for — [`crate::Parallelism::jobs`], the number
    /// `scan started` already announced.
    pub(crate) asked: usize,
    /// Readers the leader runs instead: one where the source declines to be
    /// split, and what the budget affords otherwise.
    pub(crate) delivered: usize,
    /// Which rule cut it.
    pub(crate) bound_by: BoundBy,
    /// **What the arrangement that was refused would have held resident** —
    /// the source's own arithmetic in both arms, never re-derived here. Under
    /// [`BoundBy::Budget`] it is what `asked` readers of the advice in force
    /// would hold ([`crate::io::Partitioning::worker_memory`]); under
    /// [`BoundBy::Source`], what one reader of the container path costs
    /// ([`crate::io::ByteRangeSource::block_decode_bytes`]). `None` for a
    /// source with no such path to name.
    pub(crate) would_hold_bytes: Option<u64>,
}

/// Scan the interior of the `COPY` block that opens at `data_offset` with
/// concurrent fused workers, and answer the block's totals.
///
/// The caller has just read `COPY … FROM stdin;`, so it knows every byte from
/// `data_offset` until `\.` is line-structured rows — which is the whole
/// licence for handing them out to workers that never parse structure
/// (`docs/design/decisions.md`, "D52").
///
/// **Where the file ends is `size`, and `columns` is the header's width.**
/// `header_offset` is carried only to name the block in
/// [`Error::UnterminatedCopyBlock`], which is this function's to raise: a
/// [`scan_piece`] never claims end of file, and the leader is the only party
/// that knows there is none left.
///
/// **It reads `partitions()` for the shape of the cut and never for whether to
/// make one.** Whether cutting pays is the caller's economics, stated as
/// `ScanOptions::parallelism` — which is why a plain file is cut here exactly
/// as a compressed one is, and why the refusal of parallel plain-file
/// discovery lives in the plain source's own worker recommendation
/// (`ByteRangeSource::default_workers`), reaching an unstated `--jobs`, rather
/// than in a branch
/// (`docs/design/decisions.md`, "D25").
///
/// **The one rule it does apply is a floor**, and it is derived rather than
/// chosen: cutting spends a whole reader's worth of the caller's budget on
/// each piece and charges the scheduling besides, so what is cut must be worth
/// more than one `partition_bytes()`. The region's extent is not known here —
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
pub(crate) async fn scan_region(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    header_offset: u64,
    data_offset: u64,
    columns: usize,
    size: u64,
) -> Result<RegionOutcome> {
    let advice = source.partitions(data_offset..size);
    let partition_bytes = advice.partition_bytes();
    let workers = worker_count(options.parallelism, advice.worker_memory());
    // Whether this region could hold one of the source's own partitions — the
    // floor above, and the one refusal about *this* block rather than about
    // the arrangement. A source stating no cost states no floor either.
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
    let scanned = run_region(source, options, &advice, workers, data_offset, columns, size).await;
    source.hint_wait_policy(WaitPolicy::NeverWait);

    let scan = match scanned? {
        Some(interior) => RegionScan::Closed(interior),
        None if options.cancelled() => RegionScan::Cancelled,
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
/// ([`crate::io::ByteRangeSource::block_decode_bytes`]), so a second copy of
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
/// that count. **Wide in the source's partitions, not in its charge**
/// (`Partitioning::window_end`, and `docs/design/decisions.md`, "D8"): the
/// charge covers a block unit, a chunk and a decoder, so sizing the window by
/// it would offer `cut` more boundaries than there are workers.
#[allow(clippy::too_many_arguments)]
async fn run_region(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    advice: &Partitioning,
    workers: usize,
    data_offset: u64,
    columns: usize,
    size: u64,
) -> Result<Option<Interior>> {
    let mut rows = 0u64;
    let mut census: Vec<ArrayShape> = vec![ArrayShape::default(); columns];
    let mut frontier = data_offset;
    let mut entry = PieceEntry::RowStart;

    while frontier < size {
        // Once per window, on the same argument the mapping loop checks once
        // per chunk (`docs/design/decisions.md`, "D26"): the window is the
        // shortest boundary inside a block a stop can be answered at.
        if options.cancelled() {
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
                scan_partition(
                    source,
                    options,
                    advice.partition_read(),
                    entry,
                    range,
                    columns,
                    size,
                )
            })
            .collect();
        // **The lowest-offset error is the one raised, and this is what
        // arranges it** (`docs/design/decisions.md`, "D52"). A window's pieces
        // tile the region in ascending order, so partition order *is* file
        // order and [`futures::stream::FuturesOrdered`] hands the results back
        // in it: the `?` below fires on the earliest failing piece, where
        // `try_join_all` would return the first error it *observes*.
        while let Some(piece) = futures::StreamExt::next(&mut dispatched).await {
            scans.extend(piece?);
        }

        if let Some(interior) = merge(&scans) {
            absorb_census(&mut census, &interior.census);
            return Ok(Some(Interior {
                census,
                end: CopyEnd { row_count: rows + interior.end.row_count, ..interior.end },
            }));
        }
        for piece in &scans {
            rows += piece.rows;
            absorb_census(&mut census, &piece.census);
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
/// ([`crate::io::PartitionRead`], and `docs/design/decisions.md`, "D9"). A
/// plain file is read [`crate::io::PartitionRead::Chunked`], so
/// [`crate::io::BufferPool`] pools every buffer a worker takes; a
/// block-decoding file is read [`crate::io::PartitionRead::Whole`], one of
/// that source's own units at a time. Both halves are asserted rather than
/// argued: [`no_read_a_worker_makes_exceeds_the_chunk_size`] and
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
/// piece of its own — always chunk-sized, which is why a partition's stated
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
    columns: usize,
    size: u64,
) -> Result<Vec<PieceScan>> {
    let limit = range.end;
    let mut out = Vec::new();
    let mut entry = entry;
    let mut start = range.start;
    let mut want = match read {
        PartitionRead::Whole { unit } => range.end.saturating_sub(range.start).min(unit).max(1),
        PartitionRead::Chunked => (options.chunk_size as u64).max(1),
    };

    while start < size {
        let end = (start + want).min(size);
        let len = usize::try_from(end - start).unwrap_or(usize::MAX);
        let bytes = source.read_range(start, len).await?;
        let scan =
            tokio::task::spawn_blocking(move || scan_piece(&bytes, start, entry, limit, columns))
                .await??;
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
            want = options.chunk_size as u64;
        } else {
            // Not one line boundary in `want` bytes. Growing rather than
            // failing lets a legitimately long row through; `max_line_bytes`
            // is the same ceiling the serial loop enforces.
            if want >= options.max_line_bytes as u64 {
                return Err(Error::LineTooLong { offset: start, limit: options.max_line_bytes });
            }
            want = (want * 2).min(options.max_line_bytes as u64);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::io::{LocalFileSource, Parallelism};
    use crate::map::{Builder, DataBlock, SpanBody};
    use crate::scan::ChunkCarry;

    /// One `COPY` block as the **serial** pass states it: the reference every
    /// split below is checked against.
    #[derive(Debug, PartialEq, Eq)]
    struct Reference {
        header_offset: u64,
        data_offset: u64,
        end_offset: u64,
        columns: usize,
        interior: Interior,
    }

    /// Drive the serial scanner and `map::Builder` over `file` exactly as
    /// `stream::map_forward` does, and report every `COPY` block it found.
    /// Synchronous on purpose: the reference must not depend on a source, a
    /// runtime or a chunk size, the very things the split may differ in.
    fn reference(file: &[u8]) -> Vec<Reference> {
        let mut scanner = CopyScanner::new();
        let mut carry = ChunkCarry::new();
        let mut builder = Builder::with_database(None);
        let mut columns: Vec<usize> = Vec::new();
        carry.absorb(file);
        for pass in ChunkCarry::PASSES {
            let (span, span_eof) = carry.span(pass, file, true);
            while let Some(event) = scanner.next_event(span, span_eof).expect("a scannable file") {
                match event {
                    Event::CopyStart(start) => {
                        columns.push(start.header.columns.len());
                        builder.on_copy_start(start);
                    }
                    Event::Row(row) => builder.on_row(row.raw),
                    Event::CopyEnd(end) => builder.on_copy_end(end),
                    Event::Line(line) => builder.feed_line(line.offset, line.raw),
                    Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
                    Event::LargeObjectStart(start) => {
                        builder.on_large_object_start(start.start_offset)
                    }
                    Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
                }
            }
            carry.consumed(pass, file, scanner.take_consumed());
        }
        let spans = builder.finish(file.len() as u64);
        let mut widths = columns.into_iter();
        spans
            .into_iter()
            .filter_map(|span| match span.body {
                SpanBody::Data(DataBlock::Copy(block)) => Some(Reference {
                    header_offset: block.header_offset,
                    data_offset: block.data_offset,
                    end_offset: block.end_offset,
                    columns: widths.next().expect("one header per block"),
                    interior: Interior {
                        census: block.array_shapes,
                        end: CopyEnd {
                            terminator_offset: block.terminator_offset,
                            end_offset: block.end_offset,
                            row_count: block.row_count,
                        },
                    },
                }),
                _ => None,
            })
            .collect()
    }

    /// Cut `[data_offset, end_offset)` into `pieces` even ranges, scan each the
    /// way a worker would, and merge. The bytes handed to each piece run to
    /// end of file — the caller's half of the contract.
    fn split(file: &[u8], block: &Reference, pieces: usize) -> Option<Interior> {
        cut_up(file, block, pieces).0
    }

    /// The same, reporting how many pieces owned a row — what says a passing
    /// equality was a real split.
    fn cut_up(file: &[u8], block: &Reference, pieces: usize) -> (Option<Interior>, usize) {
        let len = block.end_offset - block.data_offset;
        let mut scans = Vec::with_capacity(pieces);
        for i in 0..pieces as u64 {
            let start = block.data_offset + len * i / pieces as u64;
            let limit = block.data_offset + len * (i + 1) / pieces as u64;
            let entry = if i == 0 { PieceEntry::RowStart } else { PieceEntry::Resync };
            scans.push(
                scan_piece(&file[start as usize..], start, entry, limit, block.columns)
                    .expect("a piece of a scannable file"),
            );
        }
        let working = scans.iter().filter(|p| p.rows > 0).count();
        (merge(&scans), working)
    }

    fn fixture(version: u32, schema: &str, flag_set: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(version.to_string())
            .join(schema)
            .join(format!("{flag_set}.sql"))
    }

    fn edge_cases() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql")
    }

    /// **The property the whole mechanism rests on: how a block's interior was
    /// split must not be visible in the answer.** A block cut every which way
    /// still reports the serial scanner's row count, terminator and end, and
    /// the serial pass's census.
    #[test]
    fn a_split_interior_answers_what_the_serial_scanner_answers() {
        let files = [
            edge_cases(),
            fixture(16, "edge_cases", "default"),
            fixture(16, "types", "default"),
            fixture(16, "partitions", "default"),
            fixture(13, "edge_cases", "default"),
            fixture(18, "objects", "default"),
        ];
        let mut blocks_checked = 0usize;
        let mut really_split = 0usize;
        for path in files {
            let file = std::fs::read(&path).unwrap();
            let blocks = reference(&file);
            for block in &blocks {
                for pieces in [1usize, 2, 3, 4, 5, 7, 8, 16, 33, 64] {
                    let (got, working) = cut_up(&file, block, pieces);
                    assert_eq!(
                        got.as_ref(),
                        Some(&block.interior),
                        "{} block at {} in {pieces} pieces",
                        path.display(),
                        block.data_offset
                    );
                    if working > 1 {
                        really_split += 1;
                    }
                }
                blocks_checked += 1;
            }
        }
        assert!(blocks_checked > 0, "fixture discovery found no COPY blocks");
        // Without this the equalities above could all be one piece owning
        // every row: the serial path compared to itself.
        assert!(really_split > 0, "no cut ever gave two pieces rows of their own");
    }

    /// More pieces than rows is the degenerate end of the same property: a
    /// piece with no row start owns nothing, and every row is still seen once.
    #[test]
    fn more_pieces_than_rows_still_counts_every_row_once() {
        let file = b"COPY public.t (a) FROM stdin;\n1\n2\n3\n\\.\n\nSELECT 1;\n".to_vec();
        let block = &reference(&file)[0];
        assert_eq!(block.interior.end.row_count, 3);
        for pieces in 1..=40 {
            assert_eq!(split(&file, block, pieces).as_ref(), Some(&block.interior), "{pieces}");
        }
    }

    /// A window that does not reach the terminator answers `None` rather than
    /// closing the block short, which is what tells the leader to hand out more
    /// of the interior. The limit lands one byte into the first row.
    #[test]
    fn a_window_short_of_the_terminator_closes_nothing() {
        let file = b"COPY public.t (a) FROM stdin;\n1\n2\n3\n\\.\n".to_vec();
        let block = &reference(&file)[0];
        let piece = scan_piece(
            &file[block.data_offset as usize..],
            block.data_offset,
            PieceEntry::RowStart,
            block.data_offset + 1,
            block.columns,
        )
        .unwrap();
        assert_eq!(piece.terminator, None);
        assert_eq!(piece.rows, 1);
        assert_eq!(piece.through, block.data_offset + 2);
        assert_eq!(merge(std::slice::from_ref(&piece)), None);
    }

    /// **A `\.` line past the block's end is not a terminator**, an earlier
    /// piece having found the real one: `merge` never reaches past it.
    #[test]
    fn a_later_pieces_false_terminator_is_never_taken() {
        let file = b"COPY public.t (a) FROM stdin;\n1\n\\.\nSELECT 1;\n\\.\nSELECT 2;\n".to_vec();
        let block = &reference(&file)[0];

        let real = scan_piece(
            &file[block.data_offset as usize..],
            block.data_offset,
            PieceEntry::RowStart,
            block.end_offset,
            block.columns,
        )
        .unwrap();
        // The piece after the block reads the trailing `\.` as a terminator.
        let after = scan_piece(
            &file[block.end_offset as usize..],
            block.end_offset,
            PieceEntry::RowStart,
            file.len() as u64,
            block.columns,
        )
        .unwrap();
        assert!(after.terminator.is_some(), "the fixture must offer a false terminator");

        assert_eq!(merge(&[real, after]).as_ref(), Some(&block.interior));
    }

    /// The census is a union over the pieces, which is what makes a mixed
    /// column resolvable: `{1,2}` in one piece and `{{1,2}}` in another records
    /// `(1, 2)`, where either alone records a wrong single depth.
    #[test]
    fn the_census_unions_across_pieces() {
        let file = b"COPY public.t (a) FROM stdin;\n{1,2}\n{{1,2},{3,4}}\n\\.\n".to_vec();
        let block = &reference(&file)[0];
        assert_eq!(block.interior.census[0].dims, Some((1, 2)));

        let split = split(&file, block, 2).unwrap();
        assert_eq!(split.census[0].dims, Some((1, 2)));
        assert_eq!(&split, &block.interior);
    }

    /// **A piece never hands the scanner a mid-row byte**, and this is the row
    /// that would be misread if it did: `a\.`, which COPY TEXT writes as
    /// `a\\.`, so the tail from its second backslash is `\.` and an LF. I7's
    /// guarantee is about a *line start*, so a piece entered there would close
    /// the block early; the resync is a real forward read to the next LF.
    #[test]
    fn a_cut_inside_a_row_whose_tail_is_a_backslash_dot_is_resynced_past() {
        let file = b"COPY public.t (a) FROM stdin;\na\\\\.\n\\.\n".to_vec();
        let block = &reference(&file)[0];
        assert_eq!(block.interior.end.row_count, 1);
        // From the row's second backslash the bytes read `\.\n` — a
        // terminator line to anyone entering mid-row.
        let cut = block.data_offset + 2;
        assert_eq!(&file[cut as usize..cut as usize + 3], b"\\.\n");

        let piece = scan_piece(&file[cut as usize..], cut, PieceEntry::Resync, block.end_offset, 1)
            .unwrap();
        assert_eq!(
            piece.terminator,
            Some((block.interior.end.terminator_offset, block.end_offset)),
            "the resync must skip the row's tail and find the real terminator"
        );
        assert_eq!(piece.rows, 0);

        for pieces in 1..=12 {
            assert_eq!(split(&file, block, pieces).as_ref(), Some(&block.interior), "{pieces}");
        }
    }

    /// The byte counts a fixture-sized block has to be scheduled against.
    ///
    /// A dump fixture is kilobytes and the source's own partition is several
    /// read chunks (`crate::io::PLAIN_PARTITION_CHUNKS`), so at the shipped
    /// chunk size every block here sits inside one partition and the scheduler
    /// declines it. Announcing 8 bytes as the read size makes the source
    /// advise 64-byte partitions, which is what exercises the window loop, the
    /// tail read and the growth path at all. **It is bounded below by the
    /// smallest region any fixture here ends with**: [`scan_region`]'s floor
    /// is one whole partition, so a larger partition declines a trailing block
    /// and the sweep below stops asserting anything about it.
    fn scheduled(source: &LocalFileSource, jobs: usize) -> ScanOptions {
        source.hint_read_size(8);
        ScanOptions {
            chunk_size: 8,
            parallelism: Parallelism::workers(jobs, crate::io::DEFAULT_MEMORY_BUDGET),
            ..ScanOptions::default()
        }
    }

    /// **The same property [`a_split_interior_answers_what_the_serial_scanner_answers`]
    /// asserts about the parse, asserted about the schedule**: the cut the
    /// leader actually makes — the source's own boundaries, window size and
    /// tail reads — is still invisible in the answer. At eight jobs against
    /// the local source's four pool slots this also exercises the wait, the
    /// run completing because a worker's read drops before its parse returns
    /// (`ByteRangeSource::hint_wait_policy`).
    #[tokio::test]
    async fn a_scheduled_region_answers_what_the_serial_scanner_answers() {
        let files = [
            edge_cases(),
            fixture(16, "edge_cases", "default"),
            fixture(16, "types", "default"),
            fixture(13, "edge_cases", "default"),
        ];
        let mut blocks_checked = 0usize;
        for path in files {
            let file = std::fs::read(&path).unwrap();
            let size = file.len() as u64;
            let blocks = reference(&file);
            let source = LocalFileSource::open(&path).unwrap();
            for block in &blocks {
                for jobs in [2usize, 3, 8] {
                    let options = scheduled(&source, jobs);
                    let got = scan_region(
                        &source,
                        &options,
                        block.header_offset,
                        block.data_offset,
                        block.columns,
                        size,
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        got.scan,
                        RegionScan::Closed(block.interior.clone()),
                        "{} block at {} under {jobs} jobs",
                        path.display(),
                        block.data_offset
                    );
                }
                blocks_checked += 1;
            }
        }
        assert!(blocks_checked > 0, "fixture discovery found no COPY blocks");
    }

    /// Answers exactly what it wraps, recording every read's length.
    struct Recording {
        inner: LocalFileSource,
        reads: std::sync::Mutex<Vec<(u64, usize)>>,
        /// When set, the advice is restated as
        /// [`crate::io::PartitionRead::Whole`] at this unit — the only way to
        /// put a `Whole` piece *wider* than a unit in front of the leader.
        unit: Option<u64>,
    }

    impl crate::io::ByteRangeSource for Recording {
        fn read_range(
            &self,
            offset: u64,
            len: usize,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::Result<bytes::Bytes>> + Send + '_>,
        > {
            self.reads.lock().unwrap().push((offset, len));
            self.inner.read_range(offset, len)
        }
        fn size(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<u64>> + Send + '_>>
        {
            self.inner.size()
        }
        fn modified(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = crate::Result<Option<std::time::SystemTime>>>
                    + Send
                    + '_,
            >,
        > {
            self.inner.modified()
        }
        fn hint_read_size(&self, len: usize) {
            self.inner.hint_read_size(len);
        }
        fn partitions(&self, range: std::ops::Range<u64>) -> crate::io::Partitioning {
            let advice = self.inner.partitions(range);
            match self.unit {
                Some(unit) => advice.reading(PartitionRead::Whole { unit }),
                None => advice,
            }
        }
    }

    /// **No read a worker makes on a `PartitionRead::Chunked` source exceeds
    /// `ScanOptions::chunk_size`** — the first read of a piece as much as the
    /// tail. It is what keeps every buffer such a worker takes a *pooled* one
    /// (`crate::io::BufferPool::keeps` admits the announced length and nothing
    /// longer). It is **not** source-agnostic: the block-decoding path reads a
    /// piece whole, where the bound that binds is the source's own unit
    /// ([`no_read_a_worker_makes_exceeds_the_stated_unit`]) — evidence
    /// `parallel-peak-rss`, decision `docs/design/decisions.md`, "D9". The
    /// assertion is on the reads the leader issued, not on the advice.
    #[tokio::test]
    async fn no_read_a_worker_makes_exceeds_the_chunk_size() {
        let path = edge_cases();
        let file = std::fs::read(&path).unwrap();
        let size = file.len() as u64;
        let blocks = reference(&file);
        let inner = LocalFileSource::open(&path).unwrap();
        let source = Recording { inner, reads: std::sync::Mutex::new(Vec::new()), unit: None };
        let mut scheduled_blocks = 0usize;
        for block in &blocks {
            for jobs in [2usize, 3, 8] {
                source.reads.lock().unwrap().clear();
                let options = scheduled(&source.inner, jobs);
                let got = scan_region(
                    &source,
                    &options,
                    block.header_offset,
                    block.data_offset,
                    block.columns,
                    size,
                )
                .await
                .unwrap();
                if matches!(got.scan, RegionScan::Closed(_)) {
                    scheduled_blocks += 1;
                }
                let reads = source.reads.lock().unwrap();
                assert!(!reads.is_empty(), "a scheduled region reads something");
                // **The first read at any offset is the one this is about.**
                // The growth path is the one legitimate way past the chunk and
                // is never the first read at its offset, while a piece's own
                // opening read always is — so the invariant needs no
                // arithmetic on the lengths.
                let mut seen: Vec<u64> = Vec::new();
                for &(offset, len) in reads.iter() {
                    if seen.contains(&offset) {
                        assert!(
                            len <= options.max_line_bytes,
                            "a {len}-byte read at {offset} is past the growth ceiling"
                        );
                        continue;
                    }
                    seen.push(offset);
                    assert!(
                        len <= options.chunk_size,
                        "the first read at {offset} under {jobs} jobs is {len} bytes, past the \
                         {}-byte chunk",
                        options.chunk_size
                    );
                }
            }
        }
        assert!(scheduled_blocks > 0, "no region was actually scheduled");
    }

    /// **No read a worker makes on a `PartitionRead::Whole` source exceeds the
    /// unit that source stated**, however many units the piece covers. It is
    /// what bounds the body buffer on the block-decoding path, where
    /// `Whole` is the piece exactly and safe only while a piece is one unit
    /// (`crate::io::BOUNDARIED_PARTITION_UNITS`, and
    /// `docs/design/decisions.md`, "D8"). Raising that width fails this test
    /// and nothing else here. Asserted over a **plain** file restated as
    /// `Whole`, because on the one source that says `Whole` today
    /// `min(piece, unit)` is `piece` and the bound is vacuous.
    ///
    /// It does **not** assert that the read is poolable:
    /// `crate::io::BufferPool::keeps` admits the announced chunk and nothing
    /// longer, at either width.
    #[tokio::test]
    async fn no_read_a_worker_makes_exceeds_the_stated_unit() {
        const UNIT: u64 = 24;
        let path = edge_cases();
        let file = std::fs::read(&path).unwrap();
        let size = file.len() as u64;
        let blocks = reference(&file);
        let inner = LocalFileSource::open(&path).unwrap();
        let source =
            Recording { inner, reads: std::sync::Mutex::new(Vec::new()), unit: Some(UNIT) };
        let mut scheduled_blocks = 0usize;
        let mut wide_pieces = 0usize;
        for block in &blocks {
            for jobs in [2usize, 3, 8] {
                source.reads.lock().unwrap().clear();
                let options = scheduled(&source.inner, jobs);
                let got = scan_region(
                    &source,
                    &options,
                    block.header_offset,
                    block.data_offset,
                    block.columns,
                    size,
                )
                .await
                .unwrap();
                if matches!(got.scan, RegionScan::Closed(_)) {
                    scheduled_blocks += 1;
                }
                // The piece is wider than the stated unit, so an unbounded
                // `Whole` would read past `UNIT`: the assertion is not vacuous.
                let advice = source.partitions(block.data_offset..size);
                if advice.partition_bytes() > UNIT {
                    wide_pieces += 1;
                }
                let reads = source.reads.lock().unwrap();
                assert!(!reads.is_empty(), "a scheduled region reads something");
                // Same shape as the chunked bound above: a repeat at one
                // offset is the growth retry, ceilinged at `max_line_bytes`.
                let mut seen: Vec<u64> = Vec::new();
                for &(offset, len) in reads.iter() {
                    if seen.contains(&offset) {
                        assert!(
                            len <= options.max_line_bytes,
                            "a {len}-byte read at {offset} is past the growth ceiling"
                        );
                        continue;
                    }
                    seen.push(offset);
                    assert!(
                        len as u64 <= UNIT,
                        "the first read at {offset} under {jobs} jobs is {len} bytes, past the \
                         {UNIT}-byte unit"
                    );
                }
            }
        }
        assert!(scheduled_blocks > 0, "no region was actually scheduled");
        assert!(wide_pieces > 0, "no piece was wider than the stated unit");
    }

    /// **Two ways to be declined, and neither is a branch on the source's
    /// type**: a caller that states no parallelism keeps the serial path, and a
    /// region too small for one of the source's own partitions is left alone
    /// however many jobs were stated.
    #[tokio::test]
    async fn a_region_too_small_to_cut_is_left_to_the_serial_scanner() {
        let path = fixture(16, "types", "default");
        let file = std::fs::read(&path).unwrap();
        let size = file.len() as u64;
        let block = &reference(&file)[0];
        let source = LocalFileSource::open(&path).unwrap();

        let serial = scheduled(&source, 1);
        assert!(serial.parallelism.is_serial(), "one job is the serial value, not a Workers of 1");
        let got = scan_region(
            &source,
            &serial,
            block.header_offset,
            block.data_offset,
            block.columns,
            size,
        )
        .await
        .unwrap();
        assert_eq!(got.scan, RegionScan::Declined);

        let shipped = ScanOptions {
            parallelism: Parallelism::workers(8, crate::io::DEFAULT_MEMORY_BUDGET),
            ..ScanOptions::default()
        };
        source.hint_read_size(shipped.chunk_size);
        let got = scan_region(
            &source,
            &shipped,
            block.header_offset,
            block.data_offset,
            block.columns,
            size,
        )
        .await
        .unwrap();
        assert_eq!(got.scan, RegionScan::Declined, "a kilobyte block against a 1 MiB partition");
    }

    /// Cancellation is answered between windows, and is neither a closed block
    /// nor an error: the caller banks the map it had before the region.
    #[tokio::test]
    async fn a_cancelled_region_closes_nothing_and_raises_nothing() {
        let path = fixture(16, "types", "default");
        let file = std::fs::read(&path).unwrap();
        let size = file.len() as u64;
        let block = &reference(&file)[0];
        let source = LocalFileSource::open(&path).unwrap();
        let options =
            ScanOptions { cancel: Some(Arc::new(AtomicBool::new(true))), ..scheduled(&source, 4) };
        let got = scan_region(
            &source,
            &options,
            block.header_offset,
            block.data_offset,
            block.columns,
            size,
        )
        .await
        .unwrap();
        assert_eq!(got.scan, RegionScan::Cancelled);
    }

    /// **A count asked for is not a count delivered, and this is the only
    /// party that knows the difference** (`docs/design/decisions.md`, "D64").
    /// It is answered whatever became of the region, so a block too small to
    /// cut still carries the arrangement-wide fact.
    #[tokio::test]
    async fn a_budget_affording_fewer_readers_than_asked_for_answers_a_shortfall() {
        let path = fixture(16, "types", "default");
        let file = std::fs::read(&path).unwrap();
        let size = file.len() as u64;
        let block = &reference(&file)[0];
        let source = LocalFileSource::open(&path).unwrap();
        source.hint_read_size(8);
        // `scheduled` announces an 8-byte chunk, so a partition is 64 bytes,
        // and a 128-byte budget buys two readers where four were asked for.
        let advice = source.partitions(block.data_offset..size);
        assert_eq!(advice.partition_bytes(), 64, "the fixture's own partition size moved");

        let options =
            ScanOptions { parallelism: Parallelism::workers(4, 128), ..scheduled(&source, 4) };
        let got = scan_region(
            &source,
            &options,
            block.header_offset,
            block.data_offset,
            block.columns,
            size,
        )
        .await
        .unwrap();
        assert_eq!(
            got.shortfall,
            Some(Shortfall {
                asked: 4,
                delivered: 2,
                bound_by: BoundBy::Budget,
                // What the four asked for would have held — the source's own
                // per-worker term.
                would_hold_bytes: Some(64 * 4),
            }),
        );
    }

    /// **Silence is the claim that the announced count was dispatched**, so the
    /// two arrangements that deliver what was asked answer no shortfall: one
    /// reader asked for, and a budget affording every reader asked for. The
    /// region floor is not a shortfall either ([`Shortfall`]), which the
    /// second case here pins.
    #[tokio::test]
    async fn an_arrangement_that_delivers_what_was_asked_answers_no_shortfall() {
        let path = fixture(16, "types", "default");
        let file = std::fs::read(&path).unwrap();
        let size = file.len() as u64;
        let block = &reference(&file)[0];
        let source = LocalFileSource::open(&path).unwrap();

        let serial = scheduled(&source, 1);
        let got = scan_region(
            &source,
            &serial,
            block.header_offset,
            block.data_offset,
            block.columns,
            size,
        )
        .await
        .unwrap();
        assert_eq!(got.scan, RegionScan::Declined, "one job is the serial path");
        assert_eq!(got.shortfall, None, "one reader asked for is one reader delivered");

        // A shipped-size partition against a file of kilobytes: declined by
        // the floor, a fact about where the leader stands and not about the
        // arrangement.
        let shipped = ScanOptions {
            parallelism: Parallelism::workers(8, crate::io::DEFAULT_MEMORY_BUDGET),
            ..ScanOptions::default()
        };
        source.hint_read_size(shipped.chunk_size);
        let got = scan_region(
            &source,
            &shipped,
            block.header_offset,
            block.data_offset,
            block.columns,
            size,
        )
        .await
        .unwrap();
        assert_eq!(got.scan, RegionScan::Declined);
        assert_eq!(got.shortfall, None, "a region too small to cut is not a shortfall");
    }

    /// **Only the leader may say a block is unterminated**, the other half of
    /// `scan_piece` never claiming end of file. The offset named is the
    /// header's, as the serial scanner names it too.
    #[tokio::test]
    async fn a_block_the_file_ends_inside_is_unterminated() {
        let mut file = b"COPY public.t (a) FROM stdin;\n".to_vec();
        let data_offset = file.len() as u64;
        for i in 0..500 {
            file.extend_from_slice(format!("{i}\n").as_bytes());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.sql");
        std::fs::write(&path, &file).unwrap();

        let source = LocalFileSource::open(&path).unwrap();
        let options = scheduled(&source, 4);
        let err = scan_region(&source, &options, 0, data_offset, 1, file.len() as u64)
            .await
            .expect_err("a COPY block with no terminator");
        assert!(
            matches!(err, Error::UnterminatedCopyBlock { header_offset: 0 }),
            "unexpected error: {err}"
        );
    }

    /// A row longer than the tail read is read by growing the read, bounded by
    /// the same ceiling the serial loop enforces.
    #[tokio::test]
    async fn a_row_longer_than_a_tail_read_is_still_one_row() {
        let wide = "x".repeat(4096);
        let file =
            format!("COPY public.t (a) FROM stdin;\n{wide}\n{wide}\n{wide}\n\\.\n\nSELECT 1;\n")
                .into_bytes();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.sql");
        std::fs::write(&path, &file).unwrap();
        let block = &reference(&file)[0];
        assert_eq!(block.interior.end.row_count, 3);

        let source = LocalFileSource::open(&path).unwrap();
        for jobs in [2usize, 4, 8] {
            let options = scheduled(&source, jobs);
            let got = scan_region(
                &source,
                &options,
                block.header_offset,
                block.data_offset,
                block.columns,
                file.len() as u64,
            )
            .await
            .unwrap();
            assert_eq!(got.scan, RegionScan::Closed(block.interior.clone()), "{jobs} jobs");
        }
    }
}
