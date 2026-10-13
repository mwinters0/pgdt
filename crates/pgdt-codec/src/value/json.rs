//! `json` and `jsonb`: whether `json_in` reads a text, which a strict parse
//! and a filter's literal call, and the `jsonb` document an order key holds
//! ([`Jsonb`]), with why its parser reads no value from a text.

use std::cmp::Ordering;

use crate::decode::{Read, Unread};
use crate::value::first_difference;
use crate::value::numeric::{NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX, NumericKey};

/// Whether `json_in` reads `text`, which it stores as written: one JSON value
/// in RFC 8259's grammar, with blanks of `' '`, `\t`, `\n` and `\r` around
/// any token. A string holds no byte below `0x20` and no escape but the eight
/// single-character ones and `\u` with four hex digits; **what a `\u` escape
/// names is not checked**, so `\u0000` and a lone surrogate half, which
/// `jsonb_in` refuses, are read, as is a number past `numeric`'s range.
///
/// Iterative, over a stack of the containers open, so no depth refuses: the
/// server's recursion is bounded by its `max_stack_depth`, a setting of the
/// server restoring the dump (I72).
// pg-refuses: I72 — every refusal here is `json_in`'s.
pub(crate) fn json_in(text: &str) -> bool {
    let b = text.as_bytes();
    let mut at = 0;
    // The containers open around the value being read, innermost last:
    // `true` for an object.
    let mut open: Vec<bool> = Vec::new();
    loop {
        // A value is expected at `at`.
        json_blanks(b, &mut at);
        let ok = match b.get(at) {
            Some(b'{') => {
                at += 1;
                json_blanks(b, &mut at);
                if b.get(at) == Some(&b'}') {
                    at += 1;
                    true
                } else {
                    if !json_member_key(b, &mut at) {
                        return false;
                    }
                    open.push(true);
                    continue;
                }
            }
            Some(b'[') => {
                at += 1;
                json_blanks(b, &mut at);
                if b.get(at) == Some(&b']') {
                    at += 1;
                    true
                } else {
                    open.push(false);
                    continue;
                }
            }
            Some(b'"') => json_string(b, &mut at),
            Some(b'-' | b'0'..=b'9') => json_number(b, &mut at),
            _ => ["true", "false", "null"].iter().any(|word| {
                let found = b[at..].starts_with(word.as_bytes());
                at += if found { word.len() } else { 0 };
                found
            }),
        };
        if !ok {
            return false;
        }
        // A value has been read: close what it ends, or go on to the next.
        loop {
            json_blanks(b, &mut at);
            let Some(&object) = open.last() else { return at == b.len() };
            match b.get(at) {
                Some(b',') => {
                    at += 1;
                    if object && !json_member_key(b, &mut at) {
                        return false;
                    }
                    break;
                }
                Some(b'}') if object => {
                    at += 1;
                    open.pop();
                }
                Some(b']') if !object => {
                    at += 1;
                    open.pop();
                }
                _ => return false,
            }
        }
    }
}

