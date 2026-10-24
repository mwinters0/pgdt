//! The pass's decodes — the forward one at the frontier placing checkpoints,
//! one behind it reading what the frontier has passed — and the compressed
//! input each is fed from.
//!
//! **A read behind the frontier is checked as the indexed source's piecewise
//! arm checks a unit.** Inside a closed unit, one whose end the frontier has
//! placed, it decodes that unit alone from its checkpoint, so its bytes are
//! compared against the CRC-32 the unit recorded when the decode leaves it —
//! reaching its end, giving way to a decode elsewhere, or as the run's reads
//! complete — whichever provider its window was read through. Inside the open
//! unit it is a forward decode resumed at the unit's checkpoint, whose bytes
//! are the frontier's own, from the window the frontier began from, and
//! checked where the frontier's are.

use pgdt_core::{Error, Result};

use super::super::super::{Positioned, short_read};
use super::super::{INPUT_CHUNK, WindowProvider, gzip_error};
use super::spill::PassWindows;

/// How many plaintext bytes a read skipping forward to its first wanted byte
/// throws away per decoder call: the pass's one scratch buffer, which its
/// charge names.
pub(super) const SKIP_SCRATCH: usize = 64 * 1024;

/// The decodes and what the pass has learnt.
pub(super) struct Pass {
    /// The decode at the frontier, until it reaches the end or fails.
    pub(super) frontier: Option<Inflating<gz_seek::Forward>>,
    /// The complete index, once the frontier has reached the end.
    pub(super) ended: Option<gz_seek::Index>,
    /// The index as far as a frontier decode got before it failed, which
    /// nothing continues.
    pub(super) stopped: Option<gz_seek::Index>,
    /// How many checkpoints the frontier's pass holds: the next it places is
    /// this one.
    pub(super) placed: usize,
    /// The index the pass was resumed over, answered until the pass places a
    /// checkpoint of its own: a pass resumed at a member start sets that
    /// checkpoint again with its first step.
    pub(super) resumed_over: Option<gz_seek::Index>,
    /// The decode behind the frontier, kept across reads.
    behind: Option<Inflating<Behind>>,
    /// What follows once the decode behind is left — set where a read it
    /// cannot serve, or the run's reads completing, leaves it, so a closed
    /// unit it sits inside is decoded to its end and compared first.
    leaving: Option<Leaving>,
    /// Where a skipped read's plaintext is thrown away.
    scratch: Vec<u8>,
}

/// Which decode a read is served by.
#[derive(Clone, Copy)]
pub(super) enum Route {
    Frontier,
    Behind,
}

/// What a decode step left a read at.
pub(super) enum Served {
    /// The read's buffer is full.
    Full,
    /// The decode needs the compressed bytes from this offset on.
    Needs(u64),
    /// The decode reached the file's end, the read holding what there was.
    Ended,
}

/// A decode behind the frontier.
enum Behind {
    /// Inside a closed unit: that unit alone, decoded from its checkpoint and
    /// compared against the CRC-32 it recorded as it ends.
    Closed(gz_seek::UnitRead),
    /// Inside the open unit: a forward decode resumed at its checkpoint.
    Open(gz_seek::Forward),
}

/// What the decode behind gives way to once it has been left.
#[derive(Clone, Copy)]
enum Leaving {
    /// A decode begun at this checkpoint.
    For(usize),
    /// None: the run's reads are complete.
    Done,
}

impl Pass {
    /// The pass with `frontier` its decode, `placed` checkpoints held, and
    /// the index it was resumed over, if it was.
    pub(super) fn new(
        frontier: Inflating<gz_seek::Forward>,
        placed: usize,
        resumed_over: Option<gz_seek::Index>,
    ) -> Pass {
        Pass {
            frontier: Some(frontier),
            ended: None,
            stopped: None,
            placed,
            resumed_over,
            behind: None,
            leaving: None,
            scratch: vec![0u8; SKIP_SCRATCH],
        }
    }

    /// The index as far as the pass has got.
    pub(super) fn index(&self) -> gz_seek::Index {
        let known = self.ended.as_ref().or(self.resumed_over.as_ref()).or(self.stopped.as_ref());
        match (known, &self.frontier) {
            (Some(index), _) => index.clone(),
            (None, Some(frontier)) => frontier.decode.index(),
            (None, None) => unreachable!("a pass that lost its frontier records where it stood"),
        }
    }

    /// The frontier's plaintext offset.
    pub(super) fn frontier_position(&self) -> Result<u64> {
        match &self.frontier {
            Some(frontier) => Ok(frontier.decode.position()),
            None => Err(Error::Io(std::io::Error::other(
                "an earlier read of this gzip file's first pass failed, and the pass does not \
                 continue past a failure",
            ))),
        }
    }

