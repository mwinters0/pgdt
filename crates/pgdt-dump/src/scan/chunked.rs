//! The chunked read loop: a [`CopyScanner`] driven over a source a chunk at a
//! time, the one line straddling each chunk boundary carried across it up to
//! the line bound ([`ReadOptions::line_bound`]). Every serial pass reads through
//! [`ChunkedScanner`] — the preamble scan here ([`scan`]), `pgdt_mapping`'s
//! forward pass and statistics back-fill, and `pgdt_query`'s replay.

mod line;

use std::ops::ControlFlow;
use std::task::Poll;

use bytes::Bytes;

use self::line::CarriedLine;
pub use self::line::LineMapping;
use super::{CopyScanner, EndingScan, Event, RowEnding};
use pgdt_core::internal::delimiters;
use pgdt_core::{Error, LINE_BOUND, Result};
use pgdt_io::{
    Admission, ByteRangeSource, Extent, Holder, Parallelism, SCAN_CHUNK_DEFAULT_SIZE_BYTES,
    WaitPolicy, internal::memory_budget_display,
};

/// Which of a chunk's two spans is being scanned. The carry pass comes first:
/// the straddling line, then the rest of the chunk in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPass {
    /// The line straddling the chunk's front edge — what was carried over
    /// from the previous chunk, completed by this one's first line-terminated
    /// prefix. The bytes a read loop copies, beside a field held whole.
    Carry,
    /// The rest of the chunk, scanned where the reader left it.
    InPlace,
}

/// The one line a chunked read loop has to carry across a chunk boundary —
/// [`ChunkedScanner`]'s, and an interior worker's over its piece
/// (`pgdt_mapping`'s `scan_partition`), so each reads a line once, read by
/// read, and again from its start only where the ledger has it give the line
/// up ([`give_up`](ChunkCarry::give_up)).
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
/// **A line that outgrows its chunk moves to a mapping of its own**
/// ([`LineMapping`]), grown by remapping in huge-page steps and unmapped once
/// its last reference drops, so it holds what it has written and less than
/// one step whatever the allocator keeps; a
/// line no longer than a chunk stays in the carry's buffer
/// (`docs/design/decisions.md`, "D139").
///
/// The degenerate case is a chunk containing no newline at all: the whole of
/// it joins the carry, the in-place pass empty wherever there is a carry to
/// join — one arriving with none is handed over whole, finds no newline and
/// consumes nothing, there being no carry yet to base it against. That is the growth
/// [`ReadOptions::line_bound`] bounds ([`passes_bound`](Self::passes_bound)).
/// **Its carry pass is empty too**
/// short of the end of the file, since a span holding no newline is one the
/// scanner can neither consume nor emit anything from; handing it over anyway
/// would search the carried line again from its first byte at every chunk it
/// spans, quadratic in the line.
///
/// **A row whose reader reads nothing past its count is streamed instead**
/// ([`streams`](Passes::streams)): once a whole chunk after the one it began
/// in passes without ending such a row, the carry keeps no byte of it, only
/// what its end needs — its
/// length and how it ends — and its fields' count ([`StreamedLine`]), which
/// a reader holds to its header's width; and the chunk that ends it, or
/// the file's end, hands it to the carry pass in place of a held line. A
/// reader folding some of its fields ([`FieldFold`]) is handed each run of
/// a field's bytes as they go by, and a field it holds whole once, grown as
/// a long line is. So a pass decoding no field holds no line once a whole
/// chunk passes inside it,
/// and a census holds only the fields it decodes
/// (`docs/design/decisions.md`, "D138").
///
/// **What it holds past its chunk is drawn from the run's ledger**, where its
/// read loop is handed one, before each chunk is absorbed ([`draw_absorbing`]), and dropped and read again
/// where an earlier line needs the room ([`give_up`](Self::give_up))
/// (`docs/design/decisions.md`, "D157", "D158").
///
/// **What it carries between two chunks is one `Leftover`**, matched whole
/// both by what it says it will hold once it absorbs the next
/// ([`charge_absorbing`](Self::charge_absorbing)) and by the absorb itself
/// ([`absorb`](Self::absorb)), so a draw cannot price one case while the
/// absorb takes another (`docs/design/decisions.md`, "D158").
#[derive(Debug)]
pub struct ChunkCarry {
    /// What the last chunk's passes left for the next, and where it lies.
    left: Leftover,
    /// The line being carried, extended by [`absorb`](Self::absorb) with the
    /// chunk bytes that complete it, and grown chunk by chunk while no chunk
    /// does ([`Leftover::Waiting`]). Never written while the chunk is scanned.
    line: CarriedLine,
    /// The current chunk's carry pass: `line` frozen once a chunk completes
    /// it or the file ends, empty while it waits.
    held: Bytes,
    /// What the in-place pass left unconsumed ([`Leftover::Tail`]): the start
    /// of the next chunk's line.
    tail: Vec<u8>,
    /// Whether the line carried into the next chunk is a row whose reader
    /// holds none of its bytes, so it may be streamed.
    streams: bool,
    /// The field of a streamed row its fold holds whole, until it ends.
    field: CarriedLine,
}

