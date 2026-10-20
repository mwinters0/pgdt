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
