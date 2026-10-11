//! `boolean`: its reader, its renderer and why `boolin` refuses a text.

use crate::decode::Unread;
use crate::value::libc::is_c_space;

/// `t`/`f`, COPY TEXT's boolean spelling.
pub fn decode_bool(s: &str) -> Option<bool> {
    match s {
        "t" => Some(true),
        "f" => Some(false),
        _ => None,
    }
}

/// Why [`decode_bool`] read no value from `s`: `boolin` refuses it, or reads
/// a spelling `boolout` never writes — `true`, `yes`, `on`, `1` or a
/// negation, given by any prefix naming the word alone, in either case, with
/// blanks around it (`docs/design/decisions.md`, "D55").
// pg-refuses: I65 — every refusal here is `boolin`'s.
pub(crate) fn bool_unread(s: &str) -> Unread {
    let word = s.trim_matches(|c: char| c.is_ascii() && is_c_space(c as u8));
    // `parse_bool_with_len`: the word is a prefix of the one its first
    // letter opens, and `o` alone opens both `on` and `off`.
    let prefix_of =
        |full: &str| word.len() <= full.len() && word.eq_ignore_ascii_case(&full[..word.len()]);
    let reads = match word.as_bytes().first() {
        Some(b't' | b'T') => prefix_of("true"),
        Some(b'f' | b'F') => prefix_of("false"),
        Some(b'y' | b'Y') => prefix_of("yes"),
        Some(b'n' | b'N') => prefix_of("no"),
        Some(b'o' | b'O') => word.len() >= 2 && (prefix_of("on") || prefix_of("off")),
        Some(b'0' | b'1') => word.len() == 1,
        _ => false,
    };
    if reads { Unread::Unparsed } else { Unread::Refused }
}

pub fn render_bool(v: bool) -> &'static str {
    if v { "t" } else { "f" }
}

#[cfg(test)]
mod tests;
