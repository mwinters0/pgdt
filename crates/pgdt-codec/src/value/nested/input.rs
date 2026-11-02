//! The literal scanner: the `*_in` supersets a filter's literal and a strict
//! parse's field are read by.
//!
//! [`super::output`] reads what a dump *holds*. This module reads what the
//! server would — a filter's right-hand side, which the `*_in` functions
//! accept a good deal more of than the matching `*_out` ever writes (I44),
//! through separate scanners (`docs/design/decisions.md`, "D45"). **Each
//! grammar is a walk** over a byte source handing its caller every part as it
//! reads it, which `parse_*` collect and a strict check reads a field's bytes
//! by, holding no part past its turn ("D143").
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

use super::{ArrayLiteral, RangeLiteral, RecordLiteral, is_space};
use crate::value::source::{Bytes, Lexer, Part, Source, Step, collect};

/// PostgreSQL's `MAXDIM`: the most dimensions an array value may have.
/// `pgdt_model::PG_ARRAY_MAX_DIMS` is the same number for the shape census;
/// it is a `u8` there and this is an index bound, and neither module is worth
/// a dependency edge on the other for one constant.
const MAXDIM: usize = 6;

/// **A literal one of these grammars refuses**, wherever in it the walk
/// found that — or a part its caller read and refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refused;

/// What a part's reader answers: [`Refused`] stops the walk.
pub(crate) type Walked<T = ()> = Result<T, Refused>;

/// **What a walk hands its caller of each part**: the part's own text, read
/// off the literal's as its container's grammar reads it, or `None` for SQL
/// NULL. A part its caller leaves unread is read on by the walk.
pub(crate) type Each<'w, 'a, T> = dyn FnMut(T, Option<&mut dyn Source<'a>>) -> Walked + 'w;

/// An `array_in` literal's shape: one entry per dimension in each, both
/// empty for the zero-dimensional empty array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArrayShape {
    pub(crate) dims: Vec<usize>,
    pub(crate) lower_bounds: Vec<i32>,
}

/// One `[m:n]` bound. `ReadDimensionInt` refuses leading whitespace and then
/// hands the rest to `strtol`, so a sign is accepted and a bare sign is not.
fn dimension_int(text: &mut dyn Source<'_>) -> Walked<i32> {
    let negative = text.eat(b'-');
    if !negative {
        text.eat(b'+');
    }
    let magnitude = digits(text, i64::from(i32::MAX) + 1)?;
    i32::try_from(if negative { -magnitude } else { magnitude }).map_err(|_| Refused)
}

/// A run of one or more ASCII digits, its value, which past `limit` is any
/// number past it: as many digits as `strtol` reads, leading zeros among them.
fn digits(text: &mut dyn Source<'_>, limit: i64) -> Walked<i64> {
    let mut value = 0i64;
    let mut any = false;
    while let Some(digit) = text.peek().filter(u8::is_ascii_digit) {
        text.next();
        any = true;
        value = (value * 10 + i64::from(digit - b'0')).min(limit + 1);
    }
    if any { Ok(value) } else { Err(Refused) }
}

