//! Line-anchored structural scanner for `pg_dump` plain-format files.
//!
//! [`CopyScanner`] is a pure synchronous state machine. It never owns or
//! copies the bytes it scans: the caller owns a buffer, hands it out as a
//! slice, and the scanner reports how much of it was consumed. That keeps the
//! events zero-copy and lets the same state machine serve every driver: the
//! serial read loops through [`ChunkedScanner`] (`chunked`), and a leader
//! piece of `pgdt_mapping`'s interior split over each read it makes
//! (`scan_piece`).
//!
//! Robustness rules this implements (see `docs/design/decisions.md`,
//! "D23"):
//!
//! * `COPY` detection is line-anchored. Row data may contain a literal
//!   `COPY ... TO stdout;` mid-line, and a non-anchored search would misfire.
//! * While inside a COPY block the scanner looks for nothing but the `\.`
//!   terminator, so data can never be mistaken for structure. COPY TEXT
//!   escapes a literal backslash as `\\`, so a line that *is* exactly `\.` is
//!   unambiguously the terminator. Each row's line ending is held to its
//!   block's first row's, as a restore holds it (I91).
//! * psql meta-command lines (`\restrict`, `\unrestrict`, `\connect`, and any
//!   other leading-backslash line) are ordinary [`Event::Line`]s: outside a
//!   block only a line matching the full `COPY ... FROM stdin;` grammar, or a
//!   bare `BEGIN;` opening the large-object region, is structural.
//! * Outside a block every line is lexed ([`crate::lex`]), and only a line
//!   that begins between tokens and touches no dollar-quoted body can be
//!   structural: one that begins inside a string, a quoted identifier or a
//!   comment is that region's content.

mod chunked;

use crate::lex::{Lexer, Region, standard_conforming_strings};
use pgdt_core::{CopyHeader, is_terminator, parse_copy_header};
use pgdt_core::{Error, Result, RowEndingRefusal};

pub use self::chunked::{
    ChunkEvents, ChunkedScanner, SCAN_LINE_DEFAULT_MAX_BYTES, ScanOptions, announce_read_loop, scan,
};

/// The start of a COPY data block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyStart {
    pub header: CopyHeader,
    /// Absolute file offset of the header line's first byte — the `C` in
    /// `COPY`, but where spaces or tabs come before it.
    pub header_offset: u64,
    /// Absolute file offset of the first data byte, just past the header
    /// line's newline.
    pub data_offset: u64,
}

/// One raw data row inside a COPY block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row<'a> {
    /// Absolute file offset of the row's first byte.
    pub offset: u64,
    /// Zero-based index of the row within its COPY block.
    pub index: u64,
    /// The row line with its terminating newline stripped, and a `\r` before
    /// it unless a backslash escapes it, when it is the last field's own byte
    /// (I91). Fields are still delimiter-separated and still escaped; use
    /// [`pgdt_core::split_fields`] and [`pgdt_core::unescape_field`].
    pub raw: &'a [u8],
}

/// The end of a COPY data block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyEnd {
    /// Absolute file offset of the `\.` terminator line.
    pub terminator_offset: u64,
    /// Absolute file offset just past the terminator line.
    pub end_offset: u64,
    /// Number of data rows in the block.
    pub row_count: u64,
}

/// One line encountered outside a COPY block that is neither a COPY header
/// nor part of a dollar-quoted string — DDL, comments, blank lines, or a
/// psql meta-command (`\connect`, `\restrict`, ...). This is the raw material
/// `crate::map::Builder::feed_line` classifies into spans, using
/// `crate::ddl`'s grammar. No line reaches a `DumpMetadata` directly:
/// [`pgdt_model::DumpMetadata`] is derived from the finished spans
/// (`docs/design/decisions.md`, "D34"), never from a second pass. A COPY block's data
/// rows never reach this arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line<'a> {
    /// Absolute file offset of the line's first byte.
    pub offset: u64,
    /// The line with its terminating newline stripped, and a `\r` before it
    /// too unless the line ends inside a quoted literal or identifier, where
    /// the `\r` is the value's own byte (I90).
    pub raw: &'a [u8],
}

