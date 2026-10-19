//! One table of a dump as a DataFusion `TableProvider`, scanned as the
//! library's partitioned replay.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::{Result, Statistics};
use datafusion::datasource::TableType;
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;
use pgdt_codec::{ResolvedSchema, SchemaMode, compare::ComparisonSemantics};
use pgdt_model::{DiagnosticSink, TableName};
use pgdt_query::{ComparisonNote, PlanNote, QueryOptions, Table, TableSelector};

use crate::budget::{BudgetAccount, ScanBudget, pool_limit};
use crate::dump::PgDump;
use crate::error::{Error, external};
use crate::exec::{PgDumpExec, Replay, ScanMetrics, StaticFilter};
use crate::filter::translate;
use crate::report::{BudgetedPlanNote, Located, Reporting, UncollatedColumns};
use crate::settings::PgDumpSettings;
use crate::statistics::{output_orderings, scanned, table_statistics};

/// One table of an opened [`PgDump`], holding the library's [`Table`] it was
/// resolved into once, when it is built. Its schema is settled then, from the
/// table's blocks and the DDL alone ([`Table::schema`]), so asking for it
/// reads nothing; a scan is the library's partitioned replay over those
/// blocks ([`Table::plan`]).
pub struct PgDumpTable {
    dump: Arc<PgDump>,
    table: Table,
    resolved: ResolvedSchema,
    /// How each column's comparison in DataFusion's semantics diverges from
    /// PostgreSQL's, settled with the schema and reported with it.
    divergences: Vec<ComparisonNote>,
    /// What the map's statistics say about the whole table, in the schema's
    /// own column order. Read off the resident map, so it never changes; and
    /// taken on the first scan rather than here, because a catalog builds
    /// every table it lists when it is registered, and folding every block's
    /// groups is not what that should cost.
    statistics: OnceLock<Arc<Statistics>>,
    /// Where this table's scans report their plans, once it has been
    /// reported ([`PgDumpTable::report`]).
    reporting: Mutex<Option<Arc<Reporting>>>,
}

impl fmt::Debug for PgDumpTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgDumpTable")
            .field("dump", &self.dump.origin())
            .field("name", self.table.name())
            .finish()
    }
}

impl PgDumpTable {
    /// `name`, which must be one of `dump`'s own [`PgDump::tables`], or the
    /// refusal its plan would raise, an [`Error`] across DataFusion's
    /// boundary as every refusal of ours is.
    pub fn new(dump: Arc<PgDump>, name: TableName) -> Result<Self> {
        Ok(Self::build(dump, name)?)
    }

    /// [`PgDumpTable::new`], its refusal the library's own, bar a table the
    /// cache holds at the metadata level under a typed schema, which the
    /// library refuses too and this names the parse for
    /// ([`Error::MetadataLevel`]).
    pub(crate) fn build(dump: Arc<PgDump>, name: TableName) -> Result<Self, Error> {
        let table = dump
            .dump()
            .table(&TableSelector::from(name))
            .expect("a selector stating every part selects one table at most")
            .expect("a table registered over a dump is one of its tables");
        let unmapped = table.blocks().iter().any(|block| block.census.is_none());
        if unmapped && dump.schema_mode() == SchemaMode::Typed {
            return Err(Error::MetadataLevel {
                table: describe(table.name()),
                parse: dump.parse_at_data_level(table.name()),
            });
        }
        let options = query_options(&dump);
        let resolved = table.schema(&options)?;
        let divergences = table.divergences(&options)?;
        Ok(Self {
            dump,
            table,
            resolved,
            divergences,
            statistics: OnceLock::new(),
            reporting: Mutex::default(),
        })
    }

    /// The table this provider reads.
    pub fn name(&self) -> &TableName {
        self.table.name()
    }

    /// The schema and per-column resolution every scan of this table carries,
    /// its `notes` included.
    pub fn resolved_schema(&self) -> &ResolvedSchema {
        &self.resolved
    }

