use super::*;

fn refusal(line: u64, column: usize) -> FieldRefusal {
    FieldRefusal {
        offset: line * 10,
        line,
        column,
        declared_type: "smallint".to_string(),
        value: Some(format!("7000{line}")),
    }
}

fn counts(record: &Option<IgnoredRefusals>) -> Vec<(usize, u64, u64)> {
    let columns = record.iter().flat_map(|r| &r.columns);
    columns.map(|c| (c.first.column, c.first.line, c.count)).collect()
}

/// **A record is per column**: each column keeps its first and its count,
/// in column order whatever order they were met in; a later piece's
/// count adds to its column's and its first stands only where the column
/// had none; and a merge of two reads from the block's first row keeps
/// each column's first and the larger count, a column only one read
/// gathered keeping that read's.
#[test]
fn a_record_keeps_each_columns_first_and_count() {
    let mut held = None;
    for (line, column) in [(3, 2), (5, 0), (6, 2), (9, 0), (9, 2)] {
        IgnoredRefusals::add(&mut held, refusal(line, column));
    }
    assert_eq!(counts(&held), [(0, 5, 2), (2, 3, 3)]);

    let mut later = None;
    IgnoredRefusals::add(&mut later, refusal(12, 1));
    IgnoredRefusals::add(&mut later, refusal(14, 2));
    IgnoredRefusals::fold(&mut held, later);
    assert_eq!(counts(&held), [(0, 5, 2), (1, 12, 1), (2, 3, 4)]);

    let mut declined = None;
    IgnoredRefusals::add(&mut declined, refusal(3, 2));
    IgnoredRefusals::add(&mut declined, refusal(4, 3));
    let mut merged = held.clone();
    IgnoredRefusals::merge(&mut merged, declined.clone());
    assert_eq!(counts(&merged), [(0, 5, 2), (1, 12, 1), (2, 3, 4), (3, 4, 1)]);
    IgnoredRefusals::merge(&mut declined, held);
    assert_eq!(counts(&declined), counts(&merged), "these two merge symmetrically");

    let record = merged.expect("held some");
    let first = |tracked: &[bool]| record.first_among(tracked).map(|f| (f.line, f.column));
    assert_eq!(first(&[true, true, true, true]), Some((3, 2)));
    assert_eq!(first(&[true, true, false, false]), Some((5, 0)));
    assert_eq!(first(&[false, true]), Some((12, 1)));
    assert_eq!(first(&[false, false, false]), None);
}
