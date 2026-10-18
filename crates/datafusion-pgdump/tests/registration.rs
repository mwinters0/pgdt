//! What a registration reports, and the single-table form.
//!
//! **A registration's findings are the library's own, named.** Each finding a
//! sink hears is one the library produced — a `Diagnostic`, a `ColumnNote` or
//! a `ComparisonNote`, recovered through `as_any` — or the provider's
//! `RefusedTable` for a table whose plan refuses, with the dump or the table
//! it is about in front of its sentence. What a refusal is across
//! DataFusion's boundary is held here too.

use pgdt_io::ByteRangeSource;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use datafusion::error::DataFusionError;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{
    PgDump, PgDumpOptions, PgDumpTable, RefusedTable, UncollatedColumns, register_dump,
    register_table_factory,
};
use pgdt_codec::compare::{ComparisonDivergence, ComparisonSemantics};
use pgdt_codec::{ColumnNote, SchemaMode};
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::{self, CacheMode, StrictIdentity};
use pgdt_fixtures::arranged;
use pgdt_fixtures::{parsed_copy, ungathered};
use pgdt_io::LocalFileSource;
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::{
    StatisticsLevel, StatisticsRequest, StatisticsSelection, StatisticsTarget,
};
use pgdt_model::{Diagnostic, DiagnosticSink, Finding, PostgresInvalidValues, Severity};
use pgdt_query::{ComparisonNote, column_divergences};
use tokio_util::sync::CancellationToken;

fn fixture(schema: &str, flag_set: &str) -> PathBuf {
    pgdt_fixtures::fixture(16, schema, flag_set)
}

