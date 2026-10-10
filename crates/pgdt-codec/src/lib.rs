//! PostgreSQL values: every decoder, the datetime grammar, nested literals,
//! the type register, the comparison plans and their ordering keys, and
//! column resolution (`docs/design/decisions.md`, "D68"). Synchronous, no
//! I/O, and Arrow's schema types only (D74).

mod datetime_in;
#[path = "decode.rs"]
mod decode_impl;
#[path = "key.rs"]
mod key_impl;
mod nested;
#[path = "pgtype.rs"]
mod pgtype_impl;
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

/// Each scalar's text read into its Rust value and rendered back: an API
/// namespace, its items re-exported by name from the private module that
/// defines them, which is loaded under another name only because this one
/// takes `decode`.
pub mod decode {
    pub use crate::decode_impl::{
        Read, Unread, decimal_unscaled_digits, decode_bool, decode_bytea, decode_bytea_escape,
        decode_date32, decode_f32, decode_f64, decode_interval, decode_time64_micros,
        decode_timestamp_micros, decode_uuid, render_bool, render_bytea, render_bytea_escape,
        render_date32, render_date32_into, render_decimal, render_f32, render_f64, render_interval,
        render_time64_micros, render_time64_micros_into, render_timestamp_micros,
        render_timestamp_micros_into, render_uuid, typmod_unscaled_digits,
    };
}

/// A value's place in its column's order, as PostgreSQL's comparison orders
/// it: an API namespace, its item re-exported by name from the private module
/// that defines it, which is loaded under another name only because this one
/// takes `key`.
pub mod key {
    pub use crate::key_impl::ValueKey;
}

/// The type register: what each declared type resolves to and how its values
/// compare. An API namespace, its items re-exported by name from the private
/// module that defines them, which is loaded under another name only because
/// this one takes `pgtype`.
pub mod pgtype {
    pub use crate::pgtype_impl::{
        CanonicalExtension, CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics,
        Discrete, Geometric, IntervalQualifier, NestedCompare, NestedPlan, NumericTypmod, Position,
        RANGE_STRUCT_FIELDS, TextGrammar, TypeOutcome, UnanswerableReason, Unchecked,
        bounds_set_keyed_by, comparison_for, extension_for, resolve_declared_type,
    };
}

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::decode_impl::{
        Float, NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX, bytea_unread, char_typmod_refuses,
        float_field, numeric_in_stores, push_integer, render_timestamp_postgres_micros,
        time_of_day_micros, timestamp_postgres_micros,
    };
    pub use crate::key_impl::{
        NestedKey, OrderKey, Side, accepted_form, compare_keys, compare_nested, field_refused,
        grammar_refuses, int_in, literal_key, nested_accepted_form, nested_key, order_key,
        read_key, render_macaddr, special_order_key,
    };
    pub use crate::nested::{push_array_null, quote_array_element};
    pub use crate::pgtype_impl::{
        DiscreteExt, column_grammar, datafusion_position_divergences, unchecked_positions,
    };
    pub use crate::resolve::{database_for_name, read_as_text};
    pub use crate::unrepresentable::{
        ColumnTier, UnrepresentableRead, column_tiers, counter_for, unrepresentable_reads,
        unrepresentable_tests,
    };
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::unrepresentable::{scalar_read, scalar_tier};
}
