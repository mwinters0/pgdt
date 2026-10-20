use super::*;
use pgdt_core::CopyHeader;
use pgdt_model::{
    DataBlock, Span, SpanBody, TocHeader, TypeKind, UnrepresentableTier, internal::FieldCount,
};

/// A block's count where no column is tested: what a `COPY` with no DDL
/// above it is counted against.
struct NoCount;

impl FieldCount for NoCount {
    fn counts_any(&self) -> bool {
        false
    }

    fn tier(&self, _column: usize, _field: &[u8]) -> Option<UnrepresentableTier> {
        None
    }
}

/// Feed `lines` (each with a synthetic offset — `\n`-joined, matching
/// how a real scan would number them) through a fresh [`Builder`] and
/// return the finished spans.
fn spans_of(lines: &[&str]) -> Vec<Span> {
    let mut builder = Builder::new();
    let mut offset = 0u64;
    for line in lines {
        builder.feed_line(offset, line.as_bytes());
        offset += line.len() as u64 + 1;
    }
    builder.finish(offset)
}

#[test]
fn looks_like_framing_statement_matches_set_and_set_config_only() {
    assert!(looks_like_framing_statement("SET statement_timeout = 0;"));
    assert!(looks_like_framing_statement(
        "SELECT pg_catalog.set_config('search_path', '', false);"
    ));
    assert!(looks_like_framing_statement("set client_encoding = 'UTF8';"));
    assert!(!looks_like_framing_statement("CREATE TABLE t (id integer);"));
    assert!(!looks_like_framing_statement("SELECT 1;"));
}

#[test]
fn a_toc_commented_table_is_one_span_from_the_comment_through_the_statement() {
    let spans = spans_of(&[
        "--",
        "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
        "--",
        "",
        "CREATE TABLE public.t (",
        "    id integer",
        ");",
    ]);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].start, 0);
    assert!(matches!(&spans[0].body, SpanBody::Table { name, .. } if name == "public.t"));
    assert_eq!(
        spans[0].toc,
        Some(TocHeader {
            name: "t".to_string(),
            kind: "TABLE".to_string(),
            schema: Some("public".to_string()),
            owner: Some("postgres".to_string()),
            tablespace: None,
        })
    );
}

/// The one input on which `looks_like_toc_name_line`'s refusal of
/// `"Data for "` changes an outcome, and therefore the only thing that
/// can pin it: I31's `--disable-triggers` shape, where `SET SESSION
/// AUTHORIZATION DEFAULT;` stands between the `-- Data for Name:` block
/// and the `COPY` header it heads. Accepting the prefix would run the
/// entry into that statement's span, where `push_statement_span`'s
/// `Framing` veto discards the header outright; refusing keeps it on a
/// `Framing` span of its own. The data span is unattributed either way,
/// which is not what this test is about.
///
/// Deficiency register: `deficiency: KD1` — under I31's shape every data
/// span in the file loses its TOC attribution, `COPY` and `INSERT` alike,
/// so `Span::toc` is `None` where a plain dump names the table and the
/// coverage diagnostic under-reports by that much. **(c) unowned**, the
/// shape being opt-in. Closing it takes three changes that land together —
/// accept the prefix, veto on framing differently, re-tile the seam — and
/// this test is the only thing that can observe the outcome.
#[test]
fn a_data_entry_keeps_its_own_span_when_disable_triggers_intervenes() {
    let mut builder = Builder::new();
    let mut offset = 0u64;
    for line in [
        "--",
        "-- Data for Name: t; Type: TABLE DATA; Schema: public; Owner: postgres",
        "--",
        "",
        "SET SESSION AUTHORIZATION DEFAULT;",
        "",
        "ALTER TABLE public.t DISABLE TRIGGER ALL;",
        "",
    ] {
        builder.feed_line(offset, line.as_bytes());
        offset += line.len() as u64 + 1;
    }
    let header_offset = offset;
    builder.on_copy_start(
        CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        },
        true,
    );
    builder.count_block(Arc::new(NoCount));
    let terminator_offset = header_offset + 48;
    let end_offset = terminator_offset + 3;
    builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 }).unwrap();
    let spans = builder.finish(end_offset);

    assert_eq!(spans[0].start, 0);
    assert!(matches!(spans[0].body, SpanBody::Framing));
    assert!(spans[0].toc_owned, "the entry survives on the comment block's own span");
    assert_eq!(spans[0].toc.as_ref().map(|t| t.name.as_str()), Some("t"));

    let data = spans.last().expect("the COPY block is the last span");
    assert!(matches!(data.body, SpanBody::Data(_)));
    assert_eq!(data.toc, None, "I31: the intervening statements cost the data its entry");
}

