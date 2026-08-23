//! `COPY` header grammar and COPY TEXT-format field decoding/encoding.
//!
//! This module is pure, synchronous, byte-slice-level code: it knows nothing
//! about files, offsets, or chunking. [`crate::scan`] drives it.
//!
//! [`decode_field`]/[`encode_field`] are the one place in the codebase that
//! converts between a field's on-disk COPY-escaped bytes and its unescaped
//! text — see `docs/design/layering.md`, L1. [`crate::decode`] (L2) never
//! sees escaped bytes at all: it takes `decode_field`'s already-unescaped
//! `&str` output and works purely in "unescaped text vs. Arrow value" terms.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The default COPY TEXT field delimiter. `pg_dump` plain-format output never
/// overrides it, so it is hardcoded rather than configurable.
pub const DELIMITER: u8 = b'\t';

/// The literal that COPY TEXT uses for SQL `NULL`.
const NULL_MARKER: &[u8] = b"\\N";

/// The line that terminates a COPY data block.
pub const TERMINATOR: &[u8] = b"\\.";

/// A parsed `COPY <table> [(<columns>)] FROM stdin;` header line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyHeader {
    /// Schema qualifier, if the header named one.
    pub schema: Option<String>,
    /// Table name, unquoted and case-folded per SQL identifier rules.
    pub table: String,
    /// Column names in the order the data columns appear. Empty when the
    /// header carried no column list (all columns, in table order).
    pub columns: Vec<String>,
}

impl CopyHeader {
    /// `schema.table`, or just `table` when unqualified. Not re-quoted.
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.table),
            None => self.table.clone(),
        }
    }

    /// Whether this header refers to `name`, which may be given qualified
    /// (`schema.table`) or bare (`table`, matching any schema).
    pub fn matches(&self, name: &str) -> bool {
        match name.split_once('.') {
            Some((schema, table)) => self.schema.as_deref() == Some(schema) && self.table == table,
            None => self.table == name,
        }
    }
}

/// Parse a line as a `COPY ... FROM stdin;` header.
///
/// Returns `None` for anything that does not match the grammar exactly —
/// including `COPY ... TO stdout;` and `COPY ... FROM stdin WITH (...)`. A
/// non-match is deliberately *not* an error: a line starting with `COPY ` can
/// legitimately appear inside a dollar-quoted function body, and treating it
/// as ordinary SQL is the safe reading. The consequence is that COPY variants
/// this grammar does not cover are silently invisible rather than
/// misinterpreted as data; see `docs/design/pg-dump-compatibility.md`.
pub fn parse_copy_header(line: &[u8]) -> Option<CopyHeader> {
    let mut p = Cursor::new(line);

    p.skip_spaces();
    p.eat_keyword(b"copy")?;
    p.skip_spaces_required()?;

    // Table name, optionally schema-qualified.
    let first = p.parse_ident()?;
    let (schema, table) =
        if p.eat_byte(b'.') { (Some(first), p.parse_ident()?) } else { (None, first) };

    p.skip_spaces();
    let mut columns = Vec::new();
    if p.eat_byte(b'(') {
        loop {
            p.skip_spaces();
            columns.push(p.parse_ident()?);
            p.skip_spaces();
            if p.eat_byte(b',') {
                continue;
            }
            if p.eat_byte(b')') {
                break;
            }
            return None;
        }
        p.skip_spaces();
    }

    p.eat_keyword(b"from")?;
    p.skip_spaces_required()?;
    p.eat_keyword(b"stdin")?;
    p.skip_spaces();
    if !p.eat_byte(b';') {
        return None;
    }
    p.skip_spaces();
    if !p.at_end() {
        return None;
    }

    Some(CopyHeader { schema, table, columns })
}

/// Whether `line` is the `\.` block terminator.
pub fn is_terminator(line: &[u8]) -> bool {
    line == TERMINATOR
}

