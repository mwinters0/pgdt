//! Line-anchored structural scanner for `pg_dump` plain-format files.
//!
//! [`CopyScanner`] is a pure synchronous state machine. It never owns or
//! copies the bytes it scans: the caller owns a buffer, hands it out as a
//! slice, and the scanner reports how much of it was consumed. That keeps the
//! events zero-copy and lets the same state machine serve both the async
//! driver here ([`scan`]) and any future pull-mode stream.
//!
//! Robustness rules this implements (see `docs/design/architecture.md`,
//! "Parser robustness requirements (hardcoded)"):
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;

use crate::copy::{CopyHeader, is_terminator, parse_copy_header, scan_dollar_quotes};
use crate::io::{ByteRangeSource, HolderClass, Parallelism};
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
/// file absorbs every statement after it into one span
/// (`docs/status/history/2026-08-23.md`, measured). Surfacing the *lines*
/// stays rejected: it would leak a mapping concern into L1's event contract,
/// and span text is sliced from the file by offset anyway
/// (`docs/design/architecture.md`, "Three things close a statement").
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

#[derive(Debug)]
enum State {
    Outside,
    InCopy {
        rows: u64,
        header_offset: u64,
    },
    /// Between a `BEGIN;` and its `COMMIT;` — the large-object data region
    /// (`docs/design/architecture.md`, "Bulk regions: one span kind, three payloads").
    /// Every line in between is skipped unread, the same way [`State::InCopy`]
    /// skips row bytes: I12 guarantees a bytea hex literal can never contain a
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
/// [`ChunkCarry`] is that protocol done without copying the buffer, and is
/// what every read loop in this crate drives the scanner through.
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
                    // Line-anchored and exact, the same as `\.` below — see
                    // `LargeObjectStart`'s docs for why a bare `BEGIN;` is
                    // unambiguous.
                    if line == b"BEGIN;" {
                        self.state = State::InLargeObjectRegion { start_offset: line_offset };
                        return Ok(Some(Event::LargeObjectStart(LargeObjectStart {
                            start_offset: line_offset,
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

/// Which of a chunk's two spans a read loop is scanning.
///
/// [`ChunkCarry::PASSES`] is the order, and it is the whole of the protocol:
/// the straddling line first, then the rest of the chunk in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPass {
    /// The line straddling the chunk's front edge — what was carried over
    /// from the previous chunk, completed by this one's first line-terminated
    /// prefix. The only bytes a read loop copies.
    Carry,
    /// The rest of the chunk, scanned where the reader left it.
    InPlace,
}

/// The one line a chunked read loop has to carry across a chunk boundary.
///
/// [`CopyScanner`] consumes whole lines, so whatever a chunk leaves
/// unconsumed is always a **single unterminated line** — never more, since
/// [`next_event`](CopyScanner::next_event) stops only where it finds no
/// newline. That is the entirety of what the next chunk needs joined to it,
/// so a chunk is scanned in two passes: the carry with the chunk's first
/// line-terminated prefix appended, then the chunk's remainder **where it
/// lies**. Per chunk that is one row's worth of copying instead of the
/// chunk's whole length.
///
/// *Rejected:* one growing buffer per read loop, appended to per chunk and
/// `drain`ed of the consumed prefix. It copies every byte of the file twice —
/// once in, once when the remainder shifts down — and was **38.0% of a warm
/// `parse`'s user time**, the largest single term left in it
/// (`docs/design/architecture.md`, "parse-profile").
///
/// **Handing the scanner two buffers within one chunk costs it nothing**:
/// [`CopyScanner::base`] is an absolute file offset, and each pass is
/// bracketed by [`take_consumed`](CopyScanner::take_consumed) exactly as a
/// single buffer's refill would be.
///
/// The degenerate case is a chunk containing no newline at all: the whole of
/// it joins the carry and the in-place pass is empty, which is the growth
/// [`ScanOptions::max_line_bytes`] bounds and is what the one-buffer shape
/// did on every chunk.
#[derive(Debug, Default)]
pub struct ChunkCarry {
    /// The unterminated line carried over, extended by [`absorb`](Self::absorb)
    /// with the chunk bytes that complete it.
    buf: Vec<u8>,
    /// Where in the current chunk [`ChunkPass::InPlace`] begins — set by
    /// [`absorb`](Self::absorb), meaningless before the first call.
    split: usize,
}

impl ChunkCarry {
    /// The passes of one chunk, in the order they must be scanned.
    pub const PASSES: [ChunkPass; 2] = [ChunkPass::Carry, ChunkPass::InPlace];

    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes carried over from earlier chunks. This is what
    /// [`ScanOptions::max_line_bytes`] bounds: it is a single line's prefix,
    /// so a file whose lines fit stays flat here however large it is.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Take onto the carry whatever prefix of `chunk` completes the line it
    /// holds, and fix where the in-place pass begins.
    ///
    /// With nothing carried that is `0` — the chunk is scanned whole, where it
    /// lies. With a carry and no newline anywhere in `chunk`, it is
    /// `chunk.len()`.
    pub fn absorb(&mut self, chunk: &[u8]) {
        if self.buf.is_empty() {
            self.split = 0;
            return;
        }
        self.split = memchr::memchr(b'\n', chunk).map_or(chunk.len(), |nl| nl + 1);
        self.buf.extend_from_slice(&chunk[..self.split]);
    }

    /// The bytes `pass` scans, and whether they run to the true end of the
    /// file — which the carry pass does only when nothing of the chunk is
    /// left over for the in-place pass to see.
    pub fn span<'a>(&'a self, pass: ChunkPass, chunk: &'a [u8], eof: bool) -> (&'a [u8], bool) {
        match pass {
            ChunkPass::Carry => (&self.buf, eof && self.split == chunk.len()),
            ChunkPass::InPlace => {
                // **The carry is empty here whenever this span has anything in
                // it**, and that is what keeps `CopyScanner::base` correct
                // across the switch of buffers: a carry the scanner did not
                // finish would leave `base` short of `chunk[self.split]`, and
                // the in-place span would be mis-based by exactly the residue.
                // It holds because a carry that gained a line-terminated prefix
                // is entirely consumable, and the only carry that is not — a
                // chunk with no newline in it — took the whole chunk, leaving
                // this span empty.
                debug_assert!(
                    self.buf.is_empty() || self.split == chunk.len(),
                    "an unfinished carry beside a non-empty in-place span mis-bases the scanner"
                );
                (&chunk[self.split..], eof)
            }
        }
    }

    /// Record that the scanner consumed `used` bytes of `pass`'s span. After
    /// the in-place pass this is what makes the next chunk's carry: the tail
    /// the scanner could not finish, which is at most one line.
    pub fn consumed(&mut self, pass: ChunkPass, chunk: &[u8], used: usize) {
        match pass {
            ChunkPass::Carry => {
                self.buf.drain(..used);
            }
            ChunkPass::InPlace => self.buf.extend_from_slice(&chunk[self.split + used..]),
        }
    }
}

/// Bytes requested per read from the source, unless a caller says otherwise.
///
/// **One measured constant, not a runtime probe** — the shipped default is the
/// one that is worst-case-best across the device classes measured
/// (`docs/design/measurements.md`, "What the read chunk size is worth"). It is
/// named rather than written inline because it is what
/// [`ScanOptions::chunk_size`] is compared against, and because the
/// measurement that chose it has to be able to name the value it chose.
///
/// **It is public for callers that compare against or scale from the
/// default**, which `ScanOptions::default().chunk_size` serves badly: it
/// builds a whole options struct to read one number, and it is not a constant
/// expression. The CLI's use of it is incidental to that.
///
/// **Raising it costs memory, not pooling.** Every read loop announces the
/// size it is about to repeat ([`crate::ByteRangeSource::hint_read_size`]), so
/// a chunk of any size is kept and reused by the local source's buffer pool
/// rather than allocated and zeroed afresh. What a larger chunk does cost is
/// the pool holding four buffers of it, or the caller's stated memory
/// budget's worth ([`crate::Parallelism`], defaulting to
/// [`crate::DEFAULT_MEMORY_BUDGET`]), whichever is fewer — the slot count falls
/// out of that budget, so the cost levels off rather than scaling with the size
/// (`docs/design/architecture.md`, "Execution model and API surface").
pub const DEFAULT_CHUNK_SIZE: usize = 1 << 20;

/// Tuning knobs for a full-file scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Bytes requested per read from the source. Defaults to
    /// [`DEFAULT_CHUNK_SIZE`].
    pub chunk_size: usize,
    /// Hard cap on a single line's length. A dump whose lines exceed this is
    /// rejected rather than buffered without bound — the scanner cannot emit
    /// a row until it has the whole line, so this is the only thing standing
    /// between a malformed input and unbounded memory growth.
    pub max_line_bytes: usize,
    /// Cooperative cancellation: set this flag from another task and the
    /// mapping loop stops at the next chunk boundary, persists what it holds
    /// and reports that it was interrupted
    /// (`docs/design/architecture.md`, "`parse` resumes, and saves as it
    /// goes"). `None` — the default — is a scan nobody can stop.
    ///
    /// **Chunk granularity is the point**, not `CopyEnd` granularity: a
    /// single `COPY` block can be hundreds of gigabytes, and a Ctrl-C that
    /// waits for the next block boundary cannot be told from a hang.
    ///
    /// **Only [`crate::stream::map_forward`] reads it** — the mapping loop
    /// behind `pgdq parse` and `pgdq query`, which is the one driver with
    /// somewhere to put a partial result (the cache) and a way to report the
    /// stop. [`scan`] and the eager producers built on it ignore it, because
    /// stopping there would be indistinguishable from reaching EOF and would
    /// silently truncate the index they return.
    pub cancel: Option<Arc<AtomicBool>>,
    /// How much concurrency this scan may use, and what it may hold while it
    /// does — [`Parallelism::Serial`] by default, which is the serial code
    /// path this build has rather than a pool of one
    /// (`docs/design/architecture.md`, "Execution model and API surface").
    ///
    /// **The read path's buffer budget reads it; no worker scheduler does
    /// yet.** Every read loop announces it to the source
    /// ([`crate::ByteRangeSource::hint_parallelism`]), which sizes its pools
    /// from the byte half and — for a compressed source — decides from it
    /// whether a whole block can be decoded at all. The `jobs` half is that
    /// source's retention depth, one decoded block per concurrent reader; the
    /// scheduler that would run those readers is not in this build, so a
    /// caller that sets this still gets the serial path, executing it inside
    /// the memory it asked for.
    pub parallelism: Parallelism,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_line_bytes: 64 << 20,
            cancel: None,
            parallelism: Parallelism::default(),
        }
    }
}

impl ScanOptions {
    /// Whether a caller has asked this scan to stop. `Relaxed` is the right
    /// ordering: the flag guards nothing but itself — the reader's response
    /// is to finish the chunk it already holds and save the index it already
    /// owns — so nothing is published through it.
    pub fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
}

/// Scan `source` from the beginning, invoking `on_event` for every event.
///
/// The callback may return [`ControlFlow::Break`] to stop early.
pub async fn scan<F>(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    mut on_event: F,
) -> Result<()>
where
    F: FnMut(Event<'_>) -> ControlFlow<()>,
{
    let size = source.size().await?;
    // The chunk length this loop will ask for until EOF, announced once so a
    // buffer-recycling source can keep one of that size whatever it is
    // (`ByteRangeSource::hint_read_size`), and the budget the caller allows it
    // to keep them inside (`ByteRangeSource::hint_parallelism`).
    source.hint_read_size(options.chunk_size);
    source.hint_parallelism(options.parallelism);
    // **This loop consumes each chunk before it reads the next**, so its reads
    // may wait for a pooled slot rather than allocating past that budget
    // (`ByteRangeSource::hint_holder_class`). Nothing here outlives the
    // iteration that read it: the carry copies what it keeps, and an `Event`
    // borrows only for the callback.
    source.hint_holder_class(HolderClass::Transient);
    let mut scanner = CopyScanner::new();
    let mut carry = ChunkCarry::new();
    let mut read_pos = 0u64;

    loop {
        let want = options.chunk_size.min((size - read_pos) as usize);
        let chunk = if want > 0 {
            let bytes = source.read_range(read_pos, want).await?;
            read_pos += bytes.len() as u64;
            bytes
        } else {
            Bytes::new()
        };
        let eof = read_pos >= size;

        carry.absorb(&chunk);
        for pass in ChunkCarry::PASSES {
            let (span, span_eof) = carry.span(pass, &chunk, eof);
            while let Some(event) = scanner.next_event(span, span_eof)? {
                if on_event(event).is_break() {
                    return Ok(());
                }
            }
            carry.consumed(pass, &chunk, scanner.take_consumed());
        }

        if eof {
            return Ok(());
        }
        if carry.len() > options.max_line_bytes {
            return Err(Error::LineTooLong {
                offset: scanner.position(),
                limit: options.max_line_bytes,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive [`CopyScanner`] over `file` in `chunk_size` pieces exactly as the
    /// three read loops do, and report the events rendered as text alongside
    /// the high-water mark of the carry — which is the number this whole
    /// mechanism exists to hold down.
    fn drive(file: &[u8], chunk_size: usize) -> (Vec<String>, usize) {
        let mut scanner = CopyScanner::new();
        let mut carry = ChunkCarry::new();
        let mut events = Vec::new();
        let mut read_pos = 0usize;
        let mut high_water = 0usize;
        loop {
            let want = chunk_size.min(file.len() - read_pos);
            let chunk = &file[read_pos..read_pos + want];
            read_pos += want;
            let eof = read_pos >= file.len();

            carry.absorb(chunk);
            for pass in ChunkCarry::PASSES {
                let (span, span_eof) = carry.span(pass, chunk, eof);
                while let Some(event) = scanner.next_event(span, span_eof).unwrap() {
                    events.push(format!("{event:?}"));
                }
                carry.consumed(pass, chunk, scanner.take_consumed());
            }
            high_water = high_water.max(carry.len());

            if eof {
                return (events, high_water);
            }
        }
    }

    /// A carry holding `line`, reached the way a read loop reaches one: a chunk
    /// the scanner could not finish, whose tail it kept.
    fn carrying(line: &[u8]) -> ChunkCarry {
        let mut carry = ChunkCarry::new();
        carry.absorb(line);
        carry.consumed(ChunkPass::InPlace, line, 0);
        assert_eq!(carry.len(), line.len());
        carry
    }

    fn control() -> Vec<u8> {
        let mut file = b"--\n-- A comment\n--\nCOPY public.t (a, b) FROM stdin;\n".to_vec();
        for i in 0..64u32 {
            file.extend_from_slice(format!("{i}\tvalue-{i}\n").as_bytes());
        }
        file.extend_from_slice(b"\\.\n\nSELECT 1;\n");
        file
    }

    /// The claim the whole mechanism rests on: what a chunk leaves unconsumed
    /// is one unterminated line, so the bytes a read loop copies are bounded
    /// by the longest line and not by the chunk size. A one-buffer loop's
    /// high-water mark would be a chunk.
    #[test]
    fn the_carry_is_bounded_by_the_longest_line_not_by_the_chunk() {
        let file = control();
        let longest = file.split(|&b| b == b'\n').map(<[u8]>::len).max().unwrap();
        for chunk_size in [1usize, 2, 3, 7, 13, 64, 511, 4096] {
            let (_, high_water) = drive(&file, chunk_size);
            assert!(
                high_water <= longest,
                "chunk_size {chunk_size} carried {high_water} bytes, longest line is {longest}"
            );
        }
    }

    /// Two buffers within one chunk are as good as one, because
    /// `CopyScanner::base` is an absolute file offset — so every event,
    /// offsets included, is the same wherever the boundaries fall.
    #[test]
    fn the_event_stream_does_not_depend_on_where_the_split_falls() {
        let file = control();
        let (reference, _) = drive(&file, 1 << 20);
        for chunk_size in [1usize, 2, 3, 7, 13, 64, 511, 4096] {
            let (got, _) = drive(&file, chunk_size);
            assert_eq!(got, reference, "chunk_size {chunk_size}");
        }
    }

    /// With nothing carried the chunk is scanned whole, where it lies: no
    /// prefix is taken and the in-place span is the chunk itself.
    #[test]
    fn an_empty_carry_copies_nothing() {
        let mut carry = ChunkCarry::new();
        carry.absorb(b"one\ntwo\n");
        assert!(carry.is_empty());
        assert_eq!(carry.span(ChunkPass::Carry, b"one\ntwo\n", false).0, b"");
        assert_eq!(carry.span(ChunkPass::InPlace, b"one\ntwo\n", false).0, b"one\ntwo\n");
    }

    /// The carry takes the chunk's first line-terminated prefix and no more —
    /// one line's worth of copying, whatever the chunk's length.
    #[test]
    fn a_carry_takes_only_the_prefix_that_completes_its_line() {
        let mut carry = carrying(b"abc");

        let chunk = b"def\nghi\njkl";
        carry.absorb(chunk);
        let (span, _) = carry.span(ChunkPass::Carry, chunk, false);
        assert_eq!(span, b"abcdef\n");
        // The scanner would consume that whole line, which is the protocol's
        // precondition for the in-place span below.
        carry.consumed(ChunkPass::Carry, chunk, span.len());
        assert_eq!(carry.span(ChunkPass::InPlace, chunk, false).0, b"ghi\njkl");
    }

    /// The degenerate case, and it is the one-buffer loop's behaviour: a chunk
    /// with no newline in it joins the carry whole, which is the growth
    /// `ScanOptions::max_line_bytes` bounds.
    #[test]
    fn a_chunk_with_no_newline_joins_the_carry_whole() {
        let mut carry = carrying(b"abc");

        let chunk = b"defghi";
        carry.absorb(chunk);
        assert_eq!(carry.span(ChunkPass::Carry, chunk, false).0, b"abcdefghi");
        assert_eq!(carry.span(ChunkPass::InPlace, chunk, false).0, b"");
    }

    /// The carry pass runs to the end of the file only when the chunk has
    /// nothing left for the in-place pass — otherwise it would treat a line
    /// still being carried as the file's last.
    #[test]
    fn the_carry_pass_is_eof_only_when_the_chunk_is_exhausted() {
        let mut carry = carrying(b"abc");

        let split = b"def\nghi";
        carry.absorb(split);
        assert!(!carry.span(ChunkPass::Carry, split, true).1);
        carry.consumed(ChunkPass::Carry, split, carry.len());
        assert!(carry.span(ChunkPass::InPlace, split, true).1);

        let whole = b"defghi";
        let mut carry = carrying(b"abc");
        carry.absorb(whole);
        assert!(carry.span(ChunkPass::Carry, whole, true).1);
    }

    /// An unterminated `COPY` block is still an error when the file ends
    /// inside it, wherever the last chunk boundary fell — the carry pass takes
    /// `eof` for exactly the case where nothing follows it.
    #[test]
    fn a_file_ending_inside_a_copy_block_is_still_rejected() {
        let file = b"COPY public.t (a) FROM stdin;\n1\n2\n3".to_vec();
        for chunk_size in [1usize, 2, 3, 7, 4096] {
            let mut scanner = CopyScanner::new();
            let mut carry = ChunkCarry::new();
            let mut read_pos = 0usize;
            let mut err = None;
            'file: loop {
                let want = chunk_size.min(file.len() - read_pos);
                let chunk = &file[read_pos..read_pos + want];
                read_pos += want;
                let eof = read_pos >= file.len();
                carry.absorb(chunk);
                for pass in ChunkCarry::PASSES {
                    let (span, span_eof) = carry.span(pass, chunk, eof);
                    loop {
                        match scanner.next_event(span, span_eof) {
                            Ok(Some(_)) => {}
                            Ok(None) => break,
                            Err(e) => {
                                err = Some(e);
                                break 'file;
                            }
                        }
                    }
                    carry.consumed(pass, chunk, scanner.take_consumed());
                }
                if eof {
                    break;
                }
            }
            assert!(
                matches!(err, Some(Error::UnterminatedCopyBlock { header_offset: 0 })),
                "chunk_size {chunk_size} gave {err:?}"
            );
        }
    }
}
