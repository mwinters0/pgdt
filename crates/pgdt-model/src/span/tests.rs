use std::sync::Arc;

use super::BlockRecordExt;
use crate::{
    ArrayShape, BlockCensus, BlockRecord, BlockStatistics, ColumnRefusals, FieldRefusal,
    GroupSizing, IgnoredRefusals,
};

/// Statistics of `group_size`, holding `columns` columns none gathered.
fn statistics(group_size: u64, columns: usize) -> Arc<BlockStatistics> {
    Arc::new(BlockStatistics {
        group_size,
        sizing: GroupSizing::Stated,
        groups: Vec::new(),
        columns: vec![None; columns],
    })
}

/// One field refused in `column`, at `line`, met `count` times.
fn refused(column: usize, line: u64, count: u64) -> IgnoredRefusals {
    let first = FieldRefusal {
        offset: line * 10,
        line,
        column,
        declared_type: "integer".to_owned(),
        value: Some("x".to_owned()),
    };
    IgnoredRefusals { columns: vec![ColumnRefusals { first, count }] }
}

/// A census of one column, `shapes` deep.
fn census(shapes: usize) -> BlockCensus {
    BlockCensus { shapes: vec![ArrayShape::default(); shapes], unrepresentable: Vec::new() }
}

/// What the block holds before each read: gathered at 1024, censused,
/// declined once under 64 bytes, and one refusal recorded.
fn held() -> BlockRecord {
    BlockRecord {
        row_count: 3,
        statistics: Some(statistics(1024, 2)),
        statistics_declined: Some(64),
        ignored_refusals: Some(Box::new(refused(0, 2, 1))),
        census: Some(census(1)),
        ..BlockRecord::default()
    }
}

#[test]
fn a_gathering_read_replaces_the_statistics_and_clears_a_decline() {
    let mut record = held();
    let replaced = record.statistics.as_deref().map(BlockStatistics::heap_bytes);
    let read = BlockRecord {
        row_count: 3,
        statistics: Some(statistics(512, 2)),
        ignored_refusals: Some(Box::new(refused(0, 1, 3))),
        census: Some(census(2)),
        ..BlockRecord::default()
    };
    assert_eq!(record.merge(read, true), replaced);
    assert_eq!(record.statistics.as_ref().map(|s| s.group_size), Some(512));
    assert_eq!(record.statistics_declined, None);
    assert_eq!(record.census, Some(census(2)));
    // The block's first stands, and the larger count.
    let ignored = record.ignored_refusals.as_deref().expect("merged");
    assert_eq!((ignored.columns[0].first.line, ignored.columns[0].count), (2, 3));
}

#[test]
fn a_declining_read_keeps_the_statistics_and_records_the_allowance() {
    let mut record = held();
    let read = BlockRecord {
        row_count: 3,
        statistics_declined: Some(128),
        ignored_refusals: Some(Box::new(refused(1, 1, 1))),
        census: Some(census(2)),
        ..BlockRecord::default()
    };
    assert_eq!(record.merge(read, true), None);
    assert_eq!(record.statistics.as_ref().map(|s| s.group_size), Some(1024));
    assert_eq!(record.statistics_declined, Some(128));
    // Every row was read, so the census is the read's.
    assert_eq!(record.census, Some(census(2)));
    assert_eq!(record.ignored_refusals.as_deref().map(|i| i.columns.len()), Some(2));
}

#[test]
fn a_check_alone_records_only_that_it_checked() {
    let mut record = held();
    let read = BlockRecord {
        row_count: 3,
        checked_in_full: true,
        ignored_refusals: Some(Box::new(refused(1, 1, 1))),
        census: Some(census(2)),
        ..BlockRecord::default()
    };
    assert_eq!(record.merge(read, false), None);
    let mut expected = held();
    expected.checked_in_full = true;
    assert_eq!(record, expected);
}
