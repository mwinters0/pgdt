//! The scan's plan node: a `StreamingTableExec` over the replay's
//! sub-streams, carrying what the dump's map says about the table.
//!
//! **It exists to answer `statistics_from_inputs`, and to carry the scan's
//! own metrics.** DataFusion reads a source's statistics off the
//! `ExecutionPlan` — `TableProvider` has no statistics method in 55 — and
//! `StreamingTableExec` answers `Statistics::new_unknown`. So the streaming
//! exec stays, held as an implementation detail rather than as a child: this
//! is a leaf, and nothing in a plan tree sees the node inside it. Its metrics
//! are reported beside the streaming exec's own ([`ScanMetrics`]).
//!
//! **It holds the dynamic filters pushed to it** ([`DynamicFilters`]), and
//! consumes none: each is answered `No`, printed in `EXPLAIN`, and visited
//! where DataFusion looks for a filter's consumer.
//!
//! **Its `EXPLAIN` line prints the whole filter the scan holds as one
//! `predicate=`**, as Parquet's scan does: the static filter it answered
//! `Exact` ([`StaticFilter`]), then each dynamic filter. No other node of a
//! physical plan names the static one. Parquet's `pruning_predicate=` is not
//! copied: it exists because Parquet prunes by something other than its
//! `predicate=`, where this scan prunes by the static filter it prints.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use datafusion::common::stats::Precision;
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{Result, Statistics, internal_err};
use datafusion::config::ConfigOptions;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::expr_rewriter::unnormalize_col;
use datafusion::logical_expr::{BinaryExpr, Expr, Operator};
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::execution_plan::{ChildrenPropertiesMode, ReplaceChildrenOptions};
use datafusion::physical_plan::filter_pushdown::{
    ChildPushdownResult, FilterPushdownPhase, FilterPushdownPropagation, PushedDown,
};
use datafusion::physical_plan::metrics::{
    Count, ExecutionPlanMetricsSet, MetricBuilder, MetricCategory, MetricType, MetricValue,
    MetricsSet, PruningMetrics,
};
use datafusion::physical_plan::statistics::{ChildStats, StatisticsArgs};
use datafusion::physical_plan::streaming::StreamingTableExec;
use datafusion::physical_plan::{DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties};
use futures::{Stream, StreamExt};
use pgdump_query::{PlanNote, PlanNoteKind, TableStream};

use crate::dynamic_filter::DynamicFilters;

/// The replay of one table, with its statistics.
#[derive(Debug, Clone)]
pub(crate) struct PgDumpExec {
    /// The streaming exec this node *is*, minus the statistics. Held rather
    /// than made a child, so the optimizer sees one leaf and the node it
    /// would otherwise replace cannot be swapped out from under the
    /// statistics.
    inner: Arc<StreamingTableExec>,
    statistics: Arc<Statistics>,
    metrics: ScanMetrics,
    /// The filter the replay `inner` streams applies, as `EXPLAIN` prints it.
    static_filter: Arc<StaticFilter>,
    dynamic_filters: DynamicFilters,
}

impl PgDumpExec {
    pub(crate) fn new(
        inner: StreamingTableExec,
        statistics: Statistics,
        metrics: ScanMetrics,
        static_filter: StaticFilter,
    ) -> Self {
        Self {
            inner: Arc::new(inner),
            statistics: Arc::new(statistics),
            metrics,
            static_filter: Arc::new(static_filter),
            dynamic_filters: DynamicFilters::default(),
        }
    }

    /// `, predicate=…` or, in the tree rendering, a `predicate=…` line of its
    /// own: the static filter, then the dynamic filters as DataFusion writes
    /// each in this view. Nothing where the scan holds no filter at all.
    fn fmt_predicate(&self, f: &mut fmt::Formatter, tree: bool) -> fmt::Result {
        let answered = !self.static_filter.is_empty();
        let held = !self.dynamic_filters.is_empty();
        if !answered && !held {
            return Ok(());
        }
        f.write_str(if tree { "\npredicate=" } else { ", predicate=" })?;
        if answered {
            self.static_filter.fmt_beside(f, held)?;
        }
        if answered && held {
            f.write_str(" AND ")?;
        }
        match (held, tree) {
            (false, _) => Ok(()),
            (true, false) => write!(f, "{}", self.dynamic_filters),
            (true, true) => write!(f, "{}", self.dynamic_filters.sql()),
        }
    }
}

