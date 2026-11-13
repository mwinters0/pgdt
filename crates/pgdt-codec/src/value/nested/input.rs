//! The literal scanner: the `*_in` supersets a filter's literal and a strict
//! parse's field are read by.
//!
//! [`super::output`] reads what a dump *holds*. This module reads what the
//! server would — a filter's right-hand side, which the `*_in` functions
//! accept a good deal more of than the matching `*_out` ever writes (I44),
//! through separate scanners (`docs/design/decisions.md`, "D45"). **Each
//! grammar is a walk** fed its text a byte at a time, handing its caller every
//! part as it reads it, which `parse_*` collect and a strict check reads a
//! field's bytes by, holding no part past its turn but a range's two bounds
//! ("D143"), and looking at no byte past the one it is fed, so a field is
//! read as its runs go by ("D144").
//!
//! **They are five grammars, not one**, and whitespace is where they first
//! disagree: `array_in` drops unquoted whitespace around an element, while
//! `record_in` and `range_in` keep every byte of it, so `( 1 , a )` into a
//! composite is a two-field value whose second field is `" a "`. One predicate
//! still serves all five, because `array_isspace`, `scanner_isspace` and the C
//! locale's `isspace` are the same six characters.
//!
//! The result is the same `ArrayLiteral`/`RecordLiteral`/`RangeLiteral` the
//! decoders produce, holding the parts **as the user spelled them**: an
//! element is whatever text the server would have handed the element type's
//! own `*_in`, not that type's canonical output. Putting the two sides of a
//! comparison into one spelling is the caller's job, only the caller knowing
//! the element type.

use std::borrow::Cow;

use super::{ArrayLiteral, RangeLiteral, RecordLiteral};
use crate::value::source::{Parts, Refused, Walked};

mod array;
mod int2vector;
mod quoted;
mod range;
mod record;

pub(crate) use array::ArrayWalk;
pub(crate) use int2vector::Int2VectorWalk;
pub(crate) use range::{MultirangeMark, MultirangeWalk, RangeMark, RangeWalk};
pub(crate) use record::RecordWalk;

/// **Every part a walk hands over, collected**: each one's text, `None` for
/// SQL NULL, refused where it is not UTF-8.
#[derive(Default)]
struct Collect {
    parts: Vec<Option<String>>,
    part: Vec<u8>,
    /// How much of the part was collected at the mark.
    mark: usize,
}

impl<K: Copy> Parts<K> for Collect {
    fn begin(&mut self, _: K) -> Walked {
        self.part.clear();
        Ok(())
    }

    fn byte(&mut self, byte: u8) -> Walked {
        self.part.push(byte);
        Ok(())
    }

    fn end(&mut self) -> Walked {
        let text = String::from_utf8(std::mem::take(&mut self.part)).map_err(|_| Refused)?;
        self.parts.push(Some(text));
        Ok(())
    }

    fn null(&mut self, _: K) -> Walked {
        self.parts.push(None);
        Ok(())
    }

    fn mark(&mut self) {
        self.mark = self.part.len();
    }

    fn rewind(&mut self) {
        self.part.truncate(self.mark);
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
/// `box`, the one built-in's that is not `,` (I22): [`ArrayWalk`]'s elements
/// collected.
pub fn parse_array_delimited(s: &str, delim: u8) -> Option<ArrayLiteral<'_>> {
    // Owned unconditionally: `array_in` trims and unescapes, so an element
    // here is rarely the bytes the user typed, and a strict check walks a
    // field's array rather than collecting it.
    let mut parts = Collect::default();
    let mut walk = ArrayWalk::new(delim);
    for byte in s.bytes() {
        walk.byte(byte, &mut parts).ok()?;
    }
    walk.end().ok()?;
    let shape = walk.shape();
    Some(ArrayLiteral {
        elements: parts.parts.into_iter().map(|part| part.map(Cow::Owned)).collect(),
        dims: shape.dims,
        lower_bounds: shape.lower_bounds,
    })
}

/// Parse a `record_in` literal for a composite with `columns` declared fields:
/// [`RecordWalk`]'s fields collected.
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
    let mut parts = Collect::default();
    let mut walk = RecordWalk::new(columns);
    for byte in s.bytes() {
        walk.byte(byte, &mut parts).ok()?;
    }
    walk.end().ok()?;
    Some(RecordLiteral { fields: parts.parts })
}

/// Parse a `range_in` literal ([`RangeWalk`]).
///
/// The bounds are **not canonicalized** — `[1,10]` stays inclusive-inclusive
/// here, where the server would rewrite it to `[1,11)` for a discrete subtype.
/// That needs the subtype's own successor function, which this module does not
/// have.
pub fn parse_range(s: &str) -> Option<RangeLiteral> {
    let mut walk = RangeWalk::default();
    for byte in s.bytes() {
        walk.byte(byte).ok()?;
    }
    walk.end().ok()
}

/// Parse a `multirange_in` literal into its member ranges
/// ([`MultirangeWalk`]).
///
/// **A member spelled `empty` is accepted and dropped**, which
/// [`decode_multirange`] refuses because `multirange_out` never writes one. A
/// member that is *equivalent* to empty — `[1,1)` — is kept here and dropped
/// by the server, which compares its bounds in the subtype's order, an order
/// this module does not have, as [`parse_range`] has no successor function.
///
/// [`decode_multirange`]: super::output::decode_multirange
pub fn parse_multirange(s: &str) -> Option<Vec<RangeLiteral>> {
    let mut members = Vec::new();
    let mut walk = MultirangeWalk::default();
    let mut each = |member| {
        members.push(member);
        Ok(())
    };
    for byte in s.bytes() {
        walk.byte(byte, &mut each).ok()?;
    }
    walk.end().ok()?;
    Some(members)
}

/// Parse an `int2vectorin` literal — everything the server accepts for an
/// `int2vector` column, which is a strict superset of what
/// [`decode_int2vector`] reads ([`Int2VectorWalk`]).
///
/// [`decode_int2vector`]: super::output::decode_int2vector
pub fn parse_int2vector(s: &str) -> Option<Vec<i16>> {
    let mut out = Vec::new();
    let mut walk = Int2VectorWalk::default();
    let mut each = |value| {
        out.push(value);
        Ok(())
    };
    for byte in s.bytes() {
        walk.byte(byte, &mut each).ok()?;
    }
    walk.end(&mut each).ok()?;
    Some(out)
}

#[cfg(test)]
mod differential;
#[cfg(test)]
mod prior_shape;
#[cfg(test)]
mod tests;
