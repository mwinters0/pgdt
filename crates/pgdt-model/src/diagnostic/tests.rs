use super::*;

fn tiling_message(issues: Vec<TilingIssue>) -> String {
    Finding::message(&<Diagnostic as DiagnosticExt>::tiling_broken(issues))
}

/// **A tiling failure is tallied by kind**: a discontinuity is a gap or an
/// overlap by whether the next span starts past this one's end, every kind
/// is named in `TilingIssue`'s order, and a kind with none is left out.
#[test]
fn a_tiling_failure_names_each_kind_it_counts() {
    let gap =
        |after_index| TilingIssue::Discontinuity { after_index, span_end: 10, next_start: 12 };
    let overlap = TilingIssue::Discontinuity { after_index: 3, span_end: 10, next_start: 8 };
    assert_eq!(
        tiling_message(vec![gap(0), overlap, gap(5)]),
        "the file map does not tile the file (2 gap(s), 1 overlap(s)) — this is a pgdt bug, \
         please report it"
    );
    assert_eq!(
        tiling_message(vec![
            TilingIssue::Empty { index: 1 },
            TilingIssue::OutOfOrder { index: 2 },
            TilingIssue::DoesNotStartAtZero { first_start: 4 },
            TilingIssue::DoesNotReachEnd { last_end: 10, expected_end: 20 },
        ]),
        "the file map does not tile the file (1 span(s) out of order, not starting at byte 0, \
         not reaching the end of the file, 1 empty span(s)) — this is a pgdt bug, please report it"
    );
    assert_eq!(
        tiling_message(vec![TilingIssue::DoesNotReachEnd { last_end: 0, expected_end: 20 }]),
        "the file map does not tile the file (not reaching the end of the file) — this is a pgdt \
         bug, please report it"
    );
}

/// **A TimescaleDB finding names its database only where the dump does**,
/// the first absent chunk and how many more, and which tables an unreadable
/// catalog leaves unread with their descendants.
#[test]
fn a_timescaledb_finding_names_what_it_is_about() {
    let names = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let absent = <Diagnostic as DiagnosticExt>::timescale_chunks_absent(
        Some("tsdb".to_owned()),
        "public.metrics".to_owned(),
        names(&[
            "_timescaledb_internal._hyper_1_1_chunk",
            "_timescaledb_internal._hyper_1_2_chunk",
        ]),
    );
    assert_eq!(
        absent.message(),
        "the TimescaleDB catalog of hypertable `public.metrics` in database `tsdb` lists chunk \
         `_timescaledb_internal._hyper_1_1_chunk` and 1 more that the file does not hold; read \
         as absent, as a restore of this file lacks them"
    );
    let root = <Diagnostic as DiagnosticExt>::timescale_root_rows(None, "public.m".to_owned(), 2);
    assert!(
        root.message().starts_with("TimescaleDB hypertable `public.m` holds 2 row(s) in its root")
    );
    let unreadable = |hypertables| {
        <Diagnostic as DiagnosticExt>::timescale_catalog_unreadable(
            None,
            names(&["dimension_slice.chunk_id"]),
            Vec::new(),
            hypertables,
        )
        .message()
    };
    assert_eq!(
        unreadable(Some(names(&["public.m"]))),
        "the TimescaleDB catalog lacks `dimension_slice.chunk_id`: each hypertable its rows name \
         and the file holds a chunk of is not read with its descendants, and each chunk reads \
         under its own name, unchecked for compression"
    );
    assert!(unreadable(None).contains(": every table with a descendant is not read"));
}
