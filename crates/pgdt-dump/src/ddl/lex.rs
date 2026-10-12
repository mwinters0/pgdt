//! The statement grammar's own lexing: keywords matched at a word boundary,
//! `'…'` literals and `"…"` identifiers stepped over, parens matched, a list
//! split at its top-level commas, a qualified name read. Not the SQL lexer
//! ([`crate::lex`]), which tells code from a literal across a line; these
//! read one complete statement the map builder already holds.

use crate::lex::ident_cont;
use pgdt_core::internal::Cursor;

/// Case-insensitive substring search, since none of `str`'s own methods do
/// one and the DDL grammar below only ever cares about a handful of fixed
/// ASCII keywords.
pub(super) fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let pat = needle.as_bytes();
    if pat.is_empty() || hay.len() < pat.len() {
        return None;
    }
    (0..=hay.len() - pat.len()).find(|&i| hay[i..i + pat.len()].eq_ignore_ascii_case(pat))
}

/// Case-insensitively strip a leading keyword ending at a word boundary,
/// returning the (whitespace stripped) remainder: the byte after it may not
/// continue an identifier, so `CREATE TABLE` is no prefix of
/// `CREATE TABLESPACE`.
pub(super) fn strip_kw<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    if bytes.len() < kw.len()
        || !bytes[..kw.len()].eq_ignore_ascii_case(kw.as_bytes())
        || bytes.get(kw.len()).is_some_and(|&b| ident_cont(b))
    {
        return None;
    }
    Some(s[kw.len()..].trim_start())
}

/// Parse a (possibly schema-qualified) identifier from the start of `s`,
/// same folding rules as [`pgdt_core::parse_copy_header`]'s table names —
/// so the result matches [`pgdt_core::CopyHeader::qualified_name`] exactly
/// for the same object. Returns the qualified name and how many bytes of `s`
/// it consumed. `pub(crate)` for [`crate::map`]'s `INSERT INTO` target
/// parsing, which needs the same identifier grammar this module already
/// hardened.
pub(crate) fn parse_qualified_name(s: &str) -> Option<(String, usize)> {
    let mut cur = Cursor::new(s.as_bytes());
    cur.skip_spaces();
    let first = cur.parse_ident()?;
    let name = if cur.eat_byte(b'.') {
        let second = cur.parse_ident()?;
        format!("{first}.{second}")
    } else {
        first
    };
    Some((name, cur.pos()))
}

/// Skip over the single-quoted string literal starting at `open_idx`,
/// returning the index just past its closing quote — or `bytes.len()` for an
/// unterminated one, which lets a caller's scan terminate rather than loop.
///
/// One implementation of the quoting rule, `''` being an escaped quote rather
/// than the end of the string, for every scanner below that steps over a
/// literal.
pub(super) fn skip_quoted(bytes: &[u8], open_idx: usize) -> usize {
    debug_assert_eq!(bytes.get(open_idx), Some(&b'\''));
    let mut i = open_idx + 1;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Skip over the double-quoted identifier starting at `open_idx`, returning
/// the index just past its closing quote — `""` being the escape, exactly as
/// `''` is inside a string literal. Unterminated behaves like
/// [`skip_quoted`]'s: the scan ends rather than looping.
pub(super) fn skip_double_quoted(bytes: &[u8], open_idx: usize) -> usize {
    debug_assert_eq!(bytes.get(open_idx), Some(&b'"'));
    let mut i = open_idx + 1;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if bytes.get(i + 1) == Some(&b'"') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Whether byte `i` starts a word — i.e. the byte before it cannot be part of
/// an identifier. Keeps a keyword search from matching inside a longer name.
pub(super) fn is_word_start(bytes: &[u8], i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).and_then(|p| bytes.get(p)) else { return true };
    !(prev.is_ascii_alphanumeric() || *prev == b'_' || *prev == b'$' || *prev >= 0x80)
}

/// Find the index of `bytes[open_idx..]`'s matching `)`, respecting
/// single-quoted strings and double-quoted identifiers, so a literal like
/// `')'` or a name like `s."a)"` (I29) never confuses the depth count.
pub(super) fn matching_paren(bytes: &[u8], open_idx: usize) -> Option<usize> {
    debug_assert_eq!(bytes.get(open_idx), Some(&b'('));
    let mut depth: i32 = 0;
    let mut i = open_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_quoted(bytes, i);
                continue;
            }
            b'"' => {
                i = skip_double_quoted(bytes, i);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split `s` on top-level commas — not ones nested inside parens, an
/// `ARRAY[…]` constructor's or a subscript's brackets, a single-quoted string
/// or a double-quoted identifier — trimming and dropping empty fragments.
pub(super) fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_quoted(bytes, i);
                continue;
            }
            b'"' => {
                i = skip_double_quoted(bytes, i);
                continue;
            }
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

/// Parse a single SQL string literal (`'...'`, `''` as an escaped quote)
/// starting at `s`'s first non-whitespace byte, ending at its closing quote;
/// whatever follows that is not read.
pub(super) fn parse_string_literal(s: &str) -> Option<String> {
    let mut chars = s.trim().chars().peekable();
    if chars.next() != Some('\'') {
        return None;
    }
    let mut out = String::new();
    loop {
        let c = chars.next()?;
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            return Some(out);
        }
        out.push(c);
    }
}

