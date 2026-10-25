//! The output scanner: the nested forms as PostgreSQL's `*_out` functions
//! write them inside a `COPY` field, read by `decode_*` and written back by
//! `render_*`, each the exact inverse of the other.
//!
//! # The output side: one scanner, three parameter sets
//!
//! The three container forms are *not* one quoting rule wearing three hats
//! (I20, `docs/design/postgres-invariants.md`). They share a shape — a
//! wrapper, a separator, a force-quote predicate and an escape convention —
//! and, the separator being `,` in all three, disagree on the last two:
//!
//! | Form | Wrapper | Force-quote on | Inside quotes |
//! |---|---|---|---|
//! | array (`array_out`) | `{`…`}` | `"` `\` `{` `}` `,`, whitespace, empty, `NULL` | `"` → `\"` |
//! | record (`record_out`) | `(`…`)` | `"` `\` `(` `)` `,`, whitespace, empty | `"` → `""` |
//! | range bound (`range_bound_escape`) | `[`/`(`…`]`/`)` | `"` `\` `(` `)` `[` `]` `,`, whitespace, empty | `"` → `""` |
//!
//! Both conventions double a backslash; only the quote differs. A composite
//! inside an array is therefore escaped both ways at once, one layer per
//! nesting level, which is the case a single-convention decoder gets wrong
//! while passing everything else.
//!
//! A multirange needs no fourth parameter set: `multirange_out` concatenates
//! its members' `range_out` results with no escaping step at all, and the
//! members' own brackets are what make the result re-parseable.
//!
//! Neither does `int2vector`, and for a stronger reason: `int2vectorout`
//! writes `int16`s separated by one space, and an `int16`'s spelling can
//! contain neither the separator nor a quote nor a NULL, so there is nothing
//! for a quoting rule to decide. It is a container by shape and a scan by
//! `split(' ')` in fact — see [`decode_int2vector`].
//!
//! # What `decode_*` accepts
//!
//! The `decode_*`/`render_*` half is inverses of the *output* functions, not
//! reimplementations of the considerably more permissive `*_in` parsers (I20's
//! scope limit; the input half, [`super::input`], is I44). Whitespace padding around an
//! element, an unquoted token containing a backslash escape, a nested `{}` —
//! all things `*_in` accepts and `*_out` never emits — are rejected rather than
//! guessed at. A dimension prefix `array_out` would not write (every lower
//! bound 1, or a bound with leading zeros) is read as the bounds it states and
//! renders as `array_out` writes them. The other deliberate leniency is that a bare `NULL` array element
//! is matched case-insensitively, as `array_in` does: `array_out` force-quotes
//! any element whose text *is* `null` in any casing, so agreeing with
//! PostgreSQL here can never misread real output.
//!
//! The decoders' separator is hardcoded to `,`. An array whose element type
//! sets a different `typdelim` (`box`, or any C-level base type) is not decoded
//! as an array at all (I22) — though a strict
//! parse reads a `box` array's elements to check them, and splits a base
//! type's array beneath a container at its `DELIMITER`
//! ([`parse_array_delimited`]).
//!
//! [`parse_array_delimited`]: super::input::parse_array_delimited

use std::borrow::Cow;

use super::{ArrayLiteral, Elements, RangeLiteral, RecordLiteral, is_space};

/// How a quoted token escapes an embedded `"`. Both conventions escape an
/// embedded `\` by doubling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    /// `"` → `\"` — `array_out`.
    Backslash,
    /// `"` → `""` — `record_out` and `range_bound_escape`.
    Double,
}

/// A set of bytes as 256 bits, tested by index rather than by search.
///
/// `needs_quote` asks it once per byte of every token, in both directions, so
/// one indexed bit answers in a shift and a mask where a byte slice would be
/// a linear `[u8]::contains` (`nested-decode-micro`). Every `Syntax` is a
/// `const`, so every set is built at compile time.
#[derive(Debug, Clone, Copy)]
struct ByteSet([u64; 4]);

