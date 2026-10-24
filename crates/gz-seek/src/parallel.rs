//! The parallel first pass: [`Forward`](crate::Forward)'s pass, with chunks of
//! the file ahead of it decoded speculatively on the caller's threads.
//!
//! **Its frontier is a forward pass**, decoding through the zlib API and
//! handing out plaintext and checkpoints in file order, exactly as
//! [`Forward`](crate::Forward) does. Beside it, the caller takes
//! [`Speculation`]s — chunk `k` the file's compressed bytes from `k` chunk
//! lengths in to `k + 1` — runs each wherever it likes, and hands each result
//! back. Each block boundary or member start the frontier reaches, it looks for
//! a chunk beginning exactly there; finding one whose markers resolve against
//! the frontier's own history and whose every member trailer matches, it hands
//! out that chunk's plaintext as decoded and stands where the chunk stopped,
//! where the next chunk begins. Wherever no chunk begins where it stands, it
//! decodes on itself (`docs/design/decisions.md`, "D117").
//!
//! **So the index is the serial pass's at every worker count.** Both kinds of
//! decode report the same events at the same bits and plaintext offsets, and
//! one placement rule turns them into checkpoints (`placement.rs`); a chunk
//! changes only who decoded the bytes, never where a checkpoint goes, its
//! window or its CRC-32. A speculation the caller never
//! runs, or one that guessed wrong, leaves the frontier to decode those bytes
//! itself, so a pass with none at all is the serial pass.
//!
//! **Every byte handed out is checked as a forward pass checks it**: each
//! unit's CRC-32 is recorded from the plaintext, whoever decoded it, and each
//! member's trailer is compared — by the engine where its decode reaches it,
//! and before a chunk is taken where the chunk holds it — so a member a chunk
//! decoded wrongly is decoded again through the engine, whose refusal is the
//! one a caller sees.
//!
//! # What the caller does
//!
//! Read as [`Forward::read`](crate::Forward::read) is read, supplying bytes
//! from [`Parallel::compressed_position`], which jumps forward past a chunk
//! taken. Take [`Parallel::speculate`] tasks while it has memory and threads
//! for them, feed each from its [`Speculation::wants`], and hand each result
//! to [`Parallel::speculated`]. A step whose `wait` names a chunk stands where
//! that chunk may begin: the next read waits for its result rather than
//! decoding those bytes a second time.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use crate::engine::{Engine, Event as EngineEvent, Member};
use crate::error::{Error, Result};
use crate::index::{Checkpoint, End, Index, Size, Start};
use crate::native::MARKER;
use crate::placement::{Placement, Resumed};
use crate::speculate::{Chunk, Event as ChunkEvent, Point, Speculated, Speculation};
use crate::unit::Progress;
use crate::zlib::{self, WINDOW_SIZE};

/// What a step of the parallel pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParallelStep {
    /// Compressed bytes taken from the input given, and plaintext bytes
    /// written; a step handing out a chunk's plaintext takes no input.
    pub progress: Progress,
    /// The checkpoint this step placed, its window at [`Parallel::window`].
    pub checkpoint: Option<Checkpoint>,
    /// The chunk whose speculation the pass stands waiting for: hand its
    /// result to [`Parallel::speculated`] before reading on.
    pub wait: Option<u64>,
    /// The pass moved past a chunk to where the input given does not reach.
    moved: bool,
}

impl ParallelStep {
    /// Whether the pass needs more input than it was given to go on: the
    /// caller supplies more from [`Parallel::compressed_position`] or, having
    /// none, calls [`Parallel::finish`].
    pub fn needs_input(&self) -> bool {
        self.progress == Progress::default()
            && self.checkpoint.is_none()
            && self.wait.is_none()
            && !self.moved
    }
}

/// An event either kind of decode reports.
#[derive(Debug, Clone, Copy)]
enum PassEvent {
    MemberStart { offset: u64 },
    Header { bgzf: bool },
    Block { bit: u64 },
    MemberEnd,
}

/// A CRC-32 and the length of what it covers.
#[derive(Debug, Clone, Copy, Default)]
struct Crc {
    crc: u32,
    len: u64,
}

impl Crc {
    fn extend(&mut self, crc: u32, len: u64) {
        if len > 0 {
            self.crc = zlib::crc32_combine(self.crc, crc, len);
            self.len += len;
        }
    }
}

/// The last 32 KiB of the member being decoded.
struct History {
    ring: Box<[u8; WINDOW_SIZE]>,
    /// Where the next byte goes.
    head: usize,
    len: usize,
}