    /// Which decode serves a read from `offset`: the frontier at or past it,
    /// a decode behind it otherwise — the one kept from an earlier read where
    /// it can continue there, or one begun at the last checkpoint at or
    /// before `offset` once the kept one has been left.
    pub(super) fn route(&mut self, offset: u64) -> Result<Route> {
        if offset >= self.frontier_position()? {
            return Ok(Route::Frontier);
        }
        let index = self.index();
        let checkpoints = index.checkpoints();
        let at = checkpoints.partition_point(|c| c.plaintext <= offset).saturating_sub(1);
        let begins = checkpoints[at].plaintext;
        // Continuing is never dearer than restarting where the kept decode
        // stands between the checkpoint and the offset.
        if self.behind.as_ref().is_some_and(|decode| {
            let position = decode.decode.position();
            position <= offset && position >= begins
        }) {
            return Ok(Route::Behind);
        }
        self.leaving = Some(Leaving::For(at));
        Ok(Route::Behind)
    }

    /// Whether the decode behind sits inside a closed unit it has not been
    /// decoded to the end of — the one thing the run's reads completing owes
    /// a check — marking it to be left, to no decode after it, if so.
    pub(super) fn leave(&mut self) -> bool {
        let owes = self.behind.as_ref().is_some_and(|decode| decode.decode.owes_check());
        self.leaving = owes.then_some(Leaving::Done);
        owes
    }

    /// Decode on into `out` for a read from `offset`, `filled` bytes of it
    /// already there.
    pub(super) fn serve(
        &mut self,
        route: Route,
        offset: u64,
        out: &mut [u8],
        filled: &mut usize,
        windows: &PassWindows,
        stored_size: u64,
    ) -> Result<Served> {
        match route {
            Route::Frontier => self.serve_frontier(offset, out, filled, windows, stored_size),
            Route::Behind => {
                let served = self.serve_behind(offset, out, filled, windows, stored_size);
                if served.is_err() {
                    self.behind = None;
                    self.leaving = None;
                }
                served
            }
        }
    }

    fn serve_frontier(
        &mut self,
        offset: u64,
        out: &mut [u8],
        filled: &mut usize,
        windows: &PassWindows,
        stored_size: u64,
    ) -> Result<Served> {
        let Pass { frontier, scratch, placed, resumed_over, ended, stopped, .. } = self;
        let decode = frontier.as_mut().expect("a frontier read has its decode");
        let served = decode.fill(offset, out, filled, stored_size, scratch, |forward| {
            windows.place(*placed, forward.window())?;
            *placed += 1;
            *resumed_over = None;
            Ok(())
        });
        match served {
            Ok(Filled::Ended) => {
                let decode = frontier.take().expect("the frontier was decoding");
                let before = decode.decode.index();
                match decode.decode.finish() {
                    Ok(index) => {
                        *resumed_over = None;
                        *ended = Some(index);
                        Ok(Served::Ended)
                    }
                    Err(e) => {
                        *stopped = Some(before);
                        Err(gzip_error(e))
                    }
                }
            }
            Ok(Filled::Full) => Ok(Served::Full),
            Ok(Filled::Needs(at)) => Ok(Served::Needs(at)),
            Ok(Filled::UnitEnded) => unreachable!("the frontier is a forward pass"),
            Err(e) => {
                let decode = frontier.take().expect("the frontier was decoding");
                *stopped = Some(decode.decode.index());
                Err(e)
            }
        }
    }

    /// A read behind the frontier: the decode it was routed to left first
    /// where it must be, and the next unit's decode begun wherever a closed
    /// unit ends inside the read.
    fn serve_behind(
        &mut self,
        offset: u64,
        out: &mut [u8],
        filled: &mut usize,
        windows: &PassWindows,
        stored_size: u64,
    ) -> Result<Served> {
        loop {
            if let Some(leaving) = self.leaving {
                if let Some(decode) =
                    self.behind.as_mut().filter(|decode| decode.decode.owes_check())
                {
                    match decode.fill(
                        u64::MAX,
                        &mut [],
                        &mut 0,
                        stored_size,
                        &mut self.scratch,
                        |_| Ok(()),
                    )? {
                        Filled::UnitEnded => {}
                        Filled::Needs(at) => return Ok(Served::Needs(at)),
                        Filled::Full | Filled::Ended => {
                            return Err(Error::Io(std::io::Error::other(
                                "a gzip unit's decode stopped short of its check",
                            )));
                        }
                    }
                }
                self.leaving = None;
                match leaving {
                    Leaving::For(at) => self.behind = Some(self.begin_behind(at, windows)?),
                    Leaving::Done => {
                        self.behind = None;
                        return Ok(Served::Full);
                    }
                }
            }
            let Pass { behind, scratch, .. } = self;
            let decode = behind.as_mut().expect("a read behind has its decode");
            match decode.fill(offset, out, filled, stored_size, scratch, |_| Ok(()))? {
                Filled::Full => return Ok(Served::Full),
                Filled::Needs(at) => return Ok(Served::Needs(at)),
                Filled::Ended => return Ok(Served::Ended),
                Filled::UnitEnded => {
                    let Behind::Closed(read) = &decode.decode else {
                        unreachable!("only a closed unit's decode ends at its unit's end")
                    };
                    let next = read.task().unit() + 1;
                    self.behind = Some(self.begin_behind(next, windows)?);
                }
            }
        }
    }

