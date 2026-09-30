//! A value its column's Arrow type cannot hold (D96), held to one outcome
//! per query under each of the three ways of reading one.
//!
//! **A query's outcome never depends on which rows it read.** So each case
//! runs in every configuration that changes which rows a
//! scan reaches — its partitions in each order ([`in_order`]), the map's
//! statistics gathered and not, DataFusion's dynamic filters off, on, and on
//! with `pgdump.dynamic_filter_rows` dropping the rows they reject — over
//! majors 13, 16 and 18, and each must give the one outcome its mode
//! promises:
//!
//! - **[`Mode::Null`]** answers as DataFusion answers the same SQL over the
//!   table with each such value NULL ([`Oracle`]), types as declared.
//! - **[`Mode::Text`]** answers as DataFusion answers it over the table with
//!   each column holding one read as `Utf8View`, each value its text.
//! - **[`Mode::Refuse`]** refuses at planning, naming the column, wherever
//!   the query materializes one holding such a value; a column read only by
//!   a filter the library answers is not materialized, and there the case
//!   states the answer PostgreSQL's order gives.
//!
//! **Every case gives its outcome but where DataFusion's own defect meets
//! it** (`KD56`): a case marked as one it meets ([`Case::meets_kd56`]) is
//! excluded in the configurations it meets it in ([`Config::meets_kd56`]),
//! and held to still failing there, so the pin carrying the fix strikes the
//! exclusion.
//!
//! **And the sweep is held to having exercised the order**: every table a
//! case reads is scanned by two partitions, so the two orders [`Order::ALL`]
//! names are every order there is — or by one, where statistics kept one
//! partition's worth of its rows, which has one order.
//!
//! **Which values are unrepresentable is read, not listed**: the `types`
//! fixture's extremes, at every major, each decode to a value
//! [`arrow_holds`] admits or are recorded in [`UNREPRESENTABLE`], exactly,
//! and the library's count of them is the record's, tier by tier.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, RecordBatch, new_null_array};
use arrow::compute::{CastOptions, can_cast_types, cast, cast_with_options, concat};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::catalog::TableProvider;
use datafusion::common::DataFusionError;
use datafusion::datasource::MemTable;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::physical_plan::{ExecutionPlan, collect};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions, PgDumpSettings, register_dump};
use pgdump_query::cache::{self, CacheMode, CacheStatus};
use pgdump_query::{
    DataBlock, Finding, LocalFileSource, RowEvaluation, ScanOptions, SchemaMode, SpanBody,
    StatisticsRequest, Unrepresentable, UnrepresentableMode, decode_field, map_file,
};

mod in_order;
use in_order::{Order, RunScansInOrder};

/// A sink for a registration whose findings this target is not about.
fn ignore(_: &dyn Finding) {}

/// The oldest major, the newest, and one between.
const MAJORS: [u32; 3] = [13, 16, 18];

/// The tables the cases read, each holding at least one value its typed
/// column cannot: `NaN` on a `numeric(10,2)`; the infinities on a `date`, a
/// `timestamp` and a `timestamptz`; and on the last two, PostgreSQL's
/// greatest timestamp, past what `Timestamp(Microsecond)` counts from 1970.
/// Then the extremes, [`UNREPRESENTABLE`]'s values, `interval`'s and the
/// nested shapes among them.
const TABLES: [&str; 5] = ["t_numeric", "t_date", "t_timestamp", "t_extremes", "t_extremes_nested"];

/// How an unrepresentable value is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Mode {
    /// The default: the declared type, the value NULL.
    Null,
    /// The column read as text wherever it holds such a value.
    Text,
    /// A query materializing such a column refuses at planning.
    Refuse,
}

const MODES: [Mode; 3] = [Mode::Null, Mode::Text, Mode::Refuse];

/// The options a dump is opened with under `mode`.
fn options(mode: Mode) -> PgDumpOptions {
    let unrepresentable = match mode {
        Mode::Null => UnrepresentableMode::Null,
        Mode::Text => UnrepresentableMode::Text,
        Mode::Refuse => UnrepresentableMode::Refuse,
    };
    PgDumpOptions { unrepresentable, ..PgDumpOptions::default() }
}

/// Whether the map a case reads carries statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Statistics {
    /// Every statistic, in groups a few rows long: pruning, and an aggregate
    /// answered from the map.
    Gathered,
    /// None: every aggregate reads rows, and nothing is pruned.
    Absent,
}

/// DataFusion's dynamic filters, and how a scan consumes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Dynamic {
    /// Every producer's flag off.
    Off,
    /// On, the scan pruning groups and blocks by them: the default.
    On,
    /// On, the scan also dropping each row a filter rejects before decoding
    /// it (`pgdump.dynamic_filter_rows`).
    OnWithRows,
}

const DYNAMIC: [Dynamic; 3] = [Dynamic::Off, Dynamic::On, Dynamic::OnWithRows];

/// One way of running a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    statistics: Statistics,
    dynamic: Dynamic,
    order: Order,
}

