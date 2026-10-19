//! **DataFusion is the oracle of [`compare_value_keys`]**: every pair of the
//! `types` fixture's scalar columns below is compared by a query, `a < b`
//! and `a = b` over the two tables a provider registers, value pair by
//! value pair, and the function answers what the query answers — `None`
//! where the query refuses, as its cast does a text that does not read as a
//! number; and where the scan reads a value as NULL, only what the keys'
//! own order answers. What is not true by construction is the value the
//! function rebuilds from a key, which a scan emits.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::Int32Type;
use datafusion::prelude::SessionContext;
use datafusion_pgdump::{PgDump, PgDumpOptions, compare_value_keys, register_dump};
use futures::StreamExt;
use pgdt_codec::compare::ComparisonSemantics;
use pgdt_codec::key::ValueKey;
use pgdt_codec::{ResolvedSchema, SchemaMode};
use pgdt_dump::ScanOptions;
use pgdt_dump::cache::CacheMode;
use pgdt_fixtures::{parsed_copy, sweep_request, types_fixture};
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_model::Finding;
use pgdt_query::{QueryOptions, ScanExtent, Table, TableSelector};
use tokio_util::sync::CancellationToken;

/// The columns compared, every pair of them: each emitted type a scan
/// produces for a scalar, the extremes and the infinities among their values,
/// and the text-emitted kinds DataFusion parses as a number or a time.
const COLUMNS: &[(&str, &str)] = &[
    ("t_extremes", "v_smallint"),
    ("t_extremes", "v_integer"),
    ("t_extremes", "v_bigint"),
    ("t_extremes", "v_oid"),
    ("t_extremes", "v_boolean"),
    ("t_extremes", "v_real"),
    ("t_extremes", "v_double"),
    ("t_extremes", "v_numeric38"),
    ("t_extremes", "v_numeric76"),
    ("t_extremes", "v_date"),
    ("t_extremes", "v_ts"),
    ("t_extremes", "v_tstz"),
    ("t_extremes", "v_time"),
    ("t_extremes", "v_interval"),
    ("t_extremes", "v_uuid"),
    ("t_extremes", "v_bytea"),
    ("t_int", "v_integer"),
    ("t_float", "v_real"),
    ("t_numeric", "v_typed"),
    ("t_numeric", "v_small"),
    ("t_numeric", "v_untyped"),
    ("t_date", "v_date"),
    ("t_time", "v_time"),
    ("t_time", "v_timetz"),
    ("t_timestamp", "v_ts"),
    ("t_timestamp", "v_tstz"),
    ("t_text", "v_text"),
    ("t_text", "v_varchar"),
    ("t_text", "v_char"),
    ("t_bytea", "v_bytea"),
    ("t_uuid", "v_uuid"),
    ("t_enum_domain", "v_mood"),
    ("t_enum_domain", "v_domain"),
];

fn ignore(_: &dyn Finding) {}

/// Every batch of `table`'s stream in file order, read cold in `schema_mode`
/// and DataFusion's semantics, and the schema it resolved.
async fn read(dump: &Path, table: &str, schema_mode: SchemaMode) -> (ResolvedSchema, RecordBatch) {
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(dump).unwrap());
    let options = QueryOptions {
        schema_mode,
        semantics: ComparisonSemantics::DataFusion,
        ..QueryOptions::default()
    };
    let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
    let selector = TableSelector::named(table).in_schema("public");
    let (scan, extent) = (ScanOptions::default(), ScanExtent::UntilTargetSettled);
    let token = CancellationToken::new();
    let settled = Table::settle(opened, &selector, scan, extent, schema_mode, &token).await;
    let plan = settled.unwrap().expect("the fixture's table").plan(scan.read, options).unwrap();
    let mut stream = plan.in_file_order(8192);
    let mut batches = Vec::new();
    while let Some(batch) = stream.next().await {
        batches.push(batch.unwrap());
    }
    let resolved = stream.resolved_schema();
    let batch = arrow::compute::concat_batches(&resolved.schema, &batches).unwrap();
    (resolved, batch)
}

