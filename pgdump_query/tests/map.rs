//! The full file map (`crate::map`): the tiling invariant over every
//! generated fixture plus the hand-written edge-case dump, targeted
//! targeted classification checks, and TOC-enrichment checks.

use std::path::Path;

use pgdump_query::map::{SpanBody, TilingIssue};
use pgdump_query::{
    DiagnosticKind, LocalFileSource, ScanOptions, build_index, build_map, check_tiling,
    dump_metadata_from_spans,
};

mod common;
use common::{all_fixtures, edge_cases, edge_cases_fixture, objects_fixture};

async fn map_of(path: &Path) -> (Vec<pgdump_query::Span>, u64) {
    let source = LocalFileSource::open(path).unwrap();
    let spans = build_map(&source, &ScanOptions::default()).await.unwrap();
    use pgdump_query::ByteRangeSource;
    let size = source.size().await.unwrap();
    (spans, size)
}

/// The standing rule (`docs/design/roadmap.md`, "Coverage increases
/// monotonically") made concrete: every span list this module can produce
/// tiles its file exactly, with no exemptions.
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

/// `build_index` builds its spans via the same `map::Builder`
/// `build_map` drives, fed from the same scan pass as `build_index`'s own
/// `CopyBlock`/metadata extraction — no second pass over the file, and
/// `DumpIndex::blocks()` is a filter over the result rather than a second
/// stored structure (`docs/design/decisions.md`, "D34"). This pins the two
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

/// `dump_metadata_from_spans` (span-driven) must recover exactly what a
/// line-driven preamble pass does, across every fixture shape — multi-database `\connect` segmenting,
/// version-header staging across that boundary, and `--binary-upgrade` enum
/// label folding included (`docs/design/decisions.md`, "D30").
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
/// headers, `docs/design/decisions.md`, "D30") still have to tile.
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
    let path = edge_cases_fixture(18, "schema-only");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());
    assert!(!spans.iter().any(|s| matches!(s.body, SpanBody::Data(_))));
}

/// A data-only dump (no DDL) is nothing but `Data` spans (and framing) —
/// still tiles.
#[tokio::test]
async fn data_only_dump_tiles_exactly() {
    let path = edge_cases_fixture(18, "data-only");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());
    assert!(spans.iter().any(|s| matches!(s.body, SpanBody::Data(_))));
}

/// `--inserts` output has zero `COPY` blocks; per the `Data`-span fast path, a whole table's run of `INSERT INTO` statements is one `Data`
/// span rather than one `Unparsed` span per statement — this fixture has six
/// `TABLE DATA` entries (`logs.events`, `public.dropped_column`,
/// `public.empty_table` [zero rows — absorbed into the next entry's span, per
/// `crate::map`'s comment-boundary rules, rather than getting one of its
/// own], `public.escapes`, `public.generated_column`, `public.widgets`), so
/// five `Data(InsertRun)` spans, not 132-plus `Unparsed` ones. The value that
/// matters here is tiling across the embedded-raw-newline row
/// (`public.escapes` row 10) and the zero-row table.
#[tokio::test]
async fn inserts_dump_with_an_embedded_newline_value_still_tiles() {
    use pgdump_query::DataBlock;

    let path = edge_cases_fixture(18, "inserts");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());

    let runs: Vec<&pgdump_query::InsertRun> = spans
        .iter()
        .filter_map(|s| match &s.body {
            SpanBody::Data(DataBlock::InsertRun(run)) => Some(run),
            _ => None,
        })
        .collect();
    let tables: Vec<&str> = runs.iter().map(|r| r.table.as_str()).collect();
    assert_eq!(
        tables,
        vec![
            "logs.events",
            "public.dropped_column",
            "public.escapes",
            "public.generated_column",
            "public.widgets",
        ]
    );
    let escapes = runs.iter().find(|r| r.table == "public.escapes").unwrap();
    assert_eq!(escapes.row_count, 132, "the same 132 codepoint rows tests/scan.rs checks");

    assert!(
        spans.iter().filter(|s| matches!(s.body, SpanBody::Unparsed)).count() < 20,
        "no more Unparsed spans than framing/DDL lines — INSERT rows must not leak into it"
    );
}