/// **Walk an `array_in` literal**, `delim` being its element type's
/// `typdelim`, handing `each` every element in row-major order as it reads
/// it, and answering its shape: what [`parse_array_delimited`] reads, with
/// no element held past its turn (`docs/design/decisions.md`, "D143").
// pg-refuses: I44 — every refusal here is `array_in`'s.
pub(crate) fn walk_array<'a>(
    text: &mut dyn Source<'a>,
    delim: u8,
    each: &mut Each<'_, 'a, ()>,
) -> Walked<ArrayShape> {
    let mut dim = [None; MAXDIM];
    let mut lower = [1i32; MAXDIM];
    let mut declared = 0usize;

    // Zero or more `[n]` or `[m:n]` items, with whitespace allowed between
    // them but not inside one.
    loop {
        text.skip_while(is_space);
        if !text.eat(b'[') {
            break;
        }
        if declared >= MAXDIM {
            return Err(Refused);
        }
        let first = dimension_int(text)?;
        let (lb, ub) = if text.eat(b':') { (first, dimension_int(text)?) } else { (1, first) };
        if !text.eat(b']') {
            return Err(Refused);
        }
        // A zero-length dimension is refused rather than collapsed, and an
        // upper bound of `INT_MAX` is refused outright.
        if ub < lb || ub == i32::MAX {
            return Err(Refused);
        }
        dim[declared] =
            Some(usize::try_from(i64::from(ub) - i64::from(lb) + 1).map_err(|_| Refused)?);
        lower[declared] = lb;
        declared += 1;
    }

    if declared > 0 {
        if !text.eat(b'=') {
            return Err(Refused);
        }
        text.skip_while(is_space);
    }
    if text.peek() != Some(b'{') {
        return Err(Refused);
    }

    let (ndim, count) = array_body(text, declared, &mut dim, delim, each)?;
    text.skip_while(is_space);
    if text.next().is_some() {
        return Err(Refused);
    }

    if count == 0 {
        return Ok(ArrayShape { dims: Vec::new(), lower_bounds: Vec::new() });
    }
    let dims = dim[..ndim].iter().copied().collect::<Option<Vec<usize>>>().ok_or(Refused)?;
    if dims.iter().product::<usize>() != count {
        return Err(Refused);
    }
    Ok(ArrayShape { dims, lower_bounds: lower[..ndim].to_vec() })
}

/// Walk the brace structure from the opening `{`, as `ReadArrayToken`
/// classifies its tokens, answering the dimensionality it turned out to have
/// and how many elements it held; `text` is left just past the matching `}`.
///
/// `dim` is `Some` per dimension where the literal carried an `[lb:ub]=`
/// decoration and `None` where the structure has to say — the same array
/// serving as both input and output, which is what lets one walk check a
/// declared shape and deduce an undeclared one. `ndim` is `0` when nothing was
/// declared.
fn array_body<'a>(
    text: &mut dyn Source<'a>,
    mut ndim: usize,
    dim: &mut [Option<usize>; MAXDIM],
    delim: u8,
    each: &mut Each<'_, 'a, ()>,
) -> Walked<(usize, usize)> {
    // Once a dimensionality is declared, or an element has been seen, the
    // nesting may not get deeper.
    let mut frozen = ndim != 0;
    let mut expect_delim = false;
    let mut nest_level = 0usize;
    let mut nelems = [0usize; MAXDIM];
    let mut count = 0usize;
    loop {
        text.skip_while(is_space);
        match text.peek().ok_or(Refused)? {
            b'{' => {
                text.next();
                if expect_delim || nest_level >= MAXDIM {
                    return Err(Refused);
                }
                nelems[nest_level] = 0;
                nest_level += 1;
                if nest_level > ndim {
                    if frozen {
                        return Err(Refused);
                    }
                    ndim = nest_level;
                }
            }
            b'}' => {
                text.next();
                if nest_level == 0 {
                    return Err(Refused);
                }
                // A `}` may close an empty sub-array; otherwise it stands
                // where a separator would have.
                if nelems[nest_level - 1] > 0 && !expect_delim {
                    return Err(Refused);
                }
                nest_level -= 1;
                if nest_level > 0 {
                    nelems[nest_level - 1] += 1;
                }
                match dim[nest_level] {
                    None => dim[nest_level] = Some(nelems[nest_level]),
                    Some(n) if n == nelems[nest_level] => {}
                    Some(_) => return Err(Refused),
                }
                expect_delim = true;
            }
            byte if byte == delim => {
                text.next();
                if !expect_delim {
                    return Err(Refused);
                }
                expect_delim = false;
            }
            first => {
                if expect_delim || nest_level == 0 {
                    return Err(Refused);
                }
                array_element(text, first, delim, each)?;
                frozen = true;
                // Every element sits at the same depth, or the literal is
                // ragged.
                if nest_level != ndim {
                    return Err(Refused);
                }
                nelems[nest_level - 1] += 1;
                count += 1;
                expect_delim = true;
            }
        }
        if nest_level == 0 {
            return Ok((ndim, count));
        }
    }
}

