//! PostgreSQL values: every decoder and renderer, the datetime grammar, nested
//! literals, a declared type's Arrow mapping, the comparison register and its
//! order keys, whether PostgreSQL refuses a field, a value's unrepresentable
//! tier, and column resolution (`docs/design/decisions.md`, "D68").
//! Synchronous, no I/O, and Arrow's schema types only but `i256` (D74).

#![forbid(unsafe_code)]

pub mod compare;
mod declared;
pub mod decode;
pub mod grammar;
pub mod key;
pub mod nested;
pub mod pgtype;
mod resolve;
mod unrepresentable;
mod value;

pub use resolve::{ColumnResolution, ResolvedColumn, ResolvedSchema, SchemaMode, resolve_columns};
pub use unrepresentable::UnrepresentableMode;

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::compare::discrete::DiscreteExt;
    pub use crate::compare::divergence::datafusion_position_divergences;
    pub use crate::grammar::check::{
        column_grammar, field_refused, text_typmod, unchecked_positions, walks,
    };
    pub use crate::grammar::walk::{FieldWalk, grammar_refuses};
    pub use crate::key::equality::{Equality, SpellingRuns, Spellings, literal_equality};
    pub use crate::key::factor::{KeyFactor, key_factor, nested_key_factor};
    pub use crate::key::form::{accepted_form, nested_accepted_form, unlisted_labels};
    pub use crate::key::order::{
        OrderKey, Side, compare_keys, compare_keys_stored, field_key, jsonb_order, literal_key,
        order_key, read_key,
    };
    pub use crate::key::runs::{BytewiseRuns, FieldOrder};
    pub use crate::resolve::{column_of, read_as_text, schema_of};
    pub use crate::unrepresentable::{
        ColumnTier, UnrepresentableRead, column_tiers, counter_for, unrepresentable_reads,
        unrepresentable_tests,
    };
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::unrepresentable::{column_read, scalar_read, scalar_tier};
    pub use crate::value::bytea::{ByteaRuns, bytea_escape_head, bytea_unread, decode_bytea_into};
    pub use crate::value::digits::push_integer;
    pub use crate::value::float::{Float, float_field};
    pub use crate::value::nested::key::{
        NestedKey, compare_nested, nested_field_order, nested_key,
    };
    pub use crate::value::nested::{
        FieldText, RangeShape, walk_array, walk_bracketed, walk_int2vector, walk_multirange,
        walk_range, walk_record,
    };
    pub use crate::value::nested::{push_array_null, quote_array_element};
    pub use crate::value::numeric::{NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX};
    pub use crate::value::text::{TextFold, TextRead, char_typmod_refuses, escaped_utf8_error};
}
