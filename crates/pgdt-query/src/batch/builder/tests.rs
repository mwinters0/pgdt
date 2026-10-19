use arrow::datatypes::Field;

use std::sync::Arc;

use arrow::array::{Array, BooleanArray, Int32Array, RecordBatch};

use super::*;
use crate::render::render_plan;
use pgdt_codec::pgtype::RANGE_STRUCT_FIELDS;
use pgdt_core::Error;

fn list_of(child: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", child, true)))
}

fn point2d() -> DataType {
    DataType::Struct(Fields::from(vec![
        Field::new("x", DataType::Int32, true),
        Field::new("y", DataType::Utf8View, true),
    ]))
}

fn range_struct(bound: DataType) -> DataType {
    DataType::Struct(Fields::from(vec![
        Field::new(RANGE_STRUCT_FIELDS[0], bound.clone(), true),
        Field::new(RANGE_STRUCT_FIELDS[1], bound, true),
        Field::new(RANGE_STRUCT_FIELDS[2], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[3], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[4], DataType::Boolean, false),
    ]))
}

fn record(plans: &[NestedPlan]) -> NestedPlan {
    NestedPlan::record(plans.iter().cloned())
}

fn build(
    data_type: &DataType,
    plan: &NestedPlan,
    values: &[Option<&str>],
) -> std::result::Result<ArrayRef, String> {
    let mut builder = new_column_builder(data_type, plan);
    for value in values {
        match value {
            None => append_null(&mut builder),
            Some(text) => append_typed(&mut builder, text, PostgresInvalidValues::Default)?,
        }
    }
    Ok(finish_column(&mut builder))
}

/// Build a column from literals, then render every row back. The array's
/// own `DataType` must match what the schema promised — `RecordBatch::try_new`
/// checks exactly this — and the text must survive unchanged.
#[track_caller]
fn round_trips(data_type: DataType, plan: NestedPlan, values: &[Option<&str>]) -> ArrayRef {
    let array = build(&data_type, &plan, values).expect("every value here is well-formed");
    assert_eq!(array.data_type(), &data_type, "built array's type must match the schema");
    assert_eq!(array.len(), values.len());
    let rendered: Vec<Option<String>> = (0..array.len())
        .map(|i| render_plan(array.as_ref(), i, &plan).expect("renders back"))
        .collect();
    let expected: Vec<Option<String>> = values.iter().map(|v| v.map(str::to_string)).collect();
    assert_eq!(rendered, expected);
    array
}

/// An `Interval(MonthDayNano)` array holding a value no `interval` has.
/// Built by hand because nothing in this crate can produce one —
/// `append_typed` fills the column from `decode_interval`, which
/// multiplies microseconds by a thousand — so the refusal it pins is the
/// contract for an array a caller assembled. It travels out of a nested
/// column too, the nested walk being the same function.
#[test]
fn render_refuses_an_interval_with_no_postgresql_text_form() {
    let mut b = IntervalMonthDayNanoBuilder::new();
    b.append_value(IntervalMonthDayNano { months: 0, days: 0, nanoseconds: 1_500 });
    b.append_value(IntervalMonthDayNano { months: 0, days: 0, nanoseconds: 1_000 });
    let array = Arc::new(b.finish()) as ArrayRef;

    let err = render_plan(array.as_ref(), 0, &NestedPlan::Scalar).unwrap_err();
    assert!(
        matches!(&err, Error::FieldRender { declared_type: "interval", reason } if reason.contains("1500 ns")),
        "{err}"
    );
    assert_eq!(
        render_plan(array.as_ref(), 1, &NestedPlan::Scalar).unwrap().as_deref(),
        Some("00:00:00.000001"),
    );

    let list = ListArray::new(
        Arc::new(Field::new("item", DataType::Interval(IntervalUnit::MonthDayNano), true)),
        OffsetBuffer::from_lengths([2usize]),
        array,
        None,
    );
    let plan = NestedPlan::array(NestedPlan::Scalar);
    assert!(matches!(
        render_plan(&list, 0, &plan).unwrap_err(),
        Error::FieldRender { declared_type: "interval", .. }
    ));
}

