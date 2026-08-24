//! The full file map (`crate::map`, Phase 3.2): the tiling invariant over
//! every generated fixture plus the hand-written edge-case dump, and
//! targeted classification checks.

use std::path::{Path, PathBuf};

use pgdump_query::map::{SpanBody, TilingIssue};
use pgdump_query::{
    LocalFileSource, ScanOptions, build_index, build_map, check_tiling, dump_metadata_from_spans,
};

fn edge_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql")
}

/// Every real `pg_dump` output file the fixture generator produced, across
/// all six routine versions and all three schemas
/// (`edge_cases`/`objects`/`types`) — including the degenerate shapes the
/// design doc calls out by name: `data-only`, `schema-only`, `inserts`/
/// `column-inserts` (no `COPY` blocks at all), and `dumpall` (concatenated,
/// multi-`\connect`).
fn all_fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    let mut out = Vec::new();
    for version in std::fs::read_dir(&root).unwrap() {
        let version = version.unwrap().path();
        if !version.is_dir() {
            continue;
        }
        for schema in std::fs::read_dir(&version).unwrap() {
            let schema = schema.unwrap().path();
            if !schema.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&schema).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "sql") {
                    out.push(path);
                }
            }
        }
    }
    assert!(!out.is_empty(), "fixture discovery found nothing — did the tree move?");
    out
}

async fn map_of(path: &Path) -> (Vec<pgdump_query::Span>, u64) {
    let source = LocalFileSource::open(path).unwrap();
    let spans = build_map(&source, &ScanOptions::default()).await.unwrap();
    use pgdump_query::ByteRangeSource;
    let size = source.size().await.unwrap();
    (spans, size)
}

/// The standing rule (`roadmap-phase3-object-inventory.md`, "Standing rule:
/// coverage increases monotonically") made concrete: every span list this
/// module can produce tiles its file exactly, with no exemptions.
#[tokio::test]
async fn every_fixture_tiles_exactly() {
    let mut failures = Vec::new();
    for path in all_fixtures() {
        let (spans, size) = map_of(&path).await;
        let issues = check_tiling(&spans, size);
        if !issues.is_empty() {
            failures.push(format!("{}: {issues:?}", path.display()));
        }
    }
    assert!(failures.is_empty(), "tiling violations:\n{}", failures.join("\n"));
}

/// Phase 3.2.1: `build_index` builds its spans via the same `map::Builder`
/// `build_map` drives, fed from the same scan pass as `build_index`'s own
/// `CopyBlock`/metadata extraction — no second pass over the file, and
/// `DumpIndex::blocks()` is a filter over the result rather than a second
/// stored structure (`docs/design/roadmap-phase3-object-inventory.md`, "The
/// map is the structure, not a description of it"). This pins the two
/// producers from drifting apart across every fixture shape, including the
/// hand-written `edge_cases.sql` this file's other tests single out for its
/// TOC-comment-less dollar-quoted functions.
#[tokio::test]
async fn build_index_spans_match_build_map_exactly() {
    for path in all_fixtures().into_iter().chain(std::iter::once(edge_cases())) {
        let source = LocalFileSource::open(&path).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();
        let (spans, _size) = map_of(&path).await;
        assert_eq!(index.spans, spans, "{}", path.display());
    }
}

/// Phase 3.2.1.1: `dump_metadata_from_spans` (span-driven) must recover
/// exactly what `build_index`'s own `PreambleBuilder` pass (line-driven)
/// does, across every fixture shape — multi-database `\connect` segmenting,
/// version-header staging across that boundary, and `--binary-upgrade` enum
/// label folding included. This is the equivalence this slice's cutover
/// (removing the separate `PreambleBuilder` pass from `build_index`) rests
/// on; see `docs/design/roadmap-phase3.2.1-span-wiring-notes.md`.
#[tokio::test]
async fn metadata_from_spans_matches_preamble_builder_exactly() {
    for path in all_fixtures().into_iter().chain(std::iter::once(edge_cases())) {
        let source = LocalFileSource::open(&path).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();
        let from_spans = dump_metadata_from_spans(&index.spans);
        assert_eq!(from_spans, index.metadata.unwrap(), "{}", path.display());
    }
}

