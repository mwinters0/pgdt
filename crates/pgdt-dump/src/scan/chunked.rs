//! The chunked read loop: a [`CopyScanner`] driven over a source a chunk at a
//! time, the one line straddling each chunk boundary carried across it under
//! the line limit ([`ReadOptions`]). Every serial pass reads through
//! [`ChunkedScanner`] — the preamble scan here ([`scan`]), `pgdt_mapping`'s
//! forward pass and statistics back-fill, and `pgdt_query`'s replay.

use std::ops::ControlFlow;

use bytes::Bytes;

use super::{CopyScanner, Event};
use pgdt_core::{Error, Result};
use pgdt_io::{
    ByteRangeSource, Parallelism, SCAN_CHUNK_DEFAULT_SIZE_BYTES, WaitPolicy,
    internal::memory_budget_display,
};

/// Which of a chunk's two spans is being scanned. The carry pass comes first:
/// the straddling line, then the rest of the chunk in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkPass {
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
/// `CopyScanner::base` is an absolute file offset, and each pass is
/// bracketed by [`take_consumed`](CopyScanner::take_consumed) exactly as a
/// single buffer's refill would be.
///
/// **The carry pass's line and what the passes leave over are two buffers**,
/// so a chunk's events may borrow the line while the in-place pass writes the
/// next chunk's carry; [`absorb`](Self::absorb) swaps them rather than
/// copying one into the other.
///
/// The degenerate case is a chunk containing no newline at all: the whole of
/// it joins the carry, and from the second such chunk on the in-place pass is
/// empty — the first is handed over whole, finds no newline and consumes
/// nothing, there being no carry yet to base it against. That is the growth
/// [`ReadOptions::max_line_bytes`] bounds. **Its carry pass is empty too**
/// short of the end of the file, since a span holding no newline is one the
/// scanner can neither consume nor emit anything from; handing it over anyway
/// would search the carried line again from its first byte at every chunk it
/// spans, quadratic in the line.
#[derive(Debug, Default)]
struct ChunkCarry {
    /// The current chunk's carry pass: the line carried in, extended by
    /// [`absorb`](Self::absorb) with the chunk bytes that complete it. Never
    /// written while the chunk is scanned.
    line: Vec<u8>,
    /// How much of `line` the carry pass consumed.
    line_used: usize,
    /// What the in-place pass left unconsumed: the start of the next chunk's
    /// line.
    tail: Vec<u8>,
    /// Where in the current chunk [`ChunkPass::InPlace`] begins.
    split: usize,
    /// Whether `line` still holds no newline after the current chunk was
    /// absorbed — the chunk held none — so its carry pass has nothing to scan
    /// unless the file ends here.
    unfinished: bool,
}

impl ChunkCarry {
    /// [`Passes::carried`], between two chunks.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.line.len() - self.line_used + self.tail.len()
    }

    /// Take onto the carry whatever prefix of `chunk` completes the line it
    /// holds, and hand out the chunk's two passes.
    ///
    /// With nothing carried the in-place pass begins at `0` — the chunk is
    /// scanned whole, where it lies. With a carry and no newline anywhere in
    /// `chunk`, it begins at `chunk.len()`, and the carry is left unfinished.
    fn absorb<'a>(&'a mut self, chunk: &'a [u8], eof: bool) -> Passes<'a> {
        // The line carried in is whichever pass left one, never both
        // (`Passes::span`'s assertion): an unfinished line stays where it
        // lies, and a tail the in-place pass left becomes the line.
        if self.line_used < self.line.len() {
            debug_assert!(self.tail.is_empty(), "both passes left a line over");
            self.line.drain(..self.line_used);
        } else {
            self.line.clear();
            std::mem::swap(&mut self.line, &mut self.tail);
        }
        self.line_used = 0;
        if self.line.is_empty() {
            self.split = 0;
            self.unfinished = false;
        } else {
            let newline = memchr::memchr(b'\n', chunk);
            self.split = newline.map_or(chunk.len(), |nl| nl + 1);
            self.unfinished = newline.is_none();
            self.line.extend_from_slice(&chunk[..self.split]);
        }
        let Self { line, line_used, tail, split, unfinished } = self;
        Passes {
            line,
            rest: &chunk[*split..],
            // Nothing in an unfinished line is consumable before the end of
            // the file, so it is not searched again (the type's docs).
            line_waits: *unfinished && !eof,
            // The carry pass runs to the true end of the file only when
            // nothing of the chunk is left over for the in-place pass.
            line_eof: eof && *split == chunk.len(),
            eof,
            line_used,
            tail,
        }
    }
}