/// `json_lex`'s blanks.
fn json_blanks(b: &[u8], at: &mut usize) {
    while matches!(b.get(*at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        *at += 1;
    }
}

/// An object member's key and its `:`, blanks around each.
fn json_member_key(b: &[u8], at: &mut usize) -> bool {
    json_blanks(b, at);
    if !(b.get(*at) == Some(&b'"') && json_string(b, at)) {
        return false;
    }
    json_blanks(b, at);
    let colon = b.get(*at) == Some(&b':');
    *at += usize::from(colon);
    colon
}

/// `json_lex_string` without de-escaping, its opening `"` at `at`.
fn json_string(b: &[u8], at: &mut usize) -> bool {
    *at += 1;
    loop {
        match b.get(*at) {
            Some(b'"') => {
                *at += 1;
                return true;
            }
            Some(b'\\') => match b.get(*at + 1) {
                Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => *at += 2,
                Some(b'u') => match b.get(*at + 2..*at + 6) {
                    Some(hex) if hex.iter().all(u8::is_ascii_hexdigit) => *at += 6,
                    _ => return false,
                },
                _ => return false,
            },
            Some(0x00..=0x1f) | None => return false,
            Some(_) => *at += 1,
        }
    }
}

/// `json_lex_number`: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`. A
/// letter, digit or `_` right after it would join the token in the server's
/// lexer and fail it; here it fails as the next token, which no position takes.
fn json_number(b: &[u8], at: &mut usize) -> bool {
    let digits = |at: &mut usize| {
        let start = *at;
        while b.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
        *at > start
    };
    *at += usize::from(b.get(*at) == Some(&b'-'));
    match b.get(*at) {
        Some(b'0') => *at += 1,
        Some(b'1'..=b'9') => {
            digits(at);
        }
        _ => return false,
    }
    if b.get(*at) == Some(&b'.') {
        *at += 1;
        if !digits(at) {
            return false;
        }
    }
    if matches!(b.get(*at), Some(b'e' | b'E')) {
        *at += 1;
        *at += usize::from(matches!(b.get(*at), Some(b'+' | b'-')));
        if !digits(at) {
            return false;
        }
    }
    true
}

/// One `jsonb` value, in the shape PostgreSQL stores and compares one in.
///
/// The variants are `JsonbValue`'s own type codes, in their order, which is
/// the fallback order `compareJsonbContainers` uses whenever two positions
/// hold different kinds (I41): an object outranks an array, an array outranks
/// every scalar, and a boolean outranks a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jsonb {
    Null,
    String(String),
    /// A JSON number is stored as a `numeric` and compared by `numeric_cmp`,
    /// which is exactly [`NumericKey`] — so `1`, `1.0` and `1e0` are one
    /// value, as they are for a bare `numeric` column.
    Number(NumericKey),
    Bool(bool),
    /// `raw_scalar` marks the pseudo-array a top-level scalar is stored in.
    /// `compareJsonbContainers` tests it before the element count and lets
    /// the count *overwrite* the answer, so a scalar sorts below a one- or
    /// many-element array and **above an empty one** (I41). Only
    /// `crate::key`'s `jsonb_key` ever sets it; a nested array is a real array
    /// at every depth.
    Array {
        raw_scalar: bool,
        items: Vec<Jsonb>,
    },
    /// Pairs in **storage** order — key length first, then bytes — which is
    /// the order the walk visits them in and is *not* the order the keys are
    /// compared in ([`Jsonb::cmp`]).
    Object(Vec<(String, Jsonb)>),
}

impl Jsonb {
    /// The heap this document holds, every nested container and string
    /// included.
    pub(crate) fn heap_bytes(&self) -> u64 {
        match self {
            Self::Null | Self::Bool(_) => 0,
            Self::String(text) => text.capacity() as u64,
            Self::Number(numeric) => numeric.heap_bytes(),
            Self::Array { items, .. } => {
                (items.capacity() * std::mem::size_of::<Jsonb>()) as u64
                    + items.iter().map(Self::heap_bytes).sum::<u64>()
            }
            Self::Object(pairs) => {
                (pairs.capacity() * std::mem::size_of::<(String, Jsonb)>()) as u64
                    + pairs
                        .iter()
                        .map(|(key, value)| key.capacity() as u64 + value.heap_bytes())
                        .sum::<u64>()
            }
        }
    }

    /// `JsonbValue.type`, which is the type-defined order itself (I41).
    fn rank(&self) -> u8 {
        match self {
            Self::Null => 0x0,
            Self::String(_) => 0x1,
            Self::Number(_) => 0x2,
            Self::Bool(_) => 0x3,
            Self::Array { .. } => 0x10,
            Self::Object(_) => 0x11,
        }
    }