/// `tests/data/edge_cases.sql` is hand-written, not real `pg_dump` output —
/// no `-- Name: ...; Type: ...` TOC comments at all, unlike every generated
/// fixture — so it's exactly the "statement-grammar fallback, no TOC
/// header" path, and its two dollar-quoted `CREATE FUNCTION`s (including
/// one whose adversarial body contains lines that look exactly like `COPY`
/// headers, `docs/design/roadmap-phase1-mvp.md`'s dollar-quote-tracking
/// motivation) still have to tile.
#[tokio::test]
async fn edge_cases_dump_tiles_exactly() {
    let (spans, size) = map_of(&edge_cases()).await;
    assert!(check_tiling(&spans, size).is_empty());
    assert!(spans.iter().any(|s| matches!(s.body, SpanBody::Data(_))));
    assert_eq!(
        spans.iter().filter(|s| matches!(s.body, SpanBody::Unparsed)).count(),
        2,
        "the two CREATE FUNCTIONs, with no TOC comment to anchor on, via the statement fallback"
    );
}

/// A schema-only dump (no `COPY` blocks at all) still tiles: every span is
/// DDL/framing, and `check_tiling`'s no-`Data`-span path is exercised.
#[tokio::test]
async fn schema_only_dump_has_no_data_spans_but_still_tiles() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/edge_cases/schema-only.sql");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());
    assert!(!spans.iter().any(|s| matches!(s.body, SpanBody::Data(_))));
}

/// A data-only dump (no DDL) is nothing but `Data` spans (and framing) —
/// still tiles.
#[tokio::test]
async fn data_only_dump_tiles_exactly() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/edge_cases/data-only.sql");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());
    assert!(spans.iter().any(|s| matches!(s.body, SpanBody::Data(_))));
}

/// `--inserts` output has zero `COPY` blocks and, per the module docs,
/// no dedicated `Data`-span fast path in this slice — every `INSERT`
/// becomes its own `Unparsed` span via the generic statement grammar. The
/// value that matters here is tiling across the embedded-raw-newline row
/// (`public.escapes` row 10, `docs/status/history/2026-08-23.md`).
#[tokio::test]
async fn inserts_dump_with_an_embedded_newline_value_still_tiles() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/edge_cases/inserts.sql");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());
    assert!(!spans.iter().any(|s| matches!(s.body, SpanBody::Data(_))));
    assert!(
        spans.iter().filter(|s| matches!(s.body, SpanBody::Unparsed)).count() > 100,
        "expected roughly one Unparsed span per INSERT statement"
    );
}

#[tokio::test]
async fn column_inserts_dump_still_tiles() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/edge_cases/column-inserts.sql");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());
}

/// `dumpall.sql` concatenates several `\connect`-separated dumps, each with
/// its own prologue/epilogue banner — still tiles, and spans after each
/// `\connect` are attributed to the new database.
#[tokio::test]
async fn concatenated_dumpall_tiles_and_attributes_databases_by_connect() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/edge_cases/dumpall.sql");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());

    let databases: std::collections::BTreeSet<Option<String>> =
        spans.iter().map(|s| s.database.clone()).collect();
    assert!(databases.len() > 1, "a concatenated dumpall must span more than one database");
}

/// The `objects` fixture is the one place large objects (I12) and every
/// dollar-quoted-body TOC kind (`FUNCTION`, `AGGREGATE`, `EVENT TRIGGER`,
/// ...) actually appear — the case the module docs' "why TOC-block
/// boundaries" reasoning exists for.
#[tokio::test]
async fn objects_fixture_with_dollar_quoted_function_bodies_tiles_exactly() {
    for flavor in ["default", "verbose"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/18/objects")
            .join(format!("{flavor}.sql"));
        let (spans, size) = map_of(&path).await;
        let issues = check_tiling(&spans, size);
        assert!(issues.is_empty(), "{flavor}: {issues:?}");

        let table_span = spans
            .iter()
            .find(|s| matches!(&s.body, SpanBody::Table { name, .. } if name == "objects.widgets"));
        assert!(table_span.is_some(), "{flavor}: objects.widgets must classify as a Table span");
    }
}

/// A `CREATE TABLE`'s span carries the same name/columns
/// `crate::preamble::PreambleBuilder` would have parsed out of the same
/// statement.
#[tokio::test]
async fn create_table_span_carries_name_and_columns() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/edge_cases/default.sql");
    let (spans, _) = map_of(&path).await;
    let widgets = spans
        .iter()
        .find(|s| matches!(&s.body, SpanBody::Table { name, .. } if name == "public.widgets"))
        .expect("public.widgets must be a Table span");
    let SpanBody::Table { columns, .. } = &widgets.body else { unreachable!() };
    assert!(columns.iter().any(|(name, _)| name == "id"));
}

