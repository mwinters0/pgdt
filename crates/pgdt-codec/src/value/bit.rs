//! Whether `bit_in` or `varbit_in` reads a text under a column's typmod. Not
//! `decode`'s API; a strict parse and a filter's literal call it.

/// `VARBITMAXLEN`, the most bits a `bit` or `bit varying` value holds, which
/// bounds a hex spelling's digits at a quarter of it (I73).
const VARBIT_MAX_BITS: usize = i32::MAX as usize - 7;

/// Whether `bit_in`, or `varbit_in` where `varying`, reads `text` as a field
/// of a column whose typmod is `length`: binary digits, or after a leading `x`
/// or `X` hex digits of four bits each, a leading `b` or `B` optional before
/// binary ones; then exactly `length` bits for `bit(n)` and at most `length`
/// for `bit varying(n)`, any number for a column with no typmod. Nothing is
/// trimmed (I73).
// pg-refuses: I73 — every refusal here is `bit_in`'s or `varbit_in`'s.
pub(crate) fn bit_in(text: &str, varying: bool, length: Option<u32>) -> bool {
    let (hex, digits) = match text.as_bytes() {
        [b'x' | b'X', rest @ ..] => (true, rest),
        [b'b' | b'B', rest @ ..] => (false, rest),
        all => (false, all),
    };
    if hex && digits.len() > VARBIT_MAX_BITS / 4 {
        return false;
    }
    let bits = if hex { digits.len() * 4 } else { digits.len() };
    let fits = match length.map(|n| n as usize) {
        None => true,
        Some(n) if varying => bits <= n,
        Some(n) => bits == n,
    };
    fits && if hex {
        digits.iter().all(u8::is_ascii_hexdigit)
    } else {
        digits.iter().all(|&d| matches!(d, b'0' | b'1'))
    }
}

#[cfg(test)]
mod tests;
