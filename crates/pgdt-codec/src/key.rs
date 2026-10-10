//! A value's place in PostgreSQL's order, read from its text, and the field
//! checks a strict parse reads through the same readers.
//!
//! A key is a function of a column's [`CompareKind`] (or [`NestedCompare`])
//! and one unescaped text alone, so a field, a filter's literal, a stored
//! bound and a gathered value are ordered by one comparison: the filter's
//! (`pgdt_query::predicate`), the statistics gatherer's (`pgdt_query::gather`) and the
//! summaries' alike. What a literal of each kind has to look like, for the
//! refusal naming it, is here too, beside the grammar it describes.

use std::cmp::Ordering;

use arrow_buffer::i256;

use crate::datetime_in;
use crate::datetime_in::DateTimeInput;
use crate::decode;
use crate::decode::{NumericColumn, Read, Unread};
use crate::nested;
use crate::pgtype::{
    CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics, Discrete, Geometric,
    IntervalQualifier, NestedCompare, Position, TextGrammar,
};
use pgdt_model::PostgresInvalidValues;

/// One side of a bare-`numeric` comparison: the sign, and the digits either
/// side of the point with the *insignificant* ones removed — leading zeros
/// from the integer part, trailing zeros from the fraction.
///
/// Normalizing is what makes this PostgreSQL's own order: `cmp_numerics`
/// compares by value and never by display scale (I33), so `1.5` and `1.50`
/// are one value that a bare `numeric` column writes two ways.
///
/// Digit *strings* rather than a big integer, because the column is
/// arbitrary-precision: the file may hold a thousand digits, past every
/// fixed-width type including `i256`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericKey {
    /// False for a zero of either written sign — PostgreSQL has one zero, and
    /// a filter literal is free to spell it `-0`.
    negative: bool,
    /// Integer digits, leading zeros stripped; empty for a magnitude below 1.
    int: String,
    /// Fraction digits, trailing zeros stripped; empty for an integer.
    frac: String,
}

impl NumericKey {
    /// The heap its two digit strings hold.
    fn heap_bytes(&self) -> u64 {
        (self.int.capacity() + self.frac.capacity()) as u64
    }

    /// `[-]digits[.digits]`: no exponent, no sign but `-`, at least one digit
    /// somewhere, and a point at either end or leading zeros allowed, which
    /// `numeric_out` never writes ([`accepted_form`] lists them). It is the same
    /// *lexical* grammar [`decode::typmod_unscaled_digits`] accepts for a
    /// typmod'd column's field, which is read through it and
    /// [`Self::from_unscaled`] instead; this reads a bare column's field and
    /// every literal. The storage format's bounds are the caller's
    /// ([`decode::numeric_in_stores`]), a typmod'd column's field never
    /// reaching them.
    fn parse(text: &str) -> Option<Self> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self::from_parts(negative, int, frac))
    }

    /// An unscaled digit string at `scale` places, as
    /// [`decode::typmod_unscaled_digits`] writes it, split at its point.
    fn from_unscaled(unscaled: &str, scale: i16) -> Self {
        let (negative, digits) = match unscaled.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, unscaled),
        };
        match usize::try_from(scale) {
            Ok(places) if places >= digits.len() => Self::from_parts(
                negative,
                "",
                &format!("{}{digits}", "0".repeat(places - digits.len())),
            ),
            Ok(places) => {
                let (int, frac) = digits.split_at(digits.len() - places);
                Self::from_parts(negative, int, frac)
            }
            Err(_) => {
                let zeros = "0".repeat(usize::from(scale.unsigned_abs()));
                Self::from_parts(negative, &format!("{digits}{zeros}"), "")
            }
        }
    }

    /// Drop the insignificant digits from an already-split sign/integer part/
    /// fraction, which is the whole of the normalization. Shared: a `numeric`
    /// field is split straight out of the text, while a `jsonb` number's
    /// point has to be moved by its exponent first.
    fn from_parts(negative: bool, int: &str, frac: &str) -> Self {
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        Self {
            negative: negative && !(int.is_empty() && frac.is_empty()),
            int: int.to_string(),
            frac: frac.to_string(),
        }
    }

    /// Sign first, then magnitude: how many integer digits, then those digits,
    /// then the fraction. Each stage is a byte comparison over ASCII digits,
    /// which is why the normalization above has to have happened — with
    /// trailing zeros stripped, a fraction that is a prefix of another is the
    /// smaller of the two.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => return Ordering::Greater,
            (true, false) => return Ordering::Less,
            _ => {}
        }
        let magnitude = self
            .int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.as_bytes().cmp(other.int.as_bytes()))
            .then_with(|| self.frac.as_bytes().cmp(other.frac.as_bytes()));
        if self.negative { magnitude.reverse() } else { magnitude }
    }
}

/// One side of an `inet`/`cidr` comparison: the family, the netmask length,
/// and the address left-aligned in sixteen bytes.
///
/// Not a byte key, and it cannot be made into one: `network_cmp_internal`
/// compares the *shorter* netmask's worth of address bits first (I40), so how
/// many bits are significant depends on the value being compared against.
/// The pair is compared, not two independently sortable keys.
///
/// `v6` is the family; `PGSQL_AF_INET6` is `PGSQL_AF_INET + 1`, so an IPv4
/// address sorts below every IPv6 one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkKey {
    v6: bool,
    bits: u8,
    /// The address, left-aligned: an IPv4 address occupies the first four
    /// bytes and the rest are zero, which is what `bitncmp` reads, never
    /// looking past `maxbits`.
    addr: [u8; 16],
}

impl NetworkKey {
    /// `ip_maxbits` — the family's address width.
    fn maxbits(&self) -> u8 {
        if self.v6 { 128 } else { 32 }
    }

    /// `network_cmp_internal` (I40): family, then the shorter netmask's
    /// worth of address bits, then the netmask length, then the whole
    /// address.
    fn cmp(&self, other: &Self) -> Ordering {
        if self.v6 != other.v6 {
            return self.v6.cmp(&other.v6);
        }
        let shared = self.bits.min(other.bits);
        bitncmp(&self.addr, &other.addr, shared)
            .then_with(|| self.bits.cmp(&other.bits))
            .then_with(|| bitncmp(&self.addr, &other.addr, self.maxbits()))
    }
}

