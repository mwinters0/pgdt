//! A dump read through its complete map and its tables, Arrow batches, the
//! streams, the filter expression, pruning, summaries, and the render — an
//! Arrow value read back as PostgreSQL text
//! (`docs/design/decisions.md`, "D68"): what an embedder —
//! `datafusion-pgdump`, `datafusion-cli-pgdump`, `pgdt` — reads a dump
//! through.

mod batch;
mod dump;
mod options;
mod predicate;
mod prune;
mod render;
mod stream;
mod summary;

pub use dump::{Dump, Table, TableSelector};
pub use options::{QueryOptions, ScanExtent};
pub use predicate::{ComparisonNote, Expr, Operand, Predicate, PredicateOp, Truth};
pub use render::{decode_field, render_field, render_field_into};
pub use stream::{
    BlockingTableIter, DynamicFilter, DynamicPartitions, EarlyStop, PlanLever, PlanNote,
    PlanNoteKind, ResumeToken, RowEvaluation, TablePartitions, TableStream,
};
pub use summary::{Bound, ColumnSummary, TableSummary};