/// Strip `/* ... */` block comments — the literal shape `pg_dump` writes a
/// `--binary-upgrade`-recreated dropped column's placeholder type in
/// (`INTEGER /* dummy */`, I5; `fixtures/*/edge_cases/binary-upgrade.sql`) —
/// stepping over string literals and quoted identifiers, inside which `/*`
/// is text (I29). Assumes no nesting, which matches every comment `pg_dump`
/// emits.
pub(super) fn strip_block_comments(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut copied, mut i) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => i = skip_quoted(bytes, i),
            b'"' => i = skip_double_quoted(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(&s[copied..i]);
                i = s[i + 2..].find("*/").map_or(bytes.len(), |end| i + 2 + end + 2);
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&s[copied..]);
    out
}

/// The whitespace-separated words of `s`, a quoted identifier being one word
/// with whatever it holds — spaces and all (I29).
pub(super) fn type_tokens(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i = if bytes[i] == b'"' { skip_double_quoted(bytes, i) } else { i + 1 };
        }
        tokens.push(&s[start..i]);
    }
    tokens
}

/// The words of `rest` at its top level — outside parens and `'…'` — as
/// keywords: a quoted identifier is its text, quotes and all, and a word a `.`
/// qualifies an empty word, neither matching a keyword, so no sequence matches
/// across them.
pub(super) fn top_level_words(rest: &str) -> Vec<&str> {
    let bytes = rest.as_bytes();
    let mut words = Vec::new();
    let mut depth: i32 = 0;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_quoted(bytes, i);
                continue;
            }
            b'"' => {
                let start = i;
                i = skip_double_quoted(bytes, i);
                if depth == 0 {
                    words.push(rest.get(start..i).unwrap_or(""));
                }
                continue;
            }
            b'(' => depth += 1,
            b')' => depth -= 1,
            // A number's run, `1e5` say, is read whole and is no word.
            b if ident_cont(b) => {
                let start = i;
                while i < bytes.len() && ident_cont(bytes[i]) {
                    i += 1;
                }
                let keyword = depth == 0
                    && !bytes[start].is_ascii_digit()
                    && bytes[start] != b'$'
                    && (start == 0 || bytes[start - 1] != b'.');
                if depth == 0 {
                    words.push(if keyword { &rest[start..i] } else { "" });
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    words
}

/// `s` with each run of whitespace outside a `'…'` literal or a `"…"`
/// identifier one space, and none at either end.
pub(super) fn collapse_spacing(s: &str) -> String {
    let bytes = s.trim().as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let end = match bytes[i] {
            b'\'' => skip_quoted(bytes, i),
            b'"' => skip_double_quoted(bytes, i),
            b if b.is_ascii_whitespace() => {
                out.push(b' ');
                i += bytes[i..].iter().take_while(|b| b.is_ascii_whitespace()).count();
                continue;
            }
            _ => i + 1,
        };
        out.extend_from_slice(&bytes[i..end]);
        i = end;
    }
    // Cut only beside ASCII bytes, so as whole as `s`.
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `haystack` holds `word`, in any case, bounded on both sides by a
/// byte no identifier continues with.
pub(super) fn holds_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(&haystack[from..], word).map(|i| from + i) {
        let end = at + word.len();
        let before = at.checked_sub(1).map(|i| bytes[i]);
        if !before.is_some_and(ident_cont) && !bytes.get(end).copied().is_some_and(ident_cont) {
            return true;
        }
        from = at + 1;
    }
    false
}
