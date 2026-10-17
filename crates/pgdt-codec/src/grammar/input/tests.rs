use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;
use crate::compare::ComparisonPlan;
use crate::compare::NestedCompare;
use crate::compare::comparison_for;

fn ty(name: &str, kind: TypeKind) -> TypeDef {
    TypeDef { name: name.to_string(), kind }
}

/// **A bit string's grammar is read off every spelling of its type**,
/// with the length its typmod hands `bit_in` or `varbit_in` (I73): `bit`
/// alone is `bit(1)` and `"bit"` has none (I8), `varbit` is `bit varying`,
/// and a domain hands down its base type's. An array's elements, and any
/// other type, have none here; a nested position carries its own.
#[test]
fn a_bit_string_s_grammar_is_read_off_every_spelling() {
    let bit = |length| Some(TextGrammar::Bit { varying: false, length });
    let varbit = |length| Some(TextGrammar::Bit { varying: true, length });
    let types = [
        ty("public.b3", TypeKind::domain("bit(3)")),
        ty("public.b3b", TypeKind::domain("public.b3")),
        ty("public.flags", TypeKind::Composite { fields: Some(vec![ColumnDef::new("f", "bit")]) }),
    ];
    for (declared, expected) in [
        ("bit", bit(Some(1))),
        ("BIT", bit(Some(1))),
        ("bit(3)", bit(Some(3))),
        ("\"bit\"", bit(None)),
        ("\"bit\"(5)", bit(Some(5))),
        ("bit varying", varbit(None)),
        ("BIT  VARYING(4)", varbit(Some(4))),
        ("varbit", varbit(None)),
        ("\"varbit\"(2)", varbit(Some(2))),
        ("public.b3", bit(Some(3))),
        ("public.b3b", bit(Some(3))),
        ("\"bit varying\"", None),
        ("\"BIT\"", None),
        ("bit(0)", None),
        ("bit(x)", None),
        ("bit(3)[]", None),
        ("public.b3[]", None),
        ("public.flags", None),
        ("text", None),
    ] {
        assert_eq!(text_grammar(declared, &types), expected, "`{declared}`");
    }
    let ComparisonPlan::Nested(NestedCompare::Array { element, .. }) =
        comparison_for("public.b3[]", None, &types, &[])
    else {
        panic!("an array of a domain over `bit(3)` is nested");
    };
    assert!(
        matches!(element.plan, NestedCompare::Uncomparable { grammar, .. } if grammar == bit(Some(3)))
    );
    let ComparisonPlan::Nested(NestedCompare::Record { fields, .. }) =
        comparison_for("public.flags", None, &types, &[])
    else {
        panic!("a composite is nested");
    };
    assert!(
        matches!(&fields[0].1.plan, NestedCompare::Uncomparable { grammar, .. } if *grammar == bit(Some(1)))
    );
}

