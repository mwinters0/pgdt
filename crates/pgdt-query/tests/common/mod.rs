//! The fixture vocabulary every integration test speaks, `pgdt-fixtures`, and
//! what reading a batch back adds to it.
//!
//! Everything here is a *path* or an *expectation*, but `pgdt-fixtures`'
//! fixture mapped beside its cache, the [`Dump`] a test plans a table's
//! reads over ([`mapped`], [`dump_over`]), and a cold query's table and its
//! stream ([`Cold`]), as `pgdt query` reaches them. Anything else that drives
//! the library — a `collect`, a `drain`, a `census_of` — stays in the file
//! whose subject it is.

// Each test binary compiles the whole module and uses a subset of it, so
// per-binary dead-code warnings are structural rather than informative.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::datatypes::{DataType, Field};
use pgdt_codec::compare::ComparisonPlan;
use pgdt_codec::internal::column_of;
use pgdt_codec::pgtype::NestedPlan;
use pgdt_codec::{ColumnResolution, ResolvedColumn, SchemaMode};
use pgdt_dump::cache::{CacheMode, SourceWatch, StrictIdentity};
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::ByteRangeSource;
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{DumpIndex, PostgresInvalidValues};
use pgdt_query::{
    Dump, QueryOptions, ScanExtent, Table, TablePartitions, TableSelector, TableStream,
    render_field,
};
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub use pgdt_fixtures::*;

/// `source` mapped whole under `request`, serially and writing no cache, as
/// the [`Dump`] a test plans a table's reads over.
pub async fn mapped(source: Arc<dyn ByteRangeSource>, request: &StatisticsRequest) -> Dump {
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let read = ReadOptions::default();
    let invalid = PostgresInvalidValues::Default;
    let token = CancellationToken::new();
    let run = map_file(opened, read, &arranged(1), invalid, request, &token).await.unwrap();
    Dump::mapped(run).await.unwrap()
}

/// The [`Dump`] over `index`, a map of `source` a test has reshaped by hand:
/// saved as a cache in `dir` and opened through it, as any cache is, so the
/// refusal of one short of the file's end is the open's.
pub async fn dump_over(
    source: Arc<dyn ByteRangeSource>,
    index: &DumpIndex,
    dir: &Path,
) -> pgdt_core::Result<Dump> {
    let mode = CacheMode::enabled(dir.join("held.dtcache"));
    let watch = SourceWatch::open(source.as_ref(), StrictIdentity::ADVISORY).await?;
    mode.save(&watch, source.as_ref(), index).await?;
    Dump::opened(mode.open_source(source).await?).await
}

/// `table` as a person types it, qualified (`schema.table`) or bare (any
/// schema), split at its first `.` — as `pgdt query --table` reads it.
pub fn selector(table: &str) -> TableSelector {
    match table.split_once('.') {
        Some((schema, table)) => TableSelector::named(table).in_schema(schema),
        None => TableSelector::named(table),
    }
}

/// A cold query's passes, as `pgdt query` runs them: the mapping pass's
/// arrangement and extent, the cache it opens through, the database its
/// selector names beside the table (`--database`), and the rows a batch of
/// the replay holds. [`Cold::default`] is the serial pass stopping once its
/// table is settled, writing no cache, in any database, at the batch `pgdt
/// query` reads.
#[derive(Debug, Clone)]
pub struct Cold {
    pub scan: ScanOptions,
    pub extent: ScanExtent,
    pub cache: CacheMode,
    pub database: Option<String>,
    pub max_rows: usize,
}

impl Default for Cold {
    fn default() -> Self {
        Self {
            scan: ScanOptions::default(),
            extent: ScanExtent::UntilTargetSettled,
            cache: CacheMode::DISABLED,
            database: None,
            max_rows: 8192,
        }
    }
}

impl Cold {
    /// The table `table` selects ([`selector`]), in this database where one
    /// is named, in `source` opened through this cache and mapped until it
    /// is settled, holding the census a typed query of `schema_mode` reads —
    /// `None` where the dump holds none.
    pub async fn table(
        &self,
        source: Arc<dyn ByteRangeSource>,
        table: &str,
        schema_mode: SchemaMode,
    ) -> pgdt_core::Result<Option<Table>> {
        let selected = match &self.database {
            Some(database) => selector(table).in_database(database.clone()),
            None => selector(table),
        };
        let opened = self.cache.open_source(source).await?;
        let token = CancellationToken::new();
        Table::settle(opened, &selected, self.scan, self.extent, schema_mode, &token).await
    }

