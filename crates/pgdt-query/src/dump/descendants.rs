//! A table read with its descendants, as `SELECT … FROM <table>` reads it
//! on the server (`docs/design/decisions.md`, "D131"): which tables a map
//! lists — each a block holds rows for, and each declared ancestor of one —
//! and the blocks a read of one reads, its own and every descendant's, or
//! why it is refused.

use std::collections::BTreeSet;

use pgdt_codec::internal::database_for_name;
use pgdt_core::Error;
use pgdt_model::{
    CopyBlock, DataBlock, DatabaseMetadata, DumpIndex, SpanBody, TableName, TocHeader,
};

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
    /// Why it cannot be read, where it cannot.
    pub(super) refused: Option<ThroughRoot>,
}

/// **A table whose rows a partition root's blocks hold**: blocks written
/// through `root` holding its rows, or a descendant's, which only the
/// partition key's bounds would tell from its siblings'.
#[derive(Debug, Clone)]
pub(super) struct ThroughRoot {
    root: String,
    header_offsets: Vec<u64>,
}

impl ThroughRoot {
    /// The refusal of a read of `table` ([`Error::PartitionThroughRoot`]).
    pub(super) fn refusal(&self, table: &TableName) -> Error {
        Error::PartitionThroughRoot {
            table: table.qualified_name(),
            root: self.root.clone(),
            header_offsets: self.header_offsets.clone(),
        }
    }
}

/// **What a read of `name` reads**: every block of it and of each table its
/// database declares a descendant of it, in file order — or, where blocks
/// written through a partition root other than `name` hold its rows or a
/// descendant's, those blocks, for its refusal.
///
/// deficiency: KD158 — a `--load-via-partition-root` dump's partitions, and
/// each partitioned table below its root, refuse under their own names
/// (`Error::PartitionThroughRoot`): a restore routes each row to the
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
    let mut through: Vec<(String, u64)> = Vec::new();
    for (in_database, block, held) in blocks_held(index) {
        if in_database != database {
            continue;
        }
        let named = TableName::of(in_database, block);
        if reads(&named) {
            blocks.push(block.clone());
        } else if block.partition_root.is_some()
            && held.as_ref().map_or_else(|| below(&named), reads)
        {
            through.push((named.qualified_name(), block.header_offset));
        }
    }
    let refused = through.first().map(|(root, _)| ThroughRoot {
        root: root.clone(),
        header_offsets: through.iter().map(|(_, offset)| *offset).collect(),
    });
    TableRead { blocks, refused }
}