/// PostgreSQL's `bitncmp`: the first `n` bits of two addresses, most
/// significant first. Whole bytes by `memcmp`, then the straddling byte's
/// remaining bits under a high-bit mask — the same answer as `bitncmp`'s
/// bit-at-a-time loop, which stops at the first differing bit.
fn bitncmp(left: &[u8; 16], right: &[u8; 16], n: u8) -> Ordering {
    let whole = usize::from(n / 8);
    let full = left[..whole].cmp(&right[..whole]);
    let spare = n % 8;
    if full != Ordering::Equal || spare == 0 {
        return full;
    }
    let mask = 0xffu8 << (8 - spare);
    (left[whole] & mask).cmp(&(right[whole] & mask))
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
    /// [`jsonb_key`] ever sets it; a nested array is a real array at every
    /// depth.
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
    fn heap_bytes(&self) -> u64 {
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
    /// *count* before its first key, and its keys are compared by
    /// `varstr_cmp` while being *stored* by length-then-bytes (I41).
    ///
    /// A string leaf is where this stops being PostgreSQL's answer:
    /// `compareJsonbScalarValue` orders it by the database's collation, which
    /// a plain dump does not record (I32), and
    /// `ComparisonDivergence::JsonbStringCollation` announces that.
    fn cmp(&self, other: &Self) -> Ordering {
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

/// The first non-`Equal` answer, or `Equal` when there is none — the
/// member-wise half of [`Jsonb::cmp`].
fn first_difference(mut answers: impl Iterator<Item = Ordering>) -> Ordering {
    answers.find(|answer| answer.is_ne()).unwrap_or(Ordering::Equal)
}

/// How deep [`parse_jsonb`] will descend before refusing; a fixed number
/// rather than a stack-depth check (`docs/design/decisions.md`, "D55").
/// Nothing a `jsonb_out` field of a real dump holds comes near it.
const JSONB_MAX_DEPTH: usize = 1000;

/// The largest exponent `numeric_in` reads in a `jsonb` number, either sign:
/// `PG_INT32_MAX / 2` (I63).
const NUMERIC_EXPONENT_MAX: i64 = i32::MAX as i64 / 2;

/// A recursive-descent reader over one JSON document, implementing what
/// `jsonb_in` accepts and nothing wider (I41): RFC 8259 with PostgreSQL's two
/// extra refusals — `\u0000`, which cannot be part of a `text` value, and a
/// lone surrogate half.
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
            || i64::try_from(frac.len()).ok()? - exponent > decode::NUMERIC_DSCALE_MAX as i64
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
        if point - i64::try_from(first).ok()? > decode::NUMERIC_INTEGER_DIGITS_MAX as i64 {
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
fn parse_jsonb(text: &str) -> Read<Jsonb> {
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
/// text `json_in` refuses is refused ([`decode::json_in`]).
// pg-refuses: I41 — every text `json_in` refuses.
fn jsonb_unread(text: &str) -> Unread {
    if decode::json_in(text) { Unread::Unparsed } else { Unread::Refused }
}

/// A `jsonb` comparison key: the document, with a **top-level scalar wrapped
/// in the one-element pseudo-array PostgreSQL stores it in** (I41). Modelling
/// the wrapper is what gives `1 < [1]`, `1 < [1,2]` and `1 > []`.
fn jsonb_key(text: &str) -> Read<OrderKey> {
    Ok(OrderKey::Jsonb(match parse_jsonb(text)? {
        container @ (Jsonb::Array { .. } | Jsonb::Object(_)) => container,
        scalar => Jsonb::Array { raw_scalar: true, items: vec![scalar] },
    }))
}

/// One side of an ordering comparison, decoded from text per the column's
/// [`CompareKind`]. Both sides of any one comparison come from the same kind,
/// so a *finite* variant mismatch is unreachable by construction.
///
/// Three variants are not values of the column's Arrow type at all:
/// `infinity`, `-infinity` and `numeric`'s `NaN` are legal values of their
/// declared PostgreSQL types with a total order (I34), and are carried as
/// their position in that order (`docs/design/decisions.md`, "D56"). A
/// `real`/`double precision` special is not here: IEEE has all three, so the
/// column's own decoder yields them inside `Float` and [`pg_float_cmp`]
/// orders them PostgreSQL's way.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderKey {
    /// PostgreSQL's `-infinity`, below every finite value of its type.
    NegativeInfinity,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(i256),
    /// A bare `numeric`, whose digits do not fit any fixed-width integer.
    Numeric(NumericKey),
    /// An `interval`'s span in microseconds, which is 128 bits wide because
    /// PostgreSQL's own `interval_cmp_value` is (I40).
    Interval(i128),
    /// An `interval`'s three fields as `Interval(MonthDayNano)` holds them —
    /// months, days, microseconds — compared one after another, as Arrow
    /// compares that type.
    IntervalFields(i32, i32, i64),
    /// A `time with time zone`: the UTC-equivalent instant, then the stored
    /// zone as PostgreSQL stores it — seconds *west* of GMT, the negation of
    /// the sign the value displays.
    TimeTz {
        utc: i64,
        zone: i64,
    },
    Network(NetworkKey),
    /// A `jsonb` document, with a top-level scalar already wrapped in the
    /// pseudo-array PostgreSQL stores it in — see [`jsonb_key`].
    Jsonb(Jsonb),
    Bytes(Vec<u8>),
    Text(String),
    /// PostgreSQL's `infinity`, above every finite value of its type.
    PositiveInfinity,
    /// `numeric`'s `NaN`, which orders above `infinity` and equals itself
    /// (I34). Reached through [`CompareKind::Decimal`] and
    /// [`CompareKind::Numeric`].
    NotANumber,
}

/// The rank of a finite value — the second of the four [`OrderKey::rank`]
/// classes, above `-infinity` and below `infinity` and `NaN`, and the only one whose members are compared by value.
const FINITE: u8 = 1;

impl OrderKey {
    /// Where this key sits in PostgreSQL's total order relative to the finite
    /// values of its own type. Ranks are compared before values are, so a
    /// special value never collides with a finite one a sentinel could have —
    /// `i64::MAX` micros since 1970 is a date PostgreSQL itself accepts
    /// (`docs/design/decisions.md`, "D56").
    fn rank(&self) -> u8 {
        match self {
            Self::NegativeInfinity => 0,
            Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Decimal(_)
            | Self::Numeric(_)
            | Self::Interval(_)
            | Self::IntervalFields(..)
            | Self::TimeTz { .. }
            | Self::Network(_)
            | Self::Jsonb(_)
            | Self::Bytes(_)
            | Self::Text(_) => FINITE,
            Self::PositiveInfinity => 2,
            Self::NotANumber => 3,
        }
    }
}

/// PostgreSQL's special values, for the kinds whose columns can hold one and
/// in the exact spelling that type's own `*_out` writes (I34): `date`,
/// `timestamp`, `timestamptz` and `interval` write `infinity`/`-infinity`,
/// and a `numeric` writes `NaN`, a bare one its infinities too (below).
/// Nothing else is accepted — a `date` field or
/// literal reading `Infinity` is a decode failure
/// (`docs/design/decisions.md`, "D55").
///
/// `interval`'s two are read on every file: they are v17 values no older
/// server could have written, so accepting the spelling unconditionally is
/// the union rule (I35) rather than a claim about the file's own major.
/// `real`/`double precision` are absent because IEEE represents all three and
/// [`decode::decode_f64`] already returns them.
///
/// The two `numeric` kinds differ only about the infinities (I34): a
/// [`CompareKind::Decimal`] column, and a `numeric(p,s)` past 76 digits, can
/// hold a `NaN` and never an infinity, while a *bare* `numeric` has all three
/// in `numeric_out`'s own spellings, which capitalize where `date_out`'s do
/// not.
pub fn special_order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    match kind {
        CompareKind::Date
        | CompareKind::Timestamp { .. }
        | CompareKind::Interval { .. }
        | CompareKind::IntervalFields { .. } => match text {
            "infinity" => Some(OrderKey::PositiveInfinity),
            "-infinity" => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        CompareKind::Decimal { .. } => (text == "NaN").then_some(OrderKey::NotANumber),
        CompareKind::Numeric { infinities, .. } => match text {
            "NaN" => Some(OrderKey::NotANumber),
            "Infinity" if *infinities => Some(OrderKey::PositiveInfinity),
            "-Infinity" if *infinities => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        _ => None,
    }
}

/// `interval_cmp_value`'s span, in microseconds: months collapse to 30 days,
/// days to 86400 seconds, and the time field is added on (I40), so `1 mon`,
/// `30 days` and `720:00:00` are one value written three ways — which is why
/// an `interval` cannot canonicalize once and compare bytewise.
///
/// The walk over the text is [`decode::interval_parts`], shared with the
/// decoder, so a field the decoder refuses includes every literal this
/// refuses — and a time part past what Arrow's nanoseconds hold, besides. The
/// fusing is this function's alone: a decoder that did it would lose the
/// fields Arrow carries separately.
fn interval_span(text: &str) -> Read<i128> {
    let (months, days, time) = decode::interval_parts(text)?;
    let whole_days = i128::from(months) * 30 + i128::from(days);
    Ok(whole_days * 86_400_000_000 + i128::from(time))
}

/// A `time with time zone`, split into the UTC-equivalent instant and the
/// zone PostgreSQL stores — seconds *west* of GMT, the negation of the offset
/// the value displays. `timetz_cmp_internal` sorts by the first and breaks
/// ties with the second (I40), so `00:00:00+00` and `01:00:00+01` are the
/// same instant and still not equal.
fn timetz_key(text: &str) -> Read<OrderKey> {
    let (time_only, displayed) = decode::extract_offset(text)?;
    let (seconds, micros) = decode::parse_time_of_day(time_only)?;
    let zone = -displayed;
    // Both parts are bounded above, so neither sum can overflow.
    let utc = seconds * 1_000_000 + micros + zone * 1_000_000;
    Ok(OrderKey::TimeTz { utc, zone })
}

/// An `inet` or `cidr` value, as `network_in` reads it
/// ([`decode::network_in`]): `pg_inet_net_ntop`'s `<address>[/<bits>]`, with
/// `cidr_out` always appending the netmask and `inet_out` omitting it when it
/// is the family's full width (I40), and every other spelling the server
/// reads, a `cidr`'s abbreviated network among them. Every refusal is the
/// server's (I67), a `cidr`'s bit set below its netmask included: the *only*
/// thing separating the two types, their comparison being identical.
fn network_key(text: &str, cidr: bool) -> Read<OrderKey> {
    let (v6, bits, addr) = decode::network_in(text, cidr).ok_or(Unread::Refused)?;
    Ok(OrderKey::Network(NetworkKey { v6, bits, addr }))
}

/// A `macaddr`/`macaddr8` value: `octets` hex pairs of either case joined by
/// colons, `macaddr_out` and `macaddr8_out` writing lowercase (I40). The
/// server's input function takes other separator conventions and this takes
/// none of them (`docs/design/decisions.md`, "D55").
fn macaddr_key(text: &str, octets: usize) -> Option<OrderKey> {
    let mut bytes = Vec::with_capacity(octets);
    for part in text.split(':') {
        // The digit check is not what `from_str_radix` does: it accepts a
        // leading sign, so `+f` would otherwise pass as a two-character pair.
        if part.len() != 2 || bytes.len() == octets || !part.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        bytes.push(u8::from_str_radix(part, 16).ok()?);
    }
    (bytes.len() == octets).then_some(OrderKey::Bytes(bytes))
}

/// Decode one already-COPY-unescaped value into a comparable key. `None` when
/// the text is not a value of that type — for a *field* `Error::FieldDecode`,
/// for the filter's own literal `Error::PredicateValueDecode`, raised before
/// a row is read.
///
/// A special value is answered by [`special_order_key`] first: it is a legal
/// value of the declared type that the *Arrow* type cannot hold, a separate
/// population from text that is malformed for the column.
///
/// **A key is a function of the kind and the text alone**, and nothing here
/// or in [`compare_keys`] reads a row: two values that never shared one — a
/// field and a filter's literal, a stored bound and a literal, two neighbours
/// being gathered — are ordered by the one key the filter uses.
pub fn order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    field_key(kind, text).ok()
}

/// A field's key as a query reads the field: [`order_key`]'s, but for a float
/// its type's `*_in` refuses, keyed under [`PostgresInvalidValues::Ignore`] as
/// the typed read emits it ([`decode::float_field`]), so a filter and the
/// batch agree on it.
pub fn read_key(
    kind: &CompareKind,
    text: &str,
    invalid: PostgresInvalidValues,
) -> Option<OrderKey> {
    match (field_key(kind, text), kind) {
        (Ok(key), _) => Some(key),
        (Err(Unread::Refused), CompareKind::Float32) => {
            Some(OrderKey::Float(f64::from(decode::float_field::<f32>(text, invalid)?)))
        }
        (Err(Unread::Refused), CompareKind::Float64) => {
            Some(OrderKey::Float(decode::float_field::<f64>(text, invalid)?))
        }
        (Err(_), _) => None,
    }
}

/// Which side of a comparison [`nested_key`] reads: a filter's literal, or a
/// field, read as the query reads one its type's `*_in` refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Literal,
    Field(PostgresInvalidValues),
}

/// `NAMEDATALEN`: `enum_in` refuses a text of this many bytes or more before
/// looking it up, no label being so long (I70).
const ENUM_LABEL_LIMIT: usize = 64;

/// [`order_key`], saying why a text has no key: refused by the type's `*_in`,
/// which fails the parse reading it as a field, or not read here.
fn field_key(kind: &CompareKind, text: &str) -> Read<OrderKey> {
    if let Some(special) = special_order_key(kind, text) {
        return Ok(special);
    }
    let unparsed = Unread::Unparsed;
    Ok(match kind {
        CompareKind::Bool => {
            OrderKey::Bool(decode::decode_bool(text).ok_or_else(|| decode::bool_unread(text))?)
        }
        // A literal in DataFusion's semantics is read as an `i64` instead, by
        // [`literal_key`].
        CompareKind::Int { bytes } => OrderKey::Int(int_in(text, *bytes)?),
        // `u32`, and the width *is* the refusal: `oidin` reads `-1` as
        // 4294967295 and this build does not implement that wrap, so a
        // negative literal is `Error::PredicateValueDecode`. Every value the
        // column can hold widens into `i64` unchanged.
        CompareKind::UnsignedInt => {
            OrderKey::Int(text.parse::<u32>().map_err(|_| decode::oid_unread(text))?.into())
        }
        CompareKind::Float32 => OrderKey::Float(f64::from(decode::float_in::<f32>(text)?)),
        CompareKind::Float64 => OrderKey::Float(decode::float_in::<f64>(text)?),
        // A field is put through its typmod, rounded to the scale and refused
        // past the precision as `COPY` refuses it (I51). A literal is read
        // exactly, by [`literal_key`].
        CompareKind::Decimal { precision, scale } => OrderKey::Decimal(
            i256::from_string(&decode::typmod_unscaled_digits(
                text,
                *precision,
                i16::from(*scale),
            )?)
            .ok_or(unparsed)?,
        ),
        CompareKind::Numeric { typmod: Some(typmod), .. } => {
            OrderKey::Numeric(NumericKey::from_unscaled(
                &decode::typmod_unscaled_digits(text, typmod.precision, typmod.scale)?,
                typmod.scale,
            ))
        }
        // A bare column's field is bounded by `numeric_in` (I63) once read:
        // the bounds count the digits a grammar with no exponent reads, and
        // `numeric_in` stores a text whose exponent brings its scale back
        // within them. A typmod'd one never reaches the bounds, the typmod
        // rounding first. A literal in PostgreSQL's semantics is bounded by
        // [`literal_key`].
        CompareKind::Numeric { infinities: bare, typmod: None } => {
            let key = NumericKey::parse(text).ok_or_else(|| {
                let column = if *bare { NumericColumn::Bare } else { NumericColumn::UnreadTypmod };
                decode::numeric_unread(text, column)
            })?;
            if *bare && !decode::numeric_in_stores(text) {
                return Err(Unread::Refused);
            }
            OrderKey::Numeric(key)
        }
        // A label the type does not declare is no value of the column, which
        // `enum_in` refuses: where the declared labels are all the type
        // holds, or the text is longer than any label can be, and otherwise
        // it may be one this build did not read. The linear scan is over a
        // label list, a handful of entries in practice.
        // pg-refuses: I70
        CompareKind::Enum { labels, exact } => {
            let position = labels.iter().position(|l| l == text);
            let refused = *exact || text.len() >= ENUM_LABEL_LIMIT;
            OrderKey::Int(position.ok_or(if refused { Unread::Refused } else { unparsed })? as i64)
        }
        CompareKind::Date => {
            OrderKey::Int(datetime_read(decode::date_days(text), DateTimeInput::Date, text)?.into())
        }
        CompareKind::Time => OrderKey::Int(datetime_read(
            decode::time_of_day_micros(text),
            DateTimeInput::Time,
            text,
        )?),
        // From PostgreSQL's epoch, not Arrow's: every value the server admits
        // keys, those past what `i64` counts from 1970 among them (I49).
        CompareKind::Timestamp { with_tz } => {
            let input =
                if *with_tz { DateTimeInput::TimestampTz } else { DateTimeInput::Timestamp };
            OrderKey::Int(datetime_read(
                decode::timestamp_postgres_micros(text, *with_tz),
                input,
                text,
            )?)
        }
        // A field the reader does not read is refused where `interval_in`
        // refuses it under the column's qualifier. A literal is read under
        // none ([`literal_key`]).
        CompareKind::Interval { qualifier } => {
            OrderKey::Interval(interval_read(interval_span(text), text, *qualifier)?)
        }
        CompareKind::IntervalFields { qualifier } => {
            let (months, days, time) =
                interval_read(decode::interval_parts(text), text, *qualifier)?;
            OrderKey::IntervalFields(months, days, time)
        }
        CompareKind::TimeTz => datetime_read(timetz_key(text), DateTimeInput::TimeTz, text)?,
        CompareKind::Network { cidr } => network_key(text, *cidr)?,
        CompareKind::MacAddr { octets } => {
            macaddr_key(text, *octets).ok_or_else(|| match octets {
                8 if decode::macaddr8_in(text).is_none() => Unread::Refused,
                8 => unparsed,
                _ => decode::macaddr_unread(text),
            })?
        }
        CompareKind::Jsonb => jsonb_key(text)?,
        // Every refusal of the reader is `uuid_in`'s (I64).
        CompareKind::Uuid => {
            OrderKey::Bytes(decode::decode_uuid(text).ok_or(Unread::Refused)?.to_vec())
        }
        CompareKind::Bytea => {
            OrderKey::Bytes(decode::decode_bytea(text).ok_or_else(|| decode::bytea_unread(text))?)
        }
        // A field past its typmod's length is refused, and one within it is
        // keyed as written. A literal is coerced with no typmod
        // ([`literal_key`]).
        CompareKind::Text { length } => {
            if length.is_some_and(|length| decode::char_typmod_refuses(text, length)) {
                return Err(Unread::Refused);
            }
            OrderKey::Text(text.to_string())
        }
        // `bcTruelen` on both sides, which is what makes this the server's
        // comparison rather than one over the padding (I38). The blank is
        // ASCII `0x20` and nothing else; a tab is a value byte.
        CompareKind::PaddedText { length } => {
            if length.is_some_and(|length| decode::char_typmod_refuses(text, length)) {
                return Err(Unread::Refused);
            }
            OrderKey::Text(text.trim_end_matches(' ').to_string())
        }
    })
}

/// A date, time or timestamp reader's answer, a text it reads no value from
/// refused where no major's grammar reads it under any setting
/// ([`datetime_in::datetime_reads`]). The readers read `*_out`'s form and
/// little more, splitting a text otherwise than `ParseDateTime` does where it
/// is not that form, so a refusal of theirs is put to the grammar too.
fn datetime_read<T>(read: Read<T>, input: DateTimeInput, text: &str) -> Read<T> {
    read.map_err(|_| {
        if datetime_in::datetime_reads(input, text) { Unread::Unparsed } else { Unread::Refused }
    })
}

/// [`datetime_read`] for an `interval` of a column qualified by `qualifier`
/// ([`datetime_in::interval_reads`]).
fn interval_read<T>(read: Read<T>, text: &str, qualifier: Option<IntervalQualifier>) -> Read<T> {
    read.map_err(|_| {
        if datetime_in::interval_reads(text, qualifier) {
            Unread::Unparsed
        } else {
            Unread::Refused
        }
    })
}

/// A filter literal's key, which is [`order_key`]'s — a field's — but for an
/// integer in DataFusion's semantics, read as an `i64` whatever the column's
/// width, a float in DataFusion's semantics, read as the parse rounds it, an
/// infinity past the type's range, and a `numeric` in either: read within `numeric_in`'s bounds in
/// PostgreSQL's, and exactly, neither rounded to the scale nor refused past
/// the precision, the server coercing a literal with no typmod where it puts a
/// field through the typmod (I63) — and a text of any length and an `interval`
/// under no field qualifier for the same reason. In DataFusion's semantics a literal is
/// DataFusion's value, not the server's (`roadmap.md`, "A literal is
/// guaranteed in `*_out`'s form and never read past `*_in`'s").
pub fn literal_key(
    kind: &CompareKind,
    text: &str,
    semantics: ComparisonSemantics,
) -> Option<OrderKey> {
    match (semantics, kind) {
        (ComparisonSemantics::DataFusion, CompareKind::Int { .. }) => {
            Some(OrderKey::Int(text.parse::<i64>().ok()?))
        }
        (ComparisonSemantics::DataFusion, CompareKind::Float32) => {
            Some(OrderKey::Float(f64::from(text.parse::<f32>().ok()?)))
        }
        (ComparisonSemantics::DataFusion, CompareKind::Float64) => {
            Some(OrderKey::Float(text.parse::<f64>().ok()?))
        }
        (
            ComparisonSemantics::Postgres,
            CompareKind::Decimal { .. } | CompareKind::Numeric { .. },
        ) if !decode::numeric_in_stores(text) => None,
        (_, CompareKind::Decimal { scale, .. }) => special_order_key(kind, text).or_else(|| {
            let unscaled = decode::decimal_unscaled_digits(text, *scale)?;
            Some(OrderKey::Decimal(i256::from_string(&unscaled)?))
        }),
        (_, CompareKind::Numeric { typmod: Some(_), .. }) => special_order_key(kind, text)
            .or_else(|| Some(OrderKey::Numeric(NumericKey::parse(text)?))),
        (_, CompareKind::Text { .. }) => order_key(&CompareKind::Text { length: None }, text),
        (_, CompareKind::PaddedText { .. }) => {
            order_key(&CompareKind::PaddedText { length: None }, text)
        }
        (_, CompareKind::Interval { .. }) => {
            order_key(&CompareKind::Interval { qualifier: None }, text)
        }
        (_, CompareKind::IntervalFields { .. }) => {
            order_key(&CompareKind::IntervalFields { qualifier: None }, text)
        }
        _ => order_key(kind, text),
    }
}

/// A `smallint`, `integer` or `bigint`, of `bytes` bytes, field or literal,
/// read as `int2in`, `int4in` and `int8in` read it. Rust's parse is the
/// grammar: the digits `*_out` writes, and of `*_in`'s a leading `+` and
/// leading zeros. A blank around the number, an underscore between digits and
/// a `0x`, `0o` or `0b` prefix, which the server reads, are unparsed
/// (`docs/design/decisions.md`, "D55"), and every other text it does not read
/// is refused ([`decode::int_unread`]).
///
/// **A value past the column's width is refused, as the server refuses it**
/// (I60): `70000` is no value of a `smallint` column, where a literal in
/// DataFusion's semantics is read as an `i64` ([`literal_key`]).
// pg-refuses: I60 — out of range for the column's width.
pub fn int_in(text: &str, bytes: u8) -> Read<i64> {
    let value = text.parse::<i64>().map_err(|e| match e.kind() {
        std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
            Unread::Refused
        }
        _ => decode::int_unread(text, int_range(bytes)),
    })?;
    if int_range(bytes).contains(&value) { Ok(value) } else { Err(Unread::Refused) }
}

/// The values a signed integer of `bytes` bytes holds.
pub(crate) fn int_range(bytes: u8) -> std::ops::RangeInclusive<i64> {
    match bytes {
        2 => i16::MIN.into()..=i16::MAX.into(),
        4 => i32::MIN.into()..=i32::MAX.into(),
        _ => i64::MIN..=i64::MAX,
    }
}

/// PostgreSQL's float order, not Rust's: `NaN` is greater than every other
/// value, infinities included, and `NaN = NaN` is true (I33), where Rust's
/// `partial_cmp` answers `None`. `real`/`double precision` are the only
/// columns whose key is a float `NaN`: a `NaN` in a `numeric(p,s)` column is
/// keyed as a special value ([`special_order_key`]) before any float is read,
/// having no `Decimal128` representation (I4).
fn pg_float_cmp(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).expect("neither side is NaN"),
    }
}