/// One absorbed chunk's two spans, and where each pass's unconsumed bytes go.
#[derive(Debug)]
struct Passes<'a> {
    line: &'a [u8],
    rest: &'a [u8],
    line_waits: bool,
    line_eof: bool,
    eof: bool,
    line_used: &'a mut usize,
    tail: &'a mut Vec<u8>,
}

impl<'a> Passes<'a> {
    /// The bytes `pass` scans, and whether they run to the true end of the
    /// file.
    fn span(&self, pass: ChunkPass) -> (&'a [u8], bool) {
        match pass {
            ChunkPass::Carry if self.line_waits => (&[], false),
            ChunkPass::Carry => (self.line, self.line_eof),
            ChunkPass::InPlace => {
                // **The carry is spent here whenever this span has anything in
                // it**, and that is what keeps `CopyScanner::base` correct
                // across the switch of buffers: a line the scanner did not
                // finish would leave `base` short of the span's first byte, and
                // the in-place span would be mis-based by exactly the residue.
                // It holds because a line that gained a line-terminated prefix
                // is entirely consumable, and the only one that is not — a
                // chunk with no newline in it — took the whole chunk, leaving
                // this span empty.
                debug_assert!(
                    *self.line_used == self.line.len() || self.rest.is_empty(),
                    "an unfinished carry beside a non-empty in-place span mis-bases the scanner"
                );
                (self.rest, self.eof)
            }
        }
    }

    /// Record that the scanner consumed `used` bytes of `pass`'s span. After
    /// the in-place pass this is what makes the next chunk's carry: the tail
    /// the scanner could not finish, which is at most one line.
    fn consumed(&mut self, pass: ChunkPass, used: usize) {
        match pass {
            ChunkPass::Carry => *self.line_used = used,
            ChunkPass::InPlace => self.tail.extend_from_slice(&self.rest[used..]),
        }
    }

    /// Bytes carried over into the next chunk, once both passes have recorded
    /// what they used. This is what [`ReadOptions::max_line_bytes`] bounds: it
    /// is a single line's prefix, so a file whose lines fit stays flat here
    /// however large it is.
    fn carried(&self) -> usize {
        self.line.len() - *self.line_used + self.tail.len()
    }
}

/// A [`CopyScanner`] read a chunk at a time from `[position, end)` of a
/// source: the protocol every serial read loop follows, in one place.
///
/// A loop asks for a chunk ([`next_chunk`](Self::next_chunk)) and drains its
/// events ([`ChunkEvents::next_event`]); what it does between two chunks —
/// polling a cancellation, consulting a dynamic filter — is its own. Once a
/// chunk's events run out it asks [`at_end`](Self::at_end) whether that chunk
/// was the last. A chunk left before its events run out leaves this scanner
/// usable only through [`skip_to`](Self::skip_to).
#[derive(Debug)]
pub struct ChunkedScanner {
    scanner: CopyScanner,
    carry: ChunkCarry,
    /// The chunk last read, which its events borrow.
    chunk: Bytes,
    /// Where the next chunk is read from.
    read_pos: u64,
    end: u64,
    chunk_size: usize,
    max_line_bytes: usize,
    /// Whether the chunk last handed out still has events to give.
    open: bool,
}