    /// `compareJsonbContainers`, as a recursion rather than as a lockstep walk
    /// over two token streams; the two agree because both stop at the first
    /// position where the values differ. An object is ordered by its pair
    /// *count* before its first key, its keys *stored* by length-then-bytes
    /// (I41) and compared here bytewise.
    ///
    /// A string, a leaf or a key, is where this stops being PostgreSQL's
    /// answer: `compareJsonbScalarValue` orders it by `varstr_cmp` under the
    /// database's collation, which a plain dump does not record (I32), and
    /// `ComparisonDivergence::JsonbStringCollation` announces that.
    pub(crate) fn cmp(&self, other: &Self) -> Ordering {
        if self.rank() != other.rank() {
            return self.rank().cmp(&other.rank());
        }
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::String(a), Self::String(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (
                Self::Array { raw_scalar: raw_a, items: a },
                Self::Array { raw_scalar: raw_b, items: b },
            ) => {
                // Written in the source's own order, overwrite included: the
                // flag is decided first and the length then replaces the
                // answer rather than refining it.
                let mut container = Ordering::Equal;
                if raw_a != raw_b {
                    container = if *raw_a { Ordering::Less } else { Ordering::Greater };
                }
                if a.len() != b.len() {
                    container = a.len().cmp(&b.len());
                }
                container.then_with(|| first_difference(a.iter().zip(b).map(|(x, y)| x.cmp(y))))
            }
            (Self::Object(a), Self::Object(b)) => a.len().cmp(&b.len()).then_with(|| {
                first_difference(a.iter().zip(b).map(|((ka, va), (kb, vb))| {
                    ka.as_bytes().cmp(kb.as_bytes()).then_with(|| va.cmp(vb))
                }))
            }),
            _ => unreachable!("two `Jsonb` values of one rank are of one variant"),
        }
    }
}

/// How deep [`parse_jsonb`] will descend before refusing; a fixed number
/// rather than a stack-depth check (`docs/design/decisions.md`, "D55").
/// Nothing a `jsonb_out` field of a real dump holds comes near it.
const JSONB_MAX_DEPTH: usize = 1000;

/// The largest exponent `numeric_in` reads in a `jsonb` number, either sign:
/// `PG_INT32_MAX / 2` (I63).
const NUMERIC_EXPONENT_MAX: i64 = i32::MAX as i64 / 2;

/// A recursive-descent reader over one JSON document, implementing what
/// `jsonb_in` accepts and nothing wider (I41): RFC 8259 with PostgreSQL's
/// extra refusals — `\u0000`, which cannot be part of a `text` value, a lone
/// surrogate half, and a number past `numeric`'s bounds (I63).
struct JsonCursor<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
    /// Whether the read failed at a refusal of the server's, a marked one,
    /// rather than anywhere else ([`Unread`]).
    refused: bool,
}

