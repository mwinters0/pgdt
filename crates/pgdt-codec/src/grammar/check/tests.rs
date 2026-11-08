use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;
use crate::compare::comparison_for;
use crate::grammar::walk::{checked_key, grammar_refuses};
use crate::value::json::json_in;

fn ty(name: &str, kind: TypeKind) -> TypeDef {
    TypeDef { name: name.to_string(), kind }
}

/// **What a strict parse leaves unchecked is named at every position,
/// with its reason** (`unchecked_positions`): a type the restoring server
/// decides the input of, at the column or beneath it, a type no reader
/// here reads, a range declaring a canonical function, an enum whose
/// labels are inexact and a domain's `CHECK` anywhere — and nothing for
/// a type a strict parse reads, `json` and a geometric type among them,
/// nor for a `NOT NULL` domain beneath a container, whose NULL it refuses.
#[test]
fn what_a_strict_parse_leaves_unchecked_is_named_where_it_lies() {
    let domain = |base: &str, not_null, check| TypeKind::Domain {
        base_type: base.into(),
        collation: None,
        not_null,
        check,
    };
    let types = [
        ty("public.gtype", TypeKind::base()),
        ty(
            "public.canon",
            TypeKind::Range {
                subtype: Some("integer".into()),
                multirange_type_name: None,
                canonical: Some("public.canon_fn".into()),
            },
        ),
        ty(
            "public.doc",
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("id", "integer"),
                    ColumnDef::new("body", "xml"),
                    ColumnDef::new("n", "public.pos"),
                ]),
            },
        ),
        ty("public.mood", TypeKind::Enum { labels: vec!["ok".into()], exact: false }),
        ty("public.none", TypeKind::exact_enum::<&str>([])),
        ty("public.shell", TypeKind::Shell),
        ty("public.unparsed", TypeKind::Composite { fields: None }),
        ty(
            "public.nosub",
            TypeKind::Range {
                subtype: None,
                multirange_type_name: Some("public.nosubmulti".into()),
                canonical: None,
            },
        ),
        ty("public.ints", TypeKind::domain("integer[]")),
        ty(
            "public.grid",
            TypeKind::Composite { fields: Some(vec![ColumnDef::new("rows", "public.ints[]")]) },
        ),
        ty("public.pos", domain("integer", false, true)),
        ty("public.over_pos", domain("public.pos", false, false)),
        ty("public.nn", domain("integer", true, false)),
        ty(
            "public.posrange",
            TypeKind::Range {
                subtype: Some("public.pos".into()),
                multirange_type_name: Some("public.posmulti".into()),
                canonical: None,
            },
        ),
    ];
    let named = |declared: &str| -> Vec<(String, String, Unchecked)> {
        let plan = comparison_for(declared, None, &types, &[]);
        unchecked_positions(declared, &plan, column_grammar(&plan, declared, &types), &types)
    };
    let at = |path: &str, declared: &str, why| (path.to_string(), declared.to_string(), why);
    let canon = || Unchecked::RangeCanonical { function: "public.canon_fn".into() };
    for (declared, expected) in [
        ("integer", vec![]),
        ("json", vec![]),
        ("json[]", vec![]),
        ("box", vec![]),
        ("box[]", vec![]),
        ("bit(3)", vec![]),
        ("public.nn", vec![]),
        ("xml", vec![at("", "xml", Unchecked::Xml)]),
        ("money", vec![at("", "money", Unchecked::Money)]),
        ("money[]", vec![at("[]", "money", Unchecked::Money)]),
        ("tsvector", vec![at("", "tsvector", Unchecked::NoReader)]),
        ("pg_lsn[]", vec![at("[]", "pg_lsn", Unchecked::NoReader)]),
        ("regclass", vec![at("", "regclass", Unchecked::Catalog)]),
        ("aclitem[]", vec![at("[]", "aclitem", Unchecked::Catalog)]),
        ("\"char\"", vec![]),
        ("xid[]", vec![]),
        ("pg_node_tree", vec![]),
        ("public.none", vec![]),
        ("public.nowhere", vec![at("", "public.nowhere", Unchecked::UndeclaredType)]),
        ("nowhere", vec![at("", "nowhere", Unchecked::UndeclaredType)]),
        ("public.nowhere[]", vec![at("[]", "public.nowhere", Unchecked::UndeclaredType)]),
        ("public.shell", vec![at("", "public.shell", Unchecked::BaseType)]),
        ("public.unparsed", vec![at("", "public.unparsed", Unchecked::Unparsed)]),
        ("public.nosub", vec![at("", "public.nosub", Unchecked::Unparsed)]),
        ("public.nosubmulti", vec![at("", "public.nosubmulti", Unchecked::Unparsed)]),
        ("bit(0)", vec![at("", "bit(0)", Unchecked::Unparsed)]),
        ("public.ints[]", vec![at("[]", "public.ints", Unchecked::ArrayShape)]),
        ("public.grid", vec![at(".rows[]", "public.ints", Unchecked::ArrayShape)]),
        ("public.gtype", vec![at("", "public.gtype", Unchecked::BaseType)]),
        ("public.gtype[]", vec![at("[]", "public.gtype", Unchecked::BaseType)]),
        ("public.canon", vec![at("", "public.canon", canon())]),
        ("public.canon[]", vec![at("", "public.canon", canon())]),
        ("public.mood", vec![at("", "public.mood", Unchecked::LabelsInexact)]),
        ("public.mood[]", vec![at("[]", "public.mood", Unchecked::LabelsInexact)]),
        (
            "public.doc",
            vec![
                at(".body", "xml", Unchecked::Xml),
                at(".n", "public.pos", Unchecked::DomainCheck),
            ],
        ),
        ("public.pos", vec![at("", "public.pos", Unchecked::DomainCheck)]),
        ("public.over_pos", vec![at("", "public.pos", Unchecked::DomainCheck)]),
        ("public.pos[]", vec![at("[]", "public.pos", Unchecked::DomainCheck)]),
        ("public.nn[]", vec![]),
        ("public.posrange", vec![at(".bound", "public.pos", Unchecked::DomainCheck)]),
        ("public.posmulti", vec![at("[].bound", "public.pos", Unchecked::DomainCheck)]),
    ] {
        assert_eq!(named(declared), expected, "{declared}");
    }
}

