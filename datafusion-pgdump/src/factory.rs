//! `CREATE EXTERNAL TABLE … STORED AS PGDUMP`: one table of a dump registered
//! on its own, over the same [`PgDump::table`] a catalog hands out.
//!
//! ```sql
//! CREATE EXTERNAL TABLE build STORED AS PGDUMP LOCATION 'koji.dump'
//!     OPTIONS ('pgdump.table' 'build', 'pgdump.schema' 'public');
//! ```

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider, TableProviderFactory};
use datafusion::common::config::{ConfigEntry, ConfigExtension, ExtensionOptions};
use datafusion::common::{DataFusionError, Result, plan_err};
use datafusion::logical_expr::CreateExternalTable;
use datafusion::prelude::SessionContext;
use pgdump_query::cache::StrictIdentity;
use pgdump_query::{DiagnosticSink, SchemaMode, UnrepresentableMode};

use crate::budget::ScanBudget;
use crate::dump::{PgDump, PgDumpOptions};
use crate::session_budget;

/// The word after `STORED AS`, as DataFusion keys a table factory by it.
pub const PGDUMP_FILE_TYPE: &str = "PGDUMP";

/// The `pgdump.` options `CREATE EXTERNAL TABLE … STORED AS PGDUMP` takes.
///
/// A table options extension because `datafusion-cli` refuses an option key
/// whose namespace no registered extension claims, before the factory sees
/// it; the factory reads the same keys through [`ExtensionOptions::set`], so
/// a session that does not validate them refuses an unknown one all the same.
///
/// **The table's name is three options, not one dotted string**, so a
/// PostgreSQL name holding a `.` needs no quoting rule of ours.
#[derive(Debug, Clone, Default)]
pub struct PgDumpTableOptions {
    /// `pgdump.table`: the table, which is required.
    pub table: Option<String>,
    /// `pgdump.schema`: its PostgreSQL schema, where the name alone matches
    /// the table in more than one.
    pub schema: Option<String>,
    /// `pgdump.database`: its database, where the file holds several.
    pub database: Option<String>,
    /// `pgdump.schema_mode`: `typed`, the default, or `strings`, every column
    /// as the file's text — the escape hatch from a wrong type mapping.
    pub schema_mode: SchemaMode,
    /// `pgdump.unrepresentable`: `null`, the default, reading a value the
    /// column's type cannot hold as NULL, `text`, reading a column holding
    /// one as its text, or `refuse` ([`PgDumpOptions::unrepresentable`]).
    pub unrepresentable: UnrepresentableMode,
    /// `pgdump.strict_identity`: which identity signals bind for this dump,
    /// in `pgdt --strict-identity`'s grammar. Unstated, the factory's.
    pub strict_identity: Option<StrictIdentity>,
}

impl ConfigExtension for PgDumpTableOptions {
    const PREFIX: &'static str = "pgdump";
}

impl ExtensionOptions for PgDumpTableOptions {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn cloned(&self) -> Box<dyn ExtensionOptions> {
        Box::new(self.clone())
    }

    /// `key` is whole, `pgdump.` included, as `TableOptions::set` hands it
    /// over.
    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let field = key.strip_prefix("pgdump.").unwrap_or(key);
        match field {
            "table" => self.table = Some(value.to_string()),
            "schema" => self.schema = Some(value.to_string()),
            "database" => self.database = Some(value.to_string()),
            "schema_mode" => {
                self.schema_mode = match value.to_ascii_lowercase().as_str() {
                    "typed" => SchemaMode::Typed,
                    "strings" => SchemaMode::Strings,
                    _ => {
                        return plan_err!(
                            "pgdump.schema_mode is `typed` or `strings`, not `{value}`"
                        );
                    }
                }
            }
            "unrepresentable" => {
                self.unrepresentable = match value.to_ascii_lowercase().as_str() {
                    "null" => UnrepresentableMode::Null,
                    "text" => UnrepresentableMode::Text,
                    "refuse" => UnrepresentableMode::Refuse,
                    _ => {
                        return plan_err!(
                            "pgdump.unrepresentable is `null`, `text` or `refuse`, not `{value}`"
                        );
                    }
                }
            }
            "strict_identity" => match value.parse() {
                Ok(strict) => self.strict_identity = Some(strict),
                Err(why) => return plan_err!("pgdump.strict_identity `{value}`: {why}"),
            },
            _ => {
                return plan_err!(
                    "`{key}` is not a PGDUMP option — the options are pgdump.table, \
                     pgdump.schema, pgdump.database, pgdump.schema_mode, \
                     pgdump.unrepresentable and pgdump.strict_identity"
                );
            }
        }
        Ok(())
    }

    fn entries(&self) -> Vec<ConfigEntry> {
        let entry = |key: &str, value: Option<String>, description| ConfigEntry {
            key: format!("pgdump.{key}"),
            value,
            description,
        };
        let mode = match self.schema_mode {
            SchemaMode::Typed => "typed",
            SchemaMode::Strings => "strings",
        };
        let unrepresentable = match self.unrepresentable {
            UnrepresentableMode::Null => "null",
            UnrepresentableMode::Text => "text",
            UnrepresentableMode::Refuse => "refuse",
        };
        vec![
            entry("table", self.table.clone(), "The table to register; required."),
            entry("schema", self.schema.clone(), "The table's PostgreSQL schema."),
            entry("database", self.database.clone(), "The table's database."),
            entry(
                "schema_mode",
                Some(mode.to_string()),
                "`typed`, or `strings` for every column as the file's text.",
            ),
            entry(
                "unrepresentable",
                Some(unrepresentable.to_string()),
                "`null`, a value its column's type cannot hold read as NULL, or `refuse`.",
            ),
            entry(
                "strict_identity",
                self.strict_identity.map(|strict| strict.to_string()),
                "`time`, `location`, both, `advisory` or `none`; unset, the session's.",
            ),
        ]
    }
}

