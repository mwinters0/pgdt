//! `COPY` header grammar and COPY TEXT-format field decoding/encoding.
//!
//! This module is pure, synchronous, byte-slice-level code: it knows nothing
//! about files, offsets, or chunking. [`crate::scan`] drives it.
//!
//! [`decode_field`]/[`encode_field`] are the one place in the codebase that
//! converts between a field's on-disk COPY-escaped bytes and its unescaped
//! text — see `docs/design/decisions.md`, "D68". [`crate::decode`] (L2) never
//! sees escaped bytes at all: it takes `decode_field`'s already-unescaped
//! `&str` output and works purely in "unescaped text vs. Arrow value" terms.

use std::borrow::Cow;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The default COPY TEXT field delimiter. `pg_dump` plain-format output never
/// overrides it, so it is hardcoded rather than configurable.
pub const COPY_TEXT_DELIMITER: u8 = b'\t';

/// The literal that COPY TEXT uses for SQL `NULL`.
const COPY_TEXT_NULL_MARKER: &[u8] = b"\\N";

/// The line that terminates a COPY data block.
pub const COPY_BLOCK_TERMINATOR: &[u8] = b"\\.";

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
/// non-match is deliberately *not* an error: a `COPY` line this grammar does
/// not cover is not one `pg_dump` writes for a data block, and treating it as
/// ordinary SQL is the safe reading. The consequence is that COPY variants
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
    line == COPY_BLOCK_TERMINATOR
}

/// Split a raw COPY TEXT data line into its still-escaped field slices.
///
/// Splitting on raw delimiter bytes is correct: COPY TEXT output always
/// escapes an in-value tab as the two bytes `\` `t`, so a bare `0x09` byte is
/// unambiguously a field separator.
pub fn split_fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    field_ranges(line).map(|r| &line[r])
}

/// The same split as [`split_fields`], as **ranges** into the line.
///
/// A caller holding the row as `&str` as well as `&[u8]` — which is what
/// [`RawRow`] is — needs the position rather than the slice, so that it can
/// take the same field out of either. `split_fields` is this, resolved.
pub fn field_ranges(line: &[u8]) -> FieldRanges<'_> {
    FieldRanges { line, start: 0, done: false }
}

/// [`field_ranges`]'s iterator. Yields one range per field, always at least
/// one — an empty line is a single empty field, exactly as `split` gives it.
#[derive(Debug)]
pub struct FieldRanges<'a> {
    line: &'a [u8],
    start: usize,
    done: bool,
}

impl Iterator for FieldRanges<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        if self.done {
            return None;
        }
        match memchr::memchr(COPY_TEXT_DELIMITER, &self.line[self.start..]) {
            Some(rel) => {
                let end = self.start + rel;
                let range = self.start..end;
                self.start = end + 1;
                Some(range)
            }
            None => {
                self.done = true;
                Some(self.start..self.line.len())
            }
        }
    }
}

/// One row's field boundaries, discovered once and shared by everything that
/// reads that row.
///
/// [`field_ranges`] walks from the front every time it is asked; this is the
/// same split, memoized: each boundary is found by exactly one `memchr`,
/// whichever consumer asks for it first, and every later ask is an index into
/// what is already here (`docs/design/decisions.md`, "D28").
///
/// **It extends only as far as it is asked to.** A term reading field 3 finds
/// four boundaries and stops; the walk to the end of the row happens when
/// something needs the end of the row, which on a row the filter rejects is
/// never.
#[derive(Debug, Default)]
pub struct RowSplit {
    /// The end offset of every field found so far, in order. Field `i` runs
    /// from `ends[i - 1] + 1` (or `0`) to `ends[i]`.
    ends: Vec<usize>,
    /// Whether `ends` reaches the row's last field, so nothing more can be
    /// found.
    complete: bool,
    /// The length of the row the boundaries above were found in, once
    /// something has asked for one. Debug builds only — see [`Self::bind`].
    #[cfg(debug_assertions)]
    row_len: Option<usize>,
}