/// Rank first, value second. A special value on either side is decided by
/// [`OrderKey::rank`] alone, so two of the same special are equal —
/// `-infinity = -infinity`, `infinity = infinity`, `NaN = NaN` (I34) — and a
/// special against a finite never has to name a number.
///
/// Over the keys of one kind this is a **total order** — reflexive,
/// antisymmetric and transitive — which a running minimum, maximum or
/// sortedness flag depends on and a single comparison does not; the oracle
/// test `the_key_is_a_total_order_over_every_committed_value` pins it.
pub fn compare_keys(a: &OrderKey, b: &OrderKey) -> Ordering {
    let (rank_a, rank_b) = (a.rank(), b.rank());
    if rank_a != rank_b {
        return rank_a.cmp(&rank_b);
    }
    match (a, b) {
        (OrderKey::Bool(x), OrderKey::Bool(y)) => x.cmp(y),
        (OrderKey::Int(x), OrderKey::Int(y)) => x.cmp(y),
        (OrderKey::Float(x), OrderKey::Float(y)) => pg_float_cmp(*x, *y),
        (OrderKey::Decimal(x), OrderKey::Decimal(y)) => x.cmp(y),
        (OrderKey::Numeric(x), OrderKey::Numeric(y)) => x.cmp(y),
        (OrderKey::Interval(x), OrderKey::Interval(y)) => x.cmp(y),
        (OrderKey::IntervalFields(m, d, t), OrderKey::IntervalFields(n, e, u)) => {
            (m, d, t).cmp(&(n, e, u))
        }
        (
            OrderKey::TimeTz { utc: utc_x, zone: zone_x },
            OrderKey::TimeTz { utc: utc_y, zone: zone_y },
        ) => utc_x.cmp(utc_y).then_with(|| zone_x.cmp(zone_y)),
        (OrderKey::Network(x), OrderKey::Network(y)) => x.cmp(y),
        (OrderKey::Jsonb(x), OrderKey::Jsonb(y)) => x.cmp(y),
        (OrderKey::Bytes(x), OrderKey::Bytes(y)) => x.cmp(y),
        (OrderKey::Text(x), OrderKey::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ if rank_a != FINITE => Ordering::Equal,
        _ => unreachable!("both sides of a comparison decode through one column's `CompareKind`"),
    }
}

/// A value's place in its column's order, opaque outside this module: the key
/// a filter orders a row by, for a caller ordering values that meet no filter
/// — the statistics gatherer's running bounds and sortedness.
#[derive(Debug, Clone)]
pub struct ValueKey(OrderKey);

impl ValueKey {
    /// The key of one unescaped value, `None` when the text is not a value of
    /// `kind` ([`order_key`]).
    pub fn of(kind: &CompareKind, text: &str) -> Option<Self> {
        order_key(kind, text).map(Self)
    }

    /// [`Self::of`] a field, saying why it has no key: the statistics
    /// gatherer fails the parse on one the type's `*_in` refuses.
    pub fn of_field(kind: &CompareKind, text: &str) -> Read<Self> {
        field_key(kind, text).map(Self)
    }

    /// [`compare_keys`]: a total order over the keys of one kind, the one a
    /// filter compares by — what a test holds [`Self::stored_order`] against.
    #[cfg(any(test, feature = "test-support"))]
    pub fn compare(&self, other: &Self) -> Ordering {
        compare_keys(&self.0, &other.0)
    }

    /// **The order a stored bound and a block's row order are kept in**:
    /// [`compare_keys`], the order a filter compares by, with a float's `-0`
    /// below `0` where it ties them, as IEEE `totalOrder` — the order Arrow
    /// sorts a float by and DataFusion takes its `MIN`/`MAX` in — places
    /// them. A refinement, so an extreme in it is one in the filter's order
    /// and a block sorted in it is sorted in that order too
    /// (`docs/design/decisions.md`, "D79").
    pub fn stored_order(&self, other: &Self) -> Ordering {
        compare_keys(&self.0, &other.0).then_with(|| match (&self.0, &other.0) {
            (OrderKey::Float(x), OrderKey::Float(y)) => x.total_cmp(y),
            _ => Ordering::Equal,
        })
    }

    /// The heap this key holds, as the sizes the allocator was asked for:
    /// what gathering charges a keyed bound or row-order value
    /// (`pgdt_query::statistics::StatisticsAccount`).
    pub fn heap_bytes(&self) -> u64 {
        match &self.0 {
            OrderKey::Numeric(numeric) => numeric.heap_bytes(),
            OrderKey::Jsonb(jsonb) => jsonb.heap_bytes(),
            OrderKey::Bytes(bytes) => bytes.capacity() as u64,
            OrderKey::Text(text) => text.capacity() as u64,
            OrderKey::NegativeInfinity
            | OrderKey::Bool(_)
            | OrderKey::Int(_)
            | OrderKey::Float(_)
            | OrderKey::Decimal(_)
            | OrderKey::Interval(_)
            | OrderKey::IntervalFields(..)
            | OrderKey::TimeTz { .. }
            | OrderKey::Network(_)
            | OrderKey::PositiveInfinity
            | OrderKey::NotANumber => 0,
        }
    }
}

/// One side of a **nested** comparison, decoded from a container literal per
/// the column's [`NestedCompare`]. Both sides of any one comparison come from
/// the same plan, so a variant mismatch is unreachable by construction.
///
/// `None` in an element or field position is SQL NULL, a value of the
/// container rather than the absence of one: `{1,NULL}` is a two-element
/// array. The whole field being NULL is a different fact — the `\N` the row
/// carries — and is SQL's `Unknown` as it is for a scalar column.
#[derive(Debug, Clone, PartialEq)]
pub enum NestedKey {
    /// A scalar position, through the leaf type's own [`OrderKey`].
    Leaf(OrderKey),
    /// An `array_out` value: its elements flattened row-major, plus the shape
    /// they were written in, because `array_cmp` decides on the shape once
    /// the elements agree (I45).
    Array { elements: Vec<Option<NestedKey>>, dims: Vec<usize>, lower_bounds: Vec<i32> },
    /// A `record_out` value: one entry per declared field, in declaration
    /// order.
    Record(Vec<Option<NestedKey>>),
    /// A range value, already through [`make_range`] — the form the server
    /// would have stored, not the form it was written in. Boxed because a
    /// bound is itself a `NestedKey`; the vector the multirange holds is
    /// indirection enough on its own.
    Range(Box<RangeKey>),
    /// A multirange value, already sorted, coalesced and emptied out
    /// ([`canonical_multirange`]), so its members are never empty and never
    /// touch.
    Multirange(Vec<RangeKey>),
}

/// A range value in the form PostgreSQL itself stores, which is the only form
/// two ranges may be compared in: `range_in` runs every literal through
/// `make_range`, so `int4range '[1,10]'` and `int4range '(0,11)'` are one
/// value the file can only hold as `[1,11)` (I46).
#[derive(Debug, Clone, PartialEq)]
pub struct RangeKey {
    /// The empty range, which `range_cmp` sorts below every other value and
    /// which is *not* the same as a range with two absent bounds — `empty`
    /// and `(,)` are different values (I46).
    empty: bool,
    lower: RangeBoundKey,
    upper: RangeBoundKey,
}

/// One bound of a range: PostgreSQL's `RangeBound`, minus the type it is a
/// bound of.
#[derive(Debug, Clone, PartialEq)]
struct RangeBoundKey {
    /// `None` is an **unbounded** bound — the server's `infinite` flag, and
    /// not a value at all. A bound holding `infinity` is a different thing and
    /// lives in the [`OrderKey`] beneath: `daterange '[2020-01-01,infinity]'`
    /// has a finite upper bound, which is why `daterange_canonical` leaves it
    /// alone (I34, I46).
    value: Option<NestedKey>,
    inclusive: bool,
    /// Which end this bound is. It decides the answer whenever two bounds
    /// hold the same value, so it travels with the bound rather than being
    /// inferred from the caller — `bounds_adjacent` relabels a pair before
    /// comparing it, as the server does.
    lower: bool,
}

impl RangeKey {
    /// The empty range. Its bounds are never read, and are the shape
    /// `range_serialize` leaves behind: absent and exclusive.
    fn empty() -> Self {
        RangeKey {
            empty: true,
            lower: RangeBoundKey { value: None, inclusive: false, lower: true },
            upper: RangeBoundKey { value: None, inclusive: false, lower: false },
        }
    }
}

/// Read one side of a nested comparison out of `text`.
///
/// `side` is which grammar to read it in, and it is the whole of what
/// separates the two sides: a *field* is read with [`crate::nested`]'s strict
/// `decode_*`, a *literal* with the `array_in`/`record_in` supersets
/// (`parse_*`), which take `{a, b}` and `{ 1 , 2 }` (I44).
///
/// The leaf grammar does not widen with it (`docs/design/decisions.md`,
/// "D58"): a leaf is read by its type's own reader, which takes no space its
/// `*_out` form does not write, so `--filter 'p=( 1 , a )'` is refused.
///
/// `None` is "not a value of this type", which is
/// `Error::PredicateValueDecode` for a literal and `Error::FieldDecode` for a
/// field.
pub fn nested_key(plan: &NestedCompare, text: &str, side: Side) -> Option<NestedKey> {
    let input = side == Side::Literal;
    Some(match plan {
        // A nested comparison is PostgreSQL's alone, DataFusion's semantics
        // refusing one.
        NestedCompare::Leaf { kind, .. } => NestedKey::Leaf(match side {
            Side::Literal => literal_key(kind, text, ComparisonSemantics::Postgres)?,
            Side::Field(invalid) => read_key(kind, text, invalid)?,
        }),
        // Refused before any value is read: `resolve_term` never builds a
        // comparison over a tree holding one.
        NestedCompare::Uncomparable { .. } => return None,
        NestedCompare::Array(element) => {
            let literal =
                if input { nested::parse_array(text) } else { nested::decode_array(text) }?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            for value in &literal.elements {
                elements.push(match value {
                    Some(value) => Some(nested_key(&element.plan, value, side)?),
                    None if position_refuses_null(element, side) => return None,
                    None => None,
                });
            }
            NestedKey::Array { elements, dims: literal.dims, lower_bounds: literal.lower_bounds }
        }
        // `int2vector`'s own grammar on both sides; the elements are built
        // here rather than through `order_key`, `int2vectorout` writing an
        // `int16` and nothing else.
        //
        // `dims` and `lower_bounds` are `[n]` and `[0]` for every value, the
        // empty vector included: `int2vectorin` sets `ndim = 1` and
        // `lbound1 = 0` unconditionally, where `array_out`'s `{}` is
        // zero-dimensional, so an empty `int2vector` is not the empty array
        // (I47).
        NestedCompare::Int2Vector => {
            let values = if input {
                nested::parse_int2vector(text)
            } else {
                nested::decode_int2vector(text)
            }?;
            NestedKey::Array {
                elements: values
                    .iter()
                    .map(|v| Some(NestedKey::Leaf(OrderKey::Int(i64::from(*v)))))
                    .collect(),
                dims: vec![values.len()],
                lower_bounds: vec![0],
            }
        }
        NestedCompare::Record(plans) => {
            let mut fields = if input {
                nested::parse_record(text, plans.len())?.fields
            } else {
                let mut fields = nested::decode_record(text)?.fields;
                // A zero-field composite is written `()`, and so is a
                // one-field composite holding NULL — the literal cannot tell
                // them apart, so the declared field list decides (I23).
                // `parse_record` has the arity in hand; `decode_record` does
                // not, so it is asked here.
                if plans.is_empty() && fields == [None] {
                    fields.clear();
                }
                fields
            };
            if fields.len() != plans.len() {
                return None;
            }
            let mut out = Vec::with_capacity(plans.len());
            for ((_, field), value) in plans.iter().zip(fields.drain(..)) {
                out.push(match value {
                    Some(value) => Some(nested_key(&field.plan, &value, side)?),
                    None if position_refuses_null(field, side) => return None,
                    None => None,
                });
            }
            NestedKey::Record(out)
        }
        NestedCompare::Range { bound, discrete } => {
            let literal =
                if input { nested::parse_range(text) } else { nested::decode_range(text) }?;
            NestedKey::Range(Box::new(range_key(bound, &literal, *discrete, side)?))
        }
        NestedCompare::Multirange { bound, discrete } => {
            let literals = if input {
                nested::parse_multirange(text)
            } else {
                nested::decode_multirange(text)
            }?;
            let mut members = Vec::with_capacity(literals.len());
            for literal in &literals {
                members.push(range_key(bound, literal, *discrete, side)?);
            }
            NestedKey::Multirange(canonical_multirange(members, *discrete)?)
        }
    })
}

/// Whether [`nested_key`] refuses a NULL at `position` read on `side`: a
/// literal's always and a field's but under
/// [`PostgresInvalidValues::Ignore`], where the position's type is a `NOT
/// NULL` domain, as `array_in` and `record_in` refuse it (I76).
fn position_refuses_null(position: &Position<NestedCompare>, side: Side) -> bool {
    position.not_null && side != Side::Field(PostgresInvalidValues::Ignore)
}

/// One range value, read through its bound's plan and then put into the form
/// the server stores it in.
///
/// Both sides go through [`make_range`], not only the literal
/// (`docs/design/decisions.md`, "D58"): it is idempotent on a `range_out`
/// field, the server having already applied it, so one code path serves both
/// grammars.
fn range_key(
    bound: &NestedCompare,
    literal: &nested::RangeLiteral,
    discrete: Option<Discrete>,
    reading: Side,
) -> Option<RangeKey> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        Some(RangeBoundKey {
            value: match text {
                Some(text) => Some(nested_key(bound, text, reading)?),
                None => None,
            },
            inclusive,
            lower,
        })
    };
    make_range(
        side(&literal.lower, literal.lower_inclusive, true)?,
        side(&literal.upper, literal.upper_inclusive, false)?,
        literal.empty,
        discrete,
    )
}

