//! The chunked read loop: a [`CopyScanner`] driven over a source a chunk at a
//! time, the one line straddling each chunk boundary carried across it under
//! the line limit ([`ReadOptions`]). Every serial pass reads through
//! [`ChunkedScanner`] — the preamble scan here ([`scan`]), `pgdt_mapping`'s
//! forward pass and statistics back-fill, and `pgdt_query`'s replay.

use std::ops::ControlFlow;

use bytes::Bytes;

use super::{CopyScanner, EndingScan, Event, RowEnding};
use pgdt_core::{Error, Result};
use pgdt_io::{
    ByteRangeSource, Extent, Parallelism, SCAN_CHUNK_DEFAULT_SIZE_BYTES, WaitPolicy,
    internal::memory_budget_display,
};

/// Which of a chunk's two spans is being scanned. The carry pass comes first:
/// the straddling line, then the rest of the chunk in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPass {
    /// The line straddling the chunk's front edge — what was carried over
    /// from the previous chunk, completed by this one's first line-terminated
    /// prefix. The only bytes a read loop copies.
    Carry,
    /// The rest of the chunk, scanned where the reader left it.
    InPlace,
}

/// The one line a chunked read loop has to carry across a chunk boundary —
/// [`ChunkedScanner`]'s, and an interior worker's over its piece
/// (`pgdt_mapping`'s `scan_partition`), so each reads a line once, read by
/// read, and never again from its start.
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
/// **The line a carry pass scans is frozen and freed after its row**, never
/// kept at its high-water mark: [`absorb`](Self::absorb) hands the completed
/// line over as [`Bytes`] of the allocation it grew in, which a replay keeps
/// as one of its batch's blocks, so an unescaped value inside it is a view
/// rather than a copy ([`Passes::held`]), and the next chunk's absorb drops
/// this carry's reference. A pass whose lines run long holds one line's bytes,
/// not the longest line's twice for the rest of the pass
/// (`docs/design/decisions.md`, "D23").
///
/// The degenerate case is a chunk containing no newline at all: the whole of
/// it joins the carry, the in-place pass empty wherever there is a carry to
/// join — one arriving with none is handed over whole, finds no newline and
/// consumes nothing, there being no carry yet to base it against. That is the growth
/// [`ReadOptions::max_line_bytes`] bounds. **Its carry pass is empty too**
/// short of the end of the file, since a span holding no newline is one the
/// scanner can neither consume nor emit anything from; handing it over anyway
/// would search the carried line again from its first byte at every chunk it
/// spans, quadratic in the line.
///
/// **A row whose reader reads nothing past its count is streamed instead**
/// ([`streams`](Passes::streams)): once such a row outgrows the chunk it
/// began in, the carry keeps no byte of it, only what its end needs — its
/// length and how it ends ([`StreamedLine`]) — and the chunk that ends it, or
/// the file's end, hands it to the carry pass in place of a held line. So a
/// pass decoding no field holds no line at any length
/// (`docs/design/decisions.md`, "D138").
#[derive(Debug, Default)]
pub struct ChunkCarry {
    /// The line being carried, extended by [`absorb`](Self::absorb) with the
    /// chunk bytes that complete it, and grown chunk by chunk while no chunk
    /// does. Never written while the chunk is scanned.
    line: Vec<u8>,
    /// The current chunk's carry pass: `line` frozen once a chunk completes
    /// it or the file ends, empty while it waits.
    held: Bytes,
    /// How much of `held` the carry pass consumed.
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
    /// Whether the line carried into the next chunk is a row whose reader
    /// reads none of its bytes, so it may be streamed.
    streams: bool,
    /// The row being streamed, no chunk having ended it yet.
    streamed: Option<StreamedLine>,
    /// The streamed row the current chunk ended, or the file's end did,
    /// which its carry pass hands out.
    ended: Option<StreamedLine>,
}

/// A row a read loop reads through without holding ([`ChunkCarry`]): what
/// its end needs of the bytes that went by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamedLine {
    /// The row's bytes so far, an LF that ends it included.
    len: u64,
    /// How the bytes before its LF end.
    ending: EndingScan,
    /// Whether an LF has ended it, rather than the file's end or nothing yet.
    terminated: bool,
}

impl StreamedLine {
    /// The row's bytes, its LF included where one ended it.
    pub fn bytes(&self) -> u64 {
        self.len
    }

    /// How the row ends, as [`RowEnding::of`] reads its bytes before the LF.
    pub fn ending(&self) -> RowEnding {
        self.ending.ending()
    }

    /// Whether an LF ended the row: `false` only for one the file's end did.
    pub fn terminated(&self) -> bool {
        self.terminated
    }

