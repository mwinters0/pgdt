//! The library read as an embedder outside this workspace reads it: five
//! journeys over a real dump, naming nothing but the seven libraries' API
//! (`docs/design/decisions.md`, "D106") — the surface
//! `docs/design/roadmap.md`, "Pre-1.0: no compatibility obligations" says
//! this file names, and the reference a binding starts from.
//!
//! `scripts/layering.py` holds this file to the rule it holds every
//! embedder's sources to — no crate's `internal`, no `timed!` or
//! `row_evaluated!` — and to naming no `pgdt_fixtures`, which no embedder
//! can depend on; so it reaches its fixture by path, and every module it
//! declares is read with it.
//!
//! The journeys:
//!
//! 1. open a source and map it with statistics;
//! 2. load the cache that left and judge it;
//! 3. list the tables and their Arrow types;
//! 4. stream with a projection and a filter: serially, partitioned, in file
//!    order, and resumed;
//! 5. summarise.
//!
//! The dump is `fixtures/16/statistics/default.sql`, copied out of the tree
//! first, a mapping pass saving its cache beside it: its `public.ordered`
//! holds a thousand rows, `id` running from 1 to 1000, `low_card` cycling
//! `blue`, `cyan`, `dusk` and `amber`, and `all_null` NULL throughout.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use arrow::array::{Array, Int32Array};
use arrow::datatypes::DataType;
use futures::StreamExt;
use pgdt_core::{Error, Unusable};
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::{CacheMode, CacheStatus, Held, OfflineCache, StrictIdentity, WeakIdentity};
use pgdt_io::{Allowance, Arrangement, ArrangementRequest, Origin};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{DiagnosticKind, PostgresInvalidValues};
use pgdt_query::{
    Dump, Expr, Predicate, PredicateOp, QueryOptions, Table, TablePartitions, TableSelector,
    TableStream,
};
use tokio_util::sync::CancellationToken;

type Rows = Vec<Vec<Option<String>>>;

/// What the embedder states its process may hold resident: well past what
/// the carve holds back, so the workers have a budget and the statistics
/// room (`docs/design/decisions.md`, "D83", "D85").
const ALLOWANCE: u64 = 2 << 30;

/// The fixture, copied into `dir` alone.
fn copied(dir: &Path) -> PathBuf {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/16/statistics/default.sql");
    let path = dir.join("dump.sql");
    std::fs::copy(fixture, &path).unwrap();
    path
}

/// Journey 1's pass: `path` opened through the cache beside it and mapped to
/// its end at the data level, under `jobs` workers the stated allowance
/// carves, the cache saved as the pass goes — and the [`Dump`] it completes.
async fn map(path: &Path, jobs: usize) -> Dump {
    let origin = Origin::local(path);
    let opened = CacheMode::resolve(&origin, None).open(&origin).await.unwrap();
    let request = ArrangementRequest::of(opened.source.as_ref(), Some(jobs));
    let arrangement = Arrangement::resolve(&Allowance::Stated(ALLOWANCE), request);
    let run = map_file(
        opened,
        ReadOptions::default(),
        &arrangement,
        PostgresInvalidValues::default(),
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!run.interrupted, "a token never fired stops nothing");
    Dump::mapped(run).await.unwrap()
}

/// `public.ordered`, selected as a person names it.
fn ordered(dump: &Dump) -> Table {
    dump.table(&TableSelector::named("ordered").in_schema("public")).unwrap().unwrap()
}

/// The projection and filter journey 4 streams: `id` and `low_card` of the
/// rows past 900 that are `blue`.
fn blue_past_900() -> QueryOptions {
    let term = |column: &str, op| Expr::Term(Predicate { column: column.into(), op });
    QueryOptions {
        projection: Some(vec!["id".into(), "low_card".into()]),
        filter: Expr::And(vec![
            term("id", PredicateOp::Gt("900".into())),
            term("low_card", PredicateOp::Eq("blue".into())),
        ]),
        ..QueryOptions::default()
    }
}

