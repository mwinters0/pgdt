//! Whether `bit_in` or `varbit_in` reads a text under a column's typmod,
//! fed a byte at a time. Not `decode`'s API; a strict parse calls it.

/// `VARBITMAXLEN`, the most bits a `bit` or `bit varying` value holds, which
/// bounds a hex spelling's digits at a quarter of it (I73).
const VARBIT_MAX_BITS: usize = i32::MAX as usize - 7;

/// Whether `bit_in`, or `varbit_in` where `varying`, reads `text` as a field
/// of a column whose typmod is `length`, fed whole to a [`BitWalk`].
#[cfg(test)]
pub(crate) fn bit_in(text: &str, varying: bool, length: Option<u32>) -> bool {
    let mut walk = BitWalk::new(varying, length);
    text.bytes().for_each(|byte| walk.byte(byte));
    walk.end()
}

/// **Whether `bit_in`, or `varbit_in` where `varying`, reads a field of a
/// column whose typmod is `length`**, fed a byte at a time
/// (`docs/design/decisions.md`, "D144"): binary digits, or after a leading
/// `x` or `X` hex digits of four bits each, a leading `b` or `B` optional
/// before binary ones; then exactly `length` bits for `bit(n)` and at most
/// `length` for `bit varying(n)`, any number for a column with no typmod.
/// Nothing is trimmed (I73). It holds the digits' count and whether each was
/// one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BitWalk {
    varying: bool,
    length: Option<u32>,
    /// Whether the digits are hex, once the first byte has said.
    hex: Option<bool>,
    digits: u64,
    valid: bool,
}

impl BitWalk {
    pub(crate) fn new(varying: bool, length: Option<u32>) -> Self {
        Self { varying, length, hex: None, digits: 0, valid: true }
    }

    /// Ready the walk for another field.
    pub(crate) fn start(&mut self) {
        *self = Self::new(self.varying, self.length);
    }

    pub(crate) fn byte(&mut self, byte: u8) {
        let hex = match self.hex {
            Some(hex) => hex,
            None => {
                let hex = matches!(byte, b'x' | b'X');
                self.hex = Some(hex);
                if hex || matches!(byte, b'b' | b'B') {
                    return;
                }
                hex
            }
        };
        self.digits += 1;
        self.valid &= if hex { byte.is_ascii_hexdigit() } else { matches!(byte, b'0' | b'1') };
    }

    /// The field has ended: whether it is read.
    // pg-refuses: I73 — every refusal here is `bit_in`'s or `varbit_in`'s.
    pub(crate) fn end(&self) -> bool {
        let hex = self.hex == Some(true);
        if hex && self.digits > (VARBIT_MAX_BITS / 4) as u64 {
            return false;
        }
        let bits = if hex { self.digits * 4 } else { self.digits };
        let fits = match self.length.map(u64::from) {
            None => true,
            Some(n) if self.varying => bits <= n,
            Some(n) => bits == n,
        };
        fits && self.valid
    }
}

#[cfg(test)]
mod tests;
