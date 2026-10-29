//! The compressed bytes a native decode reads: held by the decode, appended
//! by its caller, read at any bit.

/// Zero bytes kept past the held ones, so that an eight-byte load at any held
/// byte stays inside the buffer.
pub(crate) const PAD: usize = 8;

/// A window of the file's bytes, addressed by file offset.
pub(crate) struct Input {
    /// The held bytes, then [`PAD`] zeros.
    bytes: Vec<u8>,
    /// File offset of `bytes[0]`.
    base: u64,
    /// How many of `bytes` are the file's.
    len: usize,
    /// The file's length: the input ends there.
    file_len: u64,
}

impl Input {
    /// No bytes yet, the next to come being the file's byte `base`.
    pub(crate) fn new(base: u64, file_len: u64) -> Input {
        Input::with_capacity(base, file_len, 0)
    }

    /// [`Input::new`], with room for `capacity` bytes before it grows: a
    /// caller that knows about how much it will be fed copies none of it
    /// again as it comes.
    pub(crate) fn with_capacity(base: u64, file_len: u64, capacity: usize) -> Input {
        let mut bytes = Vec::with_capacity(capacity + PAD);
        bytes.resize(PAD, 0);
        Input { bytes, base, len: 0, file_len }
    }

    /// How many bytes it has written, padding included: what it holds
    /// resident, room it never wrote aside.
    #[cfg(test)]
    pub(crate) fn written(&self) -> usize {
        self.bytes.len()
    }

    /// File offset of the next byte the input wants.
    pub(crate) fn end(&self) -> u64 {
        self.base + self.len as u64
    }

    /// The file's length, where the input ends.
    pub(crate) fn file_len(&self) -> u64 {
        self.file_len
    }

    /// The bit past the last held one.
    pub(crate) fn end_bit(&self) -> u64 {
        8 * self.end()
    }

    /// Whether the input holds the file to its end.
    pub(crate) fn is_complete(&self) -> bool {
        self.end() >= self.file_len
    }

    /// Append `bytes`, which begin at [`Input::end`]; anything past the file's
    /// end is not the file's and is dropped.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        let room = usize::try_from(self.file_len.saturating_sub(self.end())).unwrap_or(usize::MAX);
        let bytes = &bytes[..bytes.len().min(room)];
        self.bytes.truncate(self.len);
        // The padding too, so the bytes are not moved twice to make room.
        self.bytes.reserve(bytes.len() + PAD);
        self.bytes.extend_from_slice(bytes);
        self.len += bytes.len();
        self.bytes.resize(self.len + PAD, 0);
    }

    /// Stop holding the bytes before file offset `offset`, once enough have
    /// gone by that moving the rest is worth it.
    pub(crate) fn release_before(&mut self, offset: u64) {
        let drop = usize::try_from(offset.saturating_sub(self.base)).unwrap_or(usize::MAX);
        let drop = drop.min(self.len);
        if drop < 1 << 20 || drop < self.len / 2 {
            return;
        }
        self.bytes.drain(..drop);
        self.base += drop as u64;
        self.len -= drop;
    }

    /// Stop holding the bytes before file offset `offset`, whatever is left.
    pub(crate) fn release_all_before(&mut self, offset: u64) {
        let drop = usize::try_from(offset.saturating_sub(self.base)).unwrap_or(usize::MAX);
        let drop = drop.min(self.len);
        if drop > 0 {
            self.bytes.drain(..drop);
            self.base += drop as u64;
            self.len -= drop;
        }
    }

    /// The held byte at file offset `offset`, if held.
    pub(crate) fn byte(&self, offset: u64) -> Option<u8> {
        let i = usize::try_from(offset.checked_sub(self.base)?).ok()?;
        (i < self.len).then(|| self.bytes[i])
    }

    /// The held bytes from file offset `from` on.
    pub(crate) fn from(&self, from: u64) -> &[u8] {
        let i = usize::try_from(from.saturating_sub(self.base)).unwrap_or(usize::MAX).min(self.len);
        debug_assert!(from >= self.base, "a read before the held bytes");
        &self.bytes[i..self.len]
    }

    /// The eight bytes from file offset `offset` on, little-endian: zeros
    /// past the held bytes.
    #[inline(always)]
    pub(crate) fn word(&self, offset: u64) -> u64 {
        debug_assert!(offset >= self.base, "a read before the held bytes");
        let i = (offset - self.base) as usize;
        match self.bytes.get(i..i + 8) {
            Some(word) => u64::from_le_bytes(word.try_into().expect("eight bytes")),
            None => self.word_past(i),
        }
    }

    /// [`Input::word`] reaching past the padding.
    #[cold]
    fn word_past(&self, i: usize) -> u64 {
        let mut word = [0u8; 8];
        let tail = self.bytes.get(i..).unwrap_or(&[]);
        word[..tail.len()].copy_from_slice(tail);
        u64::from_le_bytes(word)
    }

    /// The 57 or more bits from `bit` on, in their low bits: zeros past the
    /// held bytes.
    #[inline]
    pub(crate) fn peek(&self, bit: u64) -> u64 {
        debug_assert!(bit / 8 >= self.base, "a read before the held bytes");
        let i = (bit / 8 - self.base) as usize;
        let word = match self.bytes.get(i..i + 8) {
            Some(word) => u64::from_le_bytes(word.try_into().expect("eight bytes")),
            None => {
                let mut word = [0u8; 8];
                let tail = self.bytes.get(i..).unwrap_or(&[]);
                word[..tail.len()].copy_from_slice(tail);
                u64::from_le_bytes(word)
            }
        };
        word >> (bit % 8)
    }
}

/// A bit reader over an [`Input`] for what is read rarely and in pieces — a
/// block's header — which reads zeros past the held bytes and says whether it
/// went there.
pub(crate) struct Reader<'a> {
    input: &'a Input,
    pub(crate) bit: u64,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(input: &'a Input, bit: u64) -> Reader<'a> {
        Reader { input, bit }
    }

    /// The next `n` bits, at most 32.
    pub(crate) fn bits(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        let value = (self.input.peek(self.bit) & ((1u64 << n) - 1)) as u32;
        self.bit += u64::from(n);
        value
    }

    /// Skip to the next byte boundary.
    pub(crate) fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }

    /// The bits from here, for a table lookup; nothing consumed.
    pub(crate) fn peek(&self) -> u64 {
        self.input.peek(self.bit)
    }

    /// Whether the reader has gone past the held bytes: whatever it read there
    /// was zeros standing in for bytes not yet supplied.
    pub(crate) fn overran(&self) -> bool {
        self.bit > self.input.end_bit()
    }
}