/// Every `INSERT` run carries the `-- Data for Name: ...` entry that heads
/// it, and owns it — the `COPY` path's attribution, on the `--inserts` path.
/// The run's span starts at that comment, so the entry is attributed exactly
/// once rather than to a `Framing` span the run then fails to inherit from
/// (`Framing` is one of the three kinds inheritance never crosses).
///
/// `public.empty_table` is why this checks the runs rather than the entries:
/// it has zero rows, so its comment block runs straight into the next one and
/// only the later entry survives — the same absorption the tiling test above
/// documents.
#[tokio::test]
async fn every_insert_run_owns_its_data_entry() {
    use pgdump_query::DataBlock;

    let path = edge_cases_fixture(18, "inserts");
    let (spans, _size) = map_of(&path).await;

    let attributed: Vec<(&str, &str, &str)> = spans
        .iter()
        .filter_map(|s| match &s.body {
            SpanBody::Data(DataBlock::InsertRun(run)) => {
                let toc = s
                    .toc
                    .as_ref()
                    .unwrap_or_else(|| panic!("INSERT run for {} carries no TOC entry", run.table));
                assert!(s.toc_owned, "INSERT run for {} inherited its entry", run.table);
                Some((run.table.as_str(), toc.name.as_str(), toc.kind.as_str()))
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        attributed,
        vec![
            ("logs.events", "events", "TABLE DATA"),
            ("public.dropped_column", "dropped_column", "TABLE DATA"),
            ("public.escapes", "escapes", "TABLE DATA"),
            ("public.generated_column", "generated_column", "TABLE DATA"),
            ("public.widgets", "widgets", "TABLE DATA"),
        ]
    );
}

#[tokio::test]
async fn column_inserts_dump_still_tiles() {
    let path = edge_cases_fixture(18, "column-inserts");
    let (spans, size) = map_of(&path).await;
    assert!(check_tiling(&spans, size).is_empty());
}

/// `dumpall.sql` concatenates several `\connect`-separated dumps, each with
/// its own prologue/epilogue banner — still tiles, and spans after each
/// `\connect` are attributed to the new database.
#[tokio::test]
async fn concatenated_dumpall_tiles_and_attributes_databases_by_connect() {
    let path = edge_cases_fixture(18, "dumpall");
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
        let path = objects_fixture(18, flavor);
        let (spans, size) = map_of(&path).await;
        let issues = check_tiling(&spans, size);
        assert!(issues.is_empty(), "{flavor}: {issues:?}");

        let table_span = spans
            .iter()
            .find(|s| matches!(&s.body, SpanBody::Table { name, .. } if name == "objects.widgets"));
        assert!(table_span.is_some(), "{flavor}: objects.widgets must classify as a Table span");
    }
}

/// The large-object data region (I12) is one `Data(LargeObjects)` span
/// regardless of how many archive entries `pg_dump` split it across: v13-16
/// wraps every object's `lo_open`/`lowrite`/`lo_close` run in a single
/// `BEGIN;`/`COMMIT;` pair, v17+ gives each object its own pair — both
/// fixtures define exactly two large objects
/// (`scripts/fixture_schema_objects.sql`), and both must produce exactly one
/// merged span, not two.
#[tokio::test]
async fn the_large_object_region_is_one_data_span_on_every_routine_version() {
    use pgdump_query::DataBlock;

    for version in [13, 14, 15, 16, 17, 18] {
        let path = objects_fixture(version, "default");
        let (spans, size) = map_of(&path).await;
        assert!(check_tiling(&spans, size).is_empty(), "pg_dump {version}");

        let large_object_spans: Vec<_> = spans
            .iter()
            .filter(|s| matches!(&s.body, SpanBody::Data(DataBlock::LargeObjects(_))))
            .collect();
        assert_eq!(
            large_object_spans.len(),
            1,
            "pg_dump {version}: exactly one merged large-object span, found {large_object_spans:?}"
        );

        // Nothing from inside the region — `BEGIN;`, `lo_open`/`lowrite`/
        // `lo_close`, `COMMIT;` — leaks out as its own `Unparsed` span.
        assert!(
            !spans.iter().any(|s| {
                s.text
                    .as_ref()
                    .is_some_and(|t| t.text.contains("lowrite") || t.text.trim() == "BEGIN;")
            }),
            "pg_dump {version}: large-object region content must not appear in any stored span text"
        );
    }
}

/// A `CREATE TABLE`'s span carries the same name/columns
/// `crate::preamble::PreambleBuilder` would have parsed out of the same
/// statement.
#[tokio::test]
async fn create_table_span_carries_name_and_columns() {
    let path = edge_cases_fixture(18, "default");
    let (spans, _) = map_of(&path).await;
    let widgets = spans
        .iter()
        .find(|s| matches!(&s.body, SpanBody::Table { name, .. } if name == "public.widgets"))
        .expect("public.widgets must be a Table span");
    let SpanBody::Table { columns, .. } = &widgets.body else { unreachable!() };
    assert!(columns.iter().any(|c| c.name == "id"));
}

/// A trailing `ALTER TABLE ... OWNER TO` (no TOC comment of its own) tiles
/// as its own `Unparsed` span, immediately adjacent to its table's span —
/// no grouping (`docs/design/decisions.md`, "D31") — but it
/// **inherits** `objects.widgets`' own TOC header rather than carrying
/// `None`: the two spans are attributed to the same entry, and only the
/// first carries the header text itself.
#[tokio::test]
async fn alter_owner_to_is_its_own_adjacent_unparsed_span() {
    let path = objects_fixture(18, "default");
    let (spans, _) = map_of(&path).await;
    let (i, _) = spans
        .iter()
        .enumerate()
        .find(|(_, s)| matches!(&s.body, SpanBody::Table { name, .. } if name == "objects.widgets"))
        .expect("objects.widgets must be a Table span");
    let next = &spans[i + 1];
    assert_eq!(next.body, SpanBody::Unparsed);
    assert_eq!(next.start, spans[i].end, "adjacent spans must share a boundary — no gap");
    assert!(spans[i].toc_owned, "objects.widgets' own comment carried the header");
    assert!(!next.toc_owned, "the OWNER TO follow-on inherited it instead");
    assert_eq!(next.toc, spans[i].toc, "both spans belong to the same TOC entry");
}

/// The file prologue (banner, `\restrict`, version headers, the
/// `SET`/`set_config` block) and epilogue (`\unrestrict`) all classify as
/// framing, never as an object.
#[tokio::test]
async fn prologue_and_epilogue_classify_as_framing() {
    let path = edge_cases_fixture(18, "default");
    let (spans, _) = map_of(&path).await;
    assert_eq!(spans[0].body, SpanBody::Framing);
    assert_eq!(spans.last().unwrap().body, SpanBody::Framing);
}

#[tokio::test]
async fn tiling_issue_reports_a_gap() {
    use pgdump_query::{Span, SpanBody as Body};
    let spans = vec![
        Span {
            start: 0,
            end: 10,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: Body::Framing,
        },
        Span {
            start: 12,
            end: 20,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: Body::Framing,
        },
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
        Span {
            start: 0,
            end: 10,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: Body::Framing,
        },
        Span {
            start: 8,
            end: 20,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: Body::Framing,
        },
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
    let spans = vec![Span {
        start: 0,
        end: 10,
        database: None,
        text: None,
        toc: None,
        toc_owned: false,
        body: Body::Framing,
    }];
    let issues = check_tiling(&spans, 20);
    assert_eq!(issues, vec![TilingIssue::DoesNotReachEnd { last_end: 10, expected_end: 20 }]);
}

/// An `Unscanned` tail is a legitimate, tiling shape — a prefix-covering
/// partial scan (`docs/design/decisions.md`, "D30") is not exempt from the invariant.
#[tokio::test]
async fn an_unscanned_tail_tiles_cleanly() {
    use pgdump_query::{Span, SpanBody as Body};
    let spans = vec![
        Span {
            start: 0,
            end: 10,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: Body::Framing,
        },
        Span {
            start: 10,
            end: 20,
            database: None,
            text: None,
            toc: None,
            toc_owned: false,
            body: Body::Unscanned,
        },
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
/// (`docs/design/decisions.md`, "D30"). If text were accumulated from events,
/// every function body in the file would be missing from it.
#[tokio::test]
async fn span_text_includes_dollar_quoted_bodies_events_never_surface() {
    let path = objects_fixture(16, "default");
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
        toc: None,
        toc_owned: false,
        body: SpanBody::Framing,
    }];
    attach_text(&source, &mut spans).await.unwrap();

    let stored = spans[0].text.as_ref().unwrap();
    assert_eq!(stored.text.len(), TEXT_CAP);
    assert!(stored.truncated);
    assert_eq!(spans[0].end, body.len() as u64, "offsets are untouched by the cap");
}

/// What `Event::DollarQuoteEnd` is for: with no TOC comments, `scan.rs` emits
/// no `Event::Line` for the line carrying a `CREATE FUNCTION`'s own closing
/// `;`, so nothing else tells the accumulator the statement has ended
/// (`docs/design/decisions.md`, "D32") — without it, the two functions and
/// the table after them would collapse into **one** `Unparsed` span instead
/// of degrading to one span per object.
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

/// `objects.widgets`' own TOC comment (`fixtures/18/objects/default.sql`,
/// `-- Name: widgets; Type: TABLE; Schema: objects; Owner: postgres`) fills
/// its span's `toc` — the TOC's own (unqualified) name alongside
/// `SpanBody::Table`'s schema-qualified one
/// (`docs/design/decisions.md`, "D31").
#[tokio::test]
async fn a_real_toc_header_fills_owner_kind_and_schema() {
    let path = objects_fixture(18, "default");
    let (spans, _) = map_of(&path).await;
    let widgets = spans
        .iter()
        .find(|s| matches!(&s.body, SpanBody::Table { name, .. } if name == "objects.widgets"))
        .expect("objects.widgets must be a Table span");
    let toc = widgets.toc.as_ref().expect("a real pg_dump TOC comment must have parsed");
    assert_eq!(toc.name, "widgets", "the TOC tag is unqualified, unlike SpanBody::Table's name");
    assert_eq!(toc.kind, "TABLE");
    assert_eq!(toc.schema.as_deref(), Some("objects"));
    assert_eq!(toc.owner.as_deref(), Some("postgres"));
    assert_eq!(toc.tablespace, None);
}

/// `--no-owner` writes `Owner: -` on every entry (I16) — every parsed `toc`
/// in the file has `owner: None`, and none is lost entirely (the header
/// still parses; only the field is absent).
#[tokio::test]
async fn no_owner_fixture_parses_every_toc_header_with_no_owner() {
    let path = edge_cases_fixture(18, "no-owner");
    let (spans, _) = map_of(&path).await;
    let with_toc: Vec<_> = spans.iter().filter_map(|s| s.toc.as_ref()).collect();
    assert!(!with_toc.is_empty(), "no-owner.sql still carries TOC comments, just no owners");
    assert!(with_toc.iter().all(|t| t.owner.is_none()), "{with_toc:?}");
}

/// `build_index` reports the TOC-coverage figure as a file-level `Info`
/// diagnostic (`docs/design/decisions.md`, "D31"): every span
/// accounted for in `spans`, and a strictly higher `attributed` count than
/// the number
/// of spans that carry their *own* header, because every follow-on statement
/// (`ALTER ... OWNER TO`, etc.) now inherits its governing entry's `toc`
/// rather than counting as uncovered.
#[tokio::test]
async fn build_index_reports_toc_coverage_for_a_real_dump() {
    let path = objects_fixture(18, "default");
    let source = LocalFileSource::open(&path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let coverage = index
        .diagnostics
        .iter()
        .find_map(|d| match &d.kind {
            DiagnosticKind::TocCoverage { attributed, spans } => Some((*attributed, *spans)),
            _ => None,
        })
        .expect("build_index must report a TocCoverage diagnostic");
    assert_eq!(coverage.1, index.spans.len());
    assert!(coverage.0 > 0, "a real pg_dump file carries TOC headers");
    assert!(coverage.0 <= coverage.1);

    let own_headers = index.spans.iter().filter(|s| s.toc_owned).count();
    assert!(
        coverage.0 > own_headers,
        "inheritance must attribute more spans ({}) than carry their own header ({own_headers})",
        coverage.0
    );
}

/// A header-less file — no `-- Name: ...` comments anywhere — reports zero
/// headers, the "running in degraded mode" signal the design calls a normal,
/// reported state rather than an error.
#[tokio::test]
async fn build_index_reports_zero_toc_coverage_for_a_header_less_dump() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    let coverage = index
        .diagnostics
        .iter()
        .find_map(|d| match &d.kind {
            DiagnosticKind::TocCoverage { attributed, spans } => Some((*attributed, *spans)),
            _ => None,
        })
        .expect("build_index must report a TocCoverage diagnostic");
    assert_eq!(coverage.0, 0, "tests/data/edge_cases.sql has no TOC comments at all");
}

/// The cross-reference set (`docs/design/decisions.md`,
/// "D31") over a real fixture: `postgres` (every object's
/// owner, via both `Span::toc.owner` and the file's many `ALTER ... OWNER
/// TO`) and `fixture_reader` (the `GRANT`/`ALTER DEFAULT PRIVILEGES`
/// grantee) are both present; `PUBLIC` — also a real grantee in this fixture
/// (`GRANT SELECT ON TABLE objects.widgets TO PUBLIC;`) — is not.
/// `objects.tablespaced_table` is the fixture's one
/// non-default tablespace, referenced both by its TOC header's `;
/// Tablespace: fixture_ts` suffix and by the `SET default_tablespace =
/// fixture_ts;` framing ahead of its definition; the surrounding `SET
/// default_tablespace = '';` reset lines are never a reference either way.
#[tokio::test]
async fn build_index_records_referenced_roles_and_tablespaces() {
    let path = objects_fixture(16, "default");
    let source = LocalFileSource::open(&path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    assert!(index.roles.contains("postgres"));
    assert!(index.roles.contains("fixture_reader"));
    assert!(!index.roles.iter().any(|r| r.eq_ignore_ascii_case("public")));
    assert_eq!(index.tablespaces, ["fixture_ts".to_string()].into_iter().collect());
}