/// The whole statistics entry — comment block and the
/// `pg_restore_relation_stats()` call it heads — is **one** span owning
/// its TOC entry.
#[test]
fn a_statistics_entry_is_one_attributed_span() {
    let spans = spans_of(&[
        "--",
        "-- Statistics for Name: widgets; Type: STATISTICS DATA; Schema: objects; Owner: -",
        "--",
        "",
        "SELECT * FROM pg_catalog.pg_restore_relation_stats(",
        "    'version', '180000'::integer,",
        "    'relation', 'objects.widgets'::regclass,",
        "    'relpages', '1'::integer",
        ");",
    ]);
    assert_eq!(spans.len(), 1);
    assert!(spans[0].toc_owned);
    assert_eq!(spans[0].toc.as_ref().map(|t| t.kind.as_str()), Some("STATISTICS DATA"));
}

/// A `-- Data for Name: ...` entry heading an `INSERT` run is **one**
/// span owning its TOC entry, the same shape `on_copy_start` produces for
/// a `COPY` block: the absorption happens in `Mode::Comment`'s close arm,
/// so the run starts at the comment's offset rather than at its first
/// `INSERT` line.
#[test]
fn an_insert_run_absorbs_its_data_entry_and_owns_it() {
    let spans = spans_of(&[
        "--",
        "-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres",
        "--",
        "",
        "INSERT INTO public.widgets VALUES (1, 'alpha');",
        "INSERT INTO public.widgets VALUES (2, 'beta');",
    ]);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].start, 0, "the span opens at the comment, not at the first INSERT");
    assert!(spans[0].toc_owned);
    assert_eq!(spans[0].toc.as_ref().map(|t| t.kind.as_str()), Some("TABLE DATA"));
    match &spans[0].body {
        SpanBody::Data(DataBlock::InsertRun(run)) => {
            assert_eq!(run.table, "public.widgets");
            assert_eq!(run.row_count, 2);
        }
        other => panic!("expected an INSERT run, got {other:?}"),
    }
}

/// Every line-shape [`Builder::insert_run_line`] declines, in one run,
/// counted correctly: the two paths share a [`StatementScan`] and each
/// line reaches exactly one of them, so a run whose lines alternate
/// between them folds the same way a run of plain statements does.
///
/// The shapes, in order: a value carrying a raw newline; a continuation
/// that *looks* like a comment and is really string content (the
/// `in_quote` guard); a blank line; and a statement spelled with an extra
/// space, which the byte-prefix check refuses and
/// `parse_insert_target` then accepts.
#[test]
fn an_insert_run_folds_the_same_way_when_its_lines_take_the_slow_path() {
    let spans = spans_of(&[
        "--",
        "-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres",
        "--",
        "",
        "INSERT INTO public.widgets VALUES (1, 'alpha');",
        "INSERT INTO public.widgets VALUES (2, 'two",
        "-- lines, and this is not a comment');",
        "",
        "INSERT INTO  public.widgets VALUES (3, 'gamma');",
        "INSERT INTO public.widgets VALUES (4, 'it''s ok');",
    ]);
    assert_eq!(spans.len(), 1);
    match &spans[0].body {
        SpanBody::Data(DataBlock::InsertRun(run)) => {
            assert_eq!(run.table, "public.widgets");
            assert_eq!(run.row_count, 4);
        }
        other => panic!("expected an INSERT run, got {other:?}"),
    }
}

