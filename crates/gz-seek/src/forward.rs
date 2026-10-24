//! The forward pass: a whole gzip file decoded in order, its index built as a
//! by-product.
//!
//! # Where a checkpoint goes is a function of the file alone
//!
//! A checkpoint is placed at **every member start**, and at **the first deflate
//! block boundary at or after each multiple of the spacing** in plaintext — a
//! member start counting as a boundary — so the same file and spacing give the
//! same index however the caller cuts its buffers, wherever an earlier pass was
//! interrupted, and on whatever thread each piece ran
//! (`docs/design/roadmap-P15-gzip.md`, "Settled in the grilling"). The decoder
//! stops at every block's end, and the pass checks each against the next
//! multiple; what the spacing moves is how many windows are copied, never how
//! often the decoder stops.
//!
//! **Over a run of BGZF members the pass places what a [`Walk`](crate::Walk)
//! places.** While every member's header so far has been a BGZF member's, a
//! member start is a checkpoint only where it is the file's first or the unit
//! since the last checkpoint holds the spacing, no block boundary is one, and
//! each unit of whole members records no CRC-32, its trailers checking it: so
//! the pass over a BGZF file builds the index its walk reads off the headers.
//! From the first member whose header is not BGZF's, the rule above governs.
//! Only a header ends the run, which a pass resumed at a member start reads
//! again (`docs/design/decisions.md`, "D116").
//!
//! # What a step hands out
//!
//! The plaintext, into the caller's buffer, and each checkpoint as it is
//! placed, with its window readable from [`Forward::window`] until the next
//! step. A step that places a checkpoint returns at it. Every member's CRC-32
//! and `ISIZE` are compared as its trailer is read, the member's CRC-32 being
//! the combination of its units', so each unit's value is recorded at no
//! second pass over its bytes.

use std::num::NonZeroU64;

use crate::engine::{Engine, Event, Member, Stepped};
use crate::error::{Error, Result};
use crate::index::{Checkpoint, End, Index, Size, Start};
use crate::unit::Progress;
use crate::zlib::WINDOW_SIZE;

/// What a step of the forward pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardStep {
    /// Compressed bytes taken, and plaintext bytes written.
    pub progress: Progress,
    /// The checkpoint this step placed, its window at [`Forward::window`].
    pub checkpoint: Option<Checkpoint>,
}

/// A forward pass over one gzip file.
pub struct Forward {
    engine: Engine,
    spacing: NonZeroU64,
    /// The next multiple of the spacing a checkpoint is owed at.
    next_mark: u64,
    checkpoints: Vec<Checkpoint>,
    /// Whether every member's header so far has been a BGZF member's: while
    /// it holds, members are coalesced into units as a walk coalesces them.
    coalescing: bool,
    /// A member start the spacing does not owe, placed only if its header
    /// proves not to be a BGZF member's.
    deferred: Option<Checkpoint>,
    /// How many members have begun.
    members: u64,
    /// How many members had begun when the last checkpoint was placed.
    members_at_last: u64,
    /// The member start the pass begins at and has yet to place, with the
    /// member count of the index it stands for: what [`Forward::index`]
    /// answers until it is placed.
    replacing: Option<(Checkpoint, u64)>,
    window: Box<[u8; WINDOW_SIZE]>,
    window_len: usize,
}

impl Forward {
    /// A pass from the file's first byte, placing checkpoints `spacing`
    /// plaintext bytes apart.
    pub fn new(spacing: NonZeroU64) -> Result<Forward> {
        Ok(Forward {
            engine: Engine::at_member(0, 0)?,
            spacing,
            next_mark: 0,
            checkpoints: Vec::new(),
            coalescing: true,
            deferred: None,
            members: 0,
            members_at_last: 0,
            replacing: Some((Checkpoint::member(0, 0), 1)),
            window: Box::new([0; WINDOW_SIZE]),
            window_len: 0,
        })
    }