/// A trailing `ALTER TABLE ... OWNER TO` (no TOC comment of its own) tiles
/// as its own `Unparsed` span, immediately adjacent to its table's span —
/// exactly the "no grouping in this slice" behavior the module docs
/// describe.
#[tokio::test]
async fn alter_owner_to_is_its_own_adjacent_unparsed_span() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/objects/default.sql");
    let (spans, _) = map_of(&path).await;
    let (i, _) = spans
        .iter()
        .enumerate()
        .find(|(_, s)| matches!(&s.body, SpanBody::Table { name, .. } if name == "objects.widgets"))
        .expect("objects.widgets must be a Table span");
    let next = &spans[i + 1];
    assert_eq!(next.body, SpanBody::Unparsed);
    assert_eq!(next.start, spans[i].end, "adjacent spans must share a boundary — no gap");
}

/// The file prologue (banner, `\restrict`, version headers, the
/// `SET`/`set_config` block) and epilogue (`\unrestrict`) all classify as
/// framing, never as an object.
#[tokio::test]
async fn prologue_and_epilogue_classify_as_framing() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/18/edge_cases/default.sql");
    let (spans, _) = map_of(&path).await;
    assert_eq!(spans[0].body, SpanBody::Framing);
    assert_eq!(spans.last().unwrap().body, SpanBody::Framing);
}

#[tokio::test]
async fn tiling_issue_reports_a_gap() {
    use pgdump_query::{Span, SpanBody as Body};
    let spans = vec![
        Span { start: 0, end: 10, database: None, text: None, body: Body::Framing },
        Span { start: 12, end: 20, database: None, text: None, body: Body::Framing },
    ];
    let issues = check_tiling(&spans, 20);
    assert_eq!(
        issues,
        vec![TilingIssue::Discontinuity { after_index: 0, span_end: 10, next_start: 12 }]
    );
}

#[tokio::test]
async fn tiling_issue_reports_an_overlap() {
    use pgdump_query::{Span, SpanBody as Body};
    let spans = vec![
        Span { start: 0, end: 10, database: None, text: None, body: Body::Framing },
        Span { start: 8, end: 20, database: None, text: None, body: Body::Framing },
    ];
    let issues = check_tiling(&spans, 20);
    assert_eq!(
        issues,
        vec![TilingIssue::Discontinuity { after_index: 0, span_end: 10, next_start: 8 }]
    );
}

#[tokio::test]
async fn tiling_issue_reports_a_short_final_span() {
    use pgdump_query::{Span, SpanBody as Body};
    let spans = vec![Span { start: 0, end: 10, database: None, text: None, body: Body::Framing }];
    let issues = check_tiling(&spans, 20);
    assert_eq!(issues, vec![TilingIssue::DoesNotReachEnd { last_end: 10, expected_end: 20 }]);
}

/// An `Unscanned` tail is a legitimate, tiling shape — a prefix-covering
/// partial scan (`roadmap-phase3-object-inventory.md`, "Scan coverage is a
/// prefix, expressed as a span") is not exempt from the invariant.
#[tokio::test]
async fn an_unscanned_tail_tiles_cleanly() {
    use pgdump_query::{Span, SpanBody as Body};
    let spans = vec![
        Span { start: 0, end: 10, database: None, text: None, body: Body::Framing },
        Span { start: 10, end: 20, database: None, text: None, body: Body::Unscanned },
    ];
    assert!(check_tiling(&spans, 20).is_empty());
}

#[tokio::test]
async fn empty_span_list_against_a_zero_length_scan_tiles_cleanly() {
    assert!(check_tiling(&[], 0).is_empty());
}