/// **Whether a marked check finds PostgreSQL's input function refuses `text`
/// as a field of a column compared by `plan`**: what a strict parse checks
/// every field by ([`PostgresInvalidValues::Strict`]). A scalar is [`field_key`]'s refusal;
/// a nested value is read as the server reads one on its way in, through each
/// container's input grammar, every element as a field of its own type and a
/// range's bounds in order ([`checked_key`]). `json`, held as its text, is
/// read by `json_in`'s grammar ([`decode::json_in`]), and a position holding
/// a type ordered by nothing by its [`TextGrammar`] where it has one; any
/// other plan naming no type this build reads by — another held as its text,
/// or a range declaring its own canonical function — refuses nothing, no
/// reader here decoding its fields. A column whose own type is ordered by
/// nothing is [`ComparisonPlan::Refused`] and is read by [`grammar_refuses`]
/// instead.
pub fn field_refused(plan: &ComparisonPlan, text: &str) -> bool {
    match plan {
        // `AS_TEXT`'s one member is `json`.
        ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. } => {
            !decode::json_in(text)
        }
        // Every text within its length is a value of these, and keying one
        // copies it.
        ComparisonPlan::Compared {
            kind: CompareKind::Text { length } | CompareKind::PaddedText { length },
            ..
        } => length.is_some_and(|length| decode::char_typmod_refuses(text, length)),
        ComparisonPlan::Compared { kind, .. } => {
            matches!(field_key(kind, text), Err(Unread::Refused))
        }
        ComparisonPlan::Nested(nested) => checked_key(nested, text).is_err(),
        ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_) => false,
    }
}

