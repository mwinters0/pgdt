//! Per-row-group column statistics: what a mapping pass may be asked to
//! gather for a `COPY` block ([`StatisticsRequest`]), and what holding them
//! cost it ([`StatisticsHeld`]). The shape they persist in is
//! `pgdt_model`'s ([`pgdt_model::BlockStatistics`]); which column gets which
//! statistic, and how a value is ordered, is decided by the observer a block
//! gets, the mapping pass handing it every row and staying type-blind.
//!
//! The namespace also holds a strict parse's check (`docs/design/decisions.md`,
//! "D103"): the observer checking every field against its type's `*_in` and
//! its column's `NOT NULL`, and what that check leaves unchecked
//! ([`strict_unchecked`](crate::strict_unchecked)).

pub(crate) mod account;
pub(crate) mod gather;
pub(crate) mod request;
pub(crate) mod stored;

pub use self::account::{StatisticsHeld, StatisticsTerms};
pub use self::request::{
    BLOCK_MAX_ROW_GROUPS, ROW_GROUP_DEFAULT_MIN_ROWS, ROW_GROUP_DEFAULT_SIZE_BYTES,
    StatisticsBackfill, StatisticsLevel, StatisticsRequest, StatisticsSelection, StatisticsTarget,
};
