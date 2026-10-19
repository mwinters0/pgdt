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
use datafusion::common::{Result, plan_err};
use datafusion::logical_expr::CreateExternalTable;
use datafusion::prelude::SessionContext;
use pgdt_dump::cache::StrictIdentity;
use pgdt_model::{DiagnosticSink, PostgresInvalidValues};
use pgdt_query::TableSelector;

use crate::budget::ScanBudget;
use crate::dump::{PgDump, PgDumpOptions};
use crate::error::Spelling;
use crate::session::install;

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
/// PostgreSQL name holding a `.` needs no quoting rule of ours. **The rest are
/// a [`PgDumpOptions`]'s**, the modes a dump is opened under, each read as
/// the library reads its words.
#[derive(Debug, Clone, Default)]
pub(crate) struct TableOptions {
    /// `pgdump.table`: the table, which is required.
    table: Option<String>,
    /// `pgdump.schema`: its PostgreSQL schema, where the name alone matches
    /// the table in more than one.
    schema: Option<String>,
    /// `pgdump.database`: its database, where the file holds several.
    database: Option<String>,
    /// `pgdump.schema_mode`, `pgdump.unrepresentable` and
    /// `pgdump.postgres_invalid_values`: the dump's modes, each its default
    /// unstated.
    open: PgDumpOptions,
    /// `pgdump.strict_identity`: which identity signals bind for this dump,
    /// in `pgdt --strict-identity`'s grammar. Unstated, the factory's.
    strict_identity: Option<StrictIdentity>,
}

impl ConfigExtension for TableOptions {
    const PREFIX: &'static str = "pgdump";
}

