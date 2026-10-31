//! A table read with its descendants, as `SELECT … FROM <table>` reads it
//! on the server (`docs/design/decisions.md`, "D131"): which tables a map
//! lists — each a block holds rows for, and each declared ancestor of one —
//! and the blocks a read of one reads, its own and every descendant's, or
//! why it is refused, read with them or alone.

use std::collections::BTreeSet;

use pgdt_codec::internal::database_for_name;
use pgdt_core::Error;
use pgdt_model::{
    CopyBlock, DataBlock, DatabaseMetadata, DumpIndex, SpanBody, TableName, TocHeader,
};

use crate::options::Inheritance;

/// Every `COPY` block of `index`, in file order, beside the database its span
/// records and the table it holds rows for, where that is not the one its
/// header names: the partition a block written through its partition root
/// holds, which its TOC entry names (I2), or `None` where it carries none.
fn blocks_held(
    index: &DumpIndex,
) -> impl Iterator<Item = (Option<&str>, &CopyBlock, Option<TableName>)> {
    index.spans.iter().filter_map(|span| match &span.body {
        SpanBody::Data(DataBlock::Copy(block)) => {
            let database = span.database.as_deref();
            let held = block
                .partition_root
                .as_ref()
                .and(span.toc.as_ref())
                .filter(|toc| toc.kind == "TABLE DATA")
                .map(|toc| partition_named(database, toc));
            Some((database, block, held))
        }
        _ => None,
    })
}

/// The table a `TABLE DATA` entry's header names, in `database`.
fn partition_named(database: Option<&str>, toc: &TocHeader) -> TableName {
    TableName {
        database: database.map(str::to_owned),
        schema: toc.schema.clone(),
        table: toc.name.clone(),
    }
}

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
    for (database, block, held) in blocks_held(index) {
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
    /// Why a read of it cannot be made, under each mode, where it cannot.
    pub(super) refused: Refusals,
}

/// **Why a read of a table refuses**, under each [`Inheritance`] mode: a
/// table is resolved once and read under either (`docs/design/decisions.md`,
/// "D132"), so both are settled with its blocks.
#[derive(Debug, Clone, Default)]
pub(super) struct Refusals {
    /// Read with its descendants: blocks written through a partition root
    /// other than it hold its rows or a descendant's.
    descendants: Option<Refused>,
    /// Read alone: such blocks hold its own rows, or its own blocks, naming
    /// it as their partition root, hold its partitions'.
    only: Option<Refused>,
}

impl Refusals {
    /// The refusal of a read of `table` under `inheritance`, where it
    /// refuses — `holds_own` saying whether it holds a block of its own. One
    /// read alone holding none takes the schema its read with its descendants
    /// settles, so it refuses as that read does too
    /// (`docs/design/decisions.md`, "D132").
    pub(super) fn of(
        &self,
        table: &TableName,
        inheritance: Inheritance,
        holds_own: bool,
    ) -> Option<Error> {
        let refused = match inheritance {
            Inheritance::Descendants => self.descendants.as_ref(),
            Inheritance::Only if holds_own => self.only.as_ref(),
            Inheritance::Only => self.only.as_ref().or(self.descendants.as_ref()),
        };
        refused.map(|refused| refused.refusal(table, inheritance))
    }

    /// Whether a read under either mode is made.
    pub(super) fn reads_either(&self) -> bool {
        self.descendants.is_none() || self.only.is_none()
    }
}

/// **Why a read of a table cannot tell its rows from others'**, the
/// partition key's bounds alone telling them apart.
#[derive(Debug, Clone)]
enum Refused {
    /// Blocks written through `root` hold the table's rows, or a
    /// descendant's.
    ThroughRoot { root: String, header_offsets: Vec<u64> },
    /// The table is a partition root whose own blocks hold its partitions'
    /// rows, written through it.
    RootAlone { header_offsets: Vec<u64> },
}

impl Refused {
    /// The refusal of a read of `table` under `inheritance`
    /// ([`Error::PartitionThroughRoot`], [`Error::PartitionRootAlone`]).
    fn refusal(&self, table: &TableName, inheritance: Inheritance) -> Error {
        let table = table.qualified_name();
        match self {
            Self::ThroughRoot { root, header_offsets } => Error::PartitionThroughRoot {
                table,
                root: root.clone(),
                header_offsets: header_offsets.clone(),
                inheritance_only: inheritance == Inheritance::Only,
            },
            Self::RootAlone { header_offsets } => {
                Error::PartitionRootAlone { table, header_offsets: header_offsets.clone() }
            }
        }
    }

    /// Blocks written through a root, refusing a read of what they hold: the
    /// first's root, every one's offset; `None` where there are none.
    fn through(blocks: &[(String, u64)]) -> Option<Self> {
        let (root, _) = blocks.first()?;
        let header_offsets = blocks.iter().map(|(_, offset)| *offset).collect();
        Some(Self::ThroughRoot { root: root.clone(), header_offsets })
    }
}

/// **What a read of `name` reads**: every block of it and of each table its
/// database declares a descendant of it, in file order — and, for each mode
/// it may be read in, the blocks written through a partition root that refuse
/// it ([`Refusals`]).
///
/// deficiency: KD158 — a `--load-via-partition-root` dump's partitions, and
/// each partitioned table below its root, refuse under their own names
/// (`Error::PartitionThroughRoot`), and the root read alone refuses
/// (`Error::PartitionRootAlone`): a restore routes each row to the
/// partition whose bound admits it, comparing the partition key under its
/// operator classes and collations (I86), which nothing here evaluates. The
/// root read with its descendants is whole. **(c) unowned**; promoted by
/// `roadmap.md`'s Future item "Rows routed to their partitions".
pub(super) fn table_read(index: &DumpIndex, name: &TableName) -> TableRead {
    let database = name.database.as_deref();
    let qualified = name.qualified_name();
    let descendants: Vec<TableName> = ddl(index, database)
        .map(|db| db.descendants(&qualified))
        .into_iter()
        .flatten()
        .map(|key| declared_name(database, key))
        .collect();
    let reads = |table: &TableName| table == name || descendants.contains(table);
    // A block a TOC entry names no partition for (`KD1`) may hold any of its
    // root's partitions' rows.
    let below = |root: &TableName| {
        ddl(index, database)
            .is_some_and(|db| db.descendants(&root.qualified_name()).contains(&qualified.as_str()))
    };
    let mut blocks = Vec::new();
    // Blocks through another root holding this table's rows or a
    // descendant's, and those of them that may hold its own.
    let (mut through, mut through_own) = (Vec::new(), Vec::new());
    // Its own blocks, naming it as the root they were written through.
    let mut as_root = Vec::new();
    for (in_database, block, held) in blocks_held(index) {
        if in_database != database {
            continue;
        }
        let named = TableName::of(in_database, block);
        if reads(&named) {
            if &named == name && block.partition_root.is_some() {
                as_root.push(block.header_offset);
            }
            blocks.push(block.clone());
        } else if block.partition_root.is_some()
            && held.as_ref().map_or_else(|| below(&named), reads)
        {
            let at = (named.qualified_name(), block.header_offset);
            if held.as_ref().is_none_or(|held| held == name) {
                through_own.push(at.clone());
            }
            through.push(at);
        }
    }
    let only = Refused::through(&through_own).or_else(|| {
        let header_offsets = as_root;
        (!header_offsets.is_empty()).then_some(Refused::RootAlone { header_offsets })
    });
    TableRead { blocks, refused: Refusals { descendants: Refused::through(&through), only } }
}