/// What a sink heard: each finding's severity, sentence and channel, and
/// each registration's uncollated columns whole.
#[derive(Default)]
struct Heard(Mutex<Vec<(Severity, String, &'static str)>>, Mutex<Vec<UncollatedColumns>>);

impl DiagnosticSink for Heard {
    fn report(&self, finding: &dyn Finding) {
        let any = finding.as_any();
        if let Some(uncollated) = any.downcast_ref::<UncollatedColumns>() {
            self.1.lock().unwrap().push(uncollated.clone());
        }
        let channel = if any.is::<Diagnostic>() {
            "file"
        } else if any.is::<ColumnNote>() {
            "column"
        } else if any.is::<ComparisonNote>() {
            "comparison"
        } else if any.is::<RefusedTable>() {
            "refused"
        } else if any.is::<UncollatedColumns>() {
            "uncollated"
        } else {
            "unknown"
        };
        self.0.lock().unwrap().push((finding.severity(), finding.message(), channel));
    }
}

impl Heard {
    fn take(&self) -> Vec<(Severity, String, &'static str)> {
        std::mem::take(&mut self.0.lock().unwrap())
    }

    fn uncollated(&self) -> Vec<UncollatedColumns> {
        std::mem::take(&mut self.1.lock().unwrap())
    }
}

async fn rows(ctx: &SessionContext, sql: &str) -> Result<Vec<RecordBatch>, String> {
    match ctx.sql(sql).await {
        Ok(frame) => frame.collect().await.map_err(|e| e.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

/// **A dump's findings, then every table's, each named as SQL reaches it**:
/// every column of every table the catalog lists is reported once on the
/// column channel, and a column whose comparison in DataFusion diverges from
/// PostgreSQL's says so on the comparison channel.
#[tokio::test(flavor = "multi_thread")]
async fn registration_reports_every_table_s_columns_named_by_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        parsed_copy(&fixture("types", "default"), dir.path(), &StatisticsRequest::DATA).await;
    ungathered(&copy).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let heard = Arc::new(Heard::default());
    let ctx = SessionContext::new();
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&heard) as _).unwrap();
    let heard = heard.take();

    assert!(heard.iter().all(|(_, _, channel)| *channel != "unknown"), "{heard:#?}");
    // The file-level channel speaks first, about the dump.
    let files: Vec<_> = heard.iter().take_while(|(_, _, channel)| *channel == "file").collect();
    assert!(!files.is_empty());
    assert!(files.iter().all(|(_, message, _)| message.starts_with(dump.origin())), "{files:#?}");

    let catalog = ctx.catalog("shop").unwrap();
    for schema in catalog.schema_names() {
        let tables = catalog.schema(&schema).unwrap();
        for table in tables.table_names() {
            let provider = tables.table(&table).await.unwrap().unwrap();
            let prefix = format!("shop.{schema}.{table}: ");
            let columns: Vec<_> = heard
                .iter()
                .filter(|(_, message, channel)| {
                    *channel == "column" && message.starts_with(&prefix)
                })
                .collect();
            assert_eq!(columns.len(), provider.schema().fields().len(), "{prefix}");
        }
    }

    // An enum compares by label text in DataFusion, by declaration order in
    // PostgreSQL.
    assert!(
        heard.iter().any(|(severity, message, channel)| *channel == "comparison"
            && *severity == Severity::Warning
            && message.starts_with("shop.public.")
            && message.contains("public.mood")),
        "{heard:#?}"
    );
}

/// **A dump registered as text warns once per column, and on the column
/// channel alone**: a column that fell back to text earns no comparison
/// finding beside its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_strings_registration_warns_each_column_once() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        parsed_copy(&fixture("types", "default"), dir.path(), &StatisticsRequest::DATA).await;
    ungathered(&copy).await;
    let options = PgDumpOptions { schema_mode: SchemaMode::Strings, ..PgDumpOptions::default() };
    let dump = PgDump::open(copy.to_str().unwrap(), options).await.unwrap();
    let heard = Arc::new(Heard::default());
    register_dump(&SessionContext::new(), Some("shop"), &dump, Arc::clone(&heard) as _).unwrap();
    let heard = heard.take();
    let columns: Vec<_> = heard.iter().filter(|(_, _, channel)| *channel == "column").collect();
    assert!(!columns.is_empty());
    assert!(columns.iter().all(|(severity, _, _)| *severity == Severity::Warning), "{columns:#?}");
    assert!(heard.iter().all(|(_, _, channel)| *channel != "comparison"), "{heard:#?}");
}

/// **A table whose plan refuses is not listed, and its refusal is a finding**:
/// `SHOW TABLES` lists the rest, a query of the rest answers, and a query of
/// it finds no such table — so one table the library cannot plan never fails
/// a listing, and its refusal is said once, at registration.
#[tokio::test(flavor = "multi_thread")]
async fn a_table_whose_plan_refuses_is_reported_and_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    let dump_path = dir.path().join("disagreeing.sql");
    std::fs::write(
        &dump_path,
        "COPY public.t (a, b) FROM stdin;\n1\tx\n\\.\n\n\
         COPY public.t (a, c) FROM stdin;\n4\tz\n\\.\n\n\
         COPY public.ok (a) FROM stdin;\n7\n\\.\n",
    )
    .unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump_path).unwrap());
    let cache = CacheMode::enabled(cache::colocated_path(&dump_path));
    map_file(
        cache.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    ungathered(&dump_path).await;
    let dump = PgDump::open(dump_path.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let heard = Arc::new(Heard::default());
    let ctx = SessionContext::new_with_config(SessionConfig::new().with_information_schema(true));
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&heard) as _).unwrap();

    let refused: Vec<_> =
        heard.take().into_iter().filter(|(_, _, channel)| *channel == "refused").collect();
    assert!(
        matches!(refused.as_slice(), [(Severity::Error, message, _)]
            if message.starts_with("shop.public.t: ") && message.contains("different columns")),
        "{refused:#?}"
    );
    let schema = ctx.catalog("shop").unwrap().schema("public").unwrap();
    assert_eq!(schema.table_names(), ["ok"]);
    assert!(!schema.table_exist("t"));

    let listed = rows(&ctx, "SHOW TABLES").await.unwrap();
    let listed = arrow::util::pretty::pretty_format_batches(&listed).unwrap().to_string();
    assert!(listed.contains("| ok ") && !listed.contains("| t "), "{listed}");
    let ok = rows(&ctx, "SELECT * FROM shop.public.ok").await.unwrap();
    assert_eq!(ok.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
    let err = rows(&ctx, "SELECT * FROM shop.public.t").await.unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

/// **A table the cache holds at the metadata level is not listed, and its
/// refusal names the parse that puts it at the data level**; the table that
/// parse leaves at the data level is listed and answers, and the strings
/// schema mode, which reads no census, lists both
/// (`docs/design/decisions.md`, "D35").
#[tokio::test(flavor = "multi_thread")]
async fn a_metadata_level_table_is_refused_naming_its_parse() {
    let dir = tempfile::tempdir().unwrap();
    let dump_path = dir.path().join("levels.sql");
    std::fs::write(
        &dump_path,
        "COPY public.meta (a) FROM stdin;\n1\n\\.\n\n\
         COPY public.ok (a) FROM stdin;\n7\n\\.\n",
    )
    .unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump_path).unwrap());
    let cache = CacheMode::enabled(cache::colocated_path(&dump_path));
    let levels = StatisticsRequest {
        selection: StatisticsSelection {
            default: StatisticsLevel::Metadata,
            overrides: vec![(StatisticsTarget::Table("public.ok".into()), StatisticsLevel::Data)],
        },
        ..StatisticsRequest::DATA
    };
    map_file(
        cache.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        &levels,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let location = dump_path.to_str().unwrap();

    let dump = PgDump::open(location, PgDumpOptions::default()).await.unwrap();
    let heard = Arc::new(Heard::default());
    let ctx = SessionContext::new();
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&heard) as _).unwrap();
    let refused: Vec<_> =
        heard.take().into_iter().filter(|(_, _, channel)| *channel == "refused").collect();
    let parse =
        format!("pgdt parse --source {location} --statistics-level metadata,public.meta=data");
    assert!(
        matches!(refused.as_slice(), [(Severity::Error, message, _)]
            if message.starts_with("shop.public.meta: ") && message.contains(&parse)),
        "{refused:#?}"
    );
    let schema = ctx.catalog("shop").unwrap().schema("public").unwrap();
    assert_eq!(schema.table_names(), ["ok"]);
    let ok = rows(&ctx, "SELECT * FROM shop.public.ok").await.unwrap();
    assert_eq!(ok.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);

    let strings = PgDumpOptions { schema_mode: SchemaMode::Strings, ..PgDumpOptions::default() };
    let dump = PgDump::open(location, strings).await.unwrap();
    let ctx = SessionContext::new();
    register_dump(&ctx, Some("shop"), &dump, Arc::new(Heard::default())).unwrap();
    let schema = ctx.catalog("shop").unwrap().schema("public").unwrap();
    assert_eq!(schema.table_names(), ["meta", "ok"]);
    let meta = rows(&ctx, "SELECT * FROM shop.public.meta").await.unwrap();
    assert_eq!(meta.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
}

