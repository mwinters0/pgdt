use super::*;

/// The coverage line states a ratio only against an exact total: a first
/// pass that has not reached the end of its source says how far it read and
/// that the total is not known, rather than a percentage of a bound.
#[test]
fn the_completion_line_states_no_ratio_without_a_total() {
    assert_eq!(completion_line(25, Extent::Exact(100)), "Scan completion: 25% (25 bytes)");
    assert_eq!(completion_line(0, Extent::Exact(0)), "Scan completion: 100% (0 bytes)");
    assert_eq!(
        completion_line(25, Extent::Open { at_least: 40 }),
        "Scan completion: 25 bytes scanned; the total is not known until a scan reaches the end"
    );
}
