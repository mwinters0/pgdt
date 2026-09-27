//! **The translation only loosens**: every row DataFusion's evaluation of a
//! filter keeps, the library keeps under [`loosened`]'s tree — and exactly
//! those rows wherever every part of the filter has a library term. Checked
//! with generated filters over every major's fixtures, the library reading
//! each table through the replay a scan runs, its statistics gathered at a
//! group size that leaves pruning groups to skip, beside
//! `pgdump_query/tests/pruning.rs`'s check of what pruning keeps.
//!
//! **And a replay reading it as a dynamic filter loses no row it keeps**:
//! the same filters handed to the replay as a scan hands them
//! ([`ReplayFilter`]), over several sub-streams, from the start or only once
//! the first batch is out, skip only groups, sorted tails and rows none of
//! which DataFusion keeps, and emit no row twice.
//!
//! The filters are built of what the producers publish — comparisons with a
//! literal either side, `IS [NOT] NULL`, `[NOT] IN` lists holding a `NULL`
//! or not, `AND`, `OR`, `NOT`, a `CASE` over its branches, boolean literals
//! and a dynamic filter wrapping any of them, updated or still `empty` — and
//! of parts with no library term, standing in for `hash_lookup` and
//! `struct(…) IN`: one column compared with another, and an `IN` whose list
//! holds a column.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::compute::concat_batches;
use arrow::datatypes::{DataType, Schema};
use arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::physical_expr::expressions::{
    DynamicFilterPhysicalExpr, binary, in_list, is_not_null, is_null, lit, not,
};
use datafusion::physical_expr::utils::collect_columns;
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ComparisonSemantics, LocalFileSource, Parallelism, QueryOptions, ScanOptions,
    StatisticsRequest, StatisticsSelection, TableName, TablePartitions, map_file,
};

use super::*;
use crate::dump::{PgDump, PgDumpOptions};
use crate::table::PgDumpTable;

/// Generated filters per table.
const TREES_PER_TABLE: usize = 24;

/// How deep a generated filter nests.
const DEPTH: usize = 3;

/// The group size statistics are gathered at: tens of bytes, so a fixture
/// block is many groups and a translated filter has some to prune.
const TINY_GROUP: u64 = 32;

/// SplitMix64, seeded, so a failing filter is the same filter on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    let mut found = Vec::new();
    for major in ["13", "14", "15", "16", "17", "18"] {
        for dump in [
            "types/default.sql",
            "statistics/default.sql",
            "statistics/load-via-partition-root.sql",
            "edge_cases/default.sql",
        ] {
            found.push(root.join(major).join(dump));
        }
    }
    found
}

/// `fixture` copied into `dir` beside a complete cache holding every
/// statistic at [`TINY_GROUP`].
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    let request = StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(TINY_GROUP).unwrap()),
        ..StatisticsRequest::ALL
    };
    map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    copy
}

