//! deflate's canonical Huffman codes: built from code lengths under zlib's
//! validity rules, and decoded by one table lookup for a short code, whose
//! entry carries what a length or distance symbol stands for.

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

/// Each length symbol's base length and extra bits, from 257.
pub(super) const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
pub(super) const LENGTH_EXTRA: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];

/// Each distance symbol's base distance and extra bits.
pub(super) const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub(super) const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// What a code's symbols stand for, which a decoded entry carries beside the
/// symbol so a decode reads a length's or a distance's base and extra bits in
/// the same load as its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Values {
    /// The symbols themselves.
    Symbols,
    /// The literal/length code's: a length symbol's base length.
    Lengths,
    /// The distance code's: a symbol's base distance.
    Distances,
}

impl Values {
    /// The entry for `symbol`, its code `len` bits long:
    /// `base << 17 | symbol << 8 | extra << 4 | len`, a symbol with no base
    /// carrying none.
    fn entry(self, symbol: u16, len: u32) -> u32 {
        let i = usize::from(symbol);
        let (base, extra) = match self {
            Values::Lengths if (257..257 + LENGTH_BASE.len()).contains(&i) => {
                (LENGTH_BASE[i - 257], LENGTH_EXTRA[i - 257])
            }
            Values::Distances if i < DISTANCE_BASE.len() => (DISTANCE_BASE[i], DISTANCE_EXTRA[i]),
            _ => (0, 0),
        };
        u32::from(base) << 17 | u32::from(symbol) << 8 | u32::from(extra) << 4 | len
    }
}

/// A decoded entry's parts.
#[inline(always)]
pub(crate) fn entry_len(entry: u32) -> u32 {
    entry & 0xf
}

#[inline(always)]
pub(crate) fn entry_extra(entry: u32) -> u32 {
    (entry >> 4) & 0xf
}

#[inline(always)]
pub(crate) fn entry_symbol(entry: u32) -> u16 {
    ((entry >> 8) & 0x1ff) as u16
}

#[inline(always)]
pub(crate) fn entry_base(entry: u32) -> usize {
    (entry >> 17) as usize
}

/// One code, ready to decode.
pub(crate) struct Huffman {
    /// By the first [`ROOT_BITS`] bits, low bit first: the entry
    /// ([`Values::entry`]) for a code that short, zero where the code is
    /// longer or there is none.
    root: [u32; 1 << ROOT_BITS],
    values: Values,
    /// How many codes have each length.
    count: [u16; MAX_BITS + 1],
    /// The symbols in canonical order: by length, then by symbol.
    symbols: [u16; MAX_SYMBOLS],
}

impl Huffman {
    /// A code with no symbols: every decode fails.
    pub(crate) fn empty() -> Huffman {
        Huffman {
            root: [0; 1 << ROOT_BITS],
            values: Values::Symbols,
            count: [0; MAX_BITS + 1],
            symbols: [0; MAX_SYMBOLS],
        }
    }

    /// Rebuild this table from `lengths`, a length a symbol and zero for a
    /// symbol with no code, refusing what zlib's `inflate_table` refuses.
    pub(crate) fn build(&mut self, lengths: &[u8], kind: Kind) -> Result<(), &'static str> {
        self.build_for(lengths, kind, Values::Symbols)
    }

    /// [`Huffman::build`], each entry carrying what its symbol stands for in
    /// `values`.
    pub(crate) fn build_for(
        &mut self,
        lengths: &[u8],
        kind: Kind,
        values: Values,
    ) -> Result<(), &'static str> {
        self.values = values;
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
            // zlib accepts a code with no symbols and fails at its first use;
            // a code-length code of none is refused at once here, where zlib
            // reads it as lengths of zero and refuses the missing end-of-block.
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
                let entry = values.entry(symbol as u16, len as u32);
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
        self.entry(bits).map(|entry| (entry_symbol(entry), entry_len(entry)))
    }

    /// The entry ([`Values::entry`]) of the code beginning `bits`, low bit
    /// first; `None` where no code begins them.
    #[inline(always)]
    pub(crate) fn entry(&self, bits: u64) -> Option<u32> {
        let entry = self.root[(bits & ((1 << ROOT_BITS) - 1)) as usize];
        if entry & 0xf != 0 {
            return Some(entry);
        }
        self.decode_long(bits).map(|(symbol, len)| self.values.entry(symbol, len))
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
