//! Line-anchored structural scanner for `pg_dump` plain-format files.
//!
//! [`CopyScanner`] is a pure synchronous state machine. It never owns or
//! copies the bytes it scans: the caller owns a buffer, hands it out as a
//! slice, and the scanner reports how much of it was consumed. That keeps the
//! events zero-copy and lets the same state machine serve both the async
//! driver here ([`scan`]) and the pull-mode stream (`crate::stream`).
//!
//! Robustness rules this implements (see `docs/design/decisions.md`,
//! "D23"):
//!
//! * `COPY` detection is line-anchored. Row data may contain a literal
//!   `COPY ... TO stdout;` mid-line, and a non-anchored search would misfire.
//! * While inside a COPY block the scanner looks for nothing but the `\.`
//!   terminator, so data can never be mistaken for structure. COPY TEXT
//!   escapes a literal backslash as `\\`, so a line that *is* exactly `\.` is
//!   unambiguously the terminator.
//! * psql meta-command lines (`\restrict`, `\unrestrict`, `\connect`, and any
//!   other leading-backslash line) are ordinary [`Event::Line`]s: outside a
//!   block only a line matching the full `COPY ... FROM stdin;` grammar, or a
//!   bare `BEGIN;` opening the large-object region, is structural.
//! * Outside a block every line is lexed ([`crate::lex`]), and only a line
//!   that begins between tokens and touches no dollar-quoted body can be
//!   structural: one that begins inside a string, a quoted identifier or a
//!   comment is that region's content.

use std::future::Future;
use std::ops::ControlFlow;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use futures::future::{Either, select};
use tokio::sync::Notify;

use crate::copy::{CopyHeader, is_terminator, parse_copy_header};
use crate::io::{ByteRangeSource, Parallelism, WaitPolicy, memory_budget_display};
use crate::lex::{Lexer, Region, standard_conforming_strings};
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
/// `crate::map::Builder::feed_line` classifies into spans, using
/// `crate::preamble`'s grammar. No line reaches a `DumpMetadata` directly:
/// [`crate::index::DumpMetadata`] is derived from the finished spans
/// (`docs/design/decisions.md`, "D34"), never from a second pass. A COPY block's data
/// rows never reach this arm.
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
/// file absorbs every statement after it into one span. Surfacing the
/// *lines* stays rejected: it would leak a mapping concern into L1's event
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
/// [`ChunkCarry`] is that protocol done without copying the buffer, and is
/// what the serial read loops drive the scanner through; a leader piece scans
/// each of its reads as a slice of its own.
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
}

