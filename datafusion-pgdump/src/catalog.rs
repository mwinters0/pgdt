//! A dump's databases as DataFusion catalogs: PostgreSQL schemas are
//! DataFusion schemas and tables are tables
//! (`docs/design/roadmap-P6-datafusion.md`, "How a dump appears in SQL").

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, SchemaProvider, TableProvider};
use datafusion::common::Result;
use pgdump_query::TableName;

use crate::dump::PgDump;
use crate::table::PgDumpTable;

/// Where a table whose `COPY` header names no schema is listed. `pg_dump`
/// qualifies every table it writes, so only a hand-written file reaches it.
pub const UNQUALIFIED_SCHEMA: &str = "public";

/// One database of a dump.
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
    /// `\connect` names.
    pub fn new(dump: &Arc<PgDump>, database: Option<&str>) -> Self {
        let mut schemas: BTreeMap<String, BTreeMap<String, TableName>> = BTreeMap::new();
        for table in dump.tables().iter().filter(|t| t.database.as_deref() == database) {
            let schema = table.schema.as_deref().unwrap_or(UNQUALIFIED_SCHEMA);
            schemas
                .entry(schema.to_string())
                .or_default()
                .entry(table.table.clone())
                .or_insert_with(|| table.clone());
        }
        let schemas = schemas
            .into_iter()
            .map(|(name, tables)| {
                let schema =
                    PgDumpSchema { dump: Arc::clone(dump), tables, providers: Mutex::default() };
                (name, Arc::new(schema))
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

/// One PostgreSQL schema of one database.
///
/// **`table` is cheap after its first call**: `SHOW TABLES` and
/// `information_schema` ask it of every table, so each provider is built once,
/// from the map alone, and kept.
struct PgDumpSchema {
    dump: Arc<PgDump>,
    tables: BTreeMap<String, TableName>,
    providers: Mutex<BTreeMap<String, Arc<dyn TableProvider>>>,
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
        let Some(table) = self.tables.get(name) else { return Ok(None) };
        if let Some(provider) = self.providers.lock().unwrap().get(name) {
            return Ok(Some(Arc::clone(provider)));
        }
        let provider: Arc<dyn TableProvider> =
            Arc::new(PgDumpTable::new(Arc::clone(&self.dump), table.clone())?);
        self.providers.lock().unwrap().insert(name.to_string(), Arc::clone(&provider));
        Ok(Some(provider))
    }

    fn table_exist(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }
}
