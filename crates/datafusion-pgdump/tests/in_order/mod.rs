//! A test-only node running each scan's partitions one at a time, in a
//! stated order, so an answer that depends on how they interleave is the
//! same on every run.
//!
//! **What it fixes is the schedule, not the plan.** The node takes the scan's
//! own `PlanProperties` — the same `Arc`, so a parent rebuilt over it keeps
//! whatever it derived from the scan, a TopK's dynamic filter included — and
//! the partition count with them. Each partition's stream is made when its
//! partition is executed but first polled only once every partition ahead of
//! it in the order has ended or been dropped, so the byte cut a dynamic filter
//! makes at the first poll is made by the first in the order, and each
//! producer directly above has taken every batch the partitions before it
//! emitted by the time the next starts.
//!
//! **A consumer that polls one partition to its end before polling the next
//! must poll them in the stated order**, or it waits forever; every consumer
//! a SQL plan puts above a scan here polls its inputs concurrently.
//!
//! It is added as the session's last physical optimizer rule
//! ([`RunScansInOrder`]), after every rule that plans a filter or reads a
//! statistic, so it changes nothing either sees.

// Each target including this module uses a part of it.
#![allow(dead_code)]

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRecursion};
use datafusion::common::{Result, Statistics};
use datafusion::config::ConfigOptions;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_optimizer::PhysicalOptimizerRule;
use datafusion::physical_plan::execution_plan::{ChildrenPropertiesMode, ReplaceChildrenOptions};
use datafusion::physical_plan::statistics::{ChildStats, StatisticsArgs};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties};
use futures::future::BoxFuture;
use futures::{FutureExt, Stream};
use tokio::sync::watch;

/// The order a scan's partitions run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Partition 0 first: the file's order, each partition being a later
    /// stretch of it than the one before.
    File,
    /// The last partition first.
    Reversed,
}

impl Order {
    pub const ALL: [Order; 2] = [Order::File, Order::Reversed];

    /// Where `partition` of `count` runs: 0 first.
    fn position(self, partition: usize, count: usize) -> usize {
        match self {
            Order::File => partition,
            Order::Reversed => count - 1 - partition,
        }
    }
}

/// The physical optimizer rule wrapping every `PgDumpExec` in an
/// [`InOrderExec`].
#[derive(Debug)]
pub struct RunScansInOrder(pub Order);

impl PhysicalOptimizerRule for RunScansInOrder {
    fn optimize(
        &self,
        plan: Arc<dyn ExecutionPlan>,
        _config: &ConfigOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        plan.transform_up(|node| {
            Ok(if node.name() == "PgDumpExec" {
                Transformed::yes(Arc::new(InOrderExec::new(node, self.0)) as _)
            } else {
                Transformed::no(node)
            })
        })
        .map(|transformed| transformed.data)
    }

    fn name(&self) -> &str {
        "run_scans_in_order"
    }

    fn schema_check(&self) -> bool {
        true
    }
}

/// One scan whose partitions run in [`Order`].
#[derive(Debug)]
pub struct InOrderExec {
    input: Arc<dyn ExecutionPlan>,
    order: Order,
    /// Which positions in the order have ended, one flag each: fresh for each
    /// node, so a plan reset or rebuilt runs its partitions again.
    ended: watch::Sender<Vec<bool>>,
}

impl InOrderExec {
    fn new(input: Arc<dyn ExecutionPlan>, order: Order) -> Self {
        let count = input.properties().partitioning.partition_count();
        Self { input, order, ended: watch::Sender::new(vec![false; count]) }
    }
}

impl DisplayAs for InOrderExec {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "InOrderExec: order={:?}", self.order)
    }
}

impl ExecutionPlan for InOrderExec {
    fn name(&self) -> &str {
        "InOrderExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.input.properties()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.input]
    }

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> Result<TreeNodeRecursion>,
    ) -> Result<TreeNodeRecursion> {
        Ok(TreeNodeRecursion::Continue)
    }

    fn replace_children(
        self: Arc<Self>,
        mut children: Vec<Arc<dyn ExecutionPlan>>,
        _: ReplaceChildrenOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(Arc::new(Self::new(children.swap_remove(0), self.order)))
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

    fn child_stats_requests(&self, partition: Option<usize>) -> Vec<ChildStats> {
        vec![ChildStats::At(partition)]
    }

    fn statistics_from_inputs(
        &self,
        input_stats: &[Arc<Statistics>],
        _args: &StatisticsArgs,
    ) -> Result<Arc<Statistics>> {
        Ok(Arc::clone(&input_stats[0]))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let inner = self.input.execute(partition, context)?;
        let count = self.input.properties().partitioning.partition_count();
        let position = self.order.position(partition, count);
        let mut ended = self.ended.subscribe();
        let turn = async move {
            // The sender lives as long as the node, which outlives its streams.
            let _ = ended.wait_for(|ended| ended[..position].iter().all(|&e| e)).await;
        }
        .boxed();
        let schema = inner.schema();
        let stream = InTurn {
            turn: Some(turn),
            inner,
            end: Some(End { ended: self.ended.clone(), position }),
        };
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }
}

/// Marks its position ended when dropped.
struct End {
    ended: watch::Sender<Vec<bool>>,
    position: usize,
}

impl Drop for End {
    fn drop(&mut self) {
        self.ended.send_modify(|ended| ended[self.position] = true);
    }
}

/// A partition's stream, polled only once its turn has come, and marking its
/// turn over at its end, its first error or its drop.
struct InTurn {
    turn: Option<BoxFuture<'static, ()>>,
    inner: SendableRecordBatchStream,
    end: Option<End>,
}

impl Stream for InTurn {
    type Item = Result<arrow::array::RecordBatch>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(turn) = self.turn.as_mut() {
            if turn.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            self.turn = None;
        }
        let next = self.inner.as_mut().poll_next(cx);
        if matches!(next, Poll::Ready(None | Some(Err(_)))) {
            self.end = None;
        }
        next
    }
}