impl Default for CopyScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyScanner {
    pub fn new() -> Self {
        Self { base: 0, pos: 0, state: State::Outside, lexer: Lexer::new() }
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
    pub fn resume(offset: u64, in_copy: Option<(u64, u64)>) -> Self {
        let state = match in_copy {
            Some((header_offset, rows)) => State::InCopy { rows, header_offset },
            None => State::Outside,
        };
        Self { base: offset, pos: 0, state, lexer: Lexer::new() }
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
                    let between_tokens = *self.lexer.region() == Region::Code;
                    let lexed = self.lexer.line(line, |_| {});
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
                        return Ok(Some(Event::Line(Line { offset: line_offset, raw: line })));
                    }

                    // Line-anchored: only a line that both starts with `COPY`,
                    // past spaces and tabs, and matches the full header
                    // grammar is structural.
                    // Everything else — SQL, comments, psql meta-commands —
                    // is surfaced as a line and never as structure, a bare
                    // `BEGIN;` aside (below).
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
                    // I50: the literal syntax every line after this one is
                    // written in.
                    if let Some(on) = standard_conforming_strings(line) {
                        self.lexer.set_standard_strings(on);
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
/// lies** (`docs/design/decisions.md`, "D23"). Per chunk that is one row's
/// worth of copying instead of the chunk's whole length.
///
/// **Handing the scanner two buffers within one chunk costs it nothing**:
/// [`CopyScanner::base`] is an absolute file offset, and each pass is
/// bracketed by [`take_consumed`](CopyScanner::take_consumed) exactly as a
/// single buffer's refill would be.
///
/// The degenerate case is a chunk containing no newline at all: the whole of
/// it joins the carry, and from the second such chunk on the in-place pass is
/// empty — the first is handed over whole, finds no newline and consumes
/// nothing, there being no carry yet to base it against. That is the growth
/// [`ScanOptions::max_line_bytes`] bounds. **Its carry pass is empty too**
/// short of the end of the file, since a span holding no newline is one the
/// scanner can neither consume nor emit anything from; handing it over anyway
/// would search the carried line again from its first byte at every chunk it
/// spans, quadratic in the line.
#[derive(Debug, Default)]
pub struct ChunkCarry {
    /// The unterminated line carried over, extended by [`absorb`](Self::absorb)
    /// with the chunk bytes that complete it.
    buf: Vec<u8>,
    /// Where in the current chunk [`ChunkPass::InPlace`] begins — set by
    /// [`absorb`](Self::absorb), meaningless before the first call.
    split: usize,
    /// Whether the carry still holds no newline after the current chunk was
    /// absorbed — the chunk held none — so its carry pass has nothing to scan
    /// unless the file ends here.
    unfinished: bool,
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
    /// `chunk.len()`, and the carry is left unfinished.
    pub fn absorb(&mut self, chunk: &[u8]) {
        if self.buf.is_empty() {
            self.split = 0;
            self.unfinished = false;
            return;
        }
        let newline = memchr::memchr(b'\n', chunk);
        self.split = newline.map_or(chunk.len(), |nl| nl + 1);
        self.unfinished = newline.is_none();
        self.buf.extend_from_slice(&chunk[..self.split]);
    }

    /// The bytes `pass` scans, and whether they run to the true end of the
    /// file — which the carry pass does only when nothing of the chunk is
    /// left over for the in-place pass to see.
    pub fn span<'a>(&'a self, pass: ChunkPass, chunk: &'a [u8], eof: bool) -> (&'a [u8], bool) {
        match pass {
            // Nothing in an unfinished carry is consumable before the end of
            // the file, so it is not searched again (the type's docs).
            ChunkPass::Carry if self.unfinished && !eof => (&[], false),
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
/// Shipped, not probed (`docs/design/decisions.md`, "D10"): the default that
/// is worst-case-best across the device classes measured
/// (`docs/design/measurements.md`, "What the read chunk size is worth").
/// Public and named because it is what [`ScanOptions::chunk_size_bytes`] is
/// compared against and what callers scale from.
///
/// **Raising it costs memory, not pooling.** Every read loop announces the
/// size it is about to repeat ([`crate::ByteRangeSource::hint_read_size`]), so
/// a chunk of any size is kept and reused by the local source's buffer pool.
/// What a larger chunk costs is the pool holding four buffers of it, or the
/// caller's stated memory budget's worth ([`crate::Parallelism`], defaulting
/// to [`crate::DEFAULT_MEMORY_BUDGET`]), whichever is fewer — the slot count
/// falls out of that budget, so the cost levels off
/// (`docs/design/decisions.md`, "D9").
pub const SCAN_CHUNK_DEFAULT_SIZE_BYTES: usize = 1 << 20;

/// The longest line a scan accepts unless a caller says otherwise. A row is
/// held whole before it is emitted, so this is what one row may cost in
/// memory; a dump holding larger values states a larger limit
/// ([`ScanOptions::max_line_bytes`]).
pub const SCAN_LINE_DEFAULT_MAX_BYTES: usize = 64 << 20;

/// A caller's ask that a scan stop, in both the forms a reader consumes it
/// in: a bit to poll and a signal to await.
///
/// The polled bit is what the mapping loops read once per chunk or leader
/// window, and it is the whole mechanism for a reader whose wait is a
/// `pread` — tens of milliseconds, bounded by the read already in flight. A
/// reader whose wait is a network request has no such bound: its in-flight
/// call can sit inside a retry schedule for minutes, and polling at the far
/// side of it is a program that does not answer. So the same object carries a
/// signal such a reader can select on and **drop** the request rather than
/// wait it out (`docs/design/decisions.md`, "D26").
///
/// Both halves report the same one-way transition — once cancelled, always
/// cancelled — so a reader may use either or both, and which it uses is a
/// property of what it is waiting on rather than of what the caller asked for.
#[derive(Debug, Default)]
pub struct Cancellation {
    flag: AtomicBool,
    signal: Notify,
}

impl Cancellation {
    /// A cancellation nobody has asked for yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask every reader holding this to stop. Idempotent, and never blocks:
    /// it is called from a signal-handling task, and from a source's own read
    /// path in the tests.
    ///
    /// The `SeqCst` store is what [`Cancellation::cancelled`]'s registration
    /// is ordered against; the polled side needs nothing so strong.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.signal.notify_waiters();
    }

    /// Whether cancellation has been asked for. `Relaxed` is the right
    /// ordering: the bit guards nothing but itself — the reader's response is
    /// to finish the chunk it already holds and save the index it already
    /// owns — so nothing is published through it.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Resolves once [`Cancellation::cancel`] has been called, at once if it
    /// already has.
    ///
    /// The waiter is registered *before* the bit is read, which is what makes
    /// this race-free against a `cancel` landing between the two: a cancel
    /// wholly before the registration is seen by the load, and one after it
    /// wakes a waiter that is already there.
    pub async fn cancelled(&self) {
        let mut signalled = pin!(self.signal.notified());
        signalled.as_mut().enable();
        if self.flag.load(Ordering::SeqCst) {
            return;
        }
        signalled.await;
    }

    /// Run `work` until it finishes or this is cancelled, whichever happens
    /// first. `None` is the cancellation, and `work` is **dropped where it
    /// stood** — which is how a reader gives up a request in flight instead
    /// of waiting for the answer it no longer wants.
    pub async fn until_cancelled<F: Future>(&self, work: F) -> Option<F::Output> {
        let work = pin!(work);
        let cancelled = pin!(self.cancelled());
        match select(work, cancelled).await {
            Either::Left((done, _)) => Some(done),
            Either::Right(((), _)) => None,
        }
    }
}

/// What a read does with a field its type's `*_in` refuses
/// ([`crate::decode::Unread::Refused`]), stated for a parse on [`ScanOptions`]
/// and for a query on [`crate::QueryOptions`] (`docs/design/decisions.md`,
/// "D103").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PostgresInvalidValues {
    /// Refused as a restore refuses it: a parse fails at the first such field
    /// it decodes, or that its cache records an ignoring parse went past
    /// ([`crate::Error::FieldRefusedRecorded`]), and a query decoding one fails
    /// there.
    #[default]
    Default,
    /// Read as the decoders read it, with no contract: a parse goes on past
    /// one, its row group then keeping no statistic of its column a read could
    /// contradict and its block recording it
    /// ([`crate::index::CopyBlock::ignored_refusals`]), and a query reads a float past its type's range as
    /// `decode::float_field` reads it and fails on every other such field. A
    /// filter literal is never opted out.
    Ignore,
    /// **A parse checks every field**, failing at the first its type's `*_in`
    /// refuses as [`Self::Default`] does, but over the fields `Default` leaves
    /// to a query: a column the request leaves at the metadata level, a nested
    /// value's elements, a value too long to key, and the rows of a block whose
    /// statistics declined. A block it checks whole records it
    /// ([`crate::index::CopyBlock::checked_in_full`]), and one the cache holds
    /// unchecked is re-read and checked, so a clean run has checked every field
    /// whichever runs built the cache. A query reads under it as under
    /// `Default`.
    Strict,
}

/// Tuning knobs for a full-file scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Bytes requested per read from the source. Defaults to
    /// [`SCAN_CHUNK_DEFAULT_SIZE_BYTES`].
    pub chunk_size_bytes: usize,
    /// Hard cap on a line carried across a chunk boundary. One still
    /// incomplete at a chunk's end and longer than this is rejected rather
    /// than buffered without bound; a line whole inside one chunk, or ended
    /// by the last, is not measured. The scanner cannot emit a row until it
    /// has the whole line, so this is the only thing standing between a
    /// malformed input and unbounded memory growth. Defaults to
    /// [`SCAN_LINE_DEFAULT_MAX_BYTES`].
    pub max_line_bytes: usize,
    /// Cooperative cancellation: call [`Cancellation::cancel`] from another
    /// task and the mapping loop stops at the next chunk boundary, persists
    /// what it holds and reports that it was interrupted
    /// (`docs/design/decisions.md`, "D63"). `None` — the default — is a scan nobody can stop.
    ///
    /// Chunk granularity — a window of pieces while the leader holds a region
    /// — not `CopyEnd` granularity, and read by
    /// [`crate::stream::map_file`]'s two passes alone — the mapping loop and
    /// the statistics back-fill, the drivers with somewhere to put a partial
    /// result and a way to report the stop. [`scan`] and the eager producers
    /// built on it never read the polled bit
    /// (`docs/design/decisions.md`, "D26") — but [`scan`] does announce this
    /// to the source, so a read already in flight can still be dropped and
    /// arrive as `ScanCancelled`.
    ///
    /// A source may also hold a clone of this and await
    /// [`Cancellation::cancelled`], which is what bounds a stop by the request
    /// in flight rather than by the loop's next poll.
    pub cancel: Option<Arc<Cancellation>>,
    /// How much concurrency this scan may use, and what it may hold while it
    /// does — [`Parallelism::Serial`] by default, which is the serial code
    /// path this build has rather than a pool of one
    /// (`docs/design/decisions.md`, "D1").
    ///
    /// Every read loop announces it to the source
    /// ([`crate::ByteRangeSource::hint_parallelism`]), which sizes its pools
    /// from the byte half and — for a compressed source — decides from it
    /// whether a whole block can be decoded at all. The `jobs` half is that
    /// source's retention depth, one decoded block per concurrent reader, and
    /// the ceiling on the workers [`crate::leader::scan_region`] runs over an
    /// open `COPY` block's interior, which is what a `parse` splits by.
    pub parallelism: Parallelism,
    /// **The bytes of heap this scan's statistics may hold alive**, and
    /// `None` — the default — declines nothing, an embedder that stated no
    /// allowance having chosen no bound
    /// (`docs/design/decisions.md`, "D1", "D85").
    ///
    /// It is what [`crate::statistics_allowance`] leaves under the margin once
    /// `parallelism`'s budget is spent, and it bounds the whole account
    /// (`crate::statistics::StatisticsAccount`): a `COPY` block whose
    /// gathering would pass it declines, drops what it gathered, and is
    /// recorded in the map as having declined under this number
    /// ([`crate::index::CopyBlock::statistics_declined`]).
    ///
    /// **Read by [`crate::stream::map_file`] alone**: a query gathers nothing,
    /// so on every other entry point it bounds nothing.
    pub statistics_allowance_bytes: Option<u64>,
    /// What gathering does with a field its type's `*_in` refuses: fails the
    /// pass at the first, by default, or goes on past it, or checks every
    /// field ([`PostgresInvalidValues`]). Read by [`crate::stream::map_file`] and
    /// [`crate::stream::gather_block_statistics`] alone, as gathering is; a
    /// query's reads are [`crate::QueryOptions::postgres_invalid_values`]'.
    pub postgres_invalid_values: PostgresInvalidValues,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            chunk_size_bytes: SCAN_CHUNK_DEFAULT_SIZE_BYTES,
            max_line_bytes: SCAN_LINE_DEFAULT_MAX_BYTES,
            cancel: None,
            parallelism: Parallelism::default(),
            statistics_allowance_bytes: None,
            postgres_invalid_values: PostgresInvalidValues::default(),
        }
    }
}

