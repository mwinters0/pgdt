//! What a registration reports, and the single-table form.
//!
//! **A registration's findings are the library's own, named.** Each finding a
//! sink hears is one the library produced — a `Diagnostic`, a `ColumnNote` or
//! a `ComparisonNote`, recovered through `as_any` — or the provider's
//! `RefusedTable` for a table whose plan refuses, with the dump or the table
//! it is about in front of its sentence.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{
    PgDump, PgDumpOptions, RefusedTable, register_dump, register_table_factory,
};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ColumnNote, ComparisonNote, Diagnostic, DiagnosticSink, Finding, LocalFileSource, ScanOptions,
    SchemaMode, Severity, StatisticsRequest, map_file,
};

fn fixture(schema: &str, flag_set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/16")
        .join(schema)
        .join(format!("{flag_set}.sql"))
}

/// `fixture` copied into `dir` beside the complete cache a parse leaves.
async fn parsed_copy(fixture: &Path, dir: &Path) -> PathBuf {
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&copy));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::NONE).await.unwrap();
    copy
}

/// What a sink heard: each finding's severity, sentence and channel.
#[derive(Default)]
struct Heard(Mutex<Vec<(Severity, String, &'static str)>>);

impl DiagnosticSink for Heard {
    fn report(&self, finding: &dyn Finding) {
        let any = finding.as_any();
        let channel = if any.is::<Diagnostic>() {
            "file"
        } else if any.is::<ColumnNote>() {
            "column"
        } else if any.is::<ComparisonNote>() {
            "comparison"
        } else if any.is::<RefusedTable>() {
            "refused"
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
    let copy = parsed_copy(&fixture("types", "default"), dir.path()).await;
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
    let copy = parsed_copy(&fixture("types", "default"), dir.path()).await;
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
    let source = LocalFileSource::open(&dump_path).unwrap();
    let cache = CacheMode::enabled(pgdump_query::cache::colocated_path(&dump_path));
    map_file(&source, &ScanOptions::default(), &cache, &StatisticsRequest::NONE).await.unwrap();
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

/// **`STORED AS PGDUMP` registers the table its options name**, answering
/// what the catalog form answers for it, and reports under the name the
/// statement gave it.
#[tokio::test(flavor = "multi_thread")]
async fn a_pgdump_external_table_answers_as_its_catalog_table() {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed_copy(&fixture("edge_cases", "default"), dir.path()).await;
    let location = copy.to_str().unwrap();
    let heard = Arc::new(Heard::default());
    let ctx = SessionContext::new_with_config(SessionConfig::new().with_target_partitions(2));
    register_table_factory(&ctx, Arc::clone(&heard) as Arc<dyn DiagnosticSink>);
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
    let copy = parsed_copy(&fixture("edge_cases", "default"), dir.path()).await;
    let location = copy.to_str().unwrap();
    let ctx = SessionContext::new();
    register_table_factory(&ctx, Arc::new(|_: &dyn Finding| {}));
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