/// **What a carry's last chunk left for the next**: nothing, a line in exactly
/// one of the three places a chunk's passes can leave one, or a row being
/// streamed. One value, so no two of them are ever carried at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leftover {
    /// Nothing: the last chunk's passes consumed every byte.
    Nothing,
    /// What the carry pass left of its line, `held` from `used` on — a reader
    /// that stopped inside it, the in-place span then empty.
    Held { used: usize },
    /// A line no chunk has ended yet, in `line`, or one given up and read back.
    Waiting,
    /// What the in-place pass left, in `tail`.
    Tail,
    /// A row being streamed, no chunk having ended it yet, its field held
    /// whole in `field`.
    Streamed(StreamedLine),
}

/// **What a reader folds a streamed row's fields into**, as its bytes go by
/// ([`ChunkCarry`]): each run of a field's bytes in order, its index
/// beside it, a delimiter never among them — scanning, no decoding. The
/// carry strips the row's line ending as a [`Row`](super::Row)'s `raw`
/// strips it, so a fold sees the bytes a held row's fields hold.
pub trait FieldFold: Send {
    /// Whether field `field` is wanted whole: the carry then holds it and
    /// hands it over once, as its last bytes.
    fn holds(&self, field: usize) -> bool;
    /// A streamed row begins, its first byte at file offset `offset`, ahead
    /// of any of its fields' bytes.
    fn begin_row(&mut self, offset: u64);
    /// The next `bytes` of field `field`, `ends` where they are its last.
    fn fold(&mut self, field: usize, bytes: &[u8], ends: bool);
    /// The row ends, `fields` wide, its last field's end folded already.
    fn end_row(&mut self, fields: usize);
}

/// A row a read loop reads through without holding ([`ChunkCarry`]): what
/// its end needs of the bytes that went by, and how many fields they held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamedLine {
    /// The row's bytes so far, an LF that ends it included.
    len: u64,
    /// How the bytes before its LF end.
    ending: EndingScan,
    /// Whether an LF has ended it, rather than the file's end or nothing yet.
    terminated: bool,
    /// The fields its bytes so far hold: one past its delimiters, as
    /// `pgdt_core::split_fields` counts them, so a reader holding the row to
    /// its header's width needs none of its bytes.
    fields: usize,
    /// Whether the bytes folded so far stop short of a CR no backslash
    /// escapes, the last byte read: the row's own if more follow it, and
    /// stripped if the row ends there, as a held row's is.
    cr: bool,
}

impl Default for StreamedLine {
    fn default() -> Self {
        Self { len: 0, ending: EndingScan::default(), terminated: false, fields: 1, cr: false }
    }
}

impl StreamedLine {
    /// The row's bytes, its LF included where one ended it.
    pub fn bytes(&self) -> u64 {
        self.len
    }

    /// How the row ends, as `RowEnding::of` reads its bytes before the LF.
    pub fn ending(&self) -> RowEnding {
        self.ending.ending()
    }

    /// Whether an LF ended the row: `false` only for one the file's end did.
    pub fn terminated(&self) -> bool {
        self.terminated
    }

    /// The row's fields, as `pgdt_core::split_fields` counts its line's: a
    /// raw tab is a delimiter wherever it lies, and neither a CR nor an LF
    /// is one.
    pub fn fields(&self) -> usize {
        self.fields
    }

    /// Fold in the row's next bytes, `bytes` of a chunk up to and including
    /// the LF ending it where `terminated`, handing its fields' to `fold`.
    fn read<'f>(
        &mut self,
        bytes: &[u8],
        terminated: bool,
        fold: Option<&mut (dyn FieldFold + 'f)>,
        field: &mut CarriedLine,
    ) {
        let before = if terminated { &bytes[..bytes.len() - 1] } else { bytes };
        match fold {
            Some(fold) => self.fold(before, fold, field),
            None => self.fields += delimiters(before),
        }
        self.ending.feed(before);
        self.len += bytes.len() as u64;
        self.terminated = terminated;
    }

    /// Hand `before`'s fields to `fold`, a CR ending them held back until
    /// the next byte says whether it ends the row.
    fn fold(&mut self, before: &[u8], fold: &mut dyn FieldFold, field: &mut CarriedLine) {
        if before.is_empty() {
            return;
        }
        if std::mem::take(&mut self.cr) {
            self.fold_run(b"\r", fold, field);
        }
        let run = match before.split_last() {
            Some((b'\r', head)) if !self.ending.escapes(head) => {
                self.cr = true;
                head
            }
            _ => before,
        };
        self.fold_run(run, fold, field);
    }

    /// Hand `run`'s fields to `fold`, counting its delimiters.
    fn fold_run(&mut self, run: &[u8], fold: &mut dyn FieldFold, field: &mut CarriedLine) {
        let mut start = 0;
        for tab in memchr::memchr_iter(b'\t', run) {
            self.fold_field(&run[start..tab], true, fold, field);
            self.fields += 1;
            start = tab + 1;
        }
        self.fold_field(&run[start..], false, fold, field);
    }