impl Config {
    /// **Where `KD56` meets a case that it can**: rows evaluated under the
    /// aggregate's dynamic filter and dropped before decoding, which is
    /// where the filter's lost `MIN` side cuts rows the answer needs —
    /// whatever the mode, as it is not particular to unrepresentable values.
    /// Excluded until a DataFusion pin carries the fix.
    // upstream: UF1
    fn meets_kd56(&self) -> bool {
        self.dynamic == Dynamic::OnWithRows
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}/{:?}/{:?}", self.statistics, self.dynamic, self.order)
    }
}

fn every_config() -> Vec<Config> {
    let mut configs = Vec::new();
    for statistics in [Statistics::Gathered, Statistics::Absent] {
        for dynamic in DYNAMIC {
            for order in Order::ALL {
                configs.push(Config { statistics, dynamic, order });
            }
        }
    }
    configs
}

/// Every producer's flag.
const FLAGS: [&str; 3] = [
    "datafusion.optimizer.enable_join_dynamic_filter_pushdown",
    "datafusion.optimizer.enable_topk_dynamic_filter_pushdown",
    "datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown",
];

/// Two partitions a scan, so the two orders are every order, and batches
/// two rows long, so a filter tightens while a partition still streams.
fn session(config: Config) -> SessionContext {
    let rows = match config.dynamic {
        Dynamic::OnWithRows => RowEvaluation::On,
        Dynamic::Off | Dynamic::On => RowEvaluation::Off,
    };
    let settings = PgDumpSettings { dynamic_filter_rows: rows, ..PgDumpSettings::default() };
    let mut options = SessionConfig::new()
        .with_target_partitions(2)
        .with_batch_size(2)
        .with_option_extension(settings);
    for flag in FLAGS {
        options = options.set_bool(flag, config.dynamic != Dynamic::Off);
    }
    SessionContext::new_with_state(
        SessionStateBuilder::new_with_default_features()
            .with_config(options)
            .with_physical_optimizer_rule(Arc::new(RunScansInOrder(config.order)))
            .build(),
    )
}

/// What the refuse mode does with a case.
#[derive(Debug, Clone, Copy)]
enum Refuse {
    /// Refuses at planning, naming `table.column`.
    Column(&'static str),
    /// Answers these rows, each rendered as [`rendered_rows`] renders one: a
    /// column read only by a filter the library answers, in PostgreSQL's
    /// order, is not materialized.
    Answers(&'static [&'static str]),
}

/// One query, and what each mode must make of it.
struct Case {
    /// Over the tables' bare names.
    sql: &'static str,
    /// Where the query ends in a `LIMIT` choosing no rows in particular: the
    /// answer is any `n` rows of `sql`'s, and `sql` is run with `LIMIT n`.
    pick: Option<usize>,
    refuse: Refuse,
    /// Whether DataFusion's own defect `KD56` meets this case
    /// ([`Config::meets_kd56`]), whose configurations are then excluded.
    meets_kd56: bool,
}

const fn case(sql: &'static str, refuse: Refuse) -> Case {
    Case { sql, pick: None, refuse, meets_kd56: false }
}

const fn picking(sql: &'static str, n: usize, refuse: Refuse) -> Case {
    Case { sql, pick: Some(n), refuse, meets_kd56: false }
}

/// `case`, which `KD56` meets: an ungrouped `MIN` beside a `MAX` whose
/// column is NULL in a partition's first batch.
const fn kd56(case: Case) -> Case {
    Case { meets_kd56: true, ..case }
}

use Refuse::{Answers, Column};