/// What is left of `stream`, each value rendered back to PostgreSQL's text
/// through the column record the plan resolved.
async fn drain(stream: &mut TableStream<'static>, plan: &TablePartitions) -> Rows {
    let schema = plan.resolved_schema();
    let mut rows = Rows::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.unwrap();
        for row in 0..batch.num_rows() {
            let fields = batch.columns().iter().zip(schema.columns());
            let rendered = fields.map(|(array, column)| {
                pgdt_query::render_field(array.as_ref(), row, column).unwrap()
            });
            rows.push(rendered.collect());
        }
    }
    rows
}

/// **1. A source opened through its cache and mapped with statistics**: the
/// open finds no cache, the pass maps the whole file and saves one beside
/// it, and the dump it completes holds every table the file declares data
/// for, each block carrying the statistics asked for.
#[tokio::test]
async fn a_source_is_opened_and_mapped_with_statistics() {
    let dir = tempfile::tempdir().unwrap();
    let path = copied(dir.path());
    let origin = Origin::local(&path);
    let opened = CacheMode::resolve(&origin, None).open(&origin).await.unwrap();
    assert_eq!(opened.cache, Held::Missing);
    drop(opened);

    let dump = map(&path, 2).await;
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(dump.index().is_complete(size));
    assert!(path.with_extension("sql.dtcache").is_file(), "the pass saves its cache");
    let table = ordered(&dump);
    assert!(!table.blocks().is_empty());
    assert!(table.blocks().iter().all(|block| block.statistics.is_some()), "{table:?}");
}

/// **2. The cache loaded and judged against the live source**: complete and
/// agreeing, it opens as the [`Dump`] the pass built, and reads alone
/// without the dump; a moved mtime is advisory unless the time binds; and a
/// source whose size changed is refused, naming why.
#[tokio::test]
async fn a_cache_is_loaded_and_judged() {
    let dir = tempfile::tempdir().unwrap();
    let path = copied(dir.path());
    let mapped = map(&path, 1).await;
    let origin = Origin::local(&path);
    let mode = CacheMode::resolve(&origin, None);

    let opened = mode.open(&origin).await.unwrap();
    let Held::Loaded(loaded) = &opened.cache else { panic!("{:?}", opened.cache) };
    assert!(loaded.is_complete());
    assert_eq!(loaded.weak, WeakIdentity::Agrees);
    let dump = Dump::opened(opened).await.unwrap();
    assert_eq!(dump.tables(), mapped.tables());

    let offline = OfflineCache::new(mode.path().unwrap()).load().await.unwrap();
    let CacheStatus::Loaded(offline) = offline else { panic!("{offline:?}") };
    assert!(offline.is_complete());

    let later = SystemTime::now() + Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
    let opened = mode.open(&origin).await.unwrap();
    let Held::Loaded(loaded) = &opened.cache else { panic!("{:?}", opened.cache) };
    assert!(matches!(loaded.weak, WeakIdentity::Differs { .. }), "{:?}", loaded.weak);
    let kinds = loaded.index.diagnostics.iter().map(|d| d.kind());
    assert!(kinds.into_iter().any(|kind| *kind == DiagnosticKind::CacheMtimeChanged));
    let strict = mode.clone().with_strict_identity(StrictIdentity::binding(true, false));
    let refused = strict.open(&origin).await.unwrap_err();
    assert!(matches!(refused, Error::StrictIdentityUnmet { term: "time", .. }), "{refused}");

    let mut grown = std::fs::read(&path).unwrap();
    grown.extend_from_slice(b"\n");
    std::fs::write(&path, grown).unwrap();
    let refused = mode.open(&origin).await.unwrap_err();
    assert!(
        matches!(refused, Error::CacheUnusable { unusable: Unusable::SourceChanged { .. }, .. }),
        "{refused}"
    );
}

