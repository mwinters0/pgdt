//! One piece of an open `COPY` block's interior: what a worker answers of
//! it on its own, and how the pieces fold back into the block's totals
//! (`docs/design/decisions.md`, "D52"). [`scan_piece`] is what one worker
//! runs and [`merge`] what the leader does with the answers; both are
//! synchronous and do no I/O, taking a `&[u8]` rather than a source.
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

use std::sync::Arc;

use pgdt_core::{Result, RowEndingRefusal};
use pgdt_dump::internal::{CopyScannerExt, PieceEndings, RowEnding};
use pgdt_dump::{CopyEnd, CopyScanner, Event};
use pgdt_model::internal::BlockCensusExt;
use pgdt_model::{
    BlockCensus,
    internal::{BlockObserver, FieldCount},
};

/// Where a piece's first byte sits relative to the rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PieceEntry {
    /// The piece begins exactly on a row start — the block's own
    /// `data_offset`, just past the header line's LF, or where the piece
    /// before it read through — and needs no resync.
    RowStart,
    /// The piece begins at a cut that knows nothing about rows: its first row
    /// is the one starting just past the first LF at or after the cut.
    Resync,
}

/// **How a data-level block is censused and counted**: its header's width,
/// which sizes both, and what its values are counted against
/// (`docs/design/decisions.md`, "D35", "D96"). `None` wherever one is taken
/// is a block at the metadata level, whose rows are counted and split into
/// fields by a strict parse alone.
#[derive(Clone)]
pub(crate) struct CensusPlan {
    pub width: usize,
    pub counter: Arc<dyn FieldCount>,
}

/// What one worker found in its piece of a `COPY` block's interior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PieceScan {
    /// Data rows this piece owns: every line it consumed before the
    /// terminator, or every line it consumed at all.
    pub rows: u64,
    /// This piece's own census and count, merged with its siblings' by
    /// [`merge`]: nothing may read it before the block closes
    /// (`docs/design/decisions.md`, "D34").
    pub census: BlockCensus,
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
    /// How this piece's rows end against each other, which [`BlockEndings`]
    /// holds against the block's first row.
    pub endings: PieceEndings,
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
    /// The union of the pieces' censuses and counts, through the one that
    /// held the terminator.
    pub census: BlockCensus,
    /// The `CopyEnd` the serial scanner would have emitted, byte for byte
    /// (`docs/design/decisions.md`, "D52").
    pub end: CopyEnd,
}

/// A piece's statistics observer ([`BlockObserver::piece`]), and the block's
/// first data byte, which the rows it is handed are placed against.
pub(super) struct PieceObserver {
    observer: Box<dyn BlockObserver>,
    data_offset: u64,
}

impl PieceObserver {
    /// `observer`, a piece the block's observer made, placing its rows
    /// against `data_offset`.
    pub(super) fn new(observer: Box<dyn BlockObserver>, data_offset: u64) -> Self {
        Self { observer, data_offset }
    }

    /// The piece's observer, for the block's to absorb.
    pub(super) fn into_observer(self) -> Box<dyn BlockObserver> {
        self.observer
    }
}

/// Scan one piece of an open `COPY` block's interior.
///
/// `bytes` are the file's bytes starting at absolute offset `base`, and they
/// must run **past** `limit` far enough to complete the line that straddles it
/// — the caller's job. A piece handed exactly `[base, limit)` is one whose
/// last line was cut short and therefore not counted.
///
/// `census` sizes the census the way `map::Builder::on_copy_start` does, from
/// the header's column list, and a row wider than it grows it
/// (`BlockCensusExt::observe_row`); `None` is a block at the metadata level, whose
/// rows are counted and split into fields by a strict parse alone (`docs/design/decisions.md`, "D35"). Every row the piece owns is handed to `observer`, where the block
/// gathers statistics. **Synchronous and free of I/O**: this is the body a
/// `spawn_blocking` task runs, once per read of the piece, so it takes a slice
/// rather than a source (`docs/design/decisions.md`, "D52").
pub(super) fn scan_piece(
    bytes: &[u8],
    base: u64,
    entry: PieceEntry,
    limit: u64,
    census: Option<&CensusPlan>,
    mut observer: Option<&mut PieceObserver>,
) -> Result<PieceScan> {
    let empty = |through: u64, next: PieceEntry| PieceScan {
        rows: 0,
        census: empty_census(census),
        terminator: None,
        through,
        endings: PieceEndings::default(),
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
    let mut scanner = CopyScanner::resume(start, Some((0, 0))).recording_endings();
    let plan = census;
    let mut census = empty_census(plan);
    let mut rows = 0u64;
    let mut terminator = None;

    while let Some(event) = scanner.next_event(span, false)? {
        match event {
            Event::Row(row) => {
                rows += 1;
                if let Some(plan) = plan {
                    census.observe_row(plan.counter.as_ref(), row.raw);
                }
                if let Some(piece) = &mut observer {
                    piece.observer.observe_row(row.offset - piece.data_offset, row.raw);
                }
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
        endings: scanner.piece_endings(),
        next: PieceEntry::RowStart,
    })
}

/// **The block's rows held to its first row's ending, piece by piece in file
/// order** (I91): a piece's own rows are held to its first, which is held to
/// the block's, so the first refusal found is the one a serial scan meets.
/// Nothing past the piece holding the terminator is held, being past the
/// block.
#[derive(Debug, Default)]
pub(super) struct BlockEndings {
    /// The block's first row's ending, once a piece has held a row.
    first: Option<RowEnding>,
    /// The block's rows in the pieces held so far.
    rows: u64,
    /// Whether a piece held so far held the terminator.
    closed: bool,
}

impl BlockEndings {
    /// Hold `piece`, the next in file order: the `(line, offset, refusal)` of
    /// the first row in it a restore refuses for its ending, if any.
    pub(super) fn hold(&mut self, piece: &PieceScan) -> Option<(u64, u64, RowEndingRefusal)> {
        if self.closed {
            return None;
        }
        let rows = self.rows;
        self.rows += piece.rows;
        self.closed = piece.terminator.is_some();
        let (ending, offset) = piece.endings.first?;
        let block = *self.first.get_or_insert(ending);
        if let Some(refusal) = ending.refused_in(block) {
            return Some((rows + 1, offset, refusal));
        }
        let (index, offset, row) = piece.endings.mismatch?;
        row.refused_in(block).map(|refusal| (rows + index + 1, offset, refusal))
    }
}

/// A census for a block as `plan` sizes it, before a row is folded in —
/// empty for a block at the metadata level, which folds none.
pub(super) fn empty_census(plan: Option<&CensusPlan>) -> BlockCensus {
    plan.map_or_else(BlockCensus::default, |plan| BlockCensus::new(plan.width))
}

/// Fold `pieces` — one open `COPY` block's interior, in file order — into the
/// block's totals, stopping at the first piece that held the terminator.
/// `None` where none of them did: the leader's window did not reach the end of
/// the block and must hand out more of the interior.
///
/// **The fold is order-dependent only in where it stops.** Row counts and
/// unrepresentable counts sum and censuses union, `ArrayShape`'s merge being
/// a bounded semilattice. What the
/// order decides is which pieces are *in* the fold, everything past the
/// terminator not being part of the block at all.
pub(super) fn merge(pieces: &[PieceScan]) -> Option<Interior> {
    let mut census = BlockCensus::default();
    let mut row_count = 0u64;
    for piece in pieces {
        census.merge(&piece.census);
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
