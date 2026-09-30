//! Filter pushdown against DataFusion's own comparison, over the committed
//! fixtures (`docs/design/decisions.md`, "D40" and "D88").
//!
//! **DataFusion is the oracle.** A filter the provider answers `Exact` is
//! evaluated by the library in DataFusion semantics and DataFusion never sees the
//! rows it dropped, so the rows a pushed scan returns must be exactly the
//! rows DataFusion's own evaluation of that filter keeps from the unfiltered
//! scan — the physical expression a `FilterExec` would run, over the arrays
//! the provider emits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BooleanArray, Float32Array, Int32Array, ListArray, RecordBatch,
};
use arrow::buffer::OffsetBuffer;
use arrow::compute::{concat_batches, filter_record_batch};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::util::display::{ArrayFormatter, FormatOptions};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::{Column, DFSchema, ScalarValue};
use datafusion::datasource::TableType;
use datafusion::logical_expr::{
    BinaryExpr, Expr, Operator, TableProviderFilterPushDown, expr::InList,
};
use datafusion::physical_expr_common::datum::compare_op_for_nested;
use datafusion::physical_plan::{ExecutionPlan, displayable, execute_stream_partitioned};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions, PgDumpTable};
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ColumnDef, ComparisonDivergence, ComparisonSemantics, DatabaseMetadata, DumpMetadata,
    LocalFileSource, NestedPlan, ResolvedSchema, ScanOptions, SchemaMode, StatisticsRequest,
    column_divergences, map_file, resolve_columns,
};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// Every major's `default.sql` of every schema: each kind the register has,
/// and the statistics fixture's groups, so a pushed filter is also pruned.
fn default_fixtures() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for major in std::fs::read_dir(fixtures_root()).unwrap() {
        let major = major.unwrap().path();
        if !major.is_dir() {
            continue;
        }
        for schema in std::fs::read_dir(&major).unwrap() {
            let dump = schema.unwrap().path().join("default.sql");
            if dump.is_file() {
                found.push(dump);
            }
        }
    }
    found.sort();
    assert!(!found.is_empty(), "no fixtures under {}", fixtures_root().display());
    found
}

/// `fixture` copied into `dir` beside a complete cache holding every
/// statistic a parse gathers, so the committed tree is never written into.
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::DATA).await.unwrap();
    copy
}

/// Small batches and several partitions, so the cuts land inside blocks.
fn session() -> SessionContext {
    SessionContext::new_with_config(
        SessionConfig::new().with_target_partitions(3).with_batch_size(2),
    )
}

/// Every batch of `plan`, partition by partition.
async fn run(ctx: &SessionContext, plan: Arc<dyn ExecutionPlan>) -> Vec<RecordBatch> {
    let mut batches = Vec::new();
    for mut stream in execute_stream_partitioned(plan, ctx.task_ctx()).unwrap() {
        while let Some(batch) = stream.next().await {
            batches.push(batch.unwrap());
        }
    }
    batches
}

/// Each row of `batch` as text, sorted: the rows as a multiset, whatever
/// order the partitions returned them in.
fn rows(batch: &RecordBatch) -> Vec<String> {
    let options = FormatOptions::default().with_null("NULL");
    let columns: Vec<_> = batch
        .columns()
        .iter()
        .map(|c| ArrayFormatter::try_new(c.as_ref(), &options).unwrap())
        .collect();
    let mut out: Vec<String> = (0..batch.num_rows())
        .map(|row| columns.iter().map(|c| c.value(row).to_string()).collect::<Vec<_>>().join("|"))
        .collect();
    out.sort();
    out
}

fn column(name: &str) -> Expr {
    Expr::Column(Column::new_unqualified(name))
}

fn literal(value: ScalarValue) -> Expr {
    Expr::Literal(value, None)
}

fn binary(left: Expr, op: Operator, right: Expr) -> Expr {
    Expr::BinaryExpr(BinaryExpr::new(Box::new(left), op, Box::new(right)))
}

/// The eight comparing operators, with the literal on either side.
const OPERATORS: [Operator; 8] = [
    Operator::Eq,
    Operator::NotEq,
    Operator::Lt,
    Operator::LtEq,
    Operator::Gt,
    Operator::GtEq,
    Operator::IsDistinctFrom,
    Operator::IsNotDistinctFrom,
];

