//! **The walk orders a field as its key does**: over seeds of every nested
//! form and every one of them damaged a byte at a time, read in place and as
//! `COPY` escapes it, against bounds of every length, [`nested_field_order`]
//! answers what [`nested_key`] and [`compare_nested`] answer — the same
//! order, or the same refusal.

use pgdt_core::escape_field;
use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;
use crate::compare::{ComparisonPlan, comparison_for};

fn types() -> Vec<TypeDef> {
    let composite = |name: &str, fields: Vec<ColumnDef>| TypeDef {
        name: name.into(),
        kind: TypeKind::Composite { fields: Some(fields) },
    };
    vec![
        composite("public.pair", vec![ColumnDef::new("a", "integer"), ColumnDef::new("b", "text")]),
        composite("public.nothing", vec![]),
        TypeDef {
            name: "public.nn".into(),
            kind: TypeKind::Domain {
                base_type: "integer".into(),
                collation: None,
                not_null: true,
                check: false,
            },
        },
        composite(
            "public.nnpair",
            vec![ColumnDef::new("a", "public.nn"), ColumnDef::new("b", "integer")],
        ),
    ]
}

fn plan(declared: &str) -> NestedCompare {
    match comparison_for(declared, None, &types(), &[]) {
        ComparisonPlan::Nested(tree) => tree,
        other => panic!("{declared} is no nested plan: {other:?}"),
    }
}

