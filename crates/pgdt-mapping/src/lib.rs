//! The mapping pass (`docs/design/decisions.md`, "D68"): a map extended from
//! its frontier with its observers attached, a block's interior split among
//! workers, the whole-file map and the back-fill, and what a pass gathers —
//! statistics, and a strict parse's check — and what holding it costs. Names
//! Arrow's `DataType` and builds no array (D74).

mod backfill;
mod forward;
mod interior;
pub mod statistics;
mod whole_file;

pub use backfill::{BlockReread, bounded_columns, gather_block_statistics};
pub use statistics::gather::strict::{
    StrictUnchecked, UncheckedBound, UncheckedCheck, UncheckedColumn, strict_unchecked,
};
pub use whole_file::{MapRun, build_index, build_map, map_file};

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::backfill::{announce_read_loop, reread_rows};
    pub use crate::forward::{MapStop, Pass, map_forward, splice};
    pub use crate::statistics::account::StatisticsAccount;
    #[cfg(feature = "test-support")]
    pub use crate::statistics::gather::one_group_bounds;
    pub use crate::statistics::stored::{
        declared_column, dictionary_holds_field_text, keeps_sums, stored_bounds_kinds,
        stored_resolution,
    };
}