    /// Hand `fold` the next `bytes` of the row's current field, holding a
    /// field it holds whole until `ends`.
    fn fold_field(
        &self,
        bytes: &[u8],
        ends: bool,
        fold: &mut dyn FieldFold,
        field: &mut CarriedLine,
    ) {
        let index = self.fields - 1;
        if fold.holds(index) {
            field.extend(bytes);
            if ends {
                fold.fold(index, field.bytes(), true);
                field.clear();
            }
        } else if ends || !bytes.is_empty() {
            fold.fold(index, bytes, ends);
        }
    }

    /// End the row for `fold`: a CR held back is its ending, stripped.
    fn end<'f>(&mut self, fold: Option<&mut (dyn FieldFold + 'f)>, field: &mut CarriedLine) {
        if let Some(fold) = fold {
            self.cr = false;
            self.fold_field(&[], true, fold, field);
            fold.end_row(self.fields);
        }
    }
}

/// What a carry dropped at a ledger's asking ([`ChunkCarry::give_up`]): the
/// last bytes it had read, short of a CR a streamed row holds back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GivenUp {
    len: usize,
    /// The bytes read past them: a CR held back, or none.
    after: usize,
}

impl GivenUp {
    /// Where the bytes given up lie, the carry's next chunk being read from
    /// `read_pos`.
    pub fn range(&self, read_pos: u64) -> std::ops::Range<u64> {
        let end = read_pos - self.after as u64;
        end - self.len as u64..end
    }
}

/// **A reader's draws on its run's ledger**: its [`Holder`], and what it is
/// charged for the bytes its carry holds past its chunk — those bytes alone
/// where it holds no copy of them, as a parse does, or with what a query's
/// batch decodes beside them (`docs/design/decisions.md`, "D157") — and
/// whether its reader parks where the ledger asks it to
/// ([`parking`](Self::parking)).
pub struct Drawing<'a> {
    holder: &'a mut Holder,
    charge: &'a (dyn Fn(u64) -> u64 + Sync),
    parks: bool,
}

impl<'a> Drawing<'a> {
    /// `holder`'s draws, charged the bytes held alone.
    pub fn held(holder: &'a mut Holder) -> Self {
        Self { holder, charge: &held_alone, parks: false }
    }

    /// `holder`'s draws, charged `charge` of the bytes held.
    pub fn charged(holder: &'a mut Holder, charge: &'a (dyn Fn(u64) -> u64 + Sync)) -> Self {
        Self { holder, charge, parks: false }
    }

    /// These draws, their reader standing where it reads before each and
    /// parking where the ledger asks it to ([`Holder::park`]), so a line
    /// alone past the room is held in what it gives back
    /// (`docs/design/decisions.md`, "D163").
    pub fn parking(self) -> Self {
        Self { parks: true, ..self }
    }
}

/// The charge of bytes held with no copy beside them.
fn held_alone(bytes: u64) -> u64 {
    bytes
}

