//! **The output grammars as a walk**: an `array_out`, `record_out`,
//! `range_out`, `multirange_out` or `int2vectorout` value read a part at a time over its held
//! field, each part handed to its caller as it is read and none of them kept,
//! so a reader of it holds no copy and no tree of the value: a nested term's
//! order, and a query's batch, decoding each part into its builder
//! (`docs/design/decisions.md`, "D151", "D153").
//!
//! The grammar is `decode_*`'s, token for token: a token is read by
//! [`scan_token`] itself, and the structure around it as `decode_*` reads it,
//! which `key/field/tests.rs` holds the walk to over a generated corpus,
//! through the order it reads.

use std::borrow::Cow;

use pgdt_core::internal::{Unescaped, unescaped};

use super::{ARRAY, RANGE_BOUND, RECORD, Syntax, canonical_int2, scan_token};

/// **A nested field's text as the walk reads it**: a byte at a time with
/// one looked ahead, over a text in place, or over a field as `COPY` wrote
/// it, unescaped as it is read. A token is borrowed from a text where it
/// carries no escape of its own, and an escaped field's is collected, one
/// at a time.
#[derive(Debug, Clone)]
pub struct FieldText<'a> {
    source: Source<'a>,
    /// An escaped field's current token, as its container wrote it.
    token: Vec<u8>,
}

#[derive(Debug, Clone)]
enum Source<'a> {
    /// A text, read in place.
    Plain { text: &'a [u8], at: usize },
    /// A field holding a `COPY` escape, unescaped a byte at a time, the
    /// next byte it stands for, and where in the field that byte's spelling
    /// begins.
    Escaped { field: &'a [u8], bytes: Unescaped<'a>, next: Option<u8>, next_at: usize },
}

impl<'a> FieldText<'a> {
    /// A field's unescaped text, read in place.
    pub fn plain(text: &'a str) -> Self {
        Self { source: Source::Plain { text: text.as_bytes(), at: 0 }, token: Vec::new() }
    }

    /// A field as `COPY` wrote it, no SQL `NULL`, unescaped as it is read.
    pub fn escaped(field: &'a [u8]) -> Self {
        let mut bytes = unescaped(field).expect("a field read as text is no SQL NULL");
        let next = bytes.next();
        Self { source: Source::Escaped { field, bytes, next, next_at: 0 }, token: Vec::new() }
    }

    /// The next byte, unread.
    pub(crate) fn peek(&self) -> Option<u8> {
        match &self.source {
            Source::Plain { text, at } => text.get(*at).copied(),
            Source::Escaped { next, .. } => *next,
        }
    }

    /// Read the next byte.
    pub(crate) fn bump(&mut self) {
        match &mut self.source {
            Source::Plain { at, .. } => *at += 1,
            Source::Escaped { bytes, next, next_at, .. } => {
                *next_at = bytes.offset();
                *next = bytes.next();
            }
        }
    }

    /// Where the next byte is: its place in a text, or where its spelling
    /// begins in a field as `COPY` wrote it, which [`FieldText::seek`]
    /// returns to.
    pub(crate) fn offset(&self) -> usize {
        match &self.source {
            Source::Plain { at, .. } => *at,
            Source::Escaped { next_at, .. } => *next_at,
        }
    }

    /// Read on from `offset`, a place [`FieldText::offset`] named.
    pub(crate) fn seek(&mut self, offset: usize) {
        match &mut self.source {
            Source::Plain { at, .. } => *at = offset,
            Source::Escaped { field, bytes, next, next_at } => {
                *bytes = Unescaped::resumed(field, offset);
                *next_at = offset;
                *next = bytes.next();
            }
        }
    }

    /// Read `byte` where it is next, answering whether it was.
    pub(crate) fn eat(&mut self, byte: u8) -> bool {
        let next = self.peek() == Some(byte);
        if next {
            self.bump();
        }
        next
    }

    pub(crate) fn at_end(&self) -> bool {
        self.peek().is_none()
    }

