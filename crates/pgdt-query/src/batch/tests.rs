use arrow::datatypes::Field;

use std::sync::Arc;

use arrow::datatypes::DataType;

use super::*;
use crate::render::render_field;
use pgdt_codec::pgtype::NestedPlan;

/// A [`RowBatcher`] over one nullable `data_type` column, fed by the field
/// `field_targets` says — for driving `push_row` at chosen file offsets,
/// the flush triggers being arithmetic over offsets and row lengths.
fn one_column_batcher_fed_by(
    options: QueryOptions,
    field_targets: Vec<Option<usize>>,
    data_type: DataType,
) -> RowBatcher {
    use arrow::datatypes::Schema;

    use pgdt_codec::compare::comparison_for;
    use pgdt_codec::{ColumnNote, ColumnResolution};

    let declared = if data_type == DataType::Int32 { "integer" } else { "text" };
    let resolved = ResolvedSchema {
        schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
        columns: vec![ColumnResolution::Mapped],
        notes: vec![ColumnNote {
            column: "v".into(),
            declared: Some(declared.into()),
            resolution: ColumnResolution::Mapped,
        }],
        plans: vec![NestedPlan::Scalar],
        comparisons: vec![comparison_for(declared, None, &[], &[])],
        not_null: vec![false],
    };
    RowBatcher::new(&resolved, "public.t".into(), options, field_targets, Vec::new())
}

/// The unprojected case: one `Utf8View` column, fed by the row's only
/// field.
fn one_column_batcher(options: QueryOptions) -> RowBatcher {
    one_column_batcher_fed_by(options, vec![Some(0)], DataType::Utf8View)
}

/// A field no projection asked for is walked and skipped: never decoded
/// — the first field here would be a hard `Int32` decode failure if it
/// were — and not counted towards `max_bytes`.
#[test]
fn an_unprojected_field_is_walked_but_never_decoded() {
    let options = QueryOptions { max_bytes: Some(4), ..Default::default() };
    let mut batcher = one_column_batcher_fed_by(options, vec![None, Some(0)], DataType::Int32);
    let mut chunks = RetainedChunks::new();
    batcher
        .push_row(
            0,
            0,
            RawRow::unchecked(b"not an integer\t77"),
            &mut RowSplit::default(),
            &mut chunks,
        )
        .unwrap();
    assert!(!batcher.should_flush(), "only the projected field's bytes count");
    batcher
        .push_row(
            0,
            20,
            RawRow::unchecked(b"nor is this\t88"),
            &mut RowSplit::default(),
            &mut chunks,
        )
        .unwrap();
    assert!(batcher.should_flush(), "two projected bytes, then four");
    let batch = batcher.flush().unwrap();
    assert_eq!(batch.num_columns(), 1);
    assert_eq!(rendered(&batch), vec![Some("77".to_string()), Some("88".to_string())]);
}

/// A row is checked against the **block's** field count, not the
/// projection's width — `push_row` is the system's only field-count
/// check, and a projection must not weaken it.
#[test]
fn the_field_count_check_is_against_the_block_not_the_projection() {
    let mut batcher = one_column_batcher_fed_by(
        QueryOptions::default(),
        vec![None, Some(0), None],
        DataType::Utf8View,
    );
    let mut chunks = RetainedChunks::new();
    let err = batcher
        .push_row(0, 0, RawRow::unchecked(b"a\t1"), &mut RowSplit::default(), &mut chunks)
        .unwrap_err();
    match err {
        Error::ColumnCountMismatch { expected, found, .. } => {
            assert_eq!((expected, found), (3, 2));
        }
        other => panic!("expected ColumnCountMismatch, got {other:?}"),
    }
}

/// A row a term has already begun splitting is finished by the batcher,
/// not re-walked, and gives the same answer as a split nothing touched.
#[test]
fn a_partly_filled_split_gives_the_same_row_as_a_fresh_one() {
    let rows: [&[u8]; 3] = [b"a\t1\tz", b"x\t\\N\ty", b"\t2\t"];
    for row in rows {
        let targets = vec![None, Some(0), None];
        let mut fresh =
            one_column_batcher_fed_by(QueryOptions::default(), targets.clone(), DataType::Utf8View);
        let mut shared =
            one_column_batcher_fed_by(QueryOptions::default(), targets, DataType::Utf8View);
        let mut chunks = RetainedChunks::new();
        let mut split = RowSplit::default();
        fresh.push_row(0, 0, RawRow::unchecked(row), &mut split, &mut chunks).unwrap();
        split.restart();
        // What a `--filter` on the middle column reads first.
        assert!(split.field(row, 1).is_some(), "{row:?}");
        shared.push_row(0, 0, RawRow::unchecked(row), &mut split, &mut chunks).unwrap();
        assert_eq!(rendered(&fresh.flush().unwrap()), rendered(&shared.flush().unwrap()));
    }
}