/// **`STORED AS PGDUMP` registers the table its options name**, answering
/// what the catalog form answers for it, and reports under the name the
/// statement gave it.
#[tokio::test(flavor = "multi_thread")]
async fn a_pgdump_external_table_answers_as_its_catalog_table() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        parsed_copy(&fixture("edge_cases", "default"), dir.path(), &StatisticsRequest::DATA).await;
    ungathered(&copy).await;
    let location = copy.to_str().unwrap();
    let heard = Arc::new(Heard::default());
    let ctx = SessionContext::new_with_config(SessionConfig::new().with_target_partitions(2));
    register_table_factory(
        &ctx,
        Arc::clone(&heard) as Arc<dyn DiagnosticSink>,
        StrictIdentity::ADVISORY,
    );
    rows(
        &ctx,
        &format!(
            "CREATE EXTERNAL TABLE ev STORED AS PGDUMP LOCATION '{location}' \
             OPTIONS ('pgdump.table' 'events', 'pgdump.schema' 'logs')"
        ),
    )
    .await
    .unwrap();
    let heard = heard.take();
    assert!(
        heard
            .iter()
            .any(|(_, message, channel)| *channel == "column" && message.starts_with("ev: ")),
        "{heard:#?}"
    );

    let dump = PgDump::open(location, PgDumpOptions::default()).await.unwrap();
    register_dump(&ctx, Some("shop"), &dump, Arc::new(|_: &dyn Finding| {})).unwrap();
    let alone = rows(&ctx, "SELECT * FROM ev ORDER BY event_id").await.unwrap();
    let catalogued = rows(&ctx, "SELECT * FROM shop.logs.events ORDER BY event_id").await.unwrap();
    assert_eq!(alone, catalogued);
    assert!(alone.iter().map(RecordBatch::num_rows).sum::<usize>() > 0);

    // The table's name alone suffices where it is unique, and the schema mode
    // is an option.
    rows(
        &ctx,
        &format!(
            "CREATE EXTERNAL TABLE ev_text STORED AS PGDUMP LOCATION '{location}' \
             OPTIONS ('pgdump.table' 'events', 'pgdump.schema_mode' 'strings')"
        ),
    )
    .await
    .unwrap();
    let text = ctx.table("ev_text").await.unwrap();
    assert!(
        text.schema()
            .fields()
            .iter()
            .all(|f| *f.data_type() == arrow::datatypes::DataType::Utf8View)
    );
}

