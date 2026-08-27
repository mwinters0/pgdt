//! The nested literal codec: array, composite (record), range and multirange
//! literals, as PostgreSQL's `*_out` functions write them inside a single
//! `COPY` field.
//!
//! Pure, synchronous, no I/O and no Arrow — see `docs/design/layering.md`, L2.
//! Like `crate::decode`, every function here works on the *already
//! COPY-unescaped* text `crate::copy::decode_field` returns, and every
//! `render_*` is the exact inverse of its `decode_*`: feeding it a value
//! decoded from real `pg_dump` output reproduces that output byte for byte.
//! Turning the result back into on-disk bytes is `crate::copy::encode_field`'s
//! job, not this module's.
//!
//! # One scanner, three parameter sets
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
//! # What it accepts
//!
//! These are inverses of the *output* functions, not reimplementations of the
//! considerably more permissive `*_in` parsers (I20's scope limit). Whitespace
//! padding around an element, an unquoted token containing a backslash escape,
//! a nested `{}` — all things `*_in` accepts and `*_out` never emits — are
//! rejected rather than guessed at. The one deliberate leniency is that a bare
//! `NULL` array element is matched case-insensitively, as `array_in` does:
//! `array_out` force-quotes any element whose text *is* `null` in any casing,
//! so agreeing with PostgreSQL here can never misread real output.
//!
//! The separator is hardcoded to `,`. An array whose element type sets a
//! different `typdelim` (`box`, or any C-level base type) is not decoded as an
//! array at all — see `docs/design/architecture.md`, "Type resolution".

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
/// unescaped bytes and the index just past the closing quote.
fn scan_quoted(s: &[u8], mut i: usize, escape: Escape) -> Option<(Vec<u8>, usize)> {
    debug_assert_eq!(s.get(i), Some(&b'"'));
    i += 1;
    let mut out = Vec::new();
    loop {
        match *s.get(i)? {
            b'"' => {
                if escape == Escape::Double && s.get(i + 1) == Some(&b'"') {
                    out.push(b'"');
                    i += 2;
                } else {
                    return Some((out, i + 1));
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
fn scan_token(
    s: &[u8],
    i: usize,
    syntax: &Syntax,
    terminators: &[u8],
) -> Option<(Option<String>, usize)> {
    let stops = |c: u8| c == syntax.separator || terminators.contains(&c);

    if s.get(i) == Some(&b'"') {
        let (bytes, next) = scan_quoted(s, i, syntax.escape)?;
        // Nothing may follow a closing quote but a separator or a terminator;
        // `"a"b` is not something any `*_out` function can produce.
        if !stops(*s.get(next)?) {
            return None;
        }
        return Some((Some(String::from_utf8(bytes).ok()?), next));
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
    Some((Some(text.to_string()), i))
}

/// An `array_out` literal: elements flattened row-major, plus the shape they
/// were written in.
///
/// Dimensionality and lower bounds belong to the value, never to the column
/// (I21) — `integer[][]`, `integer[3]` and `integer[]` are all written
/// `integer[]`, and consecutive rows of one column may legitimately disagree.
/// That is what this type exists to carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayLiteral {
    /// Row-major, flattened across every dimension. `None` is a SQL NULL
    /// element (which an array spells as a bare `NULL`, distinguishing it from
    /// the *string* `NULL` by quoting alone).
    pub elements: Vec<Option<String>>,
    /// One entry per dimension. Empty for `{}`, which `array_out` emits for a
    /// zero-element array whatever its dimensionality (I20).
    pub dims: Vec<usize>,
    /// One entry per dimension, all `1` unless the literal carried an
    /// `[lb:ub]=` prefix. Empty exactly when `dims` is.
    pub lower_bounds: Vec<i32>,
}

impl ArrayLiteral {
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

/// Accumulator for the recursive brace walk. `dims` and `leaf_depth` are
/// filled in as the structure is discovered and cross-checked as it repeats:
/// every sibling list at one depth must have the same length, and every token
/// must sit at the same depth, or the literal is ragged and rejected.
struct ArrayScan<'a> {
    s: &'a [u8],
    dims: Vec<Option<usize>>,
    leaf_depth: Option<usize>,
    elements: Vec<Option<String>>,
}

impl ArrayScan<'_> {
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
pub fn decode_array(s: &str) -> Option<ArrayLiteral> {
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

fn render_braces(out: &mut String, a: &ArrayLiteral, depth: usize, cursor: &mut usize) {
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
pub fn render_array(a: &ArrayLiteral) -> String {
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
        fields.push(value);
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
    if b.get(i) != Some(&b',') {
        return None;
    }
    let (upper, i) = scan_token(b, i + 1, &RANGE_BOUND, b"])")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn some(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    /// The whole point of the module: decode then render must be the identity
    /// on anything a `*_out` function can produce.
    #[track_caller]
    fn array_round_trips(literal: &str) -> ArrayLiteral {
        let decoded = decode_array(literal).unwrap_or_else(|| panic!("decode failed: {literal}"));
        assert_eq!(render_array(&decoded), literal);
        decoded
    }

    #[test]
    fn one_dimensional_arrays_round_trip_with_every_null_and_quoting_case() {
        let a = array_round_trips("{1,2,3}");
        assert_eq!(a.elements, vec![some("1"), some("2"), some("3")]);
        assert_eq!(a.dims, vec![3]);
        assert_eq!(a.ndim(), 1);

        // `{}`, `{NULL}` and a one-element array holding the *string* `NULL`
        // are three different values that look alike.
        assert_eq!(array_round_trips("{}").elements, Vec::<Option<String>>::new());
        assert_eq!(array_round_trips("{NULL}").elements, vec![None]);
        assert_eq!(array_round_trips("{\"NULL\"}").elements, vec![some("NULL")]);
        assert_eq!(array_round_trips("{\"\"}").elements, vec![some("")]);
        assert_eq!(array_round_trips("{1,NULL,3}").elements, vec![some("1"), None, some("3")]);
    }

    #[test]
    fn an_array_backslash_escapes_inside_a_quoted_element() {
        let a = array_round_trips(r#"{"a,b","c{d}","e\"f","g\\h"}"#);
        assert_eq!(a.elements, vec![some("a,b"), some("c{d}"), some(r#"e"f"#), some(r"g\h")]);
    }

    #[test]
    fn whitespace_and_the_delimiter_force_quotes_on_render() {
        let a = ArrayLiteral {
            elements: vec![some("has space"), some("has,comma"), some("has'quote"), some("plain")],
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
        assert_eq!(a.elements, vec![some("1"), some("2"), some("3"), some("4")]);

        let deep = array_round_trips("{{{1},{2}},{{3},{4}}}");
        assert_eq!(deep.dims, vec![2, 2, 1]);
        assert_eq!(deep.elements, vec![some("1"), some("2"), some("3"), some("4")]);
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
        assert_eq!(field.elements, vec![some(r#"x"y"#), some("p q"), None]);
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
}
