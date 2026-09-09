//! The interior split: what one piece of an open `COPY` block's interior
//! answers on its own, and how the pieces fold back into the block's totals
//! (`docs/design/architecture.md`, "The interior split").
//!
//! A serial leader that has just read `COPY … FROM stdin;` knows every byte
//! until `\.` is line-structured rows, so the interior can be handed out in
//! LF-split ranges to workers that never parse *structure* — the leader has
//! already proved there is none in there to find. [`scan_piece`] is what one
//! worker runs and [`merge`] is what the leader does with the answers; both
//! are pure and synchronous, taking a `&[u8]` rather than a source.
//! [`scan_region`] is the scheduler over them: it asks the source where a
//! range may be cut, hands out a window of pieces at a time, and folds what
//! comes back until one piece holds the terminator.
//!
//! **Two invariants make the split sound, and no third is needed.** I15 puts a
//! literal LF among exactly seven escaped things in COPY TEXT output, so every
//! LF byte inside a data region is a row boundary; I7 says `LF 5C 2E` cannot
//! open a data line, so the first line of the interior that *is* `\.` is the
//! terminator and no other line can be mistaken for it. Both are about a **line
//! start**, which is why a piece resyncs to one rather than being handed a
//! mid-row byte ([`PieceEntry`]).
//!
//! **A piece is not a byte range, exactly as a replay segment is not**
//! (`docs/design/architecture.md`, "Partitioned replay"). Its `limit` is not
//! where reading stops: the piece runs through the line that *ends* at the
//! first LF at or after `limit`, which is the row the next piece's resync then
//! skips — the two rules being one rule, since the resync looks for that same
//! LF. So a row
//! straddling a cut belongs to the piece before it, once, and a cut needs to
//! know nothing about rows — which is what lets `ByteRangeSource::partitions`
//! advise block boundaries.
//!
//! **`stream::map_forward` is the leader.** Its `CopyStart` arm offers each
//! open region to [`scan_region`] and closes one it took through the same path
//! a serial `CopyEnd` takes, then puts the serial scanner back down past the
//! block — so what `--jobs` buys a `parse` is the interior of every `COPY`
//! block large enough to cut.

use std::ops::Range;

use crate::index::ArrayShape;
use crate::io::{ByteRangeSource, Partitioning, WaitPolicy};
use crate::map::census_row;
use crate::scan::{CopyEnd, CopyScanner, Event, ScanOptions};
use crate::stream::{cut, worker_count};
use crate::{Error, Result};

/// Where a piece's first byte sits relative to the rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PieceEntry {
    /// The piece begins exactly on a row start — the block's own
    /// `data_offset`, which is the byte just past the header line's LF. The
    /// first piece of a block, and the only one that needs no resync.
    RowStart,
    /// The piece begins at a cut that knows nothing about rows: its first row
    /// is the one starting just past the first LF at or after the cut.
    Resync,
}

/// What one worker found in its piece of an open `COPY` block's interior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PieceScan {
    /// Data rows this piece owns — every line it consumed before the
    /// terminator, if it held one, and every line it consumed otherwise.
    pub rows: u64,
    /// This piece's own array-shape census, one [`ArrayShape`] per column.
    /// Merged with its siblings' by [`merge`]; on its own it describes only
    /// the rows this piece saw, which is why nothing may read it before the
    /// block closes (`docs/design/architecture.md`, "What the census decides,
    /// and who may believe it").
    pub census: Vec<ArrayShape>,
    /// `(terminator_offset, end_offset)` where this piece held the `\.` line.
    ///
    /// **A piece past the block's end can report one too**, having scanned DDL
    /// as if it were rows — a `\.` line inside a dollar-quoted body, say. That
    /// costs nothing, because [`merge`] takes the **earliest** piece that
    /// reports one and every piece before the true terminator is inside the
    /// block, where I7 makes a `\.` line unambiguous.
    pub terminator: Option<(u64, u64)>,
    /// Absolute offset just past the last line this piece consumed. Where a
    /// contiguous next piece's resync would land, and what a caller checks the
    /// tiling against.
    pub through: u64,
    /// How a piece continuing from [`PieceScan::through`] must enter.
    ///
    /// **[`PieceEntry::RowStart`] almost always, and the exception is why this
    /// is reported rather than inferred.** A piece that consumed at least one
    /// line boundary leaves `through` just past an LF, which is a row start; a
    /// [`PieceEntry::Resync`] piece that found no LF at all leaves it at the
    /// end of the bytes it was given, which is mid-row. Those two cases can
    /// produce the same `(rows, through)` pair, so nothing outside this
    /// function can tell them apart — and a continuation that guessed wrong
    /// would hand the scanner a mid-row byte, which is the one thing the
    /// resync exists to prevent.
    pub next: PieceEntry,
}

