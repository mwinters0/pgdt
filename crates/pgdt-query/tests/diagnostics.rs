//! The diagnostics sink: the file-level, per-column and comparison channels
//! drained into one caller-supplied `DiagnosticSink`, each finding on the
//! shared `Severity` scale with its own sentence.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use pgdt_codec::ColumnNote;
use pgdt_codec::compare::{ComparisonDivergence, ComparisonSemantics};
use pgdt_dump::ScanOptions;
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::build_index;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::drain;
use pgdt_model::{Diagnostic, DiagnosticKind, DiagnosticSink, Finding, Severity};
use pgdt_query::{
    ComparisonNote, Expr, Predicate, PredicateOp, QueryOptions, TableSelector, TableStream,
};

mod common;
use common::{Cold, mapped, types_fixture};

/// A cold query of `table` in the 16 `types` fixture under `options`, its
/// whole table in file order.
async fn query(table: &str, options: QueryOptions) -> TableStream<'static> {
    let source: Arc<dyn ByteRangeSource> =
        Arc::new(LocalFileSource::open(types_fixture(16, "default")).unwrap());
    Cold::default().stream(source, table, options).await.unwrap().expect("the fixture holds it")
}

/// What a sink saw of one finding: which channel's type it was, recovered by
/// downcasting, its severity and its sentence.
#[derive(Debug)]
struct Seen {
    channel: &'static str,
    severity: Severity,
    message: String,
}

fn channel_of(finding: &dyn Finding) -> &'static str {
    let any = finding.as_any();
    if any.is::<Diagnostic>() {
        "file"
    } else if any.is::<ColumnNote>() {
        "column"
    } else if any.is::<ComparisonNote>() {
        "comparison"
    } else {
        "unknown"
    }
}

/// One sink, one closure, three channels: a real index's file-level
/// diagnostics, the resolved schema of a query over a `box` domain column,
/// and the comparison note its `=` term raises (`KD10`) all reach it, each
/// recoverable as its own type and each on the one scale.
#[tokio::test]
async fn one_sink_drains_all_three_channels() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();

    let mut stream = query(
        "public.t_delimiter",
        QueryOptions {
            filter: Expr::all([Predicate {
                column: "v_box_domain".into(),
                op: PredicateOp::Eq("(1,1),(0,0)".into()),
            }]),
            projection: Some(vec!["v_box_domain".to_string()]),
            ..Default::default()
        },
    )
    .await;
    while stream.next().await.transpose().unwrap().is_some() {}

    let seen = Mutex::new(Vec::<Seen>::new());
    let sink = |finding: &dyn Finding| {
        seen.lock().unwrap().push(Seen {
            channel: channel_of(finding),
            severity: finding.severity(),
            message: finding.message(),
        })
    };
    let sink: &dyn DiagnosticSink = &sink;
    drain(sink, &index.diagnostics);
    drain(sink, stream.resolved_schema().notes());
    drain(sink, &stream.comparison_notes());
    let seen = seen.into_inner().unwrap();

    let of = |channel| seen.iter().filter(|s| s.channel == channel).collect::<Vec<_>>();
    assert_eq!(of("file").len(), index.diagnostics.len(), "{seen:#?}");
    assert!(
        index.diagnostics.iter().any(|d| matches!(d.kind(), DiagnosticKind::TocCoverage { .. })),
        "{:?}",
        index.diagnostics
    );
    assert!(of("file").iter().any(|s| s.message.starts_with("TOC coverage: ")), "{seen:#?}");

    let columns = of("column");
    assert_eq!(columns.len(), 1, "{seen:#?}");
    assert!(columns[0].message.starts_with("column `v_box_domain` ("), "{}", columns[0].message);

    let comparisons = of("comparison");
    assert_eq!(comparisons.len(), 1, "{seen:#?}");
    assert_eq!(comparisons[0].severity, Severity::Warning);
    assert!(comparisons[0].message.contains("models no comparison"), "{}", comparisons[0].message);
    assert_eq!(of("unknown").len(), 0);
}

/// A column that maps is an `Info` finding and one that fell back a
/// `Warning`, and the sentence names the column, its declared type and the
/// outcome — what a sink filtering at `Warning` keeps is exactly the columns
/// whose values come back unparsed.
#[tokio::test]
async fn a_column_note_reads_as_a_finding() {
    let mut stream = query("public.t_int", QueryOptions::default()).await;
    while stream.next().await.transpose().unwrap().is_some() {}
    let resolved = stream.resolved_schema();
    let bigint = resolved.notes().find(|n| n.column == "v_bigint").unwrap();
    assert_eq!(bigint.severity(), Severity::Info);
    assert_eq!(bigint.message(), "column `v_bigint` (bigint): mapped");
}

/// **A table's divergences drain at registration, before any term is named**,
/// on the comparison channel and in the semantics asked for: the enum and the
/// label-less enum of `t_enum_domain` report nothing in PostgreSQL's
/// semantics, and in DataFusion's the enum is compared by its label text. The
/// label-less one, emitted as text, earns no second finding beside its
/// `Warning` column note, which says so. The domain over `integer` reports in
/// neither.
#[tokio::test]
async fn a_table_s_divergences_drain_at_registration() {
    let source = Arc::new(LocalFileSource::open(types_fixture(16, "default")).unwrap());
    let dump = mapped(source, &StatisticsRequest::DATA).await;
    let selector = TableSelector::named("t_enum_domain").in_schema("public");
    let table = dump.table(&selector).unwrap().unwrap();
    let resolved = table.schema(&QueryOptions::default()).unwrap();

    let drained = |semantics| {
        let seen = Mutex::new(Vec::new());
        let sink = |finding: &dyn Finding| {
            let note = finding.as_any().downcast_ref::<ComparisonNote>().unwrap();
            assert_eq!(finding.severity(), Severity::Warning);
            seen.lock().unwrap().push((note.column.clone(), note.divergence, finding.message()));
        };
        let options = QueryOptions { semantics, ..QueryOptions::default() };
        drain(&sink, &table.divergences(&options).unwrap());
        seen.into_inner().unwrap()
    };
    assert_eq!(drained(ComparisonSemantics::Postgres), []);
    let arrow = drained(ComparisonSemantics::DataFusion);
    let divergences: Vec<_> = arrow.iter().map(|(c, d, _)| (c.as_str(), *d)).collect();
    assert_eq!(divergences, [("v_mood", ComparisonDivergence::LabelText)]);
    assert!(
        arrow[0].2.starts_with("`v_mood` (public.mood) is compared by its labels' text"),
        "{}",
        arrow[0].2
    );
    let empty = resolved.notes().find(|n| n.column == "v_empty_enum").unwrap();
    assert_eq!(empty.severity(), Severity::Warning);
    assert!(empty.message().ends_with("compares as that text"), "{}", empty.message());
}
