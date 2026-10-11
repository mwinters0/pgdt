//! Arrow batches, the streams, the filter expression, pruning, and summaries
//! (`docs/design/decisions.md`, "D68"): what an embedder —
//! `datafusion-pgdump`, `datafusion-cli-pgdump`, `pgdt` — reads a dump
//! through.

mod batch;
mod options;
mod predicate;
mod prune;
mod render;
mod stream;
mod summary;

pub use options::{QueryOptions, ScanExtent};
pub use predicate::{
    ComparisonNote, Expr, Membership, Predicate, PredicateOp, Truth, column_divergences,
};
pub use render::{decode_field, render_field, render_field_into};
pub use stream::{
    BlockingTableIter, DynamicFilter, DynamicPartitions, EarlyStop, PlanLever, PlanNote,
    PlanNoteKind, ResumeToken, RowEvaluation, TablePartitions, TableStream, read_table,
    table_schema, table_stream, table_stream_partitions,
};
pub use summary::{Bound, ColumnSummary, TableSummary, table_summary};