    /// A decode behind the frontier begun at checkpoint `at`, its window read
    /// through the pass's provider: the unit alone where the frontier has
    /// closed it, a forward decode resumed there where it is the open one.
    fn begin_behind(&self, at: usize, windows: &PassWindows) -> Result<Inflating<Behind>> {
        let index = self.index();
        let checkpoints = index.checkpoints();
        let mut window = vec![0u8; checkpoints[at].window_len()];
        if !window.is_empty() {
            windows.read_window(at, &mut window)?;
        }
        let decode = match index.unit(at) {
            Some(task) => Behind::Closed(task.begin(&window).map_err(gzip_error)?),
            None => {
                let mut parts = checkpoints[..=at].to_vec();
                parts[at].crc = None;
                let prefix =
                    gz_seek::Index::from_parts(index.spacing(), parts, None).map_err(gzip_error)?;
                Behind::Open(gz_seek::Forward::resume(&prefix, &window).map_err(gzip_error)?)
            }
        };
        Ok(Inflating::new(decode))
    }

    /// The routed decode's input, pulled from `file` at `at`.
    pub(super) fn refill_from_file(
        &mut self,
        route: Route,
        file: &std::fs::File,
        at: u64,
        stored_size: u64,
    ) -> Result<()> {
        let read = |buf: &mut [u8]| Ok(Positioned::read_at(file, at, buf)?);
        match route {
            Route::Frontier => self.frontier_input().refill(at, stored_size, read),
            Route::Behind => self.behind_input().refill(at, stored_size, read),
        }
    }

    /// The routed decode's input, `bytes` fetched from `at`.
    pub(super) fn supply(&mut self, route: Route, at: u64, bytes: &[u8]) {
        match route {
            Route::Frontier => self.frontier_input().supply(at, bytes),
            Route::Behind => self.behind_input().supply(at, bytes),
        }
    }

    fn frontier_input(&mut self) -> &mut Inflating<gz_seek::Forward> {
        self.frontier.as_mut().expect("a frontier read has its decode")
    }

    fn behind_input(&mut self) -> &mut Inflating<Behind> {
        self.behind.as_mut().expect("a read behind has its decode")
    }
}

/// What filling a read left its decode at: [`Served`], and the end of a
/// closed unit, past which the read goes on in the next unit's decode.
enum Filled {
    Full,
    Needs(u64),
    Ended,
    UnitEnded,
}

/// One step of a decode, as [`Inflating::fill`] drives either kind.
pub(super) struct Step {
    progress: gz_seek::Progress,
    /// Whether the step placed a checkpoint.
    checkpoint: bool,
}

/// A decode [`Inflating`] feeds: a forward pass, or the decode behind the
/// frontier.
pub(super) trait Decode {
    fn position(&self) -> u64;
    fn compressed_position(&self) -> u64;
    /// Where the compressed bytes this decode may be fed end.
    fn input_end(&self, stored_size: u64) -> u64;
    fn step(&mut self, input: &[u8], out: &mut [u8]) -> Result<Step>;
    /// Whether it is a closed unit decoded to its end and compared.
    fn unit_ended(&self) -> bool;
}

impl Decode for gz_seek::Forward {
    fn position(&self) -> u64 {
        gz_seek::Forward::position(self)
    }

    fn compressed_position(&self) -> u64 {
        gz_seek::Forward::compressed_position(self)
    }

    fn input_end(&self, stored_size: u64) -> u64 {
        stored_size
    }

    fn step(&mut self, input: &[u8], out: &mut [u8]) -> Result<Step> {
        let step = self.read(input, out).map_err(gzip_error)?;
        Ok(Step { progress: step.progress, checkpoint: step.checkpoint.is_some() })
    }

    fn unit_ended(&self) -> bool {
        false
    }
}

impl Behind {
    /// Whether this sits inside a closed unit not yet decoded to its end:
    /// bytes may have gone out that its check has not compared.
    fn owes_check(&self) -> bool {
        matches!(self, Behind::Closed(read) if !read.is_finished())
    }
}