impl ChunkedScanner {
    /// Read `scanner` on from its [`position`](CopyScanner::position) to
    /// `end`, which is the true end of what it scans — the file's, or a replay
    /// segment's block's — in `read`'s chunks under its line limit.
    pub fn new(scanner: CopyScanner, end: u64, read: ReadOptions) -> Self {
        Self {
            read_pos: scanner.position(),
            scanner,
            carry: ChunkCarry::default(),
            chunk: Bytes::new(),
            end,
            chunk_size: read.chunk_size_bytes,
            max_line_bytes: read.max_line_bytes,
            open: false,
        }
    }

    /// Read the next chunk from `source` — empty where nothing is left to
    /// read, so the last line is still scanned to the end — and hand out its
    /// events. A read's error is returned as the source gave it.
    pub async fn next_chunk(&mut self, source: &dyn ByteRangeSource) -> Result<ChunkEvents<'_>> {
        let want = self.chunk_size.min((self.end - self.read_pos) as usize);
        let bytes =
            if want > 0 { source.read_range(self.read_pos, want).await? } else { Bytes::new() };
        Ok(self.feed(bytes))
    }

    /// Hand out the events of `bytes`, the source's bytes from where the last
    /// chunk ended.
    pub(super) fn feed(&mut self, bytes: Bytes) -> ChunkEvents<'_> {
        debug_assert!(!self.open, "a chunk was left before its events ran out");
        let offset = self.read_pos;
        self.read_pos += bytes.len() as u64;
        let eof = self.read_pos >= self.end;
        self.chunk = bytes;
        self.open = true;
        let passes = self.carry.absorb(&self.chunk, eof);
        let (span, span_eof) = passes.span(ChunkPass::Carry);
        ChunkEvents {
            scanner: &mut self.scanner,
            passes,
            pass: Some(ChunkPass::Carry),
            span,
            span_eof,
            chunk: &self.chunk,
            offset,
            max_line_bytes: self.max_line_bytes,
            open: &mut self.open,
        }
    }

    /// The scanner, standing where the events handed out so far leave it.
    pub fn scanner(&self) -> &CopyScanner {
        &self.scanner
    }

    /// Whether the chunk last handed out ran to the end.
    pub fn at_end(&self) -> bool {
        self.read_pos >= self.end
    }

    /// Put the scanner back down at `at`, a line start outside any block past
    /// bytes read elsewhere — where a leader closed a block its workers read.
    /// The carry is dropped rather than fixed up, whatever it held being inside
    /// those bytes; the `standard_conforming_strings` reached is kept.
    pub fn skip_to(&mut self, at: u64) {
        self.scanner =
            CopyScanner::resume(at, None).with_standard_strings(self.scanner.standard_strings());
        self.carry = ChunkCarry::default();
        self.read_pos = at;
        self.open = false;
    }

    /// Bytes carried into the next chunk.
    #[cfg(test)]
    pub(super) fn carried(&self) -> usize {
        self.carry.len()
    }
}

/// One chunk's events, in file order across its two passes.
///
/// An event borrows the chunk, not this, so a loop may hold one while it asks
/// where the scanner stands ([`scanner`](Self::scanner)).
#[derive(Debug)]
pub struct ChunkEvents<'a> {
    scanner: &'a mut CopyScanner,
    passes: Passes<'a>,
    /// The pass being scanned, `None` once both have run out.
    pass: Option<ChunkPass>,
    span: &'a [u8],
    span_eof: bool,
    chunk: &'a Bytes,
    offset: u64,
    max_line_bytes: usize,
    open: &'a mut bool,
}