/// The byte-prefix check must not let `public.widgets2` continue
/// `public.widgets`'s run: it is a prefix of the longer name, so the
/// byte after it has to end the identifier.
#[test]
fn a_table_whose_name_extends_the_runs_own_starts_a_new_run() {
    let spans = spans_of(&[
        "INSERT INTO public.widgets VALUES (1);",
        "INSERT INTO public.widgets2 VALUES (2);",
    ]);
    let runs: Vec<_> = spans
        .iter()
        .filter_map(|s| match &s.body {
            SpanBody::Data(DataBlock::InsertRun(run)) => Some((run.table.as_str(), run.row_count)),
            _ => None,
        })
        .collect();
    assert_eq!(runs, vec![("public.widgets", 1), ("public.widgets2", 1)]);
}

/// `_printTocEntry()`'s last field runs to the line's end (I18), so the TOC
/// header is read from the line as `pg_dump` wrote it: `COMMENT ON
/// EXTENSION`'s empty `Owner: ` is an attributed entry with no owner, and an
/// owner or tablespace name ending in a space reaches the cross-reference
/// sets whole.
#[test]
fn a_toc_header_is_read_to_the_end_of_its_line() {
    let comment = spans_of(&[
        "--",
        "-- Name: EXTENSION file_fdw; Type: COMMENT; Schema: -; Owner: ",
        "--",
        "",
        "COMMENT ON EXTENSION file_fdw IS 'foreign-data wrapper for flat file access';",
    ]);
    assert_eq!(comment.len(), 1);
    assert!(comment[0].toc_owned);
    assert_eq!(
        comment[0].toc,
        Some(TocHeader {
            name: "EXTENSION file_fdw".to_string(),
            kind: "COMMENT".to_string(),
            schema: None,
            owner: None,
            tablespace: None,
        })
    );

    let mut builder = Builder::new();
    let mut offset = 0u64;
    for line in [
        "--",
        "-- Name: t; Type: TABLE; Schema: public; Owner: bob ",
        "--",
        "",
        "CREATE TABLE public.t (id integer);",
        "",
        "--",
        "-- Name: u; Type: TABLE; Schema: public; Owner: carol; Tablespace: fast ",
        "--",
        "",
        "CREATE TABLE public.u (id integer);",
    ] {
        builder.feed_line(offset, line.as_bytes());
        offset += line.len() as u64 + 1;
    }
    assert!(builder.roles().contains("bob "), "{:?}", builder.roles());
    assert!(builder.tablespaces().contains("fast "), "{:?}", builder.tablespaces());
    let spans = builder.finish(offset);
    assert_eq!(spans[0].toc.as_ref().and_then(|t| t.owner.as_deref()), Some("bob "));
    assert_eq!(spans[1].toc.as_ref().and_then(|t| t.tablespace.as_deref()), Some("fast "));
}

/// `--verbose` inserts `-- TOC entry ...`/`-- Dependencies: ...` lines
/// ahead of the Name line (`fixtures/*/objects/verbose.sql`) — they don't
/// parse as headers themselves, and don't stop the Name line after them
/// from being recognized.
#[test]
fn verbose_lines_ahead_of_the_name_line_do_not_prevent_it_parsing() {
    let spans = spans_of(&[
        "--",
        "-- TOC entry 7 (class 2615 OID 16386)",
        "-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres",
        "--",
        "",
        "CREATE SCHEMA objects;",
    ]);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].toc.as_ref().map(|t| t.name.as_str()), Some("objects"));
}

/// A `Data` span's `toc` comes from its own `-- Data for Name: ...`
/// comment, threaded through `pending_data` from `on_copy_start` to
/// `on_copy_end`.
#[test]
fn a_data_span_carries_the_toc_header_its_data_for_name_comment_parsed_to() {
    let mut builder = Builder::new();
    let mut offset = 0u64;
    for line in
        ["--", "-- Data for Name: t; Type: TABLE DATA; Schema: public; Owner: postgres", "--", ""]
    {
        builder.feed_line(offset, line.as_bytes());
        offset += line.len() as u64 + 1;
    }
    let header_offset = offset;
    builder.on_copy_start(
        CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        },
        true,
    );
    builder.count_block(Arc::new(NoCount));
    let terminator_offset = header_offset + 48;
    let end_offset = terminator_offset + 3;
    builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 }).unwrap();

    let spans = builder.finish(end_offset);
    assert_eq!(spans.len(), 1);
    assert!(matches!(spans[0].body, SpanBody::Data(_)));
    assert_eq!(spans[0].toc.as_ref().map(|t| t.kind.as_str()), Some("TABLE DATA"));
}