/// What a scan's replay returns of `table` under `filter`, in Arrow
/// semantics, projected to `projection`: every row as one batch (`None` for
/// none), or the refusal as text.
async fn read(
    dump: &PgDump,
    table: &TableName,
    projection: &Option<Vec<String>>,
    filter: L,
) -> std::result::Result<(Option<RecordBatch>, ResolvedSchema), String> {
    let options = QueryOptions {
        projection: projection.clone(),
        filter,
        semantics: ComparisonSemantics::Arrow,
        schema_mode: dump.schema_mode(),
        ..QueryOptions::default()
    };
    let partitions = TablePartitions::plan(
        Arc::clone(dump.source()),
        dump.index(),
        Arc::clone(dump.watch()),
        table,
        ScanOptions::default(),
        options,
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut batches = Vec::new();
    for partition in 0..partitions.len() {
        let mut stream = partitions.stream(partition, 1024);
        while let Some(batch) = stream.next().await {
            batches.push(batch.map_err(|e| e.to_string())?);
        }
    }
    let rows = batches.first().map(|first| concat_batches(&first.schema(), &batches).unwrap());
    Ok((rows, partitions.resolved_schema()))
}

/// The sub-streams [`read_dynamic`] asks for.
const DYNAMIC_JOBS: usize = 3;

/// What a replay read under a dynamic filter emitted: every row as one batch
/// (`None` for none), the groups it pruned, the rows it dropped, and whether
/// an early stop left a byte unread.
type ReadDynamic = (Option<RecordBatch>, u64, u64, bool);

/// What a scan's replay returns of `table`, projected to `projection`, under
/// no static filter and `tree` as a dynamic filter, over up to
/// [`DYNAMIC_JOBS`] sub-streams in batches of a few rows: the filter is
/// `empty` until the first batch is out unless `from_the_start`, as a join's
/// is complete at the probe's first poll and a TopK's tightens while the scan
/// streams.
async fn read_dynamic(
    dump: &PgDump,
    table: &TableName,
    projection: &Option<Vec<String>>,
    tree: &Arc<dyn PhysicalExpr>,
    from_the_start: bool,
) -> std::result::Result<ReadDynamic, String> {
    let options = QueryOptions {
        projection: projection.clone(),
        semantics: ComparisonSemantics::Arrow,
        schema_mode: dump.schema_mode(),
        parallelism: Parallelism::workers(DYNAMIC_JOBS, 1 << 30),
        ..QueryOptions::default()
    };
    let partitions = TablePartitions::plan(
        Arc::clone(dump.source()),
        dump.index(),
        Arc::clone(dump.watch()),
        table,
        ScanOptions::default(),
        options,
    )
    .await
    .map_err(|e| e.to_string())?;
    let children =
        collect_columns(tree).into_iter().map(|c| Arc::new(c) as Arc<dyn PhysicalExpr>).collect();
    let dynamic = Arc::new(DynamicFilterPhysicalExpr::new(children, lit(true)));
    if from_the_start {
        dynamic.update(Arc::clone(tree)).unwrap();
    }
    let held = DynamicFilters::default().with(vec![Arc::clone(&dynamic) as _]).unwrap();
    let filter: Arc<dyn DynamicFilter> =
        Arc::new(ReplayFilter::new(held, partitions.resolved_schema()));
    let partitions = Arc::new(partitions);
    let under = partitions.under(filter);
    let (mut batches, mut pruned, mut dropped, mut stopped) = (Vec::new(), 0, 0, false);
    for partition in 0..partitions.len() {
        let mut stream = under.stream(partition, 8);
        while let Some(batch) = stream.next().await {
            batches.push(batch.map_err(|e| e.to_string())?);
            if batches.len() == 1 && !from_the_start {
                dynamic.update(Arc::clone(tree)).unwrap();
            }
        }
        pruned += stream.dynamic_filter_pruned_groups();
        dropped += stream.dynamic_filter_pruned_rows();
        stopped |= stream.early_stops().iter().any(|stop| stop.unread_bytes.is_some());
    }
    let rows = batches.first().map(|first| concat_batches(&first.schema(), &batches).unwrap());
    Ok((rows, pruned, dropped, stopped))
}

/// Each row of `batch` as text, counted: the rows as a multiset.
fn rows(batch: &RecordBatch, keep: impl Fn(usize) -> bool) -> BTreeMap<String, usize> {
    let options = FormatOptions::default().with_null("NULL");
    let columns: Vec<_> = batch
        .columns()
        .iter()
        .map(|column| ArrayFormatter::try_new(column.as_ref(), &options).unwrap())
        .collect();
    let mut out = BTreeMap::new();
    for row in (0..batch.num_rows()).filter(|&row| keep(row)) {
        let text: Vec<String> = columns.iter().map(|c| c.value(row).to_string()).collect();
        *out.entry(text.join("|")).or_default() += 1;
    }
    out
}

/// A generated filter, and whether every part of it has a library term that
/// answers as DataFusion does, so its translation keeps exactly its rows.
#[derive(Clone)]
struct Tree {
    expr: Arc<dyn PhysicalExpr>,
    exact: bool,
}

/// Builds filters over one table's rows.
struct Generator<'a> {
    rng: &'a mut Rng,
    batch: &'a RecordBatch,
    resolved: &'a ResolvedSchema,
}