    /// Fold in the row's next bytes, `bytes` of a chunk up to and including
    /// the LF ending it where `terminated`.
    fn read(&mut self, bytes: &[u8], terminated: bool) {
        let before = if terminated { &bytes[..bytes.len() - 1] } else { bytes };
        self.ending.feed(before);
        self.len += bytes.len() as u64;
        self.terminated = terminated;
    }
}

/// The longest row a carry holds though it may stream it: a line no longer
/// can be the terminator, `\.` before a CR, which only the scanner tells
/// from a row.
const STREAM_AFTER_BYTES: usize = b"\\.\r".len();

impl ChunkCarry {
    /// [`Passes::carried`], between two chunks: the bytes ahead of the next
    /// chunk that its carry pass begins with.
    pub fn carried(&self) -> usize {
        let streamed = self.streamed.map_or(0, |line| line.len as usize);
        self.held.len() - self.line_used + self.line.len() + self.tail.len() + streamed
    }

    /// The bytes this carry keeps allocated between two chunks: the buffers
    /// it reuses and the line it still references.
    #[cfg(test)]
    pub(super) fn allocated(&self) -> usize {
        self.line.capacity() + self.tail.capacity() + self.held.len()
    }

    /// Take onto the carry whatever prefix of `chunk` completes the line it
    /// holds, and hand out the chunk's two passes.
    ///
    /// With nothing carried the in-place pass begins at `0` — the chunk is
    /// scanned whole, where it lies. With a carry and no newline anywhere in
    /// `chunk`, it begins at `chunk.len()`, and the carry is left unfinished.
    pub fn absorb<'a>(&'a mut self, chunk: &'a [u8], eof: bool) -> Passes<'a> {
        // The line carried in is whichever pass left one, never both
        // (`Passes::span`'s assertion): an unfinished line stays where it
        // lies, what a carry pass left of its line is taken back, and a tail
        // the in-place pass left becomes the line.
        if self.line_used < self.held.len() {
            debug_assert!(self.tail.is_empty(), "both passes left a line over");
            debug_assert!(self.line.is_empty(), "a held line beside a waiting one");
            self.line.extend_from_slice(&self.held[self.line_used..]);
        } else if self.line.is_empty() {
            std::mem::swap(&mut self.line, &mut self.tail);
        } else {
            debug_assert!(self.tail.is_empty(), "both passes left a line over");
        }
        // The last carry pass's line, freed here unless a batch kept it.
        self.held = Bytes::new();
        self.line_used = 0;
        self.ended = None;
        if let Some(mut line) = self.streamed.take() {
            let newline = memchr::memchr(b'\n', chunk);
            self.split = newline.map_or(chunk.len(), |nl| nl + 1);
            self.unfinished = newline.is_none();
            line.read(&chunk[..self.split], newline.is_some());
            if newline.is_some() || eof {
                self.ended = Some(line);
            } else {
                self.streamed = Some(line);
            }
        } else if self.line.is_empty() {
            self.split = 0;
            self.unfinished = false;
        } else {
            let newline = memchr::memchr(b'\n', chunk);
            self.split = newline.map_or(chunk.len(), |nl| nl + 1);
            self.unfinished = newline.is_none();
            // A row outgrowing its chunk, which its reader does not read, is
            // streamed from here: its bytes are folded in, not kept.
            if self.unfinished
                && !eof
                && self.streams
                && self.line.len() + chunk.len() > STREAM_AFTER_BYTES
            {
                let mut line = StreamedLine::default();
                line.read(&self.line, false);
                line.read(chunk, false);
                self.line.clear();
                self.streamed = Some(line);
            } else {
                self.line.extend_from_slice(&chunk[..self.split]);
            }
        }
        // Nothing in an unfinished line is consumable before the end of the
        // file, so it is not searched again (the type's docs), and it grows on
        // where it lies; one the carry pass scans is frozen, its allocation
        // handed over whole.
        let line_waits = self.unfinished && !eof;
        if !line_waits && !self.line.is_empty() {
            self.held = Bytes::from(std::mem::take(&mut self.line));
        }
        let streaming = self.streamed.map_or(0, |line| line.len as usize);
        let Self { line, held, line_used, tail, split, streams, ended, .. } = self;
        let (line, held): (&'a [u8], &'a Bytes) = (line, held);
        Passes {
            line: if line_waits { line } else { held },
            held,
            rest: &chunk[*split..],
            line_waits,
            ended: *ended,
            streaming,
            streams,
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
pub struct Passes<'a> {
    line: &'a [u8],
    /// `line` frozen, where the carry pass scans it.
    held: &'a Bytes,
    rest: &'a [u8],
    line_waits: bool,
    /// The streamed row this chunk ended, which the carry pass hands out.
    ended: Option<StreamedLine>,
    /// Bytes of a row still being streamed.
    streaming: usize,
    line_eof: bool,
    eof: bool,
    line_used: &'a mut usize,
    tail: &'a mut Vec<u8>,
    streams: &'a mut bool,
}

impl<'a> Passes<'a> {
    /// The line the carry pass scans, as the allocation holding it — empty
    /// where there is none, or it waits for a chunk to end it. A replay keeps
    /// it beside the chunks its views point into.
    pub fn held(&self) -> &'a Bytes {
        self.held
    }

    /// The streamed row this chunk's carry pass hands out in place of a held
    /// line: one this chunk ended, or the file's end.
    pub fn streamed(&self) -> Option<StreamedLine> {
        self.ended
    }

    /// Say whether the line this chunk leaves for the next is a row whose
    /// reader reads none of its bytes, which the carry may then stream once
    /// it outgrows its chunk. A row of a block whose rows are read is never
    /// streamed.
    pub fn streams(&mut self, streams: bool) {
        *self.streams = streams;
    }

    /// The bytes `pass` scans, and whether they run to the true end of the
    /// file. A carry pass handing out a streamed row scans nothing.
    pub fn span(&self, pass: ChunkPass) -> (&'a [u8], bool) {
        match pass {
            ChunkPass::Carry if self.line_waits || self.ended.is_some() => (&[], false),
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
    pub fn consumed(&mut self, pass: ChunkPass, used: usize) {
        match pass {
            ChunkPass::Carry => *self.line_used = used,
            ChunkPass::InPlace => self.tail.extend_from_slice(&self.rest[used..]),
        }
    }

    /// Bytes carried over into the next chunk, once both passes have recorded
    /// what they used. This is what [`ReadOptions::max_line_bytes`] bounds: it
    /// is a single line's prefix, so a file whose lines fit stays flat here
    /// however large it is.
    pub fn carried(&self) -> usize {
        self.line.len() - *self.line_used + self.tail.len() + self.streaming
    }
}

/// A [`CopyScanner`] read a chunk at a time from `[position, end)` of a
/// source: the protocol every serial read loop follows, in one place.
///
/// **The end is given or found.** One given ([`new`](Self::new)) bounds every
/// read, which is strict ([`ByteRangeSource::read_range`]); one read to a
/// source's end whose extent is open ([`to_source_end`](Self::to_source_end))
/// is found by the read that answers short
/// ([`ByteRangeSource::read_at_most`]), and is [`end`](Self::end) from then on.
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
    /// Where the scan ends — `None` until a short read finds it.
    end: Option<u64>,
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
        Self::ending(scanner, Some(end), read)
    }

    /// Read `scanner` on to the end of a source whose extent is `extent`:
    /// [`new`](Self::new) where it is exact, and otherwise to wherever a read
    /// first answers short.
    pub fn to_source_end(scanner: CopyScanner, extent: Extent, read: ReadOptions) -> Self {
        Self::ending(scanner, extent.exact(), read)
    }

    fn ending(scanner: CopyScanner, end: Option<u64>, read: ReadOptions) -> Self {
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
        let bytes = match self.end {
            Some(end) => {
                let want = self.chunk_size.min((end - self.read_pos) as usize);
                if want > 0 { source.read_range(self.read_pos, want).await? } else { Bytes::new() }
            }
            None => {
                let bytes = source.read_at_most(self.read_pos, self.chunk_size).await?;
                if bytes.len() < self.chunk_size {
                    self.end = Some(self.read_pos + bytes.len() as u64);
                }
                bytes
            }
        };
        Ok(self.feed(bytes))
    }

    /// Hand out the events of `bytes`, the source's bytes from where the last
    /// chunk ended.
    pub(super) fn feed(&mut self, bytes: Bytes) -> ChunkEvents<'_> {
        debug_assert!(!self.open, "a chunk was left before its events ran out");
        let offset = self.read_pos;
        // Where the line carried in starts: the scanner stands at the first
        // byte the last chunk's passes left unconsumed.
        let line_start = self.scanner.position();
        self.read_pos += bytes.len() as u64;
        let eof = self.at_end();
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
            line_start,
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
        self.end.is_some_and(|end| self.read_pos >= end)
    }

