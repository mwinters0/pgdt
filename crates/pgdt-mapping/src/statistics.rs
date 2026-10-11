//! Per-row-group column statistics: what a mapping pass may be asked to
//! gather for a `COPY` block ([`StatisticsRequest`]), and what holding them
//! cost it ([`StatisticsHeld`]). The shape they persist in is
//! `pgdt_model`'s ([`pgdt_model::BlockStatistics`]); which column gets which
//! statistic, and how a value is ordered, is decided by the observer a block
//! gets, the mapping pass handing it every row and staying type-blind.

pub(crate) mod account;
pub(crate) mod gather;
pub(crate) mod request;
pub(crate) mod stored;

pub use self::account::{StatisticsHeld, StatisticsTerms};
pub use self::request::{
    BLOCK_MAX_ROW_GROUPS, ROW_GROUP_DEFAULT_MIN_ROWS, ROW_GROUP_DEFAULT_SIZE_BYTES,
    StatisticsBackfill, StatisticsLevel, StatisticsRequest, StatisticsSelection, StatisticsTarget,
};
