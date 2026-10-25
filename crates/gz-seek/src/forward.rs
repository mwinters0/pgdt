//! The forward pass: a whole gzip file decoded in order, its index built as a
//! by-product, its checkpoints where the placement rule puts them
//! (`placement.rs`) — at a block boundary, or before a static block's symbol,
//! the engine walking the block to stop there.
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

use crate::engine::{Engine, Event, Stepped};
use crate::error::{Error, Result};
use crate::index::{Checkpoint, End, Index, Size};
use crate::placement::{Placement, Resumed};
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
    placement: Placement,
    window: Box<[u8; WINDOW_SIZE]>,
    window_len: usize,
}

impl Forward {
    /// A pass from the file's first byte, placing checkpoints `spacing`
    /// plaintext bytes apart.
    pub fn new(spacing: NonZeroU64) -> Result<Forward> {
        Ok(Forward {
            engine: Engine::at_member(0, 0)?,
            placement: Placement::new(spacing),
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
        let (placement, resumed) = Placement::resume(index, window)?;
        let engine = match resumed {
            Resumed::Member { offset, plaintext } => Engine::at_member(offset, plaintext)?,
            Resumed::Block { bit, plaintext, member } => {
                Engine::at_block(bit, plaintext, window, member)?
            }
            Resumed::Symbol { bit, last, plaintext, member } => {
                Engine::at_symbol(bit, last, plaintext, window, member)?
            }
        };
        Ok(Forward { engine, placement, window: Box::new([0; WINDOW_SIZE]), window_len: 0 })
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
        self.placement.index()
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
        self.engine.set_mark(self.placement.mark());
        let stepped = self.engine.step(input, out)?;
        let Stepped { consumed, produced, event } = stepped;
        let progress = Progress { consumed, produced };
        let plaintext = self.engine.plaintext();
        let checkpoint = match event {
            Some(Event::MemberStart) => {
                self.window_len = 0;
                self.placement.member_start(plaintext, self.engine.member().offset)
            }
            Some(Event::Header { bgzf }) => self.placement.header(bgzf),
            Some(Event::Block { bit }) if self.placement.owes_block(plaintext) => {
                self.placement.close(self.engine.cut());
                self.window_len = self.engine.dictionary(&mut self.window);
                Some(Checkpoint::block(plaintext, bit, self.window_len))
            }
            // A static block resumes before any of its symbols, so the walk's
            // stop is a checkpoint wherever no block boundary came first.
            Some(Event::Symbol { bit, last }) if self.placement.owes_block(plaintext) => {
                self.placement.close(self.engine.cut());
                self.window_len = self.engine.dictionary(&mut self.window);
                Some(Checkpoint::symbol(plaintext, bit, self.window_len, last))
            }
            // A unit of whole BGZF members is checked by their trailers.
            Some(Event::MemberEnd) if self.placement.member_end_records_crc() => {
                self.placement.close(self.engine.run_crc());
                None
            }
            _ => None,
        };
        if let Some(checkpoint) = checkpoint {
            self.placement.place(checkpoint);
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
        if self.placement.is_replacing() || !self.engine.is_between() {
            return Err(Error::Truncated { offset: self.engine.compressed_position() });
        }
        let end = End {
            plaintext: self.engine.plaintext(),
            compressed: self.engine.compressed_position(),
        };
        Ok(self.placement.finish(end))
    }
}

#[cfg(test)]
mod tests;
