//! The statistics the provider hands DataFusion, against the answers a scan
//! gives (`docs/design/roadmap-P6-datafusion.md`, "Verification", item 4).
//!
//! **The oracle is the same query with the rule that reads statistics taken
//! out.** DataFusion answers `COUNT(*)`, `COUNT(<column>)`, `MIN` and `MAX`
//! from `Exact` statistics by replacing the aggregate with a literal, so a
//! wrong `Exact` is a wrong answer with no error; a session whose physical
//! optimizer does not carry `aggregate_statistics` reads every row instead,
//! and the two must agree wherever reading the column answers at all. Where a
//! typed read refuses (`KD8`), a count is still checked: read as text, no
//! value refuses, and a `COUNT(<column>)` counts values without their meaning.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use arrow::util::pretty::pretty_format_batches;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions, register_dump};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    Finding, LocalFileSource, ScanOptions, SchemaMode, StatisticsRequest, TableName, map_file,
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

/// `fixture` copied into `dir` beside the complete cache a gathering parse
/// leaves, so the committed tree is never written into and the map carries
/// the statistics this target is about.
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::ALL).await.unwrap();
    copy
}

/// A session that reads the provider's statistics, and one that cannot.
fn sessions() -> (SessionContext, SessionContext) {
    let config = SessionConfig::new().with_target_partitions(2).with_batch_size(64);
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
        if !sql.starts_with("SELECT COUNT(") {
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
    assert_eq!(
        rendered(&from_statistics),
        rendered(&from_rows),
        "`{sql}` answered differently with the statistics than without them"
    );
    if answered { Answer::FromStatistics } else { Answer::FromRows }
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
    counted_as_text: bool,
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
                seen.bound |= agrees(reading, blind, Some(text), &sql).await.answered();
            }
        }
    }
}

/// **Typed and as text**, as the provider's other targets are: the text pass
/// is the typed one's oracle wherever a typed read refuses (`KD8`), and is
/// itself read against the rows, bounds included (D89).
#[tokio::test]
async fn statistics_never_change_an_answer() {
    let scratch = tempfile::tempdir().unwrap();
    let mut typed = Seen::default();
    let mut strings = Seen::default();
    for fixture in every_fixture() {
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
    for (mode, seen) in [(SchemaMode::Typed, &typed), (SchemaMode::Strings, &strings)] {
        assert!(seen.count, "no `COUNT(*)` was answered from the statistics ({mode:?})");
        assert!(
            seen.null_count,
            "no `COUNT(<column>)` was answered from the statistics ({mode:?})"
        );
        assert!(seen.bound, "no `MIN`/`MAX` was answered from the statistics ({mode:?})");
    }
    assert!(
        typed.counted_as_text,
        "no typed `COUNT(<column>)` over a refusing column was checked against the text"
    );
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
