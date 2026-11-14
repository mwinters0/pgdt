//! **A `jsonb` field ordered against a document as its positions hold it**:
//! the field walked whole first, so it is refused where [`parse_jsonb`]
//! refuses it, and then compared as `compareJsonbContainers` compares two
//! (I41) — each object's pairs held as where its keys are written, sorted
//! into storage order and rid of duplicates as `uniqueifyJsonbObject` leaves
//! them, a pair's value walked from there — so its order holds a position a
//! pair, on the path the comparison is on, and no tree of the field
//! (`docs/design/decisions.md`, "D152").
//!
//! [`parse_jsonb`]: super::parse_jsonb

use std::cmp::Ordering;
use std::mem::size_of;

use super::{JsonCursor, JsonWalk, Jsonb};
use crate::decode::{Read, Unread};
use crate::value::factor::KeyFactor;
use crate::value::nested::FieldText;

/// What a `jsonb` document's order holds per byte of its text, at most: a
/// position a pair, the shortest pair and its comma being `"":0,`.
///
/// Positions alone: a number compared is keyed whole from its text, as a key
/// reads a scalar (D152), so it holds what numeric's precision bounds whatever
/// the line's length, and on a dump's field no digit its text lacks,
/// `numeric_out` writing no exponent to pad (I41). A reader holds that as
/// it holds a short value's decode, and no factor carries it.
pub(crate) const JSONB_KEY_FACTOR: KeyFactor =
    KeyFactor { held: size_of::<u32>() as u64, per: "\"\":0,".len() as u64 };

/// How the `jsonb` value `text` holds orders against `bound`, a document
/// [`jsonb_key`](crate::key::order) built, a top-level scalar in its
/// pseudo-array: what [`Jsonb::cmp`] answers of the field's own document,
/// or why there is none, as [`parse_jsonb`](super::parse_jsonb) says;
/// `Equal` where there is no bound, the field read all the same.
pub(crate) fn jsonb_field_order(mut text: FieldText<'_>, bound: Option<&Jsonb>) -> Read<Ordering> {
    let start = text.clone();
    let mut walk = JsonWalk::new(true);
    while let Some(byte) = text.peek() {
        text.bump();
        walk.byte(byte).map_err(|_| Unread::Refused)?;
    }
    walk.end()?;
    let Some(bound) = bound else { return Ok(Ordering::Equal) };
    let mut text = start;
    blanks(&mut text);
    let document = Document { field: text.clone() };
    match text.peek() {
        Some(b'[' | b'{') => document.value(text.offset(), bound),
        // A scalar is stored in a pseudo-array of one (I41).
        _ => Ok(match bound {
            Jsonb::Array { raw_scalar, items } => {
                let mut container = if *raw_scalar { Ordering::Equal } else { Ordering::Less };
                if items.len() != 1 {
                    container = 1.cmp(&items.len());
                }
                match container {
                    Ordering::Equal => document.value(text.offset(), &items[0])?,
                    _ => container,
                }
            }
            _ => Ordering::Less,
        }),
    }
}

/// `json_lex`'s blanks, read past.
fn blanks(text: &mut FieldText<'_>) {
    while matches!(text.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        text.bump();
    }
}

/// A position as the order holds it: within a line, which I71 bounds below
/// `u32::MAX`.
fn position(text: &FieldText<'_>) -> Read<u32> {
    u32::try_from(text.offset()).map_err(|_| Unread::Unparsed)
}

/// A document walked whole and read, read again from any value's position.
struct Document<'a> {
    field: FieldText<'a>,
}

