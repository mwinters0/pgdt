//! PostgreSQL values: every decoder, the datetime grammar, nested literals,
//! the type register, the comparison plans and their ordering keys, and
//! column resolution (`docs/design/decisions.md`, "D68"). Synchronous, no
//! I/O, and Arrow's schema types only (D74).

mod datetime_in;
pub mod decode;
mod key;
pub mod nested;
pub mod pgtype;
pub mod resolve;
mod unrepresentable;

pub use pgtype::{
    CanonicalExtension, CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics,
    IntervalQualifier, NestedCompare, NestedPlan, NumericTypmod, Position, TypeOutcome,
    UnanswerableReason, Unchecked, bounds_set_keyed_by, comparison_for, extension_for,
    resolve_declared_type,
};
pub use resolve::{ColumnNote, ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
pub use unrepresentable::UnrepresentableMode;

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::decode::{
        Float, bytea_unread, float_field, push_integer, render_timestamp_postgres_micros,
        time_of_day_micros, timestamp_postgres_micros,
    };
    pub use crate::key::{
        NestedKey, OrderKey, Side, ValueKey, accepted_form, compare_keys, compare_nested,
        field_refused, grammar_refuses, int_in, literal_key, nested_accepted_form, nested_key,
        order_key, read_key, render_macaddr, special_order_key,
    };
    pub use crate::nested::{push_array_null, quote_array_element};
    pub use crate::pgtype::{column_grammar, datafusion_position_divergences, unchecked_positions};
    pub use crate::resolve::{database_for_name, read_as_text};
    pub use crate::unrepresentable::{
        ColumnTier, UnrepresentableRead, column_tiers, counter_for, unrepresentable_reads,
        unrepresentable_tests,
    };
    #[cfg(any(test, feature = "test-support"))]
    pub use crate::unrepresentable::{scalar_read, scalar_tier};
}
