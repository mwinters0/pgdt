//! Pull-mode streaming API (`docs/design/decisions.md`, "Batches, streams and
//! the leader").
//!
//! A [`TablePartitions`] plan, made by [`Table::plan`](crate::Table::plan),
//! yields its partitions, the whole table in file order, and a serial resume,
//! each a [`TableStream`] built on [`CopyScanner`](pgdt_dump::CopyScanner)
//! and [`RowBatcher`](crate::batch::RowBatcher).
//! [`ResumeToken`] resumes a consumption within the same process; it holds no
//! public fields (`docs/design/decisions.md`, "D49").
//!
//! **Mapping and streaming are separate passes**
//! (`docs/design/decisions.md`, "D48"), never interleaved:
//!
//! 1. A mapping pass settles the table's blocks — `pgdt_mapping`'s
//!    `map_file` to the file's end, a [`Dump`](crate::Dump) built over it,
//!    or a cold query's pass ([`Table::settle`](crate::Table::settle)),
//!    stopping as soon as the selected table is settled — and **yields no
//!    rows**.
//! 2. Every block the table holds is then replayed for its rows, in file
//!    order.
//!
//! So the queried block's bytes are read twice, and a [`ResumeToken`] can only
//! ever point inside already-mapped territory.
//!
//! **Type resolution**: a matching `COPY` block's column list is resolved
//! against the dump's DDL into a [`pgdt_codec::ResolvedSchema`]
//! ([`TablePartitions::resolved_schema`]), and the batches a plan's streams
//! yield are built to it: typed under `SchemaMode::Typed`, all-`Utf8View`
//! under `SchemaMode::Strings`. A table's blocks may list its columns in
//! different orders, and every batch comes out in the table's one order
//! ([`TableColumns`](self::replay::plan::TableColumns)).

mod merge;
mod note;
mod partitions;
mod replay;
mod resume;
mod table;

pub use self::note::{EarlyStop, PlanLever, PlanNote, PlanNoteKind};
pub use self::partitions::{DynamicPartitions, TablePartitions};
pub use self::replay::dynamic::{DynamicFilter, RowEvaluation};
pub use self::resume::ResumeToken;
pub(crate) use self::table::table_schema;
pub use self::table::{BlockingTableIter, TableStream};

#[cfg(test)]
mod tests;
