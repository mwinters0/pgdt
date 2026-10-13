//! The decimal digit writers every renderer appends a number through, off
//! one table of digit pairs, and hex digits both ways: the nibble table the
//! hex readers — `bytea`, `uuid`, the network types — read a digit by, and
//! the hex pairs `bytea` and `uuid` are written with. Private to the families
//! but for [`push_integer`], which a crate above calls, so `internal`
//! re-exports it.

/// The 100 two-digit decimal pairs end to end, so `v`'s pair is the two bytes
/// at `v * 2` — [`HEX_PAIRS`]'s decimal counterpart. Every
/// zero-padded field a `date`, a `time` or a `timestamp` is written from is
/// pairs — two digits, a four-digit year two of them, a fraction three — so a
/// pair becomes an indexed slice and a two-byte copy rather than a trip
/// through `core::fmt`, a year past four digits alone taking the general path
/// (`docs/design/decisions.md`, "D44").
const DEC_PAIRS_BYTES: [u8; 200] = {
    let digits = *b"0123456789";
    let mut table = [0u8; 200];
    let mut v = 0usize;
    while v < 100 {
        table[v * 2] = digits[v / 10];
        table[v * 2 + 1] = digits[v % 10];
        v += 1;
    }
    table
};

/// The same table as text, converted once at compile time, so a renderer
/// appends with `push_str` and pays no UTF-8 validation — the same shape as
/// [`HEX_PAIRS`].
static DEC_PAIRS: &str = match str::from_utf8(&DEC_PAIRS_BYTES) {
    Ok(s) => s,
    Err(_) => panic!("decimal digits are ASCII"),
};

/// The ten decimal digits, so a leading odd digit is appended as a `&str`
/// slice like every other digit and the whole function stays free of
/// byte-to-`str` conversion.
static DEC_DIGITS: &str = "0123456789";

/// `format!("{value:0width$}")` for an `i64`, digit for digit: sign-aware zero
/// padding, so a `-` leads and the padding zeros follow it, and no padding at
/// all once the digits are already that wide. `width` of `0` is therefore
/// exactly `i64::to_string`, which is what [`push_integer`] is.
///
/// Three properties are load-bearing rather than stylistic
/// (`docs/design/decisions.md`, "D44"):
///
/// - **The digits come out two at a time**, off [`DEC_PAIRS`], the algorithm
///   the standard library's own integer `Display` uses.
/// - **Every digit is a slice of a `&'static str`**, the sign and padding
///   pushed as ASCII `char`s, so nothing here converts bytes to text and no
///   UTF-8 validation is paid per field.
/// - **The whole length is reserved once**, so no `push_str` grows the
///   buffer.
#[inline]
fn push_padded(out: &mut String, value: i64, width: usize) {
    // Pair values, least significant first; a `u64` has at most nine of them
    // ahead of its leading one or two digits.
    let mut pairs = [0u8; 10];
    let mut count = 0;
    let mut magnitude = value.unsigned_abs();
    while magnitude >= 100 {
        pairs[count] = (magnitude % 100) as u8;
        count += 1;
        magnitude /= 100;
    }
    let lead = magnitude as usize;
    let negative = value < 0;
    let digits = 2 * count + if lead >= 10 { 2 } else { 1 };
    out.reserve(width.max(digits + usize::from(negative)));
    if negative {
        out.push('-');
    }
    for _ in (digits + usize::from(negative))..width {
        out.push('0');
    }
    if lead >= 10 {
        out.push_str(&DEC_PAIRS[lead * 2..lead * 2 + 2]);
    } else {
        out.push_str(&DEC_DIGITS[lead..lead + 1]);
    }
    for pair in pairs[..count].iter().rev() {
        let at = usize::from(*pair) * 2;
        out.push_str(&DEC_PAIRS[at..at + 2]);
    }
}

/// An integer field's digits, appended: the same text `i64::to_string`
/// produces, without the `String` it allocates.
///
/// `write!(out, "{value}")` reaches the same `Display` impl through
/// `core::fmt::write`, which this path never uses
/// (`docs/design/decisions.md`, "D44").
#[inline]
pub fn push_integer(out: &mut String, value: i64) {
    push_padded(out, value, 0);
}