impl Decode for Behind {
    fn position(&self) -> u64 {
        match self {
            Behind::Closed(read) => read.position(),
            Behind::Open(forward) => forward.position(),
        }
    }

    fn compressed_position(&self) -> u64 {
        match self {
            Behind::Closed(read) => read.compressed_position(),
            Behind::Open(forward) => forward.compressed_position(),
        }
    }

    /// A closed unit's decode is fed no further than the unit's own
    /// compressed extent, so its refusal of a unit running on past where the
    /// index says it ends is about the file, never about bytes over-supplied.
    fn input_end(&self, stored_size: u64) -> u64 {
        match self {
            Behind::Closed(read) => read.task().compressed_range().end,
            Behind::Open(_) => stored_size,
        }
    }

    fn step(&mut self, input: &[u8], out: &mut [u8]) -> Result<Step> {
        match self {
            Behind::Closed(read) => {
                Ok(Step { progress: read.read(input, out).map_err(gzip_error)?, checkpoint: false })
            }
            Behind::Open(forward) => Decode::step(forward, input, out),
        }
    }

    fn unit_ended(&self) -> bool {
        matches!(self, Behind::Closed(read) if read.is_finished())
    }
}

/// One decode, and the compressed input it is fed from.
pub(super) struct Inflating<D> {
    pub(super) decode: D,
    input: Vec<u8>,
    /// The compressed offset of `input`'s first byte.
    input_at: u64,
}

impl<D> Inflating<D> {
    pub(super) fn new(decode: D) -> Inflating<D> {
        Inflating { decode, input: Vec::new(), input_at: 0 }
    }

    /// Replace the input with the compressed bytes from `at`, up to
    /// [`INPUT_CHUNK`] of them, read by `read`.
    fn refill(
        &mut self,
        at: u64,
        stored_size: u64,
        read: impl FnOnce(&mut [u8]) -> Result<usize>,
    ) -> Result<()> {
        let want = usize::try_from(INPUT_CHUNK.min(stored_size.saturating_sub(at)))
            .map_err(|_| short_read())?;
        self.input.resize(want, 0);
        let got = read(&mut self.input)?;
        if got < want {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the gzip file ended before the length it was opened at",
            )));
        }
        self.input_at = at;
        Ok(())
    }

    /// Replace the input with `bytes`, fetched from `at`.
    fn supply(&mut self, at: u64, bytes: &[u8]) {
        self.input.clear();
        self.input.extend_from_slice(bytes);
        self.input_at = at;
    }
}

impl<D: Decode> Inflating<D> {
    /// Decode on into `out[filled..]`, skipping the plaintext before
    /// `offset` through `scratch`, `placed` told of each checkpoint a forward
    /// pass places, and failing the read where it fails; a closed unit's
    /// decode stops at the unit's end, compared.
    fn fill(
        &mut self,
        offset: u64,
        out: &mut [u8],
        filled: &mut usize,
        stored_size: u64,
        scratch: &mut [u8],
        mut placed: impl FnMut(&D) -> Result<()>,
    ) -> Result<Filled> {
        loop {
            let position = self.decode.position();
            let skipping = position < offset;
            if !skipping && *filled == out.len() {
                return Ok(Filled::Full);
            }
            if self.decode.unit_ended() {
                return Ok(Filled::UnitEnded);
            }
            let at = self.decode.compressed_position();
            let limit = self.decode.input_end(stored_size);
            let held_end = self.input_at + self.input.len() as u64;
            let held: &[u8] = if at >= self.input_at && at <= held_end {
                let from = (at - self.input_at) as usize;
                let to = (limit.max(at).min(held_end) - self.input_at) as usize;
                &self.input[from..to]
            } else {
                &[]
            };
            let reaches_end = at + held.len() as u64 >= limit;
            if held.is_empty() && !reaches_end {
                return Ok(Filled::Needs(at));
            }
            let target = if skipping {
                let skip = usize::try_from(offset - position).unwrap_or(usize::MAX);
                let len = skip.min(scratch.len());
                &mut scratch[..len]
            } else {
                &mut out[*filled..]
            };
            let step = self.decode.step(held, target)?;
            if !skipping {
                *filled += step.progress.produced;
            }
            if step.checkpoint {
                placed(&self.decode)?;
            }
            if step.progress == gz_seek::Progress::default() && !step.checkpoint {
                if self.decode.unit_ended() {
                    return Ok(Filled::UnitEnded);
                }
                if reaches_end {
                    return Ok(Filled::Ended);
                }
                if held.len() as u64 >= INPUT_CHUNK {
                    return Err(Error::Io(std::io::Error::other(
                        "a gzip decode made no progress over a whole input chunk",
                    )));
                }
                return Ok(Filled::Needs(at));
            }
        }
    }
}
