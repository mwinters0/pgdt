//! One table of a dump as a DataFusion `TableProvider`, scanned as the
//! library's partitioned replay.

use std::fmt;
use std::sync::{Arc, OnceLock};

use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::{Result, Statistics};
use datafusion::datasource::TableType;
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;
use pgdt_codec::{ResolvedSchema, compare::ComparisonSemantics};
use pgdt_model::{DiagnosticSink, TableName};
use pgdt_query::{PlanNote, QueryOptions, Table, TableSelector};

use crate::budget::{BudgetAccount, ScanBudget, pool_limit};
use crate::dump::PgDump;
use crate::error::{Error, Refusal};
use crate::exec::{PgDumpExec, Replay, ScanMetrics, StaticFilter};
use crate::filter::translate;
use crate::report::{BudgetedPlanNote, Reporting, UncollatedColumns};
use crate::settings::PgDumpSettings;
use crate::statistics::{output_orderings, scanned, table_statistics};

/// One table of an opened [`PgDump`], holding the library's [`Table`] it was
/// resolved into once, when it is built. Its schema is settled then, from the
/// table's blocks and the DDL alone ([`Table::schema`]), so asking for it
/// reads nothing; a scan is the library's partitioned replay over those
/// blocks ([`Table::plan`]).
///
/// **It takes its sink when it is built** ([`PgDump::table`]): its findings
/// are reported then, and every scan's plan notes when the scan is planned,
/// each about the name SQL reaches the table by.
pub struct PgDumpTable {
    dump: Arc<PgDump>,
    table: Table,
    resolved: ResolvedSchema,
    /// What the map's statistics say about the whole table, in the schema's
    /// own column order. Read off the resident map, so it never changes; and
    /// taken on the first scan rather than here, because a catalog builds
    /// every table it lists when it is registered, and folding every block's
    /// groups is not what that should cost.
    statistics: OnceLock<Arc<Statistics>>,
    /// Where this table's findings and its scans' plan notes go.
    reporting: Reporting,
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
    /// `table`, resolved from `dump`, under the schema its plan resolves —
    /// refused as the library refuses it, bar a table the cache holds at the
    /// metadata level under a typed schema, which the library refuses too and
    /// this names the mapping pass for ([`Refusal::MetadataLevel`]); the caller
    /// spells either. Built, its findings
    /// go to `reporting`: how each column resolved, and how each column's
    /// comparison in DataFusion's semantics diverges from PostgreSQL's, bar
    /// the columns no collation is recorded for, which are counted into
    /// `uncollated` for the registration to report once.
    pub(crate) fn build(
        dump: Arc<PgDump>,
        table: Table,
        reporting: Reporting,
        uncollated: &mut UncollatedColumns,
    ) -> Result<Self, Error> {
        let options = query_options(&dump);
        let resolved = table.schema(&options).map_err(|refused| match refused {
            refused @ pgdt_core::Error::TableAtMetadataLevel { .. } => Refusal::MetadataLevel {
                refused: Box::new(refused),
                pass: dump.pass().clone(),
                table: Box::new(table.name().clone()),
            }
            .into(),
            other => Error::from(other),
        })?;
        let divergences = table.divergences(&options)?;
        for finding in resolved.columns() {
            reporting.report(finding.clone());
        }
        for finding in divergences {
            if !uncollated.count(&reporting.subject, &finding) {
                reporting.report(finding);
            }
        }
        Ok(Self { dump, table, resolved, statistics: OnceLock::new(), reporting })
    }

    /// The table this provider reads.
    pub fn name(&self) -> &TableName {
        self.table.name()
    }

    /// The schema and per-column resolution every scan of this table carries.
    pub fn resolved_schema(&self) -> &ResolvedSchema {
        &self.resolved
    }

    /// Hand what one scan's plan settled to this table's sink, each note
    /// quoting a budget wrapped with `account`, the scan's own.
    fn report_plan(&self, notes: &[PlanNote], account: &BudgetAccount) {
        for note in notes {
            if note.budget_bytes().is_some() {
                let note = note.clone();
                self.reporting.report(BudgetedPlanNote { note, account: account.clone() });
            } else {
                self.reporting.report(note.clone());
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
    /// **The one table `selector` selects, on its own** — the provider its
    /// catalog would hand out — refused where it selects none, or more than
    /// one ([`pgdt_core::Error::AmbiguousTable`]), as the dump's spelling
    /// spells it.
    ///
    /// **`sink` hears its findings now, and every scan's plan notes from
    /// then on**, each about `subject`, the name SQL reaches the table by:
    /// how each column resolved, how each column's comparison in DataFusion's
    /// semantics diverges from PostgreSQL's, the columns no collation is
    /// recorded for last, as one [`UncollatedColumns`] finding; then what the
    /// memory budget declined and what statistics let a scan skip, settled
    /// when the scan is planned, one quoting a budget wrapped in a
    /// [`BudgetedPlanNote`].
    pub fn table(
        self: &Arc<Self>,
        selector: &TableSelector,
        subject: &str,
        sink: Arc<dyn DiagnosticSink>,
    ) -> Result<Arc<PgDumpTable>, Error> {
        let spelled = |err: Error| self.speller().error(err);
        let Some(table) = self.dump().table(selector).map_err(|err| spelled(err.into()))? else {
            let refused = Refusal::Table(format!("{} holds no table `{selector}`", self.origin()));
            return Err(spelled(refused.into()));
        };
        let mut uncollated = UncollatedColumns::of(subject);
        let reporting = Reporting { subject: subject.to_string(), sink: Arc::clone(&sink) };
        let built = PgDumpTable::build(Arc::clone(self), table, reporting, &mut uncollated)
            .map_err(spelled)?;
        uncollated.report(sink.as_ref());
        Ok(Arc::new(built))
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
                    Err(err) if tests => Err(self.dump.speller().external(err)),
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
    /// built with ([`PgDump::table`]), and what statistics pruned
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
        let refused = |err| self.dump.speller().external(err);
        let partitions =
            self.table.plan(settings.read_options(), query_options).map_err(refused)?;
        self.report_plan(partitions.plan_notes(), draw.account());
        let metrics = ScanMetrics::planned(partitions.plan_notes());
        let partitions = Arc::new(partitions);
        let schema = partitions.resolved_schema().schema().clone();
        let table = self.table_statistics().map_err(refused)?;
        let kept =
            (!filters.is_empty()).then(|| (partitions.kept_rows(), partitions.kept_value_bytes()));
        let statistics = scanned(&table, projection, &schema, kept);
        let orderings = output_orderings(&schema, partitions.orders());
        let replay = Replay::new(
            partitions,
            schema,
            draw,
            metrics,
            settings.dynamic_filter_rows,
            self.dump.speller().clone(),
        );
        let answered = StaticFilter::new(answered);
        Ok(Arc::new(PgDumpExec::new(replay, orderings, limit, statistics, answered)?))
    }
}