    /// Hand this table's findings to `sink`, each prefixed with `subject` —
    /// the name SQL reaches the table by: how each column resolved, and how
    /// each column's comparison in DataFusion's semantics diverges from
    /// PostgreSQL's, the columns no collation is recorded for last, as one
    /// [`UncollatedColumns`] finding.
    ///
    /// **`sink` is kept, and hears every scan's plan notes from then on**
    /// (`pgdt_query::PlanNote`), under the same `subject`: what the memory
    /// budget declined and what statistics let the scan skip, settled when
    /// the scan is planned, one quoting a budget wrapped in a
    /// [`BudgetedPlanNote`]. A table reported again reports its scans to the
    /// latest sink; one never reported reports no scan.
    pub fn report(&self, subject: &str, sink: Arc<dyn DiagnosticSink>) {
        let mut uncollated = UncollatedColumns::of(subject);
        self.report_counting(subject, Arc::clone(&sink), &mut uncollated);
        uncollated.report(sink.as_ref());
    }

    /// [`Self::report`], the uncollated columns counted into `uncollated`
    /// rather than reported, a registration of several tables reporting
    /// them once.
    pub(crate) fn report_counting(
        &self,
        subject: &str,
        sink: Arc<dyn DiagnosticSink>,
        uncollated: &mut UncollatedColumns,
    ) {
        let resolved = self.resolved_schema();
        for finding in resolved.notes() {
            sink.report(&Located { subject, finding });
        }
        for finding in &self.divergences {
            if !uncollated.count(subject, finding) {
                sink.report(&Located { subject, finding });
            }
        }
        let reporting = Reporting { subject: subject.to_string(), sink };
        *self.reporting.lock().unwrap() = Some(Arc::new(reporting));
    }

    /// Hand what one scan's plan settled to the sink this table was reported
    /// to, if it was, each note quoting a budget wrapped with `account`, the
    /// scan's own.
    pub(crate) fn report_plan(&self, notes: &[PlanNote], account: &BudgetAccount) {
        // Taken out of the lock, so a sink that reports this table again
        // does not wait on itself.
        let Some(reporting) = self.reporting.lock().unwrap().clone() else { return };
        let subject = reporting.subject.as_str();
        for note in notes {
            if note.budget_bytes().is_some() {
                let finding = &BudgetedPlanNote { note: note.clone(), account: account.clone() };
                reporting.sink.report(&Located { subject, finding });
            } else {
                reporting.sink.report(&Located { subject, finding: note });
            }
        }
    }

    /// What the map's statistics say about the whole table, in the schema it
    /// was built with: taken on the first scan, and kept.
    fn table_statistics(&self) -> pgdt_core::Result<Arc<Statistics>> {
        if let Some(statistics) = self.statistics.get() {
            return Ok(Arc::clone(statistics));
        }
        let summary = self.table.summary(&query_options(&self.dump))?;
        let statistics = Arc::new(table_statistics(&summary, &self.resolved));
        Ok(Arc::clone(self.statistics.get_or_init(|| statistics)))
    }
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
        let mut selector = TableSelector::named(table);
        if let Some(schema) = schema {
            selector = selector.in_schema(schema);
        }
        if let Some(database) = database {
            selector = selector.in_database(database);
        }
        let candidates: Vec<&TableName> =
            self.tables().iter().filter(|t| selector.selects(t)).collect();
        match candidates.as_slice() {
            [one] => Ok(Arc::new(PgDumpTable::build(Arc::clone(self), (*one).clone())?)),
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
        Some(database) => format!("{} in database {database}", table.qualified_name()),
        None => table.qualified_name(),
    }
}

/// What every query of `dump` asks, before a scan adds its projection,
/// parallelism and batch size: the dump's schema mode, how it reads a value
/// its column's type cannot hold and one PostgreSQL refuses, and DataFusion's
/// comparison semantics (`docs/design/decisions.md`, "D40", "D98", "D103").
fn query_options(dump: &PgDump) -> QueryOptions {
    QueryOptions {
        schema_mode: dump.schema_mode(),
        unrepresentable: dump.unrepresentable(),
        postgres_invalid_values: dump.postgres_invalid_values(),
        semantics: ComparisonSemantics::DataFusion,
        ..QueryOptions::default()
    }
}

