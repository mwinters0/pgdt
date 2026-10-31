//! **TimescaleDB's catalog's verdict on a read**: which of the tables a read
//! reaches are chunks whose rows the restore holds compressed, which tables
//! an unreadable catalog leaves unread with their descendants, and what a
//! read that is made hears of the catalog — its root's own rows, its absent
//! chunks, a read unchecked for compression, chunks it cannot know absent
//! (`docs/design/roadmap-P38-descendants.md`, "TimescaleDB"). The catalog's
//! conclusions are the model's ([`TimescaleDb`]); what refuses a read is
//! decided here, beside the blocks it chooses, over the tables the DDL's walk
//! reaches (`docs/design/decisions.md`, "D134"), and what it hears is the
//! map's findings said per read (`docs/design/decisions.md`, "D135").
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

use std::collections::{BTreeMap, BTreeSet};

use pgdt_model::timescaledb::{Release, Resolution, TimescaleDb, absent_chunks, root_rows};
use pgdt_model::{DatabaseMetadata, DumpIndex};

use crate::options::Inheritance;
use crate::stream::{PlanNote, PlanNoteKind};

/// What one database's catalog says of the tables a read reaches, where the
/// database is TimescaleDB's.
pub(super) struct Verdict {
    catalog: Catalog,
    /// Each hypertable's root's own rows, where it holds any: every
    /// hypertable a readable catalog lists, or each an unreadable one's rows
    /// name, as `schema.table`.
    root_rows: BTreeMap<String, u64>,
    /// Whether the release is known to be one whose `ONLY` reads a
    /// hypertable empty, so a read alone of its root differs from the
    /// server's too.
    only_reads_empty: bool,
}

/// The catalog read, or why not.
enum Catalog {
    /// Every chunk it lists compressed or partially compressed, and each
    /// hypertable's chunks the file lacks, each as `schema.table`.
    Resolved { compressed: BTreeSet<String>, absent: BTreeMap<String, Vec<String>> },
    /// What it lacks and holds unread; the hypertables its rows still name,
    /// or `None` where even those do not read.
    Unreadable {
        missing: Vec<String>,
        malformed: Vec<String>,
        hypertables: Option<BTreeSet<String>>,
    },
}

impl Verdict {
    /// The verdict of database `database`'s catalog, its DDL `ddl`, `None`
    /// where the database is not TimescaleDB's ([`TimescaleDb::of`]). The
    /// root rows and absent chunks are counted as the map's findings count
    /// them ([`root_rows`], [`absent_chunks`]), none absent without the DDL.
    // timescaledb: TS4 — bits 1 and 8 mark a chunk whose rows are compressed.
    // timescaledb: TS6 — a dropped row names no table, so it is no chunk.
    // timescaledb: TS8 — from 2.28 `ONLY` a hypertable reads empty on the server.
    pub(super) fn of(
        index: &DumpIndex,
        database: Option<&str>,
        ddl: Option<&DatabaseMetadata>,
    ) -> Option<Verdict> {
        let timescaledb = TimescaleDb::of(index, database)?;
        let only_reads_empty = timescaledb.version.at_least(Release::new(2, 28));
        let mut rooted = BTreeMap::new();
        let mut count = |hypertable: &str| {
            let rows = root_rows(index, database, hypertable);
            if rows > 0 {
                rooted.insert(hypertable.to_owned(), rows);
            }
        };
        let catalog = match timescaledb.resolution {
            Resolution::Resolved(resolved) => {
                let hypertables = &resolved.hypertables;
                hypertables.iter().for_each(|hypertable| count(&hypertable.table));
                let absent = hypertables
                    .iter()
                    .filter_map(|hypertable| {
                        let absent = absent_chunks(ddl?, hypertable);
                        (!absent.is_empty()).then(|| (hypertable.table.clone(), absent))
                    })
                    .collect();
                let compressed = hypertables
                    .iter()
                    .flat_map(|hypertable| &hypertable.chunks)
                    .filter(|chunk| !chunk.dropped)
                    .filter(|chunk| chunk.compressed() || chunk.partially_compressed())
                    .map(|chunk| chunk.table.clone())
                    .collect();
                Catalog::Resolved { compressed, absent }
            }
            Resolution::Unreadable { missing, malformed, hypertables, .. } => {
                hypertables.iter().flatten().for_each(|hypertable| count(hypertable));
                let hypertables = hypertables.map(|names| names.into_iter().collect());
                Catalog::Unreadable { missing, malformed, hypertables }
            }
        };
        Some(Verdict { catalog, root_rows: rooted, only_reads_empty })
    }

