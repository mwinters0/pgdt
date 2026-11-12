//! **A column's decode factor is its densest repeated part's**, beside a
//! value's own bytes and what a part's append holds besides, and none where
//! nothing grows with a value's text; **its Arrow form holds what the factor
//! says** over a column of each type's densest literal.

use std::sync::Arc;

use arrow::array::Array;
use arrow::datatypes::Field;
use pgdt_codec::pgtype::{TypeOutcome, resolve_declared_type};
use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;
use crate::batch::builder::{append_typed, finish_column, reserved_column_builder};
use pgdt_model::PostgresInvalidValues;

fn types() -> Vec<TypeDef> {
    vec![
        TypeDef {
            name: "public.pair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("a", "bigint"), ColumnDef::new("b", "text")]),
            },
        },
        TypeDef {
            name: "public.days".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("a", "date"), ColumnDef::new("b", "date")]),
            },
        },
    ]
}

fn mapped(declared: &str) -> (DataType, NestedPlan) {
    match resolve_declared_type(declared, &types()) {
        TypeOutcome::Mapped(data_type, plan) => (data_type, plan),
        other => panic!("{declared}: {other:?}"),
    }
}

fn factor(declared: &str) -> Option<Factor> {
    let (data_type, plan) = mapped(declared);
    column_factor(&data_type, &plan)
}

#[test]
fn a_column_s_factor_is_its_densest_part_s() {
    let f = |held, per| Some(Factor::new(held, per));
    let cases = [
        // A value's own bytes, one for one.
        ("text", f(1, 1)),
        ("character varying(10)", f(1, 1)),
        ("bytea", f(1, 1)),
        // An unconstrained `numeric` is read as its text.
        ("numeric", f(1, 1)),
        // A scalar, a range of them and a record of them hold what their type
        // bounds.
        ("bigint", None),
        ("numeric(10,2)", None),
        ("date", None),
        ("uuid", None),
        ("int4range", None),
        // An `int8`'s slot and validity bit over a digit and its comma.
        ("bigint[]", f(65, 16)),
        ("integer[]", f(33, 16)),
        ("smallint[]", f(17, 16)),
        ("numeric(76,0)[]", f(257, 16)),
        // A view over `a,`, a long element's bytes in a block of their own,
        // and the token and its unquoted copy beside it.
        ("text[]", f(129 + 16 + 32, 16)),
        // An offset and a validity bit over `a,`, the bytes it decodes to
        // reserved to the text's length; then the token, its unquoted copy
        // and its decoded bytes.
        ("bytea[]", f(33 + 16 + 48, 16)),
        // A range's validity byte, its bounds and its three flags over `(,)`
        // and its delimiter.
        ("int4range[]", f(80, 32)),
        ("int4multirange", f(80, 32)),
        // An element and the space before it.
        ("int2vector", f(17, 16)),
        // A container its tier test reads holds what its walk does, the test
        // a walk of its own before it, staging nothing: a `date`'s slot over
        // a digit and its comma, a range's over `(,)` and its delimiter; a
        // range and a record of dates hold what their type bounds.
        ("date[]", f(33, 16)),
        ("datemultirange", f(80, 32)),
        ("daterange", None),
        ("public.days", None),
        // A record of a `bigint` and a `text`: the text's bytes, the token and
        // its unquoted copy; as a part, its entry over `(,)` and its delimiter
        // beside them.
        ("public.pair", f(3, 1)),
        ("public.pair[]", f(8 + 65 + 129 + 32 + 64, 32)),
    ];
    for (declared, expected) in cases {
        assert_eq!(factor(declared), expected, "{declared}");
    }

    // A dimension's list is a part of the one around it, `{` and its
    // delimiter: its element's is the larger.
    let list = |child| DataType::List(Arc::new(Field::new("item", child, true)));
    let plan = NestedPlan::Array(Box::new(pgdt_codec::pgtype::Position::nullable(
        NestedPlan::Array(Box::new(pgdt_codec::pgtype::Position::nullable(NestedPlan::Scalar))),
    )));
    let builder = new_column_builder(&list(list(DataType::Int64)), &plan);
    assert_eq!(form(&builder), Factor::new(65, 16));
    // Reserved, each dimension's lists and the elements are each at their
    // own density.
    assert_eq!(reserved(&builder), Factor::new(40 + 65, 16));
    assert_eq!(column_factor(&list(list(DataType::Int64)), &plan), Some(reserved(&builder)));
}