/// A statement with no preceding TOC comment at all — the header-less
/// fallback — carries no `toc` and `toc_owned: false`, there being no
/// governing entry for it to inherit.
#[test]
fn a_statement_with_no_toc_comment_carries_no_toc_header() {
    let spans = spans_of(&["CREATE EXTENSION pgcrypto;"]);
    assert_eq!(spans[0].toc, None);
    assert!(!spans[0].toc_owned);
}

/// TOC inheritance: a follow-on statement with no TOC comment of its own
/// (`ALTER SCHEMA ... OWNER TO ...;`, mirroring
/// `fixtures/*/objects/default.sql`) inherits the governing entry's header
/// instead of carrying `None` (`docs/design/decisions.md`, "D31"). Both
/// spans carry the *same* `toc`; only the first has `toc_owned: true`.
#[test]
fn a_follow_on_statement_inherits_the_governing_toc_header() {
    let spans = spans_of(&[
        "--",
        "-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres",
        "--",
        "",
        "CREATE SCHEMA objects;",
        "",
        "ALTER SCHEMA objects OWNER TO postgres;",
    ]);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[1].body, SpanBody::Unparsed);
    assert_eq!(spans[0].toc, spans[1].toc, "both spans belong to the same TOC entry");
    assert!(spans[0].toc.is_some());
    assert!(spans[0].toc_owned, "the first span's own comment carried the header");
    assert!(!spans[1].toc_owned, "the second span inherited it");
}

/// Inheritance keeps propagating across more than one follow-on in a
/// row — every span until the next boundary carries the *same* governing
/// header, mirroring `fixtures/16/objects/default.sql`'s
/// `objects.simple_config`.
#[test]
fn several_consecutive_follow_ons_all_inherit_the_same_header() {
    let spans = spans_of(&[
        "--",
        "-- Name: t; Type: TEXT SEARCH CONFIGURATION; Schema: public; Owner: postgres",
        "--",
        "",
        "CREATE TEXT SEARCH CONFIGURATION public.t (COPY = simple);",
        "",
        "ALTER TEXT SEARCH CONFIGURATION public.t ALTER MAPPING FOR asciiword WITH simple;",
        "",
        "ALTER TEXT SEARCH CONFIGURATION public.t ALTER MAPPING FOR word WITH simple;",
    ]);
    assert_eq!(spans.len(), 3);
    assert!(spans[0].toc_owned);
    assert!(!spans[1].toc_owned);
    assert!(!spans[2].toc_owned);
    assert!(spans[0].toc.is_some());
    assert_eq!(spans[0].toc, spans[1].toc);
    assert_eq!(spans[1].toc, spans[2].toc);
}

/// A mid-file `SET default_tablespace = ...;` classifies as `Framing`
/// (`looks_like_framing_statement`), never inherits even when it directly
/// follows a governed entry, and clears the governing header for whatever
/// comes after it (`docs/design/decisions.md`, "D31").
#[test]
fn a_mid_file_framing_statement_never_inherits_and_clears_the_governing_header() {
    let spans = spans_of(&[
        "--",
        "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
        "--",
        "",
        "CREATE TABLE public.t (id integer);",
        "",
        "SET default_tablespace = '';",
        "",
        "CREATE EXTENSION pgcrypto;",
    ]);
    assert_eq!(spans.len(), 3);
    assert!(spans[0].toc.is_some());
    assert_eq!(spans[1].body, SpanBody::Framing);
    assert_eq!(spans[1].toc, None, "a Framing span never carries an inherited header");
    assert!(!spans[1].toc_owned);
    assert_eq!(
        spans[2].toc, None,
        "the Framing statement must clear the governing header for what follows it"
    );
}