impl<'a> Document<'a> {
    fn at(&self, offset: usize) -> FieldText<'a> {
        let mut text = self.field.clone();
        text.seek(offset);
        text
    }

    /// The value at `offset` against `bound`, as [`Jsonb::cmp`] orders two.
    fn value(&self, offset: usize, bound: &Jsonb) -> Read<Ordering> {
        let mut text = self.at(offset);
        let first = text.peek().ok_or(Unread::Refused)?;
        let rank = match first {
            b'n' => 0x0,
            b'"' => 0x1,
            b't' | b'f' => 0x3,
            b'[' => 0x10,
            b'{' => 0x11,
            _ => 0x2,
        };
        if rank != bound.rank() {
            return Ok(rank.cmp(&bound.rank()));
        }
        Ok(match bound {
            Jsonb::Null => Ordering::Equal,
            Jsonb::Bool(bound) => (first == b't').cmp(bound),
            Jsonb::Number(bound) => {
                let mut token = Vec::new();
                while let Some(byte) = text.peek().filter(|byte| {
                    !matches!(byte, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r')
                }) {
                    token.push(byte);
                    text.bump();
                }
                let mut cursor = JsonCursor { bytes: &token, at: 0, depth: 0, refused: false };
                cursor.number().ok_or(Unread::Refused)?.cmp(bound)
            }
            Jsonb::String(bound) => {
                text.bump();
                compare_string(&mut text, bound.as_bytes())
            }
            Jsonb::Array { raw_scalar, items } => {
                text.bump();
                let elements = elements(text);
                // The field's array is a real one, the bound's perhaps the
                // pseudo-array a scalar is stored in.
                let mut container = if *raw_scalar { Ordering::Greater } else { Ordering::Equal };
                if elements.len() != items.len() {
                    container = elements.len().cmp(&items.len());
                }
                if container.is_ne() {
                    return Ok(container);
                }
                for (offset, item) in elements.zip(items) {
                    let order = self.value(offset, item)?;
                    if order.is_ne() {
                        return Ok(order);
                    }
                }
                Ordering::Equal
            }
            Jsonb::Object(pairs) => {
                text.bump();
                self.object(text, pairs)?
            }
        })
    }

    /// An object's pairs, its `{` read, against `bound`'s: their count, then
    /// each key and value in storage order.
    fn object(&self, mut text: FieldText<'a>, bound: &[(String, Jsonb)]) -> Read<Ordering> {
        let start = text.clone();
        let mut count = 0usize;
        pairs(&mut text, |_| {
            count += 1;
            Ok(())
        })?;
        let mut keys: Vec<u32> = Vec::with_capacity(count);
        let mut text = start;
        pairs(&mut text, |text| {
            keys.push(position(text)?);
            Ok(())
        })?;
        // Storage order, the last written of two equal keys first and kept
        // (`storage_order`).
        keys.sort_unstable_by(|&a, &b| self.storage(a, b).then(b.cmp(&a)));
        keys.dedup_by(|a, b| self.key_order(*a, *b).is_eq());
        let order = keys.len().cmp(&bound.len());
        if order.is_ne() {
            return Ok(order);
        }
        for (&key, (bound_key, bound_value)) in keys.iter().zip(bound) {
            let mut text = self.at(key as usize);
            text.bump();
            let order = compare_string(&mut text, bound_key.as_bytes());
            if order.is_ne() {
                return Ok(order);
            }
            let mut text = self.at(key as usize);
            skip(&mut text);
            blanks(&mut text);
            text.eat(b':');
            blanks(&mut text);
            let order = self.value(text.offset(), bound_value)?;
            if order.is_ne() {
                return Ok(order);
            }
        }
        Ok(Ordering::Equal)
    }

    /// The key at `a` against the one at `b`, bytewise.
    fn key_order(&self, a: u32, b: u32) -> Ordering {
        let (mut a, mut b) =
            (StringBytes::at(self.at(a as usize)), StringBytes::at(self.at(b as usize)));
        loop {
            match (a.next(), b.next()) {
                (None, None) => return Ordering::Equal,
                (x, y) if x != y => return x.cmp(&y),
                _ => {}
            }
        }
    }

    /// `lengthCompareJsonbString`: the key at `a` against the one at `b`,
    /// by length and then bytewise.
    fn storage(&self, a: u32, b: u32) -> Ordering {
        let len = |at: u32| StringBytes::at(self.at(at as usize)).count();
        len(a).cmp(&len(b)).then_with(|| self.key_order(a, b))
    }
}

/// An array's elements, its `[` read: where each starts, counted
/// before any is read.
fn elements(mut text: FieldText<'_>) -> Elements<'_> {
    let start = text.clone();
    let mut len = 0usize;
    blanks(&mut text);
    if !text.eat(b']') {
        loop {
            skip(&mut text);
            len += 1;
            blanks(&mut text);
            if !text.eat(b',') {
                break;
            }
            blanks(&mut text);
        }
    }
    Elements { text: start, len, read: 0 }
}

/// Where each of an array's elements starts, in order.
struct Elements<'a> {
    text: FieldText<'a>,
    len: usize,
    read: usize,
}

impl Elements<'_> {
    fn len(&self) -> usize {
        self.len
    }
}

