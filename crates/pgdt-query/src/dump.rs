//! A dump read through its complete map: the source, the map and the watch of
//! one open, held together so they cannot come from different ones, the
//! tables the map holds, and one of them resolved by a selector into the
//! [`Table`] an embedder plans its reads over
//! (`docs/design/decisions.md`, "D90") — or a cold query's one table, mapped
//! only until it is settled ([`Table::settle`]).

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use pgdt_codec::ResolvedSchema;
use pgdt_core::{Error, Result};
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::{Held, Opened, SourceWatch};
use pgdt_io::{Arrangement, ByteRangeSource};
use pgdt_mapping::MapRun;
use pgdt_model::{CopyBlock, DumpIndex, DumpMetadata, TableName};

use crate::options::{Inheritance, QueryOptions};
use crate::predicate::{ComparisonNote, column_divergences};
use crate::stream::{TablePartitions, table_line_charge, table_schema};
use crate::summary::{TableSummary, table_summary};

mod descendants;
mod hypertables;
mod settle;

use self::descendants::{Refusals, TableRead, listed_tables, table_read};
use self::hypertables::ReadNotes;

/// A dump and **a map of the whole of it**, complete by construction: built
/// from an open whose cache reaches the file's end ([`Dump::opened`]), or
/// from a mapping pass run to it ([`Dump::mapped`]). It owns the source, the
/// map and the watch of that one open, so every plan over it reads the source
/// the map describes and checks it against the baseline the map was believed
/// under.
///
/// **It never maps and never writes.** A table's schema, its summary and its
/// plan are read off its blocks and the DDL alone; only a plan's streams read
/// the source, and those read rows ([`Table::plan`]).
pub struct Dump {
    source: Arc<dyn ByteRangeSource>,
    watch: Arc<SourceWatch>,
    index: DumpIndex,
    tables: Vec<TableName>,
}

impl fmt::Debug for Dump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dump")
            .field("tables", &self.tables)
            .field("watch", &self.watch)
            .finish_non_exhaustive()
    }
}

impl Dump {
    /// The dump `opened` opened, over the map its cache holds — refused,
    /// [`Error::MapIncomplete`], where that map stops short of the file's end
    /// or there is none: no cache at the path, the cache disabled, or one
    /// being replaced. Building the rest is a mapping pass's
    /// ([`Dump::mapped`]); a dump never scans.
    pub async fn opened(opened: Opened) -> Result<Self> {
        let Opened { source, watch, cache, .. } = opened;
        let index = match cache {
            Held::Loaded(loaded) if loaded.is_complete() => loaded.index,
            Held::Loaded(loaded) => {
                let (scanned_through, size) = (loaded.index.scanned_through, loaded.extent.exact());
                return Err(Error::MapIncomplete { scanned_through, size });
            }
            Held::Missing | Held::Disabled | Held::Replacing(_) => {
                return Err(Error::MapIncomplete {
                    scanned_through: 0,
                    size: source.extent().await?.exact(),
                });
            }
        };
        Ok(Self::new(source, watch, index))
    }

    /// The dump `run` mapped, over the map it built — refused,
    /// [`Error::MapIncomplete`], where it stopped short of the file's end
    /// ([`MapRun::interrupted`]). An interrupt inside the back-fill leaves the
    /// map whole, and so builds one.
    pub async fn mapped(run: MapRun) -> Result<Self> {
        let extent = run.source.extent().await?;
        if !extent.reached_by(run.index.scanned_through) {
            return Err(Error::MapIncomplete {
                scanned_through: run.index.scanned_through,
                size: extent.exact(),
            });
        }
        Ok(Self::new(run.source, run.watch, run.index))
    }

    fn new(source: Arc<dyn ByteRangeSource>, watch: SourceWatch, index: DumpIndex) -> Self {
        let tables = listed_tables(&index);
        Self { source, watch: Arc::new(watch), index, tables }
    }