/// **A strict parse finds a refusal anywhere in a field** ([`field_refused`]):
/// a scalar where [`field_key`] refuses it, and a nested value at its
/// container's input grammar, at any element read as a field of its own
/// type — under a position this build does not order too — and at a
/// range's bound order or a discrete bound's successor (I44, I46, I47). A
/// spelling the input grammar reads and the output grammar never writes is
/// no refusal, nor is any text of a type read as its text but `json`, at
/// the top level or beneath a container, which `json_in`'s grammar reads
/// (I72), and `bit` and `bit varying`, whose grammar is read in place of
/// the order they lack, their typmod with it, through a domain too (I73),
/// as are the geometric types', a `box` array's split at `;` (I74, I22),
/// and a `character varying(n)`'s or `character(n)`'s length (I75).
#[test]
fn a_strict_check_finds_a_refusal_anywhere_in_a_field() {
    let types = vec![
        TypeDef {
            name: "public.pair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("a", "integer"), ColumnDef::new("b", "smallint")]),
            },
        },
        TypeDef {
            name: "public.mood".into(),
            kind: TypeKind::Enum { labels: vec!["sad".into(), "ok".into()], exact: true },
        },
        TypeDef {
            name: "public.jsonpair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("ok", "integer"), ColumnDef::new("doc", "json")]),
            },
        },
        TypeDef { name: "public.bits3".into(), kind: TypeKind::domain("bit(3)") },
        TypeDef {
            name: "public.flags".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("a", "bit"),
                    ColumnDef::new("b", "public.bits3"),
                    ColumnDef::new("c", "bit varying(2)"),
                ]),
            },
        },
        TypeDef { name: "public.ring".into(), kind: TypeKind::domain("circle") },
        TypeDef { name: "public.short".into(), kind: TypeKind::domain("character varying(2)") },
        TypeDef {
            name: "public.shapes".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("b", "box[]"), ColumnDef::new("p", "point")]),
            },
        },
        not_null_domain("public.nn", "integer"),
        not_null_domain("public.nnbox", "box"),
        not_null_domain("public.nnmoney", "money"),
        TypeDef {
            name: "public.nnpair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("a", "public.nn"),
                    ColumnDef::new("b", "integer"),
                ]),
            },
        },
        TypeDef {
            name: "public.nnrange".into(),
            kind: TypeKind::Range {
                subtype: Some("public.nn".into()),
                multirange_type_name: None,
                canonical: None,
            },
        },
    ];
    // As `gather::field_checks` reads a column: a plan ordering nothing
    // by its declared type's grammar, where it has one.
    let refused = |declared: &str, text: &str| {
        let plan = comparison_for(declared, None, &types, &[]);
        match (&plan, text_grammar(declared, &types)) {
            (ComparisonPlan::Refused, Some(grammar)) => grammar_refuses(grammar, text),
            _ => field_refused(&plan, text),
        }
    };
    let cases: [(&str, &str, bool); 98] = [
        ("smallint", "70000", true),
        ("smallint", "7", false),
        ("smallint", " 7", false),
        ("text", "anything", false),
        ("character varying(3)", "abcd", true),
        ("character varying(3)", "abc   ", false),
        ("character(3)", "abc d", true),
        ("character(3)", "ab ", false),
        ("character", "ab", true),
        ("bpchar", "abcd", false),
        ("character varying(2)[]", "{ab,abc}", true),
        ("public.short", "abc", true),
        ("public.short[]", "{ab}", false),
        ("money", "not money", false),
        ("public.mood", "happy", true),
        ("public.mood", "ok", false),
        ("smallint[]", "{1,70000}", true),
        ("smallint[]", "{{1},{70000}}", true),
        ("smallint[]", "{1,2", true),
        ("smallint[]", "{ 1 , 2 }", false),
        ("smallint[]", "{1,NULL}", false),
        ("public.mood[]", "{ok,happy}", true),
        ("money[]", "{1", true),
        ("money[]", "{$1.00}", false),
        ("public.pair", "(1,70000)", true),
        ("public.pair", "(1)", true),
        ("public.pair", "(1,2)", false),
        ("public.pair[]", "{\"(1,2)\",\"(1,70000)\"}", true),
        ("int4range", "[5,1]", true),
        ("int4range", "[1,5)", false),
        ("int4range", "[1,2147483647]", true),
        ("int4range", "[1,70000000000)", true),
        ("int4multirange", "{[1,2), [5,1]}", true),
        ("int4multirange", "{[1,2), [5,6)}", false),
        ("int2vector", "1 70000", true),
        ("json", "{bad", true),
        ("json", "[1,]", true),
        ("json", r#""\u0000""#, false),
        ("json", r#" [1, {"a": null}] "#, false),
        ("json[]", r#"{1,"[1, 2]"}"#, false),
        ("json[]", "{1,x}", true),
        ("json[]", "{1,NULL}", false),
        ("public.jsonpair", r#"(1,"[1, 2]")"#, false),
        ("public.jsonpair", "(1,{bad)", true),
        ("public.jsonpair[]", r#"{"(1,{})","(2,[)"}"#, true),
        ("bit", "1", false),
        ("bit", "10", true),
        ("\"bit\"", "10", false),
        ("bit(3)", "x5", true),
        ("bit(4)", "x5", false),
        ("bit varying(2)", "111", true),
        ("varbit", "1012", true),
        ("public.bits3", "101", false),
        ("public.bits3", "10", true),
        ("bit(3)[]", "{101,10}", true),
        ("bit(3)[]", "{101,NULL}", false),
        ("varbit(2)[]", "{1,11}", false),
        ("public.bits3[]", "{1}", true),
        ("public.flags", "(1,101,11)", false),
        ("public.flags", "(11,101,11)", true),
        ("public.flags", "(1,10,11)", true),
        ("public.flags", "(1,101,111)", true),
        ("point", "(1,2)", false),
        ("point", "(1,2", true),
        ("line", "{0,0,1}", true),
        ("line", "[(2,0),(3,1.7976931348623157e308)]", false),
        ("lseg", "[(1,2),(3,4))", false),
        ("box", "[(1,2),(3,4)]", true),
        ("path", "(1,2),(3,4),", true),
        ("polygon", "(1,2)", false),
        ("circle", "<(1,2),-1>", true),
        ("public.ring", "<(1,2),1e400>", true),
        ("point[]", r#"{"(1,2)","(3,x)"}"#, true),
        ("box[]", "{(1,1),(0,0);(2,2),(1,1)}", false),
        ("box[]", r#"{"(1,1),(0,0)";(2,2),(1,1)}"#, false),
        ("box[]", "{(1,1),(0,0);(2,2)}", true),
        ("public.shapes", r#"("{(1,1),(0,0);(2,2),(1,1)}","(1,2)")"#, false),
        ("public.shapes", r#"("{""(1,1),(0,0)"";(2,2),(1,1)}","(1,2)")"#, false),
        ("public.shapes", r#"("{(1,1),(0,0);(2,2)}","(1,2)")"#, true),
        ("public.shapes", r#"("{(1,1),(0,0)}",(1))"#, true),
        ("pg_node_tree", "{QUERY}", true),
        ("pg_mcv_list[]", "{NULL}", false),
        ("pg_mcv_list[]", "{x}", true),
        ("\"char\"", "anything", false),
        ("refcursor", "", false),
        ("xid", "abc", false),
        ("cid", "4294967296", false),
        ("xid8[]", "{1,abc}", false),
        ("xid8[]", "{1,abc", true),
        ("public.nn[]", "{1,NULL}", true),
        ("public.nn[]", "{{1,2},{3,NULL}}", true),
        ("public.nn[]", "{1,2}", false),
        ("public.nnpair", "(,2)", true),
        ("public.nnpair", "(1,)", false),
        ("public.nnpair[]", r#"{"(,2)"}"#, true),
        ("public.nnpair[]", r#"{"(1,)",NULL}"#, false),
        ("public.nnmoney[]", "{NULL}", true),
        ("public.nnrange", "(,3)", false),
    ];
    for (declared, text, expected) in cases {
        assert_eq!(refused(declared, text), expected, "{declared} {text:?}");
    }
    // A `box[]` column resolves to text with its plan refused (I22), and is
    // read by its own grammar.
    for (text, expected) in [
        ("{(1,1),(0,0);(2,2),(1,1)}", false),
        (r#"{"(1,1),(0,0)";(2,2),(1,1)}"#, false),
        ("{{(1,1),(0,0)};{(2,2),(1,1)}}", false),
        ("{NULL;(1,1),(0,0)}", false),
        ("{(1,1),(0,0);(2,2)}", true),
        ("{(1,1),(0,0),(2,2),(1,1)}", true),
        ("{(1,1),(0,0)", true),
    ] {
        assert_eq!(
            grammar_refuses(TextGrammar::BoxArray { not_null: false }, text),
            expected,
            "{text:?}"
        );
    }
    assert!(refused("public.nnbox[]", "{NULL;(1,1),(0,0)}"));
    assert!(!refused("public.nnbox[]", "{(1,1),(0,0)}"));
}

/// A domain over `base` declaring `NOT NULL`.
fn not_null_domain(name: &str, base: &str) -> TypeDef {
    TypeDef {
        name: name.into(),
        kind: TypeKind::Domain {
            base_type: base.into(),
            collation: None,
            not_null: true,
            check: false,
        },
    }
}

/// **A base type's array beneath a container is split at the `DELIMITER`
/// its `CREATE TYPE` declares** (I22), through a domain and a typmod, so a
/// quoted element followed by it is read, as `array_in` reads it, and one
/// followed by `,` refused; a delimiter the preamble could not read, or one
/// `array_in` reads as syntax at some major, splits nothing and refuses
/// nothing but the container's own grammar.
#[test]
fn a_base_type_s_array_beneath_a_container_is_split_at_its_delimiter() {
    let base =
        |name: &str, delimiter| TypeDef { name: name.into(), kind: TypeKind::Base { delimiter } };
    let mut types = vec![
        base("public.semi", Some(b';')),
        base("public.pipe", Some(b'|')),
        base("public.plain", Some(b',')),
        base("public.unread", None),
        base("public.backslash", Some(b'\\')),
        TypeDef { name: "public.semid".into(), kind: TypeKind::domain("public.semi") },
    ];
    for (field, text, expected) in [
        ("public.semi[]", r#"("{""a b"";c}")"#, false),
        ("public.semi[]", r#"("{a,b;c}")"#, false),
        ("public.semi[]", r#"("{{a;b};{c;d}}")"#, false),
        ("public.semi[]", r#"("{{a;b};{c}}")"#, true),
        ("public.semi[]", r#"("{""a b"",c}")"#, true),
        ("public.semi(8)[]", r#"("{""a b"";c}")"#, false),
        ("public.semid[]", r#"("{""a b"";c}")"#, false),
        ("public.semid[]", r#"("{""a b"",c}")"#, true),
        ("public.pipe[]", r#"("{""a b""|c}")"#, false),
        ("public.plain[]", r#"("{""a b"",c}")"#, false),
        ("public.plain[]", r#"("{""a b"";c}")"#, true),
        ("public.unread[]", r#"("{""a b"";c}")"#, false),
        ("public.unread[]", r#"("{""a b"",c}")"#, false),
        ("public.unread[]", r#"("{a}""#, true),
        ("public.backslash[]", r#"("{""a b""\\c}")"#, false),
    ] {
        types.retain(|t| t.name != "public.holder");
        types.push(TypeDef {
            name: "public.holder".into(),
            kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("v", field)]) },
        });
        let plan = comparison_for("public.holder", None, &types, &[]);
        assert_eq!(field_refused(&plan, text), expected, "{field} {text:?}");
    }
}

/// **A walked check refuses exactly what a check over the keys refused**
/// (`refused`, `checked_key`), over nested literals built from each
/// container's grammar with leaves read and refused by their types, and a
/// field walked as `COPY` wrote it — every byte escaped some way — refused
/// exactly where its unescaped text is.
#[test]
fn a_walked_check_refuses_what_the_keys_refused() {
    let types = vec![
        TypeDef {
            name: "public.pair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("a", "smallint"),
                    ColumnDef::new("b", "character varying(2)"),
                    ColumnDef::new("c", "public.nn"),
                ]),
            },
        },
        TypeDef {
            name: "public.deep".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("p", "public.pair[]"),
                    ColumnDef::new("j", "json"),
                    ColumnDef::new("b", "jsonb"),
                    ColumnDef::new("r", "int4range"),
                ]),
            },
        },
        not_null_domain("public.nn", "integer"),
    ];
    let declared = [
        "smallint[]",
        "character varying(2)[]",
        "public.nn[]",
        "public.pair",
        "public.pair[]",
        "public.deep",
        "public.deep[]",
        "int4range",
        "int4range[]",
        "int4multirange",
        "int2vector",
        "json[]",
        "jsonb[]",
        "bytea[]",
        "text[]",
        "box[]",
        "point[]",
    ];
    let mut rng = Rng(0x5eed_0004);
    let (mut refusals, mut reads) = (0, 0);
    for _ in 0..20_000 {
        let text = literal(&mut rng, 2);
        for declared in declared {
            let plan = comparison_for(declared, None, &types, &[]);
            let ComparisonPlan::Nested(nested) = &plan else { continue };
            let keyed = checked_key(nested, &text).is_err();
            assert_eq!(field_refused(&plan, &text), keyed, "{declared} {text:?}");
            refusals += usize::from(keyed);
            reads += usize::from(!keyed);
            let escaped = pgdt_core::escape_field(Some(&text));
            assert_eq!(walked_in_runs(&plan, &escaped), keyed, "{declared} {escaped:?}");
        }
    }
    // Both answers are reached, at every depth.
    assert!(refusals > 10_000 && reads > 10_000, "{refusals} refusals, {reads} reads");
    // Every escape `COPY` reads, octal and hex among them, reads as its byte.
    let plan = comparison_for("smallint[]", None, &types, &[]);
    for (field, refused) in [
        (&b"{1,\\0627}"[..], false),
        (b"{1,\\x32}", false),
        (b"{1,\\x37\\x30000}", true),
        (b"\\{1,2\\}", false),
        (b"{1\\t,2}", false),
        (b"{1\\,2}", false),
        (b"{1\\\\,2}", true),
    ] {
        assert_eq!(walked_in_runs(&plan, field), refused, "{field:?}");
    }
}

/// **A scalar walked off its bytes is refused as its text is**, whatever
/// runs `COPY`'s bytes for it are fed in: a `bytea` in either form, read once
/// for whether it decodes and whether `byteain` refuses it; a text held to
/// its typmod, a character split across runs among them; a `json` and a
/// `jsonb`, against the document `parse_jsonb` reads.
#[test]
fn a_walked_scalar_is_refused_as_its_text_is() {
    let bytea = comparison_for("bytea", None, &[], &[]);
    for text in [
        "\\x",
        "\\xdead",
        "\\xDeAd",
        "\\xabc",
        "\\xgg",
        "\\x ab",
        "\\xab cd",
        "\\xa b",
        "\\X41",
        "abc",
        "\\000\\\\\\177A",
        "\\101",
        "\\400",
        "\\08",
        "a\\",
        "\\",
        "",
        "tab\there",
        "é",
    ] {
        let refused = matches!(field_key(&CompareKind::Bytea, text), Err(Unread::Refused));
        let escaped = pgdt_core::escape_field(Some(text));
        assert_eq!(walked_in_runs(&bytea, &escaped), refused, "bytea {text:?}");
    }
    for (declared, length) in [("character varying(3)", 3), ("character(2)", 2)] {
        let plan = comparison_for(declared, None, &[], &[]);
        for text in ["", "abc", "abcd", "ab  ", "abc ", "éé", "ééé", "éééé", "a\tb", "ab\nc\n"]
        {
            let refused = char_typmod_refuses(text, length);
            let escaped = pgdt_core::escape_field(Some(text));
            assert_eq!(walked_in_runs(&plan, &escaped), refused, "{declared} {text:?}");
        }
    }
    let (json, jsonb) =
        (comparison_for("json", None, &[], &[]), comparison_for("jsonb", None, &[], &[]));
    for text in [
        "1",
        "-0",
        "01",
        "1.",
        "1e5",
        "1e99999999999",
        "[1, 2]",
        "{\"a\": [true, null]}",
        "\"a\\nb\"",
        "\"\\u0000\"",
        "\"\\ud800\\udc00\"",
        "\"\\ud800x\"",
        "\"é\"",
        "\"\t\"",
        "[1,]",
        "tru",
        " {} ",
    ] {
        let escaped = pgdt_core::escape_field(Some(text));
        assert_eq!(walked_in_runs(&json, &escaped), !json_in(text), "json {text:?}");
        let refused = crate::value::json::parse_jsonb(text) == Err(Unread::Refused);
        assert_eq!(walked_in_runs(&jsonb, &escaped), refused, "jsonb {text:?}");
    }
}

/// **A field read by a grammar is answered as its text is, whatever runs it
/// is fed in**: each geometric type's and `bit varying`'s, and a `box[]`'s
/// unquoted element, a whitespace run inside it fed on to `box_in` and one
/// after it rewound.
#[test]
fn a_field_read_by_a_grammar_is_refused_as_its_text_is() {
    use crate::grammar::input::Geometric;
    let geometric = [
        Geometric::Point,
        Geometric::Line,
        Geometric::Lseg,
        Geometric::Box,
        Geometric::Path,
        Geometric::Polygon,
        Geometric::Circle,
    ];
    let texts = [
        "(1,2)",
        " ( 1 , 2 ) ",
        "1,2,3,4",
        "(1,2),(3,4)",
        "((1,2),(3,4))",
        "[(1,2),(3,4)]",
        "[(0,0),(1,1),(2,0)]",
        "<(1,2),3>",
        "{1,2,3}",
        "(nan(a),1),(0x1p3,1e-310)",
        "(1e400,2)",
        "(1e,2)",
        "(1,2) x",
        "10110",
        "x1f",
        "",
    ];
    for text in texts {
        let escaped = pgdt_core::escape_field(Some(text));
        for kind in geometric {
            let grammar = TextGrammar::Geometric(kind);
            let refused = fed_in_runs(&|| FieldWalk::grammar(grammar), &escaped);
            assert_eq!(refused, grammar_refuses(grammar, text), "{kind:?} {text:?}");
        }
        let grammar = TextGrammar::Bit { varying: true, length: Some(5) };
        let refused = fed_in_runs(&|| FieldWalk::grammar(grammar), &escaped);
        assert_eq!(refused, grammar_refuses(grammar, text), "bit varying(5) {text:?}");
    }
    for (text, refused) in [
        ("{(1,1), (0,0)\t;(2,2) ,(1,1)}", false),
        ("{(1,1),(0,0) \t ;  (2,2),(1,1)\u{b}}", false),
        ("{(1,1),(0,0) (2,2);(2,2),(1,1)}", true),
        ("{(1,1),(0,0)  x}", true),
        ("{NULL \t;(1,1),(0,0)}", false),
    ] {
        let grammar = TextGrammar::BoxArray { not_null: false };
        let escaped = pgdt_core::escape_field(Some(text));
        assert_eq!(fed_in_runs(&|| FieldWalk::grammar(grammar), &escaped), refused, "{text:?}");
    }
}

/// **A refusal met in an unquoted element's whitespace run stands only
/// where the run turns out interior**: `array_in` trims a `\v` and `\f`
/// that `json_in` refuses, so `{1\v}` is a `json[]` and `{1\v2}` is not,
/// and a run after `NULL` leaves it the marker, refused letters and all,
/// only where a separator or a brace follows it — a composite's field
/// inside the element among them, its run kept by `record_in`.
#[test]
fn a_refusal_in_a_whitespace_run_stands_only_where_it_is_interior() {
    let types = vec![
        not_null_domain("public.nn", "integer"),
        TypeDef {
            name: "public.pair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("a", "smallint"),
                    ColumnDef::new("b", "character varying(2)"),
                ]),
            },
        },
    ];
    let json = comparison_for("json[]", None, &types, &[]);
    let nn = comparison_for("public.nn[]", None, &types, &[]);
    let pair = comparison_for("public.pair[]", None, &types, &[]);
    for (plan, text, refused) in [
        (&json, "{1\u{b}}", false),
        (&json, "{1 \u{c}\t,2\u{b}}", false),
        (&json, "{1\u{b}2}", true),
        (&json, "{1 \u{b}\\n}", true),
        (&json, "{NULL\u{b}}", false),
        (&json, "{NULL\u{b} ,[]}", false),
        (&json, "{NULL\u{b}x}", true),
        (&json, "{nULl \t}", false),
        (&json, "{null \t}", false),
        (&nn, "{1 \t,NULL\t }", true),
        (&nn, "{1 \t,2\t }", false),
        (&nn, "{1 \t2}", true),
        (&pair, "{(1\\,ab)\t \u{b}}", false),
        (&pair, "{(1\\,a \t)}", true),
        (&pair, "{(1\\,a\u{b}) \t,(2\\, )\u{c}}", false),
        (&pair, "{(1\\,a \t) \t}", true),
    ] {
        let escaped = pgdt_core::escape_field(Some(text));
        assert_eq!(walked_in_runs(plan, &escaped), refused, "{text:?}");
        assert_eq!(field_refused(plan, text), refused, "{text:?}");
        let ComparisonPlan::Nested(nested) = plan else { unreachable!("an array's plan") };
        assert_eq!(checked_key(nested, text).is_err(), refused, "{text:?}");
    }
}

/// **A field walked as `COPY` wrote it, `escaped`, is answered alike
/// whatever runs it is fed in** (`FieldWalk`): whole, in runs of one, two
/// and three bytes, and cut in two at every byte, each a run's end inside an
/// escape, a quote, a `NULL`, a whitespace run or a part.
fn walked_in_runs(plan: &ComparisonPlan, escaped: &[u8]) -> bool {
    fed_in_runs(&|| FieldWalk::new(plan), escaped)
}

/// [`walked_in_runs`] for a walk `new` makes.
fn fed_in_runs<'p>(new: &dyn Fn() -> FieldWalk<'p>, escaped: &[u8]) -> bool {
    let feed = |runs: &mut dyn Iterator<Item = &[u8]>| {
        let mut walk = new();
        for run in runs {
            walk.run(run);
        }
        walk.finish()
    };
    let whole = feed(&mut std::iter::once(escaped));
    for width in 1..=3 {
        assert_eq!(feed(&mut escaped.chunks(width)), whole, "{escaped:?} in runs of {width}");
    }
    for cut in 0..=escaped.len() {
        let (head, tail) = escaped.split_at(cut);
        assert_eq!(feed(&mut [head, tail].into_iter()), whole, "{escaped:?} cut at {cut}");
    }
    whole
}

/// A deterministic 64-bit LCG, so a failure is reproducible from the seed.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) % n as u64) as usize
    }
}

/// The leaves [`literal`] builds from: values some leaf type reads and
/// others refuse.
const LEAVES: &[&str] = &[
    "1",
    "70000",
    "-7",
    "ab",
    "abc",
    "",
    "NULL",
    "x",
    "\\\\x0f",
    "\\\\x0",
    "[1,2]",
    "[1,",
    "{}",
    "(1,2)",
    "(1,2),(3,4)",
    "\\\"a\\\"",
    "\"\\u0000\"",
    "é",
    "1 ",
    " 1",
    "1\u{b}",
    "NULL \t",
    "a \t\u{c}b",
];

/// A nested literal `depth` levels deep at most, each level an array split
/// at `,` or `;`, a record or a range, each part quoted or bare.
fn literal(rng: &mut Rng, depth: usize) -> String {
    if depth == 0 || rng.below(4) == 0 {
        return LEAVES[rng.below(LEAVES.len())].to_string();
    }
    let parts: Vec<String> = (0..rng.below(4))
        .map(|_| {
            let part = literal(rng, depth - 1);
            if rng.below(2) == 0 {
                format!("\"{}\"", part.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                part
            }
        })
        .collect();
    match rng.below(4) {
        0 => format!("({})", parts.join(",")),
        1 => format!("[{})", parts.join(",")),
        2 => format!("{{{}}}", parts.join(";")),
        _ => format!("{{{}}}", parts.join(",")),
    }
}
