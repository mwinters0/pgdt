//! Pull-mode streaming API (`docs/design/decisions.md`, "Batches, streams and
//! the leader").
//!
//! [`table_stream`] is the primitive: an async `Stream<Item =
//! Result<RecordBatch>>` built directly on [`CopyScanner`](pgdt_dump::CopyScanner)/[`RowBatcher`](crate::batch::RowBatcher), the
//! same machinery [`crate::batch::read_table`] (push mode) drives internally.
//! [`ResumeToken`] resumes a consumption within the same process; it holds no
//! public fields (`docs/design/decisions.md`, "D49").
//!
//! **Mapping and streaming are separate passes**
//! (`docs/design/decisions.md`, "D48"). A query runs in two phases, never
//! interleaved:
//!
//! 1. `pgdt_mapping`'s `map_forward` extends the [`DumpIndex`](pgdt_model::DumpIndex)'s map from its own
//!    `scanned_through`, recording every `COPY` block it passes and
//!    classifying the DDL between them through a [`pgdt_dump::internal::Builder`], and
//!    **yields nothing**. It stops as soon as the queried table is settled
//!    ([`ScanExtent::UntilTargetSettled`](crate::batch::ScanExtent::UntilTargetSettled)).
//! 2. Every block the map holds for that table is then replayed for its rows,
//!    in file order.
//!
//! So the queried block's bytes are read twice, and a [`ResumeToken`] can only
//! ever point inside already-mapped territory. A [`CacheMode::Enabled`](pgdt_dump::cache::CacheMode::Enabled) cache
//! is persisted where the map advances: completed blocks whose save has earned
//! its cost (`pgdt_mapping`'s save throttle), the block that settles the query, and every
//! exit past the first read but an error. [`CacheMode::Disabled`](pgdt_dump::cache::CacheMode::Disabled) runs the same way with `save` a no-op.
//!
//! **Preamble capture** (`docs/design/decisions.md`, "D30"): before any of
//! that, [`table_stream`] runs [`pgdt_dump::internal::scan_preamble`] once (skipped
//! once a cache already has it), whatever table was queried and under
//! [`CacheMode::Disabled`](pgdt_dump::cache::CacheMode::Disabled) too. It covers the *first* database; every later
//! `\connect`ed one is stated by `map_forward` at that database's first
//! `COPY` block (I1).
//!
//! **Type resolution**: a matching `COPY` block's column list is resolved
//! against that captured metadata into a [`pgdt_codec::ResolvedSchema`]
//! ([`TableStream::resolved_schema`]), and the batches this stream yields are
//! built to it: typed under `SchemaMode::Typed`, all-`Utf8View` under
//! `SchemaMode::Strings`. A table's blocks may list its columns in different
//! orders, and every batch comes out in the table's one order
//! ([`TableColumns`](self::table::TableColumns)).

mod dynamic;
mod map_query;
mod note;
mod partition;
mod plan;
mod replay;
mod resolve;
mod segment;
mod table;
mod table_partitions;

pub use self::dynamic::{DynamicFilter, RowEvaluation};
pub use self::note::{EarlyStop, PlanLever, PlanNote, PlanNoteKind};
pub use self::table::{
    BlockingTableIter, ResumeToken, TableStream, table_schema, table_stream,
    table_stream_partitions,
};
pub use self::table_partitions::{DynamicPartitions, TablePartitions};

#[cfg(test)]
mod tests;