/// The block's totals, as the leader states them at `CopyEnd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interior {
    /// The union of the pieces' censuses, up to and including the one that
    /// held the terminator.
    pub census: Vec<ArrayShape>,
    /// The `CopyEnd` the serial scanner would have emitted — byte for byte,
    /// which is what makes a parallel scan's cache equal a serial one's
    /// (`docs/design/architecture.md`, "The interior split").
    pub end: CopyEnd,
}

/// Scan one piece of an open `COPY` block's interior.
///
/// `bytes` are the file's bytes starting at absolute offset `base`, and they
/// must run **past** `limit` far enough to complete the line that straddles it
/// — which is the caller's job, since only the caller knows what it read. A
/// piece handed exactly `[base, limit)` is not wrong, it is just a piece whose
/// last line was cut short and therefore not counted; the scheduler avoids that
/// by reading past the cut.
///
/// `columns` sizes the census the way `map::Builder::on_copy_start` does, from
/// the header's column list. A header-less block states zero and the rows grow
/// it, exactly as the serial pass does.
///
/// **Pure and synchronous, and deliberately so.** This is the body a
/// `spawn_blocking` worker runs, so it takes a slice rather than a source: the
/// decode that produced the slice and the parse of it are then one thread's
/// work, which is the whole argument for a fused worker
/// (`docs/design/architecture.md`, "The interior split").
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

    // The resync, and it is a real forward read rather than a scanner started
    // one byte early: I7's guarantee is about a *line start*, and a row's own
    // tail can be the two bytes `\.` — a value ending in an escaped backslash,
    // cut between them — which a scanner entered mid-row would read as the
    // block's terminator.
    let (start, skip) = match entry {
        PieceEntry::RowStart => (base, 0usize),
        PieceEntry::Resync => match memchr::memchr(b'\n', bytes) {
            Some(nl) => (base + nl as u64 + 1, nl + 1),
            // No line boundary anywhere in this piece: one row spans the whole
            // of it, and that row belongs to the piece before this one. The
            // resync is unfinished, so a continuation must go on looking for
            // the LF rather than treating this offset as a row start.
            None => return Ok(empty(base + bytes.len() as u64, PieceEntry::Resync)),
        },
    };
    // Two cuts fell inside one row. The row belongs to the piece before this
    // one, so this piece is empty rather than a duplicate.
    if start > limit {
        return Ok(empty(start, PieceEntry::RowStart));
    }

    let span = &bytes[skip..];
    // `header_offset` is only ever read back out of `Error::UnterminatedCopyBlock`,
    // which this loop cannot raise: it never claims end of file, so a piece
    // that runs out of bytes inside the block reports no terminator rather than
    // an unterminated one. Deciding *that* is the leader's, which is the only
    // party that knows whether more of the file follows.
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
        // The line just consumed ended at or past this piece's limit, so it was
        // the last one this piece owns. Checked *after* the event rather than
        // before it, because the straddling row is this piece's.
        if scanner.position() > limit {
            break;
        }
    }

    // The scanner only ever stops just past an LF, so every offset it can
    // report here is a row start — which is what makes the `Resync`-found-no-LF
    // arm above the sole exception.
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
///
/// `None` where none of them did: the leader's window did not reach the end of
/// the block, and it must hand out more of the interior before it can close it.
///
/// **The fold is order-dependent only in where it stops.** Row counts sum and
/// censuses union, and `ArrayShape`'s merge is min-of-mins, max-of-maxes and OR
/// of the `[lb:ub]=` flag over `ArrayShape::default()` — a bounded semilattice,
/// so commutative, associative and idempotent. What the order decides is which
/// pieces are *in* the fold, since everything past the terminator is not part
/// of the block at all.
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
///
/// Length-tolerant for the same reason `crate::index::union_census` is: a
/// header-less block states no width, so the rows are what grow the vector and
/// two pieces of one block can legitimately report different lengths.
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
    /// The region was not cut at all, and the serial scanner still owns it.
    /// Either the caller allows one worker, or the source declines to be
    /// split here, or what is left of the file is smaller than one partition.
    Declined,
    /// [`ScanOptions::cancel`] was set between two windows. Nothing about the
    /// region is known, and the caller banks what it had before it.
    Cancelled,
}