/// The start of a large-object data region — a `BEGIN;` line outside any
/// COPY block. `pg_backup_archiver.c`'s `StartRestoreLOs()`/`EndRestoreLOs()`
/// are the only emitter of a bare `BEGIN;`/`COMMIT;` pair anywhere in plain
/// `pg_dump` output (I12): a `plpgsql` `BEGIN`/`END` block never reaches this
/// arm at all, since it always sits inside a dollar-quoted function body,
/// which the scanner's dollar-quote tracking already filters out before this
/// check ever runs. Line-anchored and exact, the same rigor
/// [`is_terminator`] applies to `\.`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeObjectStart {
    /// Absolute file offset of the `BEGIN;` line.
    pub start_offset: u64,
}

/// The end of a large-object data region — the `COMMIT;` line closing a
/// `BEGIN;` opened by a prior [`LargeObjectStart`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeObjectEnd {
    /// Absolute file offset just past the `COMMIT;` line.
    pub end_offset: u64,
}

/// The point at which a dollar-quoted region closed, and nothing else.
///
/// **Position-only, deliberately.** `crate::map`'s statement accumulator
/// cannot otherwise observe that a `CREATE FUNCTION` ended: no
/// [`Event::Line`] is emitted for any line inside, entering or leaving a
/// dollar-quoted string — including the one carrying the statement's own
/// closing `;` — so without this the first such body in a TOC-comment-less
/// file absorbs every statement after it into one span. Surfacing the
/// *lines* stays rejected: it would leak a mapping concern into the scanner's event
/// contract, and span text is sliced from the file by offset anyway
/// (`docs/design/decisions.md`, "D30").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DollarQuoteEnd {
    /// Absolute file offset just past the line the region closed on — the
    /// same watermark a following span would start at.
    pub offset: u64,
}

/// An event emitted while scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<'a> {
    CopyStart(CopyStart),
    Row(Row<'a>),
    CopyEnd(CopyEnd),
    Line(Line<'a>),
    DollarQuoteEnd(DollarQuoteEnd),
    LargeObjectStart(LargeObjectStart),
    LargeObjectEnd(LargeObjectEnd),
}

/// How a `COPY` row's line ends, as `CopyReadLineText` reads it (I91): by the
/// first raw CR no backslash escapes, or by its LF where it holds none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowEnding {
    /// No unescaped CR: the row ends at its LF.
    Lf,
    /// The one unescaped CR is the byte before the LF.
    CrLf,
    /// An unescaped CR before the line's last byte, which ends the row where
    /// `COPY` reads it: in a block's first row, rows ending in a bare CR,
    /// which this scanner does not split (`KD101`).
    Cr,
}

impl RowEnding {
    /// How `line`, a row's bytes before its LF, ends.
    fn of(line: &[u8]) -> Self {
        match memchr::memchr_iter(b'\r', line).find(|&at| !escaped(line, at)) {
            None => Self::Lf,
            Some(at) if at + 1 == line.len() => Self::CrLf,
            Some(_) => Self::Cr,
        }
    }