/// Every shape the spec's "Evidence" names — a `LIMIT` one partition meets
/// first, an ungrouped `MIN`/`MAX`, a TopK, a join — and the filters and
/// counts the typed mode's "NULL for every purpose" reaches. The row drop is
/// [`Dynamic::OnWithRows`], under every case. Each table holds such a value
/// in its first rows, so the file's order meets one first; `t_timestamp`
/// holds one in its last row too.
const CASES: &[Case] = &[
    // A `LIMIT` one partition meets first.
    picking("SELECT id, v_small FROM t_numeric", 3, Column("t_numeric.v_small")),
    picking("SELECT v_date FROM t_date", 2, Column("t_date.v_date")),
    picking("SELECT id, v_tstz FROM t_timestamp WHERE id > 1", 2, Column("t_timestamp.v_tstz")),
    // Ungrouped `MIN`/`MAX`: answered from the map where it can be, and with
    // a static filter keeping the map from answering, by the rows under an
    // aggregate's dynamic filter.
    // Both unfiltered pairs miss where rows are dropped under the aggregate's
    // filter and no statistics answer — the untyped mode's text column has no
    // bound to answer from, gathered or not: a partition's first batch holding
    // no value of the column leaves DataFusion's shared `MIN` bound a typed
    // NULL it reads as no bound, so the filter keeps only `> max` (`KD56`),
    // and those configurations are excluded.
    kd56(case("SELECT MIN(v_small), MAX(v_small) FROM t_numeric", Column("t_numeric.v_small"))),
    case("SELECT MIN(v_small) FROM t_numeric WHERE id > 0", Column("t_numeric.v_small")),
    case("SELECT MAX(v_date) FROM t_date WHERE id > 0", Column("t_date.v_date")),
    case("SELECT MIN(v_ts) FROM t_timestamp WHERE id > 0", Column("t_timestamp.v_ts")),
    // A count of the column: NULLs are not counted, so the typed mode's are.
    case("SELECT COUNT(v_small) FROM t_numeric", Column("t_numeric.v_small")),
    case("SELECT COUNT(v_date) FROM t_date WHERE id > 0", Column("t_date.v_date")),
    // TopK, the sort keys alone.
    case("SELECT v_date FROM t_date ORDER BY v_date LIMIT 2", Column("t_date.v_date")),
    case(
        "SELECT v_small FROM t_numeric ORDER BY v_small DESC NULLS LAST LIMIT 2",
        Column("t_numeric.v_small"),
    ),
    case("SELECT v_ts FROM t_timestamp ORDER BY v_ts LIMIT 3", Column("t_timestamp.v_ts")),
    // Joins: a build side ruling out the probe rows holding the value, which
    // a dynamic filter's row drop never decodes; a join on the column itself;
    // and a join whose build side is filtered on one it never materializes.
    case(
        "SELECT n.id, n.v_small FROM (SELECT id FROM t_numeric WHERE id >= 4) b \
         JOIN t_numeric n ON n.id = b.id",
        Column("t_numeric.v_small"),
    ),
    case(
        "SELECT a.id, b.id FROM t_date a JOIN t_date b ON a.v_date = b.v_date",
        Column("t_date.v_date"),
    ),
    case(
        "SELECT t.id, t.v_ts FROM t_date d JOIN t_timestamp t ON t.id = d.id \
         WHERE d.v_date > '1000-01-01'",
        Column("t_timestamp.v_ts"),
    ),
    // A static filter over the column alone, which the library answers: the
    // refuse mode keeps PostgreSQL's order, where an infinity is above every
    // date and no special value is NULL. Each literal is text, which the
    // untyped mode compares as text: DataFusion reads a `Utf8View` against a
    // number by casting each value to the number's type.
    case("SELECT id FROM t_numeric WHERE v_small IS NOT NULL", Answers(&["3", "4", "5"])),
    case("SELECT id FROM t_date WHERE v_date > '5000-01-01'", Answers(&["1", "4", "6"])),
    case("SELECT id FROM t_date WHERE v_date IS NULL", Answers(&["7"])),
    case(
        "SELECT COUNT(*) FROM t_timestamp WHERE v_tstz < '2000-01-01 00:00:00+00'",
        Answers(&["3"]),
    ),
    // The extremes: a value the decoder refuses, and one it decodes to a
    // value `arrow-cast` cannot display — `24:00:00`, a date or timestamp
    // past `chrono`'s calendar — in each shape above, and the nested ones.
    picking(
        "SELECT id, v_interval FROM t_extremes WHERE v_interval IS NOT NULL",
        3,
        Column("t_extremes.v_interval"),
    ),
    kd56(case(
        "SELECT MIN(v_interval), MAX(v_interval) FROM t_extremes",
        Column("t_extremes.v_interval"),
    )),
    case("SELECT MAX(v_time) FROM t_extremes", Column("t_extremes.v_time")),
    case("SELECT MAX(v_date) FROM t_extremes WHERE id > 0", Column("t_extremes.v_date")),
    case("SELECT MAX(v_numeric76) FROM t_extremes", Column("t_extremes.v_numeric76")),
    case("SELECT COUNT(v_ts) FROM t_extremes", Column("t_extremes.v_ts")),
    case(
        "SELECT v_tstz FROM t_extremes ORDER BY v_tstz DESC NULLS LAST LIMIT 2",
        Column("t_extremes.v_tstz"),
    ),
    case(
        "SELECT e.id, e.v_interval FROM t_extremes_nested n JOIN t_extremes e ON e.id = n.id",
        Column("t_extremes.v_interval"),
    ),
    case("SELECT id FROM t_extremes WHERE v_time > '12:00:00'", Answers(&["2"])),
    picking(
        "SELECT id, v_date_array FROM t_extremes_nested",
        2,
        Column("t_extremes_nested.v_date_array"),
    ),
    case(
        "SELECT id, v_dated FROM t_extremes_nested WHERE id = 2",
        Column("t_extremes_nested.v_dated"),
    ),
    case(
        "SELECT MAX(v_interval_array) FROM t_extremes_nested",
        Column("t_extremes_nested.v_interval_array"),
    ),
    case("SELECT id FROM t_extremes_nested WHERE v_daterange IS NULL", Answers(&["3"])),
];

/// Which step of a query a refusal came out of.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    /// Before a row is read: SQL to a physical plan.
    Planning,
    /// While the plan runs.
    Execution,
}

/// What one run of a case gave.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Answered {
        types: Vec<DataType>,
        rows: Vec<String>,
    },
    /// A value its column's type cannot hold, named by its table and column
    /// alone: which row a refusal reports is load's to choose.
    Refused {
        column: String,
        stage: Stage,
    },
    /// Anything else, whole.
    Failed(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Outcome::Answered { types, rows } => write!(f, "{types:?} {rows:?}"),
            Outcome::Refused { column, stage } => write!(f, "refused {column} at {stage:?}"),
            Outcome::Failed(err) => write!(f, "failed: {err}"),
        }
    }
}