/// Whether PostgreSQL's input function refuses `text` as a field read by
/// `grammar`, of a type this build holds as its text and orders by nothing.
pub fn grammar_refuses(grammar: TextGrammar, text: &str) -> bool {
    let reads = match grammar {
        TextGrammar::Bit { varying, length } => decode::bit_in(text, varying, length),
        TextGrammar::Geometric(Geometric::Point) => decode::point_in(text),
        TextGrammar::Geometric(Geometric::Line) => decode::line_in(text),
        TextGrammar::Geometric(Geometric::Lseg) => decode::lseg_in(text),
        TextGrammar::Geometric(Geometric::Box) => decode::box_in(text),
        TextGrammar::Geometric(Geometric::Path) => decode::path_in(text),
        TextGrammar::Geometric(Geometric::Polygon) => decode::poly_in(text),
        TextGrammar::Geometric(Geometric::Circle) => decode::circle_in(text),
        TextGrammar::BoxArray { not_null } => {
            let element = NestedCompare::Uncomparable {
                declared: "box".into(),
                divergence: None,
                grammar: Some(TextGrammar::Geometric(Geometric::Box)),
                delimiter: Some(b';'),
            };
            let element = Position { plan: element, not_null };
            checked_key(&NestedCompare::Array(Box::new(element)), text).is_ok()
        }
        TextGrammar::RefusesAll => false,
        TextGrammar::RefusesNothing => true,
    };
    !reads
}