impl ByteSet {
    /// `const` so a `Syntax` stays a compile-time constant.
    const fn new(bytes: &[u8]) -> Self {
        let mut words = [0u64; 4];
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            words[(c >> 6) as usize] |= 1u64 << (c & 0x3f);
            i += 1;
        }
        ByteSet(words)
    }

    #[inline]
    const fn contains(&self, c: u8) -> bool {
        self.0[(c >> 6) as usize] & (1u64 << (c & 0x3f)) != 0
    }
}

/// One instantiation of the quoted-token grammar. The wrapper is not here:
/// a range's brackets vary per value (`[`/`(`, `]`/`)`), so each caller opens
/// and closes its own container and passes the bytes that terminate a token.
#[derive(Debug, Clone, Copy)]
struct Syntax {
    separator: u8,
    escape: Escape,
    /// Characters that force a token to be quoted, beyond the empty-string
    /// rule. Whitespace is tested separately, and the separator is included
    /// here explicitly rather than implied.
    force_quote: ByteSet,
    /// True where a bare `NULL` token means SQL NULL and an empty token is
    /// malformed (`array_out`); false where SQL NULL is spelled as nothing at
    /// all and `""` is the empty string (`record_out`, `range_bound_escape`).
    bare_null: bool,
}

const ARRAY: Syntax = Syntax {
    separator: b',',
    escape: Escape::Backslash,
    force_quote: ByteSet::new(b"\"\\{},"),
    bare_null: true,
};

const RECORD: Syntax = Syntax {
    separator: b',',
    escape: Escape::Double,
    force_quote: ByteSet::new(b"\"\\(),"),
    bare_null: false,
};

const RANGE_BOUND: Syntax = Syntax {
    separator: b',',
    escape: Escape::Double,
    force_quote: ByteSet::new(b"\"\\()[],"),
    bare_null: false,
};

fn needs_quote(value: &str, syntax: &Syntax) -> bool {
    if value.is_empty() {
        return true;
    }
    if syntax.bare_null && value.eq_ignore_ascii_case("NULL") {
        return true;
    }
    value.bytes().any(|c| syntax.force_quote.contains(c) || is_space(c))
}