/// **Draw from `drawing` what `carry` holds past its chunk once it absorbs
/// `chunk`**, the bytes a read loop read from `read_pos`, ending the file
/// where `eof`, before it absorbs them, a streamed row's fields folded by
/// `fold` (`docs/design/decisions.md`, "D157", "D158"): the line it carries,
/// or the field of a streamed row `fold` holds whole, keyed by where that
/// starts ([`ChunkCarry::holding_from`]), or nothing, which returns the last
/// draw.
///
/// **A draw that waits holds no read**: `chunk` is dropped and read again
/// once the draw is admitted, so a reader waiting on an earlier line never
/// keeps a pooled buffer that line's reader needs (`docs/design/decisions.md`,
/// "D5"). **A reader told to give up drops what it holds of the line**
/// ([`ChunkCarry::give_up`]), waits to be admitted again — holding nothing, no
/// later line can ask it — and reads those bytes again from the source, a
/// chunk at a time, before it absorbs `chunk`; a re-read costs time, never
/// an answer. **A draw past the ledger's floor fails the read**
/// ([`Error::LineOutgrowsMemory`]), naming where what it holds begins
/// (`docs/design/decisions.md`, "D161").
///
/// **A [`parking`](Drawing::parking) reader asked to park drops `chunk`
/// first**, parks holding no read, and reads it again as it resumes
/// (`docs/design/decisions.md`, "D163").
pub async fn draw_absorbing(
    carry: &mut ChunkCarry,
    drawing: Drawing<'_>,
    source: &dyn ByteRangeSource,
    read_pos: u64,
    chunk: &mut Bytes,
    eof: bool,
    fold: Option<&mut (dyn FieldFold + '_)>,
) -> Result<()> {
    let Drawing { holder, charge: charged, parks } = drawing;
    if parks && holder.stand(read_pos) {
        let len = chunk.len();
        *chunk = Bytes::new();
        holder.park(source).await;
        if len > 0 {
            *chunk = source.read_range(read_pos, len).await?;
        }
    }
    // Read in a scope no await crosses, a `&dyn FieldFold` being `!Send`.
    let (held, start) = {
        let fold = fold.as_deref();
        let held = carry.charge_absorbing(chunk, eof, fold);
        (held, carry.holding_from(read_pos, chunk, eof, fold))
    };
    if held == 0 {
        holder.release();
        return Ok(());
    }
    let charge = charged(held);
    let len = chunk.len();
    if admitted(holder, start, charge, chunk).await? == Admission::GiveUp {
        let given = carry.give_up();
        while admitted(holder, start, charge, chunk).await? == Admission::GiveUp {}
        let range = given.range(read_pos);
        let step = carry.line.outgrows().max(1) as u64;
        let mut at = range.start;
        while at < range.end {
            let want = step.min(range.end - at);
            carry.restore(&source.read_range(at, want as usize).await?);
            at += want;
        }
    }
    if chunk.len() < len {
        *chunk = source.read_range(read_pos, len).await?;
    }
    Ok(())
}

/// `holder`'s draw of `charge` for the line at `start`, `chunk` dropped
/// where it waits, or its refusal where it passes the ledger's floor.
async fn admitted(
    holder: &mut Holder,
    start: u64,
    charge: u64,
    chunk: &mut Bytes,
) -> Result<Admission> {
    let mut draw = std::pin::pin!(holder.draw(start, charge));
    let admission = match futures::poll!(draw.as_mut()) {
        Poll::Ready(admission) => admission,
        Poll::Pending => {
            *chunk = Bytes::new();
            draw.await
        }
    };
    match admission {
        Admission::Refused(floor) => Err(floor.refusal(start, charge)),
        admission => Ok(admission),
    }
}

/// The longest row a carry holds though it may stream it: a line no longer
/// can be the terminator, `\.` before a CR, which only the scanner tells
/// from a row.
const STREAM_AFTER_BYTES: usize = b"\\.\r".len();

impl ChunkCarry {
    /// A carry for a read loop asking for `chunk_size` bytes a read: a line
    /// longer than that leaves its buffer for a mapping of its own.
    pub fn new(chunk_size: usize) -> Self {
        Self {
            left: Leftover::Nothing,
            line: CarriedLine::new(chunk_size),
            held: Bytes::new(),
            tail: Vec::new(),
            streams: false,
            field: CarriedLine::new(chunk_size),
        }
    }

    /// [`Passes::carried`], between two chunks: the bytes ahead of the next
    /// chunk that its carry pass begins with.
    pub fn carried(&self) -> usize {
        match self.left {
            Leftover::Streamed(row) => row.len as usize,
            _ => self.carried_line().len(),
        }
    }

    /// **What it holds past its chunk once it absorbs `next`**, in bytes, the
    /// read ending the file where `eof`: the line it carries, completed by
    /// `next`'s first line-terminated prefix or joined by the whole of it, or
    /// the field of a streamed row its fold holds whole, grown to `next`'s
    /// first delimiter — nothing for a row `next` starts streaming but the
    /// field `fold` holds whole across their boundary, or, for a line held
    /// past its chunk before its block streamed rows, the line and `next`
    /// whole (`starting_field`) — and zero where that is no longer than a
    /// chunk, which the reader's own term holds. So a row a pass streams is
    /// otherwise never drawn for (`docs/design/decisions.md`, "D158"). A held
    /// field still empty past a CR the fold held back is charged zero, though
    /// `next` grows it to that CR and a chunk.
    pub fn charge_absorbing(&self, next: &[u8], eof: bool, fold: Option<&dyn FieldFold>) -> u64 {
        let (held, _) = self.holding(next, eof, fold);
        if held > self.line.outgrows() { held as u64 } else { 0 }
    }

    /// What it holds past its chunk once it absorbs `next`
    /// ([`charge_absorbing`](Self::charge_absorbing)), uncapped, and how many
    /// of those bytes lie before `next`.
    fn holding(&self, next: &[u8], eof: bool, fold: Option<&dyn FieldFold>) -> (usize, usize) {
        match self.left {
            Leftover::Nothing => (0, 0),
            Leftover::Streamed(_) if self.field.is_empty() => (0, 0),
            Leftover::Streamed(row) => {
                let before = self.field.len() + usize::from(row.cr);
                (before + memchr::memchr2(b'\t', b'\n', next).unwrap_or(next.len()), before)
            }
            Leftover::Held { .. } | Leftover::Waiting | Leftover::Tail => {
                let line = self.carried_line();
                let newline = memchr::memchr(b'\n', next);
                if self.streams_from(line.len(), next.len(), newline.is_none(), eof) {
                    return self.starting_field(line, next, fold);
                }
                (line.len() + newline.map_or(next.len(), |nl| nl + 1), line.len())
            }
        }
    }

    /// **Whether the row it carries, `line` bytes so far, streams from the
    /// next chunk on**, `next_len` bytes long, ending no line where
    /// `unfinished`: its block streams rows, a whole chunk passes inside it
    /// short of the file's end, and it is longer than the terminator
    /// (`docs/design/decisions.md`, "D138").
    fn streams_from(&self, line: usize, next_len: usize, unfinished: bool, eof: bool) -> bool {
        unfinished && !eof && self.streams && line + next_len > STREAM_AFTER_BYTES
    }

    /// The field of the row it carries, `line`, that `fold` holds whole as
    /// the row starts streaming over `next`, and how many of its bytes lie
    /// before `next`: the one the line's last delimiter opens, to `next`'s
    /// first, or none where it holds no such field. Any other field it holds
    /// lies in the line or in `next`, each within a chunk — but for a line
    /// held past its chunk before its reader streamed its rows, charged as a
    /// line joined whole, an upper bound.
    fn starting_field(
        &self,
        line: &[u8],
        next: &[u8],
        fold: Option<&dyn FieldFold>,
    ) -> (usize, usize) {
        if line.len() > self.line.outgrows() {
            return (line.len() + next.len(), line.len());
        }
        match fold {
            Some(fold) if fold.holds(delimiters(line)) => {
                let before =
                    memchr::memrchr(b'\t', line).map_or(line.len(), |tab| line.len() - tab - 1);
                (before + memchr::memchr(b'\t', next).unwrap_or(next.len()), before)
            }
            _ => (0, 0),
        }
    }

    /// The line carried into the next chunk, wherever the last one's passes
    /// left it: what its carry pass left of its line, the line still
    /// waiting, or the in-place pass's tail — empty where it carries none,
    /// or streams a row.
    fn carried_line(&self) -> &[u8] {
        match self.left {
            Leftover::Held { used } => &self.held[used..],
            Leftover::Waiting => self.line.bytes(),
            Leftover::Tail => &self.tail,
            Leftover::Nothing | Leftover::Streamed(_) => &[],
        }
    }

    /// **Whether absorbing `next` carries the line it holds past `bound`**:
    /// the line, measured to its LF where `next` holds one, or else to
    /// `next`'s end, which no terminator or later byte shortens — so a line
    /// is refused exactly where it is longer than `bound`, its terminator
    /// included, and before it is held past it
    /// (`docs/design/decisions.md`, "D164"). A line `next` holds whole is no
    /// longer than `next`, which [`ReadOptions::chunk`] keeps within the
    /// bound.
    pub fn passes_bound(&self, next: &[u8], bound: usize) -> bool {
        let carried = self.carried();
        carried > 0 && carried + memchr::memchr(b'\n', next).map_or(next.len(), |nl| nl + 1) > bound
    }

    /// Where what it holds past its chunk once it absorbs `next` starts in
    /// the file, `next` being read from `read_pos` ([`charge_absorbing`](Self::charge_absorbing)):
    /// the field of a streamed row its fold holds whole, or of a row `next`
    /// starts streaming, or else the line — what a draw for it is keyed by,
    /// so a row's next held field is a line of its own to the ledger.
    pub fn holding_from(
        &self,
        read_pos: u64,
        next: &[u8],
        eof: bool,
        fold: Option<&dyn FieldFold>,
    ) -> u64 {
        read_pos - self.holding(next, eof, fold).1 as u64
    }

    /// **Drop what it holds of the line it carries**, a ledger having asked
    /// for it (`docs/design/decisions.md`, "D157"): the field of a streamed
    /// row its fold holds whole, or else the line itself, keeping what it
    /// counted of a streamed row. Answers where those bytes lie, for
    /// [`restore`](Self::restore) to take back once they are read again.
    pub fn give_up(&mut self) -> GivenUp {
        let given = match self.left {
            Leftover::Nothing => GivenUp { len: 0, after: 0 },
            Leftover::Streamed(row) => {
                let given = GivenUp { len: self.field.len(), after: usize::from(row.cr) };
                self.field.clear();
                given
            }
            // Read back into `line`, where a line no chunk has ended waits.
            Leftover::Held { .. } | Leftover::Waiting | Leftover::Tail => {
                let given = GivenUp { len: self.carried(), after: 0 };
                self.held = Bytes::new();
                self.line.clear();
                self.tail.clear();
                self.left = Leftover::Waiting;
                given
            }
        };
        debug_assert!(given.len > 0, "a carry gives up only what it holds");
        given
    }

    /// Take back the next `bytes` of what [`give_up`](Self::give_up)
    /// dropped, read again from where it said they lie.
    pub fn restore(&mut self, bytes: &[u8]) {
        match self.left {
            Leftover::Streamed(_) => self.field.extend(bytes),
            left => {
                debug_assert_eq!(left, Leftover::Waiting, "a carry restores only what it gave up");
                self.line.extend(bytes);
            }
        }
    }

    /// The bytes this carry keeps between two chunks: the buffers it reuses,
    /// the length of the line it still references and the field it holds.
    #[cfg(test)]
    pub(super) fn allocated(&self) -> usize {
        self.line.allocated() + self.tail.capacity() + self.held.len() + self.field.allocated()
    }

    /// Take onto the carry whatever prefix of `chunk` completes the line it
    /// holds, and hand out the chunk's two passes; a row it streams hands
    /// its fields to `fold`, where its reader folds them, the line carried
    /// in starting at file offset `line_start`.
    ///
    /// With nothing carried the in-place pass begins at `0` — the chunk is
    /// scanned whole, where it lies. With a carry and no newline anywhere in
    /// `chunk`, it begins at `chunk.len()`, and the carry is left unfinished.
    pub fn absorb<'a>(
        &'a mut self,
        chunk: &'a [u8],
        eof: bool,
        line_start: u64,
        mut fold: Option<&mut dyn FieldFold>,
    ) -> Passes<'a> {
        // The line carried in is wherever the last passes left it: what a
        // carry pass left of its line is taken back, a tail the in-place pass
        // left becomes the line, and an unfinished line stays where it lies.
        let left = std::mem::replace(&mut self.left, Leftover::Nothing);
        match left {
            Leftover::Held { used } => self.line.extend(&self.held[used..]),
            Leftover::Tail => self.line.swap_in(&mut self.tail),
            Leftover::Nothing | Leftover::Waiting | Leftover::Streamed(_) => {}
        }
        // The last carry pass's line, freed here unless a batch kept it.
        self.held = Bytes::new();
        let mut ended = None;
        // Where in the chunk the in-place pass begins, and whether the chunk
        // ends no line the carry pass would scan.
        let (split, unfinished) = match left {
            Leftover::Nothing => (0, false),
            Leftover::Streamed(mut line) => {
                let newline = memchr::memchr(b'\n', chunk);
                let split = newline.map_or(chunk.len(), |nl| nl + 1);
                line.read(&chunk[..split], newline.is_some(), fold.as_deref_mut(), &mut self.field);
                if newline.is_some() || eof {
                    line.end(fold.as_deref_mut(), &mut self.field);
                    ended = Some(line);
                } else {
                    self.left = Leftover::Streamed(line);
                }
                (split, newline.is_none())
            }
            Leftover::Held { .. } | Leftover::Waiting | Leftover::Tail => {
                let newline = memchr::memchr(b'\n', chunk);
                let split = newline.map_or(chunk.len(), |nl| nl + 1);
                // A row a whole chunk passes inside, which its reader does not
                // read, is streamed from here: its bytes are folded in, not kept.
                if self.streams_from(self.line.len(), chunk.len(), newline.is_none(), eof) {
                    let mut line = StreamedLine::default();
                    if let Some(fold) = fold.as_deref_mut() {
                        fold.begin_row(line_start);
                    }
                    line.read(self.line.bytes(), false, fold.as_deref_mut(), &mut self.field);
                    line.read(chunk, false, fold, &mut self.field);
                    self.line.clear();
                    self.left = Leftover::Streamed(line);
                } else {
                    self.line.extend(&chunk[..split]);
                }
                (split, newline.is_none())
            }
        };
        // Nothing in an unfinished line is consumable before the end of the
        // file, so it is not searched again (the type's docs), and it grows on
        // where it lies; one the carry pass scans is frozen, its allocation
        // handed over whole.
        let line_waits = unfinished && !eof;
        if !self.line.is_empty() {
            if line_waits {
                self.left = Leftover::Waiting;
            } else {
                self.held = self.line.freeze();
            }
        }
        let streaming = match self.left {
            Leftover::Streamed(row) => row.len as usize,
            _ => 0,
        };
        let Self { left, line, held, tail, streams, .. } = self;
        let (line, held): (&'a [u8], &'a Bytes) = (line.bytes(), held);
        Passes {
            line: if line_waits { line } else { held },
            held,
            rest: &chunk[split..],
            line_waits,
            ended,
            streaming,
            streams,
            // The carry pass runs to the true end of the file only when
            // nothing of the chunk is left over for the in-place pass.
            line_eof: eof && split == chunk.len(),
            eof,
            left,
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
    /// What the passes leave for the next chunk, as each records what it
    /// consumed.
    left: &'a mut Leftover,
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
    /// reader holds none of its bytes — it reads none, or folds its fields
    /// as they go by ([`FieldFold`]) — which the carry may then stream once
    /// a whole chunk passes inside it. A row of a block whose rows are read whole is
    /// never streamed.
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
                    !matches!(*self.left, Leftover::Held { .. }) || self.rest.is_empty(),
                    "an unfinished carry beside a non-empty in-place span mis-bases the scanner"
                );
                (self.rest, self.eof)
            }
        }
    }

    /// Record that the scanner consumed `used` bytes of `pass`'s span. After
    /// the in-place pass this is what makes the next chunk's carry: the tail
    /// the scanner could not finish, which is at most one line. A carry pass
    /// that scanned nothing — its line waits, or it hands out a streamed row —
    /// leaves what the absorb left.
    pub fn consumed(&mut self, pass: ChunkPass, used: usize) {
        match pass {
            ChunkPass::Carry if used < self.held.len() => *self.left = Leftover::Held { used },
            ChunkPass::Carry => {}
            ChunkPass::InPlace if used < self.rest.len() => {
                self.tail.extend_from_slice(&self.rest[used..]);
                *self.left = Leftover::Tail;
            }
            ChunkPass::InPlace => {}
        }
    }

    /// Bytes carried over into the next chunk, once both passes have recorded
    /// what they used: a single line's prefix, so a file whose lines fit
    /// stays flat here however large it is.
    pub fn carried(&self) -> usize {
        match *self.left {
            Leftover::Nothing => 0,
            Leftover::Held { used } => self.held.len() - used,
            Leftover::Waiting => self.line.len(),
            Leftover::Tail => self.tail.len(),
            Leftover::Streamed(_) => self.streaming,
        }
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
    line_bound: usize,
    /// Whether the chunk last handed out still has events to give.
    open: bool,
}

