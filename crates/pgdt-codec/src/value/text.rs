//! The character types: a field against its typmod's length.

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

#[cfg(test)]
mod tests;