impl DisplayAs for PgDumpExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "PgDumpExec: partitions={}", self.inner.partitions().len())?;
                if let Some(fetch) = self.inner.limit() {
                    write!(f, ", fetch={fetch}")?;
                }
                self.fmt_predicate(f, false)
            }
            DisplayFormatType::TreeRender => {
                self.inner.fmt_as(t, f)?;
                self.fmt_predicate(f, true)
            }
        }
    }
}

/// The filters `scan()` answered `Exact`, which the library applies as one
/// conjunction, kept to be printed.
///
/// **Written by bare column name, in DataFusion's SQL form** — as
/// `Expr::human_display` writes each part, the column's qualifier dropped
/// first — rather than by index as a physical expression is: a static filter
/// may name a column the scan's projection drops, so no index into its output
/// reaches it. `human_display` parenthesises nothing, so an `AND`, `OR` or
/// `NOT` inside another is written here, parenthesised where precedence needs
/// it as DataFusion's physical `BinaryExpr` is.
#[derive(Debug)]
pub(crate) struct StaticFilter(Vec<Expr>);

impl StaticFilter {
    pub(crate) fn new<'a>(answered: impl IntoIterator<Item = &'a Expr>) -> Self {
        Self(answered.into_iter().map(|filter| unnormalize_col(filter.clone())).collect())
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The parts as the one conjunction they are, with `more` parts of it
    /// written after them or not: a part is parenthesised as a conjunct only
    /// where it has one beside it.
    fn fmt_beside(&self, f: &mut fmt::Formatter<'_>, more: bool) -> fmt::Result {
        let above = if more || self.0.len() > 1 { Operator::And.precedence() } else { 0 };
        for (i, filter) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" AND ")?;
            }
            write_sql(f, filter, above)?;
        }
        Ok(())
    }
}

/// `expr` as a part of an expression whose operator binds at `above`,
/// parenthesised where its own binds looser.
fn write_sql(f: &mut fmt::Formatter<'_>, expr: &Expr, above: u8) -> fmt::Result {
    match expr {
        Expr::BinaryExpr(BinaryExpr { left, op, right }) => {
            let own = op.precedence();
            let parenthesised = own < above;
            if parenthesised {
                f.write_str("(")?;
            }
            write_sql(f, left, own)?;
            write!(f, " {op} ")?;
            write_sql(f, right, own)?;
            if parenthesised {
                f.write_str(")")?;
            }
            Ok(())
        }
        // `NOT` binds tighter than `AND` and looser than a comparison.
        Expr::Not(inner) => {
            f.write_str("NOT ")?;
            write_sql(f, inner, Operator::And.precedence() + 1)
        }
        _ => write!(f, "{}", expr.human_display()),
    }
}

impl ExecutionPlan for PgDumpExec {
    fn name(&self) -> &str {
        "PgDumpExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.inner.properties()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    /// The dynamic filters held: a hash join computes its filter only once
    /// it finds it in its probe side this way, and an aggregate keeps its
    /// own only where some node below visits it.
    fn apply_expressions(
        &self,
        f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> Result<TreeNodeRecursion>,
    ) -> Result<TreeNodeRecursion> {
        self.dynamic_filters.apply(f)
    }

    fn replace_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
        _: ReplaceChildrenOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if children.is_empty() {
            Ok(self)
        } else {
            internal_err!("PgDumpExec is a leaf and has no children to replace")
        }
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.replace_children(
            children,
            ReplaceChildrenOptions::new(ChildrenPropertiesMode::Recompute),
        )
    }

    /// **Every filter the `Post` phase pushes is held and answered `No`**:
    /// each is a dynamic filter a producer above re-checks its own rows
    /// against. A `Pre` phase filter is a static one, which the provider
    /// already answers where it can (`crate::pushdown`), and is not held.
    fn handle_child_pushdown_result(
        &self,
        phase: FilterPushdownPhase,
        child_pushdown_result: ChildPushdownResult,
        _config: &ConfigOptions,
    ) -> Result<FilterPushdownPropagation<Arc<dyn ExecutionPlan>>> {
        let pushed: Vec<_> =
            child_pushdown_result.parent_filters.into_iter().map(|f| f.filter).collect();
        let declined = FilterPushdownPropagation::with_parent_pushdown_result(vec![
            PushedDown::No;
            pushed.len()
        ]);
        if phase != FilterPushdownPhase::Post {
            return Ok(declined);
        }
        Ok(match self.dynamic_filters.with(pushed) {
            Some(dynamic_filters) => {
                declined.with_updated_node(Arc::new(Self { dynamic_filters, ..self.clone() }))
            }
            None => declined,
        })
    }