    /// That table's plan under `options`, read with this pass's chunk.
    pub async fn plan(
        &self,
        source: Arc<dyn ByteRangeSource>,
        table: &str,
        options: QueryOptions,
    ) -> pgdt_core::Result<Option<TablePartitions>> {
        let Some(found) = self.table(source, table, options.schema_mode).await? else {
            return Ok(None);
        };
        found.plan(self.scan.read, options).map(Some)
    }

    /// That plan's whole table in file order, at [`Cold::max_rows`] a batch.
    pub async fn stream(
        &self,
        source: Arc<dyn ByteRangeSource>,
        table: &str,
        options: QueryOptions,
    ) -> pgdt_core::Result<Option<TableStream<'static>>> {
        let plan = self.plan(source, table, options).await?;
        Ok(plan.map(|plan| plan.in_file_order(self.max_rows)))
    }
}

/// Every column of `batch` rendered back to PostgreSQL text via
/// [`render_field`] — which works regardless of `SchemaMode`: a hand-written
/// fixture with no DDL (like [`edge_cases`]) resolves every column
/// `Utf8View` either way, so this reads identically to a hardcoded
/// `StringViewArray` downcast there, and it also handles a real `pg_dump`
/// fixture's typed columns (`docs/design/decisions.md`, "D66":
/// render-back is exactly this build's own decode/render round trip).
///
/// Every column is read as a scalar one ([`scalar_column`]), which is right
/// for every non-nested type. A test reading a nested column wants the
/// stream's own `ResolvedSchema::columns` instead, and asks for them where it
/// queries.
pub fn rows_of(batch: &RecordBatch) -> Vec<Vec<Option<String>>> {
    let columns: Vec<ResolvedColumn> =
        batch.columns().iter().map(|c| scalar_column(c.data_type())).collect();
    (0..batch.num_rows())
        .map(|row| {
            batch
                .columns()
                .iter()
                .zip(&columns)
                .map(|(c, column)| {
                    render_field(c.as_ref(), row, column).expect("every fixture value renders back")
                })
                .collect()
        })
        .collect()
}

/// A scalar column's record for an array of `data_type`, no DDL behind it:
/// what renders a batch's column back without the stream's schema, right
/// for every non-nested type.
pub fn scalar_column(data_type: &DataType) -> ResolvedColumn {
    column_of(
        Field::new("v", data_type.clone(), true),
        None,
        ColumnResolution::NotDeclared,
        NestedPlan::Scalar,
        ComparisonPlan::Refused,
        false,
    )
}

/// `public.widgets` from the hand-written [`edge_cases`] dump, as decoded
/// rows. Every value here is chosen to break a naive scanner or decoder: an
/// embedded newline and tab, a literal backslash, a `COPY`-like phrase inside
/// a field, an empty string next to a NULL, and a bare carriage return.
pub fn widgets_expected() -> Vec<Vec<Option<String>>> {
    vec![
        vec![
            Some("1".into()),
            Some("alpha".into()),
            Some("a simple widget".into()),
            Some("2024-01-01 00:00:00+00".into()),
        ],
        vec![Some("2".into()), Some("beta".into()), None, Some("2024-01-02 00:00:00+00".into())],
        vec![
            Some("3".into()),
            Some("gamma".into()),
            Some("multi\nline\twith a backslash \\ inside".into()),
            None,
        ],
        vec![
            Some("4".into()),
            Some("delta".into()),
            Some(
                "contains a COPY-like phrase: COPY public.widgets (id) FROM stdin; -- not a directive"
                    .into(),
            ),
            Some("2024-01-04 00:00:00+00".into()),
        ],
        vec![
            Some("5".into()),
            Some("".into()),
            Some("empty name to the left".into()),
            Some("2024-01-05 00:00:00+00".into()),
        ],
        vec![
            Some("6".into()),
            Some("epsilon".into()),
            Some("carriage\rreturn, octal A, hex B".into()),
            Some("2024-01-06 00:00:00+00".into()),
        ],
    ]
}