    /// The text not yet read, whole: what a reader of no walk reads.
    pub(crate) fn rest(&mut self) -> Option<Cow<'a, str>> {
        match &mut self.source {
            Source::Plain { text, at } => {
                let rest = &text[(*at).min(text.len())..];
                *at = text.len();
                std::str::from_utf8(rest).ok().map(Cow::Borrowed)
            }
            Source::Escaped { bytes, next, .. } => {
                let rest: Vec<u8> = next.take().into_iter().chain(bytes.by_ref()).collect();
                String::from_utf8(rest).ok().map(Cow::Owned)
            }
        }
    }

    /// One token in `syntax`, stopping at its separator or a byte of
    /// `terminators`, which it leaves unread, handed to `read` — `None`
    /// being SQL NULL — or `None` where it is no token [`scan_token`] reads.
    fn token<R>(
        &mut self,
        syntax: &Syntax,
        terminators: &[u8],
        read: impl FnOnce(Option<&str>) -> R,
    ) -> Option<R> {
        if let Source::Plain { text, at } = &mut self.source {
            let (token, next) = scan_token(text, *at, syntax, terminators)?;
            *at = next;
            return Some(read(token.as_deref()));
        }
        // The token's bytes as its container wrote them, and the byte that
        // ends it, for `scan_token` to read as it reads a text in place.
        self.token.clear();
        if self.eat(b'"') {
            self.token.push(b'"');
            loop {
                let byte = self.peek()?;
                self.bump();
                self.token.push(byte);
                match byte {
                    b'\\' => {
                        self.token.push(self.peek()?);
                        self.bump();
                    }
                    b'"' if syntax.escape == super::Escape::Double && self.peek() == Some(b'"') => {
                        self.token.push(b'"');
                        self.bump();
                    }
                    b'"' => break,
                    _ => {}
                }
            }
        } else {
            while let Some(byte) = self.peek() {
                if byte == syntax.separator || terminators.contains(&byte) {
                    break;
                }
                self.token.push(byte);
                self.bump();
            }
        }
        let end = self.token.len();
        self.token.push(self.peek()?);
        let (token, next) = scan_token(&self.token, 0, syntax, terminators)?;
        (next == end).then(|| read(token.as_deref()))
    }

    /// A signed decimal integer, as `decode_array` reads a dimension's
    /// bound: an optional `-` and at least one digit, within `i32`.
    fn int(&mut self) -> Option<i32> {
        let negative = self.eat(b'-');
        let mut value: i64 = 0;
        let mut digits = false;
        while let Some(byte) = self.peek().filter(u8::is_ascii_digit) {
            self.bump();
            digits = true;
            // Past `i32`'s magnitude no digit brings it back, but leading
            // zeros are read at any length.
            value = (value * 10 + i64::from(byte - b'0')).min(i64::from(i32::MAX) + 2);
        }
        digits.then_some(())?;
        i32::try_from(if negative { -value } else { value }).ok()
    }
}

/// An `array_out` value's shape, as `decode_array` returns it beside its
/// elements: one dimension and one lower bound a level, none for `{}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayShape {
    pub dims: Vec<usize>,
    pub lower_bounds: Vec<i32>,
}

/// The brace walk's state: `ArrayScan`'s, with the elements counted rather
/// than kept.
struct ArrayWalk {
    dims: Vec<Option<usize>>,
    leaf_depth: Option<usize>,
    count: usize,
}

impl ArrayWalk {
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

    // deficiency: KD175 — this recursion, and `ArrayScan::braces`'s beside
    // `decode_array`, descend once per `{` with no `MAXDIM` bound, where the
    // input walk refuses past it (I25), so a brace run longer than the stack
    // holds frames aborts the process instead of answering `None`. Only a
    // file no `pg_dump` wrote holds one. Bounding the depth at `MAXDIM`, as
    // the input side does, closes it.
    fn braces(
        &mut self,
        text: &mut FieldText<'_>,
        depth: usize,
        element: &mut impl FnMut(Option<&str>) -> Option<()>,
    ) -> Option<()> {
        if !text.eat(b'{') {
            return None;
        }
        if text.eat(b'}') {
            // `{}` is only ever the whole value, as `ArrayScan` reads it.
            if depth != 0 {
                return None;
            }
            return self.record_dim(depth, 0);
        }
        let mut count = 0usize;
        loop {
            if text.peek() == Some(b'{') {
                self.braces(text, depth + 1, element)?;
            } else {
                match self.leaf_depth {
                    None => self.leaf_depth = Some(depth + 1),
                    Some(d) if d == depth + 1 => {}
                    Some(_) => return None,
                }
                text.token(&ARRAY, b"}{", &mut *element)??;
                self.count += 1;
            }
            count += 1;
            if text.eat(b'}') {
                break;
            }
            if !text.eat(b',') {
                return None;
            }
        }
        self.record_dim(depth, count)
    }
}

