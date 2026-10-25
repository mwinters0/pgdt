//! Per-type field decode and render-back (`docs/design/decisions.md`,
//! "D45"): the namespace declaring each family's reader and renderer, which
//! sit with the rest of what PostgreSQL says about their type under
//! `crate::value`, and the [`Unread`] every reader answers with.
//!
//! Pure, synchronous, no I/O (`docs/design/decisions.md`, "D74"). A decode
//! function takes an already-COPY-unescaped `&str` field (what
//! `pgdt_core::unescape_field` returns) and returns a plain Rust value; it
//! never takes an Arrow builder — building the array is query's job
//! (`batch/builder.rs`), which is also where these are wired to their [`arrow_schema::DataType`].
//!
//! Every `render_*` function is the exact inverse of its `decode_*`
//! counterpart — but [`render_bytea`], which writes the hex form whichever
//! form [`decode_bytea`] read (`docs/design/decisions.md`, "D66") — and produces the same *decoded* text `pgdt_core::unescape_field`
//! would have returned for a correctly-formed value — never the raw
//! COPY-escaped bytes on disk. Converting between the two is
//! `pgdt_core::escape_field`'s job (`docs/design/decisions.md`, "D74").
//! pgdt-query's `tests/decode.rs` round-trip test compares against `SchemaMode::Strings`,
//! which is also decoded text; the on-disk-byte leg is covered separately in
//! `pgdt-mapping`'s `tests/scan.rs`.

pub use crate::value::bool::{decode_bool, render_bool};
pub use crate::value::bytea::{
    decode_bytea, decode_bytea_escape, render_bytea, render_bytea_escape,
};
pub use crate::value::datetime::{
    decode_date32, decode_interval, decode_time64_micros, decode_timestamp_micros, render_date32,
    render_date32_into, render_interval, render_time64_micros, render_time64_micros_into,
    render_timestamp_micros, render_timestamp_micros_into,
};
pub use crate::value::float::{decode_f32, decode_f64, render_f32, render_f64};
pub use crate::value::numeric::{decimal_unscaled_digits, render_decimal, typmod_unscaled_digits};
pub use crate::value::uuid::{decode_uuid, render_uuid};

/// Why a text is read as no value of its type, where a caller must tell the
/// two apart: a parse fails on the first field PostgreSQL refuses, and on no
/// other (`roadmap.md`, "A literal is guaranteed in `*_out`'s form and never
/// read past `*_in`'s"). Why a reader stays narrow and a classifier answers
/// this where it fails: `docs/design/decisions.md`, "D55".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// The type's `*_in` refuses it: found at a check carrying a
    /// `pg-refuses: I<n>` marker, so a release lifting the refusal is found
    /// by walking that invariant. Such a check sits behind a grammar no wider
    /// than the server's at the parts it bounds, or a shortfall's spelling
    /// reaches it — a run-together `+0530` read as an hour of 530, digits
    /// counted before an exponent moved the point — and refuses what the
    /// server reads; so every failure of a date or time reader, its own
    /// refusals included, is put to `crate::value::datetime::grammar`, the
    /// readers splitting a text outside `*_out`'s form otherwise than `ParseDateTime`.
    Refused,
    /// A spelling this build reads no value from that the server reads at
    /// some supported major — a shortfall (`docs/design/decisions.md`,
    /// "D55"). Each reader classifies what it fails on by the server's
    /// grammar (`int_unread`, `float_unread`, `numeric_unread`,
    /// `crate::value::json`'s `jsonb_unread`, and `crate::value::datetime::grammar` for the
    /// date and time types). No reader is widened to read one: a blank
    /// around a number, a hexadecimal float, `nan(…)`, an exponent in a
    /// `numeric` field, `0x1F`, `1_000` and `y2001m02d04` stay unparsed, and
    /// a query decoding one fails with `FieldDecode`.
    Unparsed,
}

/// A reader's answer where [`Unread`] matters.
pub type Read<T> = Result<T, Unread>;

/// The straightforward `format!`/`parse` spellings of the four scalar
/// decoders, the two hex renderers and the three date/time renderers, kept as
/// the oracle the allocation-free forms this namespace declares are checked
/// against. Those forms
/// are asked to be exactly this but where a test names the difference, so the
/// corpora below are generated rather
/// than listed: a disagreement on an input nobody thought to write down is a
/// test failure and not a report from the field.
#[cfg(test)]
mod prior_shape;

#[cfg(test)]
mod differential;