impl RowSplit {
    /// Begin a new row, keeping the capacity the last one discovered. Every
    /// row of a block has the same width, so once one row has been split to
    /// its end the buffer never grows again.
    pub fn restart(&mut self) {
        self.ends.clear();
        self.complete = false;
        #[cfg(debug_assertions)]
        {
            self.row_len = None;
        }
    }

    /// Tie the split to the row its boundaries belong to, in debug builds.
    ///
    /// The accessors take the row on every call and nothing in the types says
    /// it is the row the ends were found in, so a missed [`Self::restart`]
    /// would yield **in-range indices into the wrong row** and no panic
    /// anywhere (`docs/design/decisions.md`, "D28").
    ///
    /// **A length, not the row's identity.** It is free (the accessors hold
    /// `row.len()` already), it survives a row that moved, and it catches the
    /// mistake in the shape it occurs: a `restart` missed on a row path runs
    /// on every row of a block. What it does not catch is a single
    /// equal-length pair, which is why this is a guard and not a proof.
    #[inline]
    fn bind(&mut self, row: &[u8]) {
        #[cfg(debug_assertions)]
        match self.row_len {
            Some(len) => assert_eq!(
                len,
                row.len(),
                "RowSplit read against a row of a different length than the one \
                 its boundaries were found in — a `restart` was missed"
            ),
            None => self.row_len = Some(row.len()),
        }
        #[cfg(not(debug_assertions))]
        let _ = row;
    }

    /// The range of field `index`, extending the split as far as it must and
    /// no further. `None` where the row has no such field.
    ///
    /// The answer is [`field_ranges`]'s, for the same row and the same index:
    /// a row always has at least one field, and a trailing delimiter is
    /// followed by an empty one.
    ///
    /// `#[inline]` because the caller is one predicate term per row and the
    /// loop below usually does not run at all — the boundary it wants is
    /// already here, or one `memchr` away.
    #[inline]
    pub fn field(&mut self, row: &[u8], index: usize) -> Option<Range<usize>> {
        self.bind(row);
        while !self.complete && self.ends.len() <= index {
            self.extend(row);
        }
        let end = *self.ends.get(index)?;
        let start = if index == 0 { 0 } else { self.ends[index - 1] + 1 };
        Some(start..end)
    }

    /// Every field of the row, as end offsets — the split finished from
    /// wherever the last consumer stopped.
    ///
    /// This is one tight walk rather than [`Self::field`] called in a loop,
    /// which the caller — `RowBatcher::push_row`, wanting every field — would
    /// otherwise drive as `memchr` in a loop with the position in a register.
    #[inline]
    pub fn complete(&mut self, row: &[u8]) -> &[usize] {
        self.bind(row);
        if !self.complete {
            let mut pos = self.ends.last().map_or(0, |end| end + 1);
            loop {
                match memchr::memchr(COPY_TEXT_DELIMITER, &row[pos..]) {
                    Some(rel) => {
                        let end = pos + rel;
                        self.ends.push(end);
                        pos = end + 1;
                    }
                    None => {
                        self.ends.push(row.len());
                        self.complete = true;
                        break;
                    }
                }
            }
        }
        &self.ends
    }

    #[inline]
    fn extend(&mut self, row: &[u8]) {
        let start = self.ends.last().map_or(0, |end| end + 1);
        match memchr::memchr(COPY_TEXT_DELIMITER, &row[start..]) {
            Some(rel) => self.ends.push(start + rel),
            None => {
                self.ends.push(row.len());
                self.complete = true;
            }
        }
    }
}