impl Outcome {
    fn of(err: &DataFusionError, stage: Stage) -> Outcome {
        let mut cause: Option<&dyn std::error::Error> = Some(err);
        while let Some(err) = cause {
            if let Some(
                pgdump_query::Error::FieldDecode { table, column, .. }
                | pgdump_query::Error::Unrepresentable { table, column, .. },
            ) = err.downcast_ref()
            {
                let table = table.rsplit('.').next().unwrap_or(table);
                return Outcome::Refused { column: format!("{table}.{column}"), stage };
            }
            cause = err.source();
        }
        Outcome::Failed(err.to_string())
    }

    fn answered(batches: &[RecordBatch], schema: &Schema) -> Outcome {
        let types = schema.fields().iter().map(|f| f.data_type().clone()).collect();
        Outcome::Answered { types, rows: rendered_rows(batches) }
    }
}

/// What a mode promises a case.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    /// These rows, or where `pick` is stated, any that many of them.
    Rows { types: Vec<DataType>, rows: Vec<String>, pick: Option<usize> },
    /// A refusal at planning naming `table.column`.
    Refused(String),
}

impl fmt::Display for Expected {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Expected::Rows { types, rows, pick: None } => write!(f, "{types:?} {rows:?}"),
            Expected::Rows { types, rows, pick: Some(n) } => {
                write!(f, "{types:?} any {n} of {rows:?}")
            }
            Expected::Refused(column) => write!(f, "refused {column} at Planning"),
        }
    }
}

impl Expected {
    fn met_by(&self, outcome: &Outcome) -> bool {
        match (self, outcome) {
            (Expected::Refused(want), Outcome::Refused { column, stage: Stage::Planning }) => {
                want == column
            }
            (Expected::Rows { types, rows, pick }, Outcome::Answered { types: got, rows: had }) => {
                if types != got {
                    return false;
                }
                match pick {
                    None => rows == had,
                    // Each row answered is taken out of the rows allowed.
                    Some(n) => {
                        let mut left: BTreeMap<&String, usize> = BTreeMap::new();
                        for row in rows {
                            *left.entry(row).or_default() += 1;
                        }
                        had.len() == (*n).min(rows.len())
                            && had.iter().all(|row| match left.get_mut(row) {
                                Some(count) if *count > 0 => {
                                    *count -= 1;
                                    true
                                }
                                _ => false,
                            })
                    }
                }
            }
            _ => false,
        }
    }
}

/// Each row of `batches` as text, sorted: an answer as a multiset.
fn rendered_rows(batches: &[RecordBatch]) -> Vec<String> {
    let options = FormatOptions::default().with_null("NULL");
    let mut out = Vec::new();
    for batch in batches {
        let columns: Vec<_> = batch
            .columns()
            .iter()
            .map(|c| ArrayFormatter::try_new(c.as_ref(), &options).unwrap())
            .collect();
        for row in 0..batch.num_rows() {
            out.push(
                columns.iter().map(|c| c.value(row).to_string()).collect::<Vec<_>>().join("|"),
            );
        }
    }
    out.sort();
    out
}

/// `sql`'s outcome in `ctx`, and the plan it ran where it planned.
async fn outcome(ctx: &SessionContext, sql: &str) -> (Outcome, Option<Arc<dyn ExecutionPlan>>) {
    let frame = match ctx.sql(sql).await {
        Ok(frame) => frame,
        Err(err) => return (Outcome::of(&err, Stage::Planning), None),
    };
    let plan = match frame.create_physical_plan().await {
        Ok(plan) => plan,
        Err(err) => return (Outcome::of(&err, Stage::Planning), None),
    };
    let schema = plan.schema();
    let outcome = match collect(Arc::clone(&plan), ctx.task_ctx()).await {
        Ok(batches) => Outcome::answered(&batches, &schema),
        Err(err) => Outcome::of(&err, Stage::Execution),
    };
    (outcome, Some(plan))
}

/// Every scan in `plan`, and the partitions each runs.
fn scans(plan: &Arc<dyn ExecutionPlan>, found: &mut Vec<usize>) {
    for child in plan.children() {
        scans(child, found);
    }
    if plan.name() == "PgDumpExec" {
        found.push(plan.properties().partitioning.partition_count());
    }
}

fn fixture(major: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../fixtures/{major}/types/default.sql"))
}

/// A group size a few rows of every case's table long.
const SMALL_GROUP: u64 = 64;

/// `fixture` copied into its own directory under `dir` beside a complete
/// cache, gathering what `statistics` says, so the committed tree is never
/// written into.
async fn parsed_copy(fixture: &Path, dir: &Path, statistics: Statistics) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let path = cache::colocated_path(&copy);
    let cache = CacheMode::enabled(path.clone());
    let request = StatisticsRequest {
        group_size: Some(NonZeroU64::new(SMALL_GROUP).unwrap()),
        ..StatisticsRequest::DATA
    };
    map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    // Absent is a data-level map holding none, as a query's own mapping pass
    // leaves one: the provider refuses a metadata-level table outright.
    if statistics == Statistics::Absent {
        let CacheStatus::Valid { mut index, .. } = cache::load(&path, &source).await.unwrap()
        else {
            panic!("the parse left a complete cache")
        };
        for span in &mut index.spans {
            if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
                block.statistics = None;
            }
        }
        cache::save(&path, &source, &index).await.unwrap();
    }
    copy
}