impl ExtensionOptions for TableOptions {
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
            "schema_mode" => match value.parse() {
                Ok(mode) => self.open.schema_mode = mode,
                Err(why) => return plan_err!("pgdump.schema_mode: {why}"),
            },
            "unrepresentable" => match value.parse() {
                Ok(mode) => self.open.unrepresentable = mode,
                Err(why) => return plan_err!("pgdump.unrepresentable: {why}"),
            },
            "postgres_invalid_values" => {
                match value.parse().and_then(PostgresInvalidValues::queryable) {
                    Ok(mode) => self.open.postgres_invalid_values = mode,
                    Err(why) => return plan_err!("pgdump.postgres_invalid_values: {why}"),
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
                     pgdump.unrepresentable, pgdump.postgres_invalid_values and \
                     pgdump.strict_identity"
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
        let invalid = match self.open.postgres_invalid_values {
            // `set` refuses `Strict`, a parse's mode, and a scan reads under
            // one stated on the field as under `Default`.
            PostgresInvalidValues::Strict => PostgresInvalidValues::Default,
            mode => mode,
        };
        vec![
            entry("table", self.table.clone(), "The table to register; required."),
            entry("schema", self.schema.clone(), "The table's PostgreSQL schema."),
            entry("database", self.database.clone(), "The table's database."),
            entry(
                "schema_mode",
                Some(self.open.schema_mode.to_string()),
                "`typed`, or `strings` for every column as the file's text.",
            ),
            entry(
                "unrepresentable",
                Some(self.open.unrepresentable.to_string()),
                "`null`, a value its column's type cannot hold read as NULL, `text`, a column holding one read as its text, or `refuse`.",
            ),
            entry(
                "postgres_invalid_values",
                Some(invalid.to_string()),
                "`default`, a value PostgreSQL refuses for its column's type refused, or `ignore`, a float past its type's range read as the largest of its sign and one below it as zero, a `varchar(n)` or `char(n)` value past its length as written, and a NULL in a `NOT NULL` column as NULL.",
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
///
/// **Every refusal of a dump it opens is spelled by the spelling it was
/// registered with** ([`Spelling`]), a front end's words for a statement,
/// which need not be its words for a dump it registers another way.
pub(crate) struct PgDumpTableFactory {
    sink: Arc<dyn DiagnosticSink>,
    strict_identity: StrictIdentity,
    spelling: Option<Arc<dyn Spelling>>,
}

impl std::fmt::Debug for PgDumpTableFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgDumpTableFactory").finish_non_exhaustive()
    }
}

impl PgDumpTableFactory {
    /// A factory reporting to `sink`, opening a dump that states no
    /// strictness under `strict_identity` ([`PgDumpOptions::strict_identity`])
    /// and spelling its refusals with `spelling` ([`PgDumpOptions::spelling`]).
    fn new(
        sink: Arc<dyn DiagnosticSink>,
        strict_identity: StrictIdentity,
        spelling: Option<Arc<dyn Spelling>>,
    ) -> Self {
        Self { sink, strict_identity, spelling }
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
        let mut options = TableOptions::default();
        for (key, value) in &cmd.options {
            options.set(key, value)?;
        }
        let Some(table) = options.table.take() else {
            return plan_err!("a PGDUMP table needs OPTIONS ('pgdump.table' '<name>')");
        };
        let mut selector = TableSelector::named(table);
        if let Some(schema) = options.schema.take() {
            selector = selector.in_schema(schema);
        }
        if let Some(database) = options.database.take() {
            selector = selector.in_database(database);
        }
        let open = PgDumpOptions {
            strict_identity: options.strict_identity.unwrap_or(self.strict_identity),
            spelling: self.spelling.clone(),
            ..options.open
        };
        let dump = PgDump::open(location, open).await?;
        dump.bill(&ScanBudget::of(state));
        dump.report(self.sink.as_ref());
        Ok(dump.table(&selector, &cmd.name.to_string(), Arc::clone(&self.sink))?)
    }
}

/// Let `ctx` run `CREATE EXTERNAL TABLE … STORED AS PGDUMP`, reporting what
/// each registration finds to `sink`, opening a dump whose statement states
/// no `pgdump.strict_identity` under `strict_identity`, the session's, and
/// spelling every refusal of a dump it opens with `spelling`, the front end's
/// words for a statement's remedies ([`Spelling`]), where it hands one: the
/// factory keyed [`PGDUMP_FILE_TYPE`] and the table options extension naming
/// its `pgdump.` options, the session given what
/// [`crate::catalog::register_dump`] gives it, so every table a statement
/// registers draws on the one budget and `SET pgdump.…` reaches every scan.
pub fn register_table_factory(
    ctx: &SessionContext,
    sink: Arc<dyn DiagnosticSink>,
    strict_identity: StrictIdentity,
    spelling: Option<Arc<dyn Spelling>>,
) {
    ctx.register_table_options_extension(TableOptions::default());
    install(ctx);
    ctx.state_ref().write().table_factories_mut().insert(
        PGDUMP_FILE_TYPE.to_string(),
        Arc::new(PgDumpTableFactory::new(sink, strict_identity, spelling)),
    );
}

#[cfg(test)]
mod tests {
    use pgdt_codec::{SchemaMode, UnrepresentableMode};

    use super::*;

    /// **`pgdump.unrepresentable`'s and `pgdump.postgres_invalid_values`'
    /// help names every value `set` accepts**, each one set and read back
    /// before its name is looked for, so the help cannot drop a mode the
    /// option takes; another value is refused.
    #[test]
    fn a_mode_option_s_help_names_every_mode_it_takes() {
        for (key, modes) in [
            ("pgdump.unrepresentable", &["null", "text", "refuse"][..]),
            ("pgdump.postgres_invalid_values", &["default", "ignore"][..]),
        ] {
            for mode in modes {
                let mut options = TableOptions::default();
                options.set(key, mode).unwrap();
                let entries = options.entries();
                let entry = entries.iter().find(|e| e.key == key).unwrap();
                assert_eq!(entry.value.as_deref(), Some(*mode));
                assert!(entry.description.contains(&format!("`{mode}`")), "{}", entry.description);
            }
            assert!(TableOptions::default().set(key, "strict").is_err(), "{key}");
        }
    }

    /// **A mode option reads its library type's words in any case**, and
    /// `pgdump.postgres_invalid_values` refuses a parse's `strict`, in any
    /// case, with the library's one sentence for it.
    #[test]
    fn a_mode_option_reads_its_words_in_any_case() {
        let mut options = TableOptions::default();
        options.set("pgdump.schema_mode", "Strings").unwrap();
        options.set("pgdump.unrepresentable", "REFUSE").unwrap();
        options.set("pgdump.postgres_invalid_values", "Ignore").unwrap();
        assert_eq!(
            (
                options.open.schema_mode,
                options.open.unrepresentable,
                options.open.postgres_invalid_values
            ),
            (SchemaMode::Strings, UnrepresentableMode::Refuse, PostgresInvalidValues::Ignore)
        );
        let why = PostgresInvalidValues::Strict.queryable().unwrap_err();
        for strict in ["strict", "Strict"] {
            let err = options.set("pgdump.postgres_invalid_values", strict).unwrap_err();
            assert!(err.to_string().contains(&why), "{err}");
        }
    }
}