/// One element, `first` its first byte, handed to `each`; `text` is left at
/// the separator or brace after it.
fn array_element<'a>(
    text: &mut dyn Source<'a>,
    first: u8,
    delim: u8,
    each: &mut Each<'_, 'a, ()>,
) -> Walked {
    if first == b'"' {
        text.next();
        let mut element = Part::new(&mut *text, QuotedElement);
        each((), Some(&mut element))?;
        if !element.finish() {
            return Err(Refused);
        }
        // Nothing but whitespace may follow the closing quote before a
        // separator or a brace: `{"a"b}` is "incorrectly quoted", not a
        // two-part element.
        text.skip_while(is_space);
        return match text.peek() {
            Some(b'{' | b'}') => Ok(()),
            Some(byte) if byte == delim => Ok(()),
            _ => Err(Refused),
        };
    }
    if unquoted_null(text, delim) {
        text.eat_word(b"NULL");
        return each((), None);
    }
    let mut element = Part::new(&mut *text, UnquotedElement { delim, interior: 0 });
    each((), Some(&mut element))?;
    if element.finish() { Ok(()) } else { Err(Refused) }
}

/// Whether the unquoted element ahead is the `NULL` marker: those four
/// letters in any case, and no escape among them, so `{N\ULL}` is the
/// *string* `NULL`, with nothing but whitespace after them.
fn unquoted_null(text: &mut dyn Source<'_>, delim: u8) -> bool {
    let mut ahead = text.fork();
    if !ahead.eat_word(b"NULL") {
        return false;
    }
    ahead.skip_while(is_space);
    ahead.peek().is_some_and(|byte| byte == delim || byte == b'}')
}

/// A `"`-delimited array element, from just past the opening quote, to just
/// past the closing one. A quoted element is never the `NULL` marker,
/// whatever it spells.
#[derive(Clone)]
struct QuotedElement;

impl Lexer for QuotedElement {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step {
        match outer.next() {
            Some(b'\\') => outer.next().map_or(Step::Fail, Step::Byte),
            Some(b'"') => Step::End,
            Some(byte) => Step::Byte(byte),
            None => Step::Fail,
        }
    }
}

/// An unquoted array element, to the separator or brace after it, which it
/// leaves unread. Trailing whitespace is dropped and interior whitespace
/// kept — a run is interior where something but a separator or a brace
/// follows it, which `interior` counts down — and a `\` escape counts as
/// non-whitespace.
#[derive(Clone)]
struct UnquotedElement {
    delim: u8,
    /// The bytes of a whitespace run known to be interior still to read.
    interior: usize,
}

impl Lexer for UnquotedElement {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step {
        if self.interior > 0 {
            self.interior -= 1;
            return outer.next().map_or(Step::Fail, Step::Byte);
        }
        let terminates = |byte: u8, delim: u8| byte == delim || byte == b'}';
        match outer.peek() {
            // An element is quoted in full or not at all, and a `{` here is
            // a brace where a value was expected.
            None | Some(b'{' | b'"') => Step::Fail,
            Some(b'\\') => {
                outer.next();
                outer.next().map_or(Step::Fail, Step::Byte)
            }
            Some(byte) if terminates(byte, self.delim) => Step::End,
            Some(byte) if is_space(byte) => {
                let mut ahead = outer.fork();
                let mut run = 0usize;
                while ahead.peek().is_some_and(is_space) {
                    ahead.next();
                    run += 1;
                }
                if ahead.peek().is_some_and(|byte| terminates(byte, self.delim)) {
                    return Step::End;
                }
                self.interior = run - 1;
                outer.next().map_or(Step::Fail, Step::Byte)
            }
            Some(byte) => {
                outer.next();
                Step::Byte(byte)
            }
        }
    }
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
/// `box`, the one built-in's that is not `,` (I22): `walk_array`'s elements
/// collected.
pub fn parse_array_delimited(s: &str, delim: u8) -> Option<ArrayLiteral<'_>> {
    let mut elements = Vec::new();
    // Owned unconditionally: `array_in` trims, unescapes and re-cases, so an
    // element here is rarely the bytes the user typed, and this grammar reads
    // one literal per query.
    let shape = walk_array(&mut Bytes(s.bytes()), delim, &mut |(), element| {
        elements.push(match element {
            Some(element) => Some(Cow::Owned(collect(element).ok_or(Refused)?)),
            None => None,
        });
        Ok(())
    })
    .ok()?;
    Some(ArrayLiteral { elements, dims: shape.dims, lower_bounds: shape.lower_bounds })
}

