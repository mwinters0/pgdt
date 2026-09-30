//! One table of a dump as a DataFusion `TableProvider`, scanned as the
//! library's partitioned replay.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::stats::Precision;
use datafusion::common::{DataFusionError, Result, Statistics};
use datafusion::datasource::TableType;
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;
use pgdump_query::{
    ComparisonSemantics, QueryOptions, ResolvedSchema, SchemaMode, TableName, TablePartitions,
};

use crate::budget::{ScanBudget, pool_limit};
use crate::dump::PgDump;
use crate::exec::{PgDumpExec, Replay, ScanMetrics, StaticFilter};
use crate::pushdown::translate;
use crate::report::Reporting;
use crate::settings::PgDumpSettings;
use crate::statistics::{byte_size, output_orderings, table_statistics, total_byte_size};

/// One table of an opened [`PgDump`]. Its schema is settled when it is built,
/// from the map and the DDL alone (`pgdump_query::table_schema`), so asking for
/// it reads nothing; a scan is the library's partitioned replay over the
/// dump's complete map.
pub struct PgDumpTable {
    dump: Arc<PgDump>,
    name: TableName,
    resolved: ResolvedSchema,
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
            .field("name", &self.name)
            .finish()
    }
}

impl PgDumpTable {
    /// `name`, which must be one of `dump`'s own [`PgDump::tables`], or the
    /// refusal its plan would raise.
    pub fn new(dump: Arc<PgDump>, name: TableName) -> Result<Self> {
        Self::build(dump, name).map_err(|err| match err {
            crate::Error::Library(err) => external(err),
            err => DataFusionError::External(Box::new(err)),
        })
    }

    /// [`PgDumpTable::new`], its refusal the library's own, bar a table the
    /// cache holds at the metadata level under a typed schema, which the
    /// library refuses too and this names the parse for
    /// ([`crate::Error::MetadataLevel`]).
    pub(crate) fn build(dump: Arc<PgDump>, name: TableName) -> Result<Self, crate::Error> {
        let unmapped = dump.index().blocks_of(&name).any(|block| block.array_shapes.is_none());
        if unmapped && dump.schema_mode() == SchemaMode::Typed {
            return Err(crate::Error::MetadataLevel {
                table: crate::describe(&name),
                parse: dump.parse_at_data_level(&name),
            });
        }
        let resolved = pgdump_query::table_schema(dump.index(), &name, &query_options(&dump))?;
        Ok(Self { dump, name, resolved, statistics: OnceLock::new(), reporting: Mutex::default() })
    }

    /// The table this provider reads.
    pub fn name(&self) -> &TableName {
        &self.name
    }

    /// The schema and per-column resolution every scan of this table carries,
    /// its `notes` included.
    pub fn resolved_schema(&self) -> &ResolvedSchema {
        &self.resolved
    }

    pub(crate) fn reporting(&self) -> &Mutex<Option<Arc<Reporting>>> {
        &self.reporting
    }
}

/// What every query of `dump` asks, before a scan adds its projection,
/// parallelism and batch size: the dump's schema mode and how it reads a value
/// its column's type cannot hold, and DataFusion's comparison semantics
/// (`docs/design/decisions.md`, "D40", "D98").
fn query_options(dump: &PgDump) -> QueryOptions {
    QueryOptions {
        schema_mode: dump.schema_mode(),
        unrepresentable: dump.unrepresentable(),
        semantics: ComparisonSemantics::DataFusion,
        ..QueryOptions::default()
    }
}

/// A library error carried across DataFusion's boundary whole, so an embedder
/// can downcast it back.
pub(crate) fn external(err: pgdump_query::Error) -> DataFusionError {
    DataFusionError::External(Box::new(err))
}

#[async_trait]
impl TableProvider for PgDumpTable {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.resolved.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    /// `Exact` for a filter that translates into the library's tree and that
    /// the library's plan of this table resolves in DataFusion semantics, as the
    /// scan will ask it to; `Unsupported` for every other
    /// ([`crate::pushdown`]). Resolving is the plan's own check, over every
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
                match pgdump_query::table_schema(self.dump.index(), &self.name, &options) {
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
        let source = Arc::clone(self.dump.source());
        let budget = ScanBudget::of(state);
        // A table registered by hand reaches its session's budget only here.
        self.dump.bill(&budget);
        let settings = PgDumpSettings::of(state);
        let draw = Arc::new(budget.draw(
            state.config().target_partitions(),
            source.default_worker_memory(),
            pool_limit(state),
            settings.memory,
        ));
        let parallelism = draw.parallelism();
        let schema = self.resolved.schema.fields();
        // A filter that does not translate was not answered `Exact`, so
        // DataFusion keeps it above the scan and it is not needed here.
        let (answered, translated): (Vec<&Expr>, Vec<_>) = filters
            .iter()
            .filter_map(|filter| Some((filter, translate(filter, &self.resolved)?)))
            .unzip();
        let query_options = QueryOptions {
            projection: projection
                .map(|columns| columns.iter().map(|&i| schema[i].name().clone()).collect()),
            filter: pgdump_query::Expr::And(translated),
            parallelism,
            max_rows: state.config().batch_size(),
            ..query_options(&self.dump)
        };
        let reading = query_options.statistics_view();
        let scan_options = settings.scan_options(parallelism);
        let partitions = TablePartitions::plan(
            source,
            self.dump.index(),
            Arc::clone(self.dump.watch()),
            &self.name,
            scan_options,
            query_options,
        )
        .await
        .map_err(external)?;
        self.report_plan(partitions.plan_notes(), draw.account());
        let metrics = ScanMetrics::planned(partitions.plan_notes());
        let partitions = Arc::new(partitions);
        let schema = partitions.resolved_schema().schema;
        let mut statistics = self
            .statistics
            .get_or_init(|| {
                Arc::new(table_statistics(self.dump.index(), &self.name, &self.resolved, reading))
            })
            .as_ref()
            .clone()
            .project(projection);
        // A pushed-down filter takes rows out, so the table's counts and
        // extremes stop describing what the node emits and are handed over as
        // estimates, and its rows and each column's bytes are bounded by what
        // pruning kept, with no selectivity guessed below that
        // (`docs/design/decisions.md`, "D89", "D91"). `limit` is the plan
        // node's, and is applied there.
        if !filters.is_empty() {
            statistics = statistics.to_inexact();
            statistics.num_rows = match usize::try_from(partitions.kept_rows()) {
                Ok(rows) => Precision::Inexact(rows),
                Err(_) => Precision::Absent,
            };
            let kept_bytes = partitions.kept_value_bytes();
            for (i, column) in statistics.column_statistics.iter_mut().enumerate() {
                let value_bytes = kept_bytes.get(i).copied().flatten();
                column.byte_size =
                    byte_size(schema.field(i).data_type(), statistics.num_rows, value_bytes);
            }
        }
        // The projection keeps the whole table's total, which only the
        // projected columns' own sum describes.
        statistics.total_byte_size = total_byte_size(&statistics.column_statistics);
        let orderings = output_orderings(&schema, partitions.orders());
        let replay = Replay::new(partitions, schema, draw, metrics, settings.dynamic_filter_rows);
        let answered = StaticFilter::new(answered);
        Ok(Arc::new(PgDumpExec::new(replay, orderings, limit, statistics, answered)?))
    }
}
