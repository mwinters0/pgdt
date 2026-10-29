//! The pass's decodes — the frontier's pass placing checkpoints, one behind
//! it reading what the frontier has passed — and which of them serves a
//! read.
//!
//! **A read behind the frontier is checked as the indexed source's piecewise
//! arm checks a unit.** Inside a closed unit, one whose end the frontier has
//! placed, it decodes that unit alone from its checkpoint, so its bytes are
//! compared against the CRC-32 the unit recorded when the decode leaves it —
//! reaching its end, giving way to a decode elsewhere, or as the run's reads
//! complete — whichever provider its window was read through. Inside the open
//! unit it is a forward decode resumed at the unit's checkpoint, whose bytes
//! are the frontier's own, from the window the frontier began from, and
//! checked where the frontier's are — none of them once the frontier has
//! closed that unit (`KD137`).

use std::sync::Arc;

use pgdt_core::{Error, Result};

use super::super::super::Positioned;
use super::super::{WindowProvider, gzip_error};
use super::ahead::{Frontier, Lift};
use super::inflating::{Decode, Filled, Inflating, Step};
use super::spill::PassWindows;
use crate::source::pool::{BufferPool, PooledBuffer};

/// How many plaintext bytes a read skipping forward to its first wanted byte
/// throws away per decoder call: the pass's one scratch buffer, which its
/// charge names.
pub(super) const SKIP_SCRATCH: usize = 64 * 1024;

/// The decodes and what the pass has learnt.
pub(super) struct Pass {
    /// The decode at the frontier, until it reaches the end or fails.
    pub(super) frontier: Option<Inflating<Frontier>>,
    /// The complete index, once the frontier has reached the end.
    pub(super) ended: Option<pgdt_gzip_seek::Index>,
    /// The index as far as a frontier decode got before it failed, which
    /// nothing continues.
    pub(super) stopped: Option<pgdt_gzip_seek::Index>,
    /// How many checkpoints the frontier's pass holds: the next it places is
    /// this one.
    pub(super) placed: usize,
    /// The index the pass was resumed over, answered until the pass places a
    /// checkpoint of its own: a pass resumed at a member start sets that
    /// checkpoint again with its first step.
    pub(super) resumed_over: Option<pgdt_gzip_seek::Index>,
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
    /// The frontier stands waiting for a task ahead, whose landing the read
    /// awaits before serving on: a fetched feed's.
    Waits,
}