impl ChunkedScanner {
    /// Read `scanner` on from its [`position`](CopyScanner::position) to
    /// `end`, which is the true end of what it scans — the file's, or a replay
    /// segment's block's — in `read`'s chunks, its lines held to its bound.
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
            carry: ChunkCarry::new(read.chunk()),
            chunk: Bytes::new(),
            end,
            chunk_size: read.chunk(),
            line_bound: read.line_bound(),
            open: false,
        }
    }

    /// Read the next chunk from `source` — empty where nothing is left to
    /// read, so the last line is still scanned to the end — and hand out its
    /// events. A read's error is returned as the source gave it, and a line
    /// the chunk would carry past the bound is refused before it is absorbed
    /// ([`Error::LineTooLong`]).
    pub async fn next_chunk(&mut self, source: &dyn ByteRangeSource) -> Result<ChunkEvents<'_>> {
        self.next_chunk_folding(source, None, None).await
    }

    /// [`next_chunk`](Self::next_chunk), handing the fields of a row the
    /// open block streams to `fold`, its reader's ([`FieldFold`]), and
    /// drawing what the carry holds past its chunk through `drawing`, its
    /// reader's draws on the run's ledger ([`draw_absorbing`]), once the
    /// line it completes or grows is held to the bound, so a line past it
    /// draws nothing.
    pub async fn next_chunk_folding(
        &mut self,
        source: &dyn ByteRangeSource,
        mut fold: Option<&mut dyn FieldFold>,
        drawing: Option<Drawing<'_>>,
    ) -> Result<ChunkEvents<'_>> {
        // deficiency: KD172 — `self.chunk`, the read before this one, is
        // replaced only by `feed`, after the draw: a reader `draw_absorbing`
        // parks holds it for the whole park — a plain source's pool slot, or a
        // view pinning a compressed source's decoded block — so the share
        // `Terms` credits the park is short by it (`docs/design/decisions.md`,
        // "D162", "D163"). Its events have run out by here, so nothing borrows
        // it. **(c) unowned**; promoted by the next slice touching the park.
        let mut bytes = match self.end {
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
        self.hold_to_bound(&bytes)?;
        if let Some(drawing) = drawing {
            // As `feed` reads the end, but before `read_pos` moves past it.
            let eof = self.end.is_some_and(|end| self.read_pos + bytes.len() as u64 >= end);
            let (carry, at) = (&mut self.carry, self.read_pos);
            let folding = fold.as_deref_mut();
            draw_absorbing(carry, drawing, source, at, &mut bytes, eof, folding).await?;
        }
        Ok(self.feed(bytes, fold))
    }

    /// Refuse `bytes`, the source's bytes from where the last chunk ended,
    /// where absorbing them carries the line the scanner stands at past the
    /// bound ([`ChunkCarry::passes_bound`]): the line's start named, and its
    /// block's table where the scanner entered the block at its header.
    pub(super) fn hold_to_bound(&self, bytes: &[u8]) -> Result<()> {
        if self.carry.passes_bound(bytes, self.line_bound) {
            return Err(Error::LineTooLong {
                offset: self.scanner.position(),
                table: self.scanner.copy_table().map(str::to_owned),
                bound: self.line_bound,
            });
        }
        Ok(())
    }

    /// Hand out the events of `bytes`, the source's bytes from where the last
    /// chunk ended, a streamed row's fields to `fold`.
    pub(super) fn feed(
        &mut self,
        bytes: Bytes,
        fold: Option<&mut dyn FieldFold>,
    ) -> ChunkEvents<'_> {
        debug_assert!(!self.open, "a chunk was left before its events ran out");
        let offset = self.read_pos;
        // Where the line carried in starts: the scanner stands at the first
        // byte the last chunk's passes left unconsumed.
        let line_start = self.scanner.position();
        self.read_pos += bytes.len() as u64;
        let eof = self.at_end();
        self.chunk = bytes;
        self.open = true;
        let passes = self.carry.absorb(&self.chunk, eof, line_start, fold);
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
            open: &mut self.open,
        }
    }

    /// The scanner, standing where the events handed out so far leave it.
    pub fn scanner(&self) -> &CopyScanner {
        &self.scanner
    }

    /// Where its next chunk is read from: the position a reader stands at
    /// before that read ([`Holder::stand`]).
    pub fn read_pos(&self) -> u64 {
        self.read_pos
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
        self.carry = ChunkCarry::new(self.chunk_size);
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
    open: &'a mut bool,
}

