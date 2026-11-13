use arrow::datatypes::Field;

use arrow::datatypes::DataType;

use super::*;
use crate::render::render_plan;
use pgdt_codec::pgtype::NestedPlan;

/// A [`RowBatcher`] over one nullable `data_type` column, fed by the field
/// `field_targets` says — for driving `push_row` at chosen file offsets,
/// the flush triggers being arithmetic over offsets and row lengths. Its
/// row count never flushes a batch, so `options` alone bounds one.
fn one_column_batcher_fed_by(
    options: QueryOptions,
    field_targets: Vec<Option<usize>>,
    data_type: DataType,
) -> RowBatcher {
    use pgdt_codec::ColumnResolution;
    use pgdt_codec::compare::comparison_for;
    use pgdt_codec::internal::{column_of, schema_of};

    let declared = if data_type == DataType::Int32 { "integer" } else { "text" };
    let resolved = schema_of(vec![column_of(
        Field::new("v", data_type, true),
        Some(declared.into()),
        ColumnResolution::Mapped,
        NestedPlan::Scalar,
        comparison_for(declared, None, &[], &[]),
        false,
    )]);
    RowBatcher::new(
        &resolved,
        "public.t".into(),
        options,
        usize::MAX,
        usize::MAX,
        field_targets,
        Vec::new(),
    )
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

/// **A decode error quotes its field's head** as a refusal does (I94): a
/// field past 100 bytes is named by its first 100 and `...`, not whole.
#[test]
fn a_long_field_s_decode_error_quotes_its_head() {
    let mut batcher = one_column_batcher_fed_by(Default::default(), vec![Some(0)], DataType::Int32);
    let field = format!("1{}", "x".repeat(300));
    let err = batcher
        .push_row(
            0,
            0,
            RawRow::unchecked(field.as_bytes()),
            &mut RowSplit::default(),
            &mut RetainedChunks::new(),
        )
        .unwrap_err();
    match err {
        Error::FieldDecode { value, .. } => assert_eq!(value, format!("{}...", &field[..100])),
        other => panic!("expected FieldDecode, got {other:?}"),
    }
}

/// A row is checked against the **block's** field count, not the
/// projection's width — `hold_to_width`, which `push_row` and a counted
/// row's push reach, is the system's only field-count check, and a
/// projection must not weaken it.
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
    let resolved = ResolvedSchema::default();
    let mut batcher = RowBatcher::new(
        &resolved,
        "public.t".into(),
        QueryOptions::default(),
        usize::MAX,
        usize::MAX,
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
            render_plan(batch.column(0).as_ref(), row, &NestedPlan::Scalar).expect("renders back")
        })
        .collect()
}