    /// The source the map describes, which every plan's streams read.
    pub fn source(&self) -> &Arc<dyn ByteRangeSource> {
        &self.source
    }

    /// The whole map: its spans, its DDL, its statistics and its file-level
    /// diagnostics.
    pub fn index(&self) -> &DumpIndex {
        &self.index
    }

    /// Every table the dump holds rows for, each once, in the order its first
    /// block, or its first descendant's, appears: each a block holds rows
    /// for, and each table one of those descends from — a partitioned table,
    /// which holds none of its own, included (`docs/design/decisions.md`,
    /// "D131").
    pub fn tables(&self) -> &[TableName] {
        &self.tables
    }

    /// The databases the file holds ([`DumpIndex::databases`]): each
    /// `\connect`ed database by name, in file order, then `None` for one no
    /// `\connect` names — a plain single-database dump taken without
    /// `--create` — where a block is in it.
    pub fn databases(&self) -> Vec<Option<String>> {
        self.index.databases().into_iter().map(|database| database.map(str::to_owned)).collect()
    }

    /// The one table `selector` selects ([`TableSelector::selects`]), holding
    /// the blocks a read of it reads — its own and every descendant's — and
    /// sharing this dump's source, watch and DDL ([`Table`]);
    /// `None` where it selects none, and [`Error::AmbiguousTable`] where it
    /// selects more than one, naming each (`docs/design/decisions.md`,
    /// "D49").
    pub fn table(&self, selector: &TableSelector) -> Result<Option<Table>> {
        let Some(name) = select(&self.tables, selector)? else { return Ok(None) };
        let TableRead { blocks, through, refused, notes } = table_read(&self.index, name);
        Ok(Some(Table {
            source: Arc::clone(&self.source),
            watch: Arc::clone(&self.watch),
            metadata: self.index.metadata.clone(),
            name: name.clone(),
            blocks,
            through,
            refused,
            notes,
        }))
    }
}

/// The one of `tables` `selector` selects, `None` where it selects none, and
/// [`Error::AmbiguousTable`] naming each where it selects more than one —
/// over a complete map's tables, or the tables a cold query's pass reached.
fn select<'a>(tables: &'a [TableName], selector: &TableSelector) -> Result<Option<&'a TableName>> {
    let mut selected = tables.iter().filter(|name| selector.selects(name));
    let Some(name) = selected.next() else { return Ok(None) };
    let rest: Vec<&TableName> = selected.collect();
    if !rest.is_empty() {
        return Err(Error::AmbiguousTable {
            name: selector.to_string(),
            candidates: std::iter::once(name).chain(rest).map(render_candidate).collect(),
        });
    }
    Ok(Some(name))
}

/// A table as [`Error::AmbiguousTable`] names a candidate: `database.schema.table`,
/// or `schema.table` where no `\connect` names its database.
fn render_candidate(name: &TableName) -> String {
    match &name.database {
        Some(database) => format!("{database}.{}", name.qualified_name()),
        None => name.qualified_name(),
    }
}

/// Which table of a [`Dump`] a caller means: a table's name, and as much of
/// its schema and database as the caller states — each part left unstated
/// selecting a table whatever it holds there. Built from a [`TableName`], it
/// states every part, so it selects that table alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSelector {
    /// `None` selects any database; `Some(None)` the one no `\connect` names.
    database: Option<Option<String>>,
    /// `None` selects any schema; `Some(None)` a name no schema qualifies.
    schema: Option<Option<String>>,
    table: String,
}

impl TableSelector {
    /// The table named `table`, in any schema of any database.
    pub fn named(table: impl Into<String>) -> Self {
        Self { database: None, schema: None, table: table.into() }
    }

    /// This selector, narrowed to tables in `schema`.
    pub fn in_schema(self, schema: impl Into<String>) -> Self {
        Self { schema: Some(Some(schema.into())), ..self }
    }