/// Scan the interior of the `COPY` block that opens at `data_offset` with
/// concurrent fused workers, and answer the block's totals.
///
/// The caller has just read `COPY … FROM stdin;`, so it knows every byte from
/// `data_offset` until `\.` is line-structured rows — which is the whole
/// licence for handing them out to workers that never parse structure
/// (`docs/design/architecture.md`, "The interior split").
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
/// discovery lives in `--jobs`' default rather than in a branch
/// (`docs/design/architecture.md`, "What parallelism buys, and where it
/// stops").
///
/// **The one rule it does apply is a floor**, and it is derived rather than
/// chosen: a region smaller than one `partition_bytes()` is left to the serial
/// scanner, because cutting it would hand some worker less than the source's
/// own unit and charge the scheduling anyway. The region's extent is not known
/// here — finding it *is* the work — so the bound available is what is left of
/// the file, which the region cannot exceed.
pub(crate) async fn scan_region(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    header_offset: u64,
    data_offset: u64,
    columns: usize,
    size: u64,
) -> Result<RegionScan> {
    let advice = source.partitions(data_offset..size);
    let partition_bytes = advice.partition_bytes();
    let workers = worker_count(options.parallelism, partition_bytes);
    if workers <= 1
        || partition_bytes == 0
        || advice.max_partitions() == Some(1)
        || size.saturating_sub(data_offset) < partition_bytes
    {
        return Ok(RegionScan::Declined);
    }

    // **This loop grants the wait, and it is the only one that does**
    // (`ByteRangeSource::hint_wait_policy`). Each worker holds exactly one
    // read at a time — the bytes move into the `spawn_blocking` closure and
    // drop when it returns — so a worker blocked for a slot is always waiting
    // on a sibling that will finish, which is the promise the permission is.
    //
    // **It is restored on the way out, unlike the three top-level loops.**
    // Those each state their own policy when they start and leave it stated;
    // this one runs *inside* one of them and has to leave the source as it
    // found it, or the enclosing loop would go on reading under a permission
    // it never granted.
    source.hint_parallelism(options.parallelism);
    source.hint_wait_policy(WaitPolicy::MayWait);
    let scanned = run_region(source, options, &advice, workers, data_offset, columns, size).await;
    source.hint_wait_policy(WaitPolicy::NeverWait);

    match scanned? {
        Some(interior) => Ok(RegionScan::Closed(interior)),
        None if options.cancelled() => Ok(RegionScan::Cancelled),
        // Every window ran to the end of the file and none of them held the
        // terminator, which is the serial scanner's `UnterminatedCopyBlock`
        // reached the parallel way.
        None => Err(Error::UnterminatedCopyBlock { header_offset }),
    }
}