    /// What a restore says of a row ending as `self` in a block whose first row
    /// ended as `block` — `CopyReadLineText`'s refusal, or `None` where it reads
    /// the row. A block of bare-CR rows is checked for nothing (`KD101`).
    pub fn refused_in(self, block: Self) -> Option<RowEndingRefusal> {
        match (block, self) {
            // deficiency: KD101 — `CopyReadLineText` splits such a block's
            // rows at each unescaped CR and ends it at `\.` CR, discarding
            // what psql sends after that up to the next line it reads as `\.`
            // (I91); here its rows are the LF-split lines, each read whole,
            // and nothing holds them to an ending. The lines a restore
            // discards are read as rows, and both resume past psql's `\.`
            // line, so only this block is misread, though a field in it may
            // be refused where PostgreSQL reads it. Not refused as a
            // shortfall: that would fail the whole parse, and this match is
            // PostgreSQL's refusals alone. Reading one needs rows framed by
            // CR in the leader's resync (`pgdt_mapping`'s `interior::scan_piece`) and a query
            // segment's (`pgdt_query`'s `first_row_start`) too, each of which
            // searches for an LF such a block's interior does not hold.
            (Self::Cr, _) => None,
            (block, row) if block == row => None,
            (Self::CrLf, Self::Lf) => Some(RowEndingRefusal::LiteralNewline),
            _ => Some(RowEndingRefusal::LiteralCarriageReturn),
        }
    }
}

/// Whether the byte at `at` is escaped: `COPY` takes the byte after a
/// backslash as data, so a CR behind an odd run of backslashes is no line
/// ending (I91).
fn escaped(line: &[u8], at: usize) -> bool {
    line[..at].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// The rows of one piece of a block's interior, as their endings stand
/// against each other: what the leader holds against the block's first row
/// (`pgdt_mapping`'s `interior::scan_piece`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PieceEndings {
    /// The ending of the piece's first row, and that row's offset.
    pub first: Option<(RowEnding, u64)>,
    /// The first row ending otherwise than `first`: its index among the
    /// piece's rows, its offset and its ending.
    pub mismatch: Option<(u64, u64, RowEnding)>,
}

/// What the interior split asks of a [`CopyScanner`] beyond its API: a piece
/// of a block's interior scanned with its rows' endings recorded rather than
/// checked, the piece being unable to tell its first row from the block's.
pub trait CopyScannerExt {
    fn recording_endings(self) -> Self;
    fn piece_endings(&self) -> PieceEndings;
}

impl CopyScannerExt for CopyScanner {
    /// The same scanner, recording how the rows of the block it was resumed
    /// in end rather than checking nothing, for
    /// [`piece_endings`](CopyScannerExt::piece_endings).
    fn recording_endings(mut self) -> Self {
        self.endings = Endings::Recorded(PieceEndings::default());
        self
    }

    /// What a scanner [`recording_endings`](CopyScannerExt::recording_endings) found.
    fn piece_endings(&self) -> PieceEndings {
        match &self.endings {
            Endings::Recorded(piece) => piece.clone(),
            Endings::Unchecked | Endings::Refused { .. } => PieceEndings::default(),
        }
    }
}

/// How a scanner holds a block's rows to the ending its first row fixed.
#[derive(Debug)]
enum Endings {
    /// Entered inside the block: the pass that entered it at its header
    /// checked its rows, and every replay reads a block that pass mapped.
    Unchecked,
    /// Entered at the header: a row ending otherwise than the first is
    /// refused, naming `table`, as a restore refuses it.
    Refused { table: String, first: Option<RowEnding> },
    /// One piece of the interior, which cannot tell its first row from the
    /// block's: recorded for the leader, which can.
    Recorded(PieceEndings),
}

#[derive(Debug)]
enum State {
    Outside,
    InCopy {
        rows: u64,
        header_offset: u64,
    },
    /// Between a `BEGIN;` and its `COMMIT;` — the large-object data region
    /// (`docs/design/decisions.md`, "D33").
    /// Every line in between is skipped unread, nothing surfaced where
    /// [`State::InCopy`] surfaces each row: I12 guarantees a bytea hex literal can never contain a
    /// line break, so nothing in here can be mistaken for structure.
    InLargeObjectRegion {
        start_offset: u64,
    },
}

