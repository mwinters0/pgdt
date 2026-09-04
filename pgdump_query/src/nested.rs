//! The nested literal codec: array, composite (record), range, multirange and
//! `int2vector` literals, in both directions — as PostgreSQL's `*_out`
//! functions write them inside a single `COPY` field, and as its `*_in`
//! functions read what a user typed.
//!
//! Pure, synchronous, no I/O and no Arrow — see `docs/design/layering.md`, L2.
//! Like `crate::decode`, every function here works on the *already
//! COPY-unescaped* text `crate::copy::decode_field` returns, and every
//! `render_*` is the exact inverse of its `decode_*`: feeding it a value
//! decoded from real `pg_dump` output reproduces that output byte for byte.
//! Turning the result back into on-disk bytes is `crate::copy::encode_field`'s
//! job, not this module's.
//!
//! **`decode_*` and `parse_*` are two scanners, not one with a flag.** The
//! first reads a dump and is deliberately strict; the second reads a filter's
//! literal and is deliberately permissive. The strictness is what makes decode
//! and render inverses, so it cannot be loosened, and the permissiveness is
//! what makes `tags={a, b}` mean what it looks like, so it cannot be
//! tightened.
//!
//! # The output side: one scanner, three parameter sets
//!
//! The three container forms are *not* one quoting rule wearing three hats
//! (I20, `docs/design/postgres-invariants.md`). They share a shape — a
//! wrapper, a separator, a force-quote predicate and an escape convention —
//! and disagree on every one of the last three:
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
//! scope limit; the input half below is I44). Whitespace padding around an
//! element, an unquoted token containing a backslash escape, a nested `{}` —
//! all things `*_in` accepts and `*_out` never emits — are rejected rather than
//! guessed at. The one deliberate leniency is that a bare `NULL` array element
//! is matched case-insensitively, as `array_in` does: `array_out` force-quotes
//! any element whose text *is* `null` in any casing, so agreeing with
//! PostgreSQL here can never misread real output.
//!
//! The separator is hardcoded to `,`. An array whose element type sets a
//! different `typdelim` (`box`, or any C-level base type) is not decoded as an
//! array at all — see `docs/design/architecture.md`, "Type resolution".

use std::borrow::Cow;

/// How a quoted token escapes an embedded `"`. Both conventions escape an
/// embedded `\` by doubling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    /// `"` → `\"` — `array_out`.
    Backslash,
    /// `"` → `""` — `record_out` and `range_bound_escape`.
    Double,
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
    force_quote: &'static [u8],
    /// True where a bare `NULL` token means SQL NULL and an empty token is
    /// malformed (`array_out`); false where SQL NULL is spelled as nothing at
    /// all and `""` is the empty string (`record_out`, `range_bound_escape`).
    bare_null: bool,
}

const ARRAY: Syntax =
    Syntax { separator: b',', escape: Escape::Backslash, force_quote: b"\"\\{},", bare_null: true };

const RECORD: Syntax =
    Syntax { separator: b',', escape: Escape::Double, force_quote: b"\"\\(),", bare_null: false };

const RANGE_BOUND: Syntax =
    Syntax { separator: b',', escape: Escape::Double, force_quote: b"\"\\()[],", bare_null: false };

/// `array_isspace`, and `isspace()` in the C locale: the same six characters,
/// which is why one predicate serves all three forms.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn needs_quote(value: &str, syntax: &Syntax) -> bool {
    if value.is_empty() {
        return true;
    }
    if syntax.bare_null && value.eq_ignore_ascii_case("NULL") {
        return true;
    }
    value.bytes().any(|c| syntax.force_quote.contains(&c) || is_space(c))
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

/// Consume a `"`-delimited token starting at the opening quote, returning its
/// unescaped text and the index just past the closing quote.
///
/// **Borrowed unless the token actually carries an escape.** The first pass
/// looks for the closing quote and copies nothing; a `\` or a doubled `""`
/// aborts it into the second, which rebuilds the token from the bytes already
/// walked. Real `pg_dump` output quotes far more tokens than it escapes — a
/// value holding a space or a separator is quoted with nothing inside to undo
/// — so the borrowed arm is the common one even here.
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
    // matching `*_out` function wrote, so accepting it would break the one
    // property this module promises: that decode and render are inverses.
    if needs_quote(text, syntax) {
        return None;
    }
    Some((Some(Cow::Borrowed(text)), i))
}

/// An `array_out` literal: elements flattened row-major, plus the shape they
/// were written in.
///
/// Dimensionality and lower bounds belong to the value, never to the column
/// (I21) — `integer[][]`, `integer[3]` and `integer[]` are all written
/// `integer[]`, and consecutive rows of one column may legitimately disagree.
/// That is what this type exists to carry.
///
/// **An element borrows from the literal wherever it can**, which is why this
/// type carries a lifetime. `array_out` writes most elements verbatim — an
/// escape appears only inside a quoted token that held a `"` or a `\` — so
/// the common element is a slice of the field and only the rare escaped one
/// is copied. The alternative, a `String` per element, is what
/// `docs/design/measurements.md`, "Nested decode costs what it copies",
/// measured at 77 ns of the per-element decode slope against the 48 ns the
/// scan alone costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayLiteral<'a> {
    /// Row-major, flattened across every dimension. `None` is a SQL NULL
    /// element (which an array spells as a bare `NULL`, distinguishing it from
    /// the *string* `NULL` by quoting alone).
    pub elements: Vec<Option<Cow<'a, str>>>,
    /// One entry per dimension. Empty for `{}`, which `array_out` emits for a
    /// zero-element array whatever its dimensionality (I20).
    pub dims: Vec<usize>,
    /// One entry per dimension, all `1` unless the literal carried an
    /// `[lb:ub]=` prefix. Empty exactly when `dims` is.
    pub lower_bounds: Vec<i32>,
}

impl ArrayLiteral<'_> {
    /// Number of dimensions; `0` for the empty array.
    pub fn ndim(&self) -> usize {
        self.dims.len()
    }

    /// Whether the literal carried (and will re-render) an `[lb:ub]=` prefix.
    /// Arrow lists are 0-based and have no lower bound, so a decorated value
    /// has no faithful `List` representation.
    pub fn is_decorated(&self) -> bool {
        self.lower_bounds.iter().any(|lb| *lb != 1)
    }
}

/// The elements of one array literal, row-major and flattened across every
/// dimension. Borrowed from the literal wherever no escape had to be undone —
/// see [`ArrayLiteral`], whose field this is.
type Elements<'a> = Vec<Option<Cow<'a, str>>>;

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

    // `[lb:ub]` per dimension, then `=`, present iff some lower bound is not 1.
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