#[async_trait]
impl TableProvider for PgDumpTable {
    fn schema(&self) -> SchemaRef {
        Arc::clone(self.resolved.schema())
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    /// `Exact` for a filter that translates into the library's tree and that
    /// the library's plan of this table resolves in DataFusion semantics, as the
    /// scan will ask it to; `Unsupported` for every other
    /// (`crate::filter::pushdown`). Resolving is the plan's own check, over every
    /// block, and reads no byte of the dump.
    ///
    /// **A filter holding `pgdump_unrepresentable` that translates and does
    /// not resolve is the plan's refusal**, since no other node can answer it
    /// (`docs/design/decisions.md`, "D101").
    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>> {
        filters
            .iter()
            .map(|filter| {
                let Some(translated) = translate(filter, &self.resolved) else {
                    return Ok(TableProviderFilterPushDown::Unsupported);
                };
                let tests = translated.tests_unrepresentable();
                let options = QueryOptions { filter: translated, ..query_options(&self.dump) };
                match self.table.schema(&options) {
                    Ok(_) => Ok(TableProviderFilterPushDown::Exact),
                    Err(err) if tests => Err(external(err)),
                    Err(_) => Ok(TableProviderFilterPushDown::Unsupported),
                }
            })
            .collect()
    }

    /// The replay planned now, against the session's `target_partitions` and
    /// what the session's [`ScanBudget`] leaves, under the `pgdump.` settings
    /// the session states now ([`PgDumpSettings`]); the partitions are streamed
    /// when DataFusion runs them. The projection is the library's by name, so
    /// an unprojected column is never decoded, and `filters` are the ones
    /// [`Self::supports_filters_pushdown`] answered `Exact`, evaluated by the
    /// library as one conjunction. `limit` is left to the plan, which stops
    /// pulling once it is met, after the filter. A column the map proves every
    /// partition emits in order is declared that ordering
    /// ([`output_orderings`]), so a sort it satisfies is not planned.
    ///
    /// **What the plan settled is reported here**, to the sink the table was
    /// registered with ([`PgDumpTable::report`]), and what statistics pruned
    /// is the plan node's metric ([`ScanMetrics`]).
    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let source = self.table.source();
        let budget = ScanBudget::of(state);
        // A table registered by hand reaches its session's budget only here.
        self.dump.bill(&budget);
        let settings = PgDumpSettings::of(state);
        // deficiency: KD112 — the count asked for is the session's
        // `target_partitions`, the core count where nothing states one, on any
        // device: a plain source's recommendation of one
        // (`docs/design/decisions.md`, "D2") is never read, so on a device one
        // reader already saturates the split buys nothing, and on a rotational
        // one it scatters reads the record calls fatal there
        // (`docs/design/measurements.md`, "The SATA SSD is not a parallel
        // regime"), unread on a plain file. **(c) unowned**; promoted by a
        // reading of the split cold on the HDD, the fix a choice between
        // `pgdt sql` stating the source's recommendation for a plain dump and
        // leaving the count to the session, rotational detection being
        // unavailable in a container ("D10").
        let draw = Arc::new(budget.draw(
            state.config().target_partitions(),
            source.default_worker_memory(),
            pool_limit(state),
            settings.memory,
        ));
        let parallelism = draw.parallelism();
        let schema = self.resolved.schema().fields();
        // A filter that does not translate was not answered `Exact`, so
        // DataFusion keeps it above the scan and it is not needed here.
        let (answered, translated): (Vec<&Expr>, Vec<_>) = filters
            .iter()
            .filter_map(|filter| Some((filter, translate(filter, &self.resolved)?)))
            .unzip();
        let query_options = QueryOptions {
            projection: projection
                .map(|columns| columns.iter().map(|&i| schema[i].name().clone()).collect()),
            filter: pgdt_query::Expr::And(translated),
            parallelism,
            ..query_options(&self.dump)
        };
        let partitions =
            self.table.plan(settings.read_options(), query_options).map_err(external)?;
        self.report_plan(partitions.plan_notes(), draw.account());
        let metrics = ScanMetrics::planned(partitions.plan_notes());
        let partitions = Arc::new(partitions);
        let schema = partitions.resolved_schema().schema().clone();
        let table = self.table_statistics().map_err(external)?;
        let kept =
            (!filters.is_empty()).then(|| (partitions.kept_rows(), partitions.kept_value_bytes()));
        let statistics = scanned(&table, projection, &schema, kept);
        let orderings = output_orderings(&schema, partitions.orders());
        let replay = Replay::new(partitions, schema, draw, metrics, settings.dynamic_filter_rows);
        let answered = StaticFilter::new(answered);
        Ok(Arc::new(PgDumpExec::new(replay, orderings, limit, statistics, answered)?))
    }
}