/// One `record_in` field or `range_in` bound: every byte up to the first
/// terminator outside quotes, which it leaves unread, with `\` taking the
/// next byte literally and `""` inside quotes meaning one `"`. Quoting may
/// start and stop anywhere, so `["a"b,c)` has the lower bound `ab`.
///
/// **Whitespace is never trimmed.** This is the half `array_in` does
/// differently, and reusing an array-shaped scanner here is what silently
/// eats a composite field's leading blank.
#[derive(Clone)]
struct QuotedRun {
    terminators: &'static [u8],
    quoted: bool,
}

impl QuotedRun {
    fn to(terminators: &'static [u8]) -> Self {
        Self { terminators, quoted: false }
    }
}

impl Lexer for QuotedRun {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step {
        loop {
            let Some(byte) = outer.peek() else { return Step::Fail };
            if !self.quoted && self.terminators.contains(&byte) {
                return Step::End;
            }
            outer.next();
            match byte {
                b'\\' => return outer.next().map_or(Step::Fail, Step::Byte),
                b'"' if !self.quoted => self.quoted = true,
                b'"' if outer.eat(b'"') => return Step::Byte(b'"'),
                b'"' => self.quoted = false,
                byte => return Step::Byte(byte),
            }
        }
    }
}

/// **Walk a `record_in` literal for a composite with `columns` declared
/// fields**, handing `each` every field with its position as it reads it:
/// what [`parse_record`] reads, holding none of them past its turn.
// pg-refuses: I44 — every refusal here is `record_in`'s, arity included.
pub(crate) fn walk_record<'a>(
    text: &mut dyn Source<'a>,
    columns: usize,
    each: &mut Each<'_, 'a, usize>,
) -> Walked {
    text.skip_while(is_space);
    if !text.eat(b'(') {
        return Err(Refused);
    }
    for k in 0..columns {
        if k > 0 && !text.eat(b',') {
            return Err(Refused);
        }
        // Nothing at all between the separators is SQL NULL; `""` is the
        // empty string.
        if matches!(text.peek(), Some(b',' | b')')) {
            each(k, None)?;
            continue;
        }
        let mut field = Part::new(&mut *text, QuotedRun::to(b",)"));
        each(k, Some(&mut field))?;
        if !field.finish() {
            return Err(Refused);
        }
    }
    if !text.eat(b')') {
        return Err(Refused);
    }
    text.skip_while(is_space);
    if text.next().is_some() { Err(Refused) } else { Ok(()) }
}

/// Parse a `record_in` literal for a composite with `columns` declared fields:
/// `walk_record`'s fields collected.
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
pub fn parse_record(s: &str, columns: usize) -> Option<RecordLiteral> {
    let mut fields = Vec::with_capacity(columns);
    walk_record(&mut Bytes(s.bytes()), columns, &mut |_, field| {
        fields.push(match field {
            Some(field) => Some(collect(field).ok_or(Refused)?),
            None => None,
        });
        Ok(())
    })
    .ok()?;
    Some(RecordLiteral { fields })
}

