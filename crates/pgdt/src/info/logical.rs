//! **The logical model the listing opens with**: each table a read takes
//! descendants' rows into, and per TimescaleDB database what its catalog
//! concludes — the release, each hypertable's dimensions and chunks counted
//! whole, compressed and absent from the file, each continuous aggregate — and
//! the chunks the block listing folds under their hypertable, what decides
//! whether a query refuses being what an administrator reads first. Not a
//! TimescaleDB section over a flat listing, which at a reference dump's scale
//! is thousands of chunk lines. Every conclusion is the model's —
//! [`DatabaseMetadata::descendants`], [`TimescaleDb::of`], [`absent_chunks`] —
//! counted here over the blocks the cache holds, never a `Dump`'s
//! (`docs/design/decisions.md`, "D61").

use std::collections::{BTreeMap, BTreeSet};

use pgdt_model::timescaledb::{
    ContinuousAggregate, Hypertable, Resolution, TimescaleDb, Version, absent_chunks,
};
use pgdt_model::{CopyBlock, DatabaseMetadata, DumpIndex};

use super::labels::database_label;

/// The rows each table holds in `database`, by qualified name: each block's
/// counted to the table it holds rows for — a block written through its
/// partition root to the partition its TOC entry names
/// ([`DumpIndex::blocks_held`]), every other to the one its header names.
fn rows_by_table(index: &DumpIndex, database: Option<&str>) -> BTreeMap<String, u64> {
    let mut rows = BTreeMap::new();
    for (_, block, held) in index.blocks_held().filter(|(db, ..)| *db == database) {
        let table = held.map_or_else(|| block.header.qualified_name(), |t| t.qualified_name());
        *rows.entry(table).or_default() += block.row_count;
    }
    rows
}

/// One table some other table names as its `INHERITS` parent or the table it
/// is a partition of: how many tables a read of it reaches beside it
/// ([`DatabaseMetadata::descendants`]) and the rows they and it hold.
struct Parent<'a> {
    table: &'a str,
    descendants: usize,
    rows: u64,
}

/// Every parent `ddl` declares a child of, in name order.
fn parents<'a>(index: &DumpIndex, ddl: &'a DatabaseMetadata) -> Vec<Parent<'a>> {
    let rows = rows_by_table(index, ddl.name.as_deref());
    let named: BTreeSet<&str> = ddl
        .tables
        .values()
        .flat_map(|def| def.parents.iter().chain(def.partition_of.iter().map(|p| &p.parent)))
        .map(String::as_str)
        .collect();
    named
        .into_iter()
        .map(|table| {
            let descendants = ddl.descendants(table);
            let rows = std::iter::once(table)
                .chain(descendants.iter().copied())
                .map(|t| rows.get(t).copied().unwrap_or(0))
                .sum();
            Parent { table, descendants: descendants.len(), rows }
        })
        .collect()
}

/// The section naming every table with descendants, under its database's
/// heading where the cache holds more than one. Returns whether it printed.
pub(super) fn print_descendants(index: &DumpIndex) -> bool {
    let Some(metadata) = &index.metadata else { return false };
    let multi = metadata.databases.len() > 1;
    let mut printed = false;
    for ddl in &metadata.databases {
        let parents = parents(index, ddl);
        if parents.is_empty() {
            continue;
        }
        if !printed {
            println!("tables with descendants:");
            printed = true;
        }
        let indent = if multi {
            println!("    database: {}", database_label(ddl.name.as_deref()));
            "        "
        } else {
            "    "
        };
        for Parent { table, descendants, rows } in parents {
            println!("{indent}{table}: {descendants} descendant(s), {rows} row(s) in all");
        }
    }
    printed
}

/// The release as the section's heading states it.
fn version_label(version: &Version) -> String {
    const INFERRED: &str = "inferred from its catalog's columns";
    match version {
        Version::Stated { version } => format!("{version}, stated by its catalog"),
        Version::Inferred { era } => match era.through {
            Some(through) if through == era.from => format!("{through}, {INFERRED}"),
            Some(through) => format!("{}–{through}, {INFERRED}", era.from),
            None => format!("{} or later, {INFERRED}", era.from),
        },
        Version::Unknown => {
            "release unknown — no version row, and columns no release holds together".into()
        }
    }
}

