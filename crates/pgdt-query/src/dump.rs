//! A dump read through its complete map: the source, the map and the watch of
//! one open, held together so they cannot come from different ones, the
//! tables the map holds, and one of them resolved by a selector into the
//! [`Table`] an embedder plans its reads over
//! (`docs/design/decisions.md`, "D90").

use std::fmt;
use std::sync::Arc;

use pgdt_codec::ResolvedSchema;
use pgdt_core::{Error, Result};
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::{Held, Opened, SourceWatch};
use pgdt_io::ByteRangeSource;
use pgdt_mapping::MapRun;
use pgdt_model::{DumpIndex, TableName};

use crate::options::QueryOptions;
use crate::predicate::{ComparisonNote, column_divergences};
use crate::stream::{TablePartitions, table_schema};
use crate::summary::{TableSummary, table_summary};

/// A dump and **a map of the whole of it**, complete by construction: built
/// from an open whose cache reaches the file's end ([`Dump::opened`]), or
/// from a mapping pass run to it ([`Dump::mapped`]). It owns the source, the
/// map and the watch of that one open, so every plan over it reads the source
/// the map describes and checks it against the baseline the map was believed
/// under.
///
/// **It never maps and never writes.** A table's schema, its summary and its
/// plan are read off the map and the DDL alone; only a plan's streams read
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
                let (scanned_through, size) = (loaded.index.scanned_through, loaded.total_size);
                return Err(Error::MapIncomplete { scanned_through, size });
            }
            Held::Missing | Held::Disabled | Held::Replacing(_) => {
                return Err(Error::MapIncomplete {
                    scanned_through: 0,
                    size: source.size().await?,
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
        let size = run.source.size().await?;
        if !run.index.is_complete(size) {
            return Err(Error::MapIncomplete { scanned_through: run.index.scanned_through, size });
        }
        Ok(Self::new(run.source, run.watch, run.index))
    }

    fn new(source: Arc<dyn ByteRangeSource>, watch: SourceWatch, index: DumpIndex) -> Self {
        let tables = index.tables();
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
    /// block appears.
    pub fn tables(&self) -> &[TableName] {
        &self.tables
    }

    /// The databases the file holds: each `\connect`ed database by name, in
    /// file order, then `None` for one no `\connect` names — a plain
    /// single-database dump taken without `--create` — where it holds a
    /// table, one holding none yielding no entry. The stretch of a
    /// `pg_dumpall` file before its first `\connect`, which holds roles and
    /// no table, is not one.
    pub fn databases(&self) -> Vec<Option<String>> {
        let mut names: Vec<Option<String>> = Vec::new();
        let declared = self
            .index
            .metadata
            .iter()
            .flat_map(|m| &m.databases)
            .map(|db| &db.name)
            .filter(|name| name.is_some());
        let holding = self.tables.iter().map(|t| &t.database);
        for name in declared.chain(holding) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// The one table `selector` selects ([`TableSelector::selects`]), `None`
    /// where it selects none, and [`Error::AmbiguousTable`] where it selects
    /// more than one, naming each (`docs/design/decisions.md`, "D49").
    pub fn table(&self, selector: &TableSelector) -> Result<Option<Table<'_>>> {
        let mut selected = self.tables.iter().filter(|name| selector.selects(name));
        let Some(name) = selected.next() else { return Ok(None) };
        let rest: Vec<&TableName> = selected.collect();
        if !rest.is_empty() {
            return Err(Error::AmbiguousTable {
                name: selector.to_string(),
                candidates: std::iter::once(name).chain(rest).map(render_candidate).collect(),
            });
        }
        Ok(Some(Table { dump: self, name }))
    }
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
        name.table == self.table
            && self.schema.as_ref().is_none_or(|schema| *schema == name.schema)
            && self.database.as_ref().is_none_or(|database| *database == name.database)
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
/// plan under a query's options, each read off the dump's complete map and
/// none reading a byte of the dump.
#[derive(Clone, Copy)]
pub struct Table<'a> {
    dump: &'a Dump,
    name: &'a TableName,
}

impl fmt::Debug for Table<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Table").field("name", self.name).finish_non_exhaustive()
    }
}

impl<'a> Table<'a> {
    /// The table, exactly.
    pub fn name(&self) -> &'a TableName {
        self.name
    }

    /// The schema a query of this table under `options` resolves, projection
    /// included: the schema every batch of its plan carries, or the refusal
    /// its plan would raise — a table whose blocks name different column
    /// sets, a projected name no block carries, a typed query of a table the
    /// map holds at the metadata level ([`Error::TableAtMetadataLevel`]) —
    /// but for the refuse mode's, which only planning raises, and a projection
    /// naming a column twice, which only planning refuses: here the column
    /// comes back twice.
    pub fn schema(&self, options: &QueryOptions) -> Result<ResolvedSchema> {
        table_schema(&self.dump.index, self.name, options)
    }

    /// What the map's statistics say about the whole table, against the
    /// schema `options` resolves, in the semantics its comparisons answer in
    /// and in the view it reads a value its column's type cannot hold in
    /// ([`QueryOptions::statistics_view`]), or that schema's refusal
    /// (`docs/design/decisions.md`, "D89").
    pub fn summary(&self, options: &QueryOptions) -> Result<TableSummary> {
        let resolved = self.schema(options)?;
        let reading = options.statistics_view();
        Ok(table_summary(&self.dump.index, self.name, &resolved, options.semantics, reading))
    }

    /// How each column's comparison in `options`' semantics diverges from
    /// PostgreSQL's, against the schema `options` resolves, or that schema's
    /// refusal (`docs/design/decisions.md`, "D59").
    pub fn divergences(&self, options: &QueryOptions) -> Result<Vec<ComparisonNote>> {
        Ok(column_divergences(&self.schema(options)?, options.semantics))
    }

    /// The partitioned replay of this table under `options`, each piece read
    /// with `read` ([`TablePartitions`]): planned now, reading nothing, its
    /// partitions streamed when the caller runs them. Refuses what a query's
    /// plan refuses, before a byte is read, and a source the dump's watch
    /// cannot check while it is read (`docs/design/decisions.md`, "D21").
    pub fn plan(&self, read: ReadOptions, options: QueryOptions) -> Result<TablePartitions> {
        let dump = self.dump;
        TablePartitions::new(
            Arc::clone(&dump.source),
            Arc::clone(&dump.watch),
            &dump.index,
            self.name,
            read,
            options,
        )
    }
}

#[cfg(test)]
mod tests;
