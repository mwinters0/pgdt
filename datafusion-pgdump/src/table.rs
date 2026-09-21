//! One table of a dump as a DataFusion `TableProvider`, scanned as the
//! library's partitioned replay.

use std::fmt;
use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::{DataFusionError, Result};
use datafusion::datasource::TableType;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::streaming::{PartitionStream, StreamingTableExec};
use futures::StreamExt;
use pgdump_query::{
    ComparisonSemantics, QueryOptions, ResolvedSchema, ScanOptions, TableName, TablePartitions,
};

use crate::budget::{Draw, ScanBudget};
use crate::dump::PgDump;

/// One table of an opened [`PgDump`]. Its schema is settled when it is built,
/// from the map and the DDL alone (`pgdump_query::table_schema`), so asking for
/// it reads nothing; a scan is the library's partitioned replay over the
/// dump's complete map.
pub struct PgDumpTable {
    dump: Arc<PgDump>,
    name: TableName,
    resolved: ResolvedSchema,
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
        let resolved = pgdump_query::table_schema(dump.index(), &name, &query_options(&dump))
            .map_err(external)?;
        Ok(Self { dump, name, resolved })
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
}

/// What every query of `dump` asks, before a scan adds its projection,
/// parallelism and batch size: the dump's schema mode, and DataFusion's
/// comparison semantics (`docs/design/roadmap-P6-datafusion.md`, "Comparison
/// means what DataFusion means").
fn query_options(dump: &PgDump) -> QueryOptions {
    QueryOptions {
        schema_mode: dump.schema_mode(),
        semantics: ComparisonSemantics::Arrow,
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

    /// The replay planned now, against the session's `target_partitions` and
    /// what the session's [`ScanBudget`] leaves; the partitions are streamed
    /// when DataFusion runs them. The projection is the library's by name, so
    /// an unprojected column is never decoded. `limit` is left to the plan,
    /// which stops pulling once it is met.
    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let source = Arc::clone(self.dump.source());
        let draw = Arc::new(
            ScanBudget::of(state)
                .draw(state.config().target_partitions(), source.default_worker_memory()),
        );
        let parallelism = draw.parallelism();
        let schema = self.resolved.schema.fields();
        let query_options = QueryOptions {
            projection: projection
                .map(|columns| columns.iter().map(|&i| schema[i].name().clone()).collect()),
            parallelism,
            max_rows: state.config().batch_size(),
            ..query_options(&self.dump)
        };
        let scan_options = ScanOptions { parallelism, ..ScanOptions::default() };
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
        let partitions = Arc::new(partitions);
        let schema = partitions.resolved_schema().schema;
        let streams = (0..partitions.len())
            .map(|index| {
                Arc::new(Partition {
                    partitions: Arc::clone(&partitions),
                    index,
                    schema: Arc::clone(&schema),
                    draw: Arc::clone(&draw),
                }) as Arc<dyn PartitionStream>
            })
            .collect();
        Ok(Arc::new(StreamingTableExec::try_new(schema, streams, None, [], false, limit)?))
    }
}

/// One sub-stream of a planned replay, run when DataFusion executes it.
struct Partition {
    partitions: Arc<TablePartitions>,
    index: usize,
    schema: SchemaRef,
    /// Held by the plan and by every stream it starts, so the budget it drew
    /// returns once the last of them is gone.
    draw: Arc<Draw>,
}

impl fmt::Debug for Partition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Partition").field("index", &self.index).finish()
    }
}

impl PartitionStream for Partition {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// Batch size is read here, from the context the partition runs under, as
    /// DataFusion's own sources read it.
    fn execute(&self, ctx: Arc<TaskContext>) -> SendableRecordBatchStream {
        let draw = Arc::clone(&self.draw);
        let rows = self.partitions.stream(self.index, ctx.session_config().batch_size()).map(
            move |batch| {
                let _held = &draw;
                batch.map_err(external)
            },
        );
        Box::pin(RecordBatchStreamAdapter::new(Arc::clone(&self.schema), rows))
    }
}
