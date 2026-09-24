//! The statistics the provider hands DataFusion, against the answers a scan
//! gives (`docs/design/decisions.md`, "D89").
//!
//! **Each answer has its own oracle**, over the generated fixtures, and the
//! plan shape it changes is asserted beside it:
//!
//! - **An aggregate answered from statistics** is the same query with the
//!   rule that reads them taken out. DataFusion answers `COUNT(*)`,
//!   `COUNT(<column>)`, `COUNT(DISTINCT <column>)`, `MIN`, `MAX` and `SUM`
//!   from `Exact` statistics by replacing the aggregate with a literal, so a
//!   wrong `Exact` is a wrong answer with no error; a session whose physical
//!   optimizer does not carry `aggregate_statistics` reads every row instead,
//!   and the two must agree wherever reading the column answers at all. Where
//!   a typed read refuses (`KD8`), a count is still checked: read as text, no
//!   value refuses, and a `COUNT(<column>)` counts values without their
//!   meaning.
//! - **An estimate** — a row count, a byte size — is checked as the bound it
//!   claims to be: never below what the scan emits, over generated filters,
//!   and equal to it wherever it says `Exact`.
//! - **A declared ordering** has no blind session, since a wrong one changes
//!   the plan rather than a literal: every partition's rows are checked
//!   sorted under Arrow's own comparator, NULL placement included, and a sort
//!   on the column is checked to be gone exactly where one is declared.
//! - **A join's build side** is read off the plan, its answer off the blind
//!   session.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::datatypes::DataType;
use arrow::row::{RowConverter, SortField};
use arrow::util::display::{ArrayFormatter, FormatOptions};
use arrow::util::pretty::pretty_format_batches;
use datafusion::catalog::TableProvider;
use datafusion::common::stats::Precision;
use datafusion::common::{Column, ScalarValue, Statistics};
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::logical_expr::{BinaryExpr, Expr, Operator, TableProviderFilterPushDown};
use datafusion::physical_expr::LexOrdering;
use datafusion::physical_expr::expressions::Column as ColumnExpr;
use datafusion::physical_plan::joins::{HashJoinExec, PartitionMode};
use datafusion::physical_plan::metrics::MetricValue;
use datafusion::physical_plan::statistics::{StatisticsArgs, StatisticsContext};
use datafusion::physical_plan::{ExecutionPlan, displayable, execute_stream_partitioned};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions, PgDumpTable, register_dump};
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    Finding, LocalFileSource, NestedPlan, ScanOptions, SchemaMode, StatisticsRequest,
    StatisticsSelection, TableName, map_file,
};

/// A sink for a registration whose findings this target is not about.
fn ignore(_: &dyn Finding) {}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// Every generated dump: `fixtures/<major>/<schema>/<flag-set>.sql`.
fn every_fixture() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for major in std::fs::read_dir(fixtures_root()).unwrap() {
        let major = major.unwrap().path();
        if !major.is_dir() {
            continue;
        }
        for schema in std::fs::read_dir(&major).unwrap() {
            let schema = schema.unwrap().path();
            if !schema.is_dir() {
                continue;
            }
            for dump in std::fs::read_dir(&schema).unwrap() {
                let dump = dump.unwrap().path();
                if dump.extension().is_some_and(|e| e == "sql") {
                    found.push(dump);
                }
            }
        }
    }
    found.sort();
    assert!(!found.is_empty(), "no fixtures under {}", fixtures_root().display());
    found
}

/// Every `default.sql`, and every flag set of the `statistics` schema — the
/// one whose tables are several blocks under `--load-via-partition-root`. What
/// the estimate and ordering targets plan over: each executes every table,
/// where the aggregate target plans over [`every_fixture`].
fn planned_fixtures() -> Vec<PathBuf> {
    every_fixture()
        .into_iter()
        .filter(|dump| {
            dump.file_stem().is_some_and(|stem| stem == "default")
                || dump.parent().is_some_and(|schema| schema.ends_with("statistics"))
        })
        .collect()
}

/// `fixture` copied into `dir` beside the complete cache a gathering parse
/// leaves, so the committed tree is never written into and the map carries
/// the statistics this target is about.
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    parsed_copy_gathering(fixture, dir, &StatisticsRequest::ALL).await
}

/// A group size several rows of every fixture table long, so a pushed filter
/// keeps some of a block's groups and not others.
const SMALL_GROUP: u64 = 1024;

/// [`parsed_copy`], gathering every statistic at [`SMALL_GROUP`].
async fn parsed_copy_in_small_groups(fixture: &Path, dir: &Path) -> PathBuf {
    let request = StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(SMALL_GROUP).unwrap()),
        ..StatisticsRequest::ALL
    };
    parsed_copy_gathering(fixture, dir, &request).await
}

async fn parsed_copy_gathering(fixture: &Path, dir: &Path, request: &StatisticsRequest) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, request).await.unwrap();
    copy
}

/// A session that reads the provider's statistics, and one that cannot.
fn sessions() -> (SessionContext, SessionContext) {
    sessions_at(2)
}

/// [`sessions`], planning `partitions` partitions to a scan.
fn sessions_at(partitions: usize) -> (SessionContext, SessionContext) {
    let config = SessionConfig::new().with_target_partitions(partitions).with_batch_size(64);
    let reading = SessionContext::new_with_config(config.clone());
    let state = reading.state();
    let all = state.physical_optimizers();
    let rules: Vec<_> =
        all.iter().filter(|rule| rule.name() != "aggregate_statistics").cloned().collect();
    assert_eq!(
        rules.len() + 1,
        all.len(),
        "`aggregate_statistics` is the rule that reads a source's statistics"
    );
    let blind = SessionContext::new_with_state(
        SessionStateBuilder::new_with_default_features()
            .with_config(config)
            .with_physical_optimizer_rules(rules)
            .build(),
    );
    (reading, blind)
}

/// A SQL identifier, quoted.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn from(catalog: &str, table: &TableName) -> String {
    format!(
        "{}.{}.{}",
        quoted(catalog),
        quoted(table.schema.as_deref().unwrap_or(datafusion_pgdump::UNQUALIFIED_SCHEMA)),
        quoted(&table.table)
    )
}