/// **Read a `range_in` literal off `text`**, its bounds collected:
/// [`parse_range`]'s reading. `empty` is matched case-insensitively and may
/// carry whitespace on either side; the bounds keep theirs.
// pg-refuses: I44 — every refusal here is `range_in`'s grammar's.
pub(crate) fn read_range(text: &mut dyn Source<'_>) -> Option<RangeLiteral> {
    text.skip_while(is_space);
    let lower_inclusive = match text.peek()? {
        b'[' => true,
        b'(' => false,
        _ => {
            if !text.eat_word(b"empty") {
                return None;
            }
            text.skip_while(is_space);
            return text.next().is_none().then(RangeLiteral::empty);
        }
    };
    text.next();
    let lower = range_bound(text)?;
    if !text.eat(b',') {
        return None;
    }
    let upper = range_bound(text)?;
    let upper_inclusive = match text.next()? {
        b']' => true,
        b')' => false,
        _ => return None,
    };
    text.skip_while(is_space);
    text.next().is_none().then_some(RangeLiteral {
        empty: false,
        lower,
        upper,
        lower_inclusive,
        upper_inclusive,
    })
}

/// One range bound. Completely empty input is the unbounded bound, which is
/// what makes `(,a)` and `["",a)` different values.
fn range_bound(text: &mut dyn Source<'_>) -> Option<Option<String>> {
    if matches!(text.peek()?, b',' | b')' | b']') {
        return Some(None);
    }
    let mut bound = Part::new(&mut *text, QuotedRun::to(b",)]"));
    let value = collect(&mut bound);
    bound.finish().then_some(value).flatten().map(Some)
}

/// Parse a `range_in` literal (`read_range`).
///
/// The bounds are **not canonicalized** — `[1,10]` stays inclusive-inclusive
/// here, where the server would rewrite it to `[1,11)` for a discrete subtype.
/// That needs the subtype's own successor function, which this module does not
/// have.
pub fn parse_range(s: &str) -> Option<RangeLiteral> {
    read_range(&mut Bytes(s.bytes()))
}

/// Where a `multirange_in` scan stands inside one member, which it reads as
/// a whole to hand to `range_in`.
#[derive(Clone, Copy)]
enum MemberState {
    /// Its opening bracket not yet read.
    Opening,
    InRange,
    InRangeEscaped,
    InRangeQuoted,
    InRangeQuotedEscaped,
    /// Inside quotes, a `"` read that a second follows, which is the
    /// second's to read as itself.
    InRangeQuotedDoubled,
    /// Its closing bracket read.
    Closed,
}

/// **One member of a `multirange_in` literal**, its bytes as written, from
/// its opening bracket to the closing one, as `multirange_in` finds the
/// substring it hands to `range_in`: whitespace is skipped in every state,
/// member text included — it is passed on all the same — so a member's own
/// brackets, escapes and quoting are what delimit it.
#[derive(Clone)]
struct Member(MemberState);

impl Lexer for Member {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step {
        use MemberState::*;
        if matches!(self.0, Closed) {
            return Step::End;
        }
        let Some(byte) = outer.next() else { return Step::Fail };
        if is_space(byte) {
            return Step::Byte(byte);
        }
        self.0 = match (self.0, byte) {
            (Opening, _) => InRange,
            (InRange, b']' | b')') => Closed,
            (InRange, b'"') => InRangeQuoted,
            (InRange, b'\\') => InRangeEscaped,
            (InRange, _) | (InRangeEscaped, _) => InRange,
            (InRangeQuoted, b'"') if outer.peek() == Some(b'"') => InRangeQuotedDoubled,
            (InRangeQuoted, b'"') => InRange,
            (InRangeQuoted, b'\\') => InRangeQuotedEscaped,
            (InRangeQuoted | InRangeQuotedEscaped | InRangeQuotedDoubled, _) => InRangeQuoted,
            (Closed, _) => unreachable!("a closed member reads nothing"),
        };
        Step::Byte(byte)
    }
}

