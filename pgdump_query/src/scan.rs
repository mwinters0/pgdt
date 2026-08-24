//! Line-anchored structural scanner for `pg_dump` plain-format files.
//!
//! [`CopyScanner`] is a pure synchronous state machine. It never owns or
//! copies the bytes it scans: the caller owns a buffer, hands it out as a
//! slice, and the scanner reports how much of it was consumed. That keeps the
//! events zero-copy and lets the same state machine serve both the async
//! driver here ([`scan`]) and any future pull-mode stream.
//!
//! Robustness rules this implements (see `docs/design/roadmap-phase1-mvp.md`,
//! "Parser robustness requirements"):
//!
//! * `COPY` detection is line-anchored. Row data may contain a literal
//!   `COPY ... TO stdout;` mid-line, and a non-anchored search would misfire.
//! * While inside a COPY block the scanner looks for nothing but the `\.`
//!   terminator, so data can never be mistaken for structure. COPY TEXT
//!   escapes a literal backslash as `\\`, so a line that *is* exactly `\.` is
//!   unambiguously the terminator.
//! * psql meta-command lines (`\restrict`, `\unrestrict`, `\connect`, and any
//!   other leading-backslash line) are skipped: outside a block only a line
//!   matching the full `COPY ... FROM stdin;` grammar is structural.

use std::ops::ControlFlow;

use crate::copy::{CopyHeader, is_terminator, parse_copy_header, scan_dollar_quotes};
use crate::io::ByteRangeSource;
use crate::{Error, Result};

/// The start of a COPY data block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyStart {
    pub header: CopyHeader,
    /// Absolute file offset of the `C` in `COPY`.
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
    /// The row line with its terminating newline (and any `\r` before it)
    /// stripped. Fields are still delimiter-separated and still escaped; use
    /// [`crate::copy::split_fields`] and [`crate::copy::decode_field`].
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
/// `crate::preamble` parses into a [`crate::index::DumpMetadata`]; every
/// other caller ignores it. Cheap to emit: outside a COPY block's data rows,
/// which never reach this arm, the preamble of even a multi-database dump is
/// a few thousand lines against however many billion rows follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line<'a> {
    /// Absolute file offset of the line's first byte.
    pub offset: u64,
    /// The line with its terminating newline (and any `\r` before it)
    /// stripped.
    pub raw: &'a [u8],
}

/// The point at which a dollar-quoted region closed, and nothing else.
///
/// **Position-only, deliberately.** `crate::map`'s statement accumulator
/// cannot otherwise observe that a `CREATE FUNCTION` ended: no
/// [`Event::Line`] is emitted for any line inside, entering or leaving a
/// dollar-quoted string — including the one carrying the statement's own
/// closing `;` — so without this the first such body in a TOC-comment-less
/// file absorbs every statement after it into one span
/// (`docs/status/history/2026-08-23.md`, measured). Surfacing the *lines*
/// stays rejected: it would leak a Phase 3 concern into L1's event contract,
/// and span text is sliced from the file by offset anyway
/// (`docs/design/roadmap-phase3-object-inventory.md`, "Span boundaries").
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
}

#[derive(Debug)]
enum State {
    Outside,
    InCopy { rows: u64, header_offset: u64 },
}

/// Incremental, zero-copy scanner over a `pg_dump` plain-format file.
///
/// Usage: repeatedly fill a buffer, drain [`next_event`](Self::next_event)
/// until it yields `None`, then call [`take_consumed`](Self::take_consumed)
/// and drop that many bytes from the front of the buffer before refilling.
#[derive(Debug)]
pub struct CopyScanner {
    /// Absolute file offset that `buf[0]` corresponds to.
    base: u64,
    /// Bytes of the current buffer already turned into events.
    pos: usize,
    state: State,
    /// The `$tag$` delimiter of a dollar-quoted string currently open
    /// outside a COPY block, if any. Always `None` while `state` is
    /// `InCopy` — a dollar-quoted body can never appear inside COPY data,
    /// only in the DDL around it (`docs/design/postgres-invariants.md` I1).
    dollar_tag: Option<Vec<u8>>,
}

impl Default for CopyScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyScanner {
    pub fn new() -> Self {
        Self { base: 0, pos: 0, state: State::Outside, dollar_tag: None }
    }

