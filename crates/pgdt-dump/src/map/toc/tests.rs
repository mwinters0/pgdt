use super::*;

#[test]
fn looks_like_toc_name_line_matches_the_real_grammar() {
    assert!(looks_like_toc_name_line("-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres"));
    assert!(looks_like_toc_name_line(
        "-- Name: EXTENSION postgres_fdw; Type: COMMENT; Schema: -; Owner: "
    ));
    assert!(!looks_like_toc_name_line("-- PostgreSQL database dump"));
    assert!(!looks_like_toc_name_line("-- Dumped from database version 18.6"));
    assert!(!looks_like_toc_name_line("--"));
}

#[test]
fn parse_toc_header_line_handles_data_for_name_and_the_tablespace_suffix() {
    assert_eq!(
        parse_toc_header_line(
            "-- Data for Name: t; Type: TABLE DATA; Schema: public; Owner: postgres; \
                 Tablespace: fastspace"
        ),
        Some(TocHeader {
            name: "t".to_string(),
            kind: "TABLE DATA".to_string(),
            schema: Some("public".to_string()),
            owner: Some("postgres".to_string()),
            tablespace: Some("fastspace".to_string()),
        })
    );
}

/// `TOC_PREFIX_STATS`, the third prefix `_printTocEntry()` writes
/// (`fixtures/18/objects/stats.sql`). Parses like the other two, and
/// unlike `Data for ` it *is* a boundary signal.
#[test]
fn a_statistics_entry_parses_and_opens_a_span() {
    let line = "-- Statistics for Name: widgets; Type: STATISTICS DATA; Schema: objects; Owner: -";
    assert_eq!(
        parse_toc_header_line(line),
        Some(TocHeader {
            name: "widgets".to_string(),
            kind: "STATISTICS DATA".to_string(),
            schema: Some("objects".to_string()),
            owner: None,
            tablespace: None,
        })
    );
    assert!(looks_like_toc_name_line(line));
}

/// `fixtures/16/objects/verbose.sql`'s `Schema: -` and trailing empty
/// `Owner: ` both mean "none", per I16 and `_printTocEntry()`'s two ways
/// of writing it.
#[test]
fn a_hyphen_schema_and_an_empty_owner_both_parse_to_none() {
    let header =
        parse_toc_header_line("-- Name: EXTENSION postgres_fdw; Type: COMMENT; Schema: -; Owner: ")
            .unwrap();
    assert_eq!(header.schema, None);
    assert_eq!(header.owner, None);
}

/// `--no-owner` writes `Owner: -`, the other of `_printTocEntry()`'s two
/// "no owner" shapes (`fixtures/*/edge_cases/no-owner.sql`).
#[test]
fn a_hyphen_owner_parses_to_none() {
    let header =
        parse_toc_header_line("-- Name: sample_fn(); Type: FUNCTION; Schema: public; Owner: -")
            .unwrap();
    assert_eq!(header.owner, None);
}

#[test]
fn a_non_toc_comment_line_does_not_parse_as_a_toc_header() {
    assert_eq!(parse_toc_header_line("-- PostgreSQL database dump"), None);
    assert_eq!(parse_toc_header_line("-- TOC entry 7 (class 2615 OID 16386)"), None);
    assert_eq!(parse_toc_header_line("-- Dependencies: 2"), None);
}