    /// A pass resumed at the last checkpoint of an incomplete `index`, whose
    /// window is `window`; it extends that index, and builds the one an
    /// uninterrupted pass builds.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidIndex`] for a complete index, or one whose member
    /// holding the last checkpoint has a unit with no CRC-32 — one no forward
    /// pass built; [`Error::WindowLength`] for a window of another length.
    pub fn resume(index: &Index, window: &[u8]) -> Result<Forward> {
        if index.is_complete() {
            return Err(Error::InvalidIndex { reason: "a complete index has nothing to resume" });
        }
        let checkpoints = index.checkpoints().to_vec();
        let at = checkpoints.len() - 1;
        let last = checkpoints[at];
        if window.len() != last.window_len() {
            return Err(Error::WindowLength { expected: last.window_len(), given: window.len() });
        }
        let engine = match last.start {
            Start::Member => Engine::at_member(last.bit / 8, last.plaintext)?,
            Start::Block { .. } => {
                let prefix = index.member_prefix(at).ok_or(Error::InvalidIndex {
                    reason: "a unit before the resume point carries no CRC-32",
                })?;
                let member_at = checkpoints[..=at]
                    .iter()
                    .rposition(|c| c.start == Start::Member)
                    .expect("the first checkpoint is a member start");
                let member = checkpoints[member_at];
                let member =
                    Member::resumed(member.bit / 8, member.plaintext, Some(prefix), last.plaintext);
                Engine::at_block(last.bit, last.plaintext, window, member)?
            }
        };
        let members = index.member_count();
        let mut forward = Forward {
            engine,
            spacing: index.spacing(),
            next_mark: 0,
            checkpoints,
            coalescing: false,
            deferred: None,
            members,
            members_at_last: members,
            replacing: None,
            window: Box::new([0; WINDOW_SIZE]),
            window_len: 0,
        };
        if last.start == Start::Member {
            // The member's start is placed again, and its member counted
            // again, as the pass reads its header.
            forward.checkpoints.pop();
            forward.members -= 1;
            forward.replacing = Some((last, members));
            // A run of BGZF members leaves its units with no CRC-32, and only
            // such a run does: the unit before the member says whether the
            // run reached it, and the member's own header is read again.
            forward.coalescing = forward.checkpoints.last().is_none_or(|unit| unit.crc.is_none());
        } else {
            forward.checkpoints[at].crc = None;
            forward.placed(last.plaintext);
        }
        Ok(forward)
    }

    /// The plaintext offset of the next byte produced.
    pub fn position(&self) -> u64 {
        self.engine.plaintext()
    }

    /// The compressed offset of the next byte the pass takes.
    pub fn compressed_position(&self) -> u64 {
        self.engine.compressed_position()
    }

    /// The plaintext's length so far: open until [`Forward::finish`].
    pub fn size(&self) -> Size {
        Size::Open { at_least: self.engine.plaintext() }
    }

    /// The window of the last checkpoint a step placed: empty at a member
    /// start.
    pub fn window(&self) -> &[u8] {
        &self.window[..self.window_len]
    }

    /// The index as far as the pass has got: its last checkpoint's unit open,
    /// so that [`Forward::resume`] over it begins there.
    pub fn index(&self) -> Index {
        let mut checkpoints = self.checkpoints.clone();
        let members = match self.replacing {
            Some((start, members)) => {
                checkpoints.push(start);
                members
            }
            None => self.members_at_last,
        };
        if let Some(last) = checkpoints.last_mut() {
            last.crc = None;
        }
        Index::built(self.spacing, checkpoints, members, None)
    }

    /// Take compressed bytes from `input`, which begins at
    /// [`Forward::compressed_position`], and produce plaintext into `out`. A
    /// step that makes no progress needs more input: the caller supplies it
    /// or, having none, calls [`Forward::finish`].
    ///
    /// # Errors
    ///
    /// The framing's and the inflater's refusals, and a member whose trailer
    /// does not match its plaintext.
    pub fn read(&mut self, input: &[u8], out: &mut [u8]) -> Result<ForwardStep> {
        let mut progress = Progress::default();
        loop {
            let (step, progressed) =
                self.step(&input[progress.consumed..], &mut out[progress.produced..])?;
            progress.consumed += step.progress.consumed;
            progress.produced += step.progress.produced;
            if step.checkpoint.is_some() || !progressed || progress.produced == out.len() {
                return Ok(ForwardStep { progress, checkpoint: step.checkpoint });
            }
        }
    }