/// Append one token in `syntax`'s form. `None` is SQL NULL: a bare `NULL` for
/// an array, nothing at all for a record field or a range bound.
fn push_token(out: &mut String, value: Option<&str>, syntax: &Syntax) {
    let Some(value) = value else {
        if syntax.bare_null {
            out.push_str("NULL");
        }
        return;
    };
    if !needs_quote(value, syntax) {
        out.push_str(value);
        return;
    }
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => match syntax.escape {
                Escape::Backslash => out.push_str("\\\""),
                Escape::Double => out.push_str("\"\""),
            },
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

/// Append the bare `NULL` an array spells a SQL NULL element as.
///
/// Exists for the render path that writes an element straight into the
/// caller's buffer and so never builds an [`ArrayLiteral`] to hand to
/// [`render_array`] (`render::render_array_into`). The spelling stays here,
/// beside the grammar that owns it.
pub fn push_array_null(out: &mut String) {
    push_token(out, None, &ARRAY);
}

/// Quote, in place, the array element whose rendered text is already sitting
/// at `out[mark..]`.
///
/// **The common element needs no quoting and this copies nothing** — a bare
/// element costs one [`needs_quote`] walk and no move. One that does need
/// quoting is taken aside into `scratch` and re-emitted through
/// [`push_token`]; `scratch` is cleared per element and reused across the
/// whole value, so a value allocates only if some element was quotable, and
/// again only for a longer one. The scratch is unavoidable: the quoting decision is made
/// from the *finished* element text and escaping expands it, so there is no
/// room in `out` to write the escaped form over the raw one.
pub fn quote_array_element(out: &mut String, mark: usize, scratch: &mut String) {
    if !needs_quote(&out[mark..], &ARRAY) {
        return;
    }
    scratch.clear();
    scratch.push_str(&out[mark..]);
    out.truncate(mark);
    push_token(out, Some(scratch), &ARRAY);
}

/// Consume a `"`-delimited token starting at the opening quote, returning its
/// unescaped text and the index just past the closing quote.
///
/// **Borrowed unless the token actually carries an escape.** The first pass
/// looks for the closing quote and copies nothing; a `\` or a doubled `""`
/// aborts it into the second, which rebuilds the token from the bytes already
/// walked.
fn scan_quoted(s: &[u8], mut i: usize, escape: Escape) -> Option<(Cow<'_, str>, usize)> {
    debug_assert_eq!(s.get(i), Some(&b'"'));
    i += 1;
    let start = i;
    loop {
        match *s.get(i)? {
            b'"' if !(escape == Escape::Double && s.get(i + 1) == Some(&b'"')) => {
                let text = std::str::from_utf8(&s[start..i]).ok()?;
                return Some((Cow::Borrowed(text), i + 1));
            }
            b'"' | b'\\' => break,
            _ => i += 1,
        }
    }
    let mut out = Vec::from(&s[start..i]);
    loop {
        match *s.get(i)? {
            b'"' => {
                if escape == Escape::Double && s.get(i + 1) == Some(&b'"') {
                    out.push(b'"');
                    i += 2;
                } else {
                    return Some((Cow::Owned(String::from_utf8(out).ok()?), i + 1));
                }
            }
            b'\\' => {
                out.push(*s.get(i + 1)?);
                i += 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
}

/// Scan one token from `s[i..]`, stopping at an unquoted `syntax.separator` or
/// any byte in `terminators`. Returns the token — `None` for SQL NULL — and
/// the index of the byte it stopped on, which is always present: a token that
/// runs off the end of the input is malformed, since every container closes.
fn scan_token<'a>(
    s: &'a [u8],
    i: usize,
    syntax: &Syntax,
    terminators: &[u8],
) -> Option<(Option<Cow<'a, str>>, usize)> {
    let stops = |c: u8| c == syntax.separator || terminators.contains(&c);

    if s.get(i) == Some(&b'"') {
        let (text, next) = scan_quoted(s, i, syntax.escape)?;
        // Nothing may follow a closing quote but a separator or a terminator;
        // `"a"b` is not something any `*_out` function can produce.
        if !stops(*s.get(next)?) {
            return None;
        }
        return Some((Some(text), next));
    }

    let start = i;
    let mut i = i;
    while let Some(&c) = s.get(i) {
        if stops(c) {
            break;
        }
        // A quote only ever opens a token, and an unquoted token can never
        // hold one — every form force-quotes a value containing `"`.
        if c == b'"' {
            return None;
        }
        i += 1;
    }
    s.get(i)?;
    let text = std::str::from_utf8(&s[start..i]).ok()?;
    if syntax.bare_null {
        if text.is_empty() {
            return None;
        }
        if text.eq_ignore_ascii_case("NULL") {
            return Some((None, i));
        }
    } else if text.is_empty() {
        return Some((None, i));
    }
    // An unquoted token that *would* have been quoted is not something the
    // matching `*_out` function wrote, so accepting it would break the
    // property this module promises: that render inverts decode on whatever
    // `*_out` writes.
    if needs_quote(text, syntax) {
        return None;
    }
    Some((Some(Cow::Borrowed(text)), i))
}

/// Accumulator for the recursive brace walk. `dims` and `leaf_depth` are
/// filled in as the structure is discovered and cross-checked as it repeats:
/// every sibling list at one depth must have the same length, and every token
/// must sit at the same depth, or the literal is ragged and rejected.
struct ArrayScan<'a> {
    s: &'a [u8],
    dims: Vec<Option<usize>>,
    leaf_depth: Option<usize>,
    elements: Elements<'a>,
}

impl<'a> ArrayScan<'a> {
    fn record_dim(&mut self, depth: usize, count: usize) -> Option<()> {
        if self.dims.len() <= depth {
            self.dims.resize(depth + 1, None);
        }
        match self.dims[depth] {
            None => {
                self.dims[depth] = Some(count);
                Some(())
            }
            Some(n) if n == count => Some(()),
            Some(_) => None,
        }
    }

    fn braces(&mut self, mut i: usize, depth: usize) -> Option<usize> {
        if self.s.get(i) != Some(&b'{') {
            return None;
        }
        i += 1;
        if self.s.get(i) == Some(&b'}') {
            // `{}` is only ever the whole value: `array_out` collapses a
            // zero-element array to it before it ever considers dimensions,
            // so an inner empty list is not something a dump can contain.
            if depth != 0 {
                return None;
            }
            self.record_dim(depth, 0)?;
            return Some(i + 1);
        }
        let mut count = 0usize;
        loop {
            if self.s.get(i) == Some(&b'{') {
                i = self.braces(i, depth + 1)?;
            } else {
                let (value, next) = scan_token(self.s, i, &ARRAY, b"}{")?;
                match self.leaf_depth {
                    None => self.leaf_depth = Some(depth + 1),
                    Some(d) if d == depth + 1 => {}
                    Some(_) => return None,
                }
                self.elements.push(value);
                i = next;
            }
            count += 1;
            match self.s.get(i) {
                Some(&b',') => i += 1,
                Some(&b'}') => {
                    i += 1;
                    break;
                }
                _ => return None,
            }
        }
        self.record_dim(depth, count)?;
        Some(i)
    }
}

/// Parse a signed decimal integer, returning it and the index just past it.
fn parse_int(s: &[u8], mut i: usize) -> Option<(i32, usize)> {
    let start = i;
    if s.get(i) == Some(&b'-') {
        i += 1;
    }
    let digits = i;
    while s.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == digits {
        return None;
    }
    let text = std::str::from_utf8(&s[start..i]).ok()?;
    Some((text.parse().ok()?, i))
}

/// Decode an `array_out` literal. `None` means the text is not one — the
/// caller turns that into `Error::FieldDecode` naming the column.
pub fn decode_array(s: &str) -> Option<ArrayLiteral<'_>> {
    let b = s.as_bytes();
    let mut i = 0;

    // `[lb:ub]` per dimension, then `=`, which `array_out` writes iff some
    // lower bound is not 1 and which is read wherever it is written.
    let mut decoration: Vec<(i32, usize)> = Vec::new();
    while b.get(i) == Some(&b'[') {
        i += 1;
        let (lb, next) = parse_int(b, i)?;
        i = next;
        if b.get(i) != Some(&b':') {
            return None;
        }
        let (ub, next) = parse_int(b, i + 1)?;
        i = next;
        if b.get(i) != Some(&b']') {
            return None;
        }
        i += 1;
        decoration.push((lb, usize::try_from(i64::from(ub) - i64::from(lb) + 1).ok()?));
    }
    if !decoration.is_empty() {
        if b.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
    }

    let mut scan = ArrayScan { s: b, dims: Vec::new(), leaf_depth: None, elements: Vec::new() };
    if scan.braces(i, 0)? != b.len() {
        return None;
    }
    let leaf_depth = scan.leaf_depth;
    let elements = std::mem::take(&mut scan.elements);

    if elements.is_empty() {
        // `array_out` never decorates `{}` — it returns before it looks at the
        // bounds — so a decorated empty array is not real output.
        if !decoration.is_empty() {
            return None;
        }
        return Some(ArrayLiteral { elements, dims: Vec::new(), lower_bounds: Vec::new() });
    }

    let dims = scan.dims.into_iter().collect::<Option<Vec<usize>>>()?;
    if leaf_depth != Some(dims.len()) || dims.iter().product::<usize>() != elements.len() {
        return None;
    }

    let lower_bounds = if decoration.is_empty() {
        vec![1; dims.len()]
    } else {
        if decoration.len() != dims.len()
            || decoration.iter().zip(&dims).any(|((_, size), dim)| size != dim)
        {
            return None;
        }
        decoration.iter().map(|(lb, _)| *lb).collect()
    };

    Some(ArrayLiteral { elements, dims, lower_bounds })
}

fn render_braces(out: &mut String, a: &ArrayLiteral<'_>, depth: usize, cursor: &mut usize) {
    out.push('{');
    for k in 0..a.dims[depth] {
        if k > 0 {
            out.push(',');
        }
        if depth + 1 == a.dims.len() {
            push_token(out, a.elements[*cursor].as_deref(), &ARRAY);
            *cursor += 1;
        } else {
            render_braces(out, a, depth + 1, cursor);
        }
    }
    out.push('}');
}

/// Render an [`ArrayLiteral`] back to exactly what `array_out` would emit.
///
/// # Panics
///
/// Panics if `dims` and `elements` disagree, which [`decode_array`] cannot
/// produce and a hand-built value must not.
pub fn render_array(a: &ArrayLiteral<'_>) -> String {
    if a.elements.is_empty() {
        return "{}".to_string();
    }
    assert_eq!(
        a.dims.iter().product::<usize>(),
        a.elements.len(),
        "ArrayLiteral dims do not describe its elements"
    );
    assert_eq!(a.dims.len(), a.lower_bounds.len(), "ArrayLiteral has a bound-per-dimension");
    let mut out = String::new();
    if a.is_decorated() {
        for (lb, dim) in a.lower_bounds.iter().zip(&a.dims) {
            out.push_str(&format!("[{}:{}]", lb, i64::from(*lb) + *dim as i64 - 1));
        }
        out.push('=');
    }
    render_braces(&mut out, a, 0, &mut 0);
    out
}

pub fn decode_record(s: &str) -> Option<RecordLiteral> {
    let b = s.as_bytes();
    if b.first() != Some(&b'(') {
        return None;
    }
    let mut i = 1;
    let mut fields = Vec::new();
    loop {
        let (value, next) = scan_token(b, i, &RECORD, b")")?;
        fields.push(value.map(Cow::into_owned));
        i = next;
        match b.get(i) {
            Some(&b',') => i += 1,
            Some(&b')') => {
                i += 1;
                break;
            }
            _ => return None,
        }
    }
    if i != b.len() {
        return None;
    }
    Some(RecordLiteral { fields })
}

pub fn render_record(r: &RecordLiteral) -> String {
    let mut out = String::new();
    out.push('(');
    for (k, field) in r.fields.iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        push_token(&mut out, field.as_deref(), &RECORD);
    }
    out.push(')');
    out
}