/// The prefix of `span` through its last row terminator where all of it is
/// valid UTF-8, and the empty string otherwise.
///
/// This is the bulk half of the codec: one SIMD validation per chunk in place
/// of one `std::str::from_utf8` per field, sound because a COPY TEXT row's
/// delimiters (`0x09`) and terminator (`0x0A`) are ASCII and an ASCII byte
/// never occurs inside a multi-byte UTF-8 sequence, so every field of a
/// validated row is itself validated. See `docs/design/decisions.md`, "D27".
///
/// Cutting at the last newline is what makes the call safe to make on a
/// *chunk*: the bytes after it are a partial line whose continuation is in
/// the next chunk, and a multi-byte sequence split across that boundary would
/// fail validation for no reason. A failure anywhere in the prefix answers
/// empty rather than a shorter prefix, which puts every row of the span back
/// on the per-field check that raises `Error::InvalidUtf8`.
pub fn validated_prefix(span: &[u8]) -> &str {
    let end = memchr::memrchr(b'\n', span).map_or(0, |i| i + 1);
    simdutf8::basic::from_utf8(&span[..end]).unwrap_or("")
}

/// One raw COPY TEXT data row, with whatever the read loop already knows
/// about its encoding.
///
/// A row lifted out of a [`validated_prefix`] arrives as [`Self::validated`]
/// and every field it hands back skips the UTF-8 check; one the loop could
/// not validate in bulk arrives as [`Self::unchecked`] and each field is checked
/// as it decodes. The two are the same bytes and the same answers — the
/// difference is only where the validation happened.
#[derive(Debug, Clone, Copy)]
pub struct RawRow<'a> {
    bytes: &'a [u8],
    /// The same bytes as `str`, when a bulk validation already covered them.
    text: Option<&'a str>,
}

impl<'a> RawRow<'a> {
    /// A row whose bytes nothing has validated: each field is UTF-8-checked
    /// as it is decoded.
    pub fn unchecked(bytes: &'a [u8]) -> Self {
        Self { bytes, text: None }
    }

    /// A row taken out of a span [`validated_prefix`] already validated.
    pub fn validated(text: &'a str) -> Self {
        Self { bytes: text.as_bytes(), text: Some(text) }
    }

    /// The row's raw, still-escaped bytes — what [`field_ranges`] splits and
    /// what a caller measuring or locating a field works in.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Decode the field at `field`, one of [`field_ranges`]'s ranges.
    ///
    /// **The `str` path is a total fallback, not an assertion.** `str::get`
    /// answers `None` for a range that is out of bounds or not on a character
    /// boundary, and this drops back to the checked decode there rather than
    /// panicking. Neither can happen for a range this module produced: fields
    /// are delimited by ASCII bytes, which are always character boundaries.
    pub fn decode(&self, field: Range<usize>) -> Result<Option<Cow<'a, str>>> {
        match self.text.and_then(|text| text.get(field.clone())) {
            Some(text) => decode_validated_field(text),
            None => decode_field(&self.bytes[field]),
        }
    }
}

/// Decode one still-escaped COPY TEXT field.
///
/// Returns `None` for SQL `NULL` (the field `\N`). The borrowed variant is
/// returned whenever the field contains no escape sequences, which is the
/// common case.
pub fn decode_field(field: &[u8]) -> Result<Option<Cow<'_, str>>> {
    if field == COPY_TEXT_NULL_MARKER {
        return Ok(None);
    }
    if !field.contains(&b'\\') {
        return Ok(Some(Cow::Borrowed(as_utf8(field)?)));
    }
    Ok(Some(Cow::Owned(unescape_field(field)?)))
}

/// [`decode_field`] over a field whose bytes are already known to be UTF-8 —
/// the borrow path with no per-field validation left in it.
///
/// The escaped path still validates, and must: `\xNN` and the octal forms can
/// synthesize a byte sequence that is not UTF-8 out of input that is.
fn decode_validated_field(field: &str) -> Result<Option<Cow<'_, str>>> {
    if field.as_bytes() == COPY_TEXT_NULL_MARKER {
        return Ok(None);
    }
    if !field.as_bytes().contains(&b'\\') {
        return Ok(Some(Cow::Borrowed(field)));
    }
    Ok(Some(Cow::Owned(unescape_field(field.as_bytes())?)))
}