impl ScanOptions {
    /// Whether a caller has asked this scan to stop — the polled half of
    /// [`ScanOptions::cancel`], read once per chunk or leader window.
    pub fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|cancel| cancel.is_cancelled())
    }
}

/// Hand a source the caller's cancellation where there is one, so a source
/// whose wait is not a `pread` can drop the read in flight rather than be
/// polled at the far side of it ([`ByteRangeSource::hint_cancellation`]).
pub(crate) fn announce_cancellation(source: &dyn ByteRangeSource, options: &ScanOptions) {
    if let Some(cancel) = &options.cancel {
        source.hint_cancellation(Arc::clone(cancel));
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
    // Named "preamble scan", not "scan": the caller a `parse` runs is
    // `index::scan_preamble`, and `stream::map_forward` announces itself as
    // "scan". `index::build_index` and `stream::build_map` share this label
    // while scanning to EOF; neither is reached from the CLI. A single `parse` runs both in sequence, so two passes sharing
    // one name would read as an interrupted-and-resumed run.
    tracing::info!(
        bytes = size,
        chunk_size = options.chunk_size_bytes,
        jobs = options.parallelism.jobs(),
        memory_bytes = %memory_budget_display(options.parallelism),
        "preamble scan started",
    );
    // The chunk length this loop will ask for until EOF, announced once so a
    // buffer-recycling source can keep one of that size whatever it is
    // (`ByteRangeSource::hint_read_size`), and the budget the caller allows it
    // to keep them inside (`ByteRangeSource::hint_parallelism`).
    source.hint_read_size(options.chunk_size_bytes);
    source.hint_parallelism(options.parallelism);
    announce_cancellation(source, options);
    // This loop grants no wait (`ByteRangeSource::hint_wait_policy`): the
    // leader's fused worker is the holder that needs the bound and is where
    // one is granted (`crate::leader::scan_region`, and
    // `docs/design/decisions.md`, "D5").
    source.hint_wait_policy(WaitPolicy::NeverWait);
    let mut scanner = CopyScanner::new();
    let mut carry = ChunkCarry::new();
    let mut read_pos = 0u64;

    loop {
        let want = options.chunk_size_bytes.min((size - read_pos) as usize);
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
                    // The ordinary outcome for `scan_preamble`, which breaks
                    // here at the first `COPY` header — still this call's
                    // completion, just not at EOF, which `reached_eof` says.
                    tracing::info!(bytes = read_pos, reached_eof = false, "preamble scan complete");
                    return Ok(());
                }
            }
            carry.consumed(pass, &chunk, scanner.take_consumed());
        }

        if eof {
            tracing::info!(bytes = read_pos, reached_eof = true, "preamble scan complete");
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

    /// What [`drive`] saw: the events rendered as text, the high-water mark of
    /// the carry, and the bytes handed to the scanner over every pass.
    struct Driven {
        events: Vec<String>,
        high_water: usize,
        scanned: usize,
    }

    /// Drive [`CopyScanner`] over `file` in `chunk_size` pieces exactly as
    /// the three read loops do.
    fn drive(file: &[u8], chunk_size: usize) -> Driven {
        let mut scanner = CopyScanner::new();
        let mut carry = ChunkCarry::new();
        let mut events = Vec::new();
        let mut read_pos = 0usize;
        let mut high_water = 0usize;
        let mut scanned = 0usize;
        loop {
            let want = chunk_size.min(file.len() - read_pos);
            let chunk = &file[read_pos..read_pos + want];
            read_pos += want;
            let eof = read_pos >= file.len();

            carry.absorb(chunk);
            for pass in ChunkCarry::PASSES {
                let (span, span_eof) = carry.span(pass, chunk, eof);
                scanned += span.len();
                while let Some(event) = scanner.next_event(span, span_eof).unwrap() {
                    events.push(format!("{event:?}"));
                }
                carry.consumed(pass, chunk, scanner.take_consumed());
            }
            high_water = high_water.max(carry.len());

            if eof {
                return Driven { events, high_water, scanned };
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
    /// by the longest line and not by the chunk size.
    #[test]
    fn the_carry_is_bounded_by_the_longest_line_not_by_the_chunk() {
        let file = control();
        let longest = file.split(|&b| b == b'\n').map(<[u8]>::len).max().unwrap();
        for chunk_size in [1usize, 2, 3, 7, 13, 64, 511, 4096] {
            let high_water = drive(&file, chunk_size).high_water;
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
        let reference = drive(&file, 1 << 20).events;
        for chunk_size in [1usize, 2, 3, 7, 13, 64, 511, 4096] {
            let got = drive(&file, chunk_size).events;
            assert_eq!(got, reference, "chunk_size {chunk_size}");
        }
    }

    /// A file whose lines are many chunks long: a row inside a `COPY` block,
    /// and an unterminated last line that only the end of the file finishes.
    fn long_lines(len: usize) -> Vec<u8> {
        let value = "x".repeat(len);
        let mut file = b"COPY public.t (a, b) FROM stdin;\n1\tshort\n2\t".to_vec();
        file.extend_from_slice(value.as_bytes());
        file.extend_from_slice(b"\n3\tshort\n\\.\n");
        file.extend_from_slice(value.as_bytes());
        file
    }

    /// A line many chunks long is handed to the scanner once, when the chunk
    /// holding its newline — or the end of the file — arrives, so the bytes
    /// scanned are linear in the file and not in the square of the line.
    /// Counted, not timed: re-searching the carry at every chunk costs about
    /// `len² / (2 × chunk)` bytes here, orders past the bound.
    #[test]
    fn a_line_many_chunks_long_is_scanned_once() {
        let file = long_lines(1 << 16);
        let reference = drive(&file, 1 << 20).events;
        for chunk_size in [1usize, 7, 64, 4096] {
            let driven = drive(&file, chunk_size);
            assert_eq!(driven.events, reference, "chunk_size {chunk_size}");
            assert!(
                driven.scanned <= 2 * file.len(),
                "chunk_size {chunk_size} scanned {} bytes of a {}-byte file",
                driven.scanned,
                file.len()
            );
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

    /// The degenerate case: a chunk with no newline in it joins the carry
    /// whole, which is the growth `ScanOptions::max_line_bytes` bounds, and
    /// neither pass has anything to scan until the file ends.
    #[test]
    fn a_chunk_with_no_newline_joins_the_carry_whole() {
        let mut carry = carrying(b"abc");

        let chunk = b"defghi";
        carry.absorb(chunk);
        assert_eq!(carry.len(), 9);
        assert_eq!(carry.span(ChunkPass::Carry, chunk, false).0, b"");
        assert_eq!(carry.span(ChunkPass::Carry, chunk, true).0, b"abcdefghi");
        assert_eq!(carry.span(ChunkPass::InPlace, chunk, false).0, b"");

        // The chunk that finishes the line hands the whole of it over once.
        carry.consumed(ChunkPass::InPlace, chunk, 0);
        let last = b"jk\nl";
        carry.absorb(last);
        assert_eq!(carry.span(ChunkPass::Carry, last, false).0, b"abcdefghijk\n");
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

    /// A guard that records its own drop, so a test can assert that a future
    /// given up on was released rather than merely left unpolled.
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// The signal reports a cancellation that landed **before** anyone waited
    /// on it: a reader that reaches its await after the ask must not wait for
    /// a second one that will never come.
    #[tokio::test]
    async fn the_signal_resolves_at_once_for_a_cancellation_already_asked_for() {
        let cancel = Cancellation::new();
        assert!(!cancel.is_cancelled());
        cancel.cancel();
        assert!(cancel.is_cancelled());
        cancel.cancelled().await;
    }

    /// And it reports one that lands **while** a waiter is registered, which
    /// is the ordinary case: the waiter is parked before `cancel` is called.
    #[tokio::test]
    async fn a_registered_waiter_is_woken_by_a_later_cancel() {
        let cancel = Cancellation::new();
        tokio::join!(cancel.cancelled(), async {
            assert!(!cancel.is_cancelled(), "the waiter parks before the ask");
            cancel.cancel();
        });
    }

    /// `cancel` is idempotent, and the polled bit is one-way.
    #[tokio::test]
    async fn cancelling_twice_says_the_same_thing() {
        let cancel = Cancellation::new();
        cancel.cancel();
        cancel.cancel();
        assert!(cancel.is_cancelled());
        cancel.cancelled().await;
    }

    /// Work that finishes before anyone cancels is handed back whole.
    #[tokio::test]
    async fn until_cancelled_yields_work_that_finished_first() {
        let cancel = Cancellation::new();
        assert_eq!(cancel.until_cancelled(async { 7u8 }).await, Some(7));
        assert!(!cancel.is_cancelled(), "asking nothing cancels nothing");
    }

    /// **The point of the awaitable form.** Work that never finishes is given
    /// up on at the cancellation *and dropped there* — which is what lets a
    /// reader release a request in flight instead of waiting out whatever
    /// deadline sits under it.
    #[tokio::test]
    async fn until_cancelled_drops_the_work_it_gave_up_on() {
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = DropFlag(Arc::clone(&dropped));
        let forever = async move {
            let _guard = guard;
            std::future::pending::<u8>().await
        };

        let cancel = Cancellation::new();
        let (given_up, ()) = tokio::join!(cancel.until_cancelled(forever), async {
            assert!(!dropped.load(Ordering::SeqCst), "still in flight before the ask");
            cancel.cancel();
        });

        assert_eq!(given_up, None, "cancellation, not an answer");
        assert!(dropped.load(Ordering::SeqCst), "the request in flight was released");
    }
}