pub fn decode_range(s: &str) -> Option<RangeLiteral> {
    if s == "empty" {
        return Some(RangeLiteral::empty());
    }
    let b = s.as_bytes();
    let lower_inclusive = match *b.first()? {
        b'[' => true,
        b'(' => false,
        _ => return None,
    };
    let (lower, i) = scan_token(b, 1, &RANGE_BOUND, b"])")?;
    let lower = lower.map(Cow::into_owned);
    if b.get(i) != Some(&b',') {
        return None;
    }
    let (upper, i) = scan_token(b, i + 1, &RANGE_BOUND, b"])")?;
    let upper = upper.map(Cow::into_owned);
    let upper_inclusive = match *b.get(i)? {
        b']' => true,
        b')' => false,
        _ => return None,
    };
    if i + 1 != b.len() {
        return None;
    }
    Some(RangeLiteral { empty: false, lower, upper, lower_inclusive, upper_inclusive })
}

pub fn render_range(r: &RangeLiteral) -> String {
    if r.empty {
        return "empty".to_string();
    }
    let mut out = String::new();
    out.push(if r.lower_inclusive { '[' } else { '(' });
    push_token(&mut out, r.lower.as_deref(), &RANGE_BOUND);
    out.push(',');
    push_token(&mut out, r.upper.as_deref(), &RANGE_BOUND);
    out.push(if r.upper_inclusive { ']' } else { ')' });
    out
}

