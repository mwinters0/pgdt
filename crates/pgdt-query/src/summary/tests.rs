use super::*;

fn accumulator<'a>() -> Accumulator<'a> {
    Accumulator {
        kind: Some(CompareKind::Text { length: None, varying: false }),
        nulls: 0,
        nulls_complete: true,
        min: None,
        max: None,
        complete: true,
        distinct: None,
        sum: None,
        value_bytes: None,
    }
}

fn bounds(min: &str, min_exact: bool, max: &str, max_exact: bool) -> Bounds {
    Bounds { min: min.into(), max: max.into(), min_exact, max_exact }
}

fn end(end: &Option<(OrderKey, String, bool)>) -> (&str, bool) {
    let (_, text, exact) = end.as_ref().expect("an extreme");
    (text, *exact)
}

/// Where a truncated bound and an exact one share their text, the exact one is
/// the column's extreme whichever group is folded first; any other tie keeps
/// the first.
#[test]
fn an_exact_bound_is_kept_over_a_truncated_one_it_ties() {
    let truncated = bounds("abc", false, "abd", false);
    let exact = bounds("abc", true, "abd", true);
    for order in [[&truncated, &exact], [&exact, &truncated]] {
        let mut held = accumulator();
        for bounds in order {
            held.fold(bounds);
        }
        assert_eq!(end(&held.min), ("abc", true));
        assert_eq!(end(&held.max), ("abd", true));
        assert!(held.complete);
    }
    // Further out still wins over exactness, and a tie between two truncated
    // bounds stays inexact.
    let mut held = accumulator();
    held.fold(&bounds("abb", true, "abc", true));
    held.fold(&truncated);
    held.fold(&truncated);
    assert_eq!(end(&held.min), ("abb", true));
    assert_eq!(end(&held.max), ("abd", false));
}