/// One hypertable's line: its chunks counted whole, then split into those the
/// file holds uncompressed, compressed — which refuse a read reaching them,
/// partially compressed ones too (D134) — and absent from the file, where its
/// database's DDL is held to say which; then its dimensions. An internal
/// compressed hypertable's chunks are counted whole and absent alone, being
/// no user's.
fn hypertable_line(hypertable: &Hypertable, ddl: Option<&DatabaseMetadata>) -> String {
    let chunks: Vec<_> = hypertable.chunks.iter().filter(|c| !c.dropped).collect();
    let absent = ddl.map(|ddl| absent_chunks(ddl, hypertable));
    let held: Vec<_> = chunks
        .iter()
        .filter(|c| absent.as_ref().is_none_or(|absent| !absent.contains(&c.table)))
        .collect();
    let mut counts = Vec::new();
    if !hypertable.internal {
        let compressed = held.iter().filter(|c| c.compressed() || c.partially_compressed());
        let partially = held.iter().filter(|c| c.partially_compressed()).count();
        let compressed = compressed.count();
        counts.push(format!("{} uncompressed", held.len() - compressed));
        counts.push(match partially {
            0 => format!("{compressed} compressed"),
            _ => format!("{compressed} compressed ({partially} partially)"),
        });
    }
    if let Some(absent) = &absent {
        counts.push(format!("{} absent from the file", absent.len()));
    }
    let mut line = format!("hypertable {}: ", hypertable.table);
    if hypertable.internal {
        line.push_str("internal, holding compressed chunks' rows; ");
    }
    line.push_str(&format!("{} chunk(s)", chunks.len()));
    if !counts.is_empty() {
        line.push_str(&format!(": {}", counts.join(", ")));
    }
    let dimensions: Vec<String> = hypertable
        .dimensions
        .iter()
        .map(|d| if d.hashed { format!("{} (hashed)", d.column) } else { d.column.clone() })
        .collect();
    if !dimensions.is_empty() {
        line.push_str(&format!("; dimensions {}", dimensions.join(", ")));
    }
    line
}

/// One continuous aggregate's line: whether it is real-time, and the one way
/// its rows are read — as its materialization hypertable's, the view itself
/// read no way, pgdt modelling no views: reading a materialized-only
/// aggregate under its own name would be view support by the back door
/// (`docs/design/roadmap.md`'s Future item "Views read as DataFusion views").
fn aggregate_line(aggregate: &ContinuousAggregate) -> String {
    let kind = if aggregate.materialized_only { "materialized only" } else { "real-time" };
    let rows = match &aggregate.materialization {
        Some(table) => format!("its rows read as {table}"),
        None => "its materialization hypertable not listed by the catalog".into(),
    };
    format!("continuous aggregate {}: {kind}, {rows}; the view is not read", aggregate.view)
}

/// The section per TimescaleDB database: the release, then what its catalog
/// concludes or that it does not read, the diagnostics above saying why.
/// Returns whether it printed.
pub(super) fn print_timescaledb(index: &DumpIndex) -> bool {
    let databases = index.databases();
    let multi = databases.len() > 1;
    let mut printed = false;
    for database in databases {
        let Some(TimescaleDb { extension, version, resolution }) = TimescaleDb::of(index, database)
        else {
            continue;
        };
        if printed {
            println!();
        }
        printed = true;
        let ddl = index.ddl(database);
        let within =
            if multi { format!(" in database {}", database_label(database)) } else { "".into() };
        println!("TimescaleDB{within}: {}", version_label(&version));
        if !extension {
            println!(
                "    extension: not created — the catalog alone, as a dump selecting tables by \
                 `--table` writes it"
            );
        }
        match resolution {
            Resolution::Resolved(resolved) => {
                for hypertable in &resolved.hypertables {
                    println!("    {}", hypertable_line(hypertable, ddl));
                }
                match &resolved.continuous_aggregates {
                    Some(aggregates) => {
                        for aggregate in aggregates {
                            println!("    {}", aggregate_line(aggregate));
                        }
                    }
                    None => println!(
                        "    continuous aggregates: not known — the catalog's `continuous_agg` \
                         lacks a column pgdt reads"
                    ),
                }
            }
            Resolution::Unreadable { hypertables, .. } => {
                let named = match hypertables.as_deref() {
                    Some([]) => "its rows name no hypertable".to_string(),
                    Some(names) => format!("its rows name hypertable(s) {}", names.join(", ")),
                    None => "the hypertable names its rows hold do not read either".to_string(),
                };
                println!("    catalog: does not read — `diagnostics:` above says why; {named}");
            }
        }
    }
    printed
}

/// What a chunk's block is to its hypertable, as the listing folds it.
#[derive(Clone, Copy)]
enum Member {
    /// A chunk the catalog lists, not dropped.
    Chunk,
    /// The relation holding a chunk's compressed rows, where the catalog
    /// lists it as no chunk of its own (from 2.29).
    CompressedRelation,
}