/// The extent of one `range_out` member inside a multirange: from its opening
/// bracket to just past the matching closing one, stepping over quoted bounds
/// so that a `)` or `]` inside a bound does not end it early.
fn range_extent(s: &[u8], mut i: usize) -> Option<usize> {
    match *s.get(i)? {
        b'[' | b'(' => i += 1,
        _ => return None,
    }
    loop {
        match *s.get(i)? {
            b'"' => i = scan_quoted(s, i, Escape::Double)?.1,
            b']' | b')' => return Some(i + 1),
            _ => i += 1,
        }
    }
}

/// Decode a `multirange_out` literal into its member ranges.
///
/// There is no escaping layer to undo: `multirange_out` concatenates its
/// members' `range_out` results directly. A member is therefore always a
/// bracketed range — PostgreSQL drops empty ranges when it builds a
/// multirange, so the bare word `empty` never appears as a member and is not
/// accepted as one.
pub fn decode_multirange(s: &str) -> Option<Vec<RangeLiteral>> {
    let b = s.as_bytes();
    if b.first() != Some(&b'{') {
        return None;
    }
    let mut members = Vec::new();
    let mut i = 1;
    if b.get(i) == Some(&b'}') {
        return (i + 1 == b.len()).then_some(members);
    }
    loop {
        let end = range_extent(b, i)?;
        members.push(decode_range(std::str::from_utf8(&b[i..end]).ok()?)?);
        i = end;
        match b.get(i) {
            Some(&b',') => i += 1,
            Some(&b'}') => {
                i += 1;
                break;
            }
            _ => return None,
        }
    }
    if i != b.len() {
        return None;
    }
    Some(members)
}