/// `dump`'s case tables registered in `ctx` under their bare names.
fn register(ctx: &SessionContext, dump: &Arc<PgDump>) {
    // The catalog is what installs the session's budget and settings.
    register_dump(ctx, Some("dump"), dump, Arc::new(ignore)).unwrap();
    for table in TABLES {
        let provider: Arc<dyn TableProvider> = dump.table(None, None, table).unwrap();
        ctx.register_table(table, provider).unwrap();
    }
}

/// **The answers the typed and untyped modes promise**, from the dump's text
/// and nothing the modes would change: each case table read as text, and
/// each field decoded to its declared type by the library's one decoder, a
/// field it cannot decode, or decodes to a value [`arrow_holds`] refuses,
/// being an unrepresentable value — NULL in
/// [`Mode::Null`]'s tables, and its whole column text in [`Mode::Text`]'s.
/// A case's promise is DataFusion's answer to it over those tables, in one
/// partition.
struct Oracle {
    null: SessionContext,
    text: SessionContext,
}

impl Oracle {
    async fn of(copy: &Path) -> Oracle {
        let strings =
            PgDumpOptions { schema_mode: SchemaMode::Strings, ..PgDumpOptions::default() };
        let text_dump = PgDump::open(copy.to_str().unwrap(), strings).await.unwrap();
        let typed_dump =
            PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
        let reader =
            SessionContext::new_with_config(SessionConfig::new().with_target_partitions(1));
        register(&reader, &text_dump);
        let one = SessionConfig::new().with_target_partitions(1);
        let (null, text) =
            (SessionContext::new_with_config(one.clone()), SessionContext::new_with_config(one));
        for table in TABLES {
            let batches = reader
                .sql(&format!("SELECT * FROM {table}"))
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            let resolved = typed_dump.table(None, None, table).unwrap().resolved_schema().clone();
            let (mut null_columns, mut text_columns, mut text_fields) =
                (Vec::new(), Vec::new(), Vec::new());
            let mut unrepresentable = 0;
            for (i, field) in resolved.schema.fields().iter().enumerate() {
                let mut decoded: Vec<ArrayRef> = Vec::new();
                let mut spelled: Vec<ArrayRef> = Vec::new();
                let mut widened = false;
                for batch in &batches {
                    let column = cast(batch.column(i), &DataType::Utf8View).unwrap();
                    let column = column.as_any().downcast_ref::<arrow::array::StringViewArray>();
                    let column = column.unwrap();
                    for row in 0..column.len() {
                        spelled.push(Arc::new(column.slice(row, 1)));
                        if column.is_null(row) {
                            decoded.push(new_null_array(field.data_type(), 1));
                            continue;
                        }
                        let text = column.value(row);
                        match decode_field(field.data_type(), &resolved.plans[i], text) {
                            Some(value) if arrow_holds(&value).is_ok() => decoded.push(value),
                            _ => {
                                widened = true;
                                unrepresentable += 1;
                                decoded.push(new_null_array(field.data_type(), 1));
                            }
                        }
                    }
                }
                let joined = |parts: &[ArrayRef]| {
                    concat(&parts.iter().map(|a| a.as_ref()).collect::<Vec<_>>()).unwrap()
                };
                null_columns.push(joined(&decoded));
                if widened {
                    text_columns.push(joined(&spelled));
                    text_fields.push(Field::new(field.name(), DataType::Utf8View, true));
                } else {
                    text_columns.push(joined(&decoded));
                    text_fields.push(field.as_ref().clone().with_nullable(true));
                }
            }
            assert!(unrepresentable > 0, "{table} holds no value its typed columns cannot");
            let null_schema = Arc::new(Schema::new(
                resolved
                    .schema
                    .fields()
                    .iter()
                    .map(|f| f.as_ref().clone().with_nullable(true))
                    .collect::<Vec<_>>(),
            ));
            let text_schema = Arc::new(Schema::new(text_fields));
            for (ctx, schema, columns) in
                [(&null, null_schema, null_columns), (&text, text_schema, text_columns)]
            {
                let batch = RecordBatch::try_new(Arc::clone(&schema), columns).unwrap();
                let table_provider = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
                ctx.register_table(table, Arc::new(table_provider)).unwrap();
            }
        }
        Oracle { null, text }
    }