/// Incremental, zero-copy scanner over a `pg_dump` plain-format file.
///
/// Usage: repeatedly fill a buffer, drain [`next_event`](Self::next_event)
/// until it yields `None`, then call [`take_consumed`](Self::take_consumed)
/// and drop that many bytes from the front of the buffer before refilling.
/// [`ChunkedScanner`] is that protocol done without copying the buffer, and
/// is what the serial read loops drive the scanner through; a leader piece
/// scans each of its reads as a slice of its own.
#[derive(Debug)]
pub struct CopyScanner {
    /// Absolute file offset that `buf[0]` corresponds to.
    base: u64,
    /// Bytes of the current buffer already turned into events.
    pos: usize,
    state: State,
    /// Where the SQL outside a `COPY` block stands — between tokens, or
    /// inside a string, identifier, comment or dollar-quoted body — and the
    /// `standard_conforming_strings` the file last stated. Always between
    /// tokens while `state` is `InCopy`: a header line begins and ends there.
    lexer: Lexer,
    /// How the open block's rows are held to its first row's ending; read
    /// only while `state` is `InCopy`.
    endings: Endings,
}

impl Default for CopyScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyScanner {
    pub fn new() -> Self {
        Self {
            base: 0,
            pos: 0,
            state: State::Outside,
            lexer: Lexer::new(),
            endings: Endings::Unchecked,
        }
    }

    /// Resume scanning at `offset`, as if `take_consumed` had just been
    /// called there. `in_copy` carries `(header_offset, rows)` when `offset`
    /// falls inside an already-open COPY block — `rows` is how many data
    /// rows of that block have already been consumed, matching what
    /// [`in_copy_rows`](Self::in_copy_rows) reported at the point the caller
    /// captured this position.
    ///
    /// A resume point is a COPY block boundary (a cached block's
    /// `data_offset` or `end_offset`) or a row start inside a block's data
    /// (a leader piece after its resync), none of them inside a quoted
    /// region, so the lexer restarts between tokens. It restarts under
    /// `standard_conforming_strings = on` unless
    /// [`with_standard_strings`](Self::with_standard_strings) says otherwise:
    /// a literal `pg_dump` writes under `off` lexes alike under either (I50).
    ///
    /// **A scanner resumed inside a block checks none of its row endings**:
    /// the pass that entered the block at its header did, and what is resumed
    /// is a block that pass mapped. One entered at a later header checks that
    /// block's.
    pub fn resume(offset: u64, in_copy: Option<(u64, u64)>) -> Self {
        let state = match in_copy {
            Some((header_offset, rows)) => State::InCopy { rows, header_offset },
            None => State::Outside,
        };
        Self { base: offset, pos: 0, state, lexer: Lexer::new(), endings: Endings::Unchecked }
    }

    /// The same scanner, lexing under the `standard_conforming_strings` a
    /// scanner earlier in the file had reached
    /// ([`standard_strings`](Self::standard_strings)).
    pub fn with_standard_strings(mut self, on: bool) -> Self {
        self.lexer.set_standard_strings(on);
        self
    }

    /// The `standard_conforming_strings` the file last stated, `on` before
    /// it states one.
    pub fn standard_strings(&self) -> bool {
        self.lexer.standard_strings()
    }

    /// Absolute file offset of the next unconsumed byte.
    pub fn position(&self) -> u64 {
        self.base + self.pos as u64
    }

    /// Whether the scanner is currently inside a COPY data block.
    pub fn in_copy_block(&self) -> bool {
        matches!(self.state, State::InCopy { .. })
    }

    /// Data rows consumed so far in the current COPY block, or `None` when
    /// not inside one.
    pub fn in_copy_rows(&self) -> Option<u64> {
        match self.state {
            State::InCopy { rows, .. } => Some(rows),
            State::Outside | State::InLargeObjectRegion { .. } => None,
        }
    }

    /// Bytes of the buffer consumed so far. Resets the scanner's buffer
    /// cursor, so the caller must drop exactly this many bytes from the front
    /// of the buffer before calling [`next_event`](Self::next_event) again.
    pub fn take_consumed(&mut self) -> usize {
        let used = self.pos;
        self.base += used as u64;
        self.pos = 0;
        used
    }

