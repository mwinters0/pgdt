//! What a file map answers of its spans: the `COPY` blocks among them, the
//! tables those hold rows for and the partition roots they are written
//! through (`docs/design/decisions.md`, "D34", "D136"). The shapes are the
//! cache's (`crate::persisted::span`); `pgdt_dump::map` builds them and
//! `pgdt_dump::cache` persists them.

use std::collections::BTreeSet;

use crate::diagnostic::DiagnosticExt;
use crate::{
    ArrayShape, BlockAddress, BlockRecord, BlockStatistics, CopyBlock, DataBlock, DatabaseMetadata,
    Diagnostic, DumpIndex, SpanBody, Unrepresentable,
};

/// Per-span cap on stored text (`docs/design/decisions.md`, "D30") — a whole
/// schema's DDL being small, it bites only on one enormous statement.
pub const SPAN_STORED_TEXT_MAX_BYTES: usize = 64 * 1024;

/// **A block's identity**: what every site keying, matching or ordering a
/// block holds, in dump order, and nothing it prints — a display site prints
/// the block's address (`docs/design/decisions.md`, "D167"). Derived from
/// the address wherever one is held ([`BlockAddress::id`], and the scanner's
/// `CopyStart`), and never persisted: nothing holding one outlives its
/// process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(u64);

/// What builds a [`BlockId`] outside this crate: the scanner deriving one
/// from the header it reads.
pub trait BlockIdExt {
    /// The identity of the block whose `COPY` header starts at
    /// `header_offset` — a plain dump's address.
    fn at_header(header_offset: u64) -> Self;
}

impl BlockIdExt for BlockId {
    fn at_header(header_offset: u64) -> Self {
        Self(header_offset)
    }
}

impl BlockAddress {
    /// The block's identity ([`BlockId`]), derived from its address.
    pub fn id(&self) -> BlockId {
        BlockId::at_header(self.header_offset)
    }
}

impl BlockRecord {
    /// The census's array shapes, where the block holds one.
    pub fn array_shapes(&self) -> Option<&[ArrayShape]> {
        self.census.as_ref().map(|census| census.shapes.as_slice())
    }

    /// The census's unrepresentable counts, where the block holds one.
    pub fn unrepresentable(&self) -> Option<&[Unrepresentable]> {
        self.census.as_ref().map(|census| census.unrepresentable.as_slice())
    }
}

/// The census of several blocks read as one, one entry per name in `columns`
/// — what a table spanning more than one `COPY` block (I2) resolves against,
/// and what a query's schema commits to over exactly the blocks it will
/// replay (`docs/design/decisions.md`, "D35").
///
/// **Keyed by column name, not by position**: a leaf partition's block lists
/// its columns in the leaf's own order, which need not be the root's or a
/// sibling's. A name a block carries and `columns` does not contributes
/// nothing, and so does a census entry past its header's list — a block
/// naming no columns copies none (I5). A block holding no census contributes
/// nothing either, so a caller believing the union has first made sure every
/// block holds one.
pub fn union_census<'a>(
    columns: &[String],
    blocks: impl IntoIterator<Item = &'a CopyBlock>,
) -> Vec<ArrayShape> {
    let mut out = vec![ArrayShape::default(); columns.len()];
    for block in blocks {
        let census = block.record.array_shapes().into_iter().flatten();
        for (name, shape) in block.header.columns.iter().zip(census) {
            let slot = columns.iter().position(|c| c == name);
            if let Some(slot) = slot.and_then(|slot| out.get_mut(slot)) {
                slot.merge(shape);
            }
        }
    }
    out
}

/// One table as its `COPY` headers name it: the database whose `\connect`
/// governs the block, the schema qualifier and the table, each exactly as the
/// header spelled it once unquoted.
///
/// **An exact key, never a pattern**: what a caller that read the table off
/// the map holds, so nothing is split and no two tables can answer to it
/// (`docs/design/decisions.md`, "D49"). A name a person typed, bare or
/// qualified, is a front end's to read into what it selects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableName {
    pub database: Option<String>,
    pub schema: Option<String>,
    pub table: String,
}