/// The blocks and rows folded under one hypertable.
#[derive(Default)]
struct Tally {
    chunk_blocks: usize,
    chunk_rows: u64,
    relation_blocks: usize,
    relation_rows: u64,
    /// Whether its line has been printed, beneath the root's first block.
    said: bool,
}

/// **Which blocks the listing folds under their hypertable**, and what it
/// says in their place: each chunk a resolved catalog lists and each
/// compressed relation that is no chunk of its own, beneath its
/// hypertable's first block. A block whose hypertable has no block in the
/// listing stays, there being nothing to fold it under, as does one holding
/// fields a parse went past, which its own lines name.
#[derive(Default)]
pub(super) struct Folds {
    members: BTreeMap<(Option<String>, String), (String, Member)>,
    tallies: BTreeMap<(Option<String>, String), Tally>,
}

impl Folds {
    /// Nothing folded: what `--detail`, and a listing naming each block's
    /// unchecked lines, print.
    pub(super) fn none() -> Self {
        Self::default()
    }

    /// The folds over `blocks`, the listing's, in its file order.
    pub(super) fn of<'a>(
        index: &DumpIndex,
        blocks: impl Iterator<Item = (Option<&'a str>, &'a CopyBlock)> + Clone,
    ) -> Self {
        let mut members = BTreeMap::new();
        for database in index.databases() {
            let Some(TimescaleDb { resolution: Resolution::Resolved(resolved), .. }) =
                TimescaleDb::of(index, database)
            else {
                continue;
            };
            let db = database.map(str::to_owned);
            for hypertable in &resolved.hypertables {
                for chunk in hypertable.chunks.iter().filter(|c| !c.dropped) {
                    let member = (hypertable.table.clone(), Member::Chunk);
                    members.insert((db.clone(), chunk.table.clone()), member);
                }
            }
            for hypertable in &resolved.hypertables {
                for relation in
                    hypertable.chunks.iter().filter_map(|c| c.compressed_relation.as_ref())
                {
                    let member = (hypertable.table.clone(), Member::CompressedRelation);
                    members.entry((db.clone(), relation.clone())).or_insert(member);
                }
            }
        }
        let roots: BTreeSet<(Option<String>, String)> = blocks
            .clone()
            .map(|(database, block)| (database.map(str::to_owned), block.header.qualified_name()))
            .collect();
        members.retain(|(database, _), (hypertable, _)| {
            roots.contains(&(database.clone(), hypertable.clone()))
        });
        let mut folds = Folds { members, tallies: BTreeMap::new() };
        for (database, block) in blocks {
            let Some((hypertable, member)) = folds.member(database, block) else { continue };
            let key = (database.map(str::to_owned), hypertable.to_owned());
            let tally = folds.tallies.entry(key).or_default();
            match member {
                Member::Chunk => {
                    tally.chunk_blocks += 1;
                    tally.chunk_rows += block.row_count;
                }
                Member::CompressedRelation => {
                    tally.relation_blocks += 1;
                    tally.relation_rows += block.row_count;
                }
            }
        }
        folds
    }

    fn member(&self, database: Option<&str>, block: &CopyBlock) -> Option<(&str, Member)> {
        if block.ignored_refusals.is_some() {
            return None;
        }
        let key = (database.map(str::to_owned), block.header.qualified_name());
        self.members.get(&key).map(|(hypertable, member)| (hypertable.as_str(), *member))
    }

    /// Whether `block` is folded rather than listed.
    pub(super) fn folds(&self, database: Option<&str>, block: &CopyBlock) -> bool {
        self.member(database, block).is_some()
    }

    /// The line beneath `block` saying what is folded under it, where it is
    /// the first block of a hypertable with any.
    pub(super) fn beneath(&mut self, database: Option<&str>, block: &CopyBlock) -> Option<String> {
        let key = (database.map(str::to_owned), block.header.qualified_name());
        let tally = self.tallies.get_mut(&key).filter(|t| !t.said)?;
        tally.said = true;
        let mut folded = Vec::new();
        if tally.chunk_blocks > 0 {
            folded.push(format!(
                "chunks: {} block(s), {} row(s)",
                tally.chunk_blocks, tally.chunk_rows
            ));
        }
        if tally.relation_blocks > 0 {
            folded.push(format!(
                "compressed relations: {} block(s), {} row(s)",
                tally.relation_blocks, tally.relation_rows
            ));
        }
        Some(format!("{} — folded, `pgdt info --detail` lists each", folded.join("; ")))
    }
}