/// A `record_out` literal: one entry per declared field of the composite, in
/// declaration order. `None` is SQL NULL, which a record spells as nothing at
/// all between its separators — `("")` is the empty string, `()` is NULL.
///
/// Arity is not checked here: this module does not know the composite's
/// declared field list. The caller joins the two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordLiteral {
    pub fields: Vec<Option<String>>,
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

/// A `range_out` literal.
///
/// `empty` is not redundant with the bounds: `empty` and `(,)` are different
/// ranges and both have absent bounds, so the inclusivity flags alone cannot
/// tell them apart. A bound can never be SQL NULL, which is what lets `None`
/// mean "unbounded" without ambiguity — `["",a)` has an empty-string lower
/// bound, `(,a)` has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeLiteral {
    pub empty: bool,
    pub lower: Option<String>,
    pub upper: Option<String>,
    pub lower_inclusive: bool,
    pub upper_inclusive: bool,
}

impl RangeLiteral {
    /// The canonical empty range, the only shape `range_out` writes as a bare
    /// word rather than a bracketed pair.
    pub fn empty() -> Self {
        RangeLiteral {
            empty: true,
            lower: None,
            upper: None,
            lower_inclusive: false,
            upper_inclusive: false,
        }
    }
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
/// It is a fourth container form and it shares nothing with the other three —
/// no wrapper, no separator an element could contain, no quoting, no escaping
/// and no NULL element, `int2vector` having no way to hold one. So it needs
/// neither a [`Syntax`] nor a token scanner, and gets neither.
///
/// Strict, as every `decode_*` here is: an element is read only in the
/// spelling `pg_itoa` writes, so `+1`, `01`, `-0` and a doubled space are all
/// refused rather than guessed at. [`parse_int2vector`] is the input side and
/// takes all four.
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

// ---------------------------------------------------------------------------
// The literal side: the `*_in` supersets
// ---------------------------------------------------------------------------
//
// Everything above reads what a dump *holds*. What follows reads what a user
// *typed* — a filter's right-hand side, which the `*_in` functions accept a
// good deal more of than the matching `*_out` ever writes (I44). The two are
// deliberately separate scanners: `decode_*` may not loosen, because its
// strictness is what makes it and `render_*` inverses, and `parse_*` may not
// tighten, because refusing `{a, b}` makes a typed nested filter worse to use
// than the text comparison it replaces.
//
// **They are four grammars, not one**, and whitespace is where they first
// disagree: `array_in` drops unquoted whitespace around an element, while
// `record_in` and `range_in` keep every byte of it, so `( 1 , a )` into a
// composite is a two-field value whose second field is `" a "`. One predicate
// still serves all four, because `array_isspace`, `scanner_isspace` and the C
// locale's `isspace` are the same six characters.
//
// The result is the same `ArrayLiteral`/`RecordLiteral`/`RangeLiteral` the
// decoders produce, holding the parts **as the user spelled them**: an
// element is whatever text the server would have handed the element type's own
// `*_in`, not that type's canonical output. Putting the two sides of a
// comparison into one spelling is the caller's job, since only the caller
// knows the element type.

/// PostgreSQL's `MAXDIM`: the most dimensions an array value may have.
/// `crate::index::MAX_ARRAY_DIMS` is the same number for the shape census;
/// it is a `u8` there and this is an index bound, and neither module is worth
/// a dependency edge on the other for one constant.
const MAXDIM: usize = 6;

/// One token of an `array_in` literal, as `ReadArrayToken` classifies them.
enum ArrayToken {
    LevelStart,
    LevelEnd,
    Delim,
    Elem(String),
    ElemNull,
}

/// Read one `array_in` token, skipping the whitespace ahead of it. `None` is
/// a malformed literal, here and in every helper below.
fn read_array_token(s: &[u8], mut i: usize) -> Option<(ArrayToken, usize)> {
    loop {
        match *s.get(i)? {
            b'{' => return Some((ArrayToken::LevelStart, i + 1)),
            b'}' => return Some((ArrayToken::LevelEnd, i + 1)),
            b'"' => return read_quoted_array_element(s, i + 1),
            b',' => return Some((ArrayToken::Delim, i + 1)),
            c if is_space(c) => i += 1,
            _ => return read_unquoted_array_element(s, i),
        }
    }
}

/// A `"`-delimited array element, from just past the opening quote. Nothing
/// but whitespace may follow the closing quote before a separator or a brace:
/// `{"a"b}` is "incorrectly quoted", not a two-part element. A quoted element
/// is never the `NULL` marker, whatever it spells.
fn read_quoted_array_element(s: &[u8], mut i: usize) -> Option<(ArrayToken, usize)> {
    let mut out = Vec::new();
    loop {
        match *s.get(i)? {
            b'\\' => {
                out.push(*s.get(i + 1)?);
                i += 2;
            }
            b'"' => {
                i += 1;
                loop {
                    match *s.get(i)? {
                        b',' | b'}' | b'{' => {
                            return Some((ArrayToken::Elem(String::from_utf8(out).ok()?), i));
                        }
                        c if is_space(c) => i += 1,
                        _ => return None,
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
}

/// An unquoted array element. Trailing whitespace is dropped and interior
/// whitespace kept, which `keep` tracks; a `\` escape counts as
/// non-whitespace and also disqualifies the `NULL` marker, so `{N\ULL}` is
/// the *string* `NULL` and `{null}` is SQL NULL.
fn read_unquoted_array_element(s: &[u8], mut i: usize) -> Option<(ArrayToken, usize)> {
    let mut out = Vec::new();
    let mut keep = 0usize;
    let mut escaped = false;
    loop {
        match *s.get(i)? {
            // An element is quoted in full or not at all, and a `{` here is a
            // brace where a value was expected.
            b'{' | b'"' => return None,
            b'\\' => {
                out.push(*s.get(i + 1)?);
                i += 2;
                keep = out.len();
                escaped = true;
            }
            b',' | b'}' => {
                out.truncate(keep);
                let text = String::from_utf8(out).ok()?;
                if !escaped && text.eq_ignore_ascii_case("NULL") {
                    return Some((ArrayToken::ElemNull, i));
                }
                return Some((ArrayToken::Elem(text), i));
            }
            c => {
                out.push(c);
                if !is_space(c) {
                    keep = out.len();
                }
                i += 1;
            }
        }
    }
}

/// Walk the brace structure from the opening `{`, returning the dimensionality
/// it turned out to have, the elements row-major, and the index just past the
/// matching `}`.
///
/// `dim` is `Some` per dimension where the literal carried an `[lb:ub]=`
/// decoration and `None` where the structure has to say — the same array
/// serving as both input and output, which is what lets one walk check a
/// declared shape and deduce an undeclared one. `ndim` is `0` when nothing was
/// declared.
fn read_array_body<'a>(
    s: &'a [u8],
    mut i: usize,
    mut ndim: usize,
    dim: &mut [Option<usize>; MAXDIM],
) -> Option<(usize, Elements<'a>, usize)> {
    // Once a dimensionality is declared, or an element has been seen, the
    // nesting may not get deeper.
    let mut frozen = ndim != 0;
    let mut expect_delim = false;
    let mut nest_level = 0usize;
    let mut nelems = [0usize; MAXDIM];
    let mut elements = Vec::new();
    loop {
        let (token, next) = read_array_token(s, i)?;
        i = next;
        match token {
            ArrayToken::LevelStart => {
                if expect_delim || nest_level >= MAXDIM {
                    return None;
                }
                nelems[nest_level] = 0;
                nest_level += 1;
                if nest_level > ndim {
                    if frozen {
                        return None;
                    }
                    ndim = nest_level;
                }
            }
            ArrayToken::LevelEnd => {
                if nest_level == 0 {
                    return None;
                }
                // A `}` may close an empty sub-array; otherwise it stands
                // where a separator would have.
                if nelems[nest_level - 1] > 0 && !expect_delim {
                    return None;
                }
                nest_level -= 1;
                if nest_level > 0 {
                    nelems[nest_level - 1] += 1;
                }
                match dim[nest_level] {
                    None => dim[nest_level] = Some(nelems[nest_level]),
                    Some(n) if n == nelems[nest_level] => {}
                    Some(_) => return None,
                }
                expect_delim = true;
            }
            ArrayToken::Delim => {
                if !expect_delim {
                    return None;
                }
                expect_delim = false;
            }
            ArrayToken::Elem(_) | ArrayToken::ElemNull => {
                if expect_delim || nest_level == 0 {
                    return None;
                }
                elements.push(match token {
                    // Owned unconditionally: `array_in` trims, unescapes and
                    // re-cases, so a token here is rarely the bytes the user
                    // typed. This grammar reads one filter literal per query
                    // rather than one per row, so the borrow is not worth the
                    // second scanner it would need.
                    ArrayToken::Elem(value) => Some(Cow::Owned(value)),
                    _ => None,
                });
                frozen = true;
                // Every element sits at the same depth, or the literal is
                // ragged.
                if nest_level != ndim {
                    return None;
                }
                nelems[nest_level - 1] += 1;
                expect_delim = true;
            }
        }
        if nest_level == 0 {
            return Some((ndim, elements, i));
        }
    }
}

/// One `[m:n]` bound. `ReadDimensionInt` refuses leading whitespace and then
/// hands the rest to `strtol`, so a sign is accepted and a bare sign is not.
fn parse_dimension_int(s: &[u8], mut i: usize) -> Option<(i32, usize)> {
    let start = i;
    if matches!(s.get(i), Some(b'-') | Some(b'+')) {
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
    Some((text.trim_start_matches('+').parse().ok()?, i))
}

fn skip_space(s: &[u8], mut i: usize) -> usize {
    while s.get(i).is_some_and(|c| is_space(*c)) {
        i += 1;
    }
    i
}

/// Parse an `array_in` literal — everything the server accepts for an array
/// column, which is a strict superset of what [`decode_array`] reads.
///
/// The elements come back in the spelling the literal used, so
/// `render_array(parse_array(x))` is `array_out`'s answer only where the
/// element type's own `*_in` and `*_out` agree on every element; anywhere else
/// the caller has to canonicalize per element, which needs the element type.
///
/// **A literal with no elements is the zero-dimensional empty array**, however
/// it was written: `{}`, `{ }` and (on v17+) `{{},{}}` are all the value
/// `array_out` writes as `{}`.
pub fn parse_array(s: &str) -> Option<ArrayLiteral<'_>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut dim = [None; MAXDIM];
    let mut lower = [1i32; MAXDIM];
    let mut declared = 0usize;

    // Zero or more `[n]` or `[m:n]` items, with whitespace allowed between
    // them but not inside one.
    loop {
        i = skip_space(b, i);
        if b.get(i) != Some(&b'[') {
            break;
        }
        i += 1;
        if declared >= MAXDIM {
            return None;
        }
        let (first, next) = parse_dimension_int(b, i)?;
        i = next;
        let (lb, ub) = if b.get(i) == Some(&b':') {
            let (ub, next) = parse_dimension_int(b, i + 1)?;
            i = next;
            (first, ub)
        } else {
            (1, first)
        };
        if b.get(i) != Some(&b']') {
            return None;
        }
        i += 1;
        // A zero-length dimension is refused rather than collapsed, and an
        // upper bound of `INT_MAX` is refused outright.
        if ub < lb || ub == i32::MAX {
            return None;
        }
        dim[declared] = Some(usize::try_from(i64::from(ub) - i64::from(lb) + 1).ok()?);
        lower[declared] = lb;
        declared += 1;
    }

    if declared > 0 {
        if b.get(i) != Some(&b'=') {
            return None;
        }
        i = skip_space(b, i + 1);
    }
    if b.get(i) != Some(&b'{') {
        return None;
    }

    let (ndim, elements, next) = read_array_body(b, i, declared, &mut dim)?;
    if skip_space(b, next) != b.len() {
        return None;
    }

    if elements.is_empty() {
        return Some(ArrayLiteral { elements, dims: Vec::new(), lower_bounds: Vec::new() });
    }
    let dims = dim[..ndim].iter().copied().collect::<Option<Vec<usize>>>()?;
    if dims.iter().product::<usize>() != elements.len() {
        return None;
    }
    Some(ArrayLiteral { elements, dims, lower_bounds: lower[..ndim].to_vec() })
}

/// One `record_in` field or `range_in` bound: every byte up to the first
/// terminator outside quotes, with `\` taking the next byte literally and `""`
/// inside quotes meaning one `"`. Quoting may start and stop anywhere, so
/// `["a"b,c)` has the lower bound `ab`.
///
/// **Whitespace is never trimmed.** This is the half `array_in` does
/// differently, and reusing an array-shaped scanner here is what silently
/// eats a composite field's leading blank.
fn read_quoted_run(s: &[u8], mut i: usize, terminators: &[u8]) -> Option<(String, usize)> {
    let mut out = Vec::new();
    let mut quoted = false;
    while quoted || !terminators.contains(s.get(i)?) {
        match *s.get(i)? {
            b'\\' => {
                out.push(*s.get(i + 1)?);
                i += 2;
            }
            b'"' if !quoted => {
                quoted = true;
                i += 1;
            }
            b'"' if s.get(i + 1) == Some(&b'"') => {
                out.push(b'"');
                i += 2;
            }
            b'"' => {
                quoted = false;
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some((String::from_utf8(out).ok()?, i))
}

/// Parse a `record_in` literal for a composite with `columns` declared fields.
///
/// **Arity is a parameter because `record_in` checks it**, which is the one
/// place this differs from [`decode_record`]: the server reads exactly
/// `columns` fields and calls anything else "Too few columns" or "Too many
/// columns", so `()` is a value for a zero-field composite and for a
/// one-field one holding NULL, and a fault for every other arity. A caller
/// that does not know the field list cannot ask this question, and neither
/// can the server.
pub fn parse_record(s: &str, columns: usize) -> Option<RecordLiteral> {
    let b = s.as_bytes();
    let mut i = skip_space(b, 0);
    if b.get(i) != Some(&b'(') {
        return None;
    }
    i += 1;
    let mut fields = Vec::with_capacity(columns);
    for k in 0..columns {
        if k > 0 {
            if b.get(i) != Some(&b',') {
                return None;
            }
            i += 1;
        }
        // Nothing at all between the separators is SQL NULL; `""` is the
        // empty string.
        if matches!(b.get(i), Some(b',') | Some(b')')) {
            fields.push(None);
            continue;
        }
        let (value, next) = read_quoted_run(b, i, b",)")?;
        fields.push(Some(value));
        i = next;
    }
    if b.get(i) != Some(&b')') {
        return None;
    }
    (skip_space(b, i + 1) == b.len()).then_some(RecordLiteral { fields })
}

/// Parse a `range_in` literal. `empty` is matched case-insensitively and may
/// carry whitespace on either side; the bounds keep theirs.
///
/// The bounds are **not canonicalized** — `[1,10]` stays inclusive-inclusive
/// here, where the server would rewrite it to `[1,11)` for a discrete subtype.
/// That needs the subtype's own successor function, which this module does not
/// have.
pub fn parse_range(s: &str) -> Option<RangeLiteral> {
    let b = s.as_bytes();
    let i = skip_space(b, 0);
    if b.get(i..i + 5).is_some_and(|word| word.eq_ignore_ascii_case(b"empty")) {
        return (skip_space(b, i + 5) == b.len()).then(RangeLiteral::empty);
    }
    let lower_inclusive = match *b.get(i)? {
        b'[' => true,
        b'(' => false,
        _ => return None,
    };
    let (lower, next) = read_range_bound(b, i + 1)?;
    if b.get(next) != Some(&b',') {
        return None;
    }
    let (upper, next) = read_range_bound(b, next + 1)?;
    let upper_inclusive = match *b.get(next)? {
        b']' => true,
        b')' => false,
        _ => return None,
    };
    (skip_space(b, next + 1) == b.len()).then_some(RangeLiteral {
        empty: false,
        lower,
        upper,
        lower_inclusive,
        upper_inclusive,
    })
}

/// One range bound. Completely empty input is the unbounded bound, which is
/// what makes `(,a)` and `["",a)` different values.
fn read_range_bound(s: &[u8], i: usize) -> Option<(Option<String>, usize)> {
    if matches!(s.get(i)?, b',' | b')' | b']') {
        return Some((None, i));
    }
    let (value, next) = read_quoted_run(s, i, b",)]")?;
    Some((Some(value), next))
}

/// Where a `multirange_in` scan stands between its members.
enum MultirangeState {
    BeforeRange,
    InRange,
    InRangeEscaped,
    InRangeQuoted,
    InRangeQuotedEscaped,
    AfterRange,
}

/// Parse a `multirange_in` literal into its member ranges.
///
/// The scan finds each member's extent and hands the substring to
/// [`parse_range`], exactly as the server hands it to `range_in` — so a
/// member's own brackets and quoting are what delimit it, and whitespace
/// between members is dropped while whitespace inside a bound is kept.
///
/// **A member spelled `empty` is accepted and dropped**, which
/// [`decode_multirange`] refuses because `multirange_out` never writes one. A
/// member that is *equivalent* to empty — `[1,1)` — is kept here and dropped
/// by the server, which is the same missing successor function
/// [`parse_range`] records.
pub fn parse_multirange(s: &str) -> Option<Vec<RangeLiteral>> {
    let b = s.as_bytes();
    let mut i = skip_space(b, 0);
    if b.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;

    let mut state = MultirangeState::BeforeRange;
    let mut members = Vec::new();
    let mut seen = 0usize;
    let mut start = 0usize;
    let mut finished = false;
    while !finished {
        let c = *b.get(i)?;
        // Whitespace is skipped in every state, member text included — the
        // member is a substring, so its own blanks survive anyway.
        if is_space(c) {
            i += 1;
            continue;
        }
        match state {
            MultirangeState::BeforeRange => {
                if c == b'[' || c == b'(' {
                    start = i;
                    state = MultirangeState::InRange;
                } else if c == b'}' && seen == 0 {
                    finished = true;
                } else if b.get(i..i + 5).is_some_and(|w| w.eq_ignore_ascii_case(b"empty")) {
                    seen += 1;
                    i += 4;
                    state = MultirangeState::AfterRange;
                } else {
                    return None;
                }
            }
            MultirangeState::InRange => {
                if c == b']' || c == b')' {
                    let member = parse_range(std::str::from_utf8(&b[start..=i]).ok()?)?;
                    seen += 1;
                    if !member.empty {
                        members.push(member);
                    }
                    state = MultirangeState::AfterRange;
                } else if c == b'"' {
                    state = MultirangeState::InRangeQuoted;
                } else if c == b'\\' {
                    state = MultirangeState::InRangeEscaped;
                }
            }
            MultirangeState::InRangeEscaped => state = MultirangeState::InRange,
            MultirangeState::InRangeQuoted => {
                if c == b'"' {
                    if b.get(i + 1) == Some(&b'"') {
                        i += 1;
                    } else {
                        state = MultirangeState::InRange;
                    }
                } else if c == b'\\' {
                    state = MultirangeState::InRangeQuotedEscaped;
                }
            }
            MultirangeState::InRangeQuotedEscaped => state = MultirangeState::InRangeQuoted,
            MultirangeState::AfterRange => {
                if c == b',' {
                    state = MultirangeState::BeforeRange;
                } else if c == b'}' {
                    finished = true;
                } else {
                    return None;
                }
            }
        }
        i += 1;
    }
    (skip_space(b, i) == b.len()).then_some(members)
}

/// Parse an `int2vectorin` literal — everything the server accepts for an
/// `int2vector` column, which is a strict superset of what
/// [`decode_int2vector`] reads (I47).
///
/// `int2vectorin` skips any run of whitespace before an element, hands what
/// follows to `strtol` — so a leading `+` and a leading zero are taken —
/// range-checks the result against `int16`, and then requires the byte
/// *after* the number to be a space or the end of the string. That last rule
/// is the one worth reproducing rather than smoothing into "any whitespace
/// separates": `\t1 2` is accepted and `1\t2` is not.
pub fn parse_int2vector(s: &str) -> Option<Vec<i16>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    loop {
        i = skip_space(b, i);
        if i == b.len() {
            return Some(out);
        }
        let start = i;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let digits = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == digits {
            return None;
        }
        let text = std::str::from_utf8(&b[start..i]).ok()?;
        // `strtol`'s own `ERANGE`, then `int2vectorin`'s explicit
        // `SHRT_MIN`/`SHRT_MAX` check — one refusal here, since both say the
        // literal is not a value of this type.
        let value: i64 = text.trim_start_matches('+').parse().ok()?;
        out.push(i16::try_from(value).ok()?);
        match b.get(i) {
            None | Some(b' ') => {}
            Some(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    /// One array element. Borrowed, because `Cow` compares by its contents:
    /// an assertion built with this passes against either arm, and which arm
    /// a decode actually took is asserted on its own below.
    fn elem(v: &str) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(v))
    }

    /// The whole point of the module: decode then render must be the identity
    /// on anything a `*_out` function can produce.
    #[track_caller]
    fn array_round_trips(literal: &str) -> ArrayLiteral<'_> {
        let decoded = decode_array(literal).unwrap_or_else(|| panic!("decode failed: {literal}"));
        assert_eq!(render_array(&decoded), literal);
        decoded
    }

    #[test]
    fn one_dimensional_arrays_round_trip_with_every_null_and_quoting_case() {
        let a = array_round_trips("{1,2,3}");
        assert_eq!(a.elements, vec![elem("1"), elem("2"), elem("3")]);
        assert_eq!(a.dims, vec![3]);
        assert_eq!(a.ndim(), 1);

        // `{}`, `{NULL}` and a one-element array holding the *string* `NULL`
        // are three different values that look alike.
        assert_eq!(array_round_trips("{}").elements, Vec::<Option<Cow<str>>>::new());
        assert_eq!(array_round_trips("{NULL}").elements, vec![None]);
        assert_eq!(array_round_trips("{\"NULL\"}").elements, vec![elem("NULL")]);
        assert_eq!(array_round_trips("{\"\"}").elements, vec![elem("")]);
        assert_eq!(array_round_trips("{1,NULL,3}").elements, vec![elem("1"), None, elem("3")]);
    }

    #[test]
    fn an_array_backslash_escapes_inside_a_quoted_element() {
        let a = array_round_trips(r#"{"a,b","c{d}","e\"f","g\\h"}"#);
        assert_eq!(a.elements, vec![elem("a,b"), elem("c{d}"), elem(r#"e"f"#), elem(r"g\h")]);
    }

    /// The borrowed arm is what slice 7.9 is: an element is a slice of the
    /// literal unless it actually carried an escape, and a *quoted* element
    /// with nothing to undo is still borrowed. Asserted on the arm rather
    /// than on the text, since `Cow`'s own equality cannot tell them apart.
    #[test]
    fn an_element_is_copied_only_where_the_literal_escaped_it() {
        let plain = decode_array(r#"{1,"a,b","c{d}",NULL,""}"#).unwrap();
        assert!(
            plain.elements.iter().flatten().all(|e| matches!(e, Cow::Borrowed(_))),
            "nothing here escapes: {:?}",
            plain.elements
        );

        // `\"` and `\\` are the only two escapes `array_out` writes, and each
        // one is what forces the copy.
        let escaped = decode_array(r#"{"e\"f","g\\h",plain}"#).unwrap();
        assert!(matches!(escaped.elements[0], Some(Cow::Owned(_))));
        assert!(matches!(escaped.elements[1], Some(Cow::Owned(_))));
        assert!(matches!(escaped.elements[2], Some(Cow::Borrowed(_))));

        // A record's doubled `""` is the other escape convention, and it is
        // the same split — `scan_token` serves all three forms.
        let (doubled, _) = scan_token(br#""a""b","#, 0, &RECORD, b")").unwrap();
        assert!(matches!(doubled, Some(Cow::Owned(_))));
    }

    #[test]
    fn whitespace_and_the_delimiter_force_quotes_on_render() {
        let a = ArrayLiteral {
            elements: vec![elem("has space"), elem("has,comma"), elem("has'quote"), elem("plain")],
            dims: vec![4],
            lower_bounds: vec![1],
        };
        assert_eq!(render_array(&a), r#"{"has space","has,comma",has'quote,plain}"#);
        assert_eq!(decode_array(&render_array(&a)).unwrap(), a);
    }

    #[test]
    fn multi_dimensional_arrays_keep_their_shape_and_flatten_row_major() {
        let a = array_round_trips("{{1,2},{3,4}}");
        assert_eq!(a.dims, vec![2, 2]);
        assert_eq!(a.ndim(), 2);
        assert_eq!(a.elements, vec![elem("1"), elem("2"), elem("3"), elem("4")]);

        let deep = array_round_trips("{{{1},{2}},{{3},{4}}}");
        assert_eq!(deep.dims, vec![2, 2, 1]);
        assert_eq!(deep.elements, vec![elem("1"), elem("2"), elem("3"), elem("4")]);
    }

    #[test]
    fn a_lower_bound_prefix_survives_the_round_trip() {
        let a = array_round_trips("[0:2]={7,8,9}");
        assert_eq!(a.lower_bounds, vec![0]);
        assert_eq!(a.dims, vec![3]);
        assert!(a.is_decorated());

        let negative = array_round_trips("[-1:0]={10,11}");
        assert_eq!(negative.lower_bounds, vec![-1]);

        let two_d = array_round_trips("[0:1][1:2]={{1,2},{3,4}}");
        assert_eq!(two_d.lower_bounds, vec![0, 1]);
        assert_eq!(two_d.dims, vec![2, 2]);

        // Every bound 1 means no prefix at all, which is how `array_out`
        // decides — so the flag is about the bounds, not about how the value
        // was built.
        assert!(!array_round_trips("{1,2}").is_decorated());
    }

    #[test]
    fn ragged_and_mixed_depth_arrays_are_rejected_rather_than_guessed_at() {
        for bad in [
            "{{1},{2,3}}", // sibling lists of different lengths
            "{{1,2},3}",   // a list and a token at the same depth
            "{1,{2}}",     // the same, in the other order
            "{{},{}}",     // an inner empty list `array_out` cannot emit
            "{1,2",        // unterminated
            "{1,2}}",      // trailing junk
            "{ 1,2}",      // `array_in` padding, which `array_out` never writes
            "{a\"b}",      // a quote inside an unquoted element
            "[0:1]{1,2}",  // decoration with no `=`
            "[0:2]={1,2}", // decoration disagreeing with the element count
            "[0:-1]={}",   // `array_out` never decorates the empty array
            "{,}",         // an empty unquoted element
        ] {
            assert!(decode_array(bad).is_none(), "should not decode: {bad}");
        }
    }

    #[test]
    fn records_double_their_quotes_and_spell_null_as_nothing() {
        for (literal, fields) in [
            (r#"(1,"a,b""c")"#, vec![some("1"), some(r#"a,b"c"#)]),
            ("()", vec![None]),
            (r#"(,"")"#, vec![None, some("")]),
            ("(3,)", vec![some("3"), None]),
            // A record does *not* force-quote the string `NULL` — it does not
            // have to, since its own NULL is empty rather than a word.
            ("(NULL)", vec![some("NULL")]),
            (r#"("a\\b")"#, vec![some(r"a\b")]),
        ] {
            let decoded = decode_record(literal).unwrap_or_else(|| panic!("{literal}"));
            assert_eq!(decoded.fields, fields, "{literal}");
            assert_eq!(render_record(&decoded), literal, "{literal}");
        }
    }

    #[test]
    fn a_composite_inside_an_array_carries_both_escape_conventions_at_once() {
        // The array layer comes off first, exposing the record literal.
        let outer = array_round_trips(r#"{"(1,\"a,b\"\"c\")","(2,plain)"}"#);
        assert_eq!(outer.elements[0].as_deref(), Some(r#"(1,"a,b""c")"#));
        let inner = decode_record(outer.elements[0].as_deref().unwrap()).unwrap();
        assert_eq!(inner.fields, vec![some("1"), some(r#"a,b"c"#)]);
        assert_eq!(render_record(&inner), outer.elements[0].clone().unwrap());

        // And the other nesting order: a record whose field is an array.
        let literal = r#"("a,b","{""x\\""y"",""p q"",NULL}")"#;
        let record = decode_record(literal).unwrap();
        assert_eq!(record.fields[1].as_deref(), Some(r#"{"x\"y","p q",NULL}"#));
        let field = decode_array(record.fields[1].as_deref().unwrap()).unwrap();
        assert_eq!(field.elements, vec![elem(r#"x"y"#), elem("p q"), None]);
        assert_eq!(render_record(&record), literal);
    }

    #[test]
    fn ranges_tell_an_absent_bound_from_an_empty_one() {
        for (literal, expected) in [
            (
                "[1,10)",
                RangeLiteral {
                    empty: false,
                    lower: some("1"),
                    upper: some("10"),
                    lower_inclusive: true,
                    upper_inclusive: false,
                },
            ),
            ("empty", RangeLiteral::empty()),
            (
                "(,5)",
                RangeLiteral {
                    empty: false,
                    lower: None,
                    upper: some("5"),
                    lower_inclusive: false,
                    upper_inclusive: false,
                },
            ),
            (
                r#"["",a)"#,
                RangeLiteral {
                    empty: false,
                    lower: some(""),
                    upper: some("a"),
                    lower_inclusive: true,
                    upper_inclusive: false,
                },
            ),
            (
                r#"["a,b","c""d"]"#,
                RangeLiteral {
                    empty: false,
                    lower: some("a,b"),
                    upper: some(r#"c"d"#),
                    lower_inclusive: true,
                    upper_inclusive: true,
                },
            ),
        ] {
            let decoded = decode_range(literal).unwrap_or_else(|| panic!("{literal}"));
            assert_eq!(decoded, expected, "{literal}");
            assert_eq!(render_range(&decoded), literal, "{literal}");
        }

        // `empty` and `(,)` both have absent bounds and differ only in the
        // flag — which is why the flag exists.
        assert_ne!(decode_range("empty").unwrap(), decode_range("(,)").unwrap());
    }

    #[test]
    fn malformed_ranges_and_records_are_rejected() {
        for bad in ["[1,10", "1,10)", "[1,10)x", "[1)", "[1,2,3)", ""] {
            assert!(decode_range(bad).is_none(), "should not decode as a range: {bad}");
        }
        for bad in ["(1,2", "1,2)", "(1,2))", "(\"a)", ""] {
            assert!(decode_record(bad).is_none(), "should not decode as a record: {bad}");
        }
    }

    #[test]
    fn multiranges_split_on_their_members_brackets_not_on_commas() {
        for (literal, count) in [("{}", 0), ("{[1,10)}", 1), ("{[1,2),[5,6)}", 2)] {
            let decoded = decode_multirange(literal).unwrap_or_else(|| panic!("{literal}"));
            assert_eq!(decoded.len(), count, "{literal}");
            assert_eq!(render_multirange(&decoded), literal, "{literal}");
        }

        // A member's own bound may contain the separator and a bracket, which
        // is exactly why the split honours quoting.
        let literal = r#"{["a,b","c)d"),[x,y]}"#;
        let decoded = decode_multirange(literal).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].upper.as_deref(), Some("c)d"));
        assert_eq!(render_multirange(&decoded), literal);

        for bad in ["{[1,2)", "[1,2)}", "{empty}", "{[1,2)x}"] {
            assert!(decode_multirange(bad).is_none(), "should not decode: {bad}");
        }
    }

    // -----------------------------------------------------------------------
    // The `*_in` supersets
    // -----------------------------------------------------------------------
    //
    // Every expectation below was taken from a real server before it was
    // written down — `postgres:18.6-trixie`, one `SELECT textin(<typoutput>(
    // $1::<type>))` per literal inside a PL/pgSQL block returning `'E' ||
    // SQLSTATE` on failure, which is the same shape
    // `scripts/comparison_oracle.py` uses. The committed oracle covers the
    // family and the majors (`tests/nested.rs`); these cover the corners the
    // oracle's case table has no row for.

    #[track_caller]
    fn array_elements(literal: &str) -> Vec<Option<String>> {
        parse_array(literal)
            .unwrap_or_else(|| panic!("should parse: {literal}"))
            .elements
            .into_iter()
            .map(|e| e.map(Cow::into_owned))
            .collect()
    }

    #[test]
    fn array_input_drops_the_whitespace_and_the_quoting_that_output_never_needed() {
        // The whole point: a literal a person would type.
        assert_eq!(array_elements("{ a , b }"), vec![some("a"), some("b")]);
        assert_eq!(array_elements("{\"1\" , \"2\"}"), vec![some("1"), some("2")]);
        assert_eq!(array_elements(" {1,2} "), vec![some("1"), some("2")]);
        assert_eq!(array_elements("{\"a\" }"), vec![some("a")]);
        // Interior whitespace is part of the element; only the trailing run
        // goes.
        assert_eq!(array_elements("{a b}"), vec![some("a b")]);
        assert_eq!(array_elements("{\ta}"), vec![some("a")]);
        assert_eq!(array_elements("{\"a\tb\"}"), vec![some("a\tb")]);
        // A backslash escape is accepted anywhere, which `array_out` only
        // ever writes inside quotes.
        assert_eq!(array_elements(r"{a\,b}"), vec![some("a,b")]);
        assert_eq!(array_elements(r"{a\\b}"), vec![some(r"a\b")]);
        assert_eq!(array_elements("{\"a,b\"}"), vec![some("a,b")]);
    }

    #[test]
    fn an_unquoted_null_is_sql_null_unless_an_escape_disqualifies_it() {
        assert_eq!(array_elements("{null}"), vec![None]);
        assert_eq!(array_elements("{NuLl}"), vec![None]);
        assert_eq!(array_elements("{\"null\"}"), vec![some("null")]);
        // The escape makes it a string even though the bytes still spell
        // `NULL`, which is what `has_escapes` is for.
        assert_eq!(array_elements(r"{N\ULL}"), vec![some("NULL")]);
        assert_eq!(render_array(&parse_array(r"{N\ULL}").unwrap()), "{\"NULL\"}");
    }

    #[test]
    fn array_input_takes_the_dimension_forms_output_never_writes() {
        // `[n]` is the one-sided form, and a decoration whose bounds are all
        // 1 is dropped on the way out.
        for literal in ["[2]={1,2}", "[+1:+2]={1,2}", "[1:2] = {1,2}", "[1:2]={1,2}"] {
            let a = parse_array(literal).unwrap_or_else(|| panic!("{literal}"));
            assert_eq!(a.dims, vec![2], "{literal}");
            assert!(!a.is_decorated(), "{literal}");
            assert_eq!(render_array(&a), "{1,2}", "{literal}");
        }
        let decorated = parse_array("[0:1]={1,2}").unwrap();
        assert_eq!(decorated.lower_bounds, vec![0]);
        assert_eq!(render_array(&decorated), "[0:1]={1,2}");

        assert_eq!(parse_array("{ {1},{2} }").unwrap().dims, vec![2, 1]);
    }

    /// Every array literal with no elements is the zero-dimensional empty
    /// array, whatever brace structure produced it. `{{},{}}` is the one
    /// place two supported majors disagree about the *input* grammar: v13–v16
    /// refuse it and v17+ accept it as `{}` (I44), and implementing the newer
    /// grammar is the phase's union rule.
    #[test]
    fn an_element_less_array_literal_is_the_empty_array_however_it_was_written() {
        for literal in ["{}", "{ }", "{{},{}}"] {
            let a = parse_array(literal).unwrap_or_else(|| panic!("{literal}"));
            assert!(a.elements.is_empty(), "{literal}");
            assert_eq!(a.dims, Vec::<usize>::new(), "{literal}");
            assert_eq!(render_array(&a), "{}", "{literal}");
        }
    }

    #[test]
    fn malformed_array_input_is_refused_rather_than_guessed_at() {
        for bad in [
            "{1,2}x",      // junk after the closing brace
            "{1,{2}}",     // a token and a list at one depth
            "{{1},{2,3}}", // sibling lists of different lengths
            "[1:3]={1,2}", // the decoration disagrees with the contents
            "[1:0]={}",    // a zero-length dimension
            "[0:1]{1,2}",  // a decoration with no `=`
            "{a,}",        // an empty unquoted element
            "{,a}",
            "{\"a\"b}", // quoted in part
            "{a\"b\"}",
            "{\"a\" \"b\"}",
            "{\"a\"{1}}",
            "{\"a\"\"b\"}", // an array escapes `\"`, it does not double
            "{1,2",         // unterminated
            "",
        ] {
            assert!(parse_array(bad).is_none(), "should not parse: {bad}");
        }
    }

    /// The headline disagreement between the three grammars, in one place:
    /// `array_in` drops unquoted whitespace around an element and `record_in`
    /// and `range_in` keep every byte of it. A parser written once against the
    /// array rules and reused eats a composite field's blanks and matches
    /// nothing.
    #[test]
    fn only_the_array_grammar_strips_whitespace_around_a_part() {
        assert_eq!(array_elements("{ 1 , a }"), vec![some("1"), some("a")]);
        assert_eq!(parse_record("( 1 , a )", 2).unwrap().fields, vec![some(" 1 "), some(" a ")]);
        let range = parse_range("[ 1 , a )").unwrap();
        assert_eq!((range.lower, range.upper), (some(" 1 "), some(" a ")));
    }

    #[test]
    fn record_input_reads_exactly_the_declared_field_count() {
        assert_eq!(parse_record(" (1,a) ", 2).unwrap().fields, vec![some("1"), some("a")]);
        assert_eq!(parse_record("(1,)", 2).unwrap().fields, vec![some("1"), None]);
        assert_eq!(parse_record("(,)", 2).unwrap().fields, vec![None, None]);
        assert_eq!(parse_record("(1,\"\")", 2).unwrap().fields, vec![some("1"), some("")]);
        // Quoting may start and stop mid-field, and `""` inside quotes is one
        // quote — the doubling convention, where an array backslash-escapes.
        assert_eq!(parse_record("(1,\"a\"b)", 2).unwrap().fields, vec![some("1"), some("ab")]);
        assert_eq!(
            parse_record("(1,\"a\"\"b\")", 2).unwrap().fields,
            vec![some("1"), some("a\"b")]
        );
        assert_eq!(parse_record(r"(1,a\,b)", 2).unwrap().fields, vec![some("1"), some("a,b")]);

        // Arity is the check, and it is why `()` is a value for a zero-field
        // composite and a fault for a two-field one.
        assert_eq!(parse_record("()", 0).unwrap().fields, Vec::<Option<String>>::new());
        assert_eq!(parse_record("()", 1).unwrap().fields, vec![None]);
        for (bad, columns) in [
            ("()", 2),      // too few columns
            ("(1,a,b)", 2), // too many
            ("(1)", 0),
            ("( )", 0), // a zero-field composite has no room for the blank
            ("()x", 0),
            ("(1,a) x", 2),
            ("(1,a", 2),
            ("1,a)", 2),
            ("", 1),
        ] {
            assert!(parse_record(bad, columns).is_none(), "should not parse: {bad} as {columns}");
        }
    }

    #[test]
    fn range_input_takes_empty_in_any_case_and_keeps_its_bounds_verbatim() {
        for literal in ["empty", "EMPTY", " empty "] {
            assert_eq!(parse_range(literal).unwrap(), RangeLiteral::empty(), "{literal}");
        }
        assert_eq!(parse_range(" [1,10) ").unwrap(), decode_range("[1,10)").unwrap());
        // `["",a)` has an empty-string lower bound and `(,a)` has none, which
        // is the distinction `decode_range` also turns on.
        assert_eq!(parse_range("[\"\",a)").unwrap().lower, some(""));
        assert_eq!(parse_range("(,a)").unwrap().lower, None);
        assert_eq!(parse_range("[\"a\"b,c)").unwrap().lower, some("ab"));
        assert_eq!(parse_range(r"[a\,b,c)").unwrap().lower, some("a,b"));
        assert_eq!(parse_range("[\"a\"\"b\",c)").unwrap().lower, some("a\"b"));

        // Not canonicalized: a discrete subtype's `[1,10]` is `[1,11)` on the
        // server and stays inclusive here.
        assert!(parse_range("[1,10]").unwrap().upper_inclusive);

        for bad in ["empty x", "[1,10)x", "[1,10", "[1,2,3)", "1,10)", "[1)", ""] {
            assert!(parse_range(bad).is_none(), "should not parse: {bad}");
        }
    }

    #[test]
    fn multirange_input_drops_a_member_spelled_empty_and_keeps_the_written_order() {
        assert_eq!(parse_multirange("{ }").unwrap().len(), 0);
        assert_eq!(parse_multirange("{empty}").unwrap().len(), 0);
        assert_eq!(parse_multirange("{EMPTY,[1,2)}").unwrap().len(), 1);
        assert_eq!(parse_multirange("{ [1,2) , [5,6) }").unwrap().len(), 2);
        // The server sorts and coalesces its members and drops one that is
        // *equivalent* to empty; both need the subtype's own order, so a
        // parsed multirange is the members as written.
        assert_eq!(parse_multirange("{[1,1)}").unwrap().len(), 1);
        let unsorted = parse_multirange("{[5,6),[1,2)}").unwrap();
        assert_eq!(unsorted[0].lower, some("5"));

        // A member's own bound may carry the separator and a bracket.
        let quoted = parse_multirange("{[\"a,b\",\"c)d\"),[x,y]}").unwrap();
        assert_eq!(quoted.len(), 2);
        assert_eq!(quoted[0].upper, some("c)d"));

        for bad in ["{[1,2),}", "{[1,2)", "[1,2)}", "{[1,2)x}", "{,}", ""] {
            assert!(parse_multirange(bad).is_none(), "should not parse: {bad}");
        }
    }

    /// The input grammars are supersets of the output ones, so everything the
    /// strict decoder reads the permissive parser reads identically — which
    /// is what lets a filter compare a literal against a field at all.
    #[test]
    fn every_output_form_parses_as_itself_through_the_input_grammar() {
        for literal in [
            "{}",
            "{1,2,3}",
            "{NULL}",
            "{\"NULL\"}",
            "{\"\"}",
            "{{1,2},{3,4}}",
            // An array whose elements are themselves array literals (I26).
            // The parser reads it; nothing will hand it one, because such a
            // column resolves to text (`KD3`).
            "{\"{1,2}\",\"{3}\"}",
        ] {
            assert_eq!(parse_array(literal), decode_array(literal), "{literal}");
        }
        assert_eq!(parse_array("[0:2]={7,8,9}"), decode_array("[0:2]={7,8,9}"));
        for (literal, columns) in [("(1,\"a,b\"\"c\")", 2), ("(3,)", 2), ("(NULL)", 1), ("()", 1)] {
            assert_eq!(parse_record(literal, columns), decode_record(literal), "{literal}");
        }
        for literal in ["empty", "[1,10)", "(,5)", "[\"\",a)", "[\"a,b\",\"c\"\"d\"]"] {
            assert_eq!(parse_range(literal), decode_range(literal), "{literal}");
        }
        for literal in ["{}", "{[1,10)}", "{[1,2),[5,6)}"] {
            assert_eq!(parse_multirange(literal), decode_multirange(literal), "{literal}");
        }
        for literal in ["", "0", "1 2 3", "-32768 32767"] {
            assert_eq!(parse_int2vector(literal), decode_int2vector(literal), "{literal}");
        }
    }

    /// `int2vectorout`'s whole grammar, which is four rules long: decimal
    /// `int16`s, one space between them, nothing else, and the empty string
    /// for the empty vector.
    #[test]
    fn an_int2vector_round_trips_byte_for_byte() {
        for literal in ["", "0", "1 2 3", "-32768 32767", "42"] {
            let decoded =
                decode_int2vector(literal).unwrap_or_else(|| panic!("decode failed: {literal}"));
            assert_eq!(render_int2vector(&decoded), literal, "{literal}");
        }
        assert_eq!(decode_int2vector(""), Some(vec![]));
        assert_eq!(decode_int2vector("1 2 3"), Some(vec![1, 2, 3]));
    }

    /// The strict side refuses every spelling `pg_itoa` cannot write — which
    /// is what makes it and [`render_int2vector`] inverses.
    #[test]
    fn a_non_canonical_int2vector_element_is_refused_by_the_decoder() {
        for literal in ["+1", "01", "-0", "1  2", " 1", "1 ", "1\t2", "{1,2}", "1,2", "32768"] {
            assert_eq!(decode_int2vector(literal), None, "{literal}");
        }
    }

    /// The permissive side is `int2vectorin`, and the rule worth having is
    /// the asymmetric one: whitespace *before* a number is skipped, and the
    /// byte *after* one must be a space or the end.
    #[test]
    fn the_int2vector_input_grammar_is_int2vectorin() {
        assert_eq!(parse_int2vector("  1   2  "), Some(vec![1, 2]));
        assert_eq!(parse_int2vector("+1 01"), Some(vec![1, 1]));
        assert_eq!(parse_int2vector("\t\n1 2"), Some(vec![1, 2]));
        assert_eq!(parse_int2vector("   "), Some(vec![]));
        // The byte after a number is neither a space nor the end.
        assert_eq!(parse_int2vector("1\t2"), None);
        assert_eq!(parse_int2vector("1,2"), None);
        assert_eq!(parse_int2vector("1x"), None);
        // `int2vectorin`'s own range check, and `strtol`'s `ERANGE` beyond it.
        assert_eq!(parse_int2vector("32768"), None);
        assert_eq!(parse_int2vector("-32769"), None);
        assert_eq!(parse_int2vector("99999999999999999999999"), None);
        // Not a number at all, and the array spelling this type does not take.
        assert_eq!(parse_int2vector("{1,2}"), None);
        assert_eq!(parse_int2vector("-"), None);
    }
}
