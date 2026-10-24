//! deflate's canonical Huffman codes: built from code lengths under zlib's
//! validity rules, and decoded by one table lookup for a short code.

/// The longest code deflate allows.
const MAX_BITS: usize = 15;

/// The bits a lookup decodes at once; a longer code is decoded a bit at a
/// time, which only infrequent symbols are given.
const ROOT_BITS: u32 = 10;

/// The most symbols any of deflate's codes has: the literal/length code's
/// 288.
const MAX_SYMBOLS: usize = 288;

/// Which code a table is for: zlib refuses an incomplete code-length code
/// outright, and any other incomplete code but a single code of one bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    CodeLengths,
    Other,
}

/// One code, ready to decode.
pub(crate) struct Huffman {
    /// By the first [`ROOT_BITS`] bits, low bit first: `symbol << 4 | length`
    /// for a code that short, zero where the code is longer or there is none.
    root: [u16; 1 << ROOT_BITS],
    /// How many codes have each length.
    count: [u16; MAX_BITS + 1],
    /// The symbols in canonical order: by length, then by symbol.
    symbols: [u16; MAX_SYMBOLS],
}

impl Huffman {
    /// A code with no symbols: every decode fails.
    pub(crate) fn empty() -> Huffman {
        Huffman { root: [0; 1 << ROOT_BITS], count: [0; MAX_BITS + 1], symbols: [0; MAX_SYMBOLS] }
    }

    /// Rebuild this table from `lengths`, a length a symbol and zero for a
    /// symbol with no code, refusing what zlib's `inflate_table` refuses.
    pub(crate) fn build(&mut self, lengths: &[u8], kind: Kind) -> Result<(), &'static str> {
        debug_assert!(lengths.len() <= MAX_SYMBOLS);
        let mut count = [0u16; MAX_BITS + 1];
        for &len in lengths {
            count[usize::from(len)] += 1;
        }
        count[0] = 0;
        self.count = count;
        self.root = [0; 1 << ROOT_BITS];
        let max = (1..=MAX_BITS).rev().find(|&len| count[len] != 0).unwrap_or(0);
        if max == 0 {
            // zlib accepts a code with no symbols and fails at its first use.
            return Ok(());
        }
        let mut left: i32 = 1;
        for &n in &count[1..] {
            left = (left << 1) - i32::from(n);
            if left < 0 {
                return Err("over-subscribed code");
            }
        }
        if left > 0 && (kind == Kind::CodeLengths || max != 1) {
            return Err("incomplete code");
        }
        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + count[len];
        }
        let mut next = [0u32; MAX_BITS + 1];
        let mut code = 0u32;
        for len in 1..=MAX_BITS {
            code = (code + u32::from(count[len - 1])) << 1;
            next[len] = code;
        }
        for (symbol, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let len = usize::from(len);
            self.symbols[usize::from(offsets[len])] = symbol as u16;
            offsets[len] += 1;
            let code = next[len];
            next[len] += 1;
            if len as u32 <= ROOT_BITS {
                let reversed = reverse(code, len as u32) as usize;
                let entry = (symbol as u16) << 4 | len as u16;
                for slot in (reversed..1 << ROOT_BITS).step_by(1 << len) {
                    self.root[slot] = entry;
                }
            }
        }
        Ok(())
    }

    /// The symbol whose code begins `bits`, low bit first, and the code's
    /// length; `None` where no code begins them.
    #[inline]
    pub(crate) fn decode(&self, bits: u64) -> Option<(u16, u32)> {
        let entry = self.root[(bits & ((1 << ROOT_BITS) - 1)) as usize];
        if entry & 0xf != 0 {
            return Some((entry >> 4, u32::from(entry & 0xf)));
        }
        self.decode_long(bits)
    }

    /// A code longer than [`ROOT_BITS`], or none: canonical decoding a bit at
    /// a time.
    #[cold]
    fn decode_long(&self, bits: u64) -> Option<(u16, u32)> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= ((bits >> (len - 1)) & 1) as i32;
            let count = i32::from(self.count[len]);
            if code - count < first {
                return Some((self.symbols[(index + code - first) as usize], len as u32));
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

/// The low `len` bits of `code`, reversed: a code as deflate stores it.
fn reverse(code: u32, len: u32) -> u32 {
    code.reverse_bits() >> (32 - len)
}