/// Span text is **sliced from the file by offset**, so it survives the one
/// thing an accumulator cannot: a dollar-quoted function body, for which
/// `crate::scan` emits no `Event::Line` at all
/// (`docs/design/roadmap-phase3-object-inventory.md`, "Span text comes from
/// the file, not from the parser"). If text were accumulated from events,
/// every function body in the file would be missing from it.
#[tokio::test]
async fn span_text_includes_dollar_quoted_bodies_events_never_surface() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/objects/default.sql");
    let source = LocalFileSource::open(&path).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let spans = build_map(&source, &ScanOptions::default()).await.unwrap();

    let with_body =
        spans.iter().filter(|s| s.text.as_ref().is_some_and(|t| t.text.contains("$$"))).count();
    assert!(with_body > 0, "the objects fixture defines dollar-quoted function bodies");

    // Every stored text is exactly the file's own bytes for that range.
    for span in &spans {
        let Some(stored) = &span.text else { continue };
        assert!(!stored.truncated, "no fixture span is anywhere near the 64KB cap");
        assert_eq!(
            stored.text,
            &raw[span.start as usize..span.end as usize],
            "span at {}..{} does not match the file",
            span.start,
            span.end
        );
    }
}

/// `Data` spans never store text (their bytes are unbounded and carry
/// nothing a reader of the *map* wants), and neither does `Unscanned` — its
/// bytes are by definition unread.
#[tokio::test]
async fn data_and_unscanned_spans_store_no_text() {
    for path in all_fixtures() {
        let source = LocalFileSource::open(&path).unwrap();
        let spans = build_map(&source, &ScanOptions::default()).await.unwrap();
        for span in &spans {
            let stores = !matches!(span.body, SpanBody::Data(_) | SpanBody::Unscanned);
            assert_eq!(
                span.text.is_some(),
                stores,
                "{}: span at {} ({:?})",
                path.display(),
                span.start,
                std::mem::discriminant(&span.body)
            );
        }
    }
}

/// The 64KB cap bounds one span's stored text and marks that it did, while
/// the offsets stay whole — so a caller that needs the rest can still read
/// the file.
#[tokio::test]
async fn text_over_the_cap_is_truncated_and_marked() {
    use pgdump_query::{Span, TEXT_CAP, attach_text};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.sql");
    let body = "x".repeat(TEXT_CAP * 2);
    std::fs::write(&path, &body).unwrap();
    let source = LocalFileSource::open(&path).unwrap();

    let mut spans = vec![Span {
        start: 0,
        end: body.len() as u64,
        database: None,
        text: None,
        body: SpanBody::Framing,
    }];
    attach_text(&source, &mut spans).await.unwrap();

    let stored = spans[0].text.as_ref().unwrap();
    assert_eq!(stored.text.len(), TEXT_CAP);
    assert!(stored.truncated);
    assert_eq!(spans[0].end, body.len() as u64, "offsets are untouched by the cap");
}

/// The regression slice 3.2.3 exists for, measured in
/// `docs/status/history/2026-08-23.md`: with no TOC comments, boundary
/// detection used to work until the first dollar-quoted body and then stop
/// working at all — the two functions and the table after them collapsed into
/// **one** `Unparsed` span, because `scan.rs` emits no `Event::Line` for the
/// line carrying a `CREATE FUNCTION`'s own closing `;`, so nothing ever told
/// the accumulator the statement had ended.
///
/// `Event::DollarQuoteEnd` is what tells it. This is exactly the
/// "`pg_dump`-compatible dump from elsewhere in the ecosystem" shape the
/// phase's "Scanning" decision is justified by, and it is why "graceful
/// degradation" is one span per object rather than one span for the rest of
/// the file.
#[tokio::test]
async fn a_header_less_dump_degrades_to_one_span_per_object() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no_toc.sql");
    std::fs::write(
        &path,
        "CREATE TABLE public.a (id integer);\n\
         CREATE FUNCTION public.f() RETURNS integer\n\
         \x20   LANGUAGE sql\n\
         \x20   AS $$ SELECT 1 $$;\n\
         CREATE FUNCTION public.g() RETURNS integer\n\
         \x20   LANGUAGE sql\n\
         \x20   AS $_$ SELECT 2 $_$;\n\
         CREATE TABLE public.c (id integer);\n",
    )
    .unwrap();

    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty(), "{:?}", check_tiling(&spans, size));
    assert_eq!(spans.len(), 4, "one span per object, not one span for the whole file");

    let names: Vec<Option<&str>> = spans
        .iter()
        .map(|s| match &s.body {
            SpanBody::Table { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        vec![Some("public.a"), None, None, Some("public.c")],
        "the table after two dollar-quoted bodies is still recognized as a table"
    );
}