/// `\connect` clears the governing header the same way `Framing` does —
/// a statement in the newly-connected database has no TOC entry of its
/// own to inherit from the previous database's last one.
#[test]
fn connect_clears_the_governing_toc_header() {
    let spans = spans_of(&[
        "--",
        "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
        "--",
        "",
        "CREATE TABLE public.t (id integer);",
        "\\connect two",
        "CREATE EXTENSION pgcrypto;",
    ]);
    let last = spans.last().unwrap();
    assert!(matches!(&last.body, SpanBody::Extension { name, .. } if name == "pgcrypto"));
    assert_eq!(last.toc, None);
}

/// A `COPY` block with no TOC comment of its own resets inheritance the
/// same way `Framing`/`Connect` do: the entry governing *before* the block
/// does not leak past it, the `Data` span itself carrying `toc: None`.
#[test]
fn a_copy_block_with_no_comment_of_its_own_resets_inheritance() {
    let mut builder = Builder::new();
    let mut offset = 0u64;
    for line in [
        "--",
        "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
        "--",
        "",
        "CREATE TABLE public.t (id integer);",
        "",
    ] {
        builder.feed_line(offset, line.as_bytes());
        offset += line.len() as u64 + 1;
    }
    let header_offset = offset;
    builder.on_copy_start(
        CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        },
        true,
    );
    builder.count_block(Arc::new(NoCount));
    let terminator_offset = header_offset + 48;
    let end_offset = terminator_offset + 3;
    builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 }).unwrap();
    offset = end_offset;
    builder.feed_line(offset, b"CREATE EXTENSION pgcrypto;");

    let spans = builder.finish(offset + 27);
    assert_eq!(spans.len(), 3, "table, COPY block, extension");
    assert!(matches!(spans[1].body, SpanBody::Data(_)));
    assert_eq!(spans[1].toc, None, "no comment preceded this COPY header");
    assert_eq!(
        spans[2].toc, None,
        "the table's header must not leak across the uncommented COPY block"
    );
}

#[test]
fn a_non_toc_comment_and_a_bare_statement_are_two_adjacent_spans() {
    let spans = spans_of(&["-- just a note, not a TOC entry", "", "CREATE EXTENSION pgcrypto;"]);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].body, SpanBody::Framing);
    assert!(matches!(&spans[1].body, SpanBody::Extension { name, .. } if name == "pgcrypto"));
    assert_eq!(spans[0].end, spans[1].start, "no gap between adjacent spans");
}

#[test]
fn a_toc_comment_block_with_no_trailing_statement_closes_at_scan_end() {
    let spans = spans_of(&["--", "-- Name: x; Type: SCHEMA; Schema: -; Owner: postgres", "--"]);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].body, SpanBody::Unparsed);
}

#[test]
fn connect_opens_its_own_connect_span_and_switches_the_database() {
    let spans = spans_of(&["\\connect one", "CREATE EXTENSION pgcrypto;"]);
    assert_eq!(spans[0].body, SpanBody::Connect { database: "one".to_string() });
    assert_eq!(
        spans[0].database.as_deref(),
        Some("one"),
        "the \\connect line's own span is attributed to the database it establishes"
    );
    assert_eq!(spans[1].database.as_deref(), Some("one"));
}

/// The case the module docs' "Span boundaries" section exists for: a
/// dollar-quoted function body swallows its own closing `;` entirely
/// (never reaches `Event::Line`), so with no `DollarQuoteEnd` fed, only
/// the next TOC comment's boundary can close the span. `feed_line` here is given only the lines a real
/// scan would still emit.
#[test]
fn a_dollar_quoted_function_with_no_trailing_statement_closes_at_the_next_toc_comment() {
    let spans = spans_of(&[
        "--",
        "-- Name: f; Type: FUNCTION; Schema: public; Owner: postgres",
        "--",
        "",
        "CREATE FUNCTION public.f() RETURNS void",
        "    LANGUAGE plpgsql",
        // The `AS $$ ... $$;` body is entirely invisible to `Event::Line`
        // (`crate::scan`'s dollar-quote handling) — omitted here, exactly
        // matching what a real scan would (not) emit.
        "",
        "--",
        "-- Name: g; Type: SCHEMA; Schema: -; Owner: postgres",
        "--",
        "",
        "CREATE SCHEMA g;",
    ]);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].body, SpanBody::Unparsed, "CREATE FUNCTION isn't a classified shape");
    assert_eq!(spans[0].end, spans[1].start);
    assert_eq!(spans[1].body, SpanBody::Unparsed, "CREATE SCHEMA isn't a classified shape");
}