impl<'a> JsonCursor<'a> {
    /// The four bytes RFC 8259 calls whitespace, which is also what the
    /// server's lexer skips — not `char::is_whitespace`, and not
    /// `array_isspace` either.
    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.bytes.get(self.at) == Some(&byte);
        self.at += usize::from(found);
        found
    }

    fn keyword(&mut self, word: &str) -> Option<()> {
        let end = self.at + word.len();
        (self.bytes.get(self.at..end)? == word.as_bytes()).then(|| self.at = end)
    }

    /// A run of one or more ASCII digits, as the number grammar's three
    /// positions all want.
    fn digits(&mut self) -> Option<&'a str> {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        if self.at == start {
            return None;
        }
        // ASCII digits, so this can only be valid UTF-8.
        std::str::from_utf8(&self.bytes[start..self.at]).ok()
    }

    /// One value, and the only place the depth is counted.
    fn value(&mut self) -> Option<Jsonb> {
        self.depth += 1;
        if self.depth > JSONB_MAX_DEPTH {
            return None;
        }
        let value = match self.bytes.get(self.at)? {
            b'{' => {
                self.at += 1;
                self.object()?
            }
            b'[' => {
                self.at += 1;
                self.array()?
            }
            b'"' => Jsonb::String(self.string()?),
            b't' => {
                self.keyword("true")?;
                Jsonb::Bool(true)
            }
            b'f' => {
                self.keyword("false")?;
                Jsonb::Bool(false)
            }
            b'n' => {
                self.keyword("null")?;
                Jsonb::Null
            }
            // Anything else is a number or nothing: a leading `+`, a bare
            // `.5` and `NaN` all fail inside, as the server's lexer fails
            // them, so every failure there is a refusal.
            _ => match self.number() {
                Some(number) => Jsonb::Number(number),
                None => return self.refuse(),
            },
        };
        self.depth -= 1;
        Some(value)
    }

    /// The body of an array, its `[` already eaten.
    fn array(&mut self) -> Option<Jsonb> {
        let mut items = Vec::new();
        self.skip_ws();
        if !self.eat(b']') {
            loop {
                self.skip_ws();
                items.push(self.value()?);
                self.skip_ws();
                if self.eat(b',') {
                    continue;
                }
                if self.eat(b']') {
                    break;
                }
                return None;
            }
        }
        Some(Jsonb::Array { raw_scalar: false, items })
    }

    /// The body of an object, its `{` already eaten. The pairs come out in
    /// storage order with duplicate keys resolved, which is the state a
    /// stored `jsonb` is always in.
    fn object(&mut self) -> Option<Jsonb> {
        let mut pairs: Vec<(String, Jsonb)> = Vec::new();
        self.skip_ws();
        if !self.eat(b'}') {
            loop {
                self.skip_ws();
                let key = self.string()?;
                self.skip_ws();
                if !self.eat(b':') {
                    return None;
                }
                self.skip_ws();
                pairs.push((key, self.value()?));
                self.skip_ws();
                if self.eat(b',') {
                    continue;
                }
                if self.eat(b'}') {
                    break;
                }
                return None;
            }
        }
        Some(Jsonb::Object(storage_order(pairs)))
    }

    /// A JSON string, its escapes resolved. An unescaped byte below `0x20` is
    /// refused, which is what the server refuses and what `escape_json` never
    /// writes — it renders every one of them as `\b`/`\f`/`\n`/`\r`/`\t` or a
    /// `\u00xx` (I41).
    // pg-refuses: I41 — an unescaped byte below `0x20`.
    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let byte = *self.bytes.get(self.at)?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let escape = *self.bytes.get(self.at)?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut buffer = [0u8; 4];
                            out.extend_from_slice(
                                self.unicode_escape()?.encode_utf8(&mut buffer).as_bytes(),
                            );
                        }
                        _ => return None,
                    }
                }
                0x00..=0x1f => return self.refuse(),
                // Every other byte is copied verbatim, and each is ASCII or
                // part of a multi-byte sequence copied whole — the input is a
                // `&str`, so `from_utf8` above never fails.
                _ => out.push(byte),
            }
        }
    }

    /// `\uXXXX`, its `\u` already eaten. Surrogates must come as a matched
    /// high-then-low pair, and `\u0000` is refused outright: `jsonb` stores
    /// strings as `text`, which cannot hold a NUL (I41).
    // pg-refuses: I41 — `\u0000`, and a surrogate out of its pair.
    fn unicode_escape(&mut self) -> Option<char> {
        let first = self.hex4()?;
        if first == 0 {
            return self.refuse();
        }
        if (0xd800..0xdc00).contains(&first) {
            if !(self.eat(b'\\') && self.eat(b'u')) {
                return self.refuse();
            }
            let second = self.hex4()?;
            if !(0xdc00..0xe000).contains(&second) {
                return self.refuse();
            }
            return char::from_u32(0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00));
        }
        // A low surrogate with no high half in front of it.
        if (0xdc00..0xe000).contains(&first) {
            return self.refuse();
        }
        char::from_u32(first)
    }

    /// Fail the read as the server refuses it.
    fn refuse<T>(&mut self) -> Option<T> {
        self.refused = true;
        None
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.at..self.at.checked_add(4)?)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        self.at += 4;
        u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
    }

    /// A JSON number: an optional `-`, then `0` or a digit run that does not
    /// start with `0`, then an optional `.` with at least one digit, then an
    /// optional `e`/`E` exponent with an optional sign. `01`, `+1`, `.5`,
    /// `1.` and `NaN` are refused, as the server refuses them (I41).
    ///
    /// The result is the [`NumericKey`] the stored `numeric` would compare by,
    /// so the exponent is applied by moving the decimal point rather than
    /// kept: `1e2`, `100` and `100.00` are one value (I41). `numeric_in`'s
    /// bounds are checked before the point moves, which keeps the digit string
    /// this builds within them (I63).
    // pg-refuses: I41 — `01`, `+1`, `.5`, `1.` and `NaN`.
    fn number(&mut self) -> Option<NumericKey> {
        let negative = self.eat(b'-');
        let int = self.digits()?;
        if int.len() > 1 && int.starts_with('0') {
            return None;
        }
        let frac = if self.eat(b'.') { self.digits()? } else { "" };
        let mut exponent: i64 = 0;
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            let signed = if self.eat(b'-') {
                true
            } else {
                self.eat(b'+');
                false
            };
            let digits = self.digits()?;
            // Parsed as `i64` and then bounded, so a run of digits too long
            // for the integer is refused rather than wrapping.
            exponent = digits.parse::<i64>().ok()?;
            if signed {
                exponent = -exponent;
            }
        }
        // The display scale is the fraction's digits less the exponent, and
        // the weight counts from the first non-zero digit: a zero has none,
        // and is stored whatever its exponent moves.
        // pg-refuses: I63 — an exponent past `PG_INT32_MAX / 2`, or a display
        // scale or a weight past the storage format's.
        if exponent.abs() > NUMERIC_EXPONENT_MAX
            || i64::try_from(frac.len()).ok()? - exponent > NUMERIC_DSCALE_MAX as i64
        {
            return None;
        }
        let digits = format!("{int}{frac}");
        let Some(first) = digits.bytes().position(|b| b != b'0') else {
            return Some(NumericKey::from_parts(false, "", ""));
        };
        // Where the point lands, counted in digits from the left of
        // `int ++ frac`. The two bounds above bound the padding either side.
        let point = i64::try_from(int.len()).ok()? + exponent;
        if point - i64::try_from(first).ok()? > NUMERIC_INTEGER_DIGITS_MAX as i64 {
            return None;
        }
        let width = i64::try_from(digits.len()).ok()?;
        Some(match point {
            _ if point <= 0 => {
                NumericKey::from_parts(negative, "", &format!("{}{digits}", zeros(-point)))
            }
            _ if point >= width => {
                NumericKey::from_parts(negative, &format!("{digits}{}", zeros(point - width)), "")
            }
            _ => {
                let split = usize::try_from(point).ok()?;
                NumericKey::from_parts(negative, &digits[..split], &digits[split..])
            }
        })
    }
}