/// `int2vector` and `smallint[]` are one Arrow type and two literal
/// forms — [`NestedPlan`]'s collision — so the two are built side by
/// side, from text neither could read as the other.
#[test]
fn an_int2vector_column_round_trips_and_is_not_the_array_of_the_same_type() {
    let array = round_trips(
        list_of(DataType::Int16),
        NestedPlan::Int2Vector,
        &[Some("1 2 3"), Some(""), Some("-32768 32767"), Some("0"), None],
    );
    let list = array.as_any().downcast_ref::<ListArray>().unwrap();
    // The empty vector is the empty *field*, and `\N` is the NULL beside
    // it: an empty list, and no list.
    assert_eq!(list.value(1).len(), 0);
    assert!(!list.is_null(1));
    assert!(list.is_null(4));
    assert_eq!(list.value(0).len(), 3);

    // Neither grammar reads the other's text.
    let array_plan = NestedPlan::array(NestedPlan::Scalar);
    assert!(build(&list_of(DataType::Int16), &array_plan, &[Some("1 2 3")]).is_err());
    assert!(build(&list_of(DataType::Int16), &NestedPlan::Int2Vector, &[Some("{1,2}")]).is_err());
}

/// The one value class an `int2vector` column cannot hold: PostgreSQL's
/// `int2vector` has no NULL element, so a `List<Int16>` carrying one has
/// no `int2vectorout` text at all. Built by hand for the reason the
/// `interval` refusal above is — `append_typed` can never produce one.
#[test]
fn render_refuses_an_int2vector_holding_a_null_element() {
    let mut values = Int16Builder::new();
    values.append_value(1);
    values.append_null();
    let list = ListArray::new(
        Arc::new(Field::new("item", DataType::Int16, true)),
        OffsetBuffer::from_lengths([2usize]),
        Arc::new(values.finish()) as ArrayRef,
        None,
    );
    let err = render_plan(&list, 0, &NestedPlan::Int2Vector).unwrap_err();
    assert!(
        matches!(&err, Error::FieldRender { declared_type: "int2vector", reason }
                if reason.contains("NULL element")),
        "{err}"
    );
}