    /// This selector, narrowed to tables in the database `\connect` names
    /// `database`.
    pub fn in_database(self, database: impl Into<String>) -> Self {
        Self { database: Some(Some(database.into())), ..self }
    }

    /// Whether `name` is a table this selector selects: its table name, and
    /// every part the selector states.
    pub fn selects(&self, name: &TableName) -> bool {
        self.selects_block(name.database.as_deref(), name.schema.as_deref(), &name.table)
    }

    /// Whether a block naming `schema.table`, in `database`, holds rows for a
    /// table this selector selects — what a cold query's pass stops for,
    /// asked of each block as it is mapped.
    fn selects_block(&self, database: Option<&str>, schema: Option<&str>, table: &str) -> bool {
        table == self.table
            && self.schema.as_ref().is_none_or(|stated| stated.as_deref() == schema)
            && self.database.as_ref().is_none_or(|stated| stated.as_deref() == database)
    }
}

impl From<&TableName> for TableSelector {
    fn from(name: &TableName) -> Self {
        Self {
            database: Some(name.database.clone()),
            schema: Some(name.schema.clone()),
            table: name.table.clone(),
        }
    }
}

impl From<TableName> for TableSelector {
    fn from(name: TableName) -> Self {
        Self { database: Some(name.database), schema: Some(name.schema), table: name.table }
    }
}

/// `schema.table` where the schema is stated, and `table` where it is not —
/// the name a refusal quotes back.
impl fmt::Display for TableSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.schema {
            Some(Some(schema)) => write!(f, "{schema}.{}", self.table),
            _ => f.write_str(&self.table),
        }
    }
}

/// One table of a [`Dump`] ([`Dump::table`]): its schema, its summary and its
/// plan under a query's options, each read off the table's blocks and the
/// dump's DDL and none reading a byte of the dump.
///
/// **It reads its descendants**: its rows are its own and every table's that
/// inherits from it or is its partition, at any depth, each projected to its
/// columns, as `SELECT … FROM <table>` reads them on the server
/// (`docs/design/decisions.md`, "D131") — or its own alone, as `SELECT …
/// FROM ONLY <table>` does, where a query's
/// [`QueryOptions::inheritance`] says so. It holds every block either mode
/// reads, so one table is read under both (`docs/design/decisions.md`,
/// "D132").
///
/// **It owns what it plans over**: the dump's source, watch and DDL shared by
/// `Arc`, and its blocks — every block of the table and of its descendants
/// in the complete map it was resolved from, cloned once, their statistics
/// shared with that map.
/// So it outlives the [`Dump`] that resolved it, and a plan made from it
/// reads the source the blocks describe, checked against the baseline the
/// map was believed under. It is the one `'static` type a complete map and a
/// cold pass both yield, so an adapter holds it; a borrowing `Table<'a>`
/// would need a second type for the cold pass, or a second owner lending it.
pub struct Table {
    source: Arc<dyn ByteRangeSource>,
    watch: Arc<SourceWatch>,
    metadata: Option<Arc<DumpMetadata>>,
    name: TableName,
    blocks: Vec<CopyBlock>,
    /// The blocks written through a partition root above it holding its
    /// descendants' rows, which a read of it alone settles its columns from
    /// where it holds no other.
    through: Vec<CopyBlock>,
    /// Why a read of it refuses under each mode, where one does.
    refused: Refusals,
    /// What a read of it hears of TimescaleDB's catalog under each mode,
    /// raised by its plan ([`Table::plan`]).
    notes: ReadNotes,
}

impl fmt::Debug for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Table")
            .field("name", &self.name)
            .field("blocks", &self.blocks.len())
            .finish_non_exhaustive()
    }
}

impl Table {
    /// The table, exactly.
    pub fn name(&self) -> &TableName {
        &self.name
    }

    /// Every block a read of the table reads, its descendants' included, in
    /// file order: what it holds, whichever mode a query reads it in.
    pub fn blocks(&self) -> &[CopyBlock] {
        &self.blocks
    }

