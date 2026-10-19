//! A dump's databases as DataFusion catalogs: PostgreSQL schemas are
//! DataFusion schemas and tables are tables, each catalog named as
//! [`register_dump`] says.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, SchemaProvider, TableProvider};
use datafusion::common::Result;
use datafusion::prelude::SessionContext;
use pgdt_model::{DiagnosticSink, TableName};
use pgdt_query::TableSelector;

use crate::dump::PgDump;
use crate::error::Error;
use crate::report::{RefusedTable, Reporting, UncollatedColumns, report_located};
use crate::session::install;
use crate::table::PgDumpTable;

/// Where a table whose `COPY` header names no schema is listed. `pg_dump`
/// qualifies every table it writes, so only a hand-written file reaches it.
pub const UNQUALIFIED_SCHEMA: &str = "public";

/// Register `dump` in `ctx` as catalogs, returning their names in file order.
///
/// **A database the file names is a catalog of that name**, and one no
/// `\connect` names — a single-database dump taken without `--create` — takes
/// `name`. So `name` is required exactly where the file names no database, and
/// refused on a file of several databases, where it would have to be invented
/// into a prefix nobody wrote; given for a single named database, it replaces
/// that database's name. *Rejected: one registration per database, chosen by
/// a fragment of the location.*
///
/// The session is given the settings, the
/// [`ScanBudget`](crate::budget::ScanBudget) and `pgdump_unrepresentable`
/// [`ready_session`](crate::ready_session) gives a builder, each unless it
/// carries one, and `dump`'s statistics are billed to that budget.
/// `sink` hears the dump's findings and then every table's, each table named
/// `catalog.schema.table`, and is kept to hear each scan's plan notes under
/// that name; a table whose plan refuses is not listed, and is one
/// [`RefusedTable`] finding instead. Every table's columns compared bytewise
/// for want of a recorded collation are one finding, last, naming the dump
/// by its origin ([`UncollatedColumns`]).
pub fn register_dump(
    ctx: &SessionContext,
    name: Option<&str>,
    dump: &Arc<PgDump>,
    sink: Arc<dyn DiagnosticSink>,
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
    dump.bill(&install(ctx));
    dump.report(sink.as_ref());
    let mut uncollated = UncollatedColumns::of(dump.origin());
    let names = named
        .into_iter()
        .map(|(database, name)| {
            let catalog =
                PgDumpCatalog::new(dump, database.as_deref(), &name, &sink, &mut uncollated);
            ctx.register_catalog(&name, Arc::new(catalog));
            name
        })
        .collect();
    uncollated.report(sink.as_ref());
    Ok(names)
}

/// One database of a dump.
///
/// **Every table's provider is built when the catalog is**, from the map
/// alone, so `SHOW TABLES` and `information_schema`, which ask for every
/// table, find each one built and none is built twice. **A table whose plan
/// refuses is not listed**: its refusal is a [`RefusedTable`] finding the
/// sink hears instead, so a listing names what can be queried. *Rejected:
/// listing it and raising its refusal where it is asked for*, which fails every
/// listing of the session over one table; *building each table on its first
/// lookup*, which two statements planned at once build twice.
pub(crate) struct PgDumpCatalog {
    schemas: BTreeMap<String, Arc<PgDumpSchema>>,
}

impl fmt::Debug for PgDumpCatalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgDumpCatalog").field("schemas", &self.schemas.keys()).finish()
    }
}

impl PgDumpCatalog {
    /// The database `database` names in `dump` — `None` for the one no
    /// `\connect` names — with every table's provider built, each table's
    /// findings handed to `sink` about the name SQL reaches it by under
    /// `catalog` and its scans reporting there, the uncollated columns counted
    /// into `uncollated`. A table left unlisted is one [`RefusedTable`]
    /// finding, after the schema's listed tables'.
    fn new(
        dump: &Arc<PgDump>,
        database: Option<&str>,
        catalog: &str,
        sink: &Arc<dyn DiagnosticSink>,
        uncollated: &mut UncollatedColumns,
    ) -> Self {
        let mut named: BTreeMap<String, BTreeMap<String, TableName>> = BTreeMap::new();
        for table in dump.tables().iter().filter(|t| t.database.as_deref() == database) {
            let schema = table.schema.as_deref().unwrap_or(UNQUALIFIED_SCHEMA);
            named
                .entry(schema.to_string())
                .or_default()
                .entry(table.table.clone())
                .or_insert_with(|| table.clone());
        }
        let schemas = named
            .into_iter()
            .map(|(schema_name, tables)| {
                let mut schema = PgDumpSchema::default();
                let mut refused = Vec::new();
                for (table, qualified) in tables {
                    let subject = format!("{catalog}.{schema_name}.{table}");
                    let resolved = dump
                        .dump()
                        .table(&TableSelector::from(&qualified))
                        .expect("a selector stating every part selects one table at most")
                        .expect("a table the dump lists is one of its tables");
                    let reporting = Reporting { subject: subject.clone(), sink: Arc::clone(sink) };
                    match PgDumpTable::build(Arc::clone(dump), resolved, reporting, uncollated) {
                        Ok(provider) => {
                            schema.tables.insert(table, Arc::new(provider));
                        }
                        Err(error) => {
                            refused.push((subject, RefusedTable { table: qualified, error }))
                        }
                    }
                }
                for (subject, refused) in refused {
                    report_located(sink.as_ref(), &subject, refused);
                }
                (schema_name, Arc::new(schema))
            })
            .collect();
        Self { schemas }
    }
}

impl CatalogProvider for PgDumpCatalog {
    fn schema_names(&self) -> Vec<String> {
        self.schemas.keys().cloned().collect()
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        self.schemas.get(name).map(|schema| Arc::clone(schema) as Arc<dyn SchemaProvider>)
    }
}

/// One PostgreSQL schema of one database: the tables it lists.
#[derive(Default)]
struct PgDumpSchema {
    tables: BTreeMap<String, Arc<PgDumpTable>>,
}

impl fmt::Debug for PgDumpSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgDumpSchema").field("tables", &self.tables.keys()).finish()
    }
}

#[async_trait]
impl SchemaProvider for PgDumpSchema {
    fn table_names(&self) -> Vec<String> {
        self.tables.keys().cloned().collect()
    }

    async fn table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>> {
        Ok(self.tables.get(name).map(|provider| Arc::clone(provider) as Arc<dyn TableProvider>))
    }

    fn table_exist(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }
}