/// Find the next `$tag$` dollar-quote delimiter at or after `from` in `line`.
/// `tag` is empty for `$$`, or a PostgreSQL identifier otherwise
/// (`[A-Za-z_][A-Za-z0-9_]*`). Returns the delimiter's byte range, both `$`
/// signs included.
fn find_dollar_delimiter(line: &[u8], from: usize) -> Option<std::ops::Range<usize>> {
    let mut i = from;
    while let Some(rel) = memchr::memchr(b'$', &line[i..]) {
        let start = i + rel;
        let mut end = start + 1;
        match line.get(end) {
            Some(b'$') => return Some(start..end + 1),
            Some(&c) if c.is_ascii_alphabetic() || c == b'_' => {
                end += 1;
                while matches!(line.get(end), Some(&c) if c.is_ascii_alphanumeric() || c == b'_') {
                    end += 1;
                }
                if line.get(end) == Some(&b'$') {
                    return Some(start..end + 1);
                }
            }
            _ => {}
        }
        // Not a valid delimiter starting at `start`; retry from the next `$`.
        i = start + 1;
    }
    None
}

/// Advance dollar-quote tracking across one line.
///
/// PostgreSQL dollar-quoting (`$tag$ ... $tag$`, tag optional) is how
/// `pg_dump` emits function/procedure bodies verbatim, and a line inside one
/// can coincidentally match the `COPY` header grammar — see
/// `docs/design/roadmap-phase2-typed-columns.md`, "Dollar-quote tracking
/// closes the Phase 1 known gap". The scanner must never structurally
/// interpret a line while inside a dollar-quoted string.
///
/// `tag` is the delimiter currently open, if any — `None` outside any
/// dollar-quoted string. Returns the tag to carry into the next line, and
/// whether this line touched a delimiter at all: a line that did is quoting
/// syntax or body text, never structure, even past the point a tag closes —
/// `pg_dump` always closes a body on a line of its own, so nothing meaningful
/// follows a closing delimiter on the same line in practice.
pub fn scan_dollar_quotes(line: &[u8], mut tag: Option<Vec<u8>>) -> (Option<Vec<u8>>, bool) {
    let mut pos = 0;
    let mut touched = false;
    loop {
        match tag {
            Some(open) => {
                touched = true;
                match memchr::memmem::find(&line[pos..], &open) {
                    Some(rel) => {
                        pos += rel + open.len();
                        tag = None;
                    }
                    None => return (Some(open), touched),
                }
            }
            None => match find_dollar_delimiter(line, pos) {
                Some(range) => {
                    touched = true;
                    pos = range.end;
                    tag = Some(line[range].to_vec());
                }
                None => return (None, touched),
            },
        }
    }
}

/// Split a raw COPY TEXT data line into its still-escaped field slices.
///
/// Splitting on raw delimiter bytes is correct: COPY TEXT output always
/// escapes an in-value tab as the two bytes `\` `t`, so a bare `0x09` byte is
/// unambiguously a field separator.
pub fn split_fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|&b| b == DELIMITER)
}

/// Decode one still-escaped COPY TEXT field.
///
/// Returns `None` for SQL `NULL` (the field `\N`). The borrowed variant is
/// returned whenever the field contains no escape sequences, which is the
/// common case.
pub fn decode_field(field: &[u8]) -> Result<Option<Cow<'_, str>>> {
    if field == NULL_MARKER {
        return Ok(None);
    }
    if !field.contains(&b'\\') {
        return Ok(Some(Cow::Borrowed(as_utf8(field)?)));
    }

    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        let b = field[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&esc) = field.get(i) else {
            // Trailing lone backslash: PostgreSQL's COPY would reject the
            // line, but emitting it literally loses less than failing.
            out.push(b'\\');
            break;
        };
        i += 1;
        match esc {
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            // Lowercase only, matching PostgreSQL: `\X` falls through to the
            // "stands for itself" rule below.
            b'x' => {
                let mut val: u32 = 0;
                let mut digits = 0;
                while digits < 2 {
                    match field.get(i).and_then(|&b| hex_val(b)) {
                        Some(d) => {
                            val = val * 16 + d;
                            i += 1;
                            digits += 1;
                        }
                        None => break,
                    }
                }
                if digits == 0 {
                    // `\x` with no hex digits is just a literal `x`.
                    out.push(esc);
                } else {
                    out.push(val as u8);
                }
            }
            b'0'..=b'7' => {
                let mut val: u32 = u32::from(esc - b'0');
                let mut digits = 1;
                while digits < 3 {
                    match field.get(i).filter(|&&b| (b'0'..=b'7').contains(&b)) {
                        Some(&b) => {
                            val = val * 8 + u32::from(b - b'0');
                            i += 1;
                            digits += 1;
                        }
                        None => break,
                    }
                }
                out.push(val as u8);
            }
            // Includes `\\`, and PostgreSQL's permissive "any other character
            // stands for itself" rule.
            other => out.push(other),
        }
    }

    match String::from_utf8(out) {
        Ok(s) => Ok(Some(Cow::Owned(s))),
        Err(e) => Err(Error::InvalidUtf8 { valid_up_to: e.utf8_error().valid_up_to() }),
    }
}

