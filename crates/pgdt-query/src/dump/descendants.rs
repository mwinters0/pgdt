//! A table read with its descendants, as `SELECT … FROM <table>` reads it
//! on the server (`docs/design/decisions.md`, "D131"): which tables a map
//! lists — each a block holds rows for, and each declared ancestor of one —
//! and the blocks a read of one reads, its own and every descendant's, or
//! why it is refused, read with them or alone.

use std::collections::BTreeSet;

use pgdt_codec::internal::database_for_name;
use pgdt_core::Error;
use pgdt_model::{CopyBlock, DatabaseMetadata, DumpIndex, TableName};

use super::hypertables::{ReadNotes, Verdict};
use crate::options::Inheritance;

/// The table a key of [`DatabaseMetadata::tables`] names, in `database`: its
/// schema up to the first `.`, as [`pgdt_core::CopyHeader::matches`] splits a
/// qualified name.
pub(super) fn declared_name(database: Option<&str>, key: &str) -> TableName {
    let (schema, table) = match key.split_once('.') {
        Some((schema, table)) => (Some(schema.to_owned()), table.to_owned()),
        None => (None, key.to_owned()),
    };
    TableName { database: database.map(str::to_owned), schema, table }
}

/// The DDL of `database`, where `index` holds it.
fn ddl<'a>(index: &'a DumpIndex, database: Option<&str>) -> Option<&'a DatabaseMetadata> {
    index.metadata.as_deref().and_then(|metadata| database_for_name(metadata, database))
}

/// **Every table `index` lists, each once**: each table a block holds rows
/// for — the one its header names, and the partition a block written
/// through its root holds — each preceded by every ancestor its database
/// declares and no earlier block listed, the outermost first. So a
/// partitioned table, which `pg_dump` writes no block for, is listed where
/// its first descendant's block is, and every table is in the order its
/// first block, or its first descendant's, appears.
pub(super) fn listed_tables(index: &DumpIndex) -> Vec<TableName> {
    let mut seen = BTreeSet::new();
    let mut listed = Vec::new();
    for (database, block, held) in index.blocks_held() {
        let named = TableName::of(database, block);
        for name in std::iter::once(named).chain(held) {
            let declared = ddl(index, database);
            let ancestors = declared.map(|db| db.ancestors(&name.qualified_name()));
            for ancestor in ancestors.into_iter().flatten().rev() {
                let ancestor = declared_name(database, ancestor);
                if seen.insert(ancestor.clone()) {
                    listed.push(ancestor);
                }
            }
            if seen.insert(name.clone()) {
                listed.push(name);
            }
        }
    }
    listed
}

/// What a read of one table reads ([`table_read`]).
pub(super) struct TableRead {
    /// Its own blocks and every descendant's, in file order.
    pub(super) blocks: Vec<CopyBlock>,
    /// The blocks written through a partition root above it that hold its
    /// descendants' rows, in file order: what a partitioned table below its
    /// root, holding no block, settles its columns from when read alone.
    pub(super) through: Vec<CopyBlock>,
    /// Why a read of it cannot be made, under each mode, where it cannot.
    pub(super) refused: Refusals,
    /// What a read of it hears of TimescaleDB's catalog, under each mode.
    pub(super) notes: ReadNotes,
}

/// **Why a read of a table refuses**, under each [`Inheritance`] mode: a
/// table is resolved once and read under either (`docs/design/decisions.md`,
/// "D132"), so both are settled with its blocks.
#[derive(Debug, Clone, Default)]
pub(super) struct Refusals {
    /// Read with its descendants: blocks written through a partition root
    /// other than it hold its rows or a descendant's, a chunk it reaches is
    /// compressed, or its database's TimescaleDB catalog does not read.
    descendants: Option<Refused>,
    /// Read alone: such blocks hold its own rows — a partitioned table holds
    /// none (I93), so it never refuses here — or it is a compressed chunk.
    only: Option<Refused>,
}

impl Refusals {
    /// The refusal of a read of `table` under `inheritance`, where it
    /// refuses.
    pub(super) fn of(&self, table: &TableName, inheritance: Inheritance) -> Option<Error> {
        let refused = match inheritance {
            Inheritance::Descendants => self.descendants.as_ref(),
            Inheritance::Only => self.only.as_ref(),
        };
        refused.map(|refused| refused.refusal(table, inheritance))
    }

    /// Whether a read under either mode is made.
    pub(super) fn reads_either(&self) -> bool {
        self.descendants.is_none() || self.only.is_none()
    }
}

/// **Why a read of a table is refused**, under one mode.
#[derive(Debug, Clone)]
enum Refused {
    /// Blocks written through `root` hold its rows, or a descendant's, which
    /// the partition key's bounds alone tell apart.
    Through { root: String, header_offsets: Vec<u64> },
    /// TimescaleDB chunks it reaches hold rows compressed.
    Compressed { chunks: Vec<String> },
    /// It may be a hypertable, and its database's catalog does not read.
    CatalogUnreadable { missing: Vec<String>, malformed: Vec<String> },
}