/// The `typdelim` an array of `element` is split at, which the plan carries
/// off the preamble (I22) — `None` where the split is not this build's to
/// make: a delimiter the preamble could not read, and one `array_in` reads
/// otherwise than [`nested::parse_array_delimited`] models at some supported
/// major, which is any byte but a visible ASCII one, the space not among them,
/// other than a brace, a double quote or a backslash.
fn array_delimiter(element: &NestedCompare) -> Option<u8> {
    let delimiter = match element {
        NestedCompare::Uncomparable { delimiter, .. } => (*delimiter)?,
        _ => b',',
    };
    (delimiter.is_ascii_graphic() && !b"{}\"\\".contains(&delimiter)).then_some(delimiter)
}

/// A nested field's key, built as [`nested_key`] builds a literal's — each
/// container read by its input grammar, the one `COPY` reads a field by — with
/// every element read as a field: `Err` where a marked check on the way
/// refuses it, and `Ok(None)` where it is read but some part of it has no key
/// here, a position this build does not order or a spelling it does not read.
/// **Every element is read whatever an earlier one answered**, so a refusal
/// anywhere in the value is found.
fn checked_key(plan: &NestedCompare, text: &str) -> Read<Option<NestedKey>> {
    let refused = Unread::Refused;
    Ok(match plan {
        NestedCompare::Leaf { kind, .. } => match field_key(kind, text) {
            Ok(key) => Some(NestedKey::Leaf(key)),
            Err(Unread::Refused) => return Err(refused),
            Err(Unread::Unparsed) => None,
        },
        // `json`, the one position diverging `AsText` (`NestedCompare::Uncomparable`).
        NestedCompare::Uncomparable { divergence: Some(ComparisonDivergence::AsText), .. }
            if !decode::json_in(text) =>
        {
            return Err(refused);
        }
        NestedCompare::Uncomparable { grammar: Some(grammar), .. }
            if grammar_refuses(*grammar, text) =>
        {
            return Err(refused);
        }
        NestedCompare::Uncomparable { .. } => None,
        NestedCompare::Array(element) => {
            let Some(delimiter) = array_delimiter(&element.plan) else { return Ok(None) };
            let literal = nested::parse_array_delimited(text, delimiter).ok_or(refused)?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            let mut keyed = true;
            for value in &literal.elements {
                match value {
                    Some(value) => match checked_key(&element.plan, value)? {
                        Some(key) => elements.push(Some(key)),
                        None => keyed = false,
                    },
                    None if element.not_null => return Err(refused),
                    None => elements.push(None),
                }
            }
            keyed.then_some(NestedKey::Array {
                elements,
                dims: literal.dims,
                lower_bounds: literal.lower_bounds,
            })
        }
        NestedCompare::Int2Vector => {
            let values = nested::parse_int2vector(text).ok_or(refused)?;
            Some(NestedKey::Array {
                dims: vec![values.len()],
                elements: values
                    .into_iter()
                    .map(|v| Some(NestedKey::Leaf(OrderKey::Int(i64::from(v)))))
                    .collect(),
                lower_bounds: vec![0],
            })
        }
        NestedCompare::Record(plans) => {
            let fields = nested::parse_record(text, plans.len()).ok_or(refused)?.fields;
            let mut out = Vec::with_capacity(plans.len());
            let mut keyed = true;
            for ((_, field), value) in plans.iter().zip(fields) {
                match value {
                    Some(value) => match checked_key(&field.plan, &value)? {
                        Some(key) => out.push(Some(key)),
                        None => keyed = false,
                    },
                    None if field.not_null => return Err(refused),
                    None => out.push(None),
                }
            }
            keyed.then_some(NestedKey::Record(out))
        }
        NestedCompare::Range { bound, discrete } => {
            let literal = nested::parse_range(text).ok_or(refused)?;
            checked_range(bound, &literal, *discrete)?
                .map(|range| NestedKey::Range(Box::new(range)))
        }
        NestedCompare::Multirange { bound, discrete } => {
            let literals = nested::parse_multirange(text).ok_or(refused)?;
            let mut members = Vec::with_capacity(literals.len());
            let mut keyed = true;
            for literal in &literals {
                match checked_range(bound, literal, *discrete)? {
                    Some(member) => members.push(member),
                    None => keyed = false,
                }
            }
            // `multirange_in` refuses nothing past its members' `range_in`.
            keyed
                .then(|| canonical_multirange(members, *discrete))
                .flatten()
                .map(NestedKey::Multirange)
        }
    })
}

/// [`range_key`] for [`checked_key`]: both bounds read, then [`make_range`]'s
/// refusals where both have a key.
fn checked_range(
    bound: &NestedCompare,
    literal: &nested::RangeLiteral,
    discrete: Option<Discrete>,
) -> Read<Option<RangeKey>> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        let value = match text {
            Some(text) => match checked_key(bound, text)? {
                Some(key) => Some(key),
                None => return Ok(None),
            },
            None => None,
        };
        Ok(Some(RangeBoundKey { value, inclusive, lower }))
    };
    let lower = side(&literal.lower, literal.lower_inclusive, true)?;
    let upper = side(&literal.upper, literal.upper_inclusive, false)?;
    let (Some(lower), Some(upper)) = (lower, upper) else { return Ok(None) };
    make_range(lower, upper, literal.empty, discrete).map(Some).ok_or(Unread::Refused)
}

/// `make_range`: `range_serialize`'s type-independent checks, then the range
/// type's canonical function where it has one, then those checks again — the
/// order the server applies them in, and the reason `int4range '(1,2)'` is
/// `empty` (I46).
///
/// `None` is the server's `22000` — a lower bound above its upper, a *semantic*
/// refusal the container grammar cannot see (I44), so it is raised here where
/// the bounds have been decoded — or a discrete bound whose successor leaves
/// the subtype, past [`Discrete::largest`], the canonical function's
/// "out of range".
fn make_range(
    lower: RangeBoundKey,
    upper: RangeBoundKey,
    empty: bool,
    discrete: Option<Discrete>,
) -> Option<RangeKey> {
    let serialized = serialize_range(lower, upper, empty)?;
    let Some(discrete) = discrete.filter(|_| !serialized.empty) else {
        return Some(serialized);
    };
    // `int4range_canonical` and its two siblings, which differ only in the
    // value past which they refuse a successor: an exclusive lower bound
    // becomes inclusive at the successor, an inclusive upper becomes
    // exclusive at the successor.
    //
    // The `Int` pattern is `daterange_canonical`'s `DATE_NOT_FINITE` guard: a
    // date `infinity` decodes to `OrderKey::PositiveInfinity` rather than to
    // a day count, so it matches no arm here and is left as written, which is
    // what makes `[2020-01-01,infinity]` keep its inclusive upper (I34, I46).
    // pg-refuses: I46 — a successor past the subtype's largest value.
    let successor = |bound: &RangeBoundKey| match &bound.value {
        Some(NestedKey::Leaf(OrderKey::Int(n))) => (*n < discrete.largest()).then(|| {
            Some(RangeBoundKey {
                value: Some(NestedKey::Leaf(OrderKey::Int(n + 1))),
                inclusive: !bound.inclusive,
                lower: bound.lower,
            })
        }),
        // Not a finite integer bound, so the canonical function skips it.
        _ => Some(None),
    };
    let mut lower = serialized.lower;
    let mut upper = serialized.upper;
    if !lower.inclusive
        && let Some(shifted) = successor(&lower)?
    {
        lower = shifted;
    }
    if upper.inclusive
        && let Some(shifted) = successor(&upper)?
    {
        upper = shifted;
    }
    serialize_range(lower, upper, false)
}

/// `range_serialize`'s type-independent half: the out-of-order refusal, the
/// collapse to `empty`, and "an infinite boundary is never inclusive".
fn serialize_range(
    mut lower: RangeBoundKey,
    mut upper: RangeBoundKey,
    empty: bool,
) -> Option<RangeKey> {
    if empty {
        return Some(RangeKey::empty());
    }
    match compare_bound_values(&lower, &upper) {
        // pg-refuses: I46 — `22000`, a lower bound above its upper.
        Ordering::Greater => return None,
        // Equal bounds are a value only when both ends include it: `[1,1]` is
        // one point and `[1,1)`, `(1,1]` and `(1,1)` are all `empty`. This
        // runs before canonicalization *and* after it, which is what makes
        // `int4range '(1,2)'` empty.
        Ordering::Equal if !(lower.inclusive && upper.inclusive) => {
            return Some(RangeKey::empty());
        }
        _ => {}
    }
    if lower.value.is_none() {
        lower.inclusive = false;
    }
    if upper.value.is_none() {
        upper.inclusive = false;
    }
    Some(RangeKey { empty: false, lower, upper })
}