/// **Each type's densest literal, repeated, is held within its factor's
/// Arrow form**, beside what each row holds whatever its text: the finished
/// array's buffers, Arrow's own count of them, against the text the column
/// was filled from. Where the part's slot is the whole of it, the form is
/// what the column holds, so the factor is no argued ceiling over it.
#[test]
fn a_column_holds_its_factor_s_arrow_form() {
    const ROWS: usize = 8;
    const PARTS: usize = 1000;
    let cases: [(&str, String, bool); 7] = [
        ("bigint[]", format!("{{{}}}", vec!["1"; PARTS].join(",")), true),
        ("text[]", format!("{{{}}}", vec!["a"; PARTS].join(",")), true),
        ("bytea[]", format!("{{{}}}", vec!["a"; PARTS].join(",")), false),
        ("int4multirange", format!("{{{}}}", vec!["(,)"; PARTS].join(",")), true),
        ("int4range[]", format!("{{{}}}", vec!["\"(,)\""; PARTS].join(",")), false),
        ("int2vector", vec!["1"; PARTS].join(" "), true),
        ("public.pair[]", format!("{{{}}}", vec!["\"(,)\""; PARTS].join(",")), false),
    ];
    for (declared, literal, tight) in cases {
        let (data_type, plan) = mapped(declared);
        let mut builder = new_column_builder(&data_type, &plan);
        let form = form(&builder);
        for _ in 0..ROWS {
            append_typed(&mut builder, &literal, PostgresInvalidValues::Default)
                .unwrap_or_else(|()| panic!("{declared}: {literal}"));
        }
        let held = finish_column(&mut builder).to_data().get_slice_memory_size().unwrap() as u64;
        let text = (ROWS * literal.len()) as u64;
        let per_row = 64 * ROWS as u64;
        assert!(held <= form.of(text) + per_row, "{declared}: {held} against {form:?} of {text}");
        if tight {
            assert!(
                held * 10 >= form.of(text) * 8,
                "{declared}: {held} against {form:?} of {text}"
            );
        }
    }
}

/// **A column reserved for one value holds no more than its reservation
/// states**, whichever of its parts the value is dense in: each type's densest
/// literal, and one long part, decoded into builders reserved to the
/// literal's length, hold within [`reserved`] of it by Arrow's own count of
/// their buffers' capacity, so no buffer outgrew its reservation by doubling.
/// A long text part is longer than a view builder's largest block, which a
/// shorter one is written into whole.
#[test]
fn a_reserved_column_holds_its_reservation() {
    const PARTS: usize = 1000;
    const LONG: usize = 3 << 20;
    let dense = |part: &str| format!("{{{}}}", vec![part; PARTS].join(","));
    let long = format!("{{{}}}", "a".repeat(LONG));
    let two_dimensions = format!("{{{}}}", vec!["{1}"; PARTS].join(","));
    let cases: [(&str, String); 10] = [
        ("bigint[]", dense("1")),
        ("text[]", dense("a")),
        ("text[]", long.clone()),
        ("bytea[]", dense("a")),
        ("bytea[]", long),
        ("int4multirange", dense("(,)")),
        ("int4range[]", dense("\"(,)\"")),
        ("int2vector", vec!["1"; PARTS].join(" ")),
        ("public.pair[]", dense("\"(,)\"")),
        ("public.pair[]", format!("{{\"(1,{})\"}}", "a".repeat(LONG))),
    ];
    let list = |child| DataType::List(Arc::new(Field::new("item", child, true)));
    let nullable = |plan| Box::new(pgdt_codec::pgtype::Position::nullable(plan));
    let typed = cases.into_iter().map(|(declared, literal)| (declared, mapped(declared), literal));
    let square = (
        "a two-dimensional bigint[]",
        (
            list(list(DataType::Int64)),
            NestedPlan::Array(nullable(NestedPlan::Array(nullable(NestedPlan::Scalar)))),
        ),
        two_dimensions,
    );
    for (declared, (data_type, plan), literal) in typed.chain([square]) {
        let mut builder = reserved_column_builder(&data_type, &plan, literal.len());
        let reservation = reserved(&builder).of(literal.len() as u64);
        append_typed(&mut builder, &literal, PostgresInvalidValues::Default)
            .unwrap_or_else(|()| panic!("{declared}: {}", &literal[..16]));
        let held = finish_column(&mut builder).to_data().get_buffer_memory_size() as u64;
        // A list's entry at the top and the alignment of each buffer.
        let slack = 1024;
        assert!(held <= reservation + slack, "{declared}: {held} against {reservation}");
    }
}