/// Up to three of `array`'s distinct non-null values, spread over it in row
/// order, and for a string column each one's upper-case spelling besides — a
/// literal the file never writes.
fn literals(array: &ArrayRef) -> Vec<ScalarValue> {
    let options = FormatOptions::default();
    let formatter = ArrayFormatter::try_new(array.as_ref(), &options).unwrap();
    let mut seen = BTreeMap::new();
    for row in 0..array.len() {
        if array.is_valid(row) {
            seen.entry(formatter.value(row).to_string()).or_insert(row);
        }
    }
    let mut distinct: Vec<usize> = seen.into_values().collect();
    distinct.sort();
    let picks: Vec<usize> = match distinct.len() {
        0 => Vec::new(),
        n => {
            let mut picks = vec![distinct[0], distinct[n / 2], distinct[n - 1]];
            picks.dedup();
            picks
        }
    };
    let mut out = Vec::new();
    for row in picks {
        let value = ScalarValue::try_from_array(array, row).unwrap();
        if let ScalarValue::Utf8View(Some(text)) = &value
            && text.to_uppercase() != *text
        {
            out.push(ScalarValue::Utf8View(Some(text.to_uppercase())));
        }
        out.push(value);
    }
    out
}

/// What was asked of one column type: filters answered `Exact`, and every
/// filter tried.
#[derive(Default, Debug)]
struct Tally {
    exact: usize,
    tried: usize,
}

/// **Every filter the provider answers `Exact` keeps exactly the rows
/// DataFusion's own evaluation keeps**, per column and operator, each column's
/// own values the literals, both sides of the operator, typed and as text;
/// and every scalar column type the fixtures hold has filters answered
/// `Exact`, where no nested one does.
#[tokio::test(flavor = "multi_thread")]
async fn a_pushed_filter_keeps_the_rows_datafusion_keeps() {
    let dir = tempfile::tempdir().unwrap();
    let mut tally: BTreeMap<(String, bool), Tally> = BTreeMap::new();
    let mut compared = 0;
    for fixture in default_fixtures() {
        let copy = parsed_copy(&fixture, dir.path()).await;
        // As text, every column is `Utf8View` and compared bytewise; one
        // major's fixtures say as much as six.
        let modes: &[SchemaMode] = if fixture.starts_with(fixtures_root().join("18")) {
            &[SchemaMode::Typed, SchemaMode::Strings]
        } else {
            &[SchemaMode::Typed]
        };
        for &schema_mode in modes {
            let options = PgDumpOptions { schema_mode, ..PgDumpOptions::default() };
            let dump = PgDump::open(copy.to_str().unwrap(), options).await.unwrap();
            let ctx = session();
            let state = ctx.state();
            for name in dump.tables() {
                let Ok(table) = PgDumpTable::new(Arc::clone(&dump), name.clone()) else {
                    continue;
                };
                let resolved = table.resolved_schema().clone();
                for (index, field) in resolved.schema.fields().iter().enumerate() {
                    let what = format!(
                        "{}.{} in {} ({schema_mode:?})",
                        name.qualified(),
                        field.name(),
                        fixture.display()
                    );
                    let projection = vec![index];
                    let unfiltered =
                        table.scan(&state, Some(&projection), &[], None).await.unwrap();
                    let schema = unfiltered.schema();
                    // A column holding a value its Arrow type cannot (D96)
                    // fails the unfiltered read, and has no oracle.
                    let Ok(all) = try_run(&ctx, unfiltered).await else { continue };
                    let all = concat_batches(&schema, &all).unwrap();
                    let df_schema = DFSchema::try_from(Arc::clone(&schema)).unwrap();
                    let scalar = resolved.plans[index] == NestedPlan::Scalar;
                    let key = (field.data_type().to_string(), scalar);
                    let mut filters = vec![
                        Expr::IsNull(Box::new(column(field.name()))),
                        Expr::IsNotNull(Box::new(column(field.name()))),
                    ];
                    let values = literals(all.column(0));
                    for value in &values {
                        for op in OPERATORS {
                            let (c, v) = (column(field.name()), literal(value.clone()));
                            filters.push(binary(c.clone(), op, v.clone()));
                            filters.push(binary(v, op, c));
                        }
                    }
                    if values.len() > 1
                        && !matches!(field.data_type(), DataType::Float32 | DataType::Float64)
                    {
                        // Each way round, and with a `NULL` in the list,
                        // which the library's membership answers as SQL's.
                        let null = ScalarValue::try_from(values[0].data_type()).unwrap();
                        let with_null: Vec<_> =
                            values.iter().cloned().chain([null]).map(literal).collect();
                        let without: Vec<_> = values.iter().cloned().map(literal).collect();
                        for list in [without, with_null] {
                            for negated in [false, true] {
                                filters.push(Expr::InList(InList::new(
                                    Box::new(column(field.name())),
                                    list.clone(),
                                    negated,
                                )));
                            }
                        }
                    }
                    for filter in filters {
                        let pushed =
                            table.supports_filters_pushdown(&[&filter]).unwrap()[0].clone();
                        let null_test = matches!(filter, Expr::IsNull(_) | Expr::IsNotNull(_));
                        if null_test {
                            // Every column's, nested or not: they read no value.
                            assert_eq!(pushed, TableProviderFilterPushDown::Exact, "{what}");
                        }
                        let entry = tally.entry(key.clone()).or_default();
                        entry.tried += usize::from(!null_test);
                        if pushed != TableProviderFilterPushDown::Exact {
                            assert_eq!(pushed, TableProviderFilterPushDown::Unsupported, "{what}");
                            continue;
                        }
                        entry.exact += usize::from(!null_test);
                        let predicate =
                            state.create_physical_expr(filter.clone(), &df_schema).unwrap();
                        let mask =
                            predicate.evaluate(&all).unwrap().into_array(all.num_rows()).unwrap();
                        let mask = mask.as_any().downcast_ref::<BooleanArray>().unwrap();
                        let expected = filter_record_batch(&all, mask).unwrap();
                        let plan = table
                            .scan(&state, Some(&projection), std::slice::from_ref(&filter), None)
                            .await
                            .unwrap();
                        let got = concat_batches(&schema, &run(&ctx, plan).await).unwrap();
                        assert_eq!(rows(&got), rows(&expected), "{what}: {filter}");
                        compared += 1;
                    }
                }
            }
        }
    }
    for ((data_type, scalar), counts) in &tally {
        if *scalar {
            assert!(counts.exact > 0, "{data_type}: no filter pushed in {counts:?}");
        } else {
            assert_eq!(counts.exact, 0, "{data_type}: a nested comparison pushed, {counts:?}");
        }
    }
    // Every scalar type the register emits but `Timestamp(µs)` with no zone,
    // whose one fixture column holds `infinity` (D96) on every major.
    for data_type in [
        "Boolean",
        "Int16",
        "Int32",
        "Int64",
        "UInt32",
        "Float32",
        "Float64",
        "Decimal128(38, 10)",
        "Decimal256(39, 10)",
        "Utf8View",
        "Dictionary(Int32, Utf8)",
        "Date32",
        "Time64(µs)",
        "Timestamp(µs, \"UTC\")",
        "Interval(MonthDayNano)",
        "Binary",
        "FixedSizeBinary(16)",
    ] {
        assert!(
            tally.keys().any(|(t, _)| t == data_type),
            "no {data_type} column compared: {tally:?}"
        );
    }
    assert!(compared > 1000, "{compared}");
}