impl TableName {
    /// The table `block` holds rows for, in `database` — the one its span
    /// records ([`DumpIndex::blocks_with_database`]).
    pub fn of(database: Option<&str>, block: &CopyBlock) -> Self {
        Self {
            database: database.map(str::to_owned),
            schema: block.header.schema.clone(),
            table: block.header.table.clone(),
        }
    }

    /// Whether `block`, in `database`, holds rows for this table.
    pub fn names(&self, database: Option<&str>, block: &CopyBlock) -> bool {
        self.database.as_deref() == database
            && self.schema == block.header.schema
            && self.table == block.header.table
    }

    /// `schema.table`, or `table` where no schema qualifies it — the header's
    /// own [`CopyHeader::qualified_name`](pgdt_core::CopyHeader::qualified_name), not re-quoted.
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.table),
            None => self.table.clone(),
        }
    }
}

/// **A partition root blocks are written through** (I2), and those blocks
/// ([`DumpIndex::through_roots`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThroughRoot<'a> {
    /// The root, which every one of its blocks' headers names, in its
    /// database.
    pub root: TableName,
    /// Each block written through it, in file order, beside the partition
    /// it holds where its TOC entry names one (`KD1` names where none does).
    pub blocks: Vec<(&'a CopyBlock, Option<TableName>)>,
}

impl ThroughRoot<'_> {
    /// **Its finding**: the root, its blocks' offsets, and the partitions
    /// they hold, each once in file order — `None` where a block's TOC entry
    /// names none, so any partition below the root may hold its rows.
    pub fn finding(&self) -> Diagnostic {
        let header_offsets =
            self.blocks.iter().map(|(block, _)| block.address.header_offset).collect();
        let mut partitions: Option<Vec<String>> = Some(Vec::new());
        for (_, held) in &self.blocks {
            let (Some(listed), Some(held)) = (partitions.as_mut(), held) else {
                partitions = None;
                break;
            };
            let held = held.qualified_name();
            if !listed.contains(&held) {
                listed.push(held);
            }
        }
        Diagnostic::partitions_through_root(
            self.root.database.clone(),
            self.root.qualified_name(),
            header_offsets,
            partitions,
        )
    }
}

impl DumpIndex {
    /// The DDL of `database`, where the map holds it
    /// ([`DumpMetadata::database`](crate::DumpMetadata::database)).
    pub fn ddl(&self, database: Option<&str>) -> Option<&DatabaseMetadata> {
        self.metadata.as_deref().and_then(|metadata| metadata.database(database))
    }

    /// The databases the map holds, each once: every one the DDL names, in
    /// the order it states them, then any a block holds rows in that it does
    /// not — the unnamed database of a plain dump, or one whose catalog blocks
    /// alone name it (TS2). So an unnamed database is one only where a block
    /// is in it: the stretch of a `pg_dumpall` file before its first
    /// `\connect`, holding roles and no table, is not one.
    pub fn databases(&self) -> Vec<Option<&str>> {
        let named = self
            .metadata
            .iter()
            .flat_map(|metadata| &metadata.databases)
            .filter_map(|db| db.name.as_deref().map(Some));
        let holding = self.blocks_with_database().map(|(database, _)| database);
        let mut databases = Vec::new();
        for database in named.chain(holding) {
            if !databases.contains(&database) {
                databases.push(database);
            }
        }
        databases
    }

