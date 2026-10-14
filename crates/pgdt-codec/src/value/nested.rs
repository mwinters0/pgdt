//! The nested literal codec: array, composite (record), range, multirange and
//! `int2vector` literals, in both directions — as PostgreSQL's `*_out`
//! functions write them inside a single `COPY` field, and as its `*_in`
//! functions read what a user typed.
//!
//! Pure, synchronous, no I/O and no Arrow (`docs/design/decisions.md`, "D74").
//! Like `crate::decode`, every function here works on the *already
//! COPY-unescaped* text `pgdt_core::unescape_field` returns, and every
//! `render_*` is the exact inverse of its `decode_*`: feeding it a value
//! decoded from real `pg_dump` output reproduces that output byte for byte.
//! Turning the result back into on-disk bytes is `pgdt_core::escape_field`'s
//! job, not this module's.
//!
//! **`decode_*` and `parse_*` are two scanners, not one with a flag**
//! (`docs/design/decisions.md`, "D45"): the first reads a dump and cannot
//! loosen, the second reads a filter's literal and cannot tighten.
//!
//! Both produce the three literal types this module defines, holding the
//! parts as they were written: [`output`] reads and writes what a dump
//! holds, and [`input`] reads what a user typed. [`key`] orders either.

mod input;
pub(crate) mod key;
mod output;

use std::borrow::Cow;

pub use input::{
    parse_array, parse_array_delimited, parse_int2vector, parse_multirange, parse_range,
    parse_record,
};
pub use output::{
    decode_array, decode_int2vector, decode_multirange, decode_range, decode_record,
    push_array_null, quote_array_element, render_array, render_int2vector, render_multirange,
    render_range, render_record,
};

/// `array_isspace`, and `isspace()` in the C locale: the same six characters,
/// which is why one predicate serves all three forms.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
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
/// is copied (`nested-decode-micro`; `docs/design/decisions.md`, "D45").
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

    /// Whether the value renders an `[lb:ub]=` prefix: some lower bound is
    /// not 1, as `array_out` decides.
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