/// `format!("{value:02}")` — one table lookup for the two-digit range every
/// calendar and clock field of a well-formed value lives in.
pub(super) fn push_two(out: &mut String, value: i64) {
    if (0..100).contains(&value) {
        let at = value as usize * 2;
        out.push_str(&DEC_PAIRS[at..at + 2]);
    } else {
        push_padded(out, value, 2);
    }
}

/// `format!("{year:04}")`, as two pairs for the four-digit years and the
/// general path for anything wider — `Date32`'s day range reaches years either
/// side of five million.
pub(super) fn push_year(out: &mut String, year: i64) {
    if (0..10_000).contains(&year) {
        push_two(out, year / 100);
        push_two(out, year % 100);
    } else {
        push_padded(out, year, 4);
    }
}

/// The fractional-seconds digits, `micros` in `1..1_000_000`: six digits with
/// trailing zeros trimmed, which is what PostgreSQL writes (`00:00:00.5`, not
/// `.500000`). The trim is a `truncate` back to the last non-zero digit,
/// bounded by `start` so it can never reach text the caller had already
/// written.
pub(super) fn push_fraction(out: &mut String, micros: i64) {
    let start = out.len();
    push_two(out, micros / 10_000);
    push_two(out, (micros / 100) % 100);
    push_two(out, micros % 100);
    let mut end = out.len();
    {
        let bytes = out.as_bytes();
        while end > start && bytes[end - 1] == b'0' {
            end -= 1;
        }
    }
    out.truncate(end);
}

/// Nibble value per byte, `BAD_NIBBLE` for anything that is not a hex digit.
/// The hex decoders reading it loop per *byte* over a field of any length,
/// so they read a table rather than branching through
/// [`pgdt_core::internal::hex_val`]'s three ranges: a pair becomes two loads, a
/// shift and an or, and validity is one bit test on the accumulated `or`.
pub(crate) const HEX_NIBBLE: [u8; 256] = {
    let mut table = [BAD_NIBBLE; 256];
    let mut b = 0usize;
    while b < 256 {
        table[b] = match b as u8 {
            d @ b'0'..=b'9' => d - b'0',
            d @ b'a'..=b'f' => d - b'a' + 10,
            d @ b'A'..=b'F' => d - b'A' + 10,
            _ => BAD_NIBBLE,
        };
        b += 1;
    }
    table
};

/// Out of a nibble's range, and out of the low four bits, so `hi | lo` of a
/// pair carries "either was bad" in its high nibble.
const BAD_NIBBLE: u8 = 0xFF;

/// The 256 lowercase hex pairs end to end, so byte `b`'s pair is the two bytes
/// at `b * 2` — [`HEX_NIBBLE`]'s counterpart in
/// the render direction. `render_bytea` and `render_uuid` loop per *byte* over a value
/// of any length, and each knows its whole output length before it starts, so a
/// byte becomes an indexed slice and a two-byte copy into a pre-sized `String`
/// rather than a trip through `core::fmt` (`docs/design/decisions.md`, "D44").
const HEX_PAIRS_BYTES: [u8; 512] = {
    let digits = *b"0123456789abcdef";
    let mut table = [0u8; 512];
    let mut b = 0usize;
    while b < 256 {
        table[b * 2] = digits[b >> 4];
        table[b * 2 + 1] = digits[b & 0x0F];
        b += 1;
    }
    table
};

/// The same table as text, so a renderer appends a pair with `push_str` and
/// pays no UTF-8 validation; the conversion happens once, at compile time.
static HEX_PAIRS: &str = match str::from_utf8(&HEX_PAIRS_BYTES) {
    Ok(s) => s,
    Err(_) => panic!("hex digits are ASCII"),
};

pub(crate) fn push_hex_pair(out: &mut String, byte: u8) {
    let at = byte as usize * 2;
    out.push_str(&HEX_PAIRS[at..at + 2]);
}