    /// **The dynamic filters held are dropped**: a producer's own reset
    /// replaces its filter or discards it, and no pushdown runs again, so one
    /// held across a reset would describe the rows of the execution before.
    fn reset_state(self: Arc<Self>) -> Result<Arc<dyn ExecutionPlan>> {
        if self.dynamic_filters.is_empty() {
            return Ok(self);
        }
        Ok(Arc::new(Self { dynamic_filters: DynamicFilters::default(), ..Self::clone(&self) }))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        self.inner.execute(partition, context)
    }

    /// The streaming exec's own — its output rows and time — and this
    /// scan's ([`ScanMetrics`]).
    fn metrics(&self) -> Option<MetricsSet> {
        let mut metrics = self.inner.metrics().unwrap_or_default();
        for metric in self.metrics.set.clone_inner().iter() {
            metrics.push(Arc::clone(metric));
        }
        Some(metrics)
    }

    fn fetch(&self) -> Option<usize> {
        self.inner.fetch()
    }

    fn with_fetch(&self, limit: Option<usize>) -> Option<Arc<dyn ExecutionPlan>> {
        let inner = self.inner.with_fetch(limit)?;
        let inner = inner.downcast_ref::<StreamingTableExec>()?;
        Some(Arc::new(Self {
            inner: Arc::new(inner.clone()),
            statistics: Arc::clone(&self.statistics),
            metrics: self.metrics.clone(),
            static_filter: Arc::clone(&self.static_filter),
            dynamic_filters: self.dynamic_filters.clone(),
        }))
    }

    fn child_stats_requests(&self, _partition: Option<usize>) -> Vec<ChildStats> {
        vec![]
    }

    /// The table's statistics for the whole node, cut to whatever fetch it
    /// carries — read here rather than folded in when one is set, so the two
    /// roads a fetch arrives by, `scan`'s `limit` argument and
    /// [`Self::with_fetch`], cannot answer differently. **Per partition it
    /// says nothing**: the map counts a block's rows, not a sub-stream's, and
    /// a partition is cut through blocks.
    fn statistics_from_inputs(
        &self,
        _input_stats: &[Arc<Statistics>],
        args: &StatisticsArgs,
    ) -> Result<Arc<Statistics>> {
        Ok(match args.partition() {
            None => Arc::new(fetched(&self.statistics, self.inner.limit())),
            Some(_) => Arc::new(Statistics::new_unknown(&self.schema())),
        })
    }
}

/// `statistics` describing at most `limit` of the rows they were taken over:
/// a fetch cuts the rows the node emits, so the table's own counts and
/// extremes stop describing them and become estimates.
fn fetched(statistics: &Statistics, limit: Option<usize>) -> Statistics {
    let Some(limit) = limit else { return statistics.clone() };
    let mut statistics = statistics.clone();
    if statistics.num_rows.get_value().is_none_or(|rows| *rows > limit) {
        statistics = statistics.to_inexact();
        statistics.num_rows = Precision::Inexact(limit);
    }
    statistics
}

/// The metric counting the row groups statistics ruled out, named as Parquet's
/// scan names its own, so a reader of `EXPLAIN ANALYZE` meets one word for one
/// thing.
pub(crate) const ROW_GROUPS_PRUNED_STATISTICS: &str = "row_groups_pruned_statistics";

/// The metric counting the bytes of rows an early stop on sorted data left
/// unread.
pub(crate) const BYTES_UNREAD_EARLY_STOP: &str = "bytes_unread_early_stop";

/// What a scan reports under `EXPLAIN ANALYZE`, beside the streaming exec's
/// rows and time: the row groups the map's statistics ruled out, of those its
/// blocks list ([`ROW_GROUPS_PRUNED_STATISTICS`]), and the bytes of rows an
/// early stop left unread ([`BYTES_UNREAD_EARLY_STOP`]). Counts rather than
/// findings ([`crate::report`]):
/// the note saying the same of the pruning is also a plan note, which the
/// table's sink hears as an `Info`.
///
/// **The pruning is counted once, for the whole node, when the scan is
/// planned**, since that is when the library settles it; an early stop is
/// found while rows are read, so it is counted per partition as each one
/// ends. Clones share one set, so a plan node rebuilt around a fetch reports
/// what its partitions count.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScanMetrics {
    set: ExecutionPlanMetricsSet,
}

