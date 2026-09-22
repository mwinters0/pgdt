//! Query a `pg_dump` file's tables from Apache DataFusion.
//!
//! A dump is opened through the complete cache `pgdt parse` leaves
//! ([`PgDump::open`]) and registered as one catalog per database
//! ([`register_dump`]), its PostgreSQL schemas DataFusion schemas and its
//! tables tables; one table can also be had on its own ([`PgDump::table`]).
//! A scan is the library's partitioned replay, one DataFusion partition per
//! sub-stream, with the projection pushed into the library by name, a filter
//! pushed `Exact` wherever the library answers it as DataFusion would, the batch
//! size the session's, and the worker count the session's `target_partitions`
//! lowered to what the session's [`ScanBudget`] affords.
//!
//! The design is `docs/design/roadmap-P6-datafusion.md`.

mod budget;
mod catalog;
mod dump;
mod pushdown;
mod table;

use std::sync::Arc;

use datafusion::prelude::SessionContext;
use pgdump_query::TableName;

pub use budget::ScanBudget;
pub use catalog::{PgDumpCatalog, UNQUALIFIED_SCHEMA};
pub use dump::{PgDump, PgDumpOptions};
pub use table::PgDumpTable;

/// What opening or registering a dump can refuse.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The library's own refusal: a source it cannot read, a cache for
    /// another file, a table whose blocks disagree.
    #[error(transparent)]
    Library(#[from] pgdump_query::Error),
    /// The cache this provider reads does not cover the whole file, or is not
    /// there to read. The provider never maps, so the remedy is the parse
    /// named.
    #[error("{why}; the provider reads only a complete cache — run `{parse}` to build it")]
    CacheNotComplete { parse: String, why: String },
    /// A catalog name is needed and nothing supplies one, or one was supplied
    /// where the file already names every database.
    #[error("{0}")]
    CatalogName(String),
    /// The table asked for is not in the dump, or its name alone matches more
    /// than one.
    #[error("{0}")]
    Table(String),
    #[error(transparent)]
    DataFusion(#[from] datafusion::error::DataFusionError),
}

/// Register `dump` in `ctx` as catalogs, returning their names in file order.
///
/// **A database the file names is a catalog of that name**, and one no
/// `\connect` names — a single-database dump taken without `--create` — takes
/// `name`. So `name` is required exactly where the file names no database, and
/// refused on a file of several databases, where it would have to be invented
/// into a prefix nobody wrote; given for a single named database, it replaces
/// that database's name.
///
/// The session gains a [`ScanBudget`] discovered from the process's allowance,
/// unless it already carries one, and `dump`'s statistics are billed to it.
pub fn register_dump(
    ctx: &SessionContext,
    name: Option<&str>,
    dump: &Arc<PgDump>,
) -> Result<Vec<String>, Error> {
    let databases = dump.databases();
    let named: Vec<(Option<String>, String)> = match (databases.as_slice(), name) {
        ([database], Some(name)) => vec![(database.clone(), name.to_string())],
        ([Some(database)], None) => vec![(Some(database.clone()), database.clone())],
        ([], Some(name)) => vec![(None, name.to_string())],
        ([None] | [], None) => {
            return Err(Error::CatalogName(format!(
                "{} names no database, so its catalog needs a name — give one",
                dump.origin()
            )));
        }
        (_, Some(name)) => {
            return Err(Error::CatalogName(format!(
                "{} holds {} databases, each registered under its own name; `{name}` would name \
                 none of them — leave the name out",
                dump.origin(),
                databases.len()
            )));
        }
        (_, None) => {
            let mut named = Vec::new();
            for database in databases {
                let Some(catalog) = database.clone() else {
                    return Err(Error::CatalogName(format!(
                        "{} holds several databases and one of them has no name, so no catalog \
                         name can be taken from the file for it",
                        dump.origin()
                    )));
                };
                named.push((database, catalog));
            }
            named
        }
    };
    let budget = {
        let state = ctx.state_ref();
        let mut state = state.write();
        match state.config().get_extension::<ScanBudget>() {
            Some(budget) => budget,
            None => {
                let budget = Arc::new(ScanBudget::discover());
                state.config_mut().set_extension(Arc::clone(&budget));
                budget
            }
        }
    };
    dump.bill(&budget);
    Ok(named
        .into_iter()
        .map(|(database, catalog)| {
            ctx.register_catalog(&catalog, Arc::new(PgDumpCatalog::new(dump, database.as_deref())));
            catalog
        })
        .collect())
}

impl PgDump {
    /// One table on its own, the provider its catalog would hand out.
    ///
    /// `schema` left out matches the table in any schema, and `database` left
    /// out matches it in any database; a name that then matches more than one
    /// table is refused, naming them.
    pub fn table(
        self: &Arc<Self>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Arc<PgDumpTable>, Error> {
        let candidates: Vec<&TableName> = self
            .tables()
            .iter()
            .filter(|t| database.is_none_or(|d| t.database.as_deref() == Some(d)))
            .filter(|t| schema.is_none_or(|s| t.schema.as_deref() == Some(s)))
            .filter(|t| t.table == table)
            .collect();
        match candidates.as_slice() {
            [one] => Ok(Arc::new(PgDumpTable::new(Arc::clone(self), (*one).clone())?)),
            [] => Err(Error::Table(format!("{} holds no table `{table}`", self.origin()))),
            many => Err(Error::Table(format!(
                "`{table}` names {} tables in {}: {}; name the schema or the database",
                many.len(),
                self.origin(),
                many.iter().map(|t| describe(t)).collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

/// A table as a refusal names it: `schema.table`, and its database where it
/// has one.
fn describe(table: &TableName) -> String {
    match &table.database {
        Some(database) => format!("{} in database {database}", table.qualified()),
        None => table.qualified(),
    }
}