#[test]
fn a_one_dimensional_list_column_round_trips_every_null_and_quoting_case() {
    let array = round_trips(
        list_of(DataType::Utf8View),
        NestedPlan::array(NestedPlan::Scalar),
        &[Some("{a,b}"), Some("{}"), Some("{NULL}"), Some(r#"{"a,b","has space"}"#), None],
    );
    let list = array.as_any().downcast_ref::<ListArray>().unwrap();
    // `{}` and a NULL array are different: an empty list, and no list.
    assert_eq!(list.value(1).len(), 0);
    assert!(!list.is_null(1));
    assert!(list.is_null(4));
    // `{NULL}` is a one-element list whose element is null.
    assert_eq!(list.value(2).len(), 1);
    assert!(list.value(2).is_null(0));
}

#[test]
fn a_list_of_a_decoded_scalar_type_decodes_its_elements() {
    let array = round_trips(
        list_of(DataType::Int32),
        NestedPlan::array(NestedPlan::Scalar),
        &[Some("{1,2,3}"), Some("{-1,NULL}"), None],
    );
    let list = array.as_any().downcast_ref::<ListArray>().unwrap();
    let first = list.value(0);
    let ints = first.as_any().downcast_ref::<Int32Array>().unwrap();
    assert_eq!(ints.values(), &[1, 2, 3]);
}

#[test]
fn a_two_dimensional_value_fills_two_list_levels() {
    let plan = NestedPlan::array(NestedPlan::array(NestedPlan::Scalar));
    let array = round_trips(
        list_of(list_of(DataType::Int32)),
        plan,
        // `{}` fits any depth — `array_out` collapses a zero-element
        // array of every dimensionality to it.
        &[Some("{{1,2},{3,4}}"), Some("{{5},{6},{7}}"), Some("{}"), None],
    );
    let outer = array.as_any().downcast_ref::<ListArray>().unwrap();
    assert_eq!(outer.value(0).len(), 2);
    assert_eq!(outer.value(1).len(), 3);
    assert_eq!(outer.value(2).len(), 0);
}

#[test]
fn a_value_whose_shape_does_not_fit_the_column_is_refused_rather_than_reshaped() {
    let flat = NestedPlan::array(NestedPlan::Scalar);
    let nested = NestedPlan::array(NestedPlan::array(NestedPlan::Scalar));
    for (data_type, plan, value) in [
        // A multi-dimensional value in a 1-D column.
        (list_of(DataType::Int32), flat.clone(), "{{1,2},{3,4}}"),
        // A 1-D value in a column resolved as 2-D.
        (list_of(list_of(DataType::Int32)), nested, "{1,2}"),
        // An `[lb:ub]=` prefix: Arrow lists are 0-based with no lower
        // bound, so the index origin has nowhere to go.
        (list_of(DataType::Int32), flat.clone(), "[0:2]={7,8,9}"),
        // Not an array literal at all.
        (list_of(DataType::Int32), flat.clone(), "1,2"),
        // An element that is not a value of the element type.
        (list_of(DataType::Int32), flat, "{x}"),
    ] {
        assert_eq!(
            build(&data_type, &plan, &[Some(value)]).unwrap_err(),
            value,
            "should have been refused: {value}"
        );
    }
}

#[test]
fn a_struct_column_round_trips_a_composite_including_its_null_and_empty_fields() {
    let array = round_trips(
        point2d(),
        record(&[NestedPlan::Scalar, NestedPlan::Scalar]),
        &[Some(r#"(1,"a,b""c")"#), Some(r#"(,"")"#), Some("(3,)"), None],
    );
    let s = array.as_any().downcast_ref::<StructArray>().unwrap();
    assert!(s.is_null(3));
    // A null struct row still gives every child an entry, or the array
    // is malformed.
    assert_eq!(s.column(0).len(), 4);
    assert!(s.column(0).is_null(1), "`(,\"\")` has a NULL first field");
    assert!(!s.column(1).is_null(1), "and an empty-string second one");
}

/// **A NULL at a position whose type is a `NOT NULL` domain is refused
/// but under `ignore`** (I76): an array's element, at the innermost of the
/// census's levels, whose outer ones hold sub-arrays, and a composite's
/// field, `()` being a one-field composite's NULL; a NULL elsewhere in
/// the same value, and a range's infinite bound, are read.
#[test]
fn a_null_at_a_not_null_position_is_refused_but_under_ignore() {
    let refusing = |plan| Position { plan, not_null: true };
    let elements = NestedPlan::Array(Box::new(refusing(NestedPlan::Scalar)));
    let one = DataType::Struct(Fields::from(vec![Field::new("x", DataType::Int32, true)]));
    let first = NestedPlan::Record(vec![
        refusing(NestedPlan::Scalar),
        Position::nullable(NestedPlan::Scalar),
    ]);
    let read = |data_type: &DataType, plan: &NestedPlan, value, invalid| {
        let mut builder = new_column_builder(data_type, plan);
        append_typed(&mut builder, value, invalid).map_err(drop)
    };
    for (data_type, plan, refused, read_as_written) in [
        (list_of(DataType::Int32), elements.clone(), "{1,NULL}", "{1,2}"),
        (
            list_of(list_of(DataType::Int32)),
            NestedPlan::array(elements.clone()),
            "{{1,2},{3,NULL}}",
            "{{1,2},{3,4}}",
        ),
        (point2d(), first.clone(), "(,a)", "(1,)"),
        (one, NestedPlan::Record(vec![refusing(NestedPlan::Scalar)]), "()", "(1)"),
        (list_of(point2d()), NestedPlan::array(first), r#"{"(,a)"}"#, r#"{"(1,)",NULL}"#),
    ] {
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
            assert_eq!(read(&data_type, &plan, refused, invalid), Err(()), "{refused}");
            assert_eq!(read(&data_type, &plan, read_as_written, invalid), Ok(()));
        }
        assert_eq!(read(&data_type, &plan, refused, PostgresInvalidValues::Ignore), Ok(()));
    }
    let range = NestedPlan::Range(Box::new(NestedPlan::Scalar));
    let infinite = read(&range_struct(DataType::Int32), &range, "(,3)", Default::default());
    assert_eq!(infinite, Ok(()));
}

#[test]
fn a_composite_literal_with_the_wrong_field_count_is_refused() {
    let plan = record(&[NestedPlan::Scalar, NestedPlan::Scalar]);
    for value in ["(1)", "(1,2,3)"] {
        assert_eq!(build(&point2d(), &plan, &[Some(value)]).unwrap_err(), value);
    }
}

#[test]
fn the_range_struct_keeps_empty_apart_from_an_unbounded_range() {
    let array = round_trips(
        range_struct(DataType::Int32),
        NestedPlan::Range(Box::new(NestedPlan::Scalar)),
        &[Some("[1,10)"), Some("empty"), Some("(,5)"), Some("(,)"), None],
    );
    let s = array.as_any().downcast_ref::<StructArray>().unwrap();
    let empty = s.column(4).as_any().downcast_ref::<BooleanArray>().unwrap();
    // `empty` and `(,)` both have absent bounds; only the flag separates
    // them, which is why the fifth field is not redundant.
    assert!(s.column(0).is_null(1) && s.column(0).is_null(3));
    assert!(empty.value(1) && !empty.value(3));
}

#[test]
fn a_range_over_a_text_subtype_keeps_its_quoted_bounds() {
    round_trips(
        range_struct(DataType::Utf8View),
        NestedPlan::Range(Box::new(NestedPlan::Scalar)),
        &[Some(r#"["a,b","c""d")"#), Some(r#"["",a)"#), Some("(,z)")],
    );
}

/// The collision `NestedPlan` exists for: one Arrow type, two literal
/// forms, and nothing in the type to tell them apart.
#[test]
fn an_array_of_ranges_and_a_multirange_share_a_type_and_not_a_literal() {
    let bound = Box::new(NestedPlan::Scalar);
    let data_type = list_of(range_struct(DataType::Int32));

    let as_array = round_trips(
        data_type.clone(),
        NestedPlan::array(NestedPlan::Range(bound.clone())),
        &[Some(r#"{"[1,10)","[2,3)"}"#), Some("{}"), None],
    );
    let as_multirange = round_trips(
        data_type,
        NestedPlan::Multirange(bound),
        &[Some("{[1,10),[2,3)}"), Some("{}"), None],
    );
    assert_eq!(as_array.data_type(), as_multirange.data_type());
    // Same Arrow values, too — only the text differs.
    assert_eq!(as_array.to_data(), as_multirange.to_data());
}

#[test]
fn both_nesting_orders_carry_both_escape_conventions_through_the_builders() {
    // `public.point2d[]` — array quoting outside, record doubling inside.
    round_trips(
        list_of(point2d()),
        NestedPlan::array(record(&[NestedPlan::Scalar, NestedPlan::Scalar])),
        &[Some(r#"{"(1,\"a,b\"\"c\")","(2,plain)"}"#), Some("{NULL,\"(3,)\"}")],
    );

    // `public.tagged` — the other way up.
    let tagged = DataType::Struct(Fields::from(vec![
        Field::new("label", DataType::Utf8View, true),
        Field::new("tags", list_of(DataType::Utf8View), true),
    ]));
    round_trips(
        tagged,
        record(&[NestedPlan::Scalar, NestedPlan::array(NestedPlan::Scalar)]),
        &[Some(r#"("a,b","{""x\\""y"",""p q"",NULL}")"#), Some("(\"\",{})")],
    );
}

/// A batch is only valid if every column's array type matches the schema
/// field it was built from — the check `RecordBatch::try_new` performs
/// and the reason the builders reuse the schema's own `Field`s.
#[test]
fn nested_columns_assemble_into_a_record_batch() {
    use arrow::datatypes::Schema;

    let fields = vec![
        Field::new("tags", list_of(DataType::Utf8View), true),
        Field::new("pt", point2d(), true),
        Field::new("span", range_struct(DataType::Int32), true),
    ];
    let plans = [
        NestedPlan::array(NestedPlan::Scalar),
        record(&[NestedPlan::Scalar, NestedPlan::Scalar]),
        NestedPlan::Range(Box::new(NestedPlan::Scalar)),
    ];
    let values: [&[Option<&str>]; 3] =
        [&[Some("{a,b}"), None], &[Some("(1,x)"), None], &[Some("[1,10)"), None]];
    let arrays: Vec<ArrayRef> = fields
        .iter()
        .zip(&plans)
        .zip(values)
        .map(|((f, plan), v)| build(f.data_type(), plan, v).unwrap())
        .collect();
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap();
    assert_eq!(batch.num_rows(), 2);
}