impl ScanMetrics {
    /// The metrics of a scan whose plan settled `notes`.
    pub(crate) fn planned(notes: &[PlanNote]) -> Self {
        let set = ExecutionPlanMetricsSet::new();
        for note in notes {
            if let PlanNoteKind::StatisticsPruned { skipped_groups, groups, .. } = note.kind {
                let pruning = PruningMetrics::new();
                pruning.add_pruned(skipped_groups as usize);
                pruning.add_matched((groups - skipped_groups) as usize);
                MetricBuilder::new(&set)
                    .with_type(MetricType::Summary)
                    .with_category(MetricCategory::Rows)
                    .build(MetricValue::PruningMetrics {
                        name: ROW_GROUPS_PRUNED_STATISTICS.into(),
                        pruning_metrics: pruning,
                    });
            }
        }
        Self { set }
    }

    /// `stream`, partition `partition` of this scan, counting what its early
    /// stops left unread once it ends or is dropped — a `LIMIT` met drops a
    /// partition before its end, and what was stopped by then was still not
    /// read.
    pub(crate) fn counted(
        &self,
        stream: TableStream<'static>,
        partition: usize,
    ) -> EarlyStopsCounted {
        let unread = MetricBuilder::new(&self.set)
            .with_type(MetricType::Summary)
            .with_category(MetricCategory::Bytes)
            .counter(BYTES_UNREAD_EARLY_STOP, partition);
        EarlyStopsCounted { stream, unread, counted: false }
    }
}

/// A partition's stream, adding its early stops' unread bytes to the scan's
/// metric once ([`ScanMetrics::counted`]). A block cut across partitions is
/// counted in each for the pieces that partition read, and the pieces' bytes
/// add (`pgdump_query::EarlyStop::unread_bytes`).
pub(crate) struct EarlyStopsCounted {
    stream: TableStream<'static>,
    unread: Count,
    counted: bool,
}

impl EarlyStopsCounted {
    fn add_unread(&mut self) {
        if !std::mem::replace(&mut self.counted, true) {
            let stops = self.stream.early_stops();
            let bytes: u64 = stops.iter().filter_map(|stop| stop.unread_bytes).sum();
            self.unread.add(usize::try_from(bytes).unwrap_or(usize::MAX));
        }
    }
}

impl Stream for EarlyStopsCounted {
    type Item = pgdump_query::Result<arrow::array::RecordBatch>;

    /// Counted before the end is handed on, so a reader of the metrics who
    /// waited for the end finds it there.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = self.stream.poll_next_unpin(cx);
        if let Poll::Ready(None) = polled {
            self.add_unread();
        }
        polled
    }
}

impl Drop for EarlyStopsCounted {
    fn drop(&mut self) {
        self.add_unread();
    }
}

#[cfg(test)]
mod tests {
    use datafusion::logical_expr::{col, lit, not};

    use super::*;

    /// A static filter written with `more` parts of its conjunction after it
    /// or not.
    struct Beside(StaticFilter, bool);

    impl fmt::Display for Beside {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fmt_beside(f, self.1)
        }
    }

    fn written(filters: &[Expr], more: bool) -> String {
        Beside(StaticFilter::new(filters), more).to_string()
    }

    /// **A static filter is written by bare column name, parenthesised only
    /// where precedence needs it** — shapes a simplified SQL plan never
    /// hands a scan among them, a `NOT` over an `OR`, since an embedder's
    /// plan need not have been simplified.
    #[test]
    fn a_static_filter_is_parenthesised_only_where_precedence_needs_it() {
        let id = || col("o.id").lt(lit(5));
        let stepped = || col("o.stepped").gt(lit(3));
        let card = || col("low_card").eq(lit("amber"));
        let either = || id().or(stepped());
        assert_eq!(written(&[id()], false), "id < 5");
        assert_eq!(written(&[either()], false), "id < 5 OR stepped > 3");
        assert_eq!(written(&[either()], true), "(id < 5 OR stepped > 3)");
        assert_eq!(
            written(&[either(), card()], false),
            "(id < 5 OR stepped > 3) AND low_card = amber"
        );
        assert_eq!(
            written(&[id().or(stepped().and(card().or(id())))], false),
            "id < 5 OR stepped > 3 AND (low_card = amber OR id < 5)"
        );
        assert_eq!(written(&[not(either())], false), "NOT (id < 5 OR stepped > 3)");
        assert_eq!(written(&[not(id())], true), "NOT id < 5");
    }
}