    /// Produce the next event from `buf`, which holds the file bytes starting
    /// at [`position`](Self::position) minus the already-consumed prefix.
    ///
    /// `eof` must be true only when `buf` runs to the true end of the file.
    /// Returns `None` when more data is needed (or, at EOF, when the file is
    /// exhausted).
    pub fn next_event<'b>(&mut self, buf: &'b [u8], eof: bool) -> Result<Option<Event<'b>>> {
        loop {
            if self.pos >= buf.len() {
                if eof && let State::InCopy { header_offset, .. } = self.state {
                    return Err(Error::UnterminatedCopyBlock { header_offset });
                }
                if eof && let State::InLargeObjectRegion { start_offset } = self.state {
                    return Err(Error::UnterminatedLargeObjectRegion { start_offset });
                }
                return Ok(None);
            }

            let rest = &buf[self.pos..];
            // A row's line is found by whichever of CR and LF comes first, so
            // a row holding no CR is read in one pass and known to end in LF.
            // deficiency: KD104 — a backslash before a raw LF makes it data,
            // the row running on past it, as one before a raw tab makes that
            // data too (I91); here every LF ends a row, and `split_fields`
            // splits at every tab. `pg_dump` escapes both (I15).
            let (line, advance, holds_cr) = match self.state {
                State::InCopy { .. } => match memchr::memchr2(b'\n', b'\r', rest) {
                    Some(nl) if rest[nl] == b'\n' => (&rest[..nl], nl + 1, false),
                    Some(cr) => match memchr::memchr(b'\n', &rest[cr..]) {
                        Some(nl) => (&rest[..cr + nl], cr + nl + 1, true),
                        None if eof => (rest, rest.len(), true),
                        None => return Ok(None),
                    },
                    None if eof => (rest, rest.len(), false),
                    None => return Ok(None),
                },
                State::Outside | State::InLargeObjectRegion { .. } => {
                    match memchr::memchr(b'\n', rest) {
                        Some(nl) => (&rest[..nl], nl + 1, false),
                        None if eof => (rest, rest.len(), false),
                        None => return Ok(None),
                    }
                }
            };
            // A raw CR before the newline is stripped from a line that ends
            // outside a quoted region, psql reading one between tokens as
            // whitespace; inside a literal or a quoted identifier it is the
            // value's own byte, kept below (I90). A row strips its own (I91).
            let unstripped = line;
            let line = line.strip_suffix(b"\r").unwrap_or(line);

            let line_offset = self.position();
            self.pos += advance;

            match self.state {
                State::Outside => {
                    let between_tokens = *self.lexer.region() == Region::Code;
                    let lexed = self.lexer.line(unstripped, |_| {});
                    // A CR opens and closes no region, so the region the line
                    // ends in is the one its last byte lies in.
                    let surfaced = match self.lexer.region() {
                        Region::Quoted(_) | Region::Identifier => unstripped,
                        _ => line,
                    };
                    if lexed.dollar {
                        // Inside, entering, or leaving a dollar-quoted
                        // string: this line is body text or quoting syntax,
                        // never structure, regardless of what it looks like.
                        // A line that leaves one still reports *where* it did
                        // — see `DollarQuoteEnd`; the line itself stays
                        // unsurfaced.
                        if !matches!(self.lexer.region(), Region::Dollar(_)) {
                            return Ok(Some(Event::DollarQuoteEnd(DollarQuoteEnd {
                                offset: self.position(),
                            })));
                        }
                        continue;
                    }
                    if !between_tokens {
                        // The continuation of a string, a quoted identifier
                        // or a comment: content, surfaced as the statement's
                        // next line and never read as structure.
                        return Ok(Some(Event::Line(Line { offset: line_offset, raw: surfaced })));
                    }

                    // Line-anchored: only a line that both starts with `COPY`,
                    // past spaces and tabs, and matches the full header
                    // grammar is structural.
                    // Everything else — SQL, comments, psql meta-commands —
                    // is surfaced as a line and never as structure, a bare
                    // `BEGIN;` aside (below).
                    if let Some(header) = parse_copy_header(line) {
                        self.state = State::InCopy { rows: 0, header_offset: line_offset };
                        self.endings =
                            Endings::Refused { table: header.qualified_name(), first: None };
                        return Ok(Some(Event::CopyStart(CopyStart {
                            header,
                            header_offset: line_offset,
                            data_offset: self.position(),
                        })));
                    }
                    // Line-anchored and exact, the same as `\.` below — see
                    // `LargeObjectStart`'s docs for why a bare `BEGIN;` is
                    // unambiguous.
                    if line == b"BEGIN;" {
                        self.state = State::InLargeObjectRegion { start_offset: line_offset };
                        return Ok(Some(Event::LargeObjectStart(LargeObjectStart {
                            start_offset: line_offset,
                        })));
                    }
                    // I50: the literal syntax every line after this one is
                    // written in.
                    if let Some(on) = standard_conforming_strings(line) {
                        self.lexer.set_standard_strings(on);
                    }
                    return Ok(Some(Event::Line(Line { offset: line_offset, raw: surfaced })));
                }
                State::InCopy { rows, header_offset } => {
                    // The `\.` line is psql's to find, which it does whatever
                    // the block's rows end in (I91).
                    // deficiency: KD105 — at the file's end with no LF after
                    // it, psql does not find it and the server refuses it, a
                    // corrupt marker, or `\.` CR a mismatched one in a block
                    // of rows (I91); here it ends the block all the same.
                    if is_terminator(line) {
                        self.state = State::Outside;
                        return Ok(Some(Event::CopyEnd(CopyEnd {
                            terminator_offset: line_offset,
                            end_offset: self.position(),
                            row_count: rows,
                        })));
                    }
                    let ending = if holds_cr { RowEnding::of(unstripped) } else { RowEnding::Lf };
                    match &mut self.endings {
                        Endings::Unchecked => {}
                        Endings::Refused { table, first } => match *first {
                            None => *first = Some(ending),
                            Some(block) => {
                                if let Some(refusal) = ending.refused_in(block) {
                                    return Err(Error::RowEndingRefused {
                                        table: std::mem::take(table),
                                        line: rows + 1,
                                        line_offset,
                                        refusal,
                                    });
                                }
                            }
                        },
                        Endings::Recorded(piece) => match piece.first {
                            None => piece.first = Some((ending, line_offset)),
                            Some((first, _)) => {
                                if piece.mismatch.is_none() && ending != first {
                                    piece.mismatch = Some((rows, line_offset, ending));
                                }
                            }
                        },
                    }
                    // A CR a backslash escapes is the last field's byte; one
                    // ending a bare-CR block's line is stripped (`KD101`).
                    let raw = match unstripped.split_last() {
                        Some((b'\r', head)) if !escaped(unstripped, head.len()) => head,
                        _ => unstripped,
                    };
                    self.state = State::InCopy { rows: rows + 1, header_offset };
                    return Ok(Some(Event::Row(Row { offset: line_offset, index: rows, raw })));
                }
                State::InLargeObjectRegion { .. } => {
                    if line == b"COMMIT;" {
                        self.state = State::Outside;
                        return Ok(Some(Event::LargeObjectEnd(LargeObjectEnd {
                            end_offset: self.position(),
                        })));
                    }
                    // Contents are never parsed — see the design doc's "Bulk
                    // regions": there is nothing in a `lo_open`/`lowrite`/
                    // `lo_close` line a query engine wants, so it is skipped
                    // unread rather than surfaced as an event.
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
