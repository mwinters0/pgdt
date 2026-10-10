//! PostgreSQL values: every decoder, the datetime grammar, nested literals,
//! the type register, the comparison plans and their ordering keys, and
//! column resolution (`docs/design/decisions.md`, "D68"). Synchronous, no
//! I/O, and Arrow's schema types only (D74).

mod datetime_in;
pub mod decode;
pub mod key;
mod nested;
pub mod pgtype;
mod resolve;
mod unrepresentable;

pub use nested::{
    ArrayLiteral, RangeLiteral, RecordLiteral, decode_array, decode_int2vector, decode_multirange,
    decode_range, decode_record, parse_array, parse_array_delimited, parse_int2vector,
    parse_multirange, parse_range, parse_record, render_array, render_int2vector,
    render_multirange, render_range, render_record,
};
pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use unrepresentable::UnrepresentableMode;

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::decode::digits::push_integer;
    pub use crate::decode::float::{Float, float_field};
    pub use crate::decode::refusal::{
        NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX, bytea_unread, char_typmod_refuses,
        numeric_in_stores,
    };
    pub use crate::decode::stored::{
        render_timestamp_postgres_micros, time_of_day_micros, timestamp_postgres_micros,
    };
    pub use crate::key::order::{
        NestedKey, OrderKey, Side, accepted_form, compare_keys, compare_nested, field_refused,
        grammar_refuses, int_in, literal_key, nested_accepted_form, nested_key, order_key,
        read_key, render_macaddr, special_order_key,
    };
    pub use crate::nested::{push_array_null, quote_array_element};
    pub use crate::pgtype::discrete::DiscreteExt;
    pub use crate::pgtype::divergence::datafusion_position_divergences;
    pub use crate::pgtype::strict::{column_grammar, unchecked_positions};
    pub use crate::resolve::{database_for_name, read_as_text};
    pub use crate::unrepresentable::{
        ColumnTier, UnrepresentableRead, column_tiers, counter_for, unrepresentable_reads,
        unrepresentable_tests,
    };
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::unrepresentable::{scalar_read, scalar_tier};
}