#[test]
fn a_backslash_meta_command_is_its_own_single_line_framing_span() {
    let spans = spans_of(&["\\restrict aToken", "CREATE EXTENSION pgcrypto;"]);
    assert_eq!(spans[0].body, SpanBody::Framing);
    assert_eq!(spans[0].end, spans[1].start);
}

/// I9's two-line version-header block gets its own span kind rather than
/// generic `Framing`, so `crate::ddl::dump_metadata_from_spans` can
/// recover the strings without re-reading the file.
#[test]
fn version_header_lines_become_their_own_span() {
    let spans = spans_of(&[
        "-- Dumped from database version 16.15",
        "-- Dumped by pg_dump version 16.15",
        "",
        "CREATE EXTENSION pgcrypto;",
    ]);
    assert_eq!(spans.len(), 2);
    assert_eq!(
        spans[0].body,
        SpanBody::VersionHeader {
            server_version: Some("16.15".to_string()),
            pg_dump_version: Some("16.15".to_string()),
        }
    );
}

/// A `--binary-upgrade` dump's `ALTER TYPE ... ADD VALUE ...;` (I6) gets
/// its own span kind rather than falling into the generic `Unparsed`
/// bucket, so `crate::ddl::dump_metadata_from_spans` can fold the
/// label into the `TypeDef` span it targets.
#[test]
fn binary_upgrade_add_value_becomes_its_own_span() {
    let spans = spans_of(&["ALTER TYPE public.mood ADD VALUE 'sad';"]);
    assert_eq!(
        spans[0].body,
        SpanBody::AlterTypeAddValue {
            type_name: "public.mood".to_string(),
            label: "sad".to_string(),
        }
    );
}

/// So does its `ALTER TYPE ... DROP ATTRIBUTE ...;`, the fold removing the
/// composite's placeholder field.
#[test]
fn binary_upgrade_drop_attribute_becomes_its_own_span() {
    let spans =
        spans_of(&["ALTER TYPE public.trio DROP ATTRIBUTE \"........pg.dropped.2........\";"]);
    assert_eq!(
        spans[0].body,
        SpanBody::AlterTypeDropAttribute {
            type_name: "public.trio".to_string(),
            attribute: "........pg.dropped.2........".to_string(),
        }
    );
}

/// An `ALTER TYPE` that could change an enum's labels and is not read is
/// a span of its own naming the type, and leaves that enum's labels
/// inexact — while one changing nothing a label is, and an `ADD VALUE`
/// that is read, leave them exact. One naming no type declared here
/// leaves every enum declared so far inexact, and a definition after it
/// is exact again.
#[test]
fn an_alter_type_changing_labels_unread_leaves_them_inexact() {
    let exactness = |lines: &[&str]| -> Vec<(String, bool)> {
        let metadata = crate::ddl::dump_metadata_from_spans(&spans_of(lines));
        metadata.databases[0]
            .types
            .iter()
            .map(|t| match &t.kind {
                TypeKind::Enum { exact, .. } => (t.name.clone(), *exact),
                other => panic!("{other:?}"),
            })
            .collect()
    };
    let declared = [
        "CREATE TYPE public.mood AS ENUM ('sad', 'ok');",
        "CREATE TYPE public.size AS ENUM ('s');",
    ];
    let with = |extra: &[&'static str]| {
        let mut lines = declared.to_vec();
        lines.extend_from_slice(extra);
        exactness(&lines)
    };
    let both = |mood: bool, size: bool| {
        vec![("public.mood".to_string(), mood), ("public.size".to_string(), size)]
    };
    assert_eq!(with(&[]), both(true, true));
    for exact in [
        "ALTER TYPE public.mood OWNER TO postgres;",
        "ALTER TYPE public.mood ADD VALUE 'happy' AFTER 'ok';",
        "ALTER TYPE public.mood  add   Value 'happy';",
        "ALTER TYPE public.mood RENAME TO feeling;",
    ] {
        assert_eq!(with(&[exact]), both(true, true), "{exact}");
    }
    for unread in [
        "ALTER TYPE public.mood RENAME VALUE 'ok' TO 'fine';",
        "ALTER TYPE public.mood ADD VALUE IF NOT EXISTS 'happy';",
        "ALTER TYPE public.mood ADD VALUE E'happy';",
        "ALTER TYPE public.mood /* why */ ADD VALUE 'happy';",
    ] {
        let spans = spans_of(&[unread]);
        assert_eq!(
            spans[0].body,
            SpanBody::EnumLabelsUnread { type_name: Some("public.mood".to_string()) },
            "{unread}"
        );
        assert_eq!(with(&[unread]), both(false, true), "{unread}");
    }
    for unmatched in
        ["ALTER TYPE mood RENAME VALUE 'ok' TO 'fine';", "ALTER TYPE mood ADD VALUE 'happy';"]
    {
        assert_eq!(with(&[unmatched]), both(false, false), "{unmatched}");
    }
    assert_eq!(
        exactness(&[declared[0], "ALTER TYPE mood ADD VALUE 'happy';", declared[1],]),
        both(false, true)
    );
    assert_eq!(
        exactness(&[
            declared[0],
            "ALTER TYPE public.mood RENAME VALUE 'ok' TO 'fine';",
            "CREATE TYPE public.mood AS ENUM ('ok');",
        ]),
        vec![("public.mood".to_string(), true)]
    );
    assert_eq!(
        exactness(&["CREATE TYPE public.mood AS ENUM ('sad', E'ok');"]),
        vec![("public.mood".to_string(), false)]
    );
}

