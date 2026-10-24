//! One decode of the pass and the compressed input it is fed from: the
//! frontier's parallel pass, or a forward decode behind it, each filling a
//! read from what it holds and saying where it needs more.

use pgdt_core::{Error, Result};

use super::super::super::short_read;
use super::super::{INPUT_CHUNK, gzip_error};
use super::ahead::Frontier;

/// What filling a read left its decode at: the pass's `Served`, and the end of a
/// closed unit, past which the read goes on in the next unit's decode.
pub(super) enum Filled {
    Full,
    Needs(u64),
    Ended,
    UnitEnded,
    Waits,
}

/// One step of a decode, as [`Inflating::fill`] drives either kind.
pub(super) struct Step {
    pub(super) progress: gz_seek::Progress,
    /// Whether the step placed a checkpoint.
    pub(super) checkpoint: bool,
    /// Whether the decode needs more input than it was given to go on: a step
    /// that did nothing else, or none at all.
    pub(super) needs_input: bool,
    /// Whether a fetched frontier stands waiting for a task ahead, which its
    /// read awaits before stepping again.
    pub(super) waits: bool,
}

/// A decode [`Inflating`] feeds: the frontier, or the decode behind it.
pub(super) trait Decode {
    fn position(&self) -> u64;
    fn compressed_position(&self) -> u64;
    /// Where the compressed bytes this decode may be fed end.
    fn input_end(&self, stored_size: u64) -> u64;
    fn step(&mut self, input: &[u8], out: &mut [u8]) -> Result<Step>;
    /// Whether it is a closed unit decoded to its end and compared.
    fn unit_ended(&self) -> bool;
    /// Whether a step can go on with no input held, so it is taken before
    /// any is read: the frontier's, which may hand out a chunk's plaintext.
    fn steps_without_input(&self) -> bool {
        false
    }
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
        let needs_input =
            step.progress == gz_seek::Progress::default() && step.checkpoint.is_none();
        let checkpoint = step.checkpoint.is_some();
        Ok(Step { progress: step.progress, checkpoint, needs_input, waits: false })
    }

    fn unit_ended(&self) -> bool {
        false
    }
}

impl Decode for Frontier {
    fn position(&self) -> u64 {
        self.pass.position()
    }

    fn compressed_position(&self) -> u64 {
        self.pass.compressed_position()
    }

    fn input_end(&self, stored_size: u64) -> u64 {
        stored_size
    }

    /// A step standing waiting for a task has waited for it, or says which
    /// its read is to await, so it needs no input to go on.
    fn step(&mut self, input: &[u8], out: &mut [u8]) -> Result<Step> {
        let (step, waits) = self.read(input, out)?;
        let needs_input = step.needs_input();
        let checkpoint = step.checkpoint.is_some();
        Ok(Step { progress: step.progress, checkpoint, needs_input, waits: waits.is_some() })
    }

    fn unit_ended(&self) -> bool {
        false
    }

    /// A step may hand out a chunk's plaintext, or move past one, with no
    /// compressed input at all, so it is taken before any is read.
    fn steps_without_input(&self) -> bool {
        true
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
    pub(super) fn refill(
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
    #[cfg(any(feature = "http", test))]
    pub(super) fn supply(&mut self, at: u64, bytes: &[u8]) {
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
    pub(super) fn fill(
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
            if held.is_empty() && !reaches_end && !self.decode.steps_without_input() {
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
            if step.waits {
                return Ok(Filled::Waits);
            }
            if step.needs_input {
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