impl<'a> ChunkEvents<'a> {
    /// The chunk's next event, or `None` once it has none left. Where the
    /// chunk was not the last, the line it leaves for the next is held to
    /// [`ReadOptions::max_line_bytes`] there: one longer is
    /// [`Error::LineTooLong`].
    #[inline]
    pub fn next_event(&mut self) -> Result<Option<Event<'a>>> {
        loop {
            // Handed back as the scanner gave it, not unwrapped and rebuilt:
            // this is every row's path.
            let next = self.scanner.next_event(self.span, self.span_eof);
            if !matches!(next, Ok(None)) {
                return next;
            }
            if !self.end_pass()? {
                return Ok(None);
            }
        }
    }

    /// Record what the pass just run out consumed, and move to the next:
    /// `true` where one follows, `false` once both have run out.
    ///
    /// Fallible, a long line refused where the chunk leaves it: checked by
    /// `next_chunk` instead, it would move for a loop leaving between chunks
    /// (the replay's dynamic filter), a semantic change bought for codegen;
    /// nor does a further attribute or cold continuation split the switch,
    /// its saving resting on inlining (`docs/design/decisions.md`, "D108").
    #[cold]
    fn end_pass(&mut self) -> Result<bool> {
        let used = self.scanner.take_consumed();
        match self.pass {
            Some(ChunkPass::Carry) => {
                self.passes.consumed(ChunkPass::Carry, used);
                (self.span, self.span_eof) = self.passes.span(ChunkPass::InPlace);
                self.pass = Some(ChunkPass::InPlace);
                Ok(true)
            }
            Some(ChunkPass::InPlace) => {
                self.passes.consumed(ChunkPass::InPlace, used);
                (self.span, self.span_eof) = (&[], false);
                self.pass = None;
                *self.open = false;
                if !self.passes.eof && self.passes.carried() > self.max_line_bytes {
                    return Err(Error::LineTooLong {
                        offset: self.scanner.position(),
                        limit: self.max_line_bytes,
                    });
                }
                Ok(false)
            }
            None => Ok(false),
        }
    }

    /// The scanner, standing past the last event handed out.
    pub fn scanner(&self) -> &CopyScanner {
        self.scanner
    }

    /// The bytes this chunk read, and the file offset of the first — what a
    /// loop keeping zero-copy views into the chunk retains.
    pub fn read(&self) -> (u64, &'a Bytes) {
        (self.offset, self.chunk)
    }

    /// The span the last event was scanned out of, and the file offset of its
    /// first byte — so a row's offset less it indexes the row in the span.
    /// Two spans with an event in each start apart, each event consuming its
    /// line.
    pub fn span(&self) -> (u64, &'a [u8]) {
        (self.scanner.base, self.span)
    }

    /// Bytes handed to the scanner over both passes.
    #[cfg(test)]
    pub(super) fn scanned(&self) -> usize {
        let line = if self.passes.line_waits { 0 } else { self.passes.line.len() };
        line + self.passes.rest.len()
    }
}

/// The longest line a scan carries across a chunk boundary unless a caller
/// says otherwise ([`ReadOptions::max_line_bytes`], which says where a line
/// is measured). A row is held whole before it is emitted, so this and the
/// chunk size bound what one row may cost in memory; a dump holding larger
/// values states a larger limit.
pub const SCAN_LINE_DEFAULT_MAX_BYTES: usize = 64 << 20;

/// **How a chunked read loop reads**: the chunk it asks of the source and the
/// longest line it carries across a chunk boundary — all [`ChunkedScanner`]
/// reads, and with a [`Parallelism`] a [`ScanOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadOptions {
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
    // deficiency: KD93 — a line PostgreSQL writes and reads, up to I71's
    // bound, is refused past this limit; and a line is measured only where a
    // read ends inside it, so one up to a chunk past the limit is read or
    // refused by where the reads fall — `chunk_size_bytes`, the leader's
    // split, a resume point — not by its length.
    pub max_line_bytes: usize,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            chunk_size_bytes: SCAN_CHUNK_DEFAULT_SIZE_BYTES,
            max_line_bytes: SCAN_LINE_DEFAULT_MAX_BYTES,
        }
    }
}