impl Generator<'_> {
    fn schema(&self) -> Arc<Schema> {
        self.batch.schema()
    }

    fn column(&self, index: usize) -> Arc<dyn PhysicalExpr> {
        Arc::new(Column::new(self.schema().field(index).name(), index))
    }

    /// The columns a comparison may name: the scalar ones.
    fn scalars(&self) -> Vec<usize> {
        (0..self.batch.num_columns())
            .filter(|&i| self.resolved.plans[i] == pgdump_query::NestedPlan::Scalar)
            .collect()
    }

    /// A value of column `index`, a `NULL` among them where the column holds
    /// one — as a join's build side or a TopK's heap hands the scan values
    /// of the column itself. A dictionary's is its own type or, as a join's
    /// bounds give it, the value's.
    fn value(&mut self, index: usize) -> ScalarValue {
        let array = self.batch.column(index);
        let value = ScalarValue::try_from_array(array, self.rng.below(array.len())).unwrap();
        match value {
            ScalarValue::Dictionary(_, inner) if self.rng.below(2) == 0 => *inner,
            value => value,
        }
    }

    /// Whether DataFusion evaluates `expr` over the table: a generated part
    /// it refuses is drawn again.
    fn evaluates(&self, expr: &Arc<dyn PhysicalExpr>) -> bool {
        expr.evaluate(self.batch).and_then(|v| v.into_array(self.batch.num_rows())).is_ok()
    }

    fn leaf(&mut self) -> Tree {
        for _ in 0..16 {
            if let Some(leaf) = self.draw_leaf()
                && self.evaluates(&leaf.expr)
            {
                return leaf;
            }
        }
        Tree { expr: lit(true), exact: true }
    }

    fn draw_leaf(&mut self) -> Option<Tree> {
        const COMPARING: [Operator; 8] = [
            Operator::Eq,
            Operator::NotEq,
            Operator::Lt,
            Operator::LtEq,
            Operator::Gt,
            Operator::GtEq,
            Operator::IsDistinctFrom,
            Operator::IsNotDistinctFrom,
        ];
        let schema = self.schema();
        let scalars = self.scalars();
        let exact = |expr| Some(Tree { expr, exact: true });
        let draw = self.rng.below(12);
        if scalars.is_empty() && !matches!(draw, 5 | 8) {
            return None;
        }
        match draw {
            // A column against a literal, either side first.
            // A float against a zero has no term ([`comparison`]).
            0..=4 => {
                let index = self.rng.pick(&scalars);
                let op = self.rng.pick(&COMPARING);
                let value = self.value(index);
                let zero = is_float(index, self.resolved) && is_zero(&value);
                let value = lit(value);
                let expr = if self.rng.below(2) == 0 {
                    binary(self.column(index), op, value, &schema)
                } else {
                    binary(value, op.swap()?, self.column(index), &schema)
                };
                Some(Tree { expr: expr.ok()?, exact: !zero })
            }
            5 => {
                let index = self.rng.below(self.batch.num_columns());
                let expr = if self.rng.below(2) == 0 {
                    is_null(self.column(index))
                } else {
                    is_not_null(self.column(index))
                };
                exact(expr.ok()?)
            }
            6 | 7 => {
                let index = self.rng.pick(&scalars);
                let list = (0..1 + self.rng.below(6)).map(|_| lit(self.value(index))).collect();
                let negated = self.rng.below(3) == 0;
                let expr = in_list(self.column(index), list, &negated, &schema).ok()?;
                Some(Tree { expr, exact: !is_float(index, self.resolved) })
            }
            8 => exact(lit(match self.rng.below(3) {
                0 => ScalarValue::Boolean(Some(true)),
                1 => ScalarValue::Boolean(Some(false)),
                _ => ScalarValue::Boolean(None),
            })),
            9 => {
                let booleans: Vec<usize> = (0..self.batch.num_columns())
                    .filter(|&i| schema.field(i).data_type() == &DataType::Boolean)
                    .collect();
                if booleans.is_empty() {
                    return None;
                }
                let index = self.rng.pick(&booleans);
                exact(self.column(index))
            }
            // No library term: one column against another of its type, or
            // a list holding one.
            _ => {
                let a = self.rng.pick(&scalars);
                let alike: Vec<usize> = scalars
                    .iter()
                    .copied()
                    .filter(|&b| schema.field(b).data_type() == schema.field(a).data_type())
                    .collect();
                let b = self.rng.pick(&alike);
                let expr = if self.rng.below(2) == 0 {
                    binary(self.column(a), self.rng.pick(&COMPARING), self.column(b), &schema)
                } else {
                    let negated = self.rng.below(2) == 0;
                    let list = vec![self.column(b), lit(self.value(a))];
                    in_list(self.column(a), list, &negated, &schema)
                };
                Some(Tree { expr: expr.ok()?, exact: false })
            }
        }
    }

    fn tree(&mut self, depth: usize) -> Tree {
        let schema = self.schema();
        match if depth == 0 { 0 } else { self.rng.below(8) } {
            0 | 1 => self.leaf(),
            op @ (2 | 3) => {
                let (left, right) = (self.tree(depth - 1), self.tree(depth - 1));
                let op = if op == 2 { Operator::And } else { Operator::Or };
                Tree {
                    expr: binary(left.expr, op, right.expr, &schema).unwrap(),
                    exact: left.exact && right.exact,
                }
            }
            4 => {
                let inner = self.tree(depth - 1);
                Tree { expr: not(inner.expr).unwrap(), exact: inner.exact }
            }
            // A `CASE`, over any conditions — a partitioned join's routes
            // each row by a hash no library term reads.
            5 if !self.scalars().is_empty() => {
                let branches = 1 + self.rng.below(3);
                let base = self.rng.below(2) == 0;
                let scalars = self.scalars();
                let index = self.rng.pick(&scalars);
                let when_then = (0..branches)
                    .map(|_| {
                        let when =
                            if base { lit(self.value(index)) } else { self.tree(depth - 1).expr };
                        (when, self.tree(depth - 1).expr)
                    })
                    .collect();
                let otherwise = (self.rng.below(3) > 0).then(|| self.tree(depth - 1).expr);
                let base = base.then(|| self.column(index));
                match CaseExpr::try_new(base, when_then, otherwise) {
                    Ok(case) => {
                        let expr: Arc<dyn PhysicalExpr> = Arc::new(case);
                        if self.evaluates(&expr) {
                            Tree { expr, exact: false }
                        } else {
                            self.leaf()
                        }
                    }
                    Err(_) => self.leaf(),
                }
            }
            5 => self.leaf(),
            // A dynamic filter, updated to a tree or still `empty`.
            _ => {
                let inner = self.tree(depth - 1);
                let children = collect_columns(&inner.expr)
                    .into_iter()
                    .map(|c| Arc::new(c) as Arc<dyn PhysicalExpr>)
                    .collect();
                let filter = DynamicFilterPhysicalExpr::new(children, lit(true));
                if self.rng.below(4) == 0 {
                    return Tree { expr: Arc::new(filter), exact: true };
                }
                filter.update(inner.expr).unwrap();
                Tree { expr: Arc::new(filter), exact: inner.exact }
            }
        }
    }
}

