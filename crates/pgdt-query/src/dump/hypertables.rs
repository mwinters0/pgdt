//! **TimescaleDB's catalog's verdict on a read**: which of the tables a read
//! reaches are chunks whose rows the restore holds compressed, and which
//! tables an unreadable catalog leaves unread with their descendants
//! (`docs/design/roadmap-P38-descendants.md`, "TimescaleDB"). The catalog's
//! conclusions are the model's ([`TimescaleDb`]); what refuses a read is
//! decided here, beside the blocks it chooses, over the tables the DDL's walk
//! reaches (`docs/design/decisions.md`, "D134").
//!
//! A chunk the catalog lists that the DDL does not declare is no descendant
//! of its hypertable, so a read leaves it out, as its restore does (TS10); a
//! row marking a chunk dropped names no table (TS6), so it is no chunk.
//!
//! deficiency: KD159 — a TimescaleDB chunk whose rows are compressed or
//! partially compressed (status bit 1 or 8) refuses read by its own name,
//! under either mode, and so does every table reading it with its
//! descendants, its hypertable among them (`Error::CompressedChunks`): a
//! restore holds those rows as codec batches in a compressed relation, whose
//! format is Timescale License source and is not decoded here. The relation
//! reads as its batches, each other chunk under its own name and the
//! hypertable alone as its root's own rows. **(c) unowned**; promoted by
//! `roadmap.md`'s Future item "Compressed TimescaleDB chunks, decoded from a
//! clean room".

use std::collections::BTreeSet;

use pgdt_model::DumpIndex;
use pgdt_model::timescaledb::{Resolution, TimescaleDb};

/// What one database's catalog says of the tables a read reaches, where the
/// database is TimescaleDB's.
pub(super) enum Verdict {
    /// The catalog read: every chunk it lists compressed or partially
    /// compressed, as `schema.table`.
    Resolved { compressed: BTreeSet<String> },
    /// The catalog did not read, for what it lacks and holds unread; the
    /// hypertables its rows still name, or `None` where even those do not
    /// read.
    Unreadable {
        missing: Vec<String>,
        malformed: Vec<String>,
        hypertables: Option<BTreeSet<String>>,
    },
}

impl Verdict {
    /// The verdict of database `database`'s catalog, `None` where the
    /// database is not TimescaleDB's ([`TimescaleDb::of`]).
    // timescaledb: TS4 — bits 1 and 8 mark a chunk whose rows are compressed.
    // timescaledb: TS6 — a dropped row names no table, so it is no chunk.
    pub(super) fn of(index: &DumpIndex, database: Option<&str>) -> Option<Verdict> {
        Some(match TimescaleDb::of(index, database)?.resolution {
            Resolution::Resolved(resolved) => Verdict::Resolved {
                compressed: resolved
                    .hypertables
                    .iter()
                    .flat_map(|hypertable| &hypertable.chunks)
                    .filter(|chunk| !chunk.dropped)
                    .filter(|chunk| chunk.compressed() || chunk.partially_compressed())
                    .map(|chunk| chunk.table.clone())
                    .collect(),
            },
            Resolution::Unreadable { missing, malformed, hypertables, .. } => {
                let hypertables = hypertables.map(|names| names.into_iter().collect());
                Verdict::Unreadable { missing, malformed, hypertables }
            }
        })
    }

    /// The chunks among `reached` — the tables a read reads, in the order it
    /// walks them — whose rows are compressed: none where the catalog did
    /// not read, a chunk read so being unchecked.
    pub(super) fn compressed<'a>(&self, reached: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        match self {
            Verdict::Resolved { compressed } => reached
                .into_iter()
                .filter(|table| compressed.contains(*table))
                .map(str::to_owned)
                .collect(),
            Verdict::Unreadable { .. } => Vec::new(),
        }
    }

    /// Whether a read of `table` with its descendants is refused for the
    /// catalog not reading: it is a hypertable the catalog's rows name, or,
    /// where those do not read, any table `has_descendants`.
    pub(super) fn unreadable_refuses(&self, table: &str, has_descendants: bool) -> bool {
        match self {
            Verdict::Resolved { .. } => false,
            Verdict::Unreadable { hypertables: Some(names), .. } => names.contains(table),
            Verdict::Unreadable { hypertables: None, .. } => has_descendants,
        }
    }
}