/// The escaped path of [`decode_field`]: unescape into an owned buffer, and
/// validate that buffer, since an escape can produce bytes the input did not
/// carry.
fn unescape_field(field: &[u8]) -> Result<String> {
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

    String::from_utf8(out)
        .map_err(|e| Error::InvalidUtf8 { valid_up_to: e.utf8_error().valid_up_to() })
}

/// Re-apply COPY TEXT escaping to already-unescaped text — the exact inverse
/// of [`decode_field`], restricted to the escapes `pg_dump`'s `COPY TO` ever
/// emits: a doubled backslash and the six control-character mnemonics `\b \f
/// \n \r \t \v`, the delimiter, a tab, among them (postgres-invariants.md
/// I15). The octal/hex forms `decode_field` accepts on input are a `COPY FROM` reader
/// convenience only; `COPY TO` never produces them, so `encode_field` doesn't
/// need to reproduce them for a round trip against real `pg_dump` output to
/// hold — see `copy_text_escaping_round_trips_through_postgres` in
/// `tests/scan.rs`. `None` encodes as the `\N` null marker.
pub fn encode_field(field: Option<&str>) -> Vec<u8> {
    let Some(s) = field else {
        return COPY_TEXT_NULL_MARKER.to_vec();
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
        // invert the six mnemonics, the tab delimiter's among them, and `\\` -- exercised
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

    /// Every row this codec is ever handed, in one place: escapes, the NULL
    /// marker, an empty field, multi-byte UTF-8, and the escapes that
    /// *synthesize* bytes the input did not carry.
    const ROWS: [&str; 7] = [
        "1\talpha\ta simple widget",
        "2\t\\N\t",
        "3\tmulti\\nline\\twith a backslash \\\\ inside\t\\N",
        "4\tcafé\tsnowman ☃ and an emoji 🐈",
        "5\tcarriage\\rreturn, octal \\101, hex \\x42\tx",
        "6\t\t",
        "",
    ];

    #[test]
    fn field_ranges_resolve_to_the_same_split() {
        for row in ROWS {
            let bytes = row.as_bytes();
            let by_range: Vec<&[u8]> = field_ranges(bytes).map(|r| &bytes[r]).collect();
            let by_slice: Vec<&[u8]> = bytes.split(|&b| b == COPY_TEXT_DELIMITER).collect();
            assert_eq!(by_range, by_slice, "{row:?}");
        }
    }

    /// The shared split answers exactly what `field_ranges` answers, at every
    /// index and one past the end — the whole of its contract.
    #[test]
    fn a_row_split_answers_what_field_ranges_answers() {
        let mut split = RowSplit::default();
        for row in ROWS {
            let bytes = row.as_bytes();
            let expected: Vec<Range<usize>> = field_ranges(bytes).collect();
            // Ascending, which is how both consumers ask.
            split.restart();
            for (i, want) in expected.iter().enumerate() {
                assert_eq!(split.field(bytes, i).as_ref(), Some(want), "{row:?} field {i}");
            }
            assert_eq!(split.field(bytes, expected.len()), None, "{row:?} past the end");
            // And `complete` agrees with what `field` found one at a time.
            split.restart();
            let ends: Vec<usize> = expected.iter().map(|r| r.end).collect();
            assert_eq!(split.complete(bytes), ends.as_slice(), "{row:?}");
            // Straight to the last field, then back: a term reads one field
            // and `push_row` then reads them all.
            split.restart();
            let last = expected.len() - 1;
            assert_eq!(split.field(bytes, last).as_ref(), expected.last(), "{row:?}");
            for (i, want) in expected.iter().enumerate() {
                assert_eq!(split.field(bytes, i).as_ref(), Some(want), "{row:?} re-read {i}");
            }
            // Past the end first: the split completes and stays complete.
            split.restart();
            assert_eq!(split.field(bytes, expected.len() + 3), None, "{row:?}");
            assert_eq!(split.field(bytes, 0).as_ref(), expected.first(), "{row:?}");
        }
    }

    /// A term deep in the row leaves the split holding every boundary it
    /// crossed and no more.
    #[test]
    fn a_row_split_extends_only_as_far_as_it_is_asked() {
        let bytes = b"a\tb\tc\td\te";
        let mut split = RowSplit::default();
        assert_eq!(split.field(bytes, 1), Some(2..3));
        assert_eq!(split.ends, vec![1, 3], "two boundaries, not five");
        assert_eq!(split.field(bytes, 4), Some(8..9));
        assert_eq!(split.ends, vec![1, 3, 5, 7, 9]);
    }

    /// A split read against a row it was not restarted for panics in a debug
    /// build, in both accessors. Without this the mistake is in-range indices
    /// into the wrong row and no error anywhere, which is the one failure the
    /// borrowed-slice design has no type to prevent.
    #[test]
    #[cfg(debug_assertions)]
    fn a_missed_restart_panics_in_a_debug_build() {
        let first = b"aaa\tbbb";
        let second = b"aa\tbb";

        let mut split = RowSplit::default();
        split.complete(first);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| split.field(second, 0)))
                .is_err(),
            "`field` on an unrestarted split"
        );

        let mut split = RowSplit::default();
        split.field(first, 0);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                split.complete(second).len()
            }))
            .is_err(),
            "`complete` on an unrestarted split"
        );

        // And a `restart` in between is the whole of what the guard asks for.
        let mut split = RowSplit::default();
        split.complete(first);
        split.restart();
        assert_eq!(split.field(second, 0), Some(0..2));
    }

    #[test]
    fn a_validated_prefix_stops_at_the_last_row_terminator() {
        // The bytes past the last newline are a partial line whose rest is in
        // the next chunk; validating them would fail on a split multi-byte
        // sequence for no reason.
        let span = "a\nb\n☃".as_bytes();
        assert_eq!(validated_prefix(span), "a\nb\n");
        assert_eq!(validated_prefix(&span[..span.len() - 1]), "a\nb\n");
        // No whole row at all, and a chunk that is exactly whole rows.
        assert_eq!(validated_prefix(b"no newline here"), "");
        assert_eq!(validated_prefix(b"a\n"), "a\n");
        assert_eq!(validated_prefix(b""), "");
    }

    #[test]
    fn a_prefix_that_does_not_validate_answers_empty() {
        // 0xFF is not UTF-8 anywhere. The whole prefix is refused rather than
        // shortened, which puts every row of the span back on the per-field
        // check that raises `Error::InvalidUtf8`.
        let mut span = b"good row\n".to_vec();
        span.extend_from_slice(b"bad \xff row\n");
        assert_eq!(validated_prefix(&span), "");
    }

    #[test]
    fn a_validated_row_decodes_exactly_as_an_unchecked_one() {
        for row in ROWS {
            let bytes = row.as_bytes();
            for field in field_ranges(bytes) {
                let unchecked = RawRow::unchecked(bytes).decode(field.clone()).unwrap();
                let validated = RawRow::validated(row).decode(field.clone()).unwrap();
                assert_eq!(unchecked, validated, "{row:?} at {field:?}");
            }
        }
    }

    #[test]
    fn a_range_off_a_character_boundary_falls_back_rather_than_panicking() {
        // Not reachable from `field_ranges` — fields are delimited by ASCII —
        // but the fallback is what makes that a performance property rather
        // than a safety one.
        let row = "☃";
        assert_eq!(
            RawRow::validated(row).decode(0..1).unwrap_err().to_string(),
            RawRow::unchecked(row.as_bytes()).decode(0..1).unwrap_err().to_string()
        );
    }
}