    /// What `mode` promises `case`.
    async fn expected(&self, case: &Case, mode: Mode) -> Expected {
        let ctx = match (mode, case.refuse) {
            (Mode::Refuse, Column(column)) => return Expected::Refused(column.to_string()),
            (Mode::Text, _) => &self.text,
            (Mode::Null | Mode::Refuse, _) => &self.null,
        };
        let (answer, _) = outcome(ctx, case.sql).await;
        let Outcome::Answered { types, rows } = answer else {
            panic!("the oracle refused `{}` under {mode:?}: {answer}", case.sql)
        };
        match (mode, case.refuse) {
            (Mode::Refuse, Answers(rows)) => Expected::Rows {
                types,
                rows: rows.iter().map(|r| r.to_string()).collect(),
                pick: case.pick,
            },
            _ => Expected::Rows { types, rows, pick: case.pick },
        }
    }
}

impl Case {
    fn run_sql(&self) -> String {
        match self.pick {
            Some(n) => format!("{} LIMIT {n}", self.sql),
            None => self.sql.to_string(),
        }
    }
}

/// One case under one mode that did not give the outcome promised.
struct Miss {
    major: u32,
    config: Config,
    got: Outcome,
    expected: Expected,
    /// In a configuration `KD56` meets, of a case it meets.
    excluded: bool,
}

/// Each case and mode's misses over one major, and the partition counts of
/// every scan run.
async fn sweep(major: u32) -> (BTreeMap<(usize, Mode), Vec<Miss>>, BTreeSet<usize>) {
    let scratch = tempfile::tempdir().unwrap();
    let fixture = fixture(major);
    let mut copies = BTreeMap::new();
    for statistics in [Statistics::Gathered, Statistics::Absent] {
        copies.insert(statistics, parsed_copy(&fixture, scratch.path(), statistics).await);
    }
    let oracle = Oracle::of(&copies[&Statistics::Absent]).await;
    let mut dumps = BTreeMap::new();
    for (&statistics, copy) in &copies {
        for mode in MODES {
            let dump = PgDump::open(copy.to_str().unwrap(), options(mode)).await.unwrap();
            dumps.insert((statistics, mode), dump);
        }
    }
    let mut expected = BTreeMap::new();
    for (i, case) in CASES.iter().enumerate() {
        for mode in MODES {
            expected.insert((i, mode), oracle.expected(case, mode).await);
        }
    }
    let (mut misses, mut partitions) = (BTreeMap::<_, Vec<Miss>>::new(), BTreeSet::new());
    for config in every_config() {
        for mode in MODES {
            let ctx = session(config);
            register(&ctx, &dumps[&(config.statistics, mode)]);
            for (i, case) in CASES.iter().enumerate() {
                let (got, plan) = outcome(&ctx, &case.run_sql()).await;
                if let Some(plan) = plan {
                    let mut found = Vec::new();
                    scans(&plan, &mut found);
                    partitions.extend(found);
                }
                let want = &expected[&(i, mode)];
                let misses = misses.entry((i, mode)).or_default();
                if !want.met_by(&got) {
                    let excluded = case.meets_kd56 && config.meets_kd56();
                    misses.push(Miss { major, config, got, expected: want.clone(), excluded });
                }
            }
        }
    }
    (misses, partitions)
}