impl<'a> ChunkEvents<'a> {
    /// The chunk's next event, or `None` once it has none left.
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

    /// Say whether the open block's rows are streamed: where `streams`, its
    /// reader reads none of a row past its count and its fields' count, or
    /// folds its fields as they go by ([`ChunkedScanner::next_chunk_folding`]),
    /// so a row a whole chunk passes inside is counted without its bytes
    /// being held and handed out as an [`Event::StreamedRow`], flat in memory at
    /// any length but for a field its fold holds whole. Asked of the line this chunk leaves for the next, so a reader
    /// whose answer moves says so before each chunk's events run out; a
    /// block's header and its end undo it.
    pub fn stream_rows(&mut self, streams: bool) {
        self.scanner.stream_rows(streams);
    }

    /// Record what the pass just run out consumed, and move to the next —
    /// handing out the streamed row a carry pass stands for first — or say
    /// both have run out.
    ///
    /// Fallible, a streamed row held to its block's first row's ending as it
    /// is handed out; no further attribute or cold continuation splits the switch, its
    /// saving resting on inlining (`docs/design/decisions.md`, "D108").
    #[cold]
    fn end_pass(&mut self) -> Result<PassEnd<'a>> {
        let used = self.scanner.take_consumed();
        match self.pass {
            Some(ChunkPass::Carry) => {
                self.passes.consumed(ChunkPass::Carry, used);
                (self.span, self.span_eof) = self.passes.span(ChunkPass::InPlace);
                self.pass = Some(ChunkPass::InPlace);
                match self.passes.streamed() {
                    Some(line) => Ok(PassEnd::Streamed(self.scanner.streamed_row(
                        line.bytes(),
                        line.ending(),
                        line.fields(),
                    )?)),
                    None => Ok(PassEnd::Next),
                }
            }
            Some(ChunkPass::InPlace) => {
                self.passes.consumed(ChunkPass::InPlace, used);
                self.passes.streams(self.scanner.streams_rows());
                (self.span, self.span_eof) = (&[], false);
                self.pass = None;
                *self.open = false;
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
    /// Not a streamed row's, which no span holds: after one, the span the next
    /// event is scanned from.
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

/// **How a chunked read loop reads**: the chunk it asks of the source — all
/// [`ChunkedScanner`] reads, and with a [`Parallelism`] a [`ScanOptions`].
/// The line it carries across a chunk boundary is held to
/// [`LINE_BOUND`], which no caller states (`docs/design/decisions.md`,
/// "D164").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadOptions {
    /// Bytes requested per read from the source, at most [`LINE_BOUND`]
    /// ([`chunk`](Self::chunk)). Defaults to
    /// [`SCAN_CHUNK_DEFAULT_SIZE_BYTES`].
    pub chunk_size_bytes: usize,
    /// The bound a line is held to: [`LINE_BOUND`] from [`Default`], and
    /// another only from a test, through [`with_line_bound`]. Private, so a
    /// caller outside this crate states a chunk by
    /// [`with_chunk_size`](Self::with_chunk_size) and never a struct literal.
    line_bound: usize,
}

/// `read`, its lines held to `line_bound` in place of [`LINE_BOUND`], so a
/// test reads at a small bound what every read loop reads at the real one.
/// Reached outside this crate only through `internal`, as no caller states a
/// bound (`docs/design/decisions.md`, "D164").
#[cfg(any(test, feature = "test-support"))]
pub fn with_line_bound(read: ReadOptions, line_bound: usize) -> ReadOptions {
    ReadOptions { line_bound, ..read }
}

impl ReadOptions {
    /// The longest line a read loop reads, its terminator included, and
    /// refuses one longer as [`Error::LineTooLong`]: [`LINE_BOUND`]
    /// (`docs/design/postgres-invariants.md`, "I71").
    pub fn line_bound(&self) -> usize {
        self.line_bound
    }

    /// These options, reading `chunk_size_bytes` a read: how a caller outside
    /// this crate states a chunk.
    pub fn with_chunk_size(mut self, chunk_size_bytes: usize) -> Self {
        self.chunk_size_bytes = chunk_size_bytes;
        self
    }

    /// The chunk a read loop asks for: the one stated, but never longer than
    /// [`line_bound`](Self::line_bound), so a line a read holds whole is
    /// within the bound and only a line a read ends inside is measured.
    pub fn chunk(&self) -> usize {
        self.chunk_size_bytes.min(self.line_bound())
    }
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self { chunk_size_bytes: SCAN_CHUNK_DEFAULT_SIZE_BYTES, line_bound: LINE_BOUND }
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
    /// The chunk every read loop of the pass reads with.
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
    source.hint_read_size(read.chunk());
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
    // itself as "scan". `pgdt_fixtures`' `build_index` and `build_map` share this label
    // while scanning to EOF; neither is reached from the CLI. A single `parse` runs both in sequence, so two passes sharing
    // one name would read as an interrupted-and-resumed run.
    tracing::info!(
        bytes = %extent,
        chunk_size = read.chunk(),
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