/// **A geometric type's grammar is read off every spelling of it** (I74),
/// through a domain, and none off one written with a typmod, which no
/// geometric type takes. An array of `box` has a grammar of its own, its
/// elements split at `;` (I22); an array of any other geometric type is
/// read through its nested plan, whose element carries the grammar, as
/// does a `box` element a container holds.
#[test]
fn a_geometric_type_s_grammar_is_read_off_every_spelling() {
    let types = [
        ty("public.boxes", TypeKind::domain("box")),
        ty("public.boxlist", TypeKind::domain("box[]")),
        ty(
            "public.shapes",
            TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("b", "box[]"), ColumnDef::new("c", "\"circle\"")]),
            },
        ),
    ];
    let geometric = |g| Some(TextGrammar::Geometric(g));
    for (declared, expected) in [
        ("point", geometric(Geometric::Point)),
        ("LINE", geometric(Geometric::Line)),
        ("lseg", geometric(Geometric::Lseg)),
        ("\"box\"", geometric(Geometric::Box)),
        ("path", geometric(Geometric::Path)),
        ("polygon", geometric(Geometric::Polygon)),
        ("circle", geometric(Geometric::Circle)),
        ("public.boxes", geometric(Geometric::Box)),
        ("box[]", Some(TextGrammar::BoxArray { not_null: false })),
        ("\"box\"[][]", Some(TextGrammar::BoxArray { not_null: false })),
        ("public.boxes[]", Some(TextGrammar::BoxArray { not_null: false })),
        ("public.boxlist", Some(TextGrammar::BoxArray { not_null: false })),
        ("point(2)", None),
        ("\"Point\"", None),
        ("pg_catalog.point", None),
        ("point[]", None),
        ("public.shapes", None),
    ] {
        assert_eq!(text_grammar(declared, &types), expected, "`{declared}`");
    }
    let element_grammar = |declared| match comparison_for(declared, None, &types, &[]) {
        ComparisonPlan::Nested(NestedCompare::Array { element, .. }) => match element.plan {
            NestedCompare::Uncomparable { grammar, .. } => grammar,
            other => panic!("`{declared}`'s element is {other:?}"),
        },
        other => panic!("`{declared}` is {other:?}"),
    };
    assert_eq!(element_grammar("point[]"), geometric(Geometric::Point));
    assert_eq!(element_grammar("box[]"), geometric(Geometric::Box));
    assert_eq!(element_grammar("public.boxes[]"), geometric(Geometric::Box));
    let ComparisonPlan::Nested(NestedCompare::Record { fields, .. }) =
        comparison_for("public.shapes", None, &types, &[])
    else {
        panic!("a composite is nested");
    };
    assert!(matches!(
        &fields[0].1.plan,
        NestedCompare::Array { element, .. } if matches!(element.plan, NestedCompare::Uncomparable {
            grammar: Some(TextGrammar::Geometric(Geometric::Box)), ..
        })
    ));
    assert!(matches!(
        &fields[1].1.plan,
        NestedCompare::Uncomparable { grammar, .. } if *grammar == geometric(Geometric::Circle)
    ));
}

/// **A type whose input function refuses every value, or none, has a
/// grammar saying so**: the internal types I78 names and an enum whose
/// labels are exactly none (I70) refuse every field, and `"char"`,
/// `refcursor`, `xid`, `xid8` and `cid` none (I79) — bare `char` being
/// `character`, a typmod none of them takes no grammar, and a nested
/// position carrying its own.
#[test]
fn a_type_refusing_every_value_or_none_has_a_grammar_saying_so() {
    let types = [
        ty("public.none", TypeKind::exact_enum::<&str>([])),
        ty("public.unread", TypeKind::Enum { labels: vec![], exact: false }),
        ty("public.tree", TypeKind::domain("pg_node_tree")),
    ];
    let all = Some(TextGrammar::RefusesAll);
    let nothing = Some(TextGrammar::RefusesNothing);
    for (declared, expected) in [
        ("pg_node_tree", all),
        ("pg_ndistinct", all),
        ("pg_dependencies", all),
        ("pg_mcv_list", all),
        ("pg_brin_bloom_summary", all),
        ("pg_brin_minmax_multi_summary", all),
        ("\"gtsvector\"", all),
        ("public.tree", all),
        ("public.none", all),
        ("public.unread", None),
        ("\"char\"", nothing),
        ("refcursor", nothing),
        ("xid", nothing),
        ("XID8", nothing),
        ("cid", nothing),
        ("char", None),
        ("xid(4)", None),
        ("xid[]", None),
        ("pg_catalog.xid", None),
    ] {
        assert_eq!(text_grammar(declared, &types), expected, "`{declared}`");
    }
    let ComparisonPlan::Nested(NestedCompare::Array { element, .. }) =
        comparison_for("xid8[]", None, &types, &[])
    else {
        panic!("an array of `xid8` is nested");
    };
    assert!(
        matches!(element.plan, NestedCompare::Uncomparable { grammar, .. } if grammar == nothing)
    );
}
