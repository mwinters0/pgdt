//! The literal scanner: the `*_in` supersets a filter's literal is read by.
//!
//! [`super::output`] reads what a dump *holds*. This module reads what a user
//! *typed* — a filter's right-hand side, which the `*_in` functions accept a
//! good deal more of than the matching `*_out` ever writes (I44), through
//! separate scanners (`docs/design/decisions.md`, "D45").
//!
//! **They are four grammars, not one**, and whitespace is where they first
//! disagree: `array_in` drops unquoted whitespace around an element, while
//! `record_in` and `range_in` keep every byte of it, so `( 1 , a )` into a
//! composite is a two-field value whose second field is `" a "`. One predicate
//! still serves all four, because `array_isspace`, `scanner_isspace` and the C
//! locale's `isspace` are the same six characters.
//!
//! The result is the same `ArrayLiteral`/`RecordLiteral`/`RangeLiteral` the
//! decoders produce, holding the parts **as the user spelled them**: an
//! element is whatever text the server would have handed the element type's
//! own `*_in`, not that type's canonical output. Putting the two sides of a
//! comparison into one spelling is the caller's job, only the caller knowing
//! the element type.

use std::borrow::Cow;

use super::{ArrayLiteral, Elements, RangeLiteral, RecordLiteral, is_space};

/// PostgreSQL's `MAXDIM`: the most dimensions an array value may have.
/// `pgdt_model::PG_ARRAY_MAX_DIMS` is the same number for the shape census;
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

/// Read one `array_in` token, skipping the whitespace ahead of it, `delim`
/// being the element type's `typdelim`. `None` is a malformed literal, here
/// and in every helper below.
fn read_array_token(s: &[u8], mut i: usize, delim: u8) -> Option<(ArrayToken, usize)> {
    loop {
        match *s.get(i)? {
            b'{' => return Some((ArrayToken::LevelStart, i + 1)),
            b'}' => return Some((ArrayToken::LevelEnd, i + 1)),
            b'"' => return read_quoted_array_element(s, i + 1, delim),
            c if c == delim => return Some((ArrayToken::Delim, i + 1)),
            c if is_space(c) => i += 1,
            _ => return read_unquoted_array_element(s, i, delim),
        }
    }
}

/// A `"`-delimited array element, from just past the opening quote. Nothing
/// but whitespace may follow the closing quote before a separator or a brace:
/// `{"a"b}` is "incorrectly quoted", not a two-part element. A quoted element
/// is never the `NULL` marker, whatever it spells.
fn read_quoted_array_element(s: &[u8], mut i: usize, delim: u8) -> Option<(ArrayToken, usize)> {
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
                        b'}' | b'{' => {
                            return Some((ArrayToken::Elem(String::from_utf8(out).ok()?), i));
                        }
                        c if c == delim => {
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
fn read_unquoted_array_element(s: &[u8], mut i: usize, delim: u8) -> Option<(ArrayToken, usize)> {
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
            c if c == delim || c == b'}' => {
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
    delim: u8,
) -> Option<(usize, Elements<'a>, usize)> {
    // Once a dimensionality is declared, or an element has been seen, the
    // nesting may not get deeper.
    let mut frozen = ndim != 0;
    let mut expect_delim = false;
    let mut nest_level = 0usize;
    let mut nelems = [0usize; MAXDIM];
    let mut elements = Vec::new();
    loop {
        let (token, next) = read_array_token(s, i, delim)?;
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
                    // typed, and this grammar reads one literal per query.
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
/// column, which over what `array_out` writes is a strict superset of what
/// [`decode_array`] reads.
///
/// The elements come back in the spelling the literal used, so
/// `render_array(parse_array(x))` is `array_out`'s answer only where the
/// element type's own `*_in` and `*_out` agree on every element; anywhere else
/// the caller has to canonicalize per element, which needs the element type.
///
/// **A literal with no elements is the zero-dimensional empty array**, however
/// it was written: `{}`, `{ }` and (on v17+) `{{},{}}` are all the value
/// `array_out` writes as `{}`.
///
/// [`decode_array`]: super::output::decode_array
pub fn parse_array(s: &str) -> Option<ArrayLiteral<'_>> {
    parse_array_delimited(s, b',')
}

/// [`parse_array`] for an element type whose `typdelim` is `delim` — `;` for
/// `box`, the one built-in's that is not `,` (I22).
// pg-refuses: I44 — every refusal here is `array_in`'s.
pub fn parse_array_delimited(s: &str, delim: u8) -> Option<ArrayLiteral<'_>> {
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

    let (ndim, elements, next) = read_array_body(b, i, declared, &mut dim, delim)?;
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
///
/// [`decode_record`]: super::output::decode_record
// pg-refuses: I44 — every refusal here is `record_in`'s, arity included.
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
// pg-refuses: I44 — every refusal here is `range_in`'s grammar's.
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
///
/// [`decode_multirange`]: super::output::decode_multirange
// pg-refuses: I44 — every refusal here is `multirange_in`'s grammar's.
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
///
/// [`decode_int2vector`]: super::output::decode_int2vector
// pg-refuses: I47 — every refusal here is `int2vectorin`'s, the `int16` bound included.
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
mod tests;