/// Builds a [`crate::PgDumpTable`] for `CREATE EXTERNAL TABLE … STORED AS
/// PGDUMP`: the dump at `LOCATION` opened through its complete cache
/// ([`PgDump::open`]), the table its options name, and what the dump and the
/// table find reported to the sink, then what each scan's plan settles, the
/// table named as the statement names it.
///
/// **The table's columns are the dump's**, so a statement declaring columns,
/// partition columns or an order is refused rather than believed.
///
/// **A dump is opened under the [`StrictIdentity`] its statement states**,
/// `pgdump.strict_identity`, and the factory's where it states none: which
/// signals a source can state is a fact about that source, so one unpinnable
/// remote dump read under `none` leaves every other dump of the session its
/// in-flight check. *Rejected: `SET pgdump.strict_identity`*, a setting that
/// would bind once, at `CREATE` — the source every scan of the dump shares is
/// watched from its open — where every other `pgdump.` setting binds at each
/// scan's plan.
pub struct PgDumpTableFactory {
    sink: Arc<dyn DiagnosticSink>,
    strict_identity: StrictIdentity,
}

impl std::fmt::Debug for PgDumpTableFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgDumpTableFactory").finish_non_exhaustive()
    }
}

impl PgDumpTableFactory {
    /// A factory opening a dump that states no strictness at
    /// [`StrictIdentity::ADVISORY`].
    pub fn new(sink: Arc<dyn DiagnosticSink>) -> Self {
        Self { sink, strict_identity: StrictIdentity::ADVISORY }
    }

    /// Open a dump that states no strictness under `strict` instead
    /// ([`PgDumpOptions::strict_identity`]).
    pub fn with_strict_identity(self, strict: StrictIdentity) -> Self {
        Self { strict_identity: strict, ..self }
    }
}

#[async_trait]
impl TableProviderFactory for PgDumpTableFactory {
    async fn create(
        &self,
        state: &dyn Session,
        cmd: &CreateExternalTable,
    ) -> Result<Arc<dyn TableProvider>> {
        let [location] = cmd.locations.as_slice() else {
            return plan_err!("a PGDUMP table reads one dump, not {}", cmd.locations.len());
        };
        if !cmd.schema.fields().is_empty() {
            return plan_err!("a PGDUMP table's columns are the dump's; declare none");
        }
        if !cmd.table_partition_cols.is_empty() || !cmd.order_exprs.is_empty() {
            return plan_err!("a PGDUMP table takes no PARTITIONED BY and no WITH ORDER");
        }
        let mut options = PgDumpTableOptions::default();
        for (key, value) in &cmd.options {
            options.set(key, value)?;
        }
        let Some(table) = options.table.as_deref() else {
            return plan_err!("a PGDUMP table needs OPTIONS ('pgdump.table' '<name>')");
        };
        let open = PgDumpOptions {
            schema_mode: options.schema_mode,
            unrepresentable: options.unrepresentable,
            strict_identity: options.strict_identity.unwrap_or(self.strict_identity),
            ..PgDumpOptions::default()
        };
        let dump = PgDump::open(location, open).await.map_err(external)?;
        dump.bill(&ScanBudget::of(state));
        dump.report(self.sink.as_ref());
        let provider = dump
            .table(options.database.as_deref(), options.schema.as_deref(), table)
            .map_err(external)?;
        provider.report(&cmd.name.to_string(), Arc::clone(&self.sink));
        Ok(provider)
    }
}

/// Let `ctx` run `CREATE EXTERNAL TABLE … STORED AS PGDUMP`, reporting what
/// each registration finds to `sink` and opening a dump whose statement states
/// no `pgdump.strict_identity` under `strict_identity`, the session's: the
/// factory keyed [`PGDUMP_FILE_TYPE`], the [`PgDumpTableOptions`] extension,
/// and the session's [`crate::ScanBudget`] and [`crate::PgDumpSettings`]
/// installed as [`crate::register_dump`] installs them, so every table the
/// statement registers draws on the one budget and `SET pgdump.…` reaches
/// every scan; and `pgdump_unrepresentable`, registered as
/// [`crate::register_dump`] registers it.
pub fn register_table_factory(
    ctx: &SessionContext,
    sink: Arc<dyn DiagnosticSink>,
    strict_identity: StrictIdentity,
) {
    ctx.register_table_options_extension(PgDumpTableOptions::default());
    session_budget(ctx);
    crate::unrepresentable::register(ctx);
    ctx.state_ref().write().table_factories_mut().insert(
        PGDUMP_FILE_TYPE.to_string(),
        Arc::new(PgDumpTableFactory::new(sink).with_strict_identity(strict_identity)),
    );
}

fn external(err: crate::Error) -> DataFusionError {
    DataFusionError::External(Box::new(err))
}