    /// The chunks among `reached` — the tables a read reads, in the order it
    /// walks them — whose rows are compressed: none where the catalog did
    /// not read, a chunk read so being unchecked.
    pub(super) fn compressed<'a>(&self, reached: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        match &self.catalog {
            Catalog::Resolved { compressed, .. } => reached
                .into_iter()
                .filter(|table| compressed.contains(*table))
                .map(str::to_owned)
                .collect(),
            Catalog::Unreadable { .. } => Vec::new(),
        }
    }

    /// What the catalog lacks and holds unread, where a read of `table` with
    /// its descendants is refused for its not reading: `table`
    /// `has_descendants`, and is a hypertable the catalog's rows name, or any
    /// table where those do not read. The refusal guards compression, which a
    /// read reaching no chunk cannot meet, so a named hypertable the file
    /// holds no chunk of reads as a plain table, its note saying what is left
    /// unknown ([`PlanNoteKind::HypertableChunksUnknown`]).
    pub(super) fn unreadable_refusal(
        &self,
        table: &str,
        has_descendants: bool,
    ) -> Option<(&[String], &[String])> {
        let Catalog::Unreadable { missing, malformed, hypertables } = &self.catalog else {
            return None;
        };
        let refuses =
            has_descendants && hypertables.as_ref().is_none_or(|names| names.contains(table));
        refuses.then_some((missing.as_slice(), malformed.as_slice()))
    }

    /// **What a read of `table` hears of the catalog**, under each mode, its
    /// descendants `descendants` in `ddl`, its database's DDL: read with
    /// them, its root's own rows and its absent chunks; read alone, its
    /// root's own rows where the server's `ONLY` reads none; and under an
    /// unreadable catalog, either way, that a table it reaches may be a
    /// chunk — below a hypertable the catalog's rows name, or below any table
    /// where they name none — which no check of compression reached, and read
    /// with its descendants where it is a hypertable the rows name and has
    /// none, that which of its chunks the file lacks cannot be known. A mode
    /// whose read refuses never plans, so never raises its notes.
    pub(super) fn notes(
        &self,
        table: &str,
        descendants: &[String],
        ddl: Option<&DatabaseMetadata>,
    ) -> ReadNotes {
        let mut notes = ReadNotes::default();
        if let Some(&rows) = self.root_rows.get(table) {
            let root = PlanNoteKind::HypertableRootRows { table: table.to_owned(), rows };
            if self.only_reads_empty {
                notes.only.push(note(root.clone()));
            }
            notes.descendants.push(note(root));
        }
        match &self.catalog {
            Catalog::Resolved { absent, .. } => {
                if let Some(chunks) = absent.get(table) {
                    let table = table.to_owned();
                    let chunks = chunks.clone();
                    notes
                        .descendants
                        .push(note(PlanNoteKind::HypertableChunksAbsent { table, chunks }));
                }
            }
            Catalog::Unreadable { missing, malformed, hypertables } => {
                let chunk = |reached: &str| {
                    let ancestors = ddl.map(|ddl| ddl.ancestors(reached)).unwrap_or_default();
                    match hypertables {
                        Some(names) => ancestors.iter().any(|ancestor| names.contains(*ancestor)),
                        None => !ancestors.is_empty(),
                    }
                };
                let unchecked = || {
                    note(PlanNoteKind::CompressionUnchecked {
                        table: table.to_owned(),
                        missing: missing.clone(),
                        malformed: malformed.clone(),
                    })
                };
                if chunk(table) {
                    notes.only.push(unchecked());
                }
                if chunk(table) || descendants.iter().any(|descendant| chunk(descendant)) {
                    notes.descendants.push(unchecked());
                }
                let named = hypertables.as_ref().is_some_and(|names| names.contains(table));
                if named && descendants.is_empty() {
                    notes.descendants.push(note(PlanNoteKind::HypertableChunksUnknown {
                        table: table.to_owned(),
                        missing: missing.clone(),
                        malformed: malformed.clone(),
                    }));
                }
            }
        }
        notes
    }
}

/// A note no setting moves.
fn note(kind: PlanNoteKind) -> PlanNote {
    PlanNote { kind, levers: Vec::new() }
}

/// **What a read of a table hears of TimescaleDB's catalog**, under each
/// [`Inheritance`] mode, settled with its blocks beside its refusals, as
/// they are (`docs/design/decisions.md`, "D132").
#[derive(Debug, Clone, Default)]
pub(super) struct ReadNotes {
    descendants: Vec<PlanNote>,
    only: Vec<PlanNote>,
}

impl ReadNotes {
    /// The notes of a read under `inheritance`.
    pub(super) fn of(&self, inheritance: Inheritance) -> &[PlanNote] {
        match inheritance {
            Inheritance::Descendants => &self.descendants,
            Inheritance::Only => &self.only,
        }
    }
}