impl Iterator for Elements<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.read == self.len {
            return None;
        }
        if self.read > 0 {
            blanks(&mut self.text);
            self.text.eat(b',');
        }
        blanks(&mut self.text);
        let offset = self.text.offset();
        skip(&mut self.text);
        self.read += 1;
        Some(offset)
    }
}

/// Each pair of an object, its `{` read, handed to `pair` at its key's
/// opening quote, and the object read past its `}`.
fn pairs(text: &mut FieldText<'_>, mut pair: impl FnMut(&FieldText<'_>) -> Read<()>) -> Read<()> {
    blanks(text);
    if text.eat(b'}') {
        return Ok(());
    }
    loop {
        pair(text)?;
        skip(text);
        blanks(text);
        text.eat(b':');
        blanks(text);
        skip(text);
        blanks(text);
        if !text.eat(b',') {
            text.eat(b'}');
            return Ok(());
        }
        blanks(text);
    }
}

/// Read past the value at `text`, a document walked whole.
fn skip(text: &mut FieldText<'_>) {
    match text.peek() {
        Some(b'"') => {
            text.bump();
            skip_string(text);
        }
        Some(b'[' | b'{') => {
            let mut depth = 0usize;
            while let Some(byte) = text.peek() {
                text.bump();
                match byte {
                    b'"' => skip_string(text),
                    b'[' | b'{' => depth += 1,
                    b']' | b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {
            while text.peek().is_some_and(|byte| {
                !matches!(byte, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r')
            }) {
                text.bump();
            }
        }
    }
}

/// Read past a string, its opening quote read.
fn skip_string(text: &mut FieldText<'_>) {
    while let Some(byte) = text.peek() {
        text.bump();
        match byte {
            b'"' => return,
            b'\\' => text.bump(),
            _ => {}
        }
    }
}

/// The string at `text`, its opening quote read, against `bound`, bytewise
/// over the text its escapes stand for.
fn compare_string(text: &mut FieldText<'_>, bound: &[u8]) -> Ordering {
    let mut bytes = StringBytes { text: text.clone(), pending: [0; 4], len: 0, at: 0, done: false };
    let mut bound = bound.iter().copied();
    loop {
        match (bytes.next(), bound.next()) {
            (None, None) => return Ordering::Equal,
            (x, y) if x != y => return x.cmp(&y),
            _ => {}
        }
    }
}

/// The text a JSON string stands for, a byte at a time, its escapes
/// resolved as [`JsonCursor::string`](super::JsonCursor) resolves them.
struct StringBytes<'a> {
    text: FieldText<'a>,
    /// A `\u` escape's character, encoded, and how much of it is read.
    pending: [u8; 4],
    len: u8,
    at: u8,
    /// Whether its closing quote has been read.
    done: bool,
}

impl<'a> StringBytes<'a> {
    /// The string opening at `text`'s quote.
    fn at(mut text: FieldText<'a>) -> Self {
        text.bump();
        Self { text, pending: [0; 4], len: 0, at: 0, done: false }
    }

    /// Four hex digits, in a document walked whole.
    fn hex4(&mut self) -> u32 {
        let mut code = 0;
        for _ in 0..4 {
            let digit = self.text.peek().and_then(|byte| char::from(byte).to_digit(16));
            self.text.bump();
            code = code << 4 | digit.unwrap_or(0);
        }
        code
    }
}

impl Iterator for StringBytes<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        if self.at < self.len {
            self.at += 1;
            return Some(self.pending[usize::from(self.at - 1)]);
        }
        if self.done {
            return None;
        }
        let byte = self.text.peek()?;
        self.text.bump();
        Some(match byte {
            b'"' => {
                self.done = true;
                return None;
            }
            b'\\' => {
                let escape = self.text.peek()?;
                self.text.bump();
                match escape {
                    b'b' => 0x08,
                    b'f' => 0x0c,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'u' => {
                        let mut code = self.hex4();
                        if (0xd800..0xdc00).contains(&code) {
                            // The low half follows as `\uXXXX`.
                            self.text.bump();
                            self.text.bump();
                            code = 0x10000 + ((code - 0xd800) << 10) + (self.hex4() - 0xdc00);
                        }
                        let encoded = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
                        self.len = encoded.encode_utf8(&mut self.pending).len() as u8;
                        self.at = 1;
                        self.pending[0]
                    }
                    other => other,
                }
            }
            other => other,
        })
    }
}

#[cfg(test)]
mod tests;