/// A row wider than the block is refused with the count saturated one
/// past the width, not the row's real one.
#[test]
fn a_row_wider_than_the_block_reports_one_past_the_width() {
    let mut batcher =
        one_column_batcher_fed_by(QueryOptions::default(), vec![None, Some(0)], DataType::Utf8View);
    let mut chunks = RetainedChunks::new();
    let err = batcher
        .push_row(0, 0, RawRow::unchecked(b"a\t1\tz\textra"), &mut RowSplit::default(), &mut chunks)
        .unwrap_err();
    match err {
        Error::ColumnCountMismatch { expected, found, .. } => {
            assert_eq!((expected, found), (2, 3));
        }
        other => panic!("expected ColumnCountMismatch, got {other:?}"),
    }
}

/// A zero-column projection builds nothing and still reports its rows —
/// what `RecordBatch::try_new` cannot express.
#[test]
fn a_zero_column_batch_carries_only_its_row_count() {
    use arrow::datatypes::Schema;

    let resolved = ResolvedSchema {
        schema: Arc::new(Schema::new(Vec::<Field>::new())),
        ..ResolvedSchema::default()
    };
    let mut batcher = RowBatcher::new(
        &resolved,
        "public.t".into(),
        QueryOptions::default(),
        vec![None, None],
        Vec::new(),
    );
    let mut chunks = RetainedChunks::new();
    batcher
        .push_row(0, 0, RawRow::unchecked(b"a\tb"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    batcher
        .push_row(0, 4, RawRow::unchecked(b"c\td"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    let batch = batcher.flush().unwrap();
    assert_eq!(batch.num_columns(), 0);
    assert_eq!(batch.num_rows(), 2);
}

/// Every row of a one-column batch, rendered back to text.
fn rendered(batch: &RecordBatch) -> Vec<Option<String>> {
    (0..batch.num_rows())
        .map(|row| {
            render_field(batch.column(0).as_ref(), row, &NestedPlan::Scalar).expect("renders back")
        })
        .collect()
}

/// The span trigger measures from the first selected row's offset to one
/// past the latest selected row's last byte and fires the moment that
/// reaches the cap, so it is overshot by at most one row. The rows here
/// are four bytes apart, with `max_rows` unbounded and `max_bytes` off.
#[test]
fn the_source_span_trigger_fires_on_the_distance_between_selected_rows() {
    let options = QueryOptions {
        max_rows: usize::MAX,
        max_bytes: None,
        max_source_span: Some(100),
        ..Default::default()
    };
    let mut batcher = one_column_batcher(options);
    let mut chunks = RetainedChunks::new();
    assert!(!batcher.should_flush(), "an empty batch never flushes");

    // Offsets 0, 4, 8, … each three bytes of row: the span after the row
    // at offset `n` is `n + 3`, so the first `>= 100` is at offset 100.
    for offset in (0..100).step_by(4) {
        batcher
            .push_row(0, offset, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
            .unwrap();
        assert!(!batcher.should_flush(), "span {} is under the cap", offset + 3);
    }
    batcher
        .push_row(0, 100, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    assert!(batcher.should_flush(), "span 103 has reached the cap");

    // Flushing reopens the span at the next row rather than at the
    // flushed batch's start: the row at 104 opens a span of three bytes,
    // not one from 100.
    assert_eq!(batcher.flush().unwrap().num_rows(), 26);
    assert!(!batcher.should_flush());
    batcher
        .push_row(0, 104, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    assert!(!batcher.should_flush(), "the span restarts at the first row after a flush");
}

/// A stretch of rows the predicate rejects moves the scanner but not the
/// span, a rejected row never reaching `push_row`.
#[test]
fn rows_that_were_never_pushed_do_not_widen_the_span() {
    let options = QueryOptions {
        max_rows: usize::MAX,
        max_bytes: None,
        max_source_span: Some(100),
        ..Default::default()
    };
    let mut batcher = one_column_batcher(options);
    let mut chunks = RetainedChunks::new();
    batcher
        .push_row(0, 1_000_000, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    assert!(!batcher.should_flush(), "a span opens at the first selected row, not at zero");
    batcher
        .push_row(0, 1_000_040, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    assert!(!batcher.should_flush(), "43 bytes of span, whatever lay between");
}

/// `None` opts out of the trigger entirely: no span, however wide,
/// flushes on its own.
#[test]
fn a_none_source_span_leaves_a_batch_unbounded() {
    let options = QueryOptions {
        max_rows: usize::MAX,
        max_bytes: None,
        max_source_span: None,
        ..Default::default()
    };
    let mut batcher = one_column_batcher(options);
    let mut chunks = RetainedChunks::new();
    batcher
        .push_row(0, 0, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    batcher
        .push_row(0, 1 << 40, RawRow::unchecked(b"abc"), &mut RowSplit::default(), &mut chunks)
        .unwrap();
    assert!(!batcher.should_flush());
}