/// `range_cmp_bound_values`: the bounds' held values alone, with an absent
/// bound settled by which end it is. Inclusivity is not consulted, which is
/// what separates this from [`compare_bounds`] — the emptiness and adjacency
/// tests both need the values without it.
fn compare_bound_values(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    match (&a.value, &b.value) {
        (None, None) if a.lower == b.lower => Ordering::Equal,
        (None, None) | (None, Some(_)) => {
            if a.lower {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (Some(_), None) => {
            if b.lower {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `range_cmp_bounds`: infinity, then the held value, then inclusivity — and
/// inclusivity is where which *end* a bound is starts to matter. An exclusive
/// **lower** bound is above an inclusive one at the same value ("just
/// after"), an exclusive **upper** below it ("just before") (I46).
fn compare_bounds(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    let by_value = compare_bound_values(a, b);
    if a.value.is_none() || b.value.is_none() || by_value.is_ne() {
        return by_value;
    }
    match (a.inclusive, b.inclusive) {
        (true, true) => Ordering::Equal,
        (false, false) if a.lower == b.lower => Ordering::Equal,
        (false, _) if a.lower => Ordering::Greater,
        (false, _) => Ordering::Less,
        (true, false) if b.lower => Ordering::Less,
        (true, false) => Ordering::Greater,
    }
}

/// `range_cmp`: `empty` below everything, then lower bound, then upper.
fn compare_range(a: &RangeKey, b: &RangeKey) -> Ordering {
    match (a.empty, b.empty) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => {
            compare_bounds(&a.lower, &b.lower).then_with(|| compare_bounds(&a.upper, &b.upper))
        }
    }
}

/// `multirange_canonicalize`: sort the members, drop the empty ones, and
/// merge any two that overlap or touch. After it no member is empty and no
/// two members meet, so the sequence is the value and the comparison is a
/// plain sequence walk (I46).
///
/// `None` propagates a bound the union could not re-serialize, which the
/// shapes reaching here cannot produce; it is carried rather than unwrapped.
fn canonical_multirange(
    mut members: Vec<RangeKey>,
    discrete: Option<Discrete>,
) -> Option<Vec<RangeKey>> {
    members.sort_by(compare_range);
    let mut out: Vec<RangeKey> = Vec::with_capacity(members.len());
    for current in members {
        if current.empty {
            continue;
        }
        let Some(last) = out.last() else {
            out.push(current);
            continue;
        };
        // The server's own order. The first test needs the sort:
        // `range_adjacent_internal` answers true for "either meets the
        // other", and only sorting rules out the second direction.
        if ranges_adjacent(last, &current, discrete) {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        } else if range_before(last, &current) {
            out.push(current);
        } else {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        }
    }
    Some(out)
}

/// `range_before_internal`: every point of `a` is below every point of `b`,
/// with an empty range neither before nor after anything.
fn range_before(a: &RangeKey, b: &RangeKey) -> bool {
    !a.empty && !b.empty && compare_bounds(&a.upper, &b.lower).is_lt()
}

/// `range_adjacent_internal`: the two ranges touch without overlapping,
/// in either direction.
fn ranges_adjacent(a: &RangeKey, b: &RangeKey, discrete: Option<Discrete>) -> bool {
    !a.empty
        && !b.empty
        && (bounds_adjacent(&a.upper, &b.lower, discrete)
            || bounds_adjacent(&b.upper, &a.lower, discrete))
}

/// `bounds_adjacent`: whether an upper bound and a lower bound meet with no
/// point between them.
///
/// Equal values are adjacent exactly when one end includes the point and the
/// other does not. Values that differ are adjacent only in a discrete range,
/// which the server decides by building the range *between* them with both
/// inclusivities flipped and asking whether it came out empty.
fn bounds_adjacent(
    upper: &RangeBoundKey,
    lower: &RangeBoundKey,
    discrete: Option<Discrete>,
) -> bool {
    match compare_bound_values(upper, lower) {
        Ordering::Equal => upper.inclusive != lower.inclusive,
        Ordering::Greater => false,
        Ordering::Less => {
            discrete.is_some()
                && make_range(
                    RangeBoundKey {
                        value: upper.value.clone(),
                        inclusive: !upper.inclusive,
                        lower: true,
                    },
                    RangeBoundKey {
                        value: lower.value.clone(),
                        inclusive: !lower.inclusive,
                        lower: false,
                    },
                    false,
                    discrete,
                )
                .is_some_and(|between| between.empty)
        }
    }
}

/// `range_union_internal` for two ranges already known to overlap or touch:
/// the lower of the two lower bounds, the upper of the two uppers, back
/// through [`make_range`].
fn range_union(a: &RangeKey, b: &RangeKey, discrete: Option<Discrete>) -> Option<RangeKey> {
    let lower =
        if compare_bounds(&a.lower, &b.lower).is_lt() { a.lower.clone() } else { b.lower.clone() };
    let upper =
        if compare_bounds(&a.upper, &b.upper).is_gt() { a.upper.clone() } else { b.upper.clone() };
    make_range(lower, upper, false, discrete)
}

/// One NULL rule, at every level: two NULLs are equal, and NULL sorts above
/// not-NULL (I45). It covers equality and ordering alike, which is what makes
/// a nested comparison two-valued throughout, never SQL's `Unknown`.
fn compare_slot(a: Option<&NestedKey>, b: Option<&NestedKey>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `array_cmp`/`record_cmp`, structurally.
///
/// An array compares its elements first and its shape only afterwards (I45):
/// up to the shorter array's length, then element count, then dimension
/// count, then the dimensions, then the lower bounds — not the order
/// `array_eq` uses. So `{1,2}` is above `[0:1]={1,2}` and below
/// `{{1,2},{3,4}}`, whose extra elements are never reached.
///
/// A record's two sides always have the same arity: [`nested_key`] checks it
/// against the composite's own declared field list before building either.
pub fn compare_nested(a: &NestedKey, b: &NestedKey) -> Ordering {
    match (a, b) {
        (NestedKey::Leaf(a), NestedKey::Leaf(b)) => compare_keys(a, b),
        (
            NestedKey::Array { elements: ae, dims: ad, lower_bounds: al },
            NestedKey::Array { elements: be, dims: bd, lower_bounds: bl },
        ) => first_difference(
            ae.iter()
                .zip(be.iter())
                .map(|(a, b)| compare_slot(a.as_ref(), b.as_ref()))
                .chain(std::iter::once(ae.len().cmp(&be.len())))
                .chain(std::iter::once(ad.len().cmp(&bd.len())))
                .chain(ad.iter().zip(bd.iter()).map(|(a, b)| a.cmp(b)))
                .chain(al.iter().zip(bl.iter()).map(|(a, b)| a.cmp(b))),
        ),
        (NestedKey::Record(a), NestedKey::Record(b)) => first_difference(
            a.iter().zip(b.iter()).map(|(a, b)| compare_slot(a.as_ref(), b.as_ref())),
        ),
        (NestedKey::Range(a), NestedKey::Range(b)) => compare_range(a, b),
        // `multirange_cmp`: member-wise, and the shorter one first where the
        // members it has all agree — the server treating a missing member as
        // an empty range and `empty` as the lowest value there is.
        (NestedKey::Multirange(a), NestedKey::Multirange(b)) => first_difference(
            a.iter()
                .zip(b.iter())
                .map(|(a, b)| compare_range(a, b))
                .chain(std::iter::once(a.len().cmp(&b.len()))),
        ),
        _ => unreachable!("both sides of a comparison are read through one column's plan"),
    }
}

/// A `macaddr`/`macaddr8` literal in `macaddr_out`'s own spelling: `octets`
/// lowercase hex pairs joined by colons (I40). Those two rules are the whole
/// of that output function, which is why this type canonicalizes where the
/// two beside it decode per row — see `pgdt_query::predicate::equality_comparison`.
pub fn render_macaddr(text: &str, octets: usize) -> Option<String> {
    let OrderKey::Bytes(bytes) = macaddr_key(text, octets)? else {
        unreachable!("`macaddr_key` yields its octets")
    };
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"))
}

/// The form a **nested** literal has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence: the container's own grammar, then
/// how to spell what is inside it — as the dump does, which is each leaf
/// type's own output form.
///
/// That second half is advice rather than the boundary of what is accepted:
/// [`order_key`]'s integer arms are `str::parse`, so they take a leading `+`
/// and leading zeros no `*_out` writes, one of the exceptions [`accepted_form`]
/// lists (`docs/design/decisions.md`, "D55"). There is no per-leaf clause list, [`nested_key`]
/// answering only "not a value of this type".
pub fn nested_accepted_form(plan: &NestedCompare) -> String {
    // The one form with no leaf clause to add, because it has no leaf: an
    // `int2vector`'s elements are read by `int2vectorin` itself, so the
    // superset reaches all the way down and the sentence below would be false
    // here.
    if matches!(plan, NestedCompare::Int2Vector) {
        return "as whole numbers from -32768 to 32767 separated by spaces, and as nothing at \
                all for the empty vector"
            .to_string();
    }
    let container = match plan {
        NestedCompare::Array(element) if element.not_null => {
            "as an array literal — `{a,b}`, `{}` — with no `NULL` element, which its element \
             type, a `NOT NULL` domain, refuses"
        }
        NestedCompare::Array(_) => "as an array literal — `{a,b}`, `{}`, a bare `NULL` element",
        NestedCompare::Record(fields) if fields.iter().any(|(_, field)| field.not_null) => {
            "as a composite literal — `(a,b)`, a field left empty for NULL but where its type is \
             a `NOT NULL` domain"
        }
        NestedCompare::Record(_) => "as a composite literal — `(a,b)`, a field left empty for NULL",
        NestedCompare::Range { .. } => {
            "as a range literal — `[a,b)`, `empty`, a bound left empty for unbounded — whose \
             lower bound is not above its upper"
        }
        NestedCompare::Multirange { .. } => {
            "as a multirange literal — `{[a,b),[c,d)}`, `{}` — each member a range whose lower \
             bound is not above its upper"
        }
        // Answered above, before the leaf clause this arm cannot carry.
        NestedCompare::Int2Vector => "as an int2vector literal",
        // Neither is reachable: a leaf plan is never a column's whole
        // comparison, and an uncomparable one refuses before a literal is
        // read.
        NestedCompare::Leaf { .. } | NestedCompare::Uncomparable { .. } => "as a nested literal",
    };
    format!(
        "{container} — with each element, field or bound spelled as the dump spells it, in that \
         type's own output form"
    )
}

/// The form a literal of `kind` has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence — read after "which is written".
///
/// It lives beside the grammar rather than beside [`CompareKind`] because it
/// describes what [`literal_key`] and `pgdt_query::predicate::equality_comparison` accept, which is
/// each type's `*_out` form, widened by an integer's sign and leading zeros,
/// a float's sign, bare point, exponent case and `inf` or `nan` in any case,
/// a `uuid` or `macaddr` hex digit's case, a `uuid`'s omitted hyphens,
/// hyphens after any group of four and enclosing braces,
/// either `numeric` kind's leading or trailing point and leading zeros, a
/// network value's full-width or zero-padded netmask, an IPv6 address's
/// uncompressed, zero-padded or upper-case groups, a `cidr` written with no
/// netmask, and a time's trailing fractional zeros (`docs/design/decisions.md`, "D55").
/// `jsonb` needs the least here, its grammar being the whole of `jsonb_in`.
///
/// Two arms answer with the kind's own payload, because there the payload
/// *is* the answer: an enum's declared labels, and a `numeric(p,s)`'s scale
/// — see [`ENUM_LABELS_SHOWN`] and [`decimal_accepted_form`].
///
/// [`CompareKind::Text`] and [`CompareKind::PaddedText`] never refuse a
/// literal, and an enum with no labels is refused by `pgtype::comparison_for`
/// outright, so both arms are unreachable; they are written out rather than
/// `unreachable!` so a future kind's mistake is not a panic on a diagnostic
/// path.
pub fn accepted_form(kind: &CompareKind) -> String {
    use CompareKind as K;
    match kind {
        K::Bool => "`t` or `f`".into(),
        // The width is half the sentence, as for `oid`: past it is the
        // refusal a well-formed literal meets (I60).
        K::Int { bytes } => {
            let range = int_range(*bytes);
            format!("as a whole number from {} to {}", range.start(), range.end())
        }
        // The width *is* the refusal: `oidin` wraps a negative and this does
        // not, so the range is the useful half of the sentence.
        K::UnsignedInt => "as a whole number from 0 to 4294967295".into(),
        K::Float32 => {
            "as a number within `real`'s range, or `Infinity`, `-Infinity` or `NaN`".into()
        }
        K::Float64 => {
            "as a number within `double precision`'s range, or `Infinity`, `-Infinity` or `NaN`"
                .into()
        }
        // A typmod rejects an infinity (I34), so a `numeric(p,s)` — and a
        // `numeric` past 76 digits — has `NaN` and nothing else. Only the
        // typed arm carries a scale to be finer than: a `p > 76` column is
        // held as text and compared by value through `NumericKey`, which
        // normalizes rather than rescaling.
        // The digit bounds are `numeric_in`'s storage format's (I63), which
        // a typed arm's scale and `i256` already fall inside.
        K::Decimal { scale, .. } => decimal_accepted_form(*scale),
        K::Numeric { infinities: false, .. } => format!("{NUMERIC_DIGITS}, or `NaN`"),
        K::Numeric { infinities: true, .. } => {
            format!("{NUMERIC_DIGITS}, or `NaN`, `Infinity` or `-Infinity`")
        }
        K::Enum { labels, .. } if !labels.is_empty() => enum_accepted_form(labels),
        K::Enum { .. } => "as one of the type's own declared labels".into(),
        // The range is half the sentence, as for an integer: a day the
        // calendar lacks, or one past it, is the refusal a well-formed
        // literal meets (I61).
        K::Date => "`YYYY-MM-DD`, optionally suffixed ` BC`, a day from `4714-11-24 BC` to \
                    `5874897-12-31`, or `infinity`/`-infinity`"
            .into(),
        K::Time => {
            "`HH:MM:SS`, optionally with a fractional second, no later than `24:00:00`".into()
        }
        K::Timestamp { with_tz: false } => {
            "`YYYY-MM-DD HH:MM:SS`, optionally with a fractional second and suffixed ` BC`, from \
             `4714-11-24 00:00:00 BC` to `294276-12-31 23:59:59.999999`, or \
             `infinity`/`-infinity`"
                .into()
        }
        K::Timestamp { with_tz: true } => {
            "`YYYY-MM-DD HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, \
             optionally with a fractional second and suffixed ` BC`, from `4714-11-24 00:00:00+00 \
             BC` to `294276-12-31 23:59:59.999999+00`, or `infinity`/`-infinity`"
                .into()
        }
        K::TimeTz => {
            "`HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, optionally \
             with a fractional second, no later than `24:00:00`"
                .into()
        }
        // `interval_out` under `IntervalStyle = postgres` (I4), which is the
        // only style a `pg_dump` connection writes; the bounds are I62's.
        K::Interval { .. } | K::IntervalFields { .. } => {
            "the way `interval` prints it — `1 year 2 mons 3 days`, `-01:00:00`, `00:00:00` for \
             zero — each unit once, its minutes under `60` and seconds at most `60`, or \
             `infinity`/`-infinity`"
                .into()
        }
        // `network_in`'s grammar (I67), which reads more than this says.
        K::Network { cidr: false } => {
            "as an IPv4 or IPv6 address, optionally followed by `/bits`, an IPv4 one omitting no \
             octet but those past the netmask's whole bytes"
                .into()
        }
        K::Network { cidr: true } => {
            "as an IPv4 or IPv6 network, optionally followed by `/bits`, with no bit set below \
             the netmask, an IPv4 one omitting any trailing octets, which are zero"
                .into()
        }
        K::MacAddr { octets: 8 } => "as eight colon-separated hex pairs".into(),
        K::MacAddr { .. } => "as six colon-separated hex pairs".into(),
        K::Uuid => {
            "as 32 hex digits, grouped `8-4-4-4-12`, a hyphen allowed only after a group of four \
             but the last, the whole optionally in braces"
                .into()
        }
        K::Bytea => {
            "as `\\x` followed by hex pairs, or as `bytea_output = escape` writes it".into()
        }
        K::Jsonb => "as a JSON document".into(),
        K::Text { .. } | K::PaddedText { .. } => "as any text".into(),
    }
}

/// How many of an enum's labels [`accepted_form`] names before it stops
/// counting them out.
///
/// A count cap rather than a length cap (`map.rs`'s `SPAN_STORED_TEXT_MAX_BYTES` is the other
/// shape): every label a message prints is printed whole, where a length cap
/// would hand the user a spelling that is not a label. Nothing bounds how
/// many labels a type declares.
///
/// The overflow clause has somewhere to send the reader: `pgdt info --detail`
/// prints every label of an enum column *and* lists every user-defined type
/// with its labels, both uncapped (`docs/design/decisions.md`, "The CLI").
const ENUM_LABELS_SHOWN: usize = 12;

/// The enum clause: the declared labels themselves, which are the whole of
/// what a refused enum literal is missing.
///
/// Each label is single-quoted with any interior quote doubled — the spelling
/// the dump's own `CREATE TYPE … AS ENUM (…)` writes and the CLI's `dequote`
/// accepts — so a printed label pastes straight back into
/// `--filter "col=<label>"`. The CLI's own `label_list` renders the same way
/// and is not shared with it, sitting a layer above.
fn enum_accepted_form(labels: &[String]) -> String {
    let shown = labels.len().min(ENUM_LABELS_SHOWN);
    let list = labels[..shown]
        .iter()
        .map(|label| format!("'{}'", label.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    match labels.len() - shown {
        0 => format!("as one of the type's declared labels: {list}"),
        more => format!(
            "as one of the type's declared labels: {list}, and {more} more; see `info --detail`"
        ),
    }
}

/// A scale-free `numeric` literal's shape: `numeric_in`'s storage bounds
/// (I63), stated as the digits either side of the point.
const NUMERIC_DIGITS: &str =
    "as a number of at most 131072 digits before the point and 16383 after it";

/// The `numeric(p,s)` clause, which is about the **scale** because that is
/// what the refusal is about: `decode::decimal_unscaled_digits` drops a
/// trailing digit only when it is zero, so `--filter 'price>1.005'` on a
/// `numeric(10,2)` is refused.
///
/// Precision says nothing here, binding a field alone: a literal wider than
/// the column can hold still compares against every value in it, as the
/// server coerces it with no typmod (I63). A negative scale is legal from
/// PostgreSQL 15 and means the column stores multiples of a power of ten,
/// which the literal's reader enforces by refusing to drop a non-zero digit
/// off the integer part.
fn decimal_accepted_form(scale: i8) -> String {
    match scale {
        0 => "as a whole number, or `NaN`".into(),
        s if s > 0 => {
            let plural = if s == 1 { "" } else { "s" };
            format!("as a number with at most {s} decimal place{plural}, or `NaN`")
        }
        s => {
            let step = format!("1{}", "0".repeat(usize::from(s.unsigned_abs())));
            format!("as a whole number that is a multiple of {step}, or `NaN`")
        }
    }
}

#[cfg(test)]
mod tests;