/// **Every case gives its mode's one outcome in every configuration, but
/// where `KD56` meets it** — and there it fails in at least one, so the pin
/// carrying the fix strikes the exclusion.
#[test]
fn every_query_has_one_outcome_per_mode() {
    let results: Vec<_> = std::thread::scope(|scope| {
        let workers: Vec<_> = MAJORS
            .iter()
            .map(|&major| {
                scope.spawn(move || {
                    let runtime =
                        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                    runtime.block_on(sweep(major))
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    });
    let mut misses: BTreeMap<(usize, Mode), Vec<Miss>> = BTreeMap::new();
    let mut partitions = BTreeSet::new();
    for (major_misses, major_partitions) in results {
        for (key, found) in major_misses {
            misses.entry(key).or_default().extend(found);
        }
        partitions.extend(major_partitions);
    }
    assert!(
        partitions.contains(&2) && partitions.is_subset(&BTreeSet::from([1, 2])),
        "every scan runs two partitions, so the two orders are every order, or one, which has \
         one order: {partitions:?}"
    );

    let mut wrong = Vec::new();
    for (i, case) in CASES.iter().enumerate() {
        let mut kd56_met = false;
        for mode in MODES {
            let (excluded, found): (Vec<&Miss>, Vec<&Miss>) =
                misses[&(i, mode)].iter().partition(|miss| miss.excluded);
            kd56_met |= !excluded.is_empty();
            if found.is_empty() {
                continue;
            }
            let mut lines = format!(
                "`{}` under {mode:?} missed in {} runs, first:",
                case.run_sql(),
                found.len()
            );
            for miss in found.iter().take(4) {
                lines.push_str(&format!(
                    "\n    {} {}: got {}\n      expected {}",
                    miss.major, miss.config, miss.got, miss.expected
                ));
            }
            wrong.push(lines);
        }
        // upstream: UF1
        if case.meets_kd56 && !kd56_met {
            wrong.push(format!(
                "`{}` gives its one outcome where `KD56` met it: if the DataFusion pin carries \
                 the fix, strike the exclusion",
                case.run_sql()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **Whether `value`, one row long, is one its Arrow type holds**, read off
/// DataFusion's own path rather than a bound written per type: `arrow-cast`'s
/// display formats it and its cast to `Utf8` spells it, an error in either
/// — or a cast answering NULL — being a value the type cannot hold. A type
/// the cast does not reach is held to its display alone, which formats each
/// leaf of a nested value; so is a binary type, whose cast to `Utf8` reads
/// the bytes as UTF-8 rather than spelling them, and so refuses `bytea`'s
/// `\xff`, a value `Binary` holds.
fn arrow_holds(value: &ArrayRef) -> Result<(), String> {
    let options = FormatOptions::default();
    let formatter = ArrayFormatter::try_new(value.as_ref(), &options).map_err(|e| e.to_string())?;
    formatter.value(0).try_to_string().map_err(|e| e.to_string())?;
    let binary = matches!(
        value.data_type(),
        DataType::Binary
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::FixedSizeBinary(_)
    );
    if !binary && can_cast_types(value.data_type(), &DataType::Utf8) {
        let strict = CastOptions { safe: false, format_options: options };
        let spelled =
            cast_with_options(value, &DataType::Utf8, &strict).map_err(|e| e.to_string())?;
        if spelled.is_null(0) {
            return Err("its cast to Utf8 answered NULL".to_string());
        }
    }
    Ok(())
}

/// Every major the fixture tree holds.
const EVERY_MAJOR: [u32; 6] = [13, 14, 15, 16, 17, 18];

/// The `types` fixture's extremes: each typed arm's least, greatest and
/// special values, and the nested shapes holding one
/// (`scripts/fixture_schema_types.sql`).
const EXTREMES: [&str; 2] = ["t_extremes", "t_extremes_nested"];

/// Which limit a value is past: the tier the library's count records it in
/// (`pgdump_query::UnrepresentableTier`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Why {
    /// Arrow's format spec: the library's decoder refuses the text.
    Format,
    /// The engine's calendar: the decoder answers a value `arrow-cast`
    /// cannot display or spell.
    Engine,
}

/// **Every extreme outside its column's Arrow type**, as `table.column`, the
/// row's `id` and the tier it is past, at every major whose fixture holds
/// that row.
const UNREPRESENTABLE: &[(&str, i32, Why)] = &[
    // `NaN` under a typmod.
    ("t_extremes.v_numeric38", 5, Why::Format),
    ("t_extremes.v_numeric76", 5, Why::Format),
    // A date past `chrono`'s calendar, which `arrow-cast` displays through:
    // PostgreSQL's greatest, and the day after `262142-12-31`, the last one
    // that displays. And the infinities.
    ("t_extremes.v_date", 2, Why::Engine),
    ("t_extremes.v_date", 15, Why::Engine),
    ("t_extremes.v_date", 3, Why::Format),
    ("t_extremes.v_date", 4, Why::Format),
    // The timestamps alike: PostgreSQL's greatest and the microsecond past
    // `i64`'s are refused by the decoder; `i64`'s last, and the first instant
    // past `chrono`'s calendar, decode to values `arrow-cast` cannot display.
    ("t_extremes.v_ts", 2, Why::Format),
    ("t_extremes.v_ts", 3, Why::Format),
    ("t_extremes.v_ts", 4, Why::Format),
    ("t_extremes.v_ts", 12, Why::Engine),
    ("t_extremes.v_ts", 13, Why::Format),
    ("t_extremes.v_ts", 15, Why::Engine),
    ("t_extremes.v_tstz", 2, Why::Format),
    ("t_extremes.v_tstz", 3, Why::Format),
    ("t_extremes.v_tstz", 4, Why::Format),
    ("t_extremes.v_tstz", 12, Why::Engine),
    ("t_extremes.v_tstz", 13, Why::Format),
    ("t_extremes.v_tstz", 15, Why::Engine),
    // `24:00:00`, past `Time64`'s day.
    ("t_extremes.v_time", 2, Why::Format),
    // A time part past Arrow's nanoseconds either way, the longest a dump
    // holds included, and from 17 the infinities.
    ("t_extremes.v_interval", 6, Why::Format),
    ("t_extremes.v_interval", 7, Why::Format),
    ("t_extremes.v_interval", 8, Why::Format),
    ("t_extremes.v_interval", 10, Why::Format),
    ("t_extremes.v_interval", 17, Why::Format),
    ("t_extremes.v_interval", 18, Why::Format),
    // A nested value holding one is one.
    ("t_extremes_nested.v_date_array", 1, Why::Format),
    ("t_extremes_nested.v_daterange", 1, Why::Format),
    ("t_extremes_nested.v_dated", 1, Why::Format),
    ("t_extremes_nested.v_interval_array", 1, Why::Format),
];

/// What one major's extremes show: each value's `(table.column, id)`, the
/// tier each unrepresentable one is past and why, and the library's count
/// of each column, as the parse recorded it.
struct Extremes {
    present: BTreeSet<(String, i32)>,
    outside: BTreeMap<(String, i32), (Why, String)>,
    counted: BTreeMap<String, Unrepresentable>,
}

/// The library's count of each column of [`EXTREMES`]' blocks in the cache
/// beside `copy`, as `table.column`.
async fn counted(copy: &Path) -> BTreeMap<String, Unrepresentable> {
    let source = LocalFileSource::open(copy).unwrap();
    let CacheStatus::Valid { index, .. } =
        cache::load(&cache::colocated_path(copy), &source).await.unwrap()
    else {
        panic!("the parse left a complete cache")
    };
    let mut out = BTreeMap::new();
    for block in index.blocks().filter(|b| EXTREMES.contains(&b.header.table.as_str())) {
        let counts = block.unrepresentable.as_deref().expect("a data-level block is counted");
        for (column, count) in block.header.columns.iter().zip(counts) {
            let key = format!("{}.{column}", block.header.table);
            out.entry(key).or_insert_with(Unrepresentable::default).merge(count);
        }
    }
    out
}

/// Each extreme, and the library's count of it, over one major.
async fn extremes(major: u32) -> Extremes {
    let scratch = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture(major), scratch.path(), Statistics::Absent).await;
    let counted = counted(&copy).await;
    let strings = PgDumpOptions { schema_mode: SchemaMode::Strings, ..PgDumpOptions::default() };
    let text_dump = PgDump::open(copy.to_str().unwrap(), strings).await.unwrap();
    let typed_dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let reader = SessionContext::new_with_config(SessionConfig::new().with_target_partitions(1));
    let (mut present, mut outside) = (BTreeSet::new(), BTreeMap::new());
    for table in EXTREMES {
        let provider: Arc<dyn TableProvider> = text_dump.table(None, None, table).unwrap();
        reader.register_table(table, provider).unwrap();
        let batches = reader
            .sql(&format!("SELECT * FROM {table} ORDER BY id"))
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let resolved = typed_dump.table(None, None, table).unwrap().resolved_schema().clone();
        for batch in &batches {
            let ids = cast(batch.column(0), &DataType::Int32).unwrap();
            let ids = ids.as_any().downcast_ref::<arrow::array::Int32Array>().unwrap();
            for (i, field) in resolved.schema.fields().iter().enumerate().skip(1) {
                let column = cast(batch.column(i), &DataType::Utf8View).unwrap();
                let column =
                    column.as_any().downcast_ref::<arrow::array::StringViewArray>().unwrap();
                for row in 0..column.len() {
                    if column.is_null(row) {
                        continue;
                    }
                    let key = (format!("{table}.{}", field.name()), ids.value(row));
                    present.insert(key.clone());
                    let text = column.value(row);
                    match decode_field(field.data_type(), &resolved.plans[i], text) {
                        None => {
                            outside.insert(key, (Why::Format, text.to_string()));
                        }
                        Some(value) => {
                            if let Err(err) = arrow_holds(&value) {
                                outside.insert(key, (Why::Engine, format!("{text}: {err}")));
                            }
                        }
                    }
                }
            }
        }
    }
    Extremes { present, outside, counted }
}

/// **The category is what Arrow cannot hold**: every extreme the `types`
/// fixture holds, at every major, decodes to a value [`arrow_holds`] admits
/// or is recorded in [`UNREPRESENTABLE`], with its tier — the record exact
/// both ways, so a value the category gains or loses is a change to it.
///
/// **And the library's count is the record, tier by tier**: each column's
/// count in the map equals the record's values of that column in each tier,
/// so a `chrono` or `arrow-cast` upgrade moving the calendar's end moves
/// the fixture's rows either side of it out of one or the other.
// upstream: UF2
#[test]
fn every_extreme_is_held_by_arrow_or_recorded() {
    let results: Vec<_> = std::thread::scope(|scope| {
        let workers: Vec<_> = EVERY_MAJOR
            .iter()
            .map(|&major| {
                scope.spawn(move || {
                    let runtime =
                        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                    (major, runtime.block_on(extremes(major)))
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    });
    let mut wrong = Vec::new();
    for (major, Extremes { present, outside, counted }) in results {
        let recorded: BTreeMap<(String, i32), Why> = UNREPRESENTABLE
            .iter()
            .map(|&(column, id, why)| ((column.to_string(), id), why))
            .filter(|(key, _)| present.contains(key))
            .collect();
        for (key, (why, detail)) in &outside {
            if recorded.get(key) != Some(why) {
                wrong.push(format!(
                    "{major}: {} id {} is outside ({why:?}), unrecorded: {detail}",
                    key.0, key.1
                ));
            }
        }
        for (key, why) in &recorded {
            if !outside.contains_key(key) {
                wrong.push(format!(
                    "{major}: {} id {} is recorded {why:?} and Arrow holds it",
                    key.0, key.1
                ));
            }
        }
        let mut by_tier: BTreeMap<String, Unrepresentable> = BTreeMap::new();
        for ((column, _), why) in &recorded {
            let count = by_tier.entry(column.clone()).or_default();
            match why {
                Why::Format => count.format += 1,
                Why::Engine => count.engine += 1,
            }
        }
        let counted: BTreeMap<String, Unrepresentable> =
            counted.into_iter().filter(|(_, count)| !count.is_zero()).collect();
        if counted != by_tier {
            wrong.push(format!(
                "{major}: the library counts {counted:?}, where the record says {by_tier:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