pub fn render_multirange(members: &[RangeLiteral]) -> String {
    let mut out = String::new();
    out.push('{');
    for (k, member) in members.iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        out.push_str(&render_range(member));
    }
    out.push('}');
    out
}

/// One element of an `int2vectorout` literal, in the spelling `pg_itoa`
/// writes and no other: an optional `-`, then digits with no leading zero,
/// and never `-0`.
fn canonical_int2(token: &str) -> Option<i16> {
    let digits = token.strip_prefix('-').unwrap_or(token);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    if digits == "0" && token.starts_with('-') {
        return None;
    }
    token.parse().ok()
}

/// Decode an `int2vector` value as `int2vectorout` writes it: the elements'
/// decimal spellings joined by **one** space, and the empty string for the
/// empty vector (I47).
///
/// A fourth container form sharing nothing with the other three — no wrapper,
/// no separator an element could contain, no quoting, no escaping and no NULL
/// element — so it needs neither a [`Syntax`] nor a token scanner.
///
/// Strict, as every `decode_*` here is: an element is read only in the
/// spelling `pg_itoa` writes, so `+1`, `01`, `-0` and a doubled space are all
/// refused. [`parse_int2vector`] is the input side and takes all four.
///
/// [`parse_int2vector`]: super::input::parse_int2vector
pub fn decode_int2vector(s: &str) -> Option<Vec<i16>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    s.split(' ').map(canonical_int2).collect()
}

/// `int2vectorout`, and the exact inverse of [`decode_int2vector`].
pub fn render_int2vector(values: &[i16]) -> String {
    let mut out = String::new();
    for (k, value) in values.iter().enumerate() {
        if k > 0 {
            out.push(' ');
        }
        out.push_str(&value.to_string());
    }
    out
}

#[cfg(test)]
mod tests;