/// The case [`Builder::snapshot`] exists for: `pgdt_mapping`'s forward pass
/// needs a tiling span list *before* the scan reaches its true end, right
/// after a `CopyEnd`, `mode` being `Idle` from the `CopyStart` before
/// it. `check_tiling` is checked at every such boundary
/// rather than only at the very end.
#[test]
fn snapshot_tiles_the_prefix_seen_so_far_at_every_copy_end() {
    fn feed(builder: &mut Builder, offset: &mut u64, line: &str) {
        builder.feed_line(*offset, line.as_bytes());
        *offset += line.len() as u64 + 1;
    }

    let mut builder = Builder::new();
    let mut offset = 0u64;

    feed(&mut builder, &mut offset, "CREATE EXTENSION pgcrypto;");
    assert!(check_tiling(&builder.snapshot(offset), offset).is_empty());

    feed(&mut builder, &mut offset, "CREATE SCHEMA g;");
    assert!(check_tiling(&builder.snapshot(offset), offset).is_empty());

    let header_offset = offset;
    builder.on_copy_start(
        CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        },
        true,
    );
    builder.count_block(Arc::new(NoCount));
    let terminator_offset = header_offset + 48;
    let end_offset = terminator_offset + 3;
    builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 }).unwrap();
    offset = end_offset;

    let snapshot = builder.snapshot(end_offset);
    let issues = check_tiling(&snapshot, end_offset);
    assert!(issues.is_empty(), "{issues:?}");
    assert!(matches!(snapshot.last().unwrap().body, SpanBody::Data(_)));
    assert_eq!(snapshot.last().unwrap().end, end_offset);

    feed(&mut builder, &mut offset, "CREATE SCHEMA h;");
    let finished = builder.finish(offset);
    let issues = check_tiling(&finished, offset);
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(finished.len(), 4, "extension, schema g, the COPY block, schema h");
}

/// `snapshot` and `finish`, called at the same watermark once nothing
/// more will ever be fed, must agree; `finish` just also takes
/// ownership.
#[test]
fn snapshot_agrees_with_finish_at_the_same_watermark() {
    let lines = ["CREATE EXTENSION pgcrypto;", "", "CREATE SCHEMA g;"];
    let drive = || {
        let mut builder = Builder::new();
        let mut offset = 0u64;
        for line in lines {
            builder.feed_line(offset, line.as_bytes());
            offset += line.len() as u64 + 1;
        }
        (builder, offset)
    };

    let (builder, offset) = drive();
    let via_snapshot = builder.snapshot(offset);
    let (builder, offset) = drive();
    let via_finish = builder.finish(offset);

    assert_eq!(via_snapshot, via_finish);
}