/// Each row's key of `column` of `table`, by its `id`, read from the text
/// the file holds; a NULL has none.
async fn keys(dump: &Path, table: &str, column: &str) -> Vec<(i32, ValueKey)> {
    let (resolved, _) = read(dump, table, SchemaMode::Typed).await;
    let (_, text) = read(dump, table, SchemaMode::Strings).await;
    let index = resolved.schema.index_of(column).unwrap();
    let ids = text.column_by_name("id").unwrap().as_string_view();
    let values = text.column_by_name(column).unwrap().as_string_view();
    (0..text.num_rows())
        .filter(|row| values.is_valid(*row))
        .map(|row| {
            let key =
                ValueKey::of(&resolved, index, ComparisonSemantics::DataFusion, values.value(row));
            (ids.value(row).parse().unwrap(), key.expect("a value of the column"))
        })
        .collect()
}

/// What a query answers for `a < b` and `a = b` at each pair of ids it
/// returns: an ordering, or `None` where it answered NULL.
fn answers(batches: &[RecordBatch]) -> HashMap<(i32, i32), Option<Ordering>> {
    let mut answers = HashMap::new();
    for batch in batches {
        let a = batch.column(0).as_primitive::<Int32Type>();
        let b = batch.column(1).as_primitive::<Int32Type>();
        let less = batch.column(2).as_boolean();
        let equal = batch.column(3).as_boolean();
        for row in 0..batch.num_rows() {
            let answer = less.is_valid(row).then(|| match (less.value(row), equal.value(row)) {
                (true, _) => Ordering::Less,
                (false, true) => Ordering::Equal,
                (false, false) => Ordering::Greater,
            });
            answers.insert((a.value(row), b.value(row)), answer);
        }
    }
    answers
}

async fn query(ctx: &SessionContext, sql: &str) -> datafusion::error::Result<Vec<RecordBatch>> {
    ctx.sql(sql).await?.collect().await
}

#[tokio::test(flavor = "multi_thread")]
async fn two_orders_compare_as_datafusion_answers_a_query() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = types_fixture(16, "default");
    let copy = parsed_copy(&fixture, dir.path(), &sweep_request(&fixture)).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let ctx = SessionContext::new();
    register_dump(&ctx, Some("dump"), &dump, Arc::new(ignore)).unwrap();

    let mut columns = Vec::new();
    for (table, column) in COLUMNS {
        columns.push((*table, *column, keys(&copy, table, column).await));
    }
    let (mut answered, mut refused) = (0, 0);
    for (i, (a_table, a_column, a_keys)) in columns.iter().enumerate() {
        for (b_table, b_column, b_keys) in &columns[i + 1..] {
            let what = format!("{a_table}.{a_column} against {b_table}.{b_column}");
            let select = format!(
                "SELECT a.id, b.id, a.{a_column} < b.{b_column}, a.{a_column} = b.{b_column} \
                 FROM dump.public.{a_table} AS a CROSS JOIN dump.public.{b_table} AS b"
            );
            // A query refusing a pair refuses every row, so it is asked
            // again a pair of rows at a time.
            let whole = query(&ctx, &select).await.ok().map(|batches| answers(&batches));
            for (a_id, a) in a_keys {
                for (b_id, b) in b_keys {
                    let datafusion = match &whole {
                        Some(answers) => Ok(answers[&(*a_id, *b_id)]),
                        None => {
                            let pair = format!("{select} WHERE a.id = {a_id} AND b.id = {b_id}");
                            match query(&ctx, &pair).await {
                                Ok(batches) => Ok(answers(&batches)[&(*a_id, *b_id)]),
                                Err(error) => Err(error),
                            }
                        }
                    };
                    let expected = match &datafusion {
                        Ok(Some(order)) => Some(*order),
                        // A value the scan reads as NULL: nothing but the
                        // keys' own order relates it.
                        Ok(None) => a.compare(b),
                        Err(_) => None,
                    };
                    let got = compare_value_keys(a, b);
                    let at = format!("{what}, ids {a_id} and {b_id}: {datafusion:?}");
                    assert_eq!(got, expected, "{at}");
                    assert_eq!(compare_value_keys(b, a), expected.map(Ordering::reverse), "{at}");
                    if a.compare(b).is_none() {
                        if got.is_some() { answered += 1 } else { refused += 1 }
                    }
                }
            }
        }
    }
    // Two orders, related and not, each many times over.
    assert!(answered > 500 && refused > 500, "{answered} answered, {refused} refused");
}