async fn try_run(
    ctx: &SessionContext,
    plan: Arc<dyn ExecutionPlan>,
) -> datafusion::error::Result<Vec<RecordBatch>> {
    let mut batches = Vec::new();
    for mut stream in execute_stream_partitioned(plan, ctx.task_ctx())? {
        while let Some(batch) = stream.next().await {
            batches.push(batch?);
        }
    }
    Ok(batches)
}

/// The provider with pushdown forced off: every filter `Unsupported`, so
/// DataFusion evaluates each one itself.
#[derive(Debug)]
struct NoPushdown(Arc<PgDumpTable>);

#[async_trait]
impl TableProvider for NoPushdown {
    fn schema(&self) -> SchemaRef {
        self.0.schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        assert!(filters.is_empty());
        self.0.scan(state, projection, filters, limit).await
    }
}

/// **SQL as a user writes it reaches the provider as terms it answers, and
/// pushdown on and off answer alike**: DataFusion's coercion unwraps the
/// literal's cast onto the column's type, `BETWEEN` and `IN` arrive whole, and
/// what DataFusion keeps a cast around the column for stays with it.
#[tokio::test(flavor = "multi_thread")]
async fn sql_pushes_down_what_the_library_answers_and_answers_alike() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = fixtures_root().join("16/types/default.sql");
    let copy = parsed_copy(&fixture, dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let (on, off) = (session(), session());
    for table in [
        "t_int",
        "t_text",
        "t_enum_domain",
        "t_float",
        "t_numeric",
        "t_net",
        "t_bytea",
        "t_uuid",
        "t_oid",
        "t_interval",
        "t_time",
    ] {
        let provider = dump.table(None, Some("public"), table).unwrap();
        on.register_table(table, Arc::clone(&provider) as Arc<dyn TableProvider>).unwrap();
        off.register_table(table, Arc::new(NoPushdown(provider))).unwrap();
    }
    let objects = parsed_copy(&fixtures_root().join("16/objects/default.sql"), dir.path()).await;
    let objects = PgDump::open(objects.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let events = objects.table(None, Some("objects"), "events_2024").unwrap();
    on.register_table("events", Arc::clone(&events) as Arc<dyn TableProvider>).unwrap();
    off.register_table("events", Arc::new(NoPushdown(events))).unwrap();
    let edge = parsed_copy(&fixtures_root().join("16/edge_cases/default.sql"), dir.path()).await;
    let edge = PgDump::open(edge.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let widgets = edge.table(None, Some("public"), "widgets").unwrap();
    on.register_table("widgets", Arc::clone(&widgets) as Arc<dyn TableProvider>).unwrap();
    off.register_table("widgets", Arc::new(NoPushdown(widgets))).unwrap();
    // (query, pushed): whether the scan answers the whole filter itself.
    let cases = [
        ("SELECT id FROM widgets WHERE is_active", true),
        ("SELECT id FROM widgets WHERE NOT is_active", true),
        ("SELECT id FROM widgets WHERE is_active IS NOT TRUE", true),
        ("SELECT id FROM widgets WHERE is_active IS FALSE", true),
        ("SELECT id FROM widgets WHERE is_active IS UNKNOWN", true),
        ("SELECT id FROM widgets WHERE created_at >= '2024-01-02T00:00:00Z'", true),
        ("SELECT id FROM widgets WHERE name = '' AND is_active IS NULL", true),
        ("SELECT id FROM t_int WHERE v_integer = 0", true),
        ("SELECT id FROM t_int WHERE v_smallint < 1", true),
        ("SELECT id FROM t_int WHERE 0 <= v_bigint", true),
        ("SELECT id FROM t_int WHERE v_integer BETWEEN -5 AND 5", true),
        ("SELECT id FROM t_int WHERE v_integer NOT BETWEEN -5 AND 5", true),
        ("SELECT id FROM t_int WHERE v_integer IN (0, 2147483647, 7)", true),
        ("SELECT id FROM t_int WHERE v_integer NOT IN (0, 7)", true),
        ("SELECT id FROM t_int WHERE v_integer IS NULL OR v_smallint > 0", true),
        ("SELECT id FROM t_int WHERE NOT (v_integer > 0)", true),
        ("SELECT id FROM t_int WHERE v_integer + 1 > 0", false),
        ("SELECT id FROM t_int WHERE v_integer > 0.5", false),
        ("SELECT id FROM t_text WHERE v_text = 'hello'", true),
        ("SELECT id FROM t_text WHERE v_text < 'b'", true),
        ("SELECT id FROM t_text WHERE v_char = 'abc'", true),
        ("SELECT id FROM t_text WHERE v_varchar LIKE 'h%'", false),
        ("SELECT id FROM t_enum_domain WHERE v_mood = 'sad'", true),
        ("SELECT id FROM t_enum_domain WHERE v_mood > 'has space'", true),
        ("SELECT id FROM t_enum_domain WHERE v_domain >= 0", true),
        ("SELECT id FROM events WHERE event_date < '2024-06-01'", true),
        ("SELECT id FROM events WHERE event_date = DATE '2024-01-15'", true),
        ("SELECT id FROM t_float WHERE v_double = 0", true),
        ("SELECT id FROM t_float WHERE v_double = -0.0", true),
        ("SELECT id FROM t_float WHERE v_real > 1", true),
        ("SELECT id FROM t_float WHERE v_double = 'NaN'", true),
        // A short list becomes `=` terms before it reaches the provider; a
        // long one stays a list, answered from a set of the values.
        ("SELECT id FROM t_float WHERE v_double IN (0, 1)", true),
        ("SELECT id FROM t_float WHERE v_double IN (0, 1, 2, 3, 4)", false),
        ("SELECT id FROM t_int WHERE v_integer IN (0, 1, 2, 3, 4)", true),
        ("SELECT id FROM t_int WHERE v_integer IN (0, 1, 2, NULL)", true),
        ("SELECT id FROM t_int WHERE v_integer NOT IN (0, 1, 2, NULL)", true),
        ("SELECT id FROM t_enum_domain WHERE v_mood IN ('sad', 'ok', 'happy', NULL)", true),
        ("SELECT id FROM t_numeric WHERE v_untyped = '100.00'", true),
        ("SELECT id FROM t_numeric WHERE v_typed = -1.5", true),
        ("SELECT id FROM t_net WHERE v_inet = '192.168.1.1'", true),
        ("SELECT id FROM t_net WHERE v_macaddr = '08:00:2b:01:02:03'", true),
        ("SELECT id FROM t_net WHERE v_macaddr = '08:00:2B:01:02:03'", true),
        ("SELECT id FROM t_net WHERE v_macaddr8 > '08:00:2B:01:02:03:04:04'", true),
        ("SELECT id FROM t_oid WHERE v_oid > 2147483647", true),
        // No `v_time` case: the column holds `24:00:00`, which the side not
        // pushing it has to materialize and cannot (D96).
        ("SELECT id FROM t_time WHERE v_timetz = '24:00:00+00'", true),
    ];
    let mut misplaced = Vec::new();
    for (query, pushed) in cases {
        let plan = on.sql(query).await.unwrap().create_physical_plan().await.unwrap();
        let shown = displayable(plan.as_ref()).indent(true).to_string();
        if shown.contains("FilterExec") == pushed {
            misplaced.push(format!("{query} (pushed: {})", !pushed));
        }
        let got = on.sql(query).await.unwrap().collect().await.unwrap();
        let expected = off.sql(query).await.unwrap().collect().await.unwrap();
        let schema = plan.schema();
        assert_eq!(
            rows(&concat_batches(&schema, &got).unwrap()),
            rows(&concat_batches(&schema, &expected).unwrap()),
            "{query}"
        );
    }
    assert!(misplaced.is_empty(), "{misplaced:#?}");
}

/// A one-column table declaring `declared`, resolved as the provider
/// resolves it.
fn one_column(declared: &str) -> ResolvedSchema {
    let metadata = DumpMetadata {
        databases: vec![DatabaseMetadata {
            name: None,
            preamble_complete: true,
            server_version: None,
            pg_dump_version: None,
            extensions: Vec::new(),
            types: Vec::new(),
            collations: Vec::new(),
            tables: [("public.t".to_string(), vec![ColumnDef::new("v", declared)])]
                .into_iter()
                .collect(),
        }],
    };
    resolve_columns("public.t", &["v".to_string()], Some(&metadata), None, SchemaMode::Typed, &[])
}

/// **What registration announces about a nested column in DataFusion semantics
/// is what DataFusion does**: a NULL element orders first, and a float's `-0`
/// inside a list is below `0` and unequal to it — each note found on the
/// column, and its claim checked through `compare_op_for_nested`, which is
/// DataFusion's comparison of a list, on arrays of the type the column emits.
#[test]
fn a_nested_column_s_arrow_notes_are_datafusion_s_comparison() {
    let list = |resolved: &ResolvedSchema, values: ArrayRef| {
        let DataType::List(field) = resolved.schema.field(0).data_type() else {
            panic!("{resolved:?}")
        };
        let item = Arc::clone(field);
        ListArray::new(item, OffsetBuffer::from_lengths([values.len()]), values, None)
    };
    let announced = |resolved: &ResolvedSchema, divergence| {
        column_divergences(resolved, ComparisonSemantics::DataFusion)
            .iter()
            .any(|note| note.divergence == divergence)
    };

    let integers = one_column("integer[]");
    assert!(announced(&integers, ComparisonDivergence::NestedOrder));
    let with_null = list(&integers, Arc::new(Int32Array::from(vec![None])));
    let one = list(&integers, Arc::new(Int32Array::from(vec![Some(1)])));
    assert!(compare_op_for_nested(Operator::Lt, &with_null, &one).unwrap().value(0));

    let reals = one_column("real[]");
    assert!(announced(&reals, ComparisonDivergence::UnnormalizedZero));
    let negative = list(&reals, Arc::new(Float32Array::from(vec![-0.0])));
    let positive = list(&reals, Arc::new(Float32Array::from(vec![0.0])));
    assert!(compare_op_for_nested(Operator::Lt, &negative, &positive).unwrap().value(0));
    assert!(!compare_op_for_nested(Operator::Eq, &negative, &positive).unwrap().value(0));
}