/// Re-apply COPY TEXT escaping to already-unescaped text — the exact inverse
/// of [`decode_field`], restricted to the escapes `pg_dump`'s `COPY TO` ever
/// emits: a doubled backslash, the six control-character mnemonics `\b \f \n
/// \r \t \v`, and a backslashed delimiter (postgres-invariants.md I15). The
/// octal/hex forms `decode_field` accepts on input are a `COPY FROM` reader
/// convenience only; `COPY TO` never produces them, so `encode_field` doesn't
/// need to reproduce them for a round trip against real `pg_dump` output to
/// hold — see `copy_text_escaping_round_trips_through_postgres` in
/// `tests/scan.rs`. `None` encodes as the `\N` null marker.
pub fn encode_field(field: Option<&str>) -> Vec<u8> {
    let Some(s) = field else {
        return NULL_MARKER.to_vec();
    };
    let mut out = Vec::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            0x08 => out.extend_from_slice(b"\\b"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x0b => out.extend_from_slice(b"\\v"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            _ => out.push(b),
        }
    }
    out
}

fn as_utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|e| Error::InvalidUtf8 { valid_up_to: e.valid_up_to() })
}

pub(crate) fn hex_val(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some(u32::from(b - b'0')),
        b'a'..=b'f' => Some(u32::from(b - b'a') + 10),
        b'A'..=b'F' => Some(u32::from(b - b'A') + 10),
        _ => None,
    }
}