impl Refused {
    /// The refusal of a read of `table` under `inheritance`.
    fn refusal(&self, table: &TableName, inheritance: Inheritance) -> Error {
        let table = table.qualified_name();
        match self {
            Refused::Through { root, header_offsets } => Error::PartitionThroughRoot {
                table,
                root: root.clone(),
                header_offsets: header_offsets.clone(),
                inheritance_only: inheritance == Inheritance::Only,
            },
            Refused::Compressed { chunks } => {
                Error::CompressedChunks { table, chunks: chunks.clone() }
            }
            Refused::CatalogUnreadable { missing, malformed } => {
                Error::TimescaleCatalogUnreadable {
                    table,
                    missing: missing.clone(),
                    malformed: malformed.clone(),
                }
            }
        }
    }

    /// Blocks written through a root, refusing a read of what they hold: the
    /// first's root, every one's offset; `None` where there are none.
    fn through(blocks: &[(String, u64)]) -> Option<Self> {
        let (root, _) = blocks.first()?;
        let header_offsets = blocks.iter().map(|(_, offset)| *offset).collect();
        Some(Self::Through { root: root.clone(), header_offsets })
    }

    /// Compressed chunks, refusing a read reaching them; `None` where there
    /// are none.
    fn compressed(chunks: Vec<String>) -> Option<Self> {
        (!chunks.is_empty()).then_some(Self::Compressed { chunks })
    }
}

/// **What a read of `name` reads**: every block of it and of each table its
/// database declares a descendant of it, in file order — and, for each mode
/// it may be read in, the blocks written through a partition root that refuse
/// it, or the TimescaleDB catalog's verdict that does ([`Refusals`],
/// [`Verdict`]), and what a read of it hears of that catalog ([`ReadNotes`]).
///
/// deficiency: KD158 — a `--load-via-partition-root` dump's partitions
/// refuse under their own names, read with their descendants or alone, and
/// each partitioned table below its root refuses read with its descendants
/// (`Error::PartitionThroughRoot`): a restore routes each row to the
/// partition whose bound admits it, comparing the partition key under its
/// operator classes and collations (I86), which nothing here evaluates. The
/// root read with its descendants is whole, and a partitioned table read
/// alone reads no row (I93). **(c) unowned**; promoted by `roadmap.md`'s
/// Future item "Rows routed to their partitions".
pub(super) fn table_read(index: &DumpIndex, name: &TableName) -> TableRead {
    let database = name.database.as_deref();
    let qualified = name.qualified_name();
    let declared = ddl(index, database);
    let descendants: Vec<TableName> = declared
        .map(|db| db.descendants(&qualified))
        .into_iter()
        .flatten()
        .map(|key| declared_name(database, key))
        .collect();
    // A partitioned table holds no row of its own, wherever a block written
    // through its root may hold its partitions' (I93).
    let partitioned = declared.is_some_and(|db| db.has_partitions(&qualified));
    let reads = |table: &TableName| table == name || descendants.contains(table);
    // A block a TOC entry names no partition for (`KD1`) may hold any of its
    // root's partitions' rows.
    let below = |root: &TableName| {
        declared
            .is_some_and(|db| db.descendants(&root.qualified_name()).contains(&qualified.as_str()))
    };
    let (mut blocks, mut through) = (Vec::new(), Vec::new());
    // Blocks through another root holding this table's rows or a
    // descendant's, and those of them that may hold its own.
    let (mut through_any, mut through_own) = (Vec::new(), Vec::new());
    for (in_database, block) in index.blocks_with_database() {
        if in_database == database && reads(&TableName::of(in_database, block)) {
            blocks.push(block.clone());
        }
    }
    // The roots the map's findings name, read by the same derivation.
    for root in index.through_roots() {
        if root.root.database.as_deref() != database || reads(&root.root) {
            continue;
        }
        for (block, held) in &root.blocks {
            if !held.as_ref().map_or_else(|| below(&root.root), reads) {
                continue;
            }
            let at = (root.root.qualified_name(), block.header_offset);
            let own = match held {
                Some(held) => held == name,
                None => !partitioned,
            };
            if own {
                through_own.push(at.clone());
            }
            through_any.push(at);
            through.push((*block).clone());
        }
    }
    // In file order, whichever root each is written through.
    through.sort_by_key(|block| block.header_offset);
    through_any.sort_by_key(|(_, offset)| *offset);
    through_own.sort_by_key(|(_, offset)| *offset);
    let mut refused = Refusals {
        descendants: Refused::through(&through_any),
        only: Refused::through(&through_own),
    };
    let mut notes = ReadNotes::default();
    if let Some(verdict) = Verdict::of(index, database, declared) {
        // A compressed chunk refuses read by its own name under either mode,
        // and every table reading it with its descendants.
        let names: Vec<String> = descendants.iter().map(TableName::qualified_name).collect();
        let reached = std::iter::once(qualified.as_str()).chain(names.iter().map(String::as_str));
        let own = verdict.compressed([qualified.as_str()]);
        refused.only = refused.only.or_else(|| Refused::compressed(own));
        let reached = verdict.compressed(reached);
        refused.descendants = refused.descendants.or_else(|| Refused::compressed(reached));
        if let Some((missing, malformed)) =
            verdict.unreadable_refusal(&qualified, !descendants.is_empty())
        {
            let unreadable = Refused::CatalogUnreadable {
                missing: missing.to_vec(),
                malformed: malformed.to_vec(),
            };
            refused.descendants = refused.descendants.or(Some(unreadable));
        }
        notes = verdict.notes(&qualified, &names, declared);
    }
    TableRead { blocks, through, refused, notes }
}