    /// Where the scan ends: the end it was given, or the one a short read
    /// found, `None` until then.
    pub fn end(&self) -> Option<u64> {
        self.end
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
        self.carry.carried()
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
    /// The file offset of the carried line's first byte.
    line_start: u64,
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
            match self.end_pass()? {
                PassEnd::Next => {}
                PassEnd::Streamed(row) => return Ok(Some(row)),
                PassEnd::Done => return Ok(None),
            }
        }
    }

    /// Stream the rows of the block the last event opened: its reader reads
    /// none past its count, so a row that outgrows its chunk is counted
    /// without its bytes being held and handed out as an
    /// [`Event::StreamedRow`], flat in memory at any length. Called on its
    /// [`Event::CopyStart`]; the block's end undoes it.
    pub fn stream_rows(&mut self) {
        self.scanner.stream_rows();
    }

    /// Record what the pass just run out consumed, and move to the next —
    /// handing out the streamed row a carry pass stands for first — or say
    /// both have run out.
    ///
    /// Fallible, a long line refused where the chunk leaves it: checked by
    /// `next_chunk` instead, it would move for a loop leaving between chunks
    /// (the replay's dynamic filter), a semantic change bought for codegen;
    /// nor does a further attribute or cold continuation split the switch,
    /// its saving resting on inlining (`docs/design/decisions.md`, "D108").
    #[cold]
    fn end_pass(&mut self) -> Result<PassEnd<'a>> {
        let used = self.scanner.take_consumed();
        match self.pass {
            Some(ChunkPass::Carry) => {
                self.passes.consumed(ChunkPass::Carry, used);
                (self.span, self.span_eof) = self.passes.span(ChunkPass::InPlace);
                self.pass = Some(ChunkPass::InPlace);
                match self.passes.streamed() {
                    Some(line) => Ok(PassEnd::Streamed(
                        self.scanner.streamed_row(line.bytes(), line.ending())?,
                    )),
                    None => Ok(PassEnd::Next),
                }
            }
            Some(ChunkPass::InPlace) => {
                self.passes.consumed(ChunkPass::InPlace, used);
                self.passes.streams(self.scanner.streams_rows());
                (self.span, self.span_eof) = (&[], false);
                self.pass = None;
                *self.open = false;
                if !self.passes.eof && self.passes.carried() > self.max_line_bytes {
                    return Err(Error::LineTooLong {
                        offset: self.scanner.position(),
                        limit: self.max_line_bytes,
                    });
                }
                Ok(PassEnd::Done)
            }
            None => Ok(PassEnd::Done),
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

    /// The line this chunk's carry pass scans, as the allocation holding it,
    /// and the file offset of its first byte — `None` where the pass scans
    /// nothing. A loop keeping zero-copy views retains it as it does
    /// [`read`](Self::read)'s chunk, and it is the only buffer a row of the
    /// carry pass lies whole in.
    pub fn held(&self) -> Option<(u64, &'a Bytes)> {
        let held = self.passes.held();
        (!held.is_empty()).then_some((self.line_start, held))
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

/// What [`ChunkEvents::end_pass`] moved to.
enum PassEnd<'a> {
    /// The next pass, to be scanned.
    Next,
    /// The streamed row the carry pass stands for, ahead of the in-place
    /// pass.
    Streamed(Event<'a>),
    /// Both passes have run out.
    Done,
}

/// The longest line a scan carries across a chunk boundary unless a caller
/// says otherwise ([`ReadOptions::max_line_bytes`], which says where a line
/// is measured). A row a pass reads is held whole before it is emitted, so
/// this and the chunk size bound what one row may cost in memory; a dump
/// holding larger values states a larger limit.
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
    /// by the last, is not measured. The scanner cannot emit a row a pass
    /// reads until it has the whole line, so this is the only thing standing
    /// between a malformed input and unbounded memory growth; a row it
    /// streams ([`ChunkCarry`]) holds nothing and is measured alike. Defaults
    /// to [`SCAN_LINE_DEFAULT_MAX_BYTES`].
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
/// `options`' chunks, announcing their parallelism to the source, and answer
/// where the source ended — `None` where the callback stopped the scan
/// first.
///
/// The callback may return [`ControlFlow::Break`] to stop early. Nothing
/// stops it from outside: it polls no cancellation
/// (`docs/design/decisions.md`, "D26").
pub async fn scan<F>(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    mut on_event: F,
) -> Result<Option<u64>>
where
    F: FnMut(Event<'_>) -> ControlFlow<()>,
{
    let ScanOptions { read, parallelism } = *options;
    let extent = source.extent().await?;
    // Named "preamble scan", not "scan": the caller a `parse` runs is
    // `preamble::scan_preamble`, and `pgdt_mapping`'s `map_forward` announces
    // itself as "scan". Its `build_index` and `build_map` share this label
    // while scanning to EOF; neither is reached from the CLI. A single `parse` runs both in sequence, so two passes sharing
    // one name would read as an interrupted-and-resumed run.
    tracing::info!(
        bytes = %extent,
        chunk_size = read.chunk_size_bytes,
        jobs = parallelism.jobs(),
        memory_bytes = %memory_budget_display(parallelism),
        "preamble scan started",
    );
    announce_read_loop(source, read, parallelism);
    let mut chunked = ChunkedScanner::to_source_end(CopyScanner::new(), extent, read);

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
                return Ok(None);
            }
        }
        if let Some(end) = chunked.end().filter(|_| chunked.at_end()) {
            tracing::info!(bytes = end, reached_eof = true, "preamble scan complete");
            return Ok(Some(end));
        }
    }
}

#[cfg(test)]
mod tests;