/// Each declared type, fields of it as `*_out` writes them, and literals
/// to order them against.
fn cases() -> Vec<(&'static str, Vec<&'static str>, Vec<&'static str>)> {
    vec![
        (
            "integer[]",
            vec![
                "{}",
                "{1,2,3}",
                "{{1,2},{3,4}}",
                "[0:1]={1,2}",
                "[2:3][1:2]={{1,2},{3,4}}",
                "{1,NULL,3}",
                "{-5,70000}",
            ],
            vec!["{}", "{1,2,3}", "{1,2}", "{1,2,3,4}", "{{1,2},{3,4}}", "{1,NULL}", "{0}"],
        ),
        (
            "text[]",
            vec![
                r#"{a,"b c","d\"e","f\\g",NULL,"NULL",""}"#,
                "{a,b}",
                "{{x,y},{\"z,w\",\"\"}}",
                "{é,\"tab\there\"}",
            ],
            vec!["{a,b}", "{a}", "{a,\"b c\"}", "{z}", "{\"\"}", "{NULL}"],
        ),
        ("character(3)[]", vec!["{\"ab \",abc}", "{abcd}", "{NULL}"], vec!["{ab,abc}", "{abd}"]),
        ("bytea[]", vec![r#"{"\\x00ff","\\x"}"#, r#"{"\\xzz"}"#], vec![r#"{"\\x00"}"#]),
        (
            "public.pair",
            vec!["(1,a)", "(,)", r#"(1,"x""y")"#, r#"(2,"a\\b")"#, "(1,a,b)", "(1)"],
            vec!["(1,a)", "(1,b)", "(,)", "(0,)"],
        ),
        (
            "public.pair[]",
            vec![r#"{"(1,a)","(2,\"x\"\"y\")",NULL}"#, r#"{"(1,a)"}"#, r#"{"(1)"}"#],
            vec![r#"{"(1,a)"}"#, r#"{"(1,a)","(2,b)"}"#],
        ),
        ("public.nothing", vec!["()", "(,)", "(x)"], vec!["()"]),
        ("public.nnpair", vec!["(1,2)", "(,2)", "(1,)"], vec!["(1,2)", "(1,3)"]),
        ("public.nn[]", vec!["{1,NULL}", "{1,2}"], vec!["{1,2}", "{1}"]),
        (
            "int4range",
            vec!["[1,5)", "empty", "(,3)", "[2,)", "(,)", "[3,1)", "(1,2)", "[1,2147483647]"],
            vec!["[1,5)", "empty", "(,3)", "[1,6)", "(,)"],
        ),
        (
            "tsrange",
            vec![r#"["2020-01-01 00:00:00","2020-01-02 00:00:00")"#, "empty"],
            vec![r#"["2020-01-01 00:00:00","2020-01-03 00:00:00")"#],
        ),
        ("numrange", vec!["[1.5,2.5]", "(,0)"], vec!["[1.5,2.5)", "(,)"]),
        (
            "int4range[]",
            vec![r#"{"[1,2)","(,3)",empty}"#, "{}", r#"{"[1,3)"}"#],
            vec![r#"{"[1,2)"}"#, "{empty}"],
        ),
        ("int2vector", vec!["1 2 3", "", "-1", "1  2", "01", "40000"], vec!["1 2 3", "1 2", ""]),
        (
            "int4multirange",
            vec![
                "{[1,3),[5,7)}",
                "{}",
                "{[3,1)}",
                "{[5,7),[1,3)}",
                "{[1,3),[3,5)}",
                "{[10,12),[2,6),[1,4)}",
                "{[1,1),[2,3]}",
                "{(,3),[2,)}",
                "{[1,2147483647]}",
                "{(0,1),(4,9]}",
            ],
            vec!["{[1,3)}", "{}", "{[1,3),[5,7)}", "{[1,6),[10,12)}", "{(,)}", "{[5,10)}"],
        ),
        (
            "nummultirange",
            vec!["{[1.5,2.5],(3,4)}", "{(,0),[0,1)}", "{[2,3),[1,2.5)}", "{(3,4),[1.5,2.5]}"],
            vec!["{[1.5,4)}", "{(,1)}", "{[1.5,2.5],(3,4)}"],
        ),
        (
            "datemultirange",
            vec![
                "{[2020-01-01,2020-01-05),[2020-01-03,infinity]}",
                "{[2020-01-02,2020-01-01)}",
                "{[2020-01-05,2020-01-06),[2020-01-01,2020-01-04]}",
            ],
            vec!["{[2020-01-01,infinity]}", "{[2020-01-01,2020-01-06)}"],
        ),
        (
            "jsonb[]",
            vec![r#"{"{\"b\": 1, \"a\": [1, 2]}",NULL,3}"#, r#"{"{}","[]"}"#, "{1,true}"],
            vec![r#"{"{\"a\": [1, 2], \"b\": 1}"}"#, "{1}", r#"{"{}"}"#],
        ),
    ]
}

/// The bytes a byte-wise damage inserts, each a byte some grammar branches
/// on.
const INSERTED: &[u8] = b"{}[]()\",\\ :=-1aNe";

/// Every field `seed` holds damaged once: a byte deleted, or one inserted,
/// at each place.
fn damaged(seed: &str) -> Vec<String> {
    let bytes = seed.as_bytes();
    let mut out = vec![seed.to_string()];
    for at in 0..=bytes.len() {
        let (head, tail) = bytes.split_at(at);
        if let Some((_, rest)) = tail.split_first() {
            out.extend(String::from_utf8([head, rest].concat()).ok());
        }
        for &byte in INSERTED {
            out.extend(String::from_utf8([head, &[byte], tail].concat()).ok());
        }
    }
    out
}

#[test]
fn the_walk_orders_a_field_as_its_key_does() {
    let mut read = 0;
    for (declared, seeds, literals) in cases() {
        let plan = plan(declared);
        let bounds: Vec<NestedKey> = literals
            .iter()
            .map(|literal| {
                nested_key(&plan, literal, Side::Literal)
                    .unwrap_or_else(|| panic!("{declared} {literal:?} is no literal"))
            })
            .collect();
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Ignore] {
            for field in seeds.iter().flat_map(|seed| damaged(seed)) {
                let key = nested_key(&plan, &field, Side::Field(invalid));
                read += usize::from(key.is_some());
                let escaped = escape_field(Some(&field));
                for bound in &bounds {
                    let expected = key.as_ref().map(|key| compare_nested(key, bound));
                    let walked =
                        nested_field_order(&plan, FieldText::plain(&field), bound, invalid);
                    assert_eq!(walked, expected, "{declared} {field:?} against {bound:?}");
                    let walked =
                        nested_field_order(&plan, FieldText::escaped(&escaped), bound, invalid);
                    assert_eq!(
                        walked, expected,
                        "{declared} {escaped:?} escaped, against {bound:?}"
                    );
                }
            }
        }
    }
    // A corpus no field of which reads would agree vacuously.
    assert!(read > 300, "{read} fields read");
}