/// A decode behind the frontier.
enum Behind {
    /// Inside a closed unit: that unit alone, decoded from its checkpoint and
    /// compared against the CRC-32 it recorded as it ends.
    Closed(pgdt_gzip_seek::UnitRead),
    /// Inside the open unit: a forward decode resumed at its checkpoint.
    Open(pgdt_gzip_seek::Forward),
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
        frontier: Inflating<Frontier>,
        placed: usize,
        resumed_over: Option<pgdt_gzip_seek::Index>,
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
    pub(super) fn index(&self) -> pgdt_gzip_seek::Index {
        let known = self.ended.as_ref().or(self.resumed_over.as_ref()).or(self.stopped.as_ref());
        match (known, &self.frontier) {
            (Some(index), _) => index.clone(),
            (None, Some(frontier)) => frontier.decode.pass.index(),
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

    /// Let the frontier run tasks ahead of it as `lift` says, where it still
    /// decodes.
    pub(super) fn lift(&mut self, lift: &Lift) {
        if let Some(frontier) = &mut self.frontier {
            frontier.decode.lift(lift);
        }
    }

    /// Which decode serves a read from `offset`: the frontier at or past it,
    /// a decode behind it otherwise — the one kept from an earlier read where
    /// it can continue there, or one begun at the last checkpoint at or
    /// before `offset` once the kept one has been left.
    ///
    /// Deficiency register: `deficiency: KD137` — a kept decode is continued
    /// whatever kind it is, so one begun in the open unit, a forward decode,
    /// goes on serving after the frontier has closed that unit, handing out
    /// its bytes, and every closed unit's after it, compared against no
    /// CRC-32 a unit recorded, the member's trailer checking them only where a
    /// read reaches it. Neither the resumed decode's own CRC-32s, which
    /// compare nothing for a decode left mid-unit, nor decoding the closed
    /// unit whole, holding plaintext no charge names, closes it. **(c)
    /// unowned**; promoted by work on the gzip first pass scheduling the fix,
    /// leaving a kept `Behind::Open` decode once its unit has closed.
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
        let served = decode.fill(offset, out, filled, stored_size, scratch, |frontier| {
            windows.place(*placed, frontier.pass.window())?;
            *placed += 1;
            *resumed_over = None;
            Ok(())
        });
        match served {
            Ok(Filled::Ended) => {
                let decode = frontier.take().expect("the frontier was decoding");
                let before = decode.decode.pass.index();
                match decode.decode.pass.finish() {
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
            Ok(Filled::Waits) => Ok(Served::Waits),
            Ok(Filled::UnitEnded) => unreachable!("the frontier is a forward pass"),
            Err(e) => {
                let decode = frontier.take().expect("the frontier was decoding");
                *stopped = Some(decode.decode.pass.index());
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
                        Filled::Full | Filled::Ended | Filled::Waits => {
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
                Filled::Waits => unreachable!("only the frontier waits for a task"),
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
                let prefix = pgdt_gzip_seek::Index::from_parts(
                    index.spacing(),
                    parts,
                    index.member_count(),
                    None,
                )
                .map_err(gzip_error)?;
                Behind::Open(pgdt_gzip_seek::Forward::resume(&prefix, &window).map_err(gzip_error)?)
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
    #[cfg(any(feature = "http", test))]
    pub(super) fn supply(&mut self, route: Route, at: u64, bytes: &[u8]) {
        match route {
            Route::Frontier => self.frontier_input().supply(at, bytes),
            Route::Behind => self.behind_input().supply(at, bytes),
        }
    }

    fn frontier_input(&mut self) -> &mut Inflating<Frontier> {
        self.frontier.as_mut().expect("a frontier read has its decode")
    }

    fn behind_input(&mut self) -> &mut Inflating<Behind> {
        self.behind.as_mut().expect("a read behind has its decode")
    }
}

/// What a run over the pass does: answer a read, or leave the decode behind
/// the frontier.
#[derive(Clone, Copy)]
pub(super) enum Job {
    Read { offset: u64, len: usize },
    Leave,
}

impl Job {
    /// The buffer a read is answered in.
    pub(super) fn obtain(self, pool: &Arc<BufferPool>) -> Option<PooledBuffer> {
        match self {
            Job::Read { len, .. } => Some(pool.obtain(len)),
            Job::Leave => None,
        }
    }

    /// The decode the job is routed to, or none where there is nothing to do.
    pub(super) fn route(self, pass: &mut Pass) -> Result<Option<Route>> {
        Ok(match self {
            Job::Read { offset, .. } => Some(pass.route(offset)?),
            Job::Leave => pass.leave().then_some(Route::Behind),
        })
    }

    pub(super) fn offset(self) -> u64 {
        match self {
            Job::Read { offset, .. } => offset,
            Job::Leave => 0,
        }
    }

    /// Where the job's plaintext goes: the read's buffer, or nowhere.
    pub(super) fn buffer(self, out: &mut Option<PooledBuffer>) -> &mut [u8] {
        match (self, out) {
            (Job::Read { len, .. }, Some(out)) => &mut out.as_mut()[..len],
            _ => &mut [],
        }
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
                let progress = read.read(input, out).map_err(gzip_error)?;
                let needs_input = progress == pgdt_gzip_seek::Progress::default();
                Ok(Step { progress, checkpoint: false, needs_input, waits: false })
            }
            Behind::Open(forward) => Decode::step(forward, input, out),
        }
    }

    fn unit_ended(&self) -> bool {
        matches!(self, Behind::Closed(read) if read.is_finished())
    }
}
