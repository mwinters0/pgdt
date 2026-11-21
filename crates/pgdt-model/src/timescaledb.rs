//! TimescaleDB's catalog as a dump states it: the rows of its
//! `_timescaledb_catalog` blocks, recorded as text by the pass that maps them
//! ([`RecordedRows`]), and what they conclude ([`TimescaleDb`]) — the release,
//! stated or inferred, the recipe joining a chunk to its hypertable, its ranges
//! and its compressed relation, and the hypertables, chunks and continuous
//! aggregates it finds. A row and what it means kept together, so `pgdt info`
//! and the query layer read one set of conclusions; no Arrow and no I/O
//! (`docs/design/decisions.md`, "D133").
//!
//! **Read by column name, whatever release the catalog states**: `pg_dump`
//! names every column of a catalog block, and an update script may reorder
//! them, so a position means nothing. Where a column the recipe needs is
//! missing, the catalog is [`Resolution::Unreadable`], naming what it lacks.

use serde::Serialize;

use crate::DumpIndex;

mod catalog;
mod findings;
mod recipe;
mod recorded;
mod version;

pub use crate::persisted::timescaledb::RecordedRows;
use catalog::Catalog;
pub use catalog::EXTENSION;
pub use findings::{absent_chunks, compressed_chunks, findings, root_rows};
pub use recipe::{Chunk, ContinuousAggregate, Dimension, Hypertable, Resolved, Slice};
pub(crate) use recorded::recorded_field;
pub use recorded::{CATALOG_SCHEMA, records_rows};
pub use version::{Era, Recipe, Release, Version};

/// What one database's catalog concludes, where the database is
/// TimescaleDB's ([`TimescaleDb::of`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimescaleDb {
    /// Whether the database's DDL creates the extension. A dump selecting
    /// tables by `--table` can hold the catalog without it (TS2).
    pub extension: bool,
    /// The release the catalog states, or the era its shape bounds.
    pub version: Version,
    /// The hypertables, chunks and continuous aggregates, or why the
    /// catalog cannot say.
    pub resolution: Resolution,
}

/// The catalog read by its recipe, or the columns it lacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Resolution {
    Resolved(Resolved),
    /// Columns the recipe needs that the catalog does not hold, each
    /// `table.column` — a whole table missing names each column it needed —
    /// or the columns of fields that do not read: a value not of its
    /// column's type, a NULL or a missing field where one is required, a
    /// reference to a row the catalog lacks, a `relid` naming no qualified
    /// table; and the
    /// hypertables its `hypertable` rows name, read by `schema_name` and
    /// `table_name` alone, as `schema.table` — `None` where those do not
    /// read.
    Unreadable {
        recipe: Recipe,
        missing: Vec<String>,
        malformed: Vec<String>,
        hypertables: Option<Vec<String>>,
    },
}

impl TimescaleDb {
    /// What database `database`'s catalog concludes, `None` where it is not
    /// TimescaleDB's: detected by the extension its DDL creates or by any
    /// block of the catalog's schema (TS2). Read over the blocks `index` has
    /// mapped so far, so a map short of the catalog's blocks concludes from
    /// what it holds.
    pub fn of(index: &DumpIndex, database: Option<&str>) -> Option<TimescaleDb> {
        let catalog = Catalog::of(index, database)?;
        let version = version::version(&catalog);
        let recipe = version::recipe(&version, &catalog);
        let resolution = match recipe::resolve(recipe, &catalog) {
            Ok(resolved) => Resolution::Resolved(resolved),
            Err(fault) => Resolution::Unreadable {
                recipe,
                missing: fault.missing,
                malformed: fault.malformed,
                hypertables: recipe::hypertable_names(&catalog),
            },
        };
        Some(TimescaleDb { extension: catalog.extension, version, resolution })
    }
}

#[cfg(test)]
mod tests;