/// What the generated check saw, so a sweep that compared nothing fails.
#[derive(Debug, Default)]
struct Tally {
    compared: usize,
    exact: usize,
    /// Filters whose translation kept fewer rows than the table holds.
    narrowed: usize,
    /// Filters whose translation kept rows DataFusion's evaluation did not.
    loosened: usize,
    /// Replays under a dynamic filter that pruned a group.
    dynamic_pruning: usize,
    /// Groups those replays pruned.
    dynamic_groups: u64,
    /// Replays under a dynamic filter that dropped a row of a group they read.
    dynamic_dropping: usize,
    /// Replays under a dynamic filter that an early stop ended.
    dynamic_stopping: usize,
}

/// **Every generated filter's translation keeps every row DataFusion keeps,
/// and no other where the filter is exact**, over every table of every
/// fixture, and none is refused by the library's plan. **Read as a dynamic
/// filter, from the start or once a batch is out, it loses none of those
/// rows either, and emits no row the table does not hold as often.**
#[tokio::test(flavor = "multi_thread")]
async fn a_translated_dynamic_filter_keeps_every_row_datafusion_keeps() {
    let dir = tempfile::tempdir().unwrap();
    let mut rng = Rng(0x5eed_d15c);
    let (mut tally, mut failures) = (Tally::default(), Vec::new());
    for fixture in fixtures() {
        let copy = parsed_copy(&fixture, dir.path()).await;
        let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
        for name in dump.tables() {
            // Only the columns every row decodes are read, so a value the
            // build cannot hold (`KD8`) ends no comparison.
            let mut projection = None;
            let mut whole = read(&dump, name, &projection, L::default()).await;
            if whole.is_err() {
                let table = PgDumpTable::build(Arc::clone(&dump), name.clone()).unwrap();
                let mut decodes = Vec::new();
                for field in table.resolved_schema().schema.fields() {
                    let one = Some(vec![field.name().clone()]);
                    if read(&dump, name, &one, L::default()).await.is_ok() {
                        decodes.push(field.name().clone());
                    }
                }
                projection = Some(decodes);
                whole = read(&dump, name, &projection, L::default()).await;
            }
            let (Some(batch), resolved) = whole.unwrap() else { continue };
            if batch.num_columns() == 0 {
                continue;
            }
            let everything = rows(&batch, |_| true);
            let mut generator = Generator { rng: &mut rng, batch: &batch, resolved: &resolved };
            let trees: Vec<Tree> = (0..TREES_PER_TABLE).map(|_| generator.tree(DEPTH)).collect();
            for tree in trees {
                let at = format!("{} {}: {}", fixture.display(), name.qualified(), tree.expr);
                let kept = tree.expr.evaluate(&batch).unwrap();
                let kept = kept.into_array(batch.num_rows()).unwrap();
                let kept = kept.as_boolean();
                let expected = rows(&batch, |row| kept.is_valid(row) && kept.value(row));
                let translated = loosened(&tree.expr, &resolved);
                let got = match read(&dump, name, &projection, translated.clone()).await {
                    Ok((got, _)) => got.map(|got| rows(&got, |_| true)).unwrap_or_default(),
                    Err(e) => {
                        failures.push(format!("{at}\n  refused: {e}\n  as {translated:?}"));
                        continue;
                    }
                };
                let lost: Vec<_> =
                    expected.iter().filter(|(row, n)| got.get(*row) < Some(n)).collect();
                if !lost.is_empty() {
                    failures.push(format!("{at}\n  lost {lost:?}\n  as {translated:?}"));
                } else if tree.exact && got != expected {
                    failures.push(format!("{at}\n  kept more than exact\n  as {translated:?}"));
                }
                tally.compared += 1;
                tally.exact += usize::from(tree.exact);
                tally.narrowed += usize::from(got != everything);
                tally.loosened += usize::from(got != expected);

                let from_the_start = rng.below(2) == 0;
                let dynamic =
                    read_dynamic(&dump, name, &projection, &tree.expr, from_the_start).await;
                let (got, pruned, dropped, stopped) = match dynamic {
                    Ok((got, pruned, dropped, stopped)) => {
                        let got = got.map(|got| rows(&got, |_| true)).unwrap_or_default();
                        (got, pruned, dropped, stopped)
                    }
                    Err(e) => {
                        failures.push(format!("{at}\n  refused as a dynamic filter: {e}"));
                        continue;
                    }
                };
                let when = if from_the_start { "from the start" } else { "after a batch" };
                let lost: Vec<_> =
                    expected.iter().filter(|(row, n)| got.get(*row) < Some(n)).collect();
                if !lost.is_empty() {
                    failures.push(format!("{at}\n  dynamic {when} lost {lost:?}"));
                }
                let extra: Vec<_> =
                    got.iter().filter(|(row, n)| everything.get(*row) < Some(n)).collect();
                if !extra.is_empty() {
                    failures.push(format!("{at}\n  dynamic {when} emitted {extra:?}"));
                }
                tally.dynamic_pruning += usize::from(pruned > 0);
                tally.dynamic_groups += pruned;
                tally.dynamic_dropping += usize::from(dropped > 0);
                tally.dynamic_stopping += usize::from(stopped);
            }
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(tally.compared > 5_000, "{tally:?}");
    assert!(tally.exact > tally.compared / 3, "{tally:?}");
    assert!(tally.narrowed > tally.compared / 3, "{tally:?}");
    assert!(tally.loosened > 50, "{tally:?}");
    assert!(tally.dynamic_pruning > tally.compared / 10, "{tally:?}");
    assert!(tally.dynamic_dropping > tally.compared / 10, "{tally:?}");
    assert!(tally.dynamic_stopping > 10, "{tally:?}");
}

/// A one-table schema as the provider resolves it, for the shapes below.
fn resolved(fields: Vec<(&str, DataType, bool)>) -> ResolvedSchema {
    let schema = Arc::new(Schema::new(
        fields
            .into_iter()
            .map(|(name, data_type, nullable)| {
                arrow::datatypes::Field::new(name, data_type, nullable)
            })
            .collect::<Vec<_>>(),
    ));
    let n = schema.fields().len();
    ResolvedSchema {
        schema,
        columns: vec![pgdump_query::ColumnResolution::NotDeclared; n],
        notes: Vec::new(),
        plans: vec![pgdump_query::NestedPlan::Scalar; n],
        comparisons: vec![pgdump_query::ComparisonPlan::Refused; n],
    }
}

/// **A TopK whose heap admits no row rules out every row, and a filter not
/// yet updated keeps every row.**
#[test]
fn a_filter_that_admits_nothing_rules_out_every_row() {
    let table = resolved(vec![("k", DataType::Int32, true)]);
    let k: Arc<dyn PhysicalExpr> = Arc::new(Column::new("k", 0));
    let topk = DynamicFilterPhysicalExpr::new(vec![Arc::clone(&k)], lit(true));
    let topk: Arc<dyn PhysicalExpr> = Arc::new(topk);
    assert_eq!(format!("{:?}", loosened(&topk, &table)), "And([])");
    topk.downcast_ref::<DynamicFilterPhysicalExpr>().unwrap().update(lit(false)).unwrap();
    assert_eq!(format!("{:?}", loosened(&topk, &table)), "Or([])");
}

/// **A part with no term keeps every row wherever it sits**, `true` where it
/// is not negated and `false` where it is, and a column read by an index
/// whose field is another's stands for nothing.
#[test]
fn a_part_with_no_term_keeps_every_row_wherever_it_sits() {
    let schema = Schema::new(vec![
        arrow::datatypes::Field::new("a", DataType::Int32, true),
        arrow::datatypes::Field::new("b", DataType::Int32, true),
    ]);
    let table = resolved(vec![("a", DataType::Int32, true), ("b", DataType::Int32, true)]);
    let (a, b): (Arc<dyn PhysicalExpr>, Arc<dyn PhysicalExpr>) =
        (Arc::new(Column::new("a", 0)), Arc::new(Column::new("b", 1)));
    let opaque = binary(Arc::clone(&a), Operator::Eq, Arc::clone(&b), &schema).unwrap();
    assert_eq!(format!("{:?}", loosened(&opaque, &table)), "And([])");
    let negated = not(Arc::clone(&opaque)).unwrap();
    assert_eq!(format!("{:?}", loosened(&negated, &table)), "Not(Or([]))");
    let twice = not(Arc::clone(&negated)).unwrap();
    assert_eq!(format!("{:?}", loosened(&twice, &table)), "Not(Not(And([])))");
    let misnamed: Arc<dyn PhysicalExpr> = Arc::new(Column::new("b", 0));
    let misread = binary(misnamed, Operator::Eq, lit(1i32), &schema).unwrap();
    assert_eq!(format!("{:?}", loosened(&misread, &table)), "And([])");
}

/// **A float compared with a zero of either sign has no term**, beneath
/// either parity and with the literal on either side, where the same
/// comparison with any other value has one ([`comparison`]).
#[test]
fn a_float_compared_with_a_zero_has_no_term() {
    let schema = Schema::new(vec![
        arrow::datatypes::Field::new("f", DataType::Float64, true),
        arrow::datatypes::Field::new("g", DataType::Float32, true),
    ]);
    let table = resolved(vec![("f", DataType::Float64, true), ("g", DataType::Float32, true)]);
    let (f, g): (Arc<dyn PhysicalExpr>, Arc<dyn PhysicalExpr>) =
        (Arc::new(Column::new("f", 0)), Arc::new(Column::new("g", 1)));
    let above = binary(Arc::clone(&f), Operator::Gt, lit(-0.0f64), &schema).unwrap();
    assert_eq!(format!("{:?}", loosened(&above, &table)), "And([])");
    let below = binary(Arc::clone(&f), Operator::Lt, lit(0.0f64), &schema).unwrap();
    let not_below = not(below).unwrap();
    assert_eq!(format!("{:?}", loosened(&not_below, &table)), "Not(Or([]))");
    let reversed = binary(lit(-0.0f32), Operator::Eq, Arc::clone(&g), &schema).unwrap();
    assert_eq!(format!("{:?}", loosened(&reversed, &table)), "And([])");
    for value in [lit(1.0f64), lit(-0.5f64)] {
        let compared = binary(Arc::clone(&f), Operator::Gt, value, &schema).unwrap();
        assert!(matches!(loosened(&compared, &table), L::Term(_)), "{compared}");
    }
    let compared = binary(lit(1.0f32), Operator::Eq, Arc::clone(&g), &schema).unwrap();
    assert!(matches!(loosened(&compared, &table), L::Term(_)), "{compared}");
}

/// **Both translators hand an `IN` list over as one membership**, a `NULL`
/// in it included and `NOT IN` as its negation (`docs/design/decisions.md`,
/// "D53"); what each answers is the generated checks' — this one's and
/// `tests/pushdown.rs`'s.
#[test]
fn an_in_list_reaches_the_library_as_one_membership() {
    use datafusion::logical_expr::{col, lit as logical};
    let schema = Schema::new(vec![
        arrow::datatypes::Field::new("k", DataType::Int32, true),
        arrow::datatypes::Field::new("f", DataType::Float64, true),
    ]);
    let table = resolved(vec![("k", DataType::Int32, true), ("f", DataType::Float64, true)]);
    let (k, f): (Arc<dyn PhysicalExpr>, Arc<dyn PhysicalExpr>) =
        (Arc::new(Column::new("k", 0)), Arc::new(Column::new("f", 1)));
    let values = || vec![lit(1i32), lit(ScalarValue::Int32(None)), lit(3i32)];
    let membership = r#"In(Membership { column: "k", values: [Some("1"), None, Some("3")] })"#;
    let listed = in_list(Arc::clone(&k), values(), &false, &schema).unwrap();
    assert_eq!(format!("{:?}", loosened(&listed, &table)), membership);
    let excluded = in_list(Arc::clone(&k), values(), &true, &schema).unwrap();
    assert_eq!(format!("{:?}", loosened(&excluded, &table)), format!("Not({membership})"));
    // A float's `NOT IN` has no term, DataFusion's set keeping `-0` from `0`.
    let floats = vec![lit(1.0f64), lit(2.0f64)];
    let excluded = in_list(Arc::clone(&f), floats, &true, &schema).unwrap();
    assert_eq!(format!("{:?}", loosened(&excluded, &table)), "Not(Or([]))");

    let listed = col("k").in_list(
        vec![logical(1i32), logical(ScalarValue::Int32(None)), logical(3i32), logical(4i32)],
        false,
    );
    let translated = crate::pushdown::translate(&listed, &table).unwrap();
    assert_eq!(
        format!("{translated:?}"),
        r#"In(Membership { column: "k", values: [Some("1"), None, Some("3"), Some("4")] })"#
    );
    let excluded = col("k").in_list(vec![logical(1i32), logical(2i32)], true);
    let translated = crate::pushdown::translate(&excluded, &table).unwrap();
    assert!(matches!(translated, L::Not(inner) if matches!(*inner, L::In(_))), "{excluded}");
}