/// Minimal byte cursor for the `COPY` header grammar. Also reused by
/// `crate::preamble` for the preamble's DDL grammars (`CREATE TABLE` /
/// `TYPE` / `DOMAIN` / `EXTENSION`), which share the same identifier and
/// keyword rules.
pub(crate) struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    pub(crate) fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    /// Whitespace that the grammar requires to be present (keyword separator).
    fn skip_spaces_required(&mut self) -> Option<()> {
        let before = self.pos;
        self.skip_spaces();
        (self.pos > before).then_some(())
    }

    pub(crate) fn eat_byte(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Match an ASCII-lowercase keyword case-insensitively.
    fn eat_keyword(&mut self, kw: &[u8]) -> Option<()> {
        let end = self.pos.checked_add(kw.len())?;
        let slice = self.buf.get(self.pos..end)?;
        if slice.iter().zip(kw).all(|(a, b)| a.to_ascii_lowercase() == *b) {
            self.pos = end;
            Some(())
        } else {
            None
        }
    }

    /// Byte offset of the next unconsumed byte, usable to slice the original
    /// `&str`/`&[u8]` this cursor was built over.
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Parse a SQL identifier: either double-quoted (with `""` escaping) or
    /// bare (ASCII-case-folded, as unquoted SQL identifiers are).
    pub(crate) fn parse_ident(&mut self) -> Option<String> {
        if self.eat_byte(b'"') {
            let mut out: Vec<u8> = Vec::new();
            loop {
                let b = self.peek()?;
                self.pos += 1;
                if b == b'"' {
                    if self.peek() == Some(b'"') {
                        self.pos += 1;
                        out.push(b'"');
                        continue;
                    }
                    break;
                }
                out.push(b);
            }
            return String::from_utf8(out).ok();
        }

        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80 {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return None;
        }
        let s = std::str::from_utf8(&self.buf[start..self.pos]).ok()?;
        Some(s.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(line: &str) -> CopyHeader {
        parse_copy_header(line.as_bytes()).expect("should parse")
    }

    fn decode(field: &str) -> Option<String> {
        decode_field(field.as_bytes()).unwrap().map(|v| v.into_owned())
    }

    #[test]
    fn parses_a_schema_qualified_header() {
        let h = header("COPY logs.events (event_id, widget_id, message) FROM stdin;");
        assert_eq!(h.schema.as_deref(), Some("logs"));
        assert_eq!(h.table, "events");
        assert_eq!(h.columns, ["event_id", "widget_id", "message"]);
        assert_eq!(h.qualified_name(), "logs.events");
    }

    #[test]
    fn parses_an_unqualified_header_without_columns() {
        let h = header("COPY widgets FROM stdin;");
        assert_eq!(h.schema, None);
        assert_eq!(h.table, "widgets");
        assert!(h.columns.is_empty());
        assert_eq!(h.qualified_name(), "widgets");
    }

    #[test]
    fn parses_quoted_identifiers_verbatim() {
        let h = header(r#"COPY "My Schema"."Odd Table" ("Id", "a""b") FROM stdin;"#);
        assert_eq!(h.schema.as_deref(), Some("My Schema"));
        assert_eq!(h.table, "Odd Table");
        assert_eq!(h.columns, ["Id", r#"a"b"#]);
    }

    #[test]
    fn folds_unquoted_identifiers_to_lowercase() {
        let h = header("COPY Public.Widgets (Id) FROM STDIN;");
        assert_eq!(h.schema.as_deref(), Some("public"));
        assert_eq!(h.table, "widgets");
        assert_eq!(h.columns, ["id"]);
    }

    #[test]
    fn rejects_non_header_copy_lines() {
        // These can appear at line start inside a dollar-quoted function body
        // or as leading substrings of real SQL; none may be read as a header.
        for line in [
            "COPY public.widgets TO stdout;",
            "COPY public.widgets FROM stdin WITH (FORMAT csv);",
            "COPY public.widgets (id) FROM stdin",
            "COPY public.widgets (id FROM stdin;",
            "COPY FROM stdin;",
            "COPYpublic.widgets FROM stdin;",
            "COPY public.widgets (id) FROM stdin; -- trailing comment",
            "-- COPY public.widgets (id) FROM stdin;",
            "\\restrict aToken",
        ] {
            assert!(parse_copy_header(line.as_bytes()).is_none(), "should not parse: {line}");
        }
    }

    #[test]
    fn matches_qualified_and_bare_names() {
        let h = header("COPY logs.events (id) FROM stdin;");
        assert!(h.matches("logs.events"));
        assert!(h.matches("events"));
        assert!(!h.matches("public.events"));
        assert!(!h.matches("widgets"));
    }

    #[test]
    fn splits_on_raw_delimiter_bytes_only() {
        let row = "1\ta\\tb\t\\N";
        let fields: Vec<&[u8]> = split_fields(row.as_bytes()).collect();
        assert_eq!(fields, [&b"1"[..], &b"a\\tb"[..], &b"\\N"[..]]);
    }

    #[test]
    fn decodes_null_and_empty_distinctly() {
        assert_eq!(decode("\\N"), None);
        assert_eq!(decode(""), Some(String::new()));
        assert_eq!(decode("\\\\N"), Some("\\N".to_string()));
    }

    #[test]
    fn decodes_character_escapes() {
        assert_eq!(decode("a\\nb"), Some("a\nb".to_string()));
        assert_eq!(decode("a\\tb"), Some("a\tb".to_string()));
        assert_eq!(decode("a\\rb"), Some("a\rb".to_string()));
        assert_eq!(decode("a\\\\b"), Some("a\\b".to_string()));
        assert_eq!(decode("a\\bb"), Some("a\u{8}b".to_string()));
        assert_eq!(decode("a\\fb"), Some("a\u{c}b".to_string()));
        assert_eq!(decode("a\\vb"), Some("a\u{b}b".to_string()));
        assert_eq!(decode("\\."), Some(".".to_string()));
        // Unknown escapes stand for the character itself.
        assert_eq!(decode("a\\qb"), Some("aqb".to_string()));
    }

    #[test]
    fn encodes_null_and_empty_distinctly() {
        assert_eq!(encode_field(None), b"\\N");
        assert_eq!(encode_field(Some("")), b"");
        assert_eq!(encode_field(Some("\\N")), b"\\\\N");
    }

    #[test]
    fn encodes_character_escapes() {
        assert_eq!(encode_field(Some("a\nb")), b"a\\nb");
        assert_eq!(encode_field(Some("a\tb")), b"a\\tb");
        assert_eq!(encode_field(Some("a\rb")), b"a\\rb");
        assert_eq!(encode_field(Some("a\\b")), b"a\\\\b");
        assert_eq!(encode_field(Some("a\u{8}b")), b"a\\bb");
        assert_eq!(encode_field(Some("a\u{c}b")), b"a\\fb");
        assert_eq!(encode_field(Some("a\u{b}b")), b"a\\vb");
    }

    #[test]
    fn encode_is_the_exact_inverse_of_decode_for_canonical_escapes() {
        // decode_field also accepts octal/hex escapes and unknown
        // "stands for itself" escapes, but pg_dump's COPY TO never emits
        // them (postgres-invariants.md I15), so encode_field only needs to
        // invert the six mnemonics, `\\`, and the delimiter -- exercised
        // against real pg_dump output in
        // `copy_text_escaping_round_trips_through_postgres` (tests/scan.rs).
        for field in [None, Some(""), Some("plain"), Some("a\\b\tc\nd\re\u{8}f\u{c}g\u{b}h")] {
            let encoded = encode_field(field);
            assert_eq!(decode_field(&encoded).unwrap().as_deref(), field);
        }
    }

    #[test]
    fn decodes_octal_and_hex_escapes() {
        assert_eq!(decode("\\101\\102"), Some("AB".to_string()));
        assert_eq!(decode("\\1013"), Some("A3".to_string()));
        assert_eq!(decode("\\7"), Some("\u{7}".to_string()));
        assert_eq!(decode("\\x41\\x42"), Some("AB".to_string()));
        assert_eq!(decode("\\x4"), Some("\u{4}".to_string()));
        // `\x` with no hex digit is a literal `x`.
        assert_eq!(decode("\\xz"), Some("xz".to_string()));
        // PostgreSQL accepts only lowercase `\x`, so `\X41` is `X41`.
        assert_eq!(decode("\\X41"), Some("X41".to_string()));
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert!(decode_field(b"caf\xff").is_err());
        assert!(decode_field(br"caf\xff").is_err());
    }

    #[test]
    fn passes_through_multibyte_utf8() {
        assert_eq!(decode("café ☕"), Some("café ☕".to_string()));
        assert_eq!(decode("caf\\xc3\\xa9"), Some("café".to_string()));
    }

    #[test]
    fn recognises_only_a_bare_terminator() {
        assert!(is_terminator(b"\\."));
        assert!(!is_terminator(b"\\\\."));
        assert!(!is_terminator(b"\\.x"));
        assert!(!is_terminator(b" \\."));
    }

    #[test]
    fn dollar_quotes_untouched_line_is_a_no_op() {
        let (tag, touched) = scan_dollar_quotes(b"CREATE TABLE public.t (id integer);", None);
        assert_eq!(tag, None);
        assert!(!touched);
    }

    #[test]
    fn dollar_quotes_open_and_close_across_lines() {
        let (tag, touched) = scan_dollar_quotes(b"    AS $$", None);
        assert_eq!(tag, Some(b"$$".to_vec()));
        assert!(touched);

        let (tag, touched) = scan_dollar_quotes(b"COPY public.t (a) FROM stdin;", tag);
        assert_eq!(tag, Some(b"$$".to_vec()), "still inside the quote, not a real header");
        assert!(touched);

        let (tag, touched) = scan_dollar_quotes(b"$$;", tag);
        assert_eq!(tag, None);
        assert!(touched);
    }

    #[test]
    fn dollar_quotes_tag_matching_ignores_a_different_tag() {
        let (tag, _) = scan_dollar_quotes(b"AS $func$", None);
        assert_eq!(tag, Some(b"$func$".to_vec()));

        // An untagged $$ inside a $func$-quoted body must not close it.
        let (tag, touched) = scan_dollar_quotes(b"SELECT 'x $$ y' AS note;", tag);
        assert_eq!(tag, Some(b"$func$".to_vec()));
        assert!(touched);

        let (tag, touched) = scan_dollar_quotes(b"$func$;", tag);
        assert_eq!(tag, None);
        assert!(touched);
    }

    #[test]
    fn dollar_quotes_open_and_close_on_the_same_line() {
        let (tag, touched) = scan_dollar_quotes(b"SELECT $$literal$$ AS x;", None);
        assert_eq!(tag, None);
        assert!(touched);
    }

    #[test]
    fn dollar_quotes_reject_a_tag_starting_with_a_digit() {
        // `$1$` is a positional parameter reference, not a valid dollar-quote
        // tag (PostgreSQL identifiers can't start with a digit) -- must not
        // be mistaken for one.
        let (tag, touched) = scan_dollar_quotes(b"SELECT $1$;", None);
        assert_eq!(tag, None);
        assert!(!touched);
    }
}
