//! One unit decoded from its checkpoint: whole, or forward in pieces.
//!
//! A unit is decoded with no other unit's help: from a member start with no
//! history, or from inside a member with the window its checkpoint states —
//! inside a static block, a static block's header primed ahead of its bit. It
//! is checked before the decode reports it finished — by the CRC-32 its index
//! recorded, and by the trailer of every member it holds whole — and it must
//! end exactly where its index says, in plaintext and, at a block boundary or
//! a member's end, in the compressed file alike, or the decode is refused as
//! disagreeing with the index. One ending before a static block's symbol is
//! held to its plaintext length and its CRC-32: the inflater, stopped there by
//! its output, may have read on into the next symbol, so where it stands in
//! the compressed file says nothing.

use crate::engine::{Engine, Event, Member};
use crate::error::{Error, Result};
use crate::index::{Start, UnitTask};

/// What a step of a decode did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// Compressed bytes taken from the input.
    pub consumed: usize,
    /// Plaintext bytes written to the output.
    pub produced: usize,
}

impl UnitTask {
    /// Decode the whole unit into `out`, exactly its plaintext long, from
    /// `compressed`, its [`UnitTask::compressed_range`]'s bytes (more after
    /// them are left unread but where the unit ends before a static block's
    /// symbol, the inflater then reading on into it), with `window` its
    /// checkpoint's history.
    ///
    /// # Errors
    ///
    /// [`Error::BufferSize`] for an `out` of another length; then what
    /// [`UnitTask::begin`] and [`UnitRead::read`] refuse.
    pub fn decode(&self, window: &[u8], compressed: &[u8], out: &mut [u8]) -> Result<()> {
        let len = self.plaintext_range().end - self.plaintext_range().start;
        if out.len() as u64 != len {
            return Err(Error::BufferSize { expected: len as usize, given: out.len() });
        }
        let mut read = self.begin(window)?;
        let (mut taken, mut written) = (0, 0);
        while !read.is_finished() {
            let step = read.read(&compressed[taken..], &mut out[written..])?;
            taken += step.consumed;
            written += step.produced;
        }
        Ok(())
    }

    /// A forward-only decode of the unit, which the caller feeds.
    ///
    /// # Errors
    ///
    /// [`Error::WindowLength`] where `window` is not the length the
    /// checkpoint states.
    pub fn begin(&self, window: &[u8]) -> Result<UnitRead> {
        let start = self.start;
        if window.len() != start.window_len() {
            return Err(Error::WindowLength { expected: start.window_len(), given: window.len() });
        }
        let engine = match start.start {
            Start::Member => Engine::at_member(start.bit / 8, start.plaintext)?,
            Start::Block { .. } | Start::Symbol { .. } => {
                let member = Member::resumed(
                    self.member_offset,
                    self.member_plaintext,
                    None,
                    start.plaintext,
                );
                match start.start {
                    Start::Symbol { last, .. } => {
                        Engine::at_symbol(start.bit, last, start.plaintext, window, member)?
                    }
                    _ => Engine::at_block(start.bit, start.plaintext, window, member)?,
                }
            }
        };
        let mut read = UnitRead { task: *self, engine, finished: false };
        read.settle()?;
        Ok(read)
    }
}

/// One unit, decoded forward by a caller that feeds it.
///
/// Begun by [`UnitTask::begin`]. Each [`UnitRead::read`] takes the unit's
/// compressed bytes from [`UnitRead::compressed_position`] on — as many as
/// the caller has, a short supply meaning only that more will follow — and
/// space for plaintext. It holds no borrow and is `Send`, so a handle begun on
/// one thread can be fed on another.
pub struct UnitRead {
    task: UnitTask,
    engine: Engine,
    finished: bool,
}

impl UnitRead {
    /// The task this decode was begun from.
    pub fn task(&self) -> UnitTask {
        self.task
    }

    /// The plaintext offset of the next byte this decode produces.
    pub fn position(&self) -> u64 {
        self.engine.plaintext()
    }

    /// The compressed offset of the next byte the decode takes.
    pub fn compressed_position(&self) -> u64 {
        self.engine.compressed_position()
    }

    /// Whether the unit has been decoded to its end and checked.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Take compressed bytes from `input` and produce the unit's plaintext
    /// into `out`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] where `input` is empty and the decode cannot go on
    /// without more; [`Error::BufferSize`] for an empty `out` while plaintext
    /// remains; [`Error::IndexMismatch`] where the unit runs past or stops
    /// short of where its index says it ends; [`Error::UnitCheck`],
    /// [`Error::MemberCheck`] and [`Error::MemberLength`] where a check does not
    /// match; and the framing's and the inflater's refusals.
    pub fn read(&mut self, input: &[u8], out: &mut [u8]) -> Result<Progress> {
        let mut progress = Progress::default();
        while !self.finished {
            let left = self.task.end_plaintext - self.engine.plaintext();
            if out.len() == progress.produced && left > 0 {
                if progress.produced == 0 {
                    return Err(Error::BufferSize { expected: 1, given: 0 });
                }
                break;
            }
            let room =
                (out.len() - progress.produced).min(usize::try_from(left).unwrap_or(usize::MAX));
            let step = self.engine.step(
                &input[progress.consumed..],
                &mut out[progress.produced..progress.produced + room],
            )?;
            progress.consumed += step.consumed;
            progress.produced += step.produced;
            // Held to its plaintext and CRC-32 alone, not to its compressed
            // end (`docs/design/decisions.md`, "D118").
            if self.task.ends_at_symbol && self.engine.plaintext() == self.task.end_plaintext {
                self.finish()?;
                break;
            }
            if let Some(Event::Block { bit }) = step.event
                && !self.task.ends_at_member
            {
                // The index's end is the first boundary at its plaintext, so
                // any other boundary there, or one at or past its bit, is the
                // file and the index disagreeing.
                let at_end = self.engine.plaintext() == self.task.end_plaintext;
                if at_end && bit == self.task.end_bit {
                    self.finish()?;
                    break;
                }
                if at_end || bit >= self.task.end_bit {
                    return Err(self.mismatch("a block ends where the index places none"));
                }
            }
            self.settle()?;
            if !step.progressed() {
                if progress.consumed < input.len() {
                    return Err(self.mismatch("the unit runs on past its plaintext"));
                }
                if progress == Progress::default() {
                    return Err(Error::Truncated { offset: self.engine.compressed_position() });
                }
                break;
            }
        }
        Ok(progress)
    }

    /// Finish where the engine sits between members at the unit's end, and
    /// refuse a decode that went past it.
    fn settle(&mut self) -> Result<()> {
        if !self.task.ends_at_member || !self.engine.is_between() {
            return Ok(());
        }
        let at = self.engine.compressed_position();
        let end = self.task.end_bit / 8;
        if at < end {
            return Ok(());
        }
        if at > end || self.engine.plaintext() != self.task.end_plaintext {
            return Err(self.mismatch("the unit's members end elsewhere than its index says"));
        }
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        if let Some(stored) = self.task.start.crc {
            let computed = self.engine.run_crc();
            if computed != stored {
                return Err(Error::UnitCheck {
                    plaintext: self.task.start.plaintext,
                    stored,
                    computed,
                });
            }
        }
        self.finished = true;
        Ok(())
    }

    fn mismatch(&self, reason: &'static str) -> Error {
        Error::IndexMismatch { plaintext: self.engine.plaintext(), reason }
    }
}

#[cfg(test)]
mod tests;