/// **Walk a `multirange_in` literal**, handing `each` every member range as
/// it reads it: what [`parse_multirange`] reads, holding one member at a
/// time.
// pg-refuses: I44 — every refusal here is `multirange_in`'s grammar's.
pub(crate) fn walk_multirange(
    text: &mut dyn Source<'_>,
    each: &mut dyn FnMut(RangeLiteral) -> Walked,
) -> Walked {
    text.skip_while(is_space);
    if !text.eat(b'{') {
        return Err(Refused);
    }
    let mut seen = 0usize;
    loop {
        text.skip_while(is_space);
        match text.peek().ok_or(Refused)? {
            b'[' | b'(' => {
                let mut member = Part::new(&mut *text, Member(MemberState::Opening));
                let range = read_range(&mut member);
                if !member.finish() {
                    return Err(Refused);
                }
                let range = range.ok_or(Refused)?;
                seen += 1;
                if !range.empty {
                    each(range)?;
                }
            }
            b'}' if seen == 0 => {
                text.next();
                break;
            }
            _ => {
                if !text.eat_word(b"empty") {
                    return Err(Refused);
                }
                seen += 1;
            }
        }
        text.skip_while(is_space);
        match text.next() {
            Some(b',') => {}
            Some(b'}') => break,
            _ => return Err(Refused),
        }
    }
    text.skip_while(is_space);
    if text.next().is_some() { Err(Refused) } else { Ok(()) }
}

/// Parse a `multirange_in` literal into its member ranges
/// (`walk_multirange`).
///
/// **A member spelled `empty` is accepted and dropped**, which
/// [`decode_multirange`] refuses because `multirange_out` never writes one. A
/// member that is *equivalent* to empty — `[1,1)` — is kept here and dropped
/// by the server, which is the same missing successor function
/// [`parse_range`] records.
///
/// [`decode_multirange`]: super::output::decode_multirange
pub fn parse_multirange(s: &str) -> Option<Vec<RangeLiteral>> {
    let mut members = Vec::new();
    walk_multirange(&mut Bytes(s.bytes()), &mut |member| {
        members.push(member);
        Ok(())
    })
    .ok()?;
    Some(members)
}

/// **Walk an `int2vectorin` literal**, handing `each` every element as it
/// reads it (I47).
///
/// `int2vectorin` skips any run of whitespace before an element, hands what
/// follows to `strtol` — so a leading `+` and a leading zero are taken —
/// range-checks the result against `int16`, and then requires the byte
/// *after* the number to be a space or the end of the string. That last rule
/// is the one worth reproducing rather than smoothing into "any whitespace
/// separates": `\t1 2` is accepted and `1\t2` is not.
// pg-refuses: I47 — every refusal here is `int2vectorin`'s, the `int16` bound included.
pub(crate) fn walk_int2vector(
    text: &mut dyn Source<'_>,
    each: &mut dyn FnMut(i16) -> Walked,
) -> Walked {
    loop {
        text.skip_while(is_space);
        if text.peek().is_none() {
            return Ok(());
        }
        let negative = text.eat(b'-');
        if !negative {
            text.eat(b'+');
        }
        // `strtol`'s own `ERANGE`, then `int2vectorin`'s explicit
        // `SHRT_MIN`/`SHRT_MAX` check — one refusal here, since both say the
        // literal is not a value of this type.
        let magnitude = digits(text, i64::from(i16::MAX) + 1)?;
        each(i16::try_from(if negative { -magnitude } else { magnitude }).map_err(|_| Refused)?)?;
        if !matches!(text.peek(), None | Some(b' ')) {
            return Err(Refused);
        }
    }
}

/// Parse an `int2vectorin` literal — everything the server accepts for an
/// `int2vector` column, which is a strict superset of what
/// [`decode_int2vector`] reads (`walk_int2vector`).
///
/// [`decode_int2vector`]: super::output::decode_int2vector
pub fn parse_int2vector(s: &str) -> Option<Vec<i16>> {
    let mut out = Vec::new();
    walk_int2vector(&mut Bytes(s.bytes()), &mut |value| {
        out.push(value);
        Ok(())
    })
    .ok()?;
    Some(out)
}

#[cfg(test)]
mod differential;
#[cfg(test)]
mod prior_shape;
#[cfg(test)]
mod tests;
