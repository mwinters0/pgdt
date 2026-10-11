//! Whether PostgreSQL refuses a field, not what it reads as: the checks a
//! strict parse and a filter's literal make beside the readers, and the
//! storage format's bounds they check against. Not `decode`'s API, so
//! `internal` re-exports them.

use super::Unread;

/// Why [`decode_bytea`](super::decode_bytea) read no value from `s`: `byteain`
/// refuses it, or reads a spelling this build does not — a blank between hex
/// pairs, any byte the `escape` form writes escaped written as itself, or the
/// reverse (`docs/design/decisions.md`, "D55").
// pg-refuses: I69 — every refusal here is `byteain`'s.
pub fn bytea_unread(s: &str) -> Unread {
    let b = s.as_bytes();
    let reads = match b.strip_prefix(b"\\x") {
        // `hex_decode_safe`: a blank between pairs, never inside one.
        Some(hex) => {
            let mut digits = hex.iter().filter(|&&c| !matches!(c, b' ' | b'\n' | b'\t' | b'\r'));
            let mut pairs = hex.split(|&c| matches!(c, b' ' | b'\n' | b'\t' | b'\r'));
            digits.all(u8::is_ascii_hexdigit) && pairs.all(|run| run.len() % 2 == 0)
        }
        None => {
            let mut at = 0;
            loop {
                match b.get(at..) {
                    Some([]) | None => break true,
                    Some([b'\\', b'0'..=b'3', b'0'..=b'7', b'0'..=b'7', ..]) => at += 4,
                    Some([b'\\', b'\\', ..]) => at += 2,
                    Some([b'\\', ..]) => break false,
                    Some(_) => at += 1,
                }
            }
        }
    };
    if reads { Unread::Unparsed } else { Unread::Refused }
}

/// Whether a `character varying(n)` or `character(n)` field, `n` being
/// `length`, is one `varchar_input` or `bpchar_input` refuses: longer than
/// `length` characters, with anything but a blank, `0x20`, past them (I75).
/// The characters are counted in UTF-8, the one encoding read here; a field
/// no longer in bytes than `length` is read without counting, and a field it
/// does not refuse is read as written ([`crate::compare::CompareKind::Text`]).
// deficiency: KD96 — a field longer than `length` only by trailing blanks,
// which `varchar_input` and `bpchar_input` cut to `length` characters (I75),
// keeps them here, and a `character(n)` field shorter than `length`, which
// `bpchar_input` pads, is not padded. Either way a typed read emits a value
// the server does not hold; only a `character varying(n)`'s longer field also
// answers otherwise, keyed, bounded and compared with its blanks, so `<` and
// `=` differ from the server over it, where `character(n)`'s comparison trims
// both sides. No `pg_dump` writes either. The fix cuts the field in the read,
// the key and the bounds, and records per group whether any field was cut: a
// `SchemaMode::Strings` term reads the column as its text from the same set
// (D79), so it reads only a group where none was, never losing a real dump's
// bounds; `=`, `!=` and `IN` answer by the cut value only once `KD82` closes.
// `numeric(p,s)` is rounded where this is not because its rounding decides
// its refusal (I51), and no refusal here needs the cut. **(c) unowned**;
// promoted by a hand-written dump holding one.
// pg-refuses: I75 — a non-blank past the typmod's length.
pub fn char_typmod_refuses(text: &str, length: u32) -> bool {
    let length = length as usize;
    if text.len() <= length {
        return false;
    }
    text.char_indices().nth(length).is_some_and(|(cut, _)| text[cut..].bytes().any(|b| b != b' '))
}

/// `NUMERIC_DSCALE_MAX`: the most digits `numeric_in` stores after the point,
/// a trailing zero counting (I63).
pub const NUMERIC_DSCALE_MAX: usize = 16383;

/// The most digits `numeric_in` stores before the point, a leading zero not
/// counting: `NUMERIC_WEIGHT_MAX`, `i16::MAX`, base-10000 digits past the
/// first (I63).
pub const NUMERIC_INTEGER_DIGITS_MAX: usize = 4 * (i16::MAX as usize + 1);

/// Whether `numeric_in` with no typmod stores the number written
/// `[-]int[.frac]` rather than refusing it as overflowing the format (I63):
/// every digit after the point counts toward its display scale, and its
/// weight counts from the first non-zero digit before it. Only the two bounds
/// are checked; the grammar is its reader's.
///
/// What it bounds is a bare `numeric`'s field and every literal, which the
/// server coerces with no typmod. A typmod'd column's field is rounded to its
/// scale before the bounds are checked, and never reaches them (I51).
// pg-refuses: I63 — a display scale or a weight past the storage format's.
pub fn numeric_in_stores(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    frac.len() <= NUMERIC_DSCALE_MAX
        && (int.len() <= NUMERIC_INTEGER_DIGITS_MAX
            || int.trim_start_matches('0').len() <= NUMERIC_INTEGER_DIGITS_MAX)
}