/// The window loop: hand out `workers` pieces at a time until one of them
/// holds the terminator.
///
/// `None` is either cancellation or end of file with no terminator, which
/// [`scan_region`] tells apart — the two differ in nothing this loop can see,
/// and folding the test in here would put the error's wording inside the loop
/// that raises it.
///
/// **A window is `workers` partitions wide**, so the memory the region holds
/// at its peak is the worker count times what the source said one costs —
/// which is the number `Parallelism::memory_bytes` was divided by to reach
/// that worker count in the first place.
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
    let window_bytes = (workers as u64).saturating_mul(advice.partition_bytes());
    let mut rows = 0u64;
    let mut census: Vec<ArrayShape> = vec![ArrayShape::default(); columns];
    let mut frontier = data_offset;
    let mut entry = PieceEntry::RowStart;

    while frontier < size {
        // Once per window, on the same argument the mapping loop checks once
        // per chunk: a `COPY` block can be hundreds of gigabytes, and the
        // window is the shortest boundary inside one that a stop can be
        // answered at.
        if options.cancelled() {
            return Ok(None);
        }
        let ranges = cut(frontier..(frontier + window_bytes).min(size), advice, workers);
        let mut scans: Vec<PieceScan> = Vec::with_capacity(ranges.len());
        // One future per piece, each of them a read followed by the parse of
        // what it read, on the blocking pool. That is the fused worker: a
        // decoded block never crosses a channel, because the thread that
        // decoded it is the thread that parses it
        // (`docs/design/architecture.md`, "The interior split").
        let mut dispatched: futures::stream::FuturesOrdered<_> = ranges
            .into_iter()
            .enumerate()
            .map(|(i, range)| {
                let entry = if i == 0 { entry } else { PieceEntry::Resync };
                scan_partition(source, options, entry, range, columns, size)
            })
            .collect();
        // **The lowest-offset error is the one raised, and this is what
        // arranges it** (`docs/design/architecture.md`, "The interior
        // split"). A window's pieces tile the region in
        // ascending order, so partition order *is* file order, and
        // [`futures::stream::FuturesOrdered`] hands the results back in that
        // order however they arrived: the `?` below therefore fires on the
        // earliest failing piece, after every piece before it has finished or
        // failed, and drops the ones after it unread. `try_join_all` polls the
        // same futures concurrently but returns the first error it *observes*,
        // which is a race between the workers — so a re-run over an unchanged
        // file could name a different piece each time.
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
        // own end — which is what keeps the straddling row from being counted
        // twice or not at all.
        let Some(last) = scans.last() else { return Ok(None) };
        // A window that consumed nothing is a file that ended inside a line:
        // there is no terminator to find and no more bytes to find it in, so
        // the loop stops here rather than reading the same window forever.
        // This is the only thing standing between a truncated dump and a hang,
        // which is why it is a check rather than an argument.
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
/// **The first read is the piece exactly**, `[range.start, range.end)`, which
/// on a block-decoding source is one whole block and therefore a zero-copy
/// slice of it. That read can never complete the piece on its own — the line
/// that ends at or past `range.end` needs bytes past `range.end` — so a second,
/// **chunk-sized** read follows it and is scanned as a piece of its own,
/// starting exactly at a row boundary. That is why a partition's stated
/// footprint is a block *plus a chunk buffer*: the tail is the read that
/// straddles the boundary, and it is a chunk rather than a block
/// (`ByteRangeSource::partitions`).
///
/// Handing back several [`PieceScan`]s rather than one is what keeps
/// [`merge`]'s fold the only fold: the tail is an ordinary contiguous piece,
/// so the caller concatenates and merges exactly as it does across workers.
///
/// **Exactly one read is alive at a time**, which is what makes
/// [`WaitPolicy::MayWait`] safe here: each `Bytes` moves into the blocking
/// closure that parses it and drops when that closure returns, so a worker
/// blocked for a pool slot is never itself holding the slot it waits for.
async fn scan_partition(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    entry: PieceEntry,
    range: Range<u64>,
    columns: usize,
    size: u64,
) -> Result<Vec<PieceScan>> {
    let limit = range.end;
    let mut out = Vec::new();
    let mut entry = entry;
    let mut start = range.start;
    let mut want = range.end.saturating_sub(range.start).max(1);

    while start < size {
        let end = (start + want).min(size);
        let len = usize::try_from(end - start).unwrap_or(usize::MAX);
        let bytes = source.read_range(start, len).await?;
        let scan =
            tokio::task::spawn_blocking(move || scan_piece(&bytes, start, entry, limit, columns))
                .await??;
        // The piece is finished when it has read the line ending at or past
        // its limit, when it found the terminator, or when there is no more
        // file — the last being the only one that can leave a piece short, and
        // the leader's to turn into an error.
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
            // failing at the first attempt is what lets a legitimately long
            // row through; `max_line_bytes` is the same ceiling the serial
            // loop enforces, and it is the caller's number.
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
    ///
    /// Synchronous on purpose: the reference must not depend on a source, a
    /// runtime or a chunk size, because those are the very things the split is
    /// allowed to differ in.
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
    /// way a worker would, and merge. The bytes handed to each piece run to the
    /// end of the file, which is the caller's half of the contract: a piece must
    /// be able to finish the line that straddles its limit.
    fn split(file: &[u8], block: &Reference, pieces: usize) -> Option<Interior> {
        cut_up(file, block, pieces).0
    }

    /// The same, reporting how many pieces owned at least one row — which is
    /// what says a passing equality was a real split rather than one piece
    /// doing all the work and the rest owning nothing.
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
    /// split must not be visible in the answer.** It is the same argument
    /// `full.spans == eager.spans` makes about assembly routes, applied to the
    /// one the workers add — so a block cut every which way still reports the
    /// serial scanner's row count, terminator and end, and the serial pass's
    /// census.
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
        // Without this the equalities above could all be one piece owning every
        // row and the rest owning nothing, which is the serial path compared to
        // itself.
        assert!(really_split > 0, "no cut ever gave two pieces rows of their own");
    }

    /// More pieces than rows is the degenerate end of the same property: a
    /// piece with no row start in it owns nothing, and the pieces that do own
    /// the rows still see each of them exactly once.
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
    /// closing the block short — which is what tells the leader to hand out
    /// more of the interior.
    ///
    /// The limit lands one byte into the first row, so the piece owns exactly
    /// that row: it runs through the line ending at the first LF at or after
    /// its limit, and the next piece's resync skips precisely that line.
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

    /// **A `\.` line past the block's end is not a terminator**, because an
    /// earlier piece already found the real one. The pieces after it scan
    /// whatever follows the block as though it were rows; `merge` never reaches
    /// them.
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
        // The piece covering everything after the block, which reads the
        // trailing `\.` as a terminator of its own.
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

    /// The census is a union over the pieces, and it is the union that makes a
    /// mixed column resolvable: a column holding `{1,2}` in one piece and
    /// `{{1,2}}` in another records `(1, 2)`, where either piece alone records
    /// a confidently wrong single depth.
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
    /// that would be misread if it did: the value `a\.`, which COPY TEXT writes
    /// as `a\\.` — so the row's own tail, from its second backslash, is the two
    /// bytes `\.` followed by an LF. I7's guarantee is about a *line start*, so
    /// a piece entered there would read that tail as the block's terminator and
    /// close the block four bytes early. The resync is a real forward read to
    /// the next LF, which is why it does not.
    #[test]
    fn a_cut_inside_a_row_whose_tail_is_a_backslash_dot_is_resynced_past() {
        let file = b"COPY public.t (a) FROM stdin;\na\\\\.\n\\.\n".to_vec();
        let block = &reference(&file)[0];
        assert_eq!(block.interior.end.row_count, 1);
        // The row runs `[30, 35)`; its second backslash is at 32, and from
        // there the bytes read `\.\n` — a terminator line to anyone entering
        // mid-row.
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
    /// 1 MiB every block here is inside one partition and the scheduler
    /// correctly declines every one of them. Announcing 8 bytes as the read
    /// size makes the source advise 64-byte partitions, which turns a 4 KiB
    /// block into tens of windows of several workers each — the shape the
    /// window loop, the tail read and the growth path all need in order to be
    /// exercised at all.
    ///
    /// **The announced size is bounded below by the smallest region any
    /// fixture here ends with**, which is 103 bytes: the floor
    /// [`scan_region`] applies is one whole partition, so a partition larger
    /// than a trailing block's remaining file declines it and the sweep below
    /// stops asserting anything about that block.
    fn scheduled(source: &LocalFileSource, jobs: usize) -> ScanOptions {
        source.hint_read_size(8);
        ScanOptions {
            chunk_size: 8,
            parallelism: Parallelism::workers(jobs, crate::io::DEFAULT_MEMORY_BUDGET),
            ..ScanOptions::default()
        }
    }

    /// **The same property [`a_split_interior_answers_what_the_serial_scanner_answers`]
    /// asserts about the parse, now asserted about the schedule**: the cut the
    /// leader actually makes — the source's own boundaries, its own window
    /// size, its own tail reads — is still invisible in the answer.
    ///
    /// At eight jobs against the local source's four pool slots this also
    /// exercises the wait: four workers hold a slot and four block for one,
    /// and the run completes because a worker's read drops before its parse
    /// returns (`ByteRangeSource::hint_wait_policy`).
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
                        got,
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

    /// **Two ways to be declined, and neither is a branch on the source's
    /// type.** A caller that states no parallelism keeps the serial path, and
    /// a region that cannot hold one of the source's own partitions is left
    /// alone however many jobs were stated — which at the shipped 1 MiB chunk
    /// is every block any fixture has.
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
        assert_eq!(got, RegionScan::Declined);

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
        assert_eq!(got, RegionScan::Declined, "a kilobyte block against a 1 MiB partition");
    }

    /// Cancellation is answered between windows, and it is neither a closed
    /// block nor an error: the caller banks the map it had before the region
    /// and reports the interruption, exactly as the serial loop does.
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
        assert_eq!(got, RegionScan::Cancelled);
    }

    /// **Only the leader may say a block is unterminated**, which is the other
    /// half of `scan_piece` never claiming end of file: a piece that runs out
    /// of bytes reports no terminator, and the party that knows the file ran
    /// out is the one that raises. The offset named is the header's, as the
    /// serial scanner names it.
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

    /// A row longer than the tail read is read by growing the read, not by
    /// losing the row — the same ceiling the serial loop enforces bounds it.
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
            assert_eq!(got, RegionScan::Closed(block.interior.clone()), "{jobs} jobs");
        }
    }
}
