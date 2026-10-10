//! Arrow batches, the streams, the filter expression, statistics gathering
//! and pruning, and summaries (`docs/design/decisions.md`, "D68"): what an
//! embedder — `datafusion-pgdump`, `datafusion-cli-pgdump`, `pgdt` — reads a
//! dump through.

pub mod batch;
mod gather;
pub mod predicate;
mod prune;
pub mod statistics;
pub mod stream;
pub mod summary;

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
    BLOCK_MAX_ROW_GROUPS, ROW_GROUP_DEFAULT_MIN_ROWS, ROW_GROUP_DEFAULT_SIZE_BYTES,
    StatisticsBackfill, StatisticsHeld, StatisticsLevel, StatisticsRequest, StatisticsSelection,
    StatisticsTarget, StatisticsTerms,
};
pub use stream::{
    BlockReread, BlockingTableIter, DynamicFilter, DynamicPartitions, EarlyStop, MapRun, PlanLever,
    PlanNote, PlanNoteKind, ResumeToken, RowEvaluation, TablePartitions, TableStream,
    bounded_columns, build_index, build_map, gather_block_statistics, map_file, table_schema,
    table_stream, table_stream_partitions,
};
pub use summary::{Bound, ColumnSummary, TableSummary, table_summary};
