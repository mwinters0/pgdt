use super::*;
use arrow::datatypes::{Field, Fields};
use std::sync::Arc;

fn list_of(child: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", child, true)))
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

/// A scalar column's Arrow type is arrow's own `Display`, unmodified —
/// the reason the line is printed for every mapped non-`Utf8View` column
/// rather than only for nested ones.
#[test]
fn a_scalar_column_renders_as_arrows_own_display() {
    assert_eq!(
        arrow_type_label(&DataType::Decimal128(38, 10), &NestedPlan::Scalar),
        "Decimal128(38, 10)"
    );
}

#[test]
fn arrays_and_composites_render_through_arrows_display() {
    assert_eq!(
        arrow_type_label(&list_of(DataType::Utf8View), &NestedPlan::array(NestedPlan::Scalar)),
        "List(Utf8View)"
    );
    let point = DataType::Struct(Fields::from(vec![
        Field::new("x", DataType::Int32, true),
        Field::new("y", DataType::Utf8View, true),
    ]));
    assert_eq!(
        arrow_type_label(&point, &NestedPlan::record(vec![NestedPlan::Scalar; 2])),
        r#"Struct("x": Int32, "y": Utf8View)"#
    );
}

/// The one substitution: the five-field range struct is identical in
/// every dump, so only its bound type is worth printing.
#[test]
fn the_range_struct_collapses_to_its_bound_type() {
    assert_eq!(
        arrow_type_label(
            &range_struct(DataType::Int32),
            &NestedPlan::Range(Box::new(NestedPlan::Scalar))
        ),
        "Range<Int32>"
    );
}

/// A built-in multirange and an array of the matching range are the
/// *same* Arrow type and different plans, so rendering identically is
/// correct — the declared PostgreSQL type, on the table's `columns:` line
/// and beside it in `--json`, tells them apart.
#[test]
fn a_multirange_and_an_array_of_the_matching_range_render_identically() {
    let multirange = arrow_type_label(
        &list_of(range_struct(DataType::Int32)),
        &NestedPlan::Multirange(Box::new(NestedPlan::Scalar)),
    );
    let array_of_range = arrow_type_label(
        &list_of(range_struct(DataType::Int32)),
        &NestedPlan::array(NestedPlan::Range(Box::new(NestedPlan::Scalar))),
    );
    assert_eq!(multirange, "List(Range<Int32>)");
    assert_eq!(array_of_range, multirange);
}

/// Dispatch is the plan's, never the field names': a user composite may
/// declare five fields with the range struct's names and must still print
/// as the struct it is.
#[test]
fn a_composite_wearing_the_range_structs_field_names_is_not_collapsed() {
    let impostor = range_struct(DataType::Int32);
    let rendered = arrow_type_label(&impostor, &NestedPlan::record(vec![NestedPlan::Scalar; 5]));
    assert!(rendered.starts_with(r#"Struct("lower": Int32"#), "{rendered}");
    assert!(!rendered.contains("Range<"), "{rendered}");
}

/// Every `TypeKind` arm renders, with whatever payload it carries. Two
/// are shapes `pg_dump` does not write — a composite whose body held an
/// unparseable fragment, a range whose parameter list named no `subtype` —
/// so this is the only place they are pinned.
#[test]
fn every_type_kind_renders_with_its_own_payload() {
    use pgdt_model::ColumnDef;

    let labels = |ls: &[&str]| TypeKind::Enum {
        labels: ls.iter().map(|l| l.to_string()).collect(),
        exact: true,
    };
    assert_eq!(type_kind_summary(&labels(&["sad", "has'quote"])), "enum: 'sad', 'has''quote'");
    assert_eq!(type_kind_summary(&labels(&[])), "enum: (no labels)");
    let inexact = TypeKind::Enum { labels: vec!["sad".to_string()], exact: false };
    assert_eq!(type_kind_summary(&inexact), "enum: 'sad' (labels not read exactly)");
    let unread = TypeKind::Enum { labels: Vec::new(), exact: false };
    assert_eq!(type_kind_summary(&unread), "enum: (labels not read)");
    assert_eq!(type_kind_summary(&TypeKind::domain("integer")), "domain over integer");
    assert_eq!(
        type_kind_summary(&TypeKind::Domain {
            base_type: "text".to_string(),
            collation: Some(r#"pg_catalog."C""#.to_string()),
            not_null: false,
            check: false,
        }),
        r#"domain over text COLLATE pg_catalog."C""#
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Domain {
            base_type: "integer".to_string(),
            collation: None,
            not_null: true,
            check: true,
        }),
        "domain over integer NOT NULL CHECK"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Composite {
            fields: Some(vec![
                ColumnDef::new("x", "integer"),
                ColumnDef {
                    name: "c".to_string(),
                    declared_type: "text".to_string(),
                    collation: Some(r#"pg_catalog."C""#.to_string()),
                    not_null: None,
                },
            ]),
        }),
        r#"composite: x integer, c text COLLATE pg_catalog."C""#
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Composite { fields: Some(vec![]) }),
        "composite: (no fields)"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Composite { fields: None }),
        "composite: (fields not parsed)"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Range {
            subtype: Some("double precision".to_string()),
            multirange_type_name: Some("public.myrange_multi".to_string()),
            canonical: None,
        }),
        "range over double precision"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Range {
            subtype: None,
            multirange_type_name: None,
            canonical: None
        }),
        "range (subtype not parsed)"
    );
    // A declared `canonical` function is named: it is why every filter
    // operator refuses a column of this type.
    assert_eq!(
        type_kind_summary(&TypeKind::Range {
            subtype: Some("integer".to_string()),
            multirange_type_name: None,
            canonical: Some("public.canonrange_canonical".to_string()),
        }),
        "range over integer, canonical public.canonrange_canonical"
    );
    assert_eq!(type_kind_summary(&TypeKind::base()), "base type");
    assert_eq!(type_kind_summary(&TypeKind::Shell), "shell type");
}