/// **3. The tables and their Arrow types**, before a row is read: the
/// tables the file holds data for, one database unnamed, and each column's
/// type as its declaration resolves it.
#[tokio::test]
async fn the_tables_and_their_arrow_types_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let dump = map(&copied(dir.path()), 1).await;
    let names: Vec<String> = dump.tables().iter().map(|t| t.qualified_name()).collect();
    assert!(names.contains(&"public.ordered".to_string()), "{names:?}");
    assert_eq!(dump.databases(), vec![None]);

    let schema = ordered(&dump).schema(&QueryOptions::default()).unwrap();
    let typed: Vec<(&str, &DataType, Option<&str>)> =
        schema.columns().iter().map(|c| (c.name(), c.data_type(), c.declared())).collect();
    assert_eq!(typed.len(), 12, "{typed:?}");
    assert_eq!(typed[0], ("id", &DataType::Int32, Some("integer")));
    assert_eq!(typed[8].0, "low_card");
    assert_eq!(typed[8].1, &DataType::Utf8View);
}

/// **4. A projection under a filter, streamed four ways**: serially; over
/// several partitions, which together hold the serial rows; as one stream in
/// file order, which is them in their order; and paused and resumed at its
/// token, none twice.
#[tokio::test]
async fn a_projection_and_a_filter_stream_four_ways() {
    let dir = tempfile::tempdir().unwrap();
    let dump = map(&copied(dir.path()), 1).await;
    let table = ordered(&dump);
    let expected: Rows = (901..=1000)
        .filter(|id| id % 4 == 1)
        .map(|id| vec![Some(id.to_string()), Some("blue".to_string())])
        .collect();

    let serial = table.plan(ReadOptions::default(), blue_past_900()).unwrap();
    assert_eq!(serial.len(), 1);
    let schema = serial.resolved_schema();
    let names: Vec<&str> = schema.columns().iter().map(|c| c.name()).collect();
    assert_eq!(names, ["id", "low_card"], "the schema is the plan's before any poll");
    assert_eq!(drain(&mut serial.stream(0, 8), &serial).await, expected);

    let request = ArrangementRequest::of(dump.source().as_ref(), Some(4));
    let arrangement = Arrangement::resolve(&Allowance::Stated(ALLOWANCE), request);
    let options = QueryOptions { parallelism: arrangement.parallelism(), ..blue_past_900() };
    let partitioned = table.plan(ReadOptions::default(), options).unwrap();
    assert!(partitioned.len() > 1, "the plan must split for the partitions to differ");
    let mut every = Rows::new();
    for partition in 0..partitioned.len() {
        every.extend(drain(&mut partitioned.stream(partition, 8), &partitioned).await);
    }
    every.sort_by_key(|row| row[0].as_deref().unwrap().parse::<u32>().unwrap());
    assert_eq!(every, expected);

    let in_order = drain(&mut partitioned.in_file_order(8), &partitioned).await;
    assert_eq!(in_order, expected);

    let mut paused = partitioned.in_file_order(5);
    let mut rows = Rows::new();
    let first = paused.next().await.unwrap().unwrap();
    let ids = first.column(0).as_any().downcast_ref::<Int32Array>().unwrap();
    rows.extend(ids.iter().map(|id| vec![id.map(|id| id.to_string()), Some("blue".into())]));
    let mut resumed = partitioned.resume(paused.resume_token(), 5).unwrap();
    rows.extend(drain(&mut resumed, &partitioned).await);
    assert_eq!(rows, expected);
}

/// **5. The table summarised from its statistics**, no row read: its row
/// count, each column's NULLs, `id`'s bounds and `low_card`'s distinct
/// values.
#[tokio::test]
async fn a_table_is_summarised() {
    let dir = tempfile::tempdir().unwrap();
    let dump = map(&copied(dir.path()), 1).await;
    let summary = ordered(&dump).summary(&QueryOptions::default()).unwrap();
    assert_eq!(summary.rows, 1000);

    let id = &summary.columns[0];
    assert_eq!((id.nulls, id.nulls_complete, id.bounds_complete), (0, true, true));
    let bound = |bound: &Option<pgdt_query::Bound>| {
        let bound = bound.as_ref().unwrap();
        assert!(bound.exact);
        bound.value.as_any().downcast_ref::<Int32Array>().unwrap().value(0)
    };
    assert_eq!((bound(&id.min), bound(&id.max)), (1, 1000));
    assert_eq!(summary.columns[5].nulls, 1000, "all_null");
    assert_eq!(summary.columns[8].distinct, Some(4), "low_card");
}
