//! PostgreSQL values: every decoder and renderer, the datetime grammar, nested
//! literals, a declared type's Arrow mapping, the comparison register and its
//! order keys, whether PostgreSQL refuses a field, a value's unrepresentable
//! tier, and column resolution (`docs/design/decisions.md`, "D68").
//! Synchronous, no I/O, and Arrow's schema types only but `i256` (D74).

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

pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use unrepresentable::UnrepresentableMode;

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::compare::discrete::DiscreteExt;
    pub use crate::compare::divergence::datafusion_position_divergences;
    pub use crate::grammar::check::{
        column_grammar, field_refused, grammar_refuses, unchecked_positions,
    };
    pub use crate::grammar::form::{accepted_form, nested_accepted_form};
    pub use crate::key::{
        OrderKey, Side, compare_keys, literal_key, order_key, read_key, render_macaddr,
        special_order_key,
    };
    pub use crate::resolve::{database_for_name, read_as_text};
    pub use crate::unrepresentable::{
        ColumnTier, UnrepresentableRead, column_tiers, counter_for, unrepresentable_reads,
        unrepresentable_tests,
    };
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::unrepresentable::{scalar_read, scalar_tier};
    pub use crate::value::bytea::bytea_unread;
    pub use crate::value::datetime::{
        render_timestamp_postgres_micros, time_of_day_micros, timestamp_postgres_micros,
    };
    pub use crate::value::digits::push_integer;
    pub use crate::value::float::{Float, float_field};
    pub use crate::value::int::int_in;
    pub use crate::value::nested::key::{NestedKey, compare_nested, nested_key};
    pub use crate::value::nested::{push_array_null, quote_array_element};
    pub use crate::value::numeric::{
        NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX, numeric_in_stores,
    };
    pub use crate::value::text::char_typmod_refuses;
}