/// [`decode_array`](super::decode_array)'s grammar as a walk: each element
/// handed to `element` as it is read, row-major — `None` back stopping the
/// walk, refused — and the value's shape once its text has ended, or `None`
/// where the text is no `array_out` value.
pub fn walk_array(
    text: &mut FieldText<'_>,
    mut element: impl FnMut(Option<&str>) -> Option<()>,
) -> Option<ArrayShape> {
    let mut decoration: Vec<(i32, usize)> = Vec::new();
    while text.eat(b'[') {
        let lb = text.int()?;
        if !text.eat(b':') {
            return None;
        }
        let ub = text.int()?;
        if !text.eat(b']') {
            return None;
        }
        decoration.push((lb, usize::try_from(i64::from(ub) - i64::from(lb) + 1).ok()?));
    }
    if !decoration.is_empty() && !text.eat(b'=') {
        return None;
    }
    let mut walk = ArrayWalk { dims: Vec::new(), leaf_depth: None, count: 0 };
    walk.braces(text, 0, &mut element)?;
    if !text.at_end() {
        return None;
    }
    if walk.count == 0 {
        // `array_out` never decorates `{}`.
        return decoration
            .is_empty()
            .then(|| ArrayShape { dims: Vec::new(), lower_bounds: Vec::new() });
    }
    let dims = walk.dims.into_iter().collect::<Option<Vec<usize>>>()?;
    if walk.leaf_depth != Some(dims.len()) || dims.iter().product::<usize>() != walk.count {
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
    Some(ArrayShape { dims, lower_bounds })
}

/// [`decode_record`](super::decode_record)'s grammar as a walk: each field
/// handed to `field` as it is read, and how many there were, its arity
/// unchecked as `decode_record` leaves it.
pub fn walk_record(
    text: &mut FieldText<'_>,
    mut field: impl FnMut(Option<&str>) -> Option<()>,
) -> Option<usize> {
    if !text.eat(b'(') {
        return None;
    }
    let mut count = 0;
    loop {
        text.token(&RECORD, b")", &mut field)??;
        count += 1;
        if text.eat(b')') {
            break;
        }
        if !text.eat(b',') {
            return None;
        }
    }
    text.at_end().then_some(count)
}

/// What a `range_out` value states beside its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeShape {
    pub empty: bool,
    pub lower_inclusive: bool,
    pub upper_inclusive: bool,
}

/// [`decode_range`](super::decode_range)'s grammar as a walk: each bound
/// handed to `bound` as it is read, whether it is the lower beside it and
/// `None` an absent one, and what the value states beside them.
pub fn walk_range(
    text: &mut FieldText<'_>,
    bound: impl FnMut(bool, Option<&str>) -> Option<()>,
) -> Option<RangeShape> {
    if text.peek() == Some(b'e') {
        let empty = b"empty".iter().all(|&letter| text.eat(letter)) && text.at_end();
        return empty.then_some(RangeShape {
            empty: true,
            lower_inclusive: false,
            upper_inclusive: false,
        });
    }
    let shape = walk_bracketed(text, bound)?;
    text.at_end().then_some(shape)
}

/// A range written between its brackets, as [`walk_range`] reads one, read
/// to its closing bracket and what follows it left unread: a multirange's
/// member ([`walk_multirange`]).
pub fn walk_bracketed(
    text: &mut FieldText<'_>,
    mut bound: impl FnMut(bool, Option<&str>) -> Option<()>,
) -> Option<RangeShape> {
    let lower_inclusive = match text.peek()? {
        b'[' => true,
        b'(' => false,
        _ => return None,
    };
    text.bump();
    text.token(&RANGE_BOUND, b"])", |value| bound(true, value))??;
    if !text.eat(b',') {
        return None;
    }
    text.token(&RANGE_BOUND, b"])", |value| bound(false, value))??;
    let upper_inclusive = match text.peek()? {
        b']' => true,
        b')' => false,
        _ => return None,
    };
    text.bump();
    Some(RangeShape { empty: false, lower_inclusive, upper_inclusive })
}

/// [`decode_multirange`](super::decode_multirange)'s grammar as a walk:
/// `member` handed the text at each member's opening bracket, to read it by
/// [`walk_bracketed`], and how many members there were. A member is never
/// `empty`, which `multirange_out` drops.
pub fn walk_multirange<'a>(
    text: &mut FieldText<'a>,
    mut member: impl FnMut(&mut FieldText<'a>) -> Option<()>,
) -> Option<usize> {
    if !text.eat(b'{') {
        return None;
    }
    let mut count = 0;
    if !text.eat(b'}') {
        loop {
            member(text)?;
            count += 1;
            if text.eat(b'}') {
                break;
            }
            if !text.eat(b',') {
                return None;
            }
        }
    }
    text.at_end().then_some(count)
}

/// The longest element [`canonical_int2`] reads, `-32768`.
const INT2_SPELLING: usize = 6;

/// [`decode_int2vector`](super::decode_int2vector)'s grammar as a walk: each
/// element handed to `element` as it is read, and how many there were.
pub fn walk_int2vector(
    text: &mut FieldText<'_>,
    mut element: impl FnMut(i16) -> Option<()>,
) -> Option<usize> {
    if text.at_end() {
        return Some(0);
    }
    let mut count = 0;
    loop {
        let mut spelling = [0u8; INT2_SPELLING];
        let mut len = 0;
        while let Some(byte) = text.peek().filter(|&byte| byte != b' ') {
            // A spelling past the longest is none `canonical_int2` reads.
            *spelling.get_mut(len)? = byte;
            len += 1;
            text.bump();
        }
        element(canonical_int2(std::str::from_utf8(&spelling[..len]).ok()?)?)?;
        count += 1;
        if !text.eat(b' ') {
            return Some(count);
        }
    }
}