    /// The source the table's blocks describe, which every plan's streams
    /// read.
    pub fn source(&self) -> &Arc<dyn ByteRangeSource> {
        &self.source
    }

    /// The schema a query of this table under `options` resolves, projection
    /// included: the schema every batch of its plan carries, or the refusal
    /// its plan would raise — a table whose blocks name different column
    /// sets, a projected name no block carries, a typed query of a table the
    /// map holds at the metadata level ([`Error::TableAtMetadataLevel`]), a
    /// table whose rows are written through its partition root
    /// ([`Error::PartitionThroughRoot`]), one reaching a TimescaleDB chunk
    /// whose rows are compressed ([`Error::CompressedChunks`]) or a possible
    /// hypertable whose catalog does not read
    /// ([`Error::TimescaleCatalogUnreadable`]) — but for the refuse mode's, which
    /// only planning raises, and a projection naming a column twice, which
    /// only planning refuses: here the column comes back twice.
    ///
    /// **A table read alone that holds no block of its own** — a
    /// partitioned table — reads no row, under the columns the blocks
    /// holding its descendants' rows name: those it holds, or with none,
    /// those written through its root (`docs/design/decisions.md`, "D132").
    pub fn schema(&self, options: &QueryOptions) -> Result<ResolvedSchema> {
        let read = self.read(options)?;
        self.schema_over(self.columns_from(&read), options)
    }

    /// The schema a query of this table under `options` resolves, its
    /// columns settled from `blocks`.
    fn schema_over(&self, blocks: &[CopyBlock], options: &QueryOptions) -> Result<ResolvedSchema> {
        let database = self.name.database.as_deref();
        let table = self.name.qualified_name();
        table_schema(database, &table, blocks, self.metadata.as_deref(), options)
    }

