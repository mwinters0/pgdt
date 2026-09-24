//! A dump's databases as DataFusion catalogs: PostgreSQL schemas are
//! DataFusion schemas and tables are tables, each catalog named as
//! [`crate::register_dump`] says.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, SchemaProvider, TableProvider};
use datafusion::common::Result;
use pgdump_query::{DiagnosticSink, TableName};

use crate::dump::PgDump;
use crate::report::RefusedTable;
use crate::table::PgDumpTable;

/// Where a table whose `COPY` header names no schema is listed. `pg_dump`
/// qualifies every table it writes, so only a hand-written file reaches it.
pub const UNQUALIFIED_SCHEMA: &str = "public";

/// One database of a dump.
///
/// **Every table's provider is built when the catalog is**, from the map
/// alone, so `SHOW TABLES` and `information_schema`, which ask for every
/// table, find each one built and none is built twice. **A table whose plan
/// refuses is not listed**: its refusal is a finding [`PgDumpCatalog::report`]
/// hands the sink instead, so a listing names what can be queried. *Rejected:
/// listing it and raising its refusal where it is asked for*, which fails every
/// listing of the session over one table; *building each table on its first
/// lookup*, which two statements planned at once build twice, the sink kept
/// by a provider then replaced.
pub struct PgDumpCatalog {
    schemas: BTreeMap<String, Arc<PgDumpSchema>>,
}

impl fmt::Debug for PgDumpCatalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgDumpCatalog").field("schemas", &self.schemas.keys()).finish()
    }
}

impl PgDumpCatalog {
    /// The database `database` names in `dump` — `None` for the one no
    /// `\connect` names — with every table's provider built.
    pub fn new(dump: &Arc<PgDump>, database: Option<&str>) -> Self {
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
            .map(|(name, tables)| {
                let mut schema = PgDumpSchema::default();
                for (table, qualified) in tables {
                    match PgDumpTable::build(Arc::clone(dump), qualified.clone()) {
                        Ok(provider) => {
                            schema.tables.insert(table, Arc::new(provider));
                        }
                        Err(error) => {
                            schema.refused.insert(table, RefusedTable { table: qualified, error });
                        }
                    }
                }
                (name, Arc::new(schema))
            })
            .collect();
        Self { schemas }
    }

    /// Hand every table's findings to `sink`, each named as SQL would reach
    /// it under `catalog`, and make it the sink the listed tables' scans
    /// report to ([`PgDumpTable::report`]). A table left unlisted is one
    /// [`RefusedTable`] finding, its refusal.
    pub(crate) fn report(&self, catalog: &str, sink: &Arc<dyn DiagnosticSink>) {
        for (schema_name, schema) in &self.schemas {
            for (table, provider) in &schema.tables {
                provider.report(&format!("{catalog}.{schema_name}.{table}"), Arc::clone(sink));
            }
            for (table, refused) in &schema.refused {
                refused.report(&format!("{catalog}.{schema_name}.{table}"), sink.as_ref());
            }
        }
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

/// One PostgreSQL schema of one database: the tables it lists, and those its
/// dump holds that it does not.
#[derive(Default)]
struct PgDumpSchema {
    tables: BTreeMap<String, Arc<PgDumpTable>>,
    refused: BTreeMap<String, RefusedTable>,
}

impl fmt::Debug for PgDumpSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgDumpSchema")
            .field("tables", &self.tables.keys())
            .field("refused", &self.refused.keys())
            .finish()
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