    /// One engine step, and whether it did anything: a block boundary short
    /// of the next mark ends one with nothing consumed or produced.
    fn step(&mut self, input: &[u8], out: &mut [u8]) -> Result<(ForwardStep, bool)> {
        let stepped = self.engine.step(input, out)?;
        let Stepped { consumed, produced, event } = stepped;
        let progress = Progress { consumed, produced };
        let plaintext = self.engine.plaintext();
        let checkpoint = match event {
            Some(Event::MemberStart) => {
                self.members += 1;
                self.window_len = 0;
                let start = Checkpoint::member(plaintext, self.engine.member().offset);
                if self.owes_member(plaintext) {
                    Some(start)
                } else {
                    self.deferred = Some(start);
                    None
                }
            }
            Some(Event::Header { bgzf }) => {
                self.coalescing &= bgzf;
                let deferred = self.deferred.take();
                if self.coalescing { None } else { deferred }
            }
            // deficiency: KD129 — a checkpoint is a block boundary, so a
            // member whose writer ends a block only at `Z_FINISH`, before its
            // final one (`deflate_quick`, level 1 under zlib-ng's default build and
            // zlib-rs, which opens that block over its lookahead at
            // `Z_FINISH`) is one unit, or two the last that block, however
            // small the spacing, and every read of it decodes from its start. A static block resumes at any
            // symbol boundary given a synthetic three-bit header primed ahead
            // of the byte's remaining bits; what is missing is a stop there,
            // which no inflater gives, so placing one means walking the static
            // codes here.
            Some(Event::Block { bit }) if !self.coalescing && plaintext >= self.next_mark => {
                self.close_unit();
                self.window_len = self.engine.dictionary(&mut self.window);
                Some(Checkpoint {
                    plaintext,
                    bit,
                    start: Start::Block {
                        window: u16::try_from(self.window_len).expect("a window is at most 32 KiB"),
                    },
                    crc: None,
                })
            }
            // A unit of whole BGZF members is checked by their trailers.
            Some(Event::MemberEnd) if !self.coalescing => {
                let crc = self.engine.run_crc();
                self.checkpoints.last_mut().expect("a member began").crc = Some(crc);
                None
            }
            _ => None,
        };
        if let Some(checkpoint) = checkpoint {
            self.checkpoints.push(checkpoint);
            self.members_at_last = self.members;
            self.replacing = None;
            self.placed(plaintext);
        }
        Ok((ForwardStep { progress, checkpoint }, stepped.progressed()))
    }

    /// The input has ended: the complete index, its end where the last
    /// member's trailer did.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] where the input ended inside a member, or held
    /// none.
    pub fn finish(self) -> Result<Index> {
        if self.replacing.is_some() || !self.engine.is_between() {
            return Err(Error::Truncated { offset: self.engine.compressed_position() });
        }
        let end = End {
            plaintext: self.engine.plaintext(),
            compressed: self.engine.compressed_position(),
        };
        Ok(Index::built(self.spacing, self.checkpoints, self.members, Some(end)))
    }

    /// Whether a member beginning at `plaintext` begins a unit before its
    /// header is read: always, unless members are being coalesced and the
    /// unit since the last checkpoint is short of the spacing.
    fn owes_member(&self, plaintext: u64) -> bool {
        !self.coalescing
            || self
                .checkpoints
                .last()
                .is_none_or(|last| plaintext - last.plaintext >= self.spacing.get())
    }

    /// Record the unit ending here: the CRC-32 of the run since the last
    /// checkpoint.
    fn close_unit(&mut self) {
        let crc = self.engine.cut();
        self.checkpoints.last_mut().expect("a member began").crc = Some(crc);
    }

    /// A checkpoint was placed at `plaintext`: the next is owed at the next
    /// multiple of the spacing past it.
    fn placed(&mut self, plaintext: u64) {
        let spacing = self.spacing.get();
        self.next_mark = (plaintext / spacing + 1) * spacing;
    }
}

#[cfg(test)]
mod tests;