    /// **The blocks a query under `options` reads**: every one under
    /// [`Inheritance::Descendants`], and under [`Inheritance::Only`] those
    /// holding the table's own rows — naming it, and not as the partition
    /// root its partitions' rows were written through (I93) — or the
    /// table's refusal under that mode.
    fn read(&self, options: &QueryOptions) -> Result<Cow<'_, [CopyBlock]>> {
        let table = self.name.qualified_name();
        let own = |block: &CopyBlock| {
            block.header.qualified_name() == table && block.partition_root.is_none()
        };
        if let Some(refused) = self.refused.of(&self.name, options.inheritance) {
            return Err(refused);
        }
        Ok(match options.inheritance {
            Inheritance::Only if !self.blocks.iter().all(own) => {
                Cow::Owned(self.blocks.iter().filter(|block| own(block)).cloned().collect())
            }
            Inheritance::Descendants | Inheritance::Only => Cow::Borrowed(&self.blocks),
        })
    }

    /// **The blocks the table's columns are settled from**, for a read of
    /// `read`: `read`, or where it reads none — a partitioned table read
    /// alone — every block it holds, or with none, those written through its
    /// root (`docs/design/decisions.md`, "D132").
    fn columns_from<'a>(&'a self, read: &'a [CopyBlock]) -> &'a [CopyBlock] {
        match (read.is_empty(), self.blocks.is_empty()) {
            (false, _) => read,
            (true, false) => &self.blocks,
            (true, true) => &self.through,
        }
    }

    /// What the map's statistics say about the whole table, against the
    /// schema `options` resolves, in the semantics its comparisons answer in
    /// and in the view it reads a value its column's type cannot hold in
    /// ([`QueryOptions::statistics_view`]), or that schema's refusal
    /// (`docs/design/decisions.md`, "D89").
    pub fn summary(&self, options: &QueryOptions) -> Result<TableSummary> {
        let read = self.read(options)?;
        let resolved = self.schema_over(self.columns_from(&read), options)?;
        let (database, metadata) = (self.name.database.as_deref(), self.metadata.as_deref());
        let (semantics, reading) = (options.semantics, options.statistics_view());
        Ok(table_summary(database, &read, metadata, &resolved, semantics, reading))
    }

    /// How each column's comparison in `options`' semantics diverges from
    /// PostgreSQL's, against the schema `options` resolves, or that schema's
    /// refusal (`docs/design/decisions.md`, "D59").
    pub fn divergences(&self, options: &QueryOptions) -> Result<Vec<ComparisonNote>> {
        Ok(column_divergences(&self.schema(options)?, options.semantics))
    }

    /// **The charge of the longest line a query of this table under
    /// `options` will hold**, each piece read with `read`: the longest line of
    /// the blocks whose rows it holds, as the map records it
    /// ([`BlockRecord::longest_line`](pgdt_model::BlockRecord::longest_line)) —
    /// never a block whose every row group its filter prunes, nor one whose
    /// batch and filter read no field, its long rows streamed — and what the
    /// query's batch decodes beside it, its one factor; or nothing where that
    /// line fits its chunk, no reader holding it past its chunk term. What a
    /// caller hands
    /// [`ArrangementRequest::line_charge`](pgdt_io::ArrangementRequest::line_charge)
    /// before resolving the arrangement it plans under ([`Self::plan`]), so
    /// that line is admitted without waiting on the readers carved beside it
    /// (`docs/design/decisions.md`, "D157", "D160"). Reads no byte of the dump;
    /// refuses what [`Self::schema`] refuses over the table's own blocks, and
    /// answers zero for a table reading none of its own.
    pub fn line_charge(&self, read: ReadOptions, options: &QueryOptions) -> Result<u64> {
        let blocks = self.read(options)?;
        if blocks.is_empty() {
            return Ok(0);
        }
        let database = self.name.database.as_deref();
        let table = self.name.qualified_name();
        let metadata = self.metadata.as_deref();
        table_line_charge(database, &table, &blocks, metadata, options, read.chunk())
    }

    /// The partitioned replay of this table under `options`, each piece read
    /// with `read` ([`TablePartitions`]), cut as `arrangement`'s parallelism
    /// allows and its held lines drawing on `arrangement`'s ledger — the
    /// replay's own, carved around the charge [`Self::line_charge`] answers,
    /// as a mapping pass takes its arrangement whole
    /// (`docs/design/decisions.md`, "D83", "D157"): planned now, reading
    /// nothing, its partitions streamed when the caller runs them, its notes led by what
    /// the read hears of TimescaleDB's catalog — a hypertable's root's own
    /// rows and absent chunks, or a read unchecked for compression
    /// (`docs/design/decisions.md`, "D135"). Refuses what a query's
    /// plan refuses, before a byte is read, a source the dump's watch
    /// cannot check while it is read (`docs/design/decisions.md`, "D21"),
    /// and a read whose longest line passes `arrangement`'s floor, one
    /// reader being unable to hold it (`docs/design/decisions.md`, "D161").
    pub fn plan(
        &self,
        read: ReadOptions,
        options: QueryOptions,
        arrangement: &Arrangement,
    ) -> Result<TablePartitions> {
        let inheritance = options.inheritance;
        let blocks = self.read(&options)?;
        // A table read alone holding no block of its own reads nothing, under
        // the schema [`Self::schema`] reports.
        let columns = self.columns_from(&blocks);
        let schema = match blocks.is_empty() && !columns.is_empty() {
            true => Some(self.schema_over(columns, &options)?),
            false => None,
        };
        let partitions = TablePartitions::new(
            Arc::clone(&self.source),
            Arc::clone(&self.watch),
            self.metadata.clone(),
            &self.name,
            &blocks,
            read,
            options,
            arrangement,
        )?;
        let partitions = partitions.noting(self.notes.of(inheritance));
        Ok(match schema {
            Some(schema) => partitions.reporting(schema),
            None => partitions,
        })
    }
}

#[cfg(test)]
mod tests;