impl History {
    fn new() -> History {
        History { ring: Box::new([0; WINDOW_SIZE]), head: 0, len: 0 }
    }

    fn reset(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    fn push(&mut self, bytes: &[u8]) {
        let bytes = &bytes[bytes.len().saturating_sub(WINDOW_SIZE)..];
        let first = bytes.len().min(WINDOW_SIZE - self.head);
        self.ring[self.head..self.head + first].copy_from_slice(&bytes[..first]);
        self.ring[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        self.head = (self.head + bytes.len()) % WINDOW_SIZE;
        self.len = (self.len + bytes.len()).min(WINDOW_SIZE);
    }

    /// Copy the history, oldest first, into `out`, answering its length.
    fn copy_to(&self, out: &mut [u8; WINDOW_SIZE]) -> usize {
        let start = (self.head + WINDOW_SIZE - self.len) % WINDOW_SIZE;
        let first = self.len.min(WINDOW_SIZE - start);
        out[..first].copy_from_slice(&self.ring[start..start + first]);
        out[first..self.len].copy_from_slice(&self.ring[..self.len - first]);
        self.len
    }
}

/// A chunk being handed out.
struct Serving {
    chunk: Chunk,
    /// Its marked part, resolved.
    resolved: Vec<u8>,
    /// Each piece's plaintext range and CRC-32, in order, cut at every event.
    pieces: Vec<(u64, u64, u32)>,
    /// The next plaintext offset handed out, from the chunk's start.
    cursor: u64,
    /// The next event and piece not yet passed.
    event: usize,
    piece: usize,
}

impl Serving {
    /// The chunk's plaintext from offset `at`, as far as one buffer holds it.
    fn bytes(&self, at: u64) -> &[u8] {
        let marked = self.resolved.len() as u64;
        if at < marked {
            &self.resolved[at as usize..]
        } else {
            &self.chunk.plain[self.chunk.history + (at - marked) as usize..]
        }
    }
}

/// What one engine step did: its progress, the checkpoint it placed,
/// whether it did anything, and whether the member start is still to be
/// skipped.
struct EngineStep {
    progress: Progress,
    checkpoint: Option<Checkpoint>,
    progressed: bool,
    skip_start: bool,
}

/// Who decodes next.
enum Frontier {
    /// The engine, whose member start is skipped once where the pass already
    /// took it from a chunk.
    Engine {
        engine: Box<Engine>,
        skip_start: bool,
    },
    Serving(Box<Serving>),
    /// Between the two, standing at [`Parallel::standing`].
    Idle,
    /// Past the file's last trailer.
    End,
}

/// A parallel first pass over one gzip file.
pub struct Parallel {
    placement: Placement,
    chunk_bytes: u64,
    limit: usize,
    file_len: u64,
    frontier: Frontier,
    /// The place the frontier stands at and has yet to look for a chunk at.
    standing: Option<Point>,
    /// The bit of the last place the frontier stood at.
    at: u64,
    history: History,
    /// The CRC-32 of the plaintext since the last checkpoint, and since the
    /// member's start.
    unit: Crc,
    member: Crc,
    /// The member's header offset and first plaintext offset.
    member_at: (u64, u64),
    /// The plaintext handed out so far, and how much of it the CRC-32s hold.
    plaintext: u64,
    folded: u64,
    next_chunk: u64,
    in_flight: BTreeSet<u64>,
    done: BTreeMap<u64, Chunk>,
    window: Box<[u8; WINDOW_SIZE]>,
    window_len: usize,
    /// How many chunks the pass took.
    taken: u64,
}

impl Parallel {
    /// A pass from the file's first byte, placing checkpoints `spacing`
    /// plaintext bytes apart, over a file `file_len` bytes long cut into
    /// chunks of `chunk_bytes`, a speculation holding at most `limit` bytes
    /// of plaintext.
    pub fn new(
        spacing: NonZeroU64,
        chunk_bytes: NonZeroU64,
        limit: usize,
        file_len: u64,
    ) -> Result<Parallel> {
        let engine = Box::new(Engine::at_member(0, 0)?);
        let frontier = Frontier::Engine { engine, skip_start: false };
        Ok(Parallel::over(Placement::new(spacing), frontier, chunk_bytes, limit, file_len))
    }

    /// A pass resumed at the last checkpoint of an incomplete `index`, whose
    /// window is `window`, as [`Forward::resume`](crate::Forward::resume)
    /// resumes one.
    ///
    /// # Errors
    ///
    /// As [`Forward::resume`](crate::Forward::resume).
    pub fn resume(
        index: &Index,
        window: &[u8],
        chunk_bytes: NonZeroU64,
        limit: usize,
        file_len: u64,
    ) -> Result<Parallel> {
        let (placement, resumed) = Placement::resume(index, window)?;
        let (engine, at, plaintext) = match resumed {
            Resumed::Member { offset, plaintext } => {
                (Engine::at_member(offset, plaintext)?, 8 * offset, plaintext)
            }
            Resumed::Block { bit, plaintext, member } => {
                (Engine::at_block(bit, plaintext, window, member)?, bit, plaintext)
            }
        };
        let frontier = Frontier::Engine { engine: Box::new(engine), skip_start: false };
        let mut pass = Parallel::over(placement, frontier, chunk_bytes, limit, file_len);
        pass.at = at;
        pass.plaintext = plaintext;
        pass.folded = plaintext;
        if let Resumed::Block { member, .. } = resumed {
            pass.history.push(window);
            pass.member_at = (member.offset, member.plaintext);
            let prefix = member.prefix.expect("a resumed member's prefix");
            pass.member = Crc { crc: prefix, len: plaintext - member.plaintext };
        }
        Ok(pass)
    }

    fn over(
        placement: Placement,
        frontier: Frontier,
        chunk_bytes: NonZeroU64,
        limit: usize,
        file_len: u64,
    ) -> Parallel {
        Parallel {
            placement,
            chunk_bytes: chunk_bytes.get(),
            limit,
            file_len,
            frontier,
            standing: None,
            at: 0,
            history: History::new(),
            unit: Crc::default(),
            member: Crc::default(),
            member_at: (0, 0),
            plaintext: 0,
            folded: 0,
            next_chunk: 0,
            in_flight: BTreeSet::new(),
            done: BTreeMap::new(),
            window: Box::new([0; WINDOW_SIZE]),
            window_len: 0,
            taken: 0,
        }
    }

    /// The next chunk ahead of the frontier to speculate on, `None` past the
    /// file's last.
    pub fn speculate(&mut self) -> Option<Speculation> {
        let frontier = self.at / (8 * self.chunk_bytes);
        let chunk = self.next_chunk.max(frontier + 1);
        if chunk.checked_mul(self.chunk_bytes)? >= self.file_len {
            return None;
        }
        self.next_chunk = chunk + 1;
        self.in_flight.insert(chunk);
        Some(Speculation::new(chunk, self.chunk_bytes, self.limit, self.file_len))
    }

    /// A speculation's result, kept until the frontier takes it or passes
    /// where it begins.
    pub fn speculated(&mut self, result: Speculated) {
        self.in_flight.remove(&result.chunk);
        if let Some(chunk) = result.found
            && !behind(&chunk, self.at)
        {
            self.done.insert(result.chunk, chunk);
        }
    }

    /// How many speculations are handed out or held: what the caller's
    /// memory holds for them is at most this many chunks' plaintext limits,
    /// twice over for one still writing markers, beside each running one's
    /// input.
    pub fn pending(&self) -> usize {
        self.in_flight.len() + self.done.len()
    }

    /// The bytes the chunks held for the frontier hold.
    pub fn held(&self) -> usize {
        self.done.values().map(Chunk::held).sum()
    }

    /// How many chunks the frontier has taken as decoded.
    pub fn taken(&self) -> u64 {
        self.taken
    }

    /// The plaintext offset of the next byte produced.
    pub fn position(&self) -> u64 {
        self.plaintext
    }

    /// The compressed offset of the next byte the pass takes: past a chunk
    /// being handed out, where it stops.
    pub fn compressed_position(&self) -> u64 {
        match &self.frontier {
            Frontier::Engine { engine, .. } => engine.compressed_position(),
            Frontier::Serving(serving) => self.offset_of(serving.chunk.stop),
            Frontier::Idle => self.standing.map_or(self.file_len, |p| self.offset_of(p)),
            Frontier::End => self.file_len,
        }
    }

    /// The plaintext's length so far: open until [`Parallel::finish`].
    pub fn size(&self) -> Size {
        Size::Open { at_least: self.plaintext }
    }

    /// The window of the last checkpoint a step placed: empty at a member
    /// start.
    pub fn window(&self) -> &[u8] {
        &self.window[..self.window_len]
    }

    /// The index as far as the pass has got, as
    /// [`Forward::index`](crate::Forward::index) answers it.
    pub fn index(&self) -> Index {
        self.placement.index()
    }

    /// Take compressed bytes from `input`, which begins at
    /// [`Parallel::compressed_position`], and produce plaintext into `out`, as
    /// [`Forward::read`](crate::Forward::read) does, until a step
    /// [`ParallelStep::needs_input`].
    ///
    /// # Errors
    ///
    /// As [`Forward::read`](crate::Forward::read): the engine's refusals,
    /// whichever bytes a chunk decoded aside.
    pub fn read(&mut self, input: &[u8], out: &mut [u8]) -> Result<ParallelStep> {
        let mut progress = Progress::default();
        let base = self.compressed_position();
        let step =
            |progress, checkpoint, wait, moved| ParallelStep { progress, checkpoint, wait, moved };
        loop {
            if let Some(point) = self.standing {
                if let Some(chunk) = self.look(point)? {
                    return Ok(step(progress, None, Some(chunk), false));
                }
                self.standing = None;
            }
            let (checkpoint, progressed) =
                match std::mem::replace(&mut self.frontier, Frontier::Idle) {
                    Frontier::Engine { mut engine, skip_start } => {
                        // Past a chunk, the engine takes the input given only
                        // where it reaches the engine's own position.
                        let at = engine.compressed_position() - base;
                        let Some(input) = usize::try_from(at).ok().and_then(|at| input.get(at..))
                        else {
                            self.frontier = Frontier::Engine { engine, skip_start };
                            return Ok(step(progress, None, None, true));
                        };
                        let out = &mut out[progress.produced..];
                        let stepped = self.engine_step(&mut engine, skip_start, input, out)?;
                        self.frontier = Frontier::Engine { engine, skip_start: stepped.skip_start };
                        progress.consumed += stepped.progress.consumed;
                        progress.produced += stepped.progress.produced;
                        if !stepped.progressed && at > 0 {
                            // Moved, and short of what the engine needs here.
                            return Ok(step(progress, None, None, true));
                        }
                        (stepped.checkpoint, stepped.progressed)
                    }
                    Frontier::Serving(mut serving) => {
                        let (produced, checkpoint) =
                            self.serving_step(&mut serving, &mut out[progress.produced..]);
                        progress.produced += produced;
                        if serving.cursor == serving.chunk.len()
                            && serving.event == serving.chunk.events.len()
                        {
                            let stop = serving.chunk.stop;
                            self.stand(stop);
                        } else {
                            self.frontier = Frontier::Serving(serving);
                        }
                        (checkpoint, true)
                    }
                    Frontier::Idle => (None, self.standing.is_some()),
                    Frontier::End => {
                        self.frontier = Frontier::End;
                        (None, false)
                    }
                };
            if checkpoint.is_some() || !progressed || progress.produced == out.len() {
                return Ok(step(progress, checkpoint, None, false));
            }
        }
    }

    /// The input has ended: the complete index, as
    /// [`Forward::finish`](crate::Forward::finish) answers it.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] where the input ended inside a member, or held
    /// none.
    pub fn finish(self) -> Result<Index> {
        let truncated = Error::Truncated { offset: self.compressed_position() };
        let compressed = match &self.frontier {
            Frontier::Engine { engine, .. } if engine.is_between() && self.standing.is_none() => {
                engine.compressed_position()
            }
            Frontier::End => self.file_len,
            _ => return Err(truncated),
        };
        if self.placement.is_replacing() {
            return Err(truncated);
        }
        Ok(self.placement.finish(End { plaintext: self.plaintext, compressed }))
    }

    /// One engine step.
    fn engine_step(
        &mut self,
        engine: &mut Engine,
        skip_start: bool,
        input: &[u8],
        out: &mut [u8],
    ) -> Result<EngineStep> {
        let stepped = engine.step(input, out)?;
        let progress = Progress { consumed: stepped.consumed, produced: stepped.produced };
        self.history.push(&out[..stepped.produced]);
        self.plaintext += stepped.produced as u64;
        let mut skip = skip_start;
        let checkpoint = match stepped.event {
            Some(EngineEvent::MemberStart) if skip => {
                skip = false;
                None
            }
            Some(EngineEvent::MemberStart) => {
                let offset = engine.member().offset;
                let placed = self.event(PassEvent::MemberStart { offset });
                self.standing = Some(Point::Member(8 * offset));
                placed
            }
            Some(EngineEvent::Header { bgzf }) => self.event(PassEvent::Header { bgzf }),
            Some(EngineEvent::Block { bit }) => {
                self.fold(engine.cut());
                let placed = self.event(PassEvent::Block { bit });
                self.standing = Some(Point::Block(bit));
                placed
            }
            Some(EngineEvent::MemberEnd) => {
                self.fold(engine.cut());
                self.event(PassEvent::MemberEnd)
            }
            None => None,
        };
        Ok(EngineStep { progress, checkpoint, progressed: stepped.progressed(), skip_start: skip })
    }

    /// Hand out the chunk's plaintext into `out` as far as its next event, or
    /// pass that event: what it wrote, and the checkpoint it placed.
    fn serving_step(
        &mut self,
        serving: &mut Serving,
        out: &mut [u8],
    ) -> (usize, Option<Checkpoint>) {
        while let Some(&(from, to, crc)) = serving.pieces.get(serving.piece)
            && to <= serving.cursor
        {
            self.unit.extend(crc, to - from);
            self.member.extend(crc, to - from);
            serving.piece += 1;
        }
        self.folded = self.plaintext;
        if let Some(&(at, event)) = serving.chunk.events.get(serving.event)
            && at == serving.cursor
        {
            serving.event += 1;
            let placed = self.event(match event {
                ChunkEvent::Block { bit } => PassEvent::Block { bit },
                ChunkEvent::MemberEnd { .. } => PassEvent::MemberEnd,
                ChunkEvent::MemberStart { offset } => PassEvent::MemberStart { offset },
                ChunkEvent::Header { bgzf } => PassEvent::Header { bgzf },
            });
            match event {
                ChunkEvent::Block { bit } => self.at = bit,
                ChunkEvent::MemberStart { offset } => self.at = 8 * offset,
                _ => {}
            }
            return (0, placed);
        }
        let next =
            serving.chunk.events.get(serving.event).map_or(serving.chunk.len(), |&(at, _)| at);
        let next = serving.pieces.get(serving.piece).map_or(next, |&(_, to, _)| to.min(next));
        let n = usize::try_from(next - serving.cursor).unwrap_or(usize::MAX).min(out.len());
        let bytes = &serving.bytes(serving.cursor)[..n];
        out[..n].copy_from_slice(bytes);
        self.history.push(bytes);
        self.plaintext += n as u64;
        self.folded = self.plaintext;
        serving.cursor += n as u64;
        (n, None)
    }

    /// The frontier has reached `point`, a chunk's stop, with nothing decoding.
    fn stand(&mut self, point: Point) {
        match point {
            Point::End => self.frontier = Frontier::End,
            point => {
                self.frontier = Frontier::Idle;
                self.standing = Some(point);
            }
        }
    }

    /// Standing at `point`: take a chunk beginning there, or answer one to
    /// wait for, or go on decoding — with a new engine where none stands
    /// there.
    fn look(&mut self, point: Point) -> Result<Option<u64>> {
        let bit = match point {
            Point::Block(bit) | Point::Member(bit) => bit,
            Point::End => unreachable!("nothing stands at the end"),
        };
        self.at = bit;
        self.done.retain(|_, chunk| !behind(chunk, bit));
        let chunk = bit / (8 * self.chunk_bytes);
        let candidates = [chunk.checked_sub(1), Some(chunk)];
        for k in candidates.into_iter().flatten() {
            if self.done.get(&k).is_some_and(|c| c.begins_at(point)) {
                let found = self.done.remove(&k).expect("a held chunk");
                if let Some(serving) = self.take(found) {
                    self.taken += 1;
                    self.frontier = Frontier::Serving(Box::new(serving));
                    return Ok(None);
                }
            }
        }
        if chunk > 0 && self.in_flight.contains(&chunk) {
            return Ok(Some(chunk));
        }
        if let Frontier::Idle = self.frontier {
            self.frontier = self.engine_at(point)?;
        }
        Ok(None)
    }

    /// The engine, decoding on from `point` where a chunk stopped.
    fn engine_at(&mut self, point: Point) -> Result<Frontier> {
        self.folded = self.plaintext;
        Ok(match point {
            Point::Block(bit) => {
                let mut window = [0u8; WINDOW_SIZE];
                let len = self.history.copy_to(&mut window);
                let (offset, start) = self.member_at;
                let member = Member::resumed(offset, start, Some(self.member.crc), self.plaintext);
                let engine = Engine::at_block(bit, self.plaintext, &window[..len], member)?;
                Frontier::Engine { engine: Box::new(engine), skip_start: false }
            }
            Point::Member(bit) => {
                let engine = Engine::at_member(bit / 8, self.plaintext)?;
                Frontier::Engine { engine: Box::new(engine), skip_start: true }
            }
            Point::End => Frontier::End,
        })
    }

    /// A chunk beginning where the frontier stands, ready to hand out: `None`
    /// where its markers reach before the member's history or a trailer it
    /// holds does not match, its bytes then left to the engine.
    fn take(&self, chunk: Chunk) -> Option<Serving> {
        let mut window = [0u8; WINDOW_SIZE];
        let len = self.history.copy_to(&mut window);
        let window = &window[..len];
        let unknown = WINDOW_SIZE - len;
        let mut resolved = Vec::with_capacity(chunk.marked.len());
        for &value in &chunk.marked {
            match value.checked_sub(MARKER) {
                None => resolved.push(value as u8),
                Some(j) => resolved.push(*window.get(usize::from(j).checked_sub(unknown)?)?),
            }
        }
        let marked = resolved.len() as u64;
        let mut pieces = Vec::new();
        for cut in chunk.cuts(0, marked).windows(2) {
            let crc = zlib::crc32(0, &resolved[cut[0] as usize..cut[1] as usize]);
            pieces.push((cut[0], cut[1], crc));
        }
        for (cut, &crc) in chunk.cuts(marked, chunk.len()).windows(2).zip(&chunk.plain_crcs) {
            pieces.push((cut[0], cut[1], crc));
        }
        pieces.retain(|&(from, to, _)| to > from);
        // Every trailer the chunk holds, checked before a byte of it is
        // handed out.
        let mut member = self.member;
        let mut piece = 0;
        for &(at, event) in &chunk.events {
            while let Some(&(from, to, crc)) = pieces.get(piece)
                && to <= at
            {
                member.extend(crc, to - from);
                piece += 1;
            }
            match event {
                ChunkEvent::MemberEnd { crc, isize } => {
                    if member.crc != crc || member.len as u32 != isize {
                        return None;
                    }
                }
                ChunkEvent::MemberStart { .. } => member = Crc::default(),
                _ => {}
            }
        }
        Some(Serving { chunk, resolved, pieces, cursor: 0, event: 0, piece: 0 })
    }

    /// Fold the engine's run since the last fold, CRC-32 `crc`, into the
    /// unit's and the member's.
    fn fold(&mut self, crc: u32) {
        let len = self.plaintext - self.folded;
        self.unit.extend(crc, len);
        self.member.extend(crc, len);
        self.folded = self.plaintext;
    }

    /// The placement rule over one event at the current plaintext offset.
    fn event(&mut self, event: PassEvent) -> Option<Checkpoint> {
        let plaintext = self.plaintext;
        let placed = match event {
            PassEvent::MemberStart { offset } => {
                self.history.reset();
                self.unit = Crc::default();
                self.member = Crc::default();
                self.member_at = (offset, plaintext);
                self.window_len = 0;
                self.placement.member_start(plaintext, offset)
            }
            PassEvent::Header { bgzf } => self.placement.header(bgzf),
            PassEvent::Block { bit } if self.placement.owes_block(plaintext) => {
                self.placement.close(self.unit.crc);
                self.window_len = self.history.copy_to(&mut self.window);
                Some(Checkpoint::block(plaintext, bit, self.window_len))
            }
            PassEvent::MemberEnd if self.placement.member_end_records_crc() => {
                self.placement.close(self.unit.crc);
                None
            }
            _ => None,
        };
        if let Some(checkpoint) = placed {
            self.placement.place(checkpoint);
            self.unit = Crc::default();
            if checkpoint.start == Start::Member {
                self.window_len = 0;
            }
        }
        placed
    }

    /// The compressed byte a decode standing at `point` takes next.
    fn offset_of(&self, point: Point) -> u64 {
        match point {
            Point::Block(bit) | Point::Member(bit) => bit / 8,
            Point::End => self.file_len,
        }
    }
}

/// Whether a decode at `bit` is past every place `chunk` begins.
fn behind(chunk: &Chunk, bit: u64) -> bool {
    match (chunk.start, chunk.stored) {
        (_, Some(len)) => bit + 3 > len,
        (Point::Block(start) | Point::Member(start), None) => start < bit,
        (Point::End, None) => true,
    }
}

#[cfg(test)]
mod tests;