/// **What the statement cannot mean is refused, naming the remedy**: no
/// table named, an option outside the set, columns declared, a table the
/// dump does not hold, a dump with no complete cache.
#[tokio::test(flavor = "multi_thread")]
async fn a_pgdump_external_table_refuses_what_it_cannot_mean() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        parsed_copy(&fixture("edge_cases", "default"), dir.path(), &StatisticsRequest::DATA).await;
    ungathered(&copy).await;
    let location = copy.to_str().unwrap();
    let ctx = SessionContext::new();
    register_table_factory(&ctx, Arc::new(|_: &dyn Finding| {}), StrictIdentity::ADVISORY);
    let create = |name: &str, columns: &str, options: &str| {
        format!(
            "CREATE EXTERNAL TABLE {name} {columns} STORED AS PGDUMP LOCATION '{location}' \
             OPTIONS ({options})"
        )
    };
    let refused = |sql: String| {
        let ctx = &ctx;
        async move { rows(ctx, &sql).await.unwrap_err() }
    };

    let err = refused(create("a", "", "'pgdump.schema' 'logs'")).await;
    assert!(err.contains("pgdump.table"), "{err}");
    let err = refused(create("b", "", "'pgdump.table' 'events', 'pgdump.tabel' 'x'")).await;
    assert!(err.contains("pgdump.tabel") && err.contains("not a PGDUMP option"), "{err}");
    let err = refused(create("c", "", "'pgdump.table' 'events', 'pgdump.schema_mode' 'x'")).await;
    assert!(err.contains("schema_mode"), "{err}");
    let err = refused(create("d", "(event_id BIGINT)", "'pgdump.table' 'events'")).await;
    assert!(err.contains("declare none"), "{err}");
    let err = refused(create("e", "", "'pgdump.table' 'no_such_table'")).await;
    assert!(err.contains("no table `no_such_table`"), "{err}");

    let bare = dir.path().join("bare.sql");
    std::fs::copy(fixture("edge_cases", "default"), &bare).unwrap();
    let err = refused(format!(
        "CREATE EXTERNAL TABLE f STORED AS PGDUMP LOCATION '{}' OPTIONS ('pgdump.table' 'events')",
        bare.display()
    ))
    .await;
    assert!(err.contains(&format!("pgdt parse --source {}", bare.display())), "{err}");
}

/// **A statement's `pgdump.strict_identity` is its dump's, the session's the
/// default**: over a dump touched since its parse, a session binding `time`
/// refuses a statement stating nothing and opens one stating `advisory` or
/// `none`, and an
/// advisory session opens a statement stating nothing and refuses one stating
/// `time`. A value outside the grammar is refused, naming it.
#[tokio::test(flavor = "multi_thread")]
async fn a_statement_s_strictness_overrides_the_session_s() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        parsed_copy(&fixture("edge_cases", "default"), dir.path(), &StatisticsRequest::DATA).await;
    ungathered(&copy).await;
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&copy).unwrap().set_modified(future).unwrap();
    let location = copy.to_str().unwrap();
    let create = |name: &str, strict: Option<&str>| {
        let strict = strict.map_or(String::new(), |s| format!(", 'pgdump.strict_identity' '{s}'"));
        format!(
            "CREATE EXTERNAL TABLE {name} STORED AS PGDUMP LOCATION '{location}' \
             OPTIONS ('pgdump.table' 'events'{strict})"
        )
    };
    let session = |strict| {
        let ctx = SessionContext::new();
        register_table_factory(&ctx, Arc::new(|_: &dyn Finding| {}), strict);
        ctx
    };

    let strict = session(StrictIdentity::binding(true, false));
    let err = rows(&strict, &create("a", None)).await.unwrap_err();
    assert!(err.contains("`--strict-identity=time`"), "{err}");
    rows(&strict, &create("b", Some("none"))).await.unwrap();
    assert!(!rows(&strict, "SELECT * FROM b").await.unwrap().is_empty());
    rows(&strict, &create("b2", Some("advisory"))).await.unwrap();
    assert!(!rows(&strict, "SELECT * FROM b2").await.unwrap().is_empty());

    let advisory = session(StrictIdentity::ADVISORY);
    rows(&advisory, &create("c", None)).await.unwrap();
    let err = rows(&advisory, &create("d", Some("time"))).await.unwrap_err();
    assert!(err.contains("`--strict-identity=time`"), "{err}");
    let err = rows(&advisory, &create("e", Some("tiem"))).await.unwrap_err();
    assert!(err.contains("pgdump.strict_identity `tiem`"), "{err}");
}