/// `dump` registered in both sessions, and the catalog each database took.
fn register_in(
    sessions: (&SessionContext, &SessionContext),
    dump: &Arc<PgDump>,
) -> Vec<(Option<String>, String)> {
    let databases = dump.databases();
    let name = matches!(databases.as_slice(), [None] | []).then_some("dump");
    let catalogs = register_dump(sessions.0, name, dump, Arc::new(ignore)).unwrap();
    register_dump(sessions.1, name, dump, Arc::new(ignore)).unwrap();
    databases.into_iter().zip(catalogs).collect()
}

async fn rows(ctx: &SessionContext, sql: &str) -> Result<Vec<RecordBatch>, String> {
    match ctx.sql(sql).await {
        Ok(frame) => frame.collect().await.map_err(|e| e.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

fn rendered(batches: &Result<Vec<RecordBatch>, String>) -> String {
    match batches {
        Ok(batches) => pretty_format_batches(batches).unwrap().to_string(),
        Err(err) => format!("refused: {err}"),
    }
}

/// The aggregate's answer with and without the statistics, and a note of
/// whether the statistics actually answered it. `text` is a session reading
/// the same dump as text without its statistics, the oracle for a count over
/// a column the typed read refuses.
async fn agrees(
    reading: &SessionContext,
    blind: &SessionContext,
    text: Option<&SessionContext>,
    sql: &str,
) -> Answer {
    let from_statistics = rows(reading, sql).await;
    let from_rows = rows(blind, sql).await;
    let plan = reading.sql(sql).await.unwrap().create_physical_plan().await.unwrap();
    let answered = !datafusion::physical_plan::displayable(plan.as_ref())
        .indent(false)
        .to_string()
        .contains("PgDumpExec");
    // The one divergence the phase allows: a column holding a value its Arrow
    // type cannot represent (`KD8`) refuses when it is read, and the map
    // answers over it without reading it — the trade a pruned replay already
    // makes (`docs/design/decisions.md`, "D54").
    let unreadable = matches!(&from_rows, Err(err) if err.contains("does not parse as its mapped"));
    if unreadable && answered {
        assert!(
            from_statistics.is_ok(),
            "`{sql}` was answered from statistics and still refused: {}",
            rendered(&from_statistics)
        );
        // A distinct count read as text counts spellings, not values.
        if !sql.starts_with("SELECT COUNT(") || sql.starts_with("SELECT COUNT(DISTINCT") {
            return Answer::FromStatistics;
        }
        let text = text.unwrap_or_else(|| panic!("`{sql}` refused with no text session to count"));
        assert_eq!(
            rendered(&from_statistics),
            rendered(&rows(text, sql).await),
            "`{sql}` counted differently from the statistics than read as text"
        );
        return Answer::CountedAsText;
    }
    // `KD42`: a float's bound gathered in PostgreSQL's order, handed over
    // `Exact` as the other zero. Named rather than tolerated: the target
    // asserts `public.zeros` reaches it, so the change closing it deletes this
    // arm.
    if answered && the_other_zero(&from_statistics, &from_rows) {
        return Answer::OtherZero;
    }
    assert_eq!(
        rendered(&from_statistics),
        rendered(&from_rows),
        "`{sql}` answered differently with the statistics than without them"
    );
    if answered { Answer::FromStatistics } else { Answer::FromRows }
}

/// Whether two one-value answers are a float's two zeros.
fn the_other_zero(
    a: &Result<Vec<RecordBatch>, String>,
    b: &Result<Vec<RecordBatch>, String>,
) -> bool {
    let value = |answer: &Result<Vec<RecordBatch>, String>| {
        let batches = answer.as_ref().ok()?;
        let batch = batches.iter().find(|batch| batch.num_rows() > 0)?;
        ScalarValue::try_from_array(batch.column(0), 0).ok()
    };
    let zero = |value: Option<ScalarValue>| match value? {
        ScalarValue::Float64(Some(v)) if v == 0.0 => Some(v.is_sign_negative()),
        ScalarValue::Float32(Some(v)) if v == 0.0 => Some(v.is_sign_negative()),
        _ => None,
    };
    matches!((zero(value(a)), zero(value(b))), (Some(x), Some(y)) if x != y)
}

/// Where an aggregate's answer came from, and what it was checked against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    /// The plan read the rows; nothing was asked of the statistics.
    FromRows,
    /// The statistics answered, and the rows agreed or could not be read.
    FromStatistics,
    /// The statistics counted a column the typed read refuses, and the count
    /// read as text agreed.
    CountedAsText,
    /// The statistics answered a float's extreme with the other zero (`KD42`).
    OtherZero,
}

impl Answer {
    fn answered(self) -> bool {
        self != Answer::FromRows
    }
}

/// Which of the statistics' answers one schema mode's pass saw, so a build
/// that hands none over fails rather than passing vacuously.
#[derive(Default, Debug)]
struct Seen {
    count: bool,
    null_count: bool,
    bound: bool,
    distinct: bool,
    counted_as_text: bool,
    other_zero: bool,
}

impl Seen {
    fn or(self, other: Seen) -> Seen {
        Seen {
            count: self.count || other.count,
            null_count: self.null_count || other.null_count,
            bound: self.bound || other.bound,
            distinct: self.distinct || other.distinct,
            counted_as_text: self.counted_as_text || other.counted_as_text,
            other_zero: self.other_zero || other.other_zero,
        }
    }
}

/// `fixtures` grouped by major, each group checked by `check` on a thread of
/// its own, so a sweep over six majors takes about one major's time. Each
/// thread runs one current-thread runtime, as `#[tokio::test]` does, so the
/// partitions of a query that refuses are read in one order and it refuses on
/// the same row with the statistics as without them.
fn per_major<T, F, Fut>(fixtures: Vec<PathBuf>, check: F) -> Vec<T>
where
    F: Fn(Vec<PathBuf>) -> Fut + Sync,
    Fut: Future<Output = T>,
    T: Send,
{
    let mut majors: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for fixture in fixtures {
        let major = fixture.parent().and_then(Path::parent).unwrap().to_path_buf();
        majors.entry(major).or_default().push(fixture);
    }
    let check = &check;
    std::thread::scope(|scope| {
        let workers: Vec<_> = majors
            .into_values()
            .map(|group| {
                scope.spawn(move || {
                    let runtime =
                        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                    runtime.block_on(check(group))
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    })
}

/// `dump` opened in `schema_mode` and registered in a reading and a blind
/// session.
async fn opened(
    copy: &Path,
    schema_mode: SchemaMode,
) -> (Arc<PgDump>, SessionContext, SessionContext, Vec<(Option<String>, String)>) {
    let options = PgDumpOptions { schema_mode, ..PgDumpOptions::default() };
    let dump = PgDump::open(copy.to_str().unwrap(), options).await.unwrap();
    let (reading, blind) = sessions();
    let catalogs = register_in((&reading, &blind), &dump);
    (dump, reading, blind, catalogs)
}

/// Every table's `COUNT(*)`, and every column's `COUNT`, `MIN` and `MAX`, of
/// `dump` asked of both sessions.
async fn every_aggregate(
    dump: &Arc<PgDump>,
    (reading, blind): (&SessionContext, &SessionContext),
    catalogs: &[(Option<String>, String)],
    text: &SessionContext,
    seen: &mut Seen,
) {
    for table in dump.tables() {
        let catalog =
            catalogs.iter().find(|(database, _)| *database == table.database).unwrap().1.clone();
        let from = from(&catalog, table);
        seen.count |= agrees(reading, blind, Some(text), &format!("SELECT COUNT(*) FROM {from}"))
            .await
            .answered();
        let provider = dump.table(table.database.as_deref(), None, &table.table).unwrap();
        for field in provider.resolved_schema().schema.fields() {
            let column = quoted(field.name());
            // `COUNT(c)` is the table's rows less the column's NULLs, so it
            // reads the null count rather than a bound.
            let sql = format!("SELECT COUNT({column}) FROM {from}");
            let answer = agrees(reading, blind, Some(text), &sql).await;
            seen.null_count |= answer.answered();
            seen.counted_as_text |= answer == Answer::CountedAsText;
            // DataFusion plans no `MIN`/`MAX` over a list or a struct, in
            // either session, so there is nothing to compare.
            if matches!(
                field.data_type(),
                DataType::List(_) | DataType::LargeList(_) | DataType::Struct(_)
            ) {
                continue;
            }
            for op in ["MIN", "MAX"] {
                let sql = format!("SELECT {op}({column}) FROM {from}");
                let answer = agrees(reading, blind, Some(text), &sql).await;
                seen.bound |= answer.answered();
                seen.other_zero |= answer == Answer::OtherZero;
            }
            // Asked of every column the rows can answer it for, statistics or
            // none: a `SUM` is answered from an `Exact` sum, a distinct count
            // from an `Exact` distinct count. A lone `COUNT(DISTINCT c)` is
            // planned as a count over `GROUP BY c`, whose rows are never
            // `Exact`, so it is asked beside `COUNT(*)`, which keeps it an
            // aggregate of its own.
            let sql = format!("SELECT COUNT(DISTINCT {column}), COUNT(*) FROM {from}");
            seen.distinct |= agrees(reading, blind, Some(text), &sql).await.answered();
            if field.data_type().is_numeric() {
                let sql = format!("SELECT SUM({column}) FROM {from}");
                agrees(reading, blind, Some(text), &sql).await;
            }
        }
    }
}

/// **Typed and as text**, as the provider's other targets are: the text pass
/// is the typed one's oracle wherever a typed read refuses (`KD8`), and is
/// itself read against the rows, bounds included (D89).
#[test]
fn statistics_never_change_an_answer() {
    let seen = per_major(every_fixture(), |fixtures| async move {
        let scratch = tempfile::tempdir().unwrap();
        let (mut typed, mut strings) = (Seen::default(), Seen::default());
        for fixture in fixtures {
            let copy = parsed_copy(&fixture, scratch.path()).await;
            let (text_dump, text_reading, text_blind, text_catalogs) =
                opened(&copy, SchemaMode::Strings).await;
            let (dump, reading, blind, catalogs) = opened(&copy, SchemaMode::Typed).await;
            every_aggregate(&dump, (&reading, &blind), &catalogs, &text_blind, &mut typed).await;
            every_aggregate(
                &text_dump,
                (&text_reading, &text_blind),
                &text_catalogs,
                &text_blind,
                &mut strings,
            )
            .await;
        }
        (typed, strings)
    });
    let (typed, strings) =
        seen.into_iter().fold((Seen::default(), Seen::default()), |(typed, strings), (t, s)| {
            (typed.or(t), strings.or(s))
        });
    for (mode, seen) in [(SchemaMode::Typed, &typed), (SchemaMode::Strings, &strings)] {
        assert!(seen.count, "no `COUNT(*)` was answered from the statistics ({mode:?})");
        assert!(
            seen.null_count,
            "no `COUNT(<column>)` was answered from the statistics ({mode:?})"
        );
        assert!(seen.bound, "no `MIN`/`MAX` was answered from the statistics ({mode:?})");
        assert!(
            seen.distinct,
            "no `COUNT(DISTINCT <column>)` was answered from the statistics ({mode:?})"
        );
    }
    assert!(
        typed.counted_as_text,
        "no typed `COUNT(<column>)` over a refusing column was checked against the text"
    );
    assert!(typed.other_zero, "`public.zeros` no longer reaches `KD42`: {typed:?}");
    assert!(!strings.counted_as_text, "a text read refused: {strings:?}");
}

/// A filter cuts rows out of the scan, so the table's own counts and extremes
/// stop describing what it emits and nothing may be answered from them.
#[tokio::test]
async fn a_filtered_scan_answers_nothing_from_the_table_s_statistics() {
    let scratch = tempfile::tempdir().unwrap();
    let fixture = fixtures_root().join("16/types/default.sql");
    let copy = parsed_copy(&fixture, scratch.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let (reading, blind) = sessions();
    register_in((&reading, &blind), &dump);
    let unfiltered = "SELECT COUNT(*), MIN(id), MAX(id) FROM \"dump\".public.t_int";
    assert!(
        agrees(&reading, &blind, None, unfiltered).await.answered(),
        "the unfiltered aggregate is the control, and it must answer from the statistics"
    );
    for filtered in
        [format!("{unfiltered} WHERE id > 0"), format!("{unfiltered} WHERE v_integer < 100")]
    {
        assert!(
            !agrees(&reading, &blind, None, &filtered).await.answered(),
            "`{filtered}` was answered from statistics describing the unfiltered table"
        );
    }
}

/// A fetch cuts rows off the scan the same way. Asked of the plan rather than
/// through SQL: DataFusion keeps a limit node of its own above the scan and
/// would hide a scan that answered wrongly, and `scan`'s own `limit` argument
/// is what this pins.
#[tokio::test]
async fn a_fetched_or_filtered_plan_states_no_exact_statistic() {
    use datafusion::catalog::TableProvider;
    use datafusion::common::stats::Precision;
    use datafusion::physical_plan::statistics::{StatisticsArgs, StatisticsContext};
    use datafusion::prelude::{col, lit};

    let scratch = tempfile::tempdir().unwrap();
    let fixture = fixtures_root().join("16/types/default.sql");
    let copy = parsed_copy(&fixture, scratch.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let (reading, _blind) = sessions();
    let table = dump.table(None, None, "t_int").unwrap();
    let state = reading.state();
    let statistics = async |limit, filters: Vec<datafusion::prelude::Expr>| {
        let plan = table.scan(&state, None, &filters, limit).await.unwrap();
        StatisticsContext::new().compute(plan.as_ref(), &StatisticsArgs::new()).unwrap()
    };

    let whole = statistics(None, vec![]).await;
    assert!(
        matches!(whole.num_rows, Precision::Exact(rows) if rows > 1),
        "the control must state the table's rows exactly: {:?}",
        whole.num_rows
    );
    assert!(
        matches!(whole.column_statistics[0].min_value, Precision::Exact(_)),
        "the control must state `id`'s minimum exactly: {:?}",
        whole.column_statistics[0].min_value
    );

    for (what, statistics) in [
        ("a fetch", statistics(Some(1), vec![]).await),
        ("a filter", statistics(None, vec![col("id").gt(lit(0))]).await),
    ] {
        assert!(
            !matches!(statistics.num_rows, Precision::Exact(_)),
            "{what} left the row count exact: {:?}",
            statistics.num_rows
        );
        for column in &statistics.column_statistics {
            assert!(
                !matches!(column.min_value, Precision::Exact(_))
                    && !matches!(column.max_value, Precision::Exact(_))
                    && !matches!(column.null_count, Precision::Exact(_)),
                "{what} left a column statistic exact: {column:?}"
            );
        }
    }
}

/// A distinct count is `Exact` where every group of every block kept a
/// dictionary of the texts the column emits, and `Absent` everywhere else —
/// never the union's size as a guess (`docs/design/decisions.md`, "D89").
/// Each `Exact` one answers `COUNT(DISTINCT)`, which the blind session reads
/// back; the aggregate target asks every column, and this one pins which
/// qualify.
#[tokio::test]
async fn a_distinct_count_is_exact_only_where_every_group_kept_a_dictionary_of_emitted_text() {
    let scratch = tempfile::tempdir().unwrap();
    let spans = fixtures_root().join("16/statistics/load-via-partition-root.sql");
    let spans = parsed_copy(&spans, scratch.path()).await;
    let types = parsed_copy(&fixtures_root().join("16/types/default.sql"), scratch.path()).await;
    // `public.spans` is three blocks. `tint`'s union is larger than any one
    // block's, `small` holds NULLs, and `stamp` wrote one instant from two
    // offsets. `wide` overflows every block's dictionaries and `mixed` the
    // third's; `id` has more values than a group keeps; `padded` and `bare`
    // are `character`, whose entries give up the blanks they emit.
    let exact = [("tint", 9), ("colour", 4), ("flag", 2), ("small", 5), ("stamp", 2)];
    let absent = ["wide", "mixed", "id", "padded", "bare"];
    for schema_mode in [SchemaMode::Typed, SchemaMode::Strings] {
        let (dump, reading, blind, _) = opened(&spans, schema_mode).await;
        let table = dump.table(None, Some("public"), "spans").unwrap();
        let state = reading.state();
        let plan = table.scan(&state, None, &[], None).await.unwrap();
        let statistics =
            StatisticsContext::new().compute(plan.as_ref(), &StatisticsArgs::new()).unwrap();
        let distinct = |name: &str| {
            let index = table.schema().index_of(name).unwrap();
            statistics.column_statistics[index].distinct_count
        };
        for (name, count) in exact {
            assert_eq!(distinct(name), Precision::Exact(count), "{name} ({schema_mode:?})");
            let sql = format!("SELECT COUNT(DISTINCT {name}), COUNT(*) FROM \"dump\".public.spans");
            assert!(
                agrees(&reading, &blind, None, &sql).await.answered(),
                "{sql} ({schema_mode:?})"
            );
        }
        for name in absent {
            assert_eq!(distinct(name), Precision::Absent, "{name} ({schema_mode:?})");
            let sql = format!("SELECT COUNT(DISTINCT {name}), COUNT(*) FROM \"dump\".public.spans");
            assert!(
                !agrees(&reading, &blind, None, &sql).await.answered(),
                "{sql} ({schema_mode:?})"
            );
        }
    }
    // An enum is emitted `Dictionary`, whose `MIN` and `MAX` DataFusion types
    // as the value type (`KD45`); its `COUNT(DISTINCT)` is an `Int64` and
    // counts labels, so it answers. So does an `interval`, a
    // `MonthDayNano` distinct wherever its text is.
    let (_dump, reading, blind, _) = opened(&types, SchemaMode::Typed).await;
    for (column, table) in [("v_mood", "t_enum_domain"), ("v_interval", "t_interval")] {
        let sql = format!("SELECT COUNT(DISTINCT {column}), COUNT(*) FROM \"dump\".public.{table}");
        assert!(agrees(&reading, &blind, None, &sql).await.answered(), "{sql}");
    }
}

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
}

/// Every batch of `plan`, partition by partition, or the error reading one
/// raised.
async fn partitions_of(
    ctx: &SessionContext,
    plan: Arc<dyn ExecutionPlan>,
) -> datafusion::error::Result<Vec<Vec<RecordBatch>>> {
    let mut partitions = Vec::new();
    for mut stream in execute_stream_partitioned(plan, ctx.task_ctx())? {
        let mut batches = Vec::new();
        while let Some(batch) = stream.next().await {
            batches.push(batch?);
        }
        partitions.push(batches);
    }
    Ok(partitions)
}

/// The columns of `table` a scan can read — every one but a column holding a
/// value its Arrow type cannot (`KD8`), which fails the read of any plan
/// projecting it — by index, in schema order.
async fn readable_columns(ctx: &SessionContext, table: &PgDumpTable) -> Vec<usize> {
    let state = ctx.state();
    let mut readable = Vec::new();
    for index in 0..table.schema().fields().len() {
        let plan = table.scan(&state, Some(&vec![index]), &[], None).await.unwrap();
        if partitions_of(ctx, plan).await.is_ok() {
            readable.push(index);
        }
    }
    readable
}

/// The bytes a column emits by the measure a byte-size statistic is checked
/// against: its width for every row of a fixed-width type, and the length of
/// every non-null value of a variable-width one — the values' own bytes, not
/// the buffers holding them, a view's living in a buffer the batches share
/// (`docs/design/decisions.md`, "D46"). `None` for a type with no measure
/// here, where a stated byte size fails the check rather than passing it.
fn emitted_bytes(array: &dyn Array) -> Option<usize> {
    let data_type = array.data_type();
    if let Some(width) = data_type.primitive_width() {
        return Some(array.len() * width);
    }
    match data_type {
        DataType::FixedSizeBinary(width) => Some(array.len() * *width as usize),
        DataType::Utf8View => Some(array.as_string_view().iter().flatten().map(str::len).sum()),
        DataType::Utf8 => Some(array.as_string::<i32>().iter().flatten().map(str::len).sum()),
        DataType::BinaryView => {
            Some(array.as_binary_view().iter().flatten().map(<[u8]>::len).sum())
        }
        DataType::Binary => Some(array.as_binary::<i32>().iter().flatten().map(<[u8]>::len).sum()),
        _ => None,
    }
}

/// A count a statistic states against the one the scan emitted: equal where
/// it says `Exact`, never below where it says `Inexact` — a bound, not a
/// guess.
fn bounds(what: &str, stated: &Precision<usize>, emitted: Option<usize>) {
    if matches!(stated, Precision::Absent) {
        return;
    }
    let emitted = emitted.unwrap_or_else(|| panic!("{what}: stated {stated:?} with no measure"));
    match stated {
        Precision::Exact(stated) => assert_eq!(*stated, emitted, "{what}: an exact count"),
        Precision::Inexact(stated) => {
            assert!(*stated >= emitted, "{what}: {stated} is below the {emitted} the scan emits")
        }
        Precision::Absent => {}
    }
}

/// `statistics` against the batches they describe: the rows, the total bytes
/// and each column's bytes.
fn check_estimates(what: &str, statistics: &Statistics, batches: &[RecordBatch]) {
    let rows = batches.iter().map(RecordBatch::num_rows).sum();
    bounds(&format!("{what}: rows"), &statistics.num_rows, Some(rows));
    let columns = statistics.column_statistics.len();
    let per_column: Vec<Option<usize>> = (0..columns)
        .map(|i| batches.iter().map(|batch| emitted_bytes(batch.column(i).as_ref())).sum())
        .collect();
    let total = per_column.iter().copied().sum::<Option<usize>>();
    bounds(&format!("{what}: total bytes"), &statistics.total_byte_size, total);
    for (i, column) in statistics.column_statistics.iter().enumerate() {
        bounds(&format!("{what}: column {i}'s bytes"), &column.byte_size, per_column[i]);
    }
}

/// Where rows were taken out — by a filter or a fetch — no column statistic
/// can be `Exact`: each describes the rows before.
fn nothing_exact_per_column(what: &str, statistics: &Statistics) {
    for column in &statistics.column_statistics {
        let exact = matches!(column.null_count, Precision::Exact(_))
            || matches!(column.min_value, Precision::Exact(_))
            || matches!(column.max_value, Precision::Exact(_))
            || matches!(column.sum_value, Precision::Exact(_))
            || matches!(column.distinct_count, Precision::Exact(_))
            || matches!(column.byte_size, Precision::Exact(_));
        assert!(!exact, "{what}: a column statistic is exact past a filter or fetch: {column:?}");
    }
}

fn column(name: &str) -> Expr {
    Expr::Column(Column::new_unqualified(name))
}

fn binary(left: Expr, op: Operator, right: Expr) -> Expr {
    Expr::BinaryExpr(BinaryExpr::new(Box::new(left), op, Box::new(right)))
}

/// Up to three of `array`'s distinct non-null values, first, middle and last
/// in row order — a column's literals, drawn as `tests/pushdown.rs` draws
/// them.
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
    let mut picks = match distinct.len() {
        0 => Vec::new(),
        n => vec![distinct[0], distinct[n / 2], distinct[n - 1]],
    };
    picks.dedup();
    picks.into_iter().map(|row| ScalarValue::try_from_array(array, row).unwrap()).collect()
}

const OPERATORS: [Operator; 6] =
    [Operator::Eq, Operator::NotEq, Operator::Lt, Operator::LtEq, Operator::Gt, Operator::GtEq];

/// Seeded trees over the terms a table pushes, as the library's
/// `tests/pruning.rs` builds its own.
fn random_tree(rng: &mut Rng, terms: &[Expr], depth: usize) -> Expr {
    let child = |rng: &mut Rng| random_tree(rng, terms, depth - 1);
    match if depth == 0 { 0 } else { rng.below(5) } {
        0 | 1 => terms[rng.below(terms.len())].clone(),
        2 => child(rng).and(child(rng)),
        3 => child(rng).or(child(rng)),
        _ => Expr::Not(Box::new(child(rng))),
    }
}

/// Random trees per table, beside each pushed term alone.
const TREES_PER_TABLE: usize = 12;

/// What the estimate target compared, so a sweep that compared nothing fails.
#[derive(Default, Debug)]
struct Estimates {
    filtered: usize,
    /// Scans whose filter pruned a group, where a bound from the kept groups
    /// differs from the table's.
    pruning: usize,
    /// Scans whose filter's row bound fell below the table's rows — each one
    /// a scan that pruned.
    tightened: usize,
}

/// **An estimate is never below what the scan emits, and an `Exact` one is
/// what it emits**: every table of [`planned_fixtures`], gathered at
/// [`SMALL_GROUP`], unfiltered, fetched, and under each term the provider
/// pushes and seeded trees of them — for the whole node and for each
/// partition, rows and bytes alike. Past a filter or a fetch no column
/// statistic may be `Exact`, and a filter's row bound falls below the table's
/// rows only where its pruning skipped a group.
#[test]
fn an_estimate_is_never_below_what_the_scan_emits() {
    let seen = per_major(planned_fixtures(), estimates_over).into_iter().fold(
        Estimates::default(),
        |sum, seen| Estimates {
            filtered: sum.filtered + seen.filtered,
            pruning: sum.pruning + seen.pruning,
            tightened: sum.tightened + seen.tightened,
        },
    );
    assert!(seen.filtered > 20_000, "{seen:?}");
    assert!(seen.pruning > seen.filtered / 5, "{seen:?}");
    assert!(seen.tightened > seen.pruning * 9 / 10, "{seen:?}");
}

/// [`an_estimate_is_never_below_what_the_scan_emits`] over one major's
/// `fixtures`, its seed the major's own.
async fn estimates_over(fixtures: Vec<PathBuf>) -> Estimates {
    let scratch = tempfile::tempdir().unwrap();
    let major = fixtures[0].parent().and_then(Path::parent).and_then(Path::file_name);
    let major: u64 = major.unwrap().to_str().unwrap().parse().unwrap();
    let mut rng = Rng(0x5eed_2501 + major);
    let mut seen = Estimates::default();
    for fixture in fixtures {
        let copy = parsed_copy_in_small_groups(&fixture, scratch.path()).await;
        let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
        let (ctx, _blind) = sessions_at(3);
        let state = ctx.state();
        for name in dump.tables() {
            let Ok(table) = PgDumpTable::new(Arc::clone(&dump), name.clone()) else { continue };
            let what = format!("{} in {}", name.qualified(), fixture.display());
            let projection = readable_columns(&ctx, &table).await;
            if projection.is_empty() {
                continue;
            }
            let unfiltered = table.scan(&state, Some(&projection), &[], None).await.unwrap();
            let whole = partitions_of(&ctx, Arc::clone(&unfiltered)).await.unwrap();
            let all: Vec<RecordBatch> = whole.iter().flatten().cloned().collect();
            let table_rows = all.iter().map(RecordBatch::num_rows).sum::<usize>();
            let schema = unfiltered.schema();
            let resolved = table.resolved_schema();
            let mut terms = Vec::new();
            for (at, &index) in projection.iter().enumerate() {
                let field = schema.field(at);
                terms.push(column(field.name()).is_null());
                terms.push(column(field.name()).is_not_null());
                if resolved.plans[index] != NestedPlan::Scalar {
                    continue;
                }
                let values =
                    all.iter().map(|batch| Arc::clone(batch.column(at))).collect::<Vec<_>>();
                let Some(values) = (!values.is_empty()).then(|| {
                    arrow::compute::concat(&values.iter().map(AsRef::as_ref).collect::<Vec<_>>())
                        .unwrap()
                }) else {
                    continue;
                };
                for value in literals(&values) {
                    for op in OPERATORS {
                        terms.push(binary(
                            column(field.name()),
                            op,
                            Expr::Literal(value.clone(), None),
                        ));
                    }
                }
            }
            terms.retain(|term| {
                table.supports_filters_pushdown(&[term]).unwrap()[0]
                    == TableProviderFilterPushDown::Exact
            });
            let mut filters: Vec<Option<Expr>> = vec![None];
            filters.extend(terms.iter().cloned().map(Some));
            if !terms.is_empty() {
                filters
                    .extend((0..TREES_PER_TABLE).map(|_| Some(random_tree(&mut rng, &terms, 3))));
            }
            for filter in filters {
                let pushed = filter.iter().cloned().collect::<Vec<_>>();
                for limit in [None, Some(7)] {
                    let what = match &filter {
                        Some(filter) => format!("{what} under {filter}, fetch {limit:?}"),
                        None => format!("{what}, fetch {limit:?}"),
                    };
                    let plan = table.scan(&state, Some(&projection), &pushed, limit).await.unwrap();
                    let partitions = partitions_of(&ctx, Arc::clone(&plan)).await.unwrap();
                    let statistics = StatisticsContext::new()
                        .compute(plan.as_ref(), &StatisticsArgs::new())
                        .unwrap();
                    match limit {
                        // A fetch is applied above the partitions, which may
                        // emit more than it keeps.
                        Some(limit) => {
                            let rows = partitions.iter().flatten().map(RecordBatch::num_rows);
                            let kept = rows.sum::<usize>().min(limit);
                            bounds(&format!("{what}: rows"), &statistics.num_rows, Some(kept));
                        }
                        None => {
                            let batches: Vec<RecordBatch> =
                                partitions.iter().flatten().cloned().collect();
                            check_estimates(&what, &statistics, &batches);
                            for (i, partition) in partitions.iter().enumerate() {
                                let args = StatisticsArgs::new().with_partition(Some(i));
                                let statistics =
                                    StatisticsContext::new().compute(plan.as_ref(), &args).unwrap();
                                let what = format!("{what}, partition {i}");
                                check_estimates(&what, &statistics, partition);
                            }
                        }
                    }
                    // A fetch the table's rows fit under takes nothing out.
                    let fetch_cuts = limit.is_some_and(|limit| table_rows > limit);
                    if filter.is_some() || fetch_cuts {
                        nothing_exact_per_column(&what, &statistics);
                    }
                    if filter.is_some() && limit.is_none() {
                        seen.filtered += 1;
                        let metrics = plan.metrics().unwrap_or_default().aggregate_by_name();
                        let pruned = metrics.iter().any(|metric| {
                            matches!(
                                metric.value(),
                                MetricValue::PruningMetrics { name, pruning_metrics }
                                    if name == "row_groups_pruned_statistics"
                                        && pruning_metrics.pruned() > 0
                            )
                        });
                        seen.pruning += usize::from(pruned);
                        let bound = statistics.num_rows.get_value().copied();
                        if bound.is_some_and(|bound| bound < table_rows) {
                            assert!(pruned, "{what}: a bound below the table's with no pruning");
                            seen.tightened += 1;
                        }
                    }
                }
            }
        }
    }
    seen
}

/// The partition counts the ordering target plans each table at. `public.spans`
/// is three blocks under `--load-via-partition-root`, and a partition crossing
/// a boundary between them is where a declared ordering needs its proof; a
/// count can cut every partition at a boundary, so several are run and the
/// target asserts one of them, past a single partition, crosses — the proof
/// exercised rather than skipped.
const ORDERING_PARTITIONS: [usize; 4] = [1, 2, 3, 5];

/// Whether `plan`, or anything under it, is a sort.
fn sorts(plan: &Arc<dyn ExecutionPlan>) -> bool {
    plan.name() == "SortExec" || plan.children().into_iter().any(sorts)
}

/// **A declared ordering holds in every partition, and drops exactly the sorts
/// it satisfies.** Every table of [`planned_fixtures`] at each of
/// [`ORDERING_PARTITIONS`]: each ordering the scan declares is checked over
/// each partition's rows under Arrow's row comparator, which places NULLs as
/// the ordering says; each readable scalar column's `ORDER BY`, ascending and
/// descending under both NULL placements, plans a sort exactly where the scan
/// declares no ordering satisfying it; and where a partition of
/// `public.spans` crosses a block boundary, the proof is checked to have
/// declared what is ordered across it and nothing that is not
/// ([`crossing_proved`]).
#[test]
fn a_declared_ordering_holds_in_every_partition_and_drops_the_sort() {
    let mut crossings: BTreeMap<usize, bool> = BTreeMap::new();
    for crossed in per_major(planned_fixtures(), orderings_over) {
        for (partitions, crossed) in crossed {
            *crossings.entry(partitions).or_default() |= crossed;
        }
    }
    assert!(
        crossings.iter().any(|(&partitions, &crossed)| partitions > 1 && crossed),
        "no partition of `public.spans` cut among several crossed a block boundary: \
         {crossings:?}"
    );
}

/// [`a_declared_ordering_holds_in_every_partition_and_drops_the_sort`] over one
/// major's `fixtures`: at each partition count, whether a partition of
/// `public.spans` crossed a block boundary.
async fn orderings_over(fixtures: Vec<PathBuf>) -> BTreeMap<usize, bool> {
    let scratch = tempfile::tempdir().unwrap();
    let mut crossings = BTreeMap::new();
    for fixture in fixtures {
        let copy = parsed_copy(&fixture, scratch.path()).await;
        let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
        for partitions in ORDERING_PARTITIONS {
            let (ctx, blind) = sessions_at(partitions);
            let catalogs = register_in((&ctx, &blind), &dump);
            let state = ctx.state();
            for name in dump.tables() {
                let Ok(table) = PgDumpTable::new(Arc::clone(&dump), name.clone()) else { continue };
                let what = format!("{} in {} at {partitions}", name.qualified(), fixture.display());
                let projection = readable_columns(&ctx, &table).await;
                if projection.is_empty() {
                    continue;
                }
                let plan = table.scan(&state, Some(&projection), &[], None).await.unwrap();
                let schema = plan.schema();
                let read = partitions_of(&ctx, Arc::clone(&plan)).await.unwrap();
                let equivalences = plan.properties().equivalence_properties();
                if name.table == "moods" {
                    enum_proved(&what, equivalences.oeq_class().iter());
                }
                if let Ok(part) = schema.index_of("part")
                    && fixture.file_stem().is_some_and(|stem| stem == "load-via-partition-root")
                    && name.table == "spans"
                {
                    let crossed = read.iter().any(|batches| {
                        let parts: BTreeSet<String> = batches
                            .iter()
                            .flat_map(|batch| {
                                let formatter = ArrayFormatter::try_new(
                                    batch.column(part).as_ref(),
                                    &FormatOptions::default(),
                                )
                                .unwrap();
                                (0..batch.num_rows())
                                    .map(|row| formatter.value(row).to_string())
                                    .collect::<Vec<_>>()
                            })
                            .collect();
                        parts.len() > 1
                    });
                    *crossings.entry(partitions).or_insert(false) |= crossed;
                    if crossed {
                        crossing_proved(&what, equivalences.oeq_class().iter());
                    }
                }
                for ordering in equivalences.oeq_class().iter() {
                    for (i, batches) in read.iter().enumerate() {
                        let fields = ordering
                            .iter()
                            .map(|sort| {
                                SortField::new_with_options(
                                    sort.expr.data_type(&schema).unwrap(),
                                    sort.options,
                                )
                            })
                            .collect();
                        let converter = RowConverter::new(fields).unwrap();
                        let mut previous = None;
                        for batch in batches {
                            let columns = ordering
                                .iter()
                                .map(|sort| {
                                    sort.expr
                                        .evaluate(batch)
                                        .unwrap()
                                        .into_array(batch.num_rows())
                                        .unwrap()
                                })
                                .collect::<Vec<_>>();
                            let rows = converter.convert_columns(&columns).unwrap();
                            for row in rows.iter() {
                                if let Some(previous) = &previous {
                                    assert!(
                                        *previous <= row.owned(),
                                        "{what}: partition {i} is out of the declared {ordering}"
                                    );
                                }
                                previous = Some(row.owned());
                            }
                        }
                    }
                }
                let catalog = catalogs
                    .iter()
                    .find(|(database, _)| *database == name.database)
                    .unwrap()
                    .1
                    .clone();
                let from = from(&catalog, name);
                let resolved = table.resolved_schema();
                for (at, &index) in projection.iter().enumerate() {
                    if resolved.plans[index] != NestedPlan::Scalar {
                        continue;
                    }
                    let field = schema.field(at);
                    let quoted_column = quoted(field.name());
                    for (direction, descending, nulls_first) in [
                        ("ASC", false, false),
                        ("DESC", true, true),
                        ("ASC NULLS FIRST", false, true),
                        ("DESC NULLS LAST", true, false),
                    ] {
                        let sql = format!(
                            "SELECT {quoted_column} FROM {from} ORDER BY {quoted_column} {direction}"
                        );
                        let sorted =
                            ctx.sql(&sql).await.unwrap().create_physical_plan().await.unwrap();
                        let declared = equivalences.oeq_class().iter().any(|ordering| {
                            let first = ordering.first();
                            first.options.descending == descending
                                && first.options.nulls_first == nulls_first
                                && first
                                    .expr
                                    .downcast_ref::<ColumnExpr>()
                                    .is_some_and(|c| c.name() == field.name())
                        });
                        assert_eq!(
                            !sorts(&sorted),
                            declared,
                            "{what}: `{sql}` sorts where the scan declares {:?}",
                            equivalences.oeq_class()
                        );
                    }
                }
            }
        }
    }
    crossings
}

/// **The boundary proof, both ways**, over `public.spans` read as three
/// blocks with a partition crossing a boundary between them: every column
/// ordered across the blocks is declared in its direction, and `local`, which
/// restarts in each block, and `gappy`, which holds a NULL, are not.
fn crossing_proved<'a>(what: &str, orderings: impl Iterator<Item = &'a LexOrdering>) {
    let declared: BTreeSet<(String, bool)> = orderings
        .filter_map(|ordering| {
            let first = ordering.first();
            let column = first.expr.downcast_ref::<ColumnExpr>()?;
            Some((column.name().to_string(), first.options.descending))
        })
        .collect();
    for (column, descending) in [
        ("part", false),
        ("id", false),
        ("reversed", true),
        ("label", false),
        ("i4", true),
        ("ident", false),
    ] {
        assert!(
            declared.contains(&(column.to_string(), descending)),
            "{what}: `{column}` is ordered across every block and was not declared so: \
             {declared:?}"
        );
    }
    for column in ["local", "gappy"] {
        assert!(
            !declared.iter().any(|(name, _)| name == column),
            "{what}: `{column}` was declared ordered across a boundary: {declared:?}"
        );
    }
}

/// `public.moods.m`, an enum ascending by label text and descending in its
/// declared order, is declared ascending: the order Arrow sorts a dictionary
/// by, which the partition check above then reads it in.
fn enum_proved<'a>(what: &str, mut orderings: impl Iterator<Item = &'a LexOrdering>) {
    let declared = orderings.any(|ordering| {
        let first = ordering.first();
        !first.options.descending
            && first.expr.downcast_ref::<ColumnExpr>().is_some_and(|column| column.name() == "m")
    });
    assert!(declared, "{what}: the enum `m` is not declared ascending");
}

/// Which table the build side of `plan`'s one hash join reads, by a column
/// only it projects, and the join's mode.
fn build_side(plan: &Arc<dyn ExecutionPlan>, marker: &str) -> Option<(bool, PartitionMode)> {
    if let Some(join) = plan.downcast_ref::<HashJoinExec>() {
        let left = join.left().schema();
        return Some((left.index_of(marker).is_ok(), *join.partition_mode()));
    }
    plan.children().into_iter().find_map(|child| build_side(child, marker))
}

/// **Rows alone choose a join's build side today**, the scan stating no byte
/// size: `public.long_value`, four rows each carrying up to a mebibyte, is the
/// smaller side by rows and is collected whole to build — the shape a byte
/// size would move, the wide side then being the larger. Its answer is the
/// blind session's.
#[tokio::test]
async fn a_join_builds_the_side_its_statistics_call_smaller() {
    let scratch = tempfile::tempdir().unwrap();
    let fixture = fixtures_root().join("16/statistics/default.sql");
    let copy = parsed_copy(&fixture, scratch.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let (reading, blind) = sessions();
    register_in((&reading, &blind), &dump);
    let sql = "SELECT o.id, length(l.v) AS v_length FROM \"dump\".public.ordered AS o \
               JOIN \"dump\".public.long_value AS l ON o.id = l.id ORDER BY o.id";
    agrees(&reading, &blind, None, sql).await;
    let plan = reading.sql(sql).await.unwrap().create_physical_plan().await.unwrap();
    let shape = build_side(&plan, "v");
    assert_eq!(
        shape,
        Some((true, PartitionMode::CollectLeft)),
        "{}",
        displayable(plan.as_ref()).indent(false)
    );
}

/// **A pushed filter's row bound moves a join's build side**: `ordered`
/// joined to itself, the right side under `reversed <= 10`, which the small
/// groups prune to the last few, so the filtered side's bound is below the
/// unfiltered side's exact rows and it is swapped in to build. Stated as the
/// whole table's rows, the two sides tie and the left, as written, builds.
/// Its answer is the blind session's.
#[tokio::test]
async fn a_filtered_side_s_bound_moves_a_join_s_build_side() {
    let scratch = tempfile::tempdir().unwrap();
    let fixture = fixtures_root().join("16/statistics/default.sql");
    let copy = parsed_copy_in_small_groups(&fixture, scratch.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let (reading, blind) = sessions();
    register_in((&reading, &blind), &dump);
    let sql = "SELECT a.unsorted, b.stepped FROM \"dump\".public.ordered AS a \
               JOIN \"dump\".public.ordered AS b ON a.id = b.id \
               WHERE b.reversed <= 10 ORDER BY a.unsorted";
    agrees(&reading, &blind, None, sql).await;
    let plan = reading.sql(sql).await.unwrap().create_physical_plan().await.unwrap();
    let shape = build_side(&plan, "stepped");
    assert!(
        matches!(shape, Some((true, _))),
        "{shape:?}\n{}",
        displayable(plan.as_ref()).indent(false)
    );
}
