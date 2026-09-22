//! The scan's plan node: a `StreamingTableExec` over the replay's
//! sub-streams, carrying what the dump's map says about the table.
//!
//! **It exists only to answer `statistics_from_inputs`.** DataFusion reads a
//! source's statistics off the `ExecutionPlan` — `TableProvider` has no
//! statistics method in 55 — and `StreamingTableExec` answers
//! `Statistics::new_unknown`. So the streaming exec stays, held as an
//! implementation detail rather than as a child: this is a leaf, and nothing
//! in a plan tree sees the node inside it.

use std::fmt;
use std::sync::Arc;

use datafusion::common::stats::Precision;
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{Result, Statistics, internal_err};
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::execution_plan::{ChildrenPropertiesMode, ReplaceChildrenOptions};
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion::physical_plan::statistics::{ChildStats, StatisticsArgs};
use datafusion::physical_plan::streaming::StreamingTableExec;
use datafusion::physical_plan::{DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties};

/// The replay of one table, with its statistics.
#[derive(Debug, Clone)]
pub(crate) struct PgDumpExec {
    /// The streaming exec this node *is*, minus the statistics. Held rather
    /// than made a child, so the optimizer sees one leaf and the node it
    /// would otherwise replace cannot be swapped out from under the
    /// statistics.
    inner: Arc<StreamingTableExec>,
    statistics: Arc<Statistics>,
}

impl PgDumpExec {
    pub(crate) fn new(inner: StreamingTableExec, statistics: Statistics) -> Self {
        Self { inner: Arc::new(inner), statistics: Arc::new(statistics) }
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
                Ok(())
            }
            DisplayFormatType::TreeRender => self.inner.fmt_as(t, f),
        }
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

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> Result<TreeNodeRecursion>,
    ) -> Result<TreeNodeRecursion> {
        Ok(TreeNodeRecursion::Continue)
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

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        self.inner.execute(partition, context)
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.inner.metrics()
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