    /// Resume scanning at `offset`, as if `take_consumed` had just been
    /// called there. `in_copy` carries `(header_offset, rows)` when `offset`
    /// falls inside an already-open COPY block — `rows` is how many data
    /// rows of that block have already been consumed, matching what
    /// [`in_copy_rows`](Self::in_copy_rows) reported at the point the caller
    /// captured this position.
    ///
    /// A resume point is always a COPY block boundary (a cached block's
    /// `data_offset` or `end_offset`), which is never inside a dollar-quoted
    /// string, so dollar-quote tracking always restarts clean.
    pub fn resume(offset: u64, in_copy: Option<(u64, u64)>) -> Self {
        let state = match in_copy {
            Some((header_offset, rows)) => State::InCopy { rows, header_offset },
            None => State::Outside,
        };
        Self { base: offset, pos: 0, state, dollar_tag: None }
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
            State::Outside => None,
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
                return Ok(None);
            }

            let rest = &buf[self.pos..];
            let (line, advance) = match memchr::memchr(b'\n', rest) {
                Some(nl) => (&rest[..nl], nl + 1),
                None if eof => (rest, rest.len()),
                None => return Ok(None),
            };
            // A raw CR before the newline is never data: COPY TEXT escapes an
            // in-value carriage return as `\r`.
            let line = line.strip_suffix(b"\r").unwrap_or(line);

            let line_offset = self.position();
            self.pos += advance;

            match self.state {
                State::Outside => {
                    let (tag, touched) = scan_dollar_quotes(line, self.dollar_tag.take());
                    let closed = touched && tag.is_none();
                    self.dollar_tag = tag;
                    if touched {
                        // Inside, entering, or leaving a dollar-quoted
                        // string: this line is body text or quoting syntax,
                        // never structure, regardless of what it looks like.
                        // A line that leaves one still reports *where* it did
                        // — see `DollarQuoteEnd`; the line itself stays
                        // unsurfaced.
                        if closed {
                            return Ok(Some(Event::DollarQuoteEnd(DollarQuoteEnd {
                                offset: self.position(),
                            })));
                        }
                        continue;
                    }

                    // Line-anchored: only a line that both starts with `COPY`
                    // and matches the full header grammar is structural.
                    // Everything else — SQL, comments, psql meta-commands —
                    // is skipped.
                    if let Some(header) = parse_copy_header(line) {
                        self.state = State::InCopy { rows: 0, header_offset: line_offset };
                        return Ok(Some(Event::CopyStart(CopyStart {
                            header,
                            header_offset: line_offset,
                            data_offset: self.position(),
                        })));
                    }
                    return Ok(Some(Event::Line(Line { offset: line_offset, raw: line })));
                }
                State::InCopy { rows, header_offset } => {
                    if is_terminator(line) {
                        self.state = State::Outside;
                        return Ok(Some(Event::CopyEnd(CopyEnd {
                            terminator_offset: line_offset,
                            end_offset: self.position(),
                            row_count: rows,
                        })));
                    }
                    self.state = State::InCopy { rows: rows + 1, header_offset };
                    return Ok(Some(Event::Row(Row {
                        offset: line_offset,
                        index: rows,
                        raw: line,
                    })));
                }
            }
        }
    }
}

/// Tuning knobs for a full-file scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Bytes requested per read from the source.
    pub chunk_size: usize,
    /// Hard cap on a single line's length. A dump whose lines exceed this is
    /// rejected rather than buffered without bound — the scanner cannot emit
    /// a row until it has the whole line, so this is the only thing standing
    /// between a malformed input and unbounded memory growth.
    pub max_line_bytes: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { chunk_size: 1 << 20, max_line_bytes: 64 << 20 }
    }
}

/// Scan `source` from the beginning, invoking `on_event` for every event.
///
/// The callback may return [`ControlFlow::Break`] to stop early.
pub async fn scan<S, F>(source: &S, options: &ScanOptions, mut on_event: F) -> Result<()>
where
    S: ByteRangeSource,
    F: FnMut(Event<'_>) -> ControlFlow<()>,
{
    let size = source.size().await?;
    let mut scanner = CopyScanner::new();
    let mut buf: Vec<u8> = Vec::with_capacity(options.chunk_size);
    let mut read_pos = 0u64;

    loop {
        let want = options.chunk_size.min((size - read_pos) as usize);
        if want > 0 {
            let bytes = source.read_range(read_pos, want).await?;
            buf.extend_from_slice(&bytes);
            read_pos += bytes.len() as u64;
        }
        let eof = read_pos >= size;

        while let Some(event) = scanner.next_event(&buf, eof)? {
            if on_event(event).is_break() {
                return Ok(());
            }
        }

        let used = scanner.take_consumed();
        buf.drain(..used);

        if eof {
            return Ok(());
        }
        if buf.len() > options.max_line_bytes {
            return Err(Error::LineTooLong {
                offset: scanner.position(),
                limit: options.max_line_bytes,
            });
        }
    }
}
