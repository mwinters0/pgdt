//! A fixed block's symbols walked without producing a byte: where a decode
//! may resume inside one.
//!
//! A fixed block's codes are deflate's own rather than its header's, so a
//! decode can begin at any of its symbols given a synthetic three-bit header
//! and the history before it. No inflater stops there, so the forward pass
//! walks the block's codes beside the engine — counting the plaintext each
//! symbol stands for and writing none — to find the first symbol at or past
//! a plaintext offset, and caps the engine's output there
//! (`engine.rs`). Nothing it decodes is handed out, so it is not an
//! inflater of the crate's own (`docs/design/decisions.md`, "D117").

use super::huffman::{DISTANCE_BASE, DISTANCE_EXTRA, LENGTH_BASE, LENGTH_EXTRA};
use super::inflate::fixed_codes;
use super::input::Input;

/// Where a walk stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Walked {
    /// The next symbol begins at compressed `bit` and plaintext `plaintext`,
    /// at or past the mark, and is not the block's end.
    Mark { bit: u64, plaintext: u64 },
    /// The block's end-of-block code has been read.
    End,
    /// The next symbol runs past the bytes held.
    Input,
}

/// A walk over one fixed block's symbols, from a symbol boundary on.
pub(crate) struct Walk {
    input: Input,
    /// The compressed bit the next symbol begins at.
    bit: u64,
    /// The plaintext offset the next symbol's bytes begin at.
    plaintext: u64,
}

impl Walk {
    /// A walk from the symbol at compressed `bit` and plaintext `plaintext`,
    /// holding `held`, the file's bytes from the one `bit` is in.
    pub(crate) fn new(bit: u64, plaintext: u64, held: &[u8]) -> Walk {
        let mut input = Input::new(bit / 8, u64::MAX);
        input.push(held);
        Walk { input, bit, plaintext }
    }

    /// The file offset of the next byte the walk wants.
    pub(crate) fn end(&self) -> u64 {
        self.input.end()
    }

    /// Append the file's bytes from [`Walk::end`] on.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.input.push(bytes);
    }

    /// Stop holding the bytes before the one the next symbol begins in.
    pub(crate) fn release(&mut self) {
        self.input.release_all_before(self.bit / 8);
    }

    /// The plaintext offset the next symbol's bytes begin at: every symbol
    /// before it walked.
    pub(crate) fn plaintext(&self) -> u64 {
        self.plaintext
    }

    /// Walk on until the next symbol begins at or past plaintext `mark`, the
    /// block ends, or a symbol runs past the bytes held.
    ///
    /// # Errors
    ///
    /// A symbol a fixed block cannot hold, by zlib's message.
    pub(crate) fn run(&mut self, mark: u64) -> Result<Walked, &'static str> {
        let (literals, distances) = fixed_codes();
        let end = self.input.end_bit();
        loop {
            let at = self.bit;
            // Bits past the held bytes read as zeros: a code decoded from
            // them is whole only where it ends before them, a prefix code
            // being decided by its own bits alone.
            let bits = self.input.peek(at);
            let (symbol, len) = literals.decode(bits).ok_or("invalid literal/length code")?;
            if at + u64::from(len) > end {
                return Ok(Walked::Input);
            }
            if symbol == 256 {
                self.bit = at + u64::from(len);
                return Ok(Walked::End);
            }
            if self.plaintext >= mark {
                return Ok(Walked::Mark { bit: at, plaintext: self.plaintext });
            }
            let (used, produced) = if symbol < 256 {
                (len, 1)
            } else {
                let i = usize::from(symbol - 257);
                if i >= LENGTH_BASE.len() {
                    return Err("invalid literal/length code");
                }
                let mut used = len;
                let extra = u32::from(LENGTH_EXTRA[i]);
                let length = u64::from(LENGTH_BASE[i]) + ((bits >> used) & ((1 << extra) - 1));
                used += extra;
                let (symbol, len) =
                    distances.decode(bits >> used).ok_or("invalid distance code")?;
                if usize::from(symbol) >= DISTANCE_BASE.len() {
                    return Err("invalid distance code");
                }
                used += len + u32::from(DISTANCE_EXTRA[usize::from(symbol)]);
                (used, length)
            };
            if at + u64::from(used) > end {
                return Ok(Walked::Input);
            }
            self.bit = at + u64::from(used);
            self.plaintext += produced;
        }
    }
}