/// `n` zeros, for the padding a moved decimal point needs on either side.
fn zeros(n: i64) -> String {
    "0".repeat(usize::try_from(n).unwrap_or(0))
}

/// `uniqueifyJsonbObject`: sort the pairs into storage order and drop every
/// duplicate key but the **last** one written.
///
/// Storage order is `lengthCompareJsonbString` — key length first, then
/// `memcmp` — so `{"z":1,"aa":2}` is stored, printed and walked in that order
/// rather than alphabetically (I41). The duplicate rule comes out of
/// `lengthCompareJsonbPair` breaking a tie on *descending* insertion order
/// while the uniqueify pass keeps the first of each run, which the `reverse`
/// here reproduces against a stable sort.
fn storage_order(mut pairs: Vec<(String, Jsonb)>) -> Vec<(String, Jsonb)> {
    pairs.reverse();
    pairs.sort_by(|(a, _), (b, _)| {
        a.len().cmp(&b.len()).then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });
    pairs.dedup_by(|(a, _), (b, _)| a == b);
    pairs
}

/// One `jsonb` document, parsed. `Err` for text the server's own parser would
/// refuse — which for a *field* is `Error::FieldDecode` and for a filter's
/// literal `Error::PredicateValueDecode`, the same two faults every other
/// comparison raises — and for a document nested past [`JSONB_MAX_DEPTH`],
/// [`Unread::Unparsed`], the server's bound being its `max_stack_depth`.
pub(crate) fn parse_jsonb(text: &str) -> Read<Jsonb> {
    let mut cursor = JsonCursor { bytes: text.as_bytes(), at: 0, depth: 0, refused: false };
    cursor.skip_ws();
    let value = cursor.value();
    cursor.skip_ws();
    match value {
        Some(value) if cursor.at == cursor.bytes.len() => Ok(value),
        _ if cursor.refused => Err(Unread::Refused),
        _ => Err(jsonb_unread(text)),
    }
}

/// Why `jsonb_in` reads no value here from `text`, [`JsonCursor`] having read
/// none and refused nothing: `jsonb_in`'s grammar is `json_in`'s, which the
/// cursor reads but past its depth, with refusals of its own besides, so a
/// text `json_in` refuses is refused ([`json_in`](crate::value::json::json_in)).
// pg-refuses: I41 — every text `json_in` refuses.
fn jsonb_unread(text: &str) -> Unread {
    if json_in(text) { Unread::Unparsed } else { Unread::Refused }
}

#[cfg(test)]
mod tests;
