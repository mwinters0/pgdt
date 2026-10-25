//! Query a `pg_dump` file's tables from Apache DataFusion.
//!
//! A dump is opened through the complete cache `pgdt parse` leaves
//! ([`PgDump::open`]) and registered as one catalog per database
//! ([`register_dump`]), its PostgreSQL schemas DataFusion schemas and its
//! tables tables; one table can also be had on its own ([`PgDump::table`]),
//! registered by hand in a session [`ready_session`] readied.
//! A scan is the library's partitioned replay, one DataFusion partition per
//! sub-stream, with the projection pushed into the library by name, a filter
//! pushed `Exact` wherever the library answers it as DataFusion would, a
//! join's, a TopK's or an aggregate's dynamic filter read as each sub-stream
//! runs (`crate::filter::dynamic`), its rows evaluated only where a session
//! turns that on ([`PgDumpSettings`]), the batch
//! size the session's, and the worker count the session's `target_partitions`
//! lowered to what the session's [`ScanBudget`] affords, and the `pgdump.`
//! settings a `SET` states read as each scan is planned ([`PgDumpSettings`]).
//! The plan node carries
//! what the map's statistics say about the table, so `COUNT(*)`,
//! `COUNT(<column>)`, `MIN`, `MAX`, `SUM` and a `COUNT(DISTINCT <column>)`
//! beside another count can be answered without reading a row, and a join weighs its sides by the
//! bytes each emits ([`crate::statistics`]).
//!
//! **What a registration finds is handed to the caller's sink**
//! (`pgdt_model::DiagnosticSink`): the dump's file-level findings, which a
//! table had alone hears only through [`PgDump::report`], and each
//! table's column findings and divergence from PostgreSQL, the columns compared
//! bytewise for want of a recorded collation one finding a registration
//! ([`UncollatedColumns`]); and what a scan's
//! plan settles is handed to it when the scan is planned ([`crate::report`]).
//! What a scan finds while reading is a plan metric under `EXPLAIN ANALYZE`.
//! `CREATE EXTERNAL TABLE … STORED AS PGDUMP` is [`register_table_factory`]'s.
//! Two values keyed in DataFusion's semantics under two orders are ordered
//! as DataFusion answers `a < b` by [`compare_value_keys`].
//!
//! **It reads only the complete cache `pgdt parse` leaves, and never maps**
//! (`docs/design/decisions.md`, "D90"). **A refusal names its remedy in the
//! library's terms** ([`Error`]): a front end hands its own words for them,
//! a [`Spelling`], where it opens a dump ([`PgDumpOptions::spelling`]) or
//! registers the factory, and every refusal of what those build carries them
//! in its message ("D107"). A partition's error is the first
//! DataFusion observes, as for any of its sources, not `pgdt query`'s earliest
//! in file order ("D52"), and a partitioned read is not resumable.
//!
//! **An embedder readies its session with [`ready_session`]**, as `pgdt sql`
//! does: it refuses at planning a query DataFusion would have to evaluate
//! `pgdump_unrepresentable` in, which a session not readied refuses only when
//! it does (`docs/design/decisions.md`, "D101"), and turns off DataFusion
//! 55.1's ungrouped-aggregate dynamic filter, which can lose a column's bounds
//! so that a scan pruning under what is left answers a `MIN` or `MAX` wrongly
//! (`KD56`).
//!
//! **Three figures time it, through `pgdt sql`**:
//! `dynamic-filter-join`, `dynamic-filter-topk` and
//! `parallel-scan-throughput`'s provider legs, under `pgdt`'s allocator, which
//! is the embedder's (`docs/design/decisions.md`, "D13"); `release-profile`
//! counts it too, on its `pgdt sql` row. Every other `query` figure in `docs/design/measurements.md` times `pgdt`,
//! its text rendering included and, cold, a mapping pass, neither of which an
//! embedder holding a cache pays.

#![forbid(unsafe_code)]

mod budget;
mod catalog;
mod dump;
mod error;
mod exec;
mod factory;
mod filter;
mod key;
mod report;
mod session;
mod settings;
mod statistics;
mod table;
mod unrepresentable;

pub use budget::{BudgetAccount, ScanBudget};
pub use catalog::{UNQUALIFIED_SCHEMA, register_dump};
pub use dump::{PgDump, PgDumpOptions};
pub use error::{Error, MappingPass, Refusal, Spelling};
pub use factory::{PGDUMP_FILE_TYPE, register_table_factory};
pub use key::compare_value_keys;
pub use report::{BudgetedPlanNote, Located, RefusedTable, UncollatedColumns};
pub use session::ready_session;
pub use settings::PgDumpSettings;
pub use table::PgDumpTable;
pub use unrepresentable::UNREPRESENTABLE_FUNCTION;
