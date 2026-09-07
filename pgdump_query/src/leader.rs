//! The interior split: what one piece of an open `COPY` block's interior
//! answers on its own, and how the pieces fold back into the block's totals
//! (`docs/design/architecture.md`, "The interior split").
//!
//! A serial leader that has just read `COPY … FROM stdin;` knows every byte
//! until `\.` is line-structured rows, so the interior can be handed out in
//! LF-split ranges to workers that never parse *structure* — the leader has
//! already proved there is none in there to find. This module is the **parse**
//! half of that: [`scan_piece`] is what one worker runs, and [`merge`] is what
//! the leader does with the answers. Neither reads a source, spawns anything,
//! or knows how the cuts were chosen; the scheduler that supplies both is
//! `16.10.1`'s.
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
//! **Nothing calls this yet, deliberately.** The scheduler that cuts the
//! interior, reads the pieces and folds [`merge`]'s answer into
//! `map::Builder` is `16.10.1`'s, and it lands after this half so that the
//! parse is checked against a reference it did not produce
//! (`docs/process.md`, "Size a slice by its review, not by its scope"). That
//! is the same shape
//! `ByteRangeSource::partitions` and `Parallelism` landed in, and it is why
//! the module allows dead code rather than exporting a surface for the sake of
//! being called.
#![allow(dead_code)]

use crate::Result;
use crate::index::ArrayShape;
use crate::map::census_row;
use crate::scan::{CopyEnd, CopyScanner, Event};

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
}

/// The block's totals, as the leader states them at `CopyEnd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interior {
    /// The union of the pieces' censuses, up to and including the one that
    /// held the terminator.
    pub census: Vec<ArrayShape>,
    /// The `CopyEnd` the serial scanner would have emitted — byte for byte,
    /// which is what makes a parallel scan's cache equal a serial one's
    /// (`docs/design/roadmap-P16-parallel-scan.md`, "A parallel scan's cache is
    /// byte-identical to a serial one").
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
/// (`docs/design/roadmap-P16-parallel-scan.md`, "A worker decodes and parses in
/// one thread").
pub(crate) fn scan_piece(
    bytes: &[u8],
    base: u64,
    entry: PieceEntry,
    limit: u64,
    columns: usize,
) -> Result<PieceScan> {
    let empty = |through: u64| PieceScan {
        rows: 0,
        census: vec![ArrayShape::default(); columns],
        terminator: None,
        through,
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
            // of it, and that row belongs to the piece before this one.
            None => return Ok(empty(base + bytes.len() as u64)),
        },
    };
    // Two cuts fell inside one row. The row belongs to the piece before this
    // one, so this piece is empty rather than a duplicate.
    if start > limit {
        return Ok(empty(start));
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

    Ok(PieceScan { rows, census, terminator, through: scanner.position() })
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
        if piece.census.len() > census.len() {
            census.resize(piece.census.len(), ArrayShape::default());
        }
        for (slot, shape) in census.iter_mut().zip(&piece.census) {
            slot.merge(shape);
        }
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::map::{Builder, DataBlock, SpanBody};
    use crate::scan::ChunkCarry;

    /// One `COPY` block as the **serial** pass states it: the reference every
    /// split below is checked against.
    #[derive(Debug, PartialEq, Eq)]
    struct Reference {
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
}
