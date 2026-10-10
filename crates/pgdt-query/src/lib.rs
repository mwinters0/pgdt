//! Arrow batches, the streams, the filter expression, statistics gathering
//! and pruning, and summaries (`docs/design/decisions.md`, "D68"): what an
//! embedder — `datafusion-pgdump`, `datafusion-cli-pgdump`, `pgdt` — reads a
//! dump through.

mod batch;
mod gather;
mod predicate;
mod prune;
mod statistics;
mod stream;
mod summary;

pub use batch::{
    QueryOptions, ScanExtent, decode_field, read_table, render_field, render_field_into,
};
pub use gather::{
    StrictUnchecked, UncheckedBound, UncheckedCheck, UncheckedColumn, strict_unchecked,
};
pub use predicate::{
    ComparisonNote, Expr, Membership, Predicate, PredicateOp, Truth, column_divergences,
};
pub use statistics::{
    BLOCK_MAX_ROW_GROUPS, ROW_GROUP_DEFAULT_MIN_ROWS, ROW_GROUP_DEFAULT_SIZE_BYTES, StatisticsHeld,
    StatisticsLevel, StatisticsRequest, StatisticsSelection, StatisticsTarget, StatisticsTerms,
};
pub use stream::{
    BlockingTableIter, DynamicFilter, DynamicPartitions, EarlyStop, MapRun, PlanLever, PlanNote,
    PlanNoteKind, ResumeToken, RowEvaluation, TablePartitions, TableStream, build_index, build_map,
    map_file, table_schema, table_stream, table_stream_partitions,
};
pub use summary::{Bound, ColumnSummary, TableSummary, table_summary};

/// What this crate's own integration tests reach that is not its API: public
/// only because a crate boundary forced it, and named here so a caller
/// reaching past the API names the word. No embedder names it.
pub mod internal {
    pub use crate::statistics::{StatisticsBackfill, StatisticsRequestExt};
    pub use crate::stream::{BlockReread, bounded_columns, gather_block_statistics};
}