/// The span trigger measures from the first selected row's offset to one
/// past the latest selected row's last byte and fires the moment that
/// reaches the cap, so it is overshot by at most one row. The rows here
/// are four bytes apart, and `max_bytes` is off.
#[test]
fn the_source_span_trigger_fires_on_the_distance_between_selected_rows() {
    let options =
        QueryOptions { max_bytes: None, max_source_span: Some(100), ..Default::default() };
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
    let options =
        QueryOptions { max_bytes: None, max_source_span: Some(100), ..Default::default() };
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
    let options = QueryOptions { max_bytes: None, max_source_span: None, ..Default::default() };
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

/// A [`RowBatcher`] over one nullable nested column of `data_type`, read by
/// `plan`, fed by the row's only field.
fn nested_batcher(data_type: DataType, plan: NestedPlan) -> RowBatcher {
    nested_batcher_reading(data_type, plan, Vec::new())
}

/// [`nested_batcher`], its column's values its type cannot hold read as
/// `unrepresentable` says.
fn nested_batcher_reading(
    data_type: DataType,
    plan: NestedPlan,
    unrepresentable: Vec<Option<UnrepresentableRead>>,
) -> RowBatcher {
    use pgdt_codec::ColumnResolution;
    use pgdt_codec::compare::comparison_for;
    use pgdt_codec::internal::{column_of, schema_of};

    let resolved = schema_of(vec![column_of(
        Field::new("v", data_type, true),
        Some("text[]".into()),
        ColumnResolution::Mapped,
        plan,
        comparison_for("text[]", None, &[], &[]),
        false,
    )]);
    RowBatcher::new(
        &resolved,
        "public.t".into(),
        Default::default(),
        usize::MAX,
        usize::MAX,
        vec![Some(0)],
        unrepresentable,
    )
}

/// **A nested field is walked off its bytes as `COPY` wrote them**
/// (`docs/design/decisions.md`, "D153"): a value whose `COPY` form carries
/// an escape, outside its quoting and inside it, reads as its text does,
/// every form's builders filled as the walk hands its parts over.
#[test]
fn an_escaped_nested_field_reads_as_its_text() {
    use pgdt_codec::pgtype::RANGE_STRUCT_FIELDS;
    use pgdt_core::escape_field;
    use std::sync::Arc;

    let list = |child: DataType| DataType::List(Arc::new(Field::new("item", child, true)));
    let range = DataType::Struct(arrow::datatypes::Fields::from(
        [
            DataType::Utf8View,
            DataType::Utf8View,
            DataType::Boolean,
            DataType::Boolean,
            DataType::Boolean,
        ]
        .into_iter()
        .zip(RANGE_STRUCT_FIELDS)
        .enumerate()
        .map(|(i, (t, name))| Field::new(name, t, i < 2))
        .collect::<Vec<_>>(),
    ));
    let pair = DataType::Struct(arrow::datatypes::Fields::from(vec![
        Field::new("x", DataType::Utf8View, true),
        Field::new("y", list(DataType::Utf8View), true),
    ]));
    let text_range = NestedPlan::Range(Box::new(NestedPlan::Scalar));
    let cases = [
        (
            list(DataType::Utf8View),
            NestedPlan::array(NestedPlan::Scalar),
            "{\"a\tb\",\"c\\\\d\",\"e\nf\",NULL,plain}",
        ),
        (
            list(list(DataType::Utf8View)),
            NestedPlan::array(NestedPlan::array(NestedPlan::Scalar)),
            "{{\"\t\",x},{\"y\\\\\",\"\r\"}}",
        ),
        (
            pair,
            NestedPlan::record([NestedPlan::Scalar, NestedPlan::array(NestedPlan::Scalar)]),
            "(\"a\tb\",\"{\"\"c\\\\\\\\d\"\",\"\"e\tf\"\"}\")",
        ),
        (range.clone(), text_range.clone(), "[\"a\tb\",\"c\\\\d\")"),
        (
            list(range),
            NestedPlan::Multirange(Box::new(NestedPlan::Scalar)),
            "{[\"a\t\",b),(c,\"d\\\\\"]}",
        ),
    ];
    for (data_type, plan, text) in cases {
        let field = escape_field(Some(text));
        assert!(field.contains(&b'\\'), "{text:?} is written with an escape");
        let mut batcher = nested_batcher(data_type, plan.clone());
        batcher
            .push_row(
                0,
                0,
                RawRow::unchecked(&field),
                &mut RowSplit::default(),
                &mut RetainedChunks::new(),
            )
            .unwrap();
        let batch = batcher.flush().unwrap();
        let read = render_plan(batch.column(0).as_ref(), 0, &plan).expect("renders back");
        assert_eq!(read.as_deref(), Some(text));
    }
}

/// An escaped nested field that does not unescape to UTF-8 raises the row's
/// own error, and one that is no value of its column quotes its head as a
/// refusal does (I94).
#[test]
fn an_escaped_nested_field_fails_as_its_text_would() {
    let plan = NestedPlan::array(NestedPlan::Scalar);
    let list = DataType::List(std::sync::Arc::new(Field::new("item", DataType::Int32, true)));
    let push = |field: &[u8]| {
        nested_batcher(list.clone(), plan.clone())
            .push_row(
                0,
                0,
                RawRow::unchecked(field),
                &mut RowSplit::default(),
                &mut RetainedChunks::new(),
            )
            .unwrap_err()
    };
    match push(b"{1,\\xff}") {
        Error::InvalidUtf8 { valid_up_to } => assert_eq!(valid_up_to, 3),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
    let field = format!("{{1,\\t{}}}", "2,".repeat(100));
    match push(field.as_bytes()) {
        Error::FieldDecode { value, .. } => {
            assert_eq!(value, format!("{{1,\t{}...", &"2,".repeat(100)[..96]));
        }
        other => panic!("expected FieldDecode, got {other:?}"),
    }
}

/// **A nested value its tier test reads is tested as it is walked**
/// (`docs/design/decisions.md`, "D153"): one holding a leaf past what the
/// query holds is NULL whole, whether `COPY` wrote it in place or with an
/// escape, and one holding none is decoded, read as its text is.
#[test]
fn a_tested_nested_field_is_null_whole_as_walked() {
    use arrow::array::{Array, AsArray};
    use pgdt_codec::internal::column_read;
    use pgdt_model::UnrepresentableTier;

    let plan = NestedPlan::array(NestedPlan::Scalar);
    let list = DataType::List(std::sync::Arc::new(Field::new("item", DataType::Date32, true)));
    let read = column_read(&list, &plan, UnrepresentableTier::Format);
    assert!(read.is_some(), "a `date[]` is tested");
    let mut batcher = nested_batcher_reading(list, plan, vec![read]);
    let mut chunks = RetainedChunks::new();
    let fields: [(&[u8], Option<i32>); 5] = [
        (b"{2024-01-01,infinity}", None),
        (b"{\\x32024-01-01,\\x69nfinity}", None),
        (b"{\\x32024-01-01,2024-01-02}", Some(2)),
        (b"{2024-01-01}", Some(1)),
        (b"\\N", None),
    ];
    for (field, _) in fields {
        batcher
            .push_row(0, 0, RawRow::unchecked(field), &mut RowSplit::default(), &mut chunks)
            .unwrap();
    }
    let batch = batcher.flush().unwrap();
    let column = batch.column(0).as_list::<i32>();
    for (row, (field, length)) in fields.iter().enumerate() {
        let read = column.is_valid(row).then(|| column.value_length(row));
        assert_eq!(read, *length, "{}", String::from_utf8_lossy(field));
    }
}