    /// The `COPY` blocks among `spans`, in file order — a filtered view, not
    /// a stored field, so a block's byte offsets have exactly one owner
    /// (`docs/design/decisions.md`, "D34").
    pub fn blocks(&self) -> impl Iterator<Item = &CopyBlock> {
        self.spans.iter().filter_map(|s| match &s.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some(block),
            _ => None,
        })
    }

    /// The `COPY` blocks among `spans`, in file order, each beside the
    /// database its span records — the one place a block's database is
    /// stored (`docs/design/decisions.md`, "D34").
    pub fn blocks_with_database(&self) -> impl Iterator<Item = (Option<&str>, &CopyBlock)> {
        self.spans.iter().filter_map(|s| match &s.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some((s.database.as_deref(), block)),
            _ => None,
        })
    }

    /// Every `COPY` block, in file order, beside the database its span
    /// records and the table it holds rows for, where that is not the one its
    /// header names: the partition a block written through its partition root
    /// holds, which its TOC entry names (I2), or `None` where it carries none.
    pub fn blocks_held(
        &self,
    ) -> impl Iterator<Item = (Option<&str>, &CopyBlock, Option<TableName>)> {
        self.spans.iter().filter_map(|span| match &span.body {
            SpanBody::Data(DataBlock::Copy(block)) => {
                let database = span.database.as_deref();
                let held = block
                    .partition_root
                    .as_ref()
                    .and(span.toc.as_ref())
                    .filter(|toc| toc.kind == "TABLE DATA")
                    .map(|toc| TableName {
                        database: database.map(str::to_owned),
                        schema: toc.schema.clone(),
                        table: toc.name.clone(),
                    });
                Some((database, block, held))
            }
            _ => None,
        })
    }

    /// **Every partition root blocks are written through** (I2), each once,
    /// in the order its first such block appears, beside those blocks: what
    /// a read of each partition below it refuses on, which partition a row
    /// belongs to not being read, and what its finding names
    /// ([`ThroughRoot::finding`], `docs/design/decisions.md`, "D136").
    pub fn through_roots(&self) -> Vec<ThroughRoot<'_>> {
        let mut roots: Vec<ThroughRoot<'_>> = Vec::new();
        for (database, block, held) in self.blocks_held() {
            if block.partition_root.is_none() {
                continue;
            }
            let root = TableName::of(database, block);
            let at = match roots.iter().position(|through| through.root == root) {
                Some(at) => at,
                None => {
                    roots.push(ThroughRoot { root, blocks: Vec::new() });
                    roots.len() - 1
                }
            };
            roots[at].blocks.push((block, held));
        }
        roots
    }

    /// Every table this map holds a `COPY` block for, each once, in the order
    /// its first block appears.
    pub fn tables(&self) -> Vec<TableName> {
        let mut seen = BTreeSet::new();
        self.blocks_with_database()
            .map(|(database, block)| TableName::of(database, block))
            .filter(|name| seen.insert(name.clone()))
            .collect()
    }

    /// The blocks of exactly `table`, in file order.
    pub fn blocks_of<'a>(&'a self, table: &'a TableName) -> impl Iterator<Item = &'a CopyBlock> {
        self.blocks_with_database()
            .filter(move |(database, b)| table.names(*database, b))
            .map(|(_, block)| block)
    }

    /// The heap every block's statistics hold
    /// ([`BlockStatistics::heap_bytes`]) — what a mapping pass bills
    /// for the statistics a cache handed it, and what a caller holding this
    /// map resident bills for keeping them.
    pub fn statistics_heap_bytes(&self) -> u64 {
        self.blocks()
            .filter_map(|block| block.record.statistics.as_deref())
            .map(BlockStatistics::heap_bytes)
            .sum()
    }

    pub fn total_rows(&self) -> u64 {
        self.blocks().map(|b| b.record.row_count).sum()
    }

    /// Whether this scan reached the end of a `size`-byte file — the test
    /// that decides when a whole-file fact may be believed.
    /// [`roles`](Self::roles), [`tablespaces`](Self::tablespaces) and a
    /// **reported** table schema carry that partiality; a **streamed** schema
    /// is not gated on it, committing over exactly the blocks it will replay
    /// (`docs/design/decisions.md`, "D34", "D35"). The caller always has
    /// `size` already.
    pub fn is_complete(&self, size: u64) -> bool {
        self.scanned_through >= size
    }
}