/// The library's error beneath `err`, which must be the provider's own
/// [`datafusion_pgdump::Error`] at the root of what DataFusion carries.
fn library_error(err: &DataFusionError) -> &pgdt_core::Error {
    let DataFusionError::External(top) = err.find_root() else {
        panic!("not an external error: {err:?}")
    };
    let ours = top.downcast_ref::<datafusion_pgdump::Error>();
    core_beneath(ours.unwrap_or_else(|| panic!("not the provider's error: {err:?}")))
}

/// The first `pgdt_core::Error` a walk of `err`'s sources reaches.
fn core_beneath<'a>(err: &'a (dyn std::error::Error + 'static)) -> &'a pgdt_core::Error {
    let mut cause = Some(err);
    while let Some(err) = cause {
        if let Some(core) = err.downcast_ref() {
            return core;
        }
        cause = err.source();
    }
    panic!("no library error beneath {err}")
}

/// **Every refusal of a dump crossing DataFusion's boundary is the provider's
/// `Error`, the library's beneath it** (`docs/design/decisions.md`, "D74"): a scan's
/// refusal, a `STORED AS PGDUMP` statement's and a table built by hand each
/// downcast to `datafusion_pgdump::Error` and reach `pgdt_core::Error` by a
/// walk of their sources; `PgDump::table` returns its own variants unwrapped.
#[tokio::test(flavor = "multi_thread")]
async fn every_refusal_is_the_provider_s_error_over_the_library_s() {
    let dir = tempfile::tempdir().unwrap();
    let dump_path = dir.path().join("refusing.sql");
    std::fs::write(
        &dump_path,
        "CREATE TABLE public.t (\n    a integer\n);\n\n\
         COPY public.t (a) FROM stdin;\n1\nabc\n\\.\n\n\
         COPY public.u (a, b) FROM stdin;\n1\tx\n\\.\n\n\
         COPY public.u (a, c) FROM stdin;\n4\tz\n\\.\n",
    )
    .unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump_path).unwrap());
    let cache = CacheMode::enabled(cache::colocated_path(&dump_path));
    let ignoring = PostgresInvalidValues::Ignore;
    map_file(
        cache.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        ignoring,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let dump = PgDump::open(dump_path.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();

    let err = dump.table(None, None, "u").unwrap_err();
    assert!(matches!(err, datafusion_pgdump::Error::Library(_)), "{err:?}");
    core_beneath(&err);
    let name = dump.tables().iter().find(|name| name.table == "u").unwrap().clone();
    library_error(&PgDumpTable::new(Arc::clone(&dump), name).unwrap_err());

    let ctx = SessionContext::new();
    ctx.register_table("t", dump.table(None, None, "t").unwrap()).unwrap();
    let err = ctx.sql("SELECT * FROM t").await.unwrap().collect().await.unwrap_err();
    assert!(
        matches!(
            library_error(&err),
            pgdt_core::Error::FieldRefused { .. } | pgdt_core::Error::FieldDecode { .. }
        ),
        "{err:?}"
    );

    let foreign = dir.path().join("foreign.sql");
    std::fs::copy(&dump_path, &foreign).unwrap();
    std::fs::write(cache::colocated_path(&foreign), "not a cache").unwrap();
    register_table_factory(&ctx, Arc::new(|_: &dyn Finding| {}), StrictIdentity::ADVISORY);
    let create = format!(
        "CREATE EXTERNAL TABLE f STORED AS PGDUMP LOCATION '{}' OPTIONS ('pgdump.table' 't')",
        foreign.display()
    );
    let err = ctx.sql(&create).await.unwrap_err();
    assert!(matches!(library_error(&err), pgdt_core::Error::CacheUnusable { .. }), "{err:?}");
}

/// **`PgDump::table` refusing a table the cache holds at the metadata level
/// is its own variant**, matched as it is returned.
#[tokio::test(flavor = "multi_thread")]
async fn a_table_asked_for_at_the_metadata_level_is_refused_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let dump_path = dir.path().join("meta.sql");
    std::fs::write(&dump_path, "COPY public.meta (a) FROM stdin;\n1\n\\.\n").unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump_path).unwrap());
    let cache = CacheMode::enabled(cache::colocated_path(&dump_path));
    map_file(
        cache.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        &StatisticsRequest::METADATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let dump = PgDump::open(dump_path.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let err = dump.table(None, None, "meta").unwrap_err();
    assert!(matches!(err, datafusion_pgdump::Error::MetadataLevel { .. }), "{err:?}");
}

/// **A registration's columns compared bytewise for want of a recorded
/// collation are one finding, last**, naming the registration, every such
/// column of every table it lists and the library's reason, counting the
/// tables apart; no column of them is a finding of its own. The dump's is
/// named by its origin, a statement's by the table it names.
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_s_uncollated_columns_are_one_finding() {
    let dir = tempfile::tempdir().unwrap();
    let copy =
        parsed_copy(&fixture("types", "default"), dir.path(), &StatisticsRequest::DATA).await;
    ungathered(&copy).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let heard = Arc::new(Heard::default());
    let ctx = SessionContext::new();
    register_dump(&ctx, Some("shop"), &dump, Arc::clone(&heard) as _).unwrap();

    let mut expected = BTreeSet::new();
    let mut reason = None;
    let catalog = ctx.catalog("shop").unwrap();
    for schema in catalog.schema_names() {
        for table in catalog.schema(&schema).unwrap().table_names() {
            let provider = dump.table(None, Some(&schema), &table).unwrap();
            let named = format!("shop.{schema}.{table}");
            let divergences =
                column_divergences(provider.resolved_schema(), ComparisonSemantics::DataFusion);
            for note in divergences {
                if note.divergence == ComparisonDivergence::UnknownCollation {
                    expected.insert((named.clone(), note.column.clone()));
                    reason = note.bytewise_reason();
                }
            }
        }
    }
    let tables: BTreeSet<_> = expected.iter().map(|(table, _)| table).collect();
    assert!(tables.len() > 1, "the fixture's uncollated columns span tables: {expected:?}");

    let all = heard.take();
    let [uncollated] = heard.uncollated().try_into().unwrap();
    assert_eq!(uncollated.subject, dump.origin());
    assert_eq!(uncollated.columns, expected);
    assert_eq!(Some(uncollated.reason.as_str()), reason);
    assert_eq!(
        uncollated.message(),
        format!(
            "{}: {} column(s) in {} table(s) are each compared bytewise: {}",
            dump.origin(),
            expected.len(),
            tables.len(),
            reason.unwrap()
        )
    );
    assert_eq!(all.last().map(|(_, _, channel)| *channel), Some("uncollated"), "{all:#?}");
    assert!(
        !all.iter().any(|(_, message, channel)| *channel == "comparison"
            && message.contains("no COLLATE clause")),
        "{all:#?}"
    );

    let (table, _) = expected.first().unwrap();
    let [_, schema, table] = table.split('.').collect::<Vec<_>>().try_into().unwrap();
    register_table_factory(&ctx, Arc::clone(&heard) as _, StrictIdentity::ADVISORY);
    let create = format!(
        "CREATE EXTERNAL TABLE named STORED AS PGDUMP LOCATION '{}' \
         OPTIONS ('pgdump.table' '{table}', 'pgdump.schema' '{schema}')",
        copy.display(),
    );
    rows(&ctx, &create).await.unwrap();
    let [statement] = heard.uncollated().try_into().unwrap();
    assert_eq!(statement.subject, "named");
    assert!(!statement.columns.is_empty());
    assert!(statement.columns.iter().all(|(table, _)| table == "named"), "{statement:?}");
}