/// What a pass that gathers nothing reads with: its [`ReadOptions`] and the
/// concurrency it may use. Configuration only, reused across runs: the
/// caller's ask that a pass stop is `pgdt_mapping::map_file`'s own argument,
/// never a field here (`docs/design/decisions.md`, "D26"), and a gathering
/// pass takes an arrangement in its place, whose parallelism and statistics
/// allowance are one carve (`docs/design/decisions.md`, "D83").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanOptions {
    /// The chunk and the line limit every read loop of the pass reads with.
    pub read: ReadOptions,
    /// How much concurrency this scan may use, and what it may hold while it
    /// does — [`Parallelism::Serial`] by default, which is the serial code
    /// path this build has rather than a pool of one
    /// (`docs/design/decisions.md`, "D1").
    ///
    /// Every read loop announces it to the source
    /// ([`pgdt_io::ByteRangeSource::hint_parallelism`]), which sizes its pools
    /// from the byte half and — for a compressed source — decides from it
    /// whether a whole block can be decoded at all. The `jobs` half is that
    /// source's retention depth, one decoded block per concurrent reader, and
    /// the ceiling on the workers `pgdt_mapping`'s `scan_region` runs over an
    /// open `COPY` block's interior, which is what a `parse` splits by.
    pub parallelism: Parallelism,
}

/// **The hints every top-level read loop announces before its first read**:
/// the chunk length it repeats until it stops, so a buffer-recycling source
/// can keep one of that size whatever it is
/// ([`ByteRangeSource::hint_read_size`]); `parallelism`, the budget the
/// caller allows it to keep them inside
/// ([`ByteRangeSource::hint_parallelism`]); and that the loop grants no wait
/// ([`ByteRangeSource::hint_wait_policy`]): the leader's fused worker is the
/// holder that needs the bound and is where one is granted (`pgdt_mapping`'s
/// `scan_region`, and `docs/design/decisions.md`, "D5"). `parallelism` is the
/// pass's own, or one a query states for its replay apart from its mapping
/// pass.
pub fn announce_read_loop(
    source: &dyn ByteRangeSource,
    read: ReadOptions,
    parallelism: Parallelism,
) {
    source.hint_read_size(read.chunk_size_bytes);
    source.hint_parallelism(parallelism);
    source.hint_wait_policy(WaitPolicy::NeverWait);
}

/// Scan `source` from the beginning, invoking `on_event` for every event, in
/// `options`' chunks, announcing their parallelism to the source.
///
/// The callback may return [`ControlFlow::Break`] to stop early. Nothing
/// stops it from outside: it polls no cancellation
/// (`docs/design/decisions.md`, "D26").
pub async fn scan<F>(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    mut on_event: F,
) -> Result<()>
where
    F: FnMut(Event<'_>) -> ControlFlow<()>,
{
    let ScanOptions { read, parallelism } = *options;
    let size = source.size().await?;
    // Named "preamble scan", not "scan": the caller a `parse` runs is
    // `preamble::scan_preamble`, and `pgdt_mapping`'s `map_forward` announces
    // itself as "scan". Its `build_index` and `build_map` share this label
    // while scanning to EOF; neither is reached from the CLI. A single `parse` runs both in sequence, so two passes sharing
    // one name would read as an interrupted-and-resumed run.
    tracing::info!(
        bytes = size,
        chunk_size = read.chunk_size_bytes,
        jobs = parallelism.jobs(),
        memory_bytes = %memory_budget_display(parallelism),
        "preamble scan started",
    );
    announce_read_loop(source, read, parallelism);
    let mut chunked = ChunkedScanner::new(CopyScanner::new(), size, read);

    loop {
        let mut chunk = chunked.next_chunk(source).await?;
        while let Some(event) = chunk.next_event()? {
            if on_event(event).is_break() {
                // The ordinary outcome for `scan_preamble`, which breaks
                // here at the first `COPY` header — still this call's
                // completion, just not at EOF, which `reached_eof` says.
                let (offset, chunk_bytes) = chunk.read();
                let bytes = offset + chunk_bytes.len() as u64;
                tracing::info!(bytes, reached_eof = false, "preamble scan complete");
                return Ok(());
            }
        }
        if chunked.at_end() {
            tracing::info!(bytes = size, reached_eof = true, "preamble scan complete");
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests;
