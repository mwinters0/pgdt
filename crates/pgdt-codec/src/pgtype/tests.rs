use arrow_schema::TimeUnit;

use super::strict::{column_grammar, unchecked_positions};
use super::*;
use pgdt_model::ColumnDef;

fn ty(name: &str, kind: TypeKind) -> TypeDef {
    TypeDef { name: name.to_string(), kind }
}

#[test]
fn maps_simple_builtins() {
    assert_eq!(
        resolve_declared_type("integer", &[]),
        TypeOutcome::Mapped(DataType::Int32, NestedPlan::Scalar)
    );
    assert_eq!(
        resolve_declared_type("boolean", &[]),
        TypeOutcome::Mapped(DataType::Boolean, NestedPlan::Scalar)
    );
    assert_eq!(
        resolve_declared_type("bytea", &[]),
        TypeOutcome::Mapped(DataType::Binary, NestedPlan::Scalar)
    );
    assert_eq!(
        resolve_declared_type("uuid", &[]),
        TypeOutcome::Mapped(DataType::FixedSizeBinary(16), NestedPlan::Scalar)
    );
}

/// `oid` is PostgreSQL's one unsigned integer type, and the width is the
/// point: `oidout` writes `%u`, where the ADBC driver's `Int32` turns
/// every OID at or above 2^31 negative.
#[test]
fn oid_is_unsigned() {
    assert_eq!(
        resolve_declared_type("oid", &[]),
        TypeOutcome::Mapped(DataType::UInt32, NestedPlan::Scalar)
    );
}

#[test]
fn maps_case_insensitively_for_the_binary_upgrade_dummy_column_shape() {
    // `INTEGER /* dummy */` -> pgdt-dump's DDL grammar already strips the comment,
    // leaving bare uppercase "INTEGER" (I5).
    assert_eq!(
        resolve_declared_type("INTEGER", &[]),
        TypeOutcome::Mapped(DataType::Int32, NestedPlan::Scalar)
    );
}

#[test]
fn text_like_types_map_to_utf8view_deliberately() {
    for (declared, plan) in [
        ("text", NestedPlan::Scalar),
        ("character varying(16)", NestedPlan::Text { length: 16 }),
        ("character varying", NestedPlan::Scalar),
        ("character(10)", NestedPlan::Text { length: 10 }),
        ("character", NestedPlan::Text { length: 1 }),
        ("bpchar", NestedPlan::Scalar),
        ("name", NestedPlan::Scalar),
    ] {
        assert_eq!(
            resolve_declared_type(declared, &[]),
            TypeOutcome::Mapped(DataType::Utf8View, plan),
            "{declared}"
        );
    }
}

#[test]
fn timestamps_are_always_microsecond_regardless_of_typmod() {
    assert_eq!(
        resolve_declared_type("timestamp without time zone", &[]),
        TypeOutcome::Mapped(
            DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
            NestedPlan::Scalar
        )
    );
    assert_eq!(
        resolve_declared_type("timestamp with time zone", &[]),
        TypeOutcome::Mapped(
            DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some("UTC".into())),
            NestedPlan::Scalar
        )
    );
    assert_eq!(
        resolve_declared_type("time with time zone", &[]),
        TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
    );
    // The typmod `format_type` writes mid-name, before the zone.
    for (declared, bare) in [
        ("timestamp(3) with time zone", "timestamp with time zone"),
        ("timestamp(0) without time zone", "timestamp without time zone"),
        ("time(2) with time zone", "time with time zone"),
        ("time(6) without time zone", "time without time zone"),
    ] {
        assert_eq!(resolve_declared_type(declared, &[]), resolve_declared_type(bare, &[]));
    }
}

/// **A declared type is read as PostgreSQL's grammar reads it**: every
/// spelling on the left names the type on the right, a catalog name or a
/// grammar keyword alike; an `interval`'s field qualifier is
/// `an_interval_s_field_qualifier_reaches_its_comparison_kind`'s.
#[test]
fn every_spelling_of_a_built_in_is_that_built_in() {
    for (declared, canonical) in [
        ("int2", "smallint"),
        ("int4", "integer"),
        ("INT", "integer"),
        ("int8", "bigint"),
        ("float4", "real"),
        ("float(24)", "real"),
        ("float8", "double precision"),
        ("float", "double precision"),
        ("float(25)", "double precision"),
        ("bool", "boolean"),
        ("bpchar(5)", "character(5)"),
        ("char(5)", "character(5)"),
        ("char", "character(1)"),
        ("character", "character(1)"),
        ("\"bpchar\"", "bpchar"),
        ("national character(5)", "character(5)"),
        ("varchar(16)", "character varying(16)"),
        ("char  varying", "character varying"),
        ("decimal(10,2)", "numeric(10,2)"),
        ("dec", "numeric"),
        ("timestamp", "timestamp without time zone"),
        ("timestamptz", "timestamp with time zone"),
        ("TIMESTAMP(3)", "timestamp without time zone"),
        ("time", "time without time zone"),
        ("timetz", "time with time zone"),
        ("INTERVAL(3)", "interval"),
        ("\"interval\"", "interval"),
        ("\"int4\"", "integer"),
        ("\"timestamptz\"", "timestamp with time zone"),
    ] {
        let got = resolve_declared_type(declared, &[]);
        assert_ne!(got, TypeOutcome::Unknown, "`{declared}`");
        assert_eq!(got, resolve_declared_type(canonical, &[]), "`{declared}`");
        assert_eq!(
            comparison_for(declared, None, &[], &[]),
            comparison_for(canonical, None, &[], &[]),
            "`{declared}`"
        );
    }
}

/// **An `interval`'s field qualifier reaches its comparison kind**, read
/// as `opt_interval` reads it: each of the thirteen `format_type` writes,
/// with `second`'s precision or without, in any case and spacing, and
/// through an array and a domain; quoted, as the mask `intervaltypmodin`
/// takes. A precision alone is no qualifier, and words `opt_interval` does
/// not read name no built-in.
#[test]
fn an_interval_s_field_qualifier_reaches_its_comparison_kind() {
    use IntervalQualifier as Q;
    let kind = |declared: &str, types: &[TypeDef]| match comparison_for(declared, None, types, &[])
    {
        ComparisonPlan::Compared { kind, .. } => kind,
        ComparisonPlan::Nested(NestedCompare::Array(leaf)) => match leaf.plan {
            NestedCompare::Leaf { kind, .. } => kind,
            other => panic!("`{declared}`: {other:?}"),
        },
        other => panic!("`{declared}`: {other:?}"),
    };
    let interval = |qualifier| CompareKind::Interval { qualifier };
    let types = [ty("public.span", TypeKind::domain("interval minute to second(2)"))];
    for (declared, qualifier) in [
        ("interval", None),
        ("interval(3)", None),
        ("interval year", Some(Q::Year)),
        ("interval month", Some(Q::Month)),
        ("interval day", Some(Q::Day)),
        ("interval hour", Some(Q::Hour)),
        ("interval minute", Some(Q::Minute)),
        ("interval second", Some(Q::Second)),
        ("interval second(3)", Some(Q::Second)),
        ("interval year to month", Some(Q::YearToMonth)),
        ("interval day to hour", Some(Q::DayToHour)),
        ("interval day to minute", Some(Q::DayToMinute)),
        ("interval day to second", Some(Q::DayToSecond)),
        ("interval day to second(0)", Some(Q::DayToSecond)),
        ("interval hour to minute", Some(Q::HourToMinute)),
        ("interval hour to second(6)", Some(Q::HourToSecond)),
        ("interval minute to second", Some(Q::MinuteToSecond)),
        ("INTERVAL  Year\tTO MONTH", Some(Q::YearToMonth)),
        ("\"interval\"(4)", Some(Q::Year)),
        ("\"interval\"(6144, 3)", Some(Q::MinuteToSecond)),
        ("interval day to second(3)[]", Some(Q::DayToSecond)),
        ("public.span", Some(Q::MinuteToSecond)),
    ] {
        assert_eq!(kind(declared, &types), interval(qualifier), "`{declared}`");
        assert_eq!(
            resolve_declared_type(declared, &types),
            resolve_declared_type(
                if declared.ends_with("[]") { "interval[]" } else { "interval" },
                &[]
            ),
            "`{declared}`"
        );
    }
    for declared in ["interval year to second", "interval to month", "interval second to day"] {
        assert_eq!(resolve_declared_type(declared, &[]), TypeOutcome::Unknown, "`{declared}`");
    }
    for qualifier in Q::ALL {
        let range = qualifier.range();
        let quoted = format!("\"interval\"({range})");
        assert_eq!(kind(&quoted, &[]), interval(Some(qualifier)), "`{quoted}`");
    }
}

/// A quoted name is a catalog name, case and all: `"char"` is the one-byte
/// internal type, which no arm maps, `"INT4"` names nothing, and a
/// spelling only the grammar knows names a type `pg_catalog` does not
/// hold, so `"integer"` is not `integer`. A catalog name whose SQL
/// spelling is itself is still the built-in quoted.
#[test]
fn a_quoted_name_is_only_a_catalog_name() {
    for declared in [
        "\"char\"",
        "\"INT4\"",
        "\"bit\"",
        "interval fortnight",
        "\"integer\"",
        "\"double precision\"",
        "\"character varying\"(10)",
        "\"boolean\"",
        "\"timestamp with time zone\"",
    ] {
        assert_eq!(resolve_declared_type(declared, &[]), TypeOutcome::Unknown, "`{declared}`");
        assert_eq!(comparison_for(declared, None, &[], &[]), ComparisonPlan::Refused);
        assert_eq!(extension_for(declared, &[]), None, "`{declared}`");
    }
    for (declared, bare) in [("\"text\"", "text"), ("\"numeric\"(10,2)", "numeric(10,2)")] {
        assert_eq!(resolve_declared_type(declared, &[]), resolve_declared_type(bare, &[]));
    }
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
    let ComparisonPlan::Nested(NestedCompare::Array(element)) =
        comparison_for("public.b3[]", None, &types, &[])
    else {
        panic!("an array of a domain over `bit(3)` is nested");
    };
    assert!(
        matches!(element.plan, NestedCompare::Uncomparable { grammar, .. } if grammar == bit(Some(3)))
    );
    let ComparisonPlan::Nested(NestedCompare::Record(fields)) =
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
        ComparisonPlan::Nested(NestedCompare::Array(element)) => match element.plan {
            NestedCompare::Uncomparable { grammar, .. } => grammar,
            other => panic!("`{declared}`'s element is {other:?}"),
        },
        other => panic!("`{declared}` is {other:?}"),
    };
    assert_eq!(element_grammar("point[]"), geometric(Geometric::Point));
    assert_eq!(element_grammar("box[]"), geometric(Geometric::Box));
    assert_eq!(element_grammar("public.boxes[]"), geometric(Geometric::Box));
    let ComparisonPlan::Nested(NestedCompare::Record(fields)) =
        comparison_for("public.shapes", None, &types, &[])
    else {
        panic!("a composite is nested");
    };
    assert!(matches!(
        &fields[0].1.plan,
        NestedCompare::Array(element) if matches!(element.plan, NestedCompare::Uncomparable {
            grammar: Some(TextGrammar::Geometric(Geometric::Box)), ..
        })
    ));
    assert!(matches!(
        &fields[1].1.plan,
        NestedCompare::Uncomparable { grammar, .. } if *grammar == geometric(Geometric::Circle)
    ));
}

/// `interval` is the one type whose Arrow mapping is a *struct* of
/// PostgreSQL's own three fields rather than a reading of them: months,
/// days and a time part, independent, exactly as `Interval` stores them.
#[test]
fn interval_maps_to_arrows_three_field_calendar_interval() {
    assert_eq!(
        resolve_declared_type("interval", &[]),
        TypeOutcome::Mapped(
            DataType::Interval(arrow_schema::IntervalUnit::MonthDayNano),
            NestedPlan::Scalar
        )
    );
}

#[test]
fn numeric_picks_decimal_width_by_precision() {
    assert_eq!(
        resolve_declared_type("numeric(38,10)", &[]),
        TypeOutcome::Mapped(DataType::Decimal128(38, 10), NestedPlan::Decimal { precision: 38 })
    );
    assert_eq!(
        resolve_declared_type("numeric(39,0)", &[]),
        TypeOutcome::Mapped(DataType::Decimal256(39, 0), NestedPlan::Decimal { precision: 39 })
    );
    // The plan carries the precision the widened Arrow type does not.
    assert_eq!(
        resolve_declared_type("numeric(2,5)", &[]),
        TypeOutcome::Mapped(DataType::Decimal128(5, 5), NestedPlan::Decimal { precision: 2 })
    );
    assert_eq!(
        resolve_declared_type("numeric", &[]),
        TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
    );
    assert_eq!(
        resolve_declared_type("numeric(2,-2)", &[]),
        TypeOutcome::Mapped(DataType::Decimal128(2, -2), NestedPlan::Decimal { precision: 2 })
    );
    assert_eq!(
        resolve_declared_type("numeric(77,0)", &[]),
        TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
    );
}

/// **A scale past the precision widens it, and one no Arrow decimal
/// carries is text** — every `numeric(p,s)` PostgreSQL 15 and later admit
/// maps to a type `batch.rs` can build, compared at its own scale where
/// typed and by value where not.
#[test]
fn numeric_widens_a_precision_below_its_scale_and_texts_a_scale_arrow_lacks() {
    use arrow::datatypes::{Decimal128Type, Decimal256Type, validate_decimal_precision_and_scale};
    let decimal =
        |precision, scale| ComparisonPlan::agrees(CompareKind::Decimal { precision, scale });
    let text = |precision, scale| {
        ComparisonPlan::agrees(CompareKind::Numeric {
            infinities: false,
            typmod: Some(NumericTypmod { precision, scale }),
        })
    };
    for (typmod, mapped) in [
        ("2,5", (DataType::Decimal128(5, 5), decimal(2, 5))),
        ("1,38", (DataType::Decimal128(38, 38), decimal(1, 38))),
        ("1,40", (DataType::Decimal256(40, 40), decimal(1, 40))),
        ("50,76", (DataType::Decimal256(76, 76), decimal(50, 76))),
        ("2,-128", (DataType::Decimal128(2, -128), decimal(2, -128))),
        ("1,77", (DataType::Utf8View, text(1, 77))),
        ("1,200", (DataType::Utf8View, text(1, 200))),
        ("2,-129", (DataType::Utf8View, text(2, -129))),
        ("1000,-1000", (DataType::Utf8View, text(1000, -1000))),
    ] {
        assert_eq!(map_numeric(Some(typmod)), mapped, "numeric({typmod})");
        match mapped.0 {
            DataType::Decimal128(p, s) => {
                validate_decimal_precision_and_scale::<Decimal128Type>(p, s).unwrap();
            }
            DataType::Decimal256(p, s) => {
                validate_decimal_precision_and_scale::<Decimal256Type>(p, s).unwrap();
            }
            _ => {}
        }
    }
}

/// The twelve built-in range/multirange names carry their subtypes in the
/// catalog, never in DDL (I10). The multirange half maps to a `List` of
/// the *same* range struct — same subtype, different literal form.
#[test]
fn builtin_ranges_and_their_multirange_companions_map_to_their_hardcoded_subtypes() {
    let bounds = [
        ("int4range", "int4multirange", DataType::Int32),
        ("int8range", "int8multirange", DataType::Int64),
        // Bare `numeric` has no Arrow decimal representation, so a
        // `numrange`'s bounds are text — the subtype's own mapping.
        ("numrange", "nummultirange", DataType::Utf8View),
        ("tsrange", "tsmultirange", DataType::Timestamp(TimeUnit::Microsecond, None)),
        (
            "tstzrange",
            "tstzmultirange",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        ),
        ("daterange", "datemultirange", DataType::Date32),
    ];
    for (range, multirange, bound) in bounds {
        assert_eq!(
            resolve_declared_type(range, &[]),
            TypeOutcome::Mapped(
                range_struct(bound.clone()),
                NestedPlan::Range(Box::new(NestedPlan::Scalar))
            ),
            "{range}"
        );
        assert_eq!(
            resolve_declared_type(multirange, &[]),
            TypeOutcome::Mapped(
                list_of(range_struct(bound)),
                NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
            ),
            "{multirange}"
        );
    }
}

/// The collision `NestedPlan` exists for, asserted where it is otherwise
/// invisible: one Arrow type, two literal forms
/// (`{"[1,10)","[2,3)"}` versus `{[1,10),[2,3)}`).
#[test]
fn an_array_of_ranges_and_a_multirange_agree_on_the_type_and_not_on_the_plan() {
    let TypeOutcome::Mapped(array_type, array_plan) = resolve_declared_type("int4range[]", &[])
    else {
        panic!("int4range[] maps")
    };
    let TypeOutcome::Mapped(multi_type, multi_plan) = resolve_declared_type("int4multirange", &[])
    else {
        panic!("int4multirange maps")
    };
    assert_eq!(array_type, multi_type);
    assert_ne!(array_plan, multi_plan);
}

#[test]
fn an_array_maps_to_a_list_of_its_element_type() {
    assert_eq!(
        resolve_declared_type("integer[]", &[]),
        TypeOutcome::Mapped(list_of(DataType::Int32), NestedPlan::array(NestedPlan::Scalar))
    );
    let types = [ty("public.mood", TypeKind::Enum { labels: vec!["sad".into()], exact: true })];
    assert_eq!(
        resolve_declared_type("public.mood[]", &types),
        TypeOutcome::Mapped(
            list_of(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))),
            NestedPlan::array(NestedPlan::Scalar)
        )
    );
    // A type Arrow has no representation for is `Utf8View` *in that
    // position*, exactly as it would be at top level.
    assert_eq!(
        resolve_declared_type("inet[]", &[]),
        TypeOutcome::Mapped(list_of(DataType::Utf8View), NestedPlan::array(NestedPlan::Scalar))
    );
}

/// The delimiter trap (I22): an array's separator is its *element type's*
/// `typdelim`, and `box`'s is `;`. The refusal has to run on the terminal
/// of the domain walk, since a domain's own DDL records nothing about it.
#[test]
fn an_array_over_an_opaque_element_type_is_refused_through_any_chain_of_domains() {
    let types = [
        ty("public.mybase", TypeKind::base()),
        ty("public.shellonly", TypeKind::Shell),
        ty("public.box_domain", TypeKind::domain("box")),
        ty("public.box_domain2", TypeKind::domain("public.box_domain")),
        // `--quote-all-identifiers` quotes the built-in (I8).
        ty("public.quoted_box_domain", TypeKind::domain("\"box\"")),
    ];
    for declared in [
        "box[]",
        "BOX[]",
        "\"box\"[]",
        "public.mybase[]",
        "public.shellonly[]",
        "public.box_domain[]",
        "public.box_domain2[]",
        "public.quoted_box_domain[]",
    ] {
        assert_eq!(
            resolve_declared_type(declared, &types),
            TypeOutcome::OpaqueElementType,
            "{declared}"
        );
    }
    // The scalar cases are unchanged: only the *array* is refused for its
    // element, and `box` itself is simply a type this build never mapped.
    assert_eq!(resolve_declared_type("box", &types), TypeOutcome::Unknown);
    assert_eq!(
        resolve_declared_type("public.box_domain", &types),
        TypeOutcome::Unknown,
        "a domain resolves through to its base, which this build has no mapping for"
    );
}

/// I26, and the transitive half the fixture does not carry: a domain over
/// a domain over an array produces a literal byte-identical to the
/// single-hop case, so the walk is pinned here instead.
#[test]
fn an_array_over_an_array_typed_element_is_refused_through_any_chain_of_domains() {
    let types = [
        ty("public.intarr", TypeKind::domain("integer[]")),
        ty("public.intarr2", TypeKind::domain("public.intarr")),
        ty("public.intarr3", TypeKind::domain("public.intarr2")),
    ];
    for declared in ["public.intarr[]", "public.intarr2[]", "public.intarr3[]"] {
        assert_eq!(
            resolve_declared_type(declared, &types),
            TypeOutcome::NestedArrayElement,
            "{declared}"
        );
    }
    // Only the outer array is refused: the domain itself is an ordinary
    // `integer[]` column at every depth of the chain.
    for declared in ["public.intarr", "public.intarr2", "public.intarr3"] {
        assert_eq!(
            resolve_declared_type(declared, &types),
            TypeOutcome::Mapped(list_of(DataType::Int32), NestedPlan::array(NestedPlan::Scalar)),
            "{declared}"
        );
    }
}

/// I28: PostgreSQL accepts six spellings for an array-typed column and
/// every one is the same type — the parser keeps "array of the element"
/// and discards the bracket count and the bounds — so all six resolve
/// exactly as `integer[]` does, one array level deep.
///
/// Five of the six survive no round trip through `format_type`, so
/// `pg_dump` can never write them (I21) and no generated fixture reaches
/// this: `roadmap.md`'s "Where a fixture is impossible" carve-out.
#[test]
fn every_array_declaration_spelling_is_one_array_of_the_element_type() {
    let expected =
        TypeOutcome::Mapped(list_of(DataType::Int32), NestedPlan::array(NestedPlan::Scalar));
    for declared in [
        // The six of I28's table.
        "integer[]",
        "integer[3]",
        "integer[][]",
        "integer[3][4]",
        "integer ARRAY",
        "integer ARRAY[4]",
        // The bracket run is unbounded in the DDL even though `MAXDIM`
        // is 6; `ARRAY` is a keyword, so its case carries nothing; and
        // the lexer is free with whitespace.
        "integer[][][][][][][][][]",
        "INTEGER array[4]",
        "integer  Array",
        "integer [ 3 ] [ ]",
    ] {
        assert_eq!(resolve_declared_type(declared, &[]), expected, "{declared}");
    }
}

/// The other direction of the same rule: a declaration PostgreSQL rejects
/// gets no array type invented for it. `ARRAY` takes at most one bound and
/// only in the `[n]` form, what precedes it is a bare type name, and a
/// malformed bound is not a bound. Every string here is a syntax error,
/// and `Unknown` says so where a refusal would state something false.
#[test]
fn a_declaration_postgresql_would_reject_is_not_read_as_an_array() {
    for declared in [
        "integer ARRAY[4][5]",
        "integer ARRAY[]",
        "integer ARRAY ARRAY",
        "integer[] ARRAY",
        "integer[abc]",
        "integer[-1]",
        "integer[",
        "integerARRAY",
        "ARRAY",
    ] {
        assert_eq!(resolve_declared_type(declared, &[]), TypeOutcome::Unknown, "{declared}");
    }
}

/// A domain's base type is spelled by whoever wrote the `CREATE DOMAIN`,
/// so the array-ness the I26 refusal tests for can arrive in any spelling
/// and the domain walk stops on the raw text. `d[]` over `CREATE DOMAIN d
/// AS integer ARRAY` is refused exactly as `d[]` over `integer[]` is,
/// while `d` itself is an ordinary `integer[]` column.
#[test]
fn a_domain_over_an_array_is_recognized_in_every_spelling() {
    for base in ["integer[]", "integer[3]", "integer[][]", "integer ARRAY", "integer ARRAY[4]"] {
        let types = [ty("public.d", TypeKind::domain(base))];
        assert_eq!(
            resolve_declared_type("public.d[]", &types),
            TypeOutcome::NestedArrayElement,
            "{base}"
        );
        assert_eq!(
            resolve_declared_type("public.d", &types),
            TypeOutcome::Mapped(list_of(DataType::Int32), NestedPlan::array(NestedPlan::Scalar)),
            "{base}"
        );
    }
}

/// The refusal composes into a composite for free: `resolve_nested` maps
/// every non-`Mapped` outcome to `Utf8View` *in that position*, so a field
/// of the refused type is one string field, not a refused column.
#[test]
fn a_composite_field_of_the_refused_array_type_is_a_string_field_only() {
    let types = [
        ty("public.intarr", TypeKind::domain("integer[]")),
        ty(
            "public.arr_holder",
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("label", "text"),
                    ColumnDef::new("arr", "public.intarr[]"),
                ]),
            },
        ),
    ];
    assert_eq!(
        resolve_declared_type("public.arr_holder", &types),
        TypeOutcome::Mapped(
            DataType::Struct(Fields::from(vec![
                Field::new("label", DataType::Utf8View, true),
                Field::new("arr", DataType::Utf8View, true),
            ])),
            NestedPlan::record(vec![NestedPlan::Scalar, NestedPlan::Scalar])
        )
    );
}

#[test]
fn a_composite_maps_to_a_struct_of_its_declared_fields() {
    let types = [
        ty(
            "public.point2d",
            TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("x", "integer"), ColumnDef::new("y", "text")]),
            },
        ),
        ty("public.empty_comp", TypeKind::Composite { fields: Some(vec![]) }),
        ty("public.broken", TypeKind::Composite { fields: None }),
    ];
    assert_eq!(
        resolve_declared_type("public.point2d", &types),
        TypeOutcome::Mapped(
            DataType::Struct(Fields::from(vec![
                Field::new("x", DataType::Int32, true),
                Field::new("y", DataType::Utf8View, true),
            ])),
            NestedPlan::record(vec![NestedPlan::Scalar, NestedPlan::Scalar])
        )
    );
    // A zero-field composite is a real type with a real value, `()`
    // (I23) — a zero-field `Struct` is well-formed Arrow.
    assert_eq!(
        resolve_declared_type("public.empty_comp", &types),
        TypeOutcome::Mapped(DataType::Struct(Fields::empty()), NestedPlan::record([]))
    );
    // A body the grammar could not read is *not* a short field list:
    // `record_out` is positional, so a `Struct` built from one would
    // refuse every valid row.
    assert_eq!(resolve_declared_type("public.broken", &types), TypeOutcome::Unknown);
}

/// Nesting composes through the same function, so a composite field's own
/// array and an array of composites are the same recursion in two orders.
#[test]
fn nesting_composes_in_both_orders() {
    let types = [
        ty(
            "public.point2d",
            TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
        ),
        ty(
            "public.tagged",
            TypeKind::Composite { fields: Some(vec![ColumnDef::new("tags", "text[]")]) },
        ),
    ];
    let point = DataType::Struct(Fields::from(vec![Field::new("x", DataType::Int32, true)]));
    assert_eq!(
        resolve_declared_type("public.point2d[]", &types),
        TypeOutcome::Mapped(
            list_of(point),
            NestedPlan::array(NestedPlan::record(vec![NestedPlan::Scalar]))
        )
    );
    assert_eq!(
        resolve_declared_type("public.tagged", &types),
        TypeOutcome::Mapped(
            DataType::Struct(Fields::from(vec![Field::new(
                "tags",
                list_of(DataType::Utf8View),
                true
            )])),
            NestedPlan::record(vec![NestedPlan::array(NestedPlan::Scalar)])
        )
    );
}

/// A user-defined range's subtype comes from its own DDL, and a range
/// whose parameter list the grammar could not read keeps the struct with
/// text bounds rather than losing the shape.
#[test]
fn a_user_range_maps_through_its_declared_subtype() {
    let types = [
        ty(
            "public.myrange",
            TypeKind::Range {
                subtype: Some("double precision".to_string()),
                multirange_type_name: None,
                canonical: None,
            },
        ),
        ty(
            "public.bare",
            TypeKind::Range { subtype: None, multirange_type_name: None, canonical: None },
        ),
    ];
    assert_eq!(
        resolve_declared_type("public.myrange", &types),
        TypeOutcome::Mapped(
            range_struct(DataType::Float64),
            NestedPlan::Range(Box::new(NestedPlan::Scalar))
        )
    );
    assert_eq!(
        resolve_declared_type("public.bare", &types),
        TypeOutcome::Mapped(
            range_struct(DataType::Utf8View),
            NestedPlan::Range(Box::new(NestedPlan::Scalar))
        )
    );
}

/// I10: a user range's auto-created multirange companion has no
/// `CREATE TYPE` of its own anywhere in the dump — its only trace is the
/// `multirange_type_name` parameter inside the range's own DDL, so a
/// column declared with that companion name must still resolve, not
/// fall through to `Unknown`.
#[test]
fn a_ranges_multirange_companion_resolves_even_with_no_type_def_of_its_own() {
    let types = [ty(
        "public.myrange",
        TypeKind::Range {
            subtype: Some("double precision".to_string()),
            multirange_type_name: Some("public.myrange_multi".to_string()),
            canonical: None,
        },
    )];
    assert_eq!(
        resolve_declared_type("public.myrange_multi", &types),
        TypeOutcome::Mapped(
            list_of(range_struct(DataType::Float64)),
            NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
        ),
        "the companion's bound type comes from the range that names it"
    );
    // A name that merely resembles a companion but isn't named by any
    // range's `multirange_type_name` stays `Unknown`.
    assert_eq!(resolve_declared_type("public.not_a_companion", &types), TypeOutcome::Unknown);
}

#[test]
fn base_and_shell_types_are_opaque() {
    let types = [ty("public.gtype", TypeKind::base()), ty("public.forward", TypeKind::Shell)];
    assert_eq!(resolve_declared_type("public.gtype", &types), TypeOutcome::OpaqueBaseType);
    assert_eq!(resolve_declared_type("public.forward", &types), TypeOutcome::OpaqueBaseType);
}

#[test]
fn a_user_type_absent_from_the_list_is_unknown() {
    assert_eq!(resolve_declared_type("public.nope", &[]), TypeOutcome::Unknown);
}

// -- the canonical extension names -------------------------------------

/// The table the extension names are keyed on, and the storage type each
/// requires. Kept beside the tests so both directions read it.
const EXTENSIONS: [(&str, CanonicalExtension, DataType); 3] = [
    ("uuid", CanonicalExtension::Uuid, DataType::FixedSizeBinary(16)),
    ("json", CanonicalExtension::Json, DataType::Utf8View),
    ("jsonb", CanonicalExtension::Json, DataType::Utf8View),
];

/// **The join that keeps `extension_for` and `builtin_scalar` in step.**
/// They are separate walks of the same string: a `uuid` remapped away
/// from `FixedSizeBinary(16)` would make `apply`'s `expect` a panic on a
/// real dump. `try_with_extension_type` is arrow-rs's own
/// `supports_data_type`, so this asserts the crate's rule.
#[test]
fn the_extension_names_fit_their_storage_type() {
    for (declared, extension, storage) in EXTENSIONS {
        assert_eq!(extension_for(declared, &[]), Some(extension), "{declared}");
        assert_eq!(
            builtin_scalar(declared, None, None, None, &[]).map(|(dt, _)| dt),
            Some(storage.clone()),
            "{declared}"
        );
        let field = extension.apply(Field::new("v", storage, true));
        assert!(
            field.try_canonical_extension_type().is_ok(),
            "{declared}: arrow-rs must read back what we wrote"
        );
    }
}

/// The names themselves, in the metadata a consumer reads. `arrow.json`
/// carries an empty metadata value and `arrow.uuid` carries none, which
/// is the difference hand-writing the two keys would get wrong.
#[test]
fn the_extension_metadata_is_the_canonical_spelling() {
    let uuid = with_extension(Field::new("v", DataType::FixedSizeBinary(16), true), "uuid", &[]);
    assert_eq!(uuid.metadata().get("ARROW:extension:name").map(String::as_str), Some("arrow.uuid"));
    assert_eq!(uuid.metadata().get("ARROW:extension:metadata"), None);

    let json = with_extension(Field::new("v", DataType::Utf8View, true), "jsonb", &[]);
    assert_eq!(json.metadata().get("ARROW:extension:name").map(String::as_str), Some("arrow.json"));
    assert_eq!(json.metadata().get("ARROW:extension:metadata").map(String::as_str), Some(""));
}

/// A domain bottoms out at the extension its base type names, through any
/// chain — the same recursion `resolve_declared_type` makes, so the two
/// cannot disagree about what a domain column holds.
#[test]
fn a_domain_over_uuid_carries_the_name() {
    let types = [
        ty("public.d_uuid", TypeKind::domain("uuid")),
        ty("public.d_deep", TypeKind::domain("public.d_uuid")),
    ];
    assert_eq!(extension_for("public.d_deep", &types), Some(CanonicalExtension::Uuid));
}

/// Everything that carries no name, and the array is the one worth
/// stating: `uuid[]` is a `List<FixedSizeBinary(16)>`, and `arrow.uuid`
/// on the column's own field would claim the *list* is a UUID.
#[test]
fn nothing_else_carries_a_name() {
    let types = [
        ty("public.mood", TypeKind::Enum { labels: vec!["sad".into()], exact: true }),
        ty("public.d_int", TypeKind::domain("integer")),
    ];
    for declared in ["uuid[]", "json[]", "text", "integer", "public.mood", "public.d_int"] {
        assert_eq!(extension_for(declared, &types), None, "{declared}");
    }
    // A `.`-qualified name the dump never declared names no extension.
    assert_eq!(extension_for("public.nope", &types), None);
}

// -- the comparison register ------------------------------------------

fn agrees(kind: CompareKind) -> ComparisonPlan {
    ComparisonPlan::agrees(kind)
}

/// The register, row for row: every declared type this build maps to a
/// scalar, and how a column of it compares
/// (`docs/design/decisions.md`, "D40").
///
/// **Every arm of [`builtin_scalar`] appears here** but `int2vector`'s,
/// which maps to a list and compares as a nested column (D58): a type
/// added to that table without a row here is one nothing states the
/// comparison of.
#[test]
fn the_register_answers_every_builtin_scalar() {
    use CompareKind as K;
    let text = || ComparisonPlan::AS_TEXT;
    let unknown = |kind| ComparisonPlan::diverging(kind, ComparisonDivergence::UnknownCollation);
    for (declared, expected) in [
        ("smallint", agrees(K::Int { bytes: 2 })),
        ("integer", agrees(K::Int { bytes: 4 })),
        ("bigint", agrees(K::Int { bytes: 8 })),
        ("oid", agrees(K::UnsignedInt)),
        ("boolean", agrees(K::Bool)),
        ("real", agrees(K::Float32)),
        ("double precision", agrees(K::Float64)),
        ("numeric(10,2)", agrees(K::Decimal { precision: 10, scale: 2 })),
        ("numeric(50,0)", agrees(K::Decimal { precision: 50, scale: 0 })),
        // The two `numeric` shapes with no Arrow decimal behind them, and
        // the one place the register distinguishes them: a typmod rejects
        // an infinity, so only the bare form admits the spelling (I34).
        ("numeric", agrees(K::Numeric { infinities: true, typmod: None })),
        (
            "numeric(77,0)",
            agrees(K::Numeric {
                infinities: false,
                typmod: Some(NumericTypmod { precision: 77, scale: 0 }),
            }),
        ),
        // The four collatable arms, each asked with no `COLLATE` clause.
        // `name`'s type default is `C`, so it agrees where the others
        // cannot; `character(n)` carries a comparison of its own (the
        // padding is trimmed off both sides, I38).
        // A `varchar(n)` and a `character(n)` carry their length (I75).
        ("text", unknown(K::Text { length: None })),
        ("character varying(10)", unknown(K::Text { length: Some(10) })),
        ("character varying", unknown(K::Text { length: None })),
        ("character(10)", unknown(K::PaddedText { length: Some(10) })),
        ("character", unknown(K::PaddedText { length: Some(1) })),
        ("bpchar", unknown(K::PaddedText { length: None })),
        ("name", agrees(K::Text { length: None })),
        ("date", agrees(K::Date)),
        ("timestamp without time zone", agrees(K::Timestamp { with_tz: false })),
        ("timestamp with time zone", agrees(K::Timestamp { with_tz: true })),
        ("time without time zone", agrees(K::Time)),
        ("time with time zone", agrees(K::TimeTz)),
        ("interval", agrees(K::Interval { qualifier: None })),
        ("uuid", agrees(K::Uuid)),
        ("bytea", agrees(K::Bytea)),
        // The one the register still holds as text, because the server
        // defines no order for it to be wrong about.
        ("json", text()),
        // `jsonb` is ordered structurally and diverges only where a
        // string leaf decides, which is the database's collation (I41).
        ("jsonb", ComparisonPlan::diverging(K::Jsonb, ComparisonDivergence::JsonbStringCollation)),
        // `cidr` differs from `inet` only in refusing a literal with a
        // bit set below its netmask, and `macaddr8` from `macaddr` only
        // in its width — both carried, the declared name being gone.
        ("inet", agrees(K::Network { cidr: false })),
        ("cidr", agrees(K::Network { cidr: true })),
        ("macaddr", agrees(K::MacAddr { octets: 6 })),
        ("macaddr8", agrees(K::MacAddr { octets: 8 })),
    ] {
        assert_eq!(comparison_for(declared, None, &[], &[]), expected, "{declared}");
    }
    // A declared type this build maps to nothing has no comparison
    // either — one walk of the same string answers both.
    assert_eq!(comparison_for("money", None, &[], &[]), ComparisonPlan::Refused);
    // A keyword is a keyword on both walks (I5).
    assert_eq!(comparison_for("INTEGER", None, &[], &[]), agrees(K::Int { bytes: 4 }));
}

/// Six unrelated declared types reach the same text columns — five as
/// `Utf8View`, an enum as a `Dictionary` of its labels — which is why the
/// register cannot be keyed on the Arrow type: an enum compares by
/// declaration order, a bare `numeric` by decimal value, an `inet` by
/// family-then-prefix, a `jsonb` by a walk down two containers, `text`
/// bytewise under its collation, and `json` bytewise with no order at all.
#[test]
fn the_register_tells_apart_types_that_share_one_arrow_type() {
    let labels = ["sad".to_string(), "ok".to_string()];
    let types = [ty("public.mood", TypeKind::Enum { labels: labels.to_vec(), exact: true })];
    assert_eq!(
        comparison_for("public.mood", None, &types, &[]),
        ComparisonPlan::agrees(CompareKind::Enum {
            labels: labels.iter().cloned().collect(),
            exact: true
        }),
    );
    assert_eq!(
        comparison_for("numeric", None, &[], &[]),
        ComparisonPlan::agrees(CompareKind::Numeric { infinities: true, typmod: None }),
    );
    assert_eq!(
        comparison_for("inet", None, &[], &[]),
        ComparisonPlan::agrees(CompareKind::Network { cidr: false }),
    );
    assert_eq!(comparison_for("json", None, &[], &[]), ComparisonPlan::AS_TEXT);
    // `jsonb` shares the Arrow type and neither the comparison nor the
    // verdict: it is a container walk that diverges only at a string.
    assert_eq!(
        comparison_for("jsonb", None, &[], &[]),
        ComparisonPlan::diverging(CompareKind::Jsonb, ComparisonDivergence::JsonbStringCollation),
    );
    // `text` is the last, and it does not share `AS_TEXT` with them:
    // reading the collation is what tells them apart.
    assert_eq!(
        comparison_for("text", None, &[], &[]),
        ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::UnknownCollation
        ),
    );
    // An enum with no labels resolves to no Arrow type at all, so no
    // column of it is ever asked how it compares.
    let empty = [ty("public.empty", TypeKind::Enum { labels: vec![], exact: true })];
    assert_eq!(comparison_for("public.empty", None, &empty, &[]), ComparisonPlan::Refused);
}

/// **A scalar column is bounded in every semantics whose order over its
/// text is exact, one set where two coincide**: a divergent plan's one set
/// is DataFusion's and read there alone, a column with no plan is bounded
/// bytewise for DataFusion, and an exact kind DataFusion orders otherwise
/// keeps a second set in DataFusion's order — but `macaddr`, whose one set is read in
/// both (`docs/design/decisions.md`, "D79").
#[test]
fn a_column_is_bounded_in_each_semantics_whose_order_is_exact() {
    use BoundsSet::{DataFusion as F, Primary as P};
    use ComparisonDivergence::*;
    use ComparisonSemantics::{DataFusion, Postgres};
    let text = || Some(CompareKind::Text { length: None });
    let read = |plan: &ComparisonPlan| {
        (plan.bounds_kinds(), plan.bounds_in(Postgres), plan.bounds_in(DataFusion))
    };
    assert_eq!(
        read(&ComparisonPlan::agrees(CompareKind::Text { length: None })),
        ([text(), None], Some(P), Some(P))
    );
    for divergence in [AsText, UnknownCollation, NonBytewiseCollation, NonDeterministicCollation] {
        let plan = ComparisonPlan::diverging(CompareKind::Text { length: None }, divergence);
        assert_eq!(read(&plan), ([text(), None], None, Some(P)), "{divergence:?}");
    }
    for (kind, divergence) in [
        (CompareKind::PaddedText { length: None }, UnknownCollation),
        (CompareKind::PaddedText { length: None }, NonBytewiseCollation),
        (CompareKind::Jsonb, JsonbStringCollation),
    ] {
        let plan = ComparisonPlan::diverging(kind.clone(), divergence);
        assert_eq!(read(&plan), ([text(), None], None, Some(P)), "{kind:?}");
    }
    assert_eq!(read(&ComparisonPlan::Refused), ([text(), None], None, Some(P)));
    let labels: Arc<[String]> = Arc::from(vec!["b".to_string(), "a".to_string()]);
    for kind in [
        CompareKind::PaddedText { length: None },
        CompareKind::Enum { labels, exact: true },
        CompareKind::Numeric { infinities: true, typmod: None },
        CompareKind::TimeTz,
        CompareKind::Network { cidr: false },
        CompareKind::Jsonb,
    ] {
        let plan = ComparisonPlan::agrees(kind.clone());
        assert_eq!(read(&plan), ([Some(kind.clone()), text()], Some(P), Some(F)), "{kind:?}");
    }
    assert_eq!(
        read(&ComparisonPlan::agrees(CompareKind::Interval { qualifier: None })),
        (
            [
                Some(CompareKind::Interval { qualifier: None }),
                Some(CompareKind::IntervalFields { qualifier: None })
            ],
            Some(P),
            Some(F)
        )
    );
    for kind in
        [CompareKind::MacAddr { octets: 6 }, CompareKind::Int { bytes: 4 }, CompareKind::Bytea]
    {
        let plan = ComparisonPlan::agrees(kind.clone());
        assert_eq!(read(&plan), ([Some(kind.clone()), None], Some(P), Some(P)), "{kind:?}");
    }
    let nested = ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
        range_type: "r".into(),
        function: "f".into(),
    });
    assert_eq!(read(&nested), ([None, None], None, None));
}

/// **A term reads the set gathering stored under the kind it compares
/// by**, not the one its own plan would have stored: a column compared as
/// text for want of a plan — every column under `SchemaMode::Strings` —
/// reads a set keyed `Text` or `macaddr`'s, whose text orders as its
/// octets (I40), and never one keyed by a value order
/// (`docs/design/decisions.md`, "D79").
#[test]
fn a_term_reads_the_set_keyed_by_the_kind_it_compares_by() {
    use BoundsSet::{DataFusion as F, Primary as P};
    let text = CompareKind::Text { length: None };
    let refused = ComparisonPlan::Refused.bounds_read_by(ComparisonSemantics::DataFusion);
    assert_eq!(refused.as_ref(), Some(&text));
    assert_eq!(ComparisonPlan::Refused.bounds_read_by(ComparisonSemantics::Postgres), None);
    let labels: Arc<[String]> = Arc::from(vec!["b".to_string()]);
    for (declared, stored, want) in [
        ("text", [Some(text.clone()), None], Some(P)),
        ("an enum", [Some(CompareKind::Enum { labels, exact: true }), Some(text.clone())], Some(F)),
        ("macaddr", [Some(CompareKind::MacAddr { octets: 6 }), None], Some(P)),
        ("integer", [Some(CompareKind::Int { bytes: 4 }), None], None),
        (
            "interval",
            [
                Some(CompareKind::Interval { qualifier: None }),
                Some(CompareKind::IntervalFields { qualifier: None }),
            ],
            None,
        ),
        ("a nested column", [None, None], None),
    ] {
        assert_eq!(bounds_set_keyed_by(&stored, &text), want, "{declared}");
    }
    let int = [Some(CompareKind::Int { bytes: 4 }), None];
    assert_eq!(bounds_set_keyed_by(&int, &CompareKind::Int { bytes: 4 }), Some(P));
    let mac = [Some(CompareKind::MacAddr { octets: 6 }), None];
    assert_eq!(bounds_set_keyed_by(&mac, &CompareKind::MacAddr { octets: 8 }), None);
}

/// The collation rule, as a table: what a text column's `COLLATE` clause
/// (or its absence, against the type's own default) does to the register's
/// verdict. **The comparison never moves — only the verdict does.**
#[test]
fn a_text_column_is_judged_by_the_collation_it_states() {
    let agrees_text = || ComparisonPlan::agrees(CompareKind::Text { length: None });
    let unknown = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::UnknownCollation,
        )
    };
    let named = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::NonBytewiseCollation,
        )
    };
    for (declared, collation, expected) in [
        // No clause: the type's own default decides, and only `name`'s is
        // `C` (I37).
        ("text", None, unknown()),
        (
            "character varying(10)",
            None,
            ComparisonPlan::diverging(
                CompareKind::Text { length: Some(10) },
                ComparisonDivergence::UnknownCollation,
            ),
        ),
        ("name", None, agrees_text()),
        // An explicit clause overrides it in both directions.
        ("text", Some("pg_catalog.\"C\""), agrees_text()),
        ("text", Some("pg_catalog.\"POSIX\""), agrees_text()),
        ("text", Some("pg_catalog.\"en_US.utf8\""), named()),
        ("text", Some("public.mycoll"), named()),
        ("name", Some("pg_catalog.\"en_US.utf8\""), named()),
        // Unqualified, as a hand-written dump might spell it.
        ("text", Some("\"C\""), agrees_text()),
        // An unquoted `C` is the collation `c`, not the built-in one; the
        // server folds it the same way.
        ("text", Some("C"), named()),
        // Nor is a `"C"` some other schema happens to define.
        ("text", Some("public.\"C\""), named()),
        // `character(n)` reads the clause exactly as `text` does, over
        // its own trimmed comparison (I38).
        (
            "character(10)",
            Some("pg_catalog.\"C\""),
            ComparisonPlan::agrees(CompareKind::PaddedText { length: Some(10) }),
        ),
        (
            "character(10)",
            Some("pg_catalog.\"en_US.utf8\""),
            ComparisonPlan::diverging(
                CompareKind::PaddedText { length: Some(10) },
                ComparisonDivergence::NonBytewiseCollation,
            ),
        ),
        (
            "character(10)",
            None,
            ComparisonPlan::diverging(
                CompareKind::PaddedText { length: Some(10) },
                ComparisonDivergence::UnknownCollation,
            ),
        ),
        // A non-collatable type ignores a clause it cannot carry.
        (
            "integer",
            Some("pg_catalog.\"C\""),
            ComparisonPlan::agrees(CompareKind::Int { bytes: 4 }),
        ),
        (
            "interval",
            Some("pg_catalog.\"C\""),
            ComparisonPlan::agrees(CompareKind::Interval { qualifier: None }),
        ),
        // Including one that is still held as text: `json` is not
        // collatable either, so a clause on it moves nothing.
        ("json", Some("pg_catalog.\"C\""), ComparisonPlan::AS_TEXT),
    ] {
        assert_eq!(
            comparison_for(declared, collation, &[], &[]),
            expected,
            "{declared} {collation:?}"
        );
    }
}

/// The fourth collation branch: a column stating a collation the *same
/// dump* declared `deterministic = false` (I42).
///
/// The only one of the four that reaches `=`, and the only one read off a
/// statement rather than a name — so the same clause answers differently
/// depending on what the dump's `CREATE COLLATION` list holds.
#[test]
fn a_collation_the_dump_declares_non_deterministic_diverges_under_equality_too() {
    use CompareKind as K;
    let coll =
        |name: &str, deterministic: bool| CollationDef { name: name.to_string(), deterministic };
    let nd = ComparisonDivergence::NonDeterministicCollation;
    let named = ComparisonDivergence::NonBytewiseCollation;

    // Nothing declared: the clause is read off its name alone.
    assert_eq!(
        comparison_for("text", Some("public.icu_ci"), &[], &[]),
        ComparisonPlan::diverging(K::Text { length: None }, named)
    );
    // Declared deterministic moves nothing either.
    assert_eq!(
        comparison_for("text", Some("public.icu_ci"), &[], &[coll("public.icu_ci", true)]),
        ComparisonPlan::diverging(K::Text { length: None }, named)
    );
    // Declared non-deterministic: same comparison, stronger verdict.
    let declared = [coll("public.other", true), coll("public.icu_ci", false)];
    assert_eq!(
        comparison_for("text", Some("public.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None }, nd)
    );
    // `character(n)` and `character varying` reach the same branch, and
    // `character(n)` keeps its own trim (I38).
    assert_eq!(
        comparison_for("character(10)", Some("public.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::PaddedText { length: Some(10) }, nd)
    );
    assert_eq!(
        comparison_for("character varying(10)", Some("public.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: Some(10) }, nd)
    );
    // A domain's own clause is the column's default, so it reaches the
    // branch the same way a column-level clause does.
    let types = [ty(
        "public.dom_nd",
        TypeKind::Domain {
            base_type: "text".to_string(),
            collation: Some("public.icu_ci".to_string()),
            not_null: false,
            check: false,
        },
    )];
    assert_eq!(
        comparison_for("public.dom_nd", None, &types, &declared),
        ComparisonPlan::diverging(K::Text { length: None }, nd)
    );
    // A column of a *different* collation is untouched by the entry.
    assert_eq!(
        comparison_for("text", Some("public.other"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None }, named)
    );
    // A non-collatable type carries no clause and so never reaches it.
    assert_eq!(
        comparison_for("integer", Some("public.icu_ci"), &[], &declared),
        ComparisonPlan::agrees(K::Int { bytes: 4 })
    );
    // Quoting is not textual: the two spellings come from different
    // `pg_dump` paths and either may quote what the other leaves bare.
    assert_eq!(
        comparison_for("text", Some("public.\"icu_ci\""), &[], &[coll("\"public\".icu_ci", false)]),
        ComparisonPlan::diverging(K::Text { length: None }, nd)
    );
    // An unqualified reference matches on the name alone — the
    // announcing direction, since a file's search path is not read.
    assert_eq!(
        comparison_for("text", Some("icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None }, nd)
    );
    // ... but a *qualified* reference must agree on the schema.
    assert_eq!(
        comparison_for("text", Some("elsewhere.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None }, named)
    );
    // An unqualified `"C"` is the built-in unless the dump declares one of
    // that name, which a path naming `pg_catalog` after it puts first (`KD48`):
    // then it is the weaker verdict, and never both branches at once.
    let c = ComparisonPlan::agrees(K::Text { length: None });
    assert_eq!(comparison_for("text", Some("\"C\""), &[], &declared), c);
    assert_eq!(
        comparison_for("text", Some("pg_catalog.\"C\""), &[], &[coll("public.\"C\"", true)]),
        c
    );
    assert_eq!(
        comparison_for("text", Some("\"C\""), &[], &[coll("public.\"C\"", true)]),
        ComparisonPlan::diverging(K::Text { length: None }, named)
    );
    assert_eq!(
        comparison_for("text", Some("\"POSIX\""), &[], &[coll("public.\"POSIX\"", false)]),
        ComparisonPlan::diverging(K::Text { length: None }, nd)
    );
}

/// The operator dimension, stated once: this is the only collation
/// divergence `=`/`!=` ever sees.
#[test]
fn only_the_non_deterministic_collation_reaches_equality() {
    use ComparisonDivergence as D;
    for divergence in [D::AsText, D::UnmodelledType, D::NonDeterministicCollation] {
        assert!(divergence.affects_equality(), "{divergence:?}");
    }
    for divergence in [D::UnknownCollation, D::NonBytewiseCollation, D::JsonbStringCollation] {
        assert!(!divergence.affects_equality(), "{divergence:?}");
    }
}

/// A domain's own `COLLATE` is its type default, so a column of it with
/// no clause inherits it — and a column-level clause overrides it, which
/// is exactly the pair `pg_dump` writes a clause to express (I37).
#[test]
fn a_domain_s_collation_is_the_column_s_default_and_the_column_may_override() {
    let types = [
        ty(
            "public.dom_c",
            TypeKind::Domain {
                base_type: "text".to_string(),
                collation: Some("pg_catalog.\"C\"".to_string()),
                not_null: false,
                check: false,
            },
        ),
        ty("public.dom_plain", TypeKind::domain("text")),
        ty("public.dom_over_c", TypeKind::domain("public.dom_c")),
    ];
    let agrees_text = || ComparisonPlan::agrees(CompareKind::Text { length: None });
    let unknown = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::UnknownCollation,
        )
    };
    let named = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::NonBytewiseCollation,
        )
    };

    assert_eq!(comparison_for("public.dom_c", None, &types, &[]), agrees_text());
    assert_eq!(comparison_for("public.dom_plain", None, &types, &[]), unknown());
    // Through a chain, like every other domain answer.
    assert_eq!(comparison_for("public.dom_over_c", None, &types, &[]), agrees_text());
    // The column's clause wins over the domain's, both ways round.
    assert_eq!(
        comparison_for("public.dom_c", Some("pg_catalog.\"en_US.utf8\""), &types, &[]),
        named()
    );
    assert_eq!(
        comparison_for("public.dom_plain", Some("pg_catalog.\"C\""), &types, &[]),
        agrees_text()
    );
}

/// A domain compares as whatever it bottoms out at, through any chain —
/// the same recursion `resolve_declared_type` makes, so a domain over
/// `integer` orders numerically rather than as text.
#[test]
fn a_domain_compares_as_the_type_it_bottoms_out_at() {
    let types = [
        ty("public.d1", TypeKind::domain("integer")),
        ty("public.d2", TypeKind::domain("public.d1")),
        ty("public.dtext", TypeKind::domain("text")),
        ty("public.darr", TypeKind::domain("integer[]")),
        ty("public.dmoney", TypeKind::domain("money")),
    ];
    assert_eq!(
        comparison_for("public.d1", None, &types, &[]),
        agrees(CompareKind::Int { bytes: 4 })
    );
    assert_eq!(
        comparison_for("public.d2", None, &types, &[]),
        agrees(CompareKind::Int { bytes: 4 })
    );
    assert_eq!(
        comparison_for("public.dtext", None, &types, &[]),
        ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::UnknownCollation
        ),
    );
    // A domain over a nested type is that type's nested comparison,
    // through the same recursion.
    assert_eq!(
        comparison_for("public.darr", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Array(Box::new(Position::nullable(
            NestedCompare::Leaf {
                declared: "integer".to_string(),
                kind: CompareKind::Int { bytes: 4 },
                divergence: None,
            }
        )))),
    );
    // A domain over something with no order here has none either.
    assert_eq!(comparison_for("public.dmoney", None, &types, &[]), ComparisonPlan::Refused);
}

/// A walk visits each definition at most once along a path, so the list's
/// length bounds it (I24): a chain using every entry still resolves, and a
/// cycle — which only a hand-edited file or an unfollowed rename makes —
/// answers the weaker `Unknown`, `Refused` and no extension rather than
/// exhausting the stack.
#[test]
fn a_type_walk_is_bounded_by_the_definitions_the_file_declares() {
    let chain: Vec<TypeDef> = (0..64)
        .map(|i| {
            let base = if i == 0 { "uuid".to_string() } else { format!("public.d{}", i - 1) };
            ty(&format!("public.d{i}"), TypeKind::domain(&base))
        })
        .collect();
    assert_eq!(
        resolve_declared_type("public.d63", &chain),
        TypeOutcome::Mapped(DataType::FixedSizeBinary(16), NestedPlan::Scalar)
    );
    assert_eq!(comparison_for("public.d63", None, &chain, &[]), agrees(CompareKind::Uuid));
    assert_eq!(extension_for("public.d63", &chain), Some(CanonicalExtension::Uuid));

    let cycle = [
        ty("public.a", TypeKind::domain("public.b")),
        ty("public.b", TypeKind::domain("public.a")),
    ];
    assert_eq!(resolve_declared_type("public.a", &cycle), TypeOutcome::Unknown);
    // An array over one keeps its level, its element held as text.
    assert_eq!(
        resolve_declared_type("public.b[]", &cycle),
        TypeOutcome::Mapped(list_of(DataType::Utf8View), NestedPlan::array(NestedPlan::Scalar))
    );
    assert_eq!(comparison_for("public.a", None, &cycle, &[]), ComparisonPlan::Refused);
    assert_eq!(extension_for("public.a", &cycle), None);

    // A composite holding itself terminates too, its innermost position
    // falling back to text as any unresolved one does.
    let holder = [ty(
        "public.holder",
        TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "public.holder[]")]) },
    )];
    assert!(matches!(
        resolve_declared_type("public.holder", &holder),
        TypeOutcome::Mapped(DataType::Struct(_), NestedPlan::Record(_))
    ));
    assert!(matches!(
        comparison_for("public.holder", None, &holder, &[]),
        ComparisonPlan::Nested(NestedCompare::Record(_))
    ));
}

/// A domain over an enum carries the enum's labels, through any chain —
/// which is what lets `pgdt info --detail` list them beneath such a
/// column and a `--filter` term name one. **No fixture column is one**,
/// so this is the only check of it.
///
/// A domain over an *empty* enum is refused like the enum itself: it
/// resolves to no Arrow type, so no column of it is ever asked.
#[test]
fn a_domain_over_an_enum_compares_by_that_enum_s_declaration_order() {
    let labels = ["sad".to_string(), "ok".to_string(), "happy".to_string()];
    let types = [
        ty("public.mood", TypeKind::Enum { labels: labels.to_vec(), exact: true }),
        ty("public.empty_enum", TypeKind::Enum { labels: Vec::new(), exact: true }),
        ty("public.moodish", TypeKind::domain("public.mood")),
        ty("public.moodisher", TypeKind::domain("public.moodish")),
        ty("public.nothingish", TypeKind::domain("public.empty_enum")),
    ];
    let by_declaration =
        agrees(CompareKind::Enum { labels: labels.iter().cloned().collect(), exact: true });
    assert_eq!(comparison_for("public.moodish", None, &types, &[]), by_declaration);
    assert_eq!(comparison_for("public.moodisher", None, &types, &[]), by_declaration);
    assert_eq!(comparison_for("public.nothingish", None, &types, &[]), ComparisonPlan::Refused);
}

/// Every nested shape the register compares, as one statement: an array,
/// a composite, a range and a range's multirange companion, each built by
/// the same walk so that nesting composes with no special case. A base or
/// shell type is refused for a reason of its own and is here to keep the
/// two populations from being read as one.
#[test]
fn every_container_kind_compares_structurally() {
    let types = [
        ty(
            "public.point2d",
            TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
        ),
        ty(
            "public.myrange",
            TypeKind::Range {
                subtype: Some("integer".to_string()),
                multirange_type_name: Some("public.myrange_multi".to_string()),
                canonical: None,
            },
        ),
        ty("public.gtype", TypeKind::base()),
        ty("public.forward", TypeKind::Shell),
    ];
    let int_leaf = || NestedCompare::Leaf {
        declared: "integer".to_string(),
        kind: CompareKind::Int { bytes: 4 },
        divergence: None,
    };
    let array_of_int =
        || ComparisonPlan::Nested(NestedCompare::Array(Box::new(Position::nullable(int_leaf()))));
    for declared in ["integer[]", "integer ARRAY"] {
        assert_eq!(comparison_for(declared, None, &types, &[]), array_of_int(), "{declared}");
    }
    // The element carries the column's own clause, because an array type
    // is not collatable and the clause is about its elements (I37).
    assert_eq!(
        comparison_for("text[]", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Array(Box::new(Position::nullable(
            NestedCompare::Leaf {
                declared: "text".to_string(),
                kind: CompareKind::Text { length: None },
                divergence: Some(ComparisonDivergence::UnknownCollation),
            }
        )))),
    );
    assert_eq!(
        comparison_for("text[]", Some("pg_catalog.\"C\""), &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Array(Box::new(Position::nullable(
            NestedCompare::Leaf {
                declared: "text".to_string(),
                kind: CompareKind::Text { length: None },
                divergence: None,
            }
        )))),
    );
    let point2d = || {
        ComparisonPlan::Nested(NestedCompare::Record(vec![(
            "x".to_string(),
            Position::nullable(int_leaf()),
        )]))
    };
    assert_eq!(comparison_for("public.point2d", None, &types, &[]), point2d());
    // Nesting composes with no special case: an array of composites is
    // the composite's own answer one level down.
    let ComparisonPlan::Nested(record) = point2d() else { unreachable!() };
    assert_eq!(
        comparison_for("public.point2d[]", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Array(Box::new(Position::nullable(record)))),
    );
    // A user-defined range and the companion multirange `pg_dump` writes
    // no `CREATE TYPE` for (I10) reach the same bound through two
    // different lookups, and neither canonicalizes.
    assert_eq!(
        comparison_for("public.myrange", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Range {
            bound: Box::new(int_leaf()),
            discrete: None,
        }),
    );
    assert_eq!(
        comparison_for("public.myrange_multi", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Multirange {
            bound: Box::new(int_leaf()),
            discrete: None,
        }),
    );
    // A built-in range is named nowhere in the file (I10), so its bound
    // and its canonicalization come off the hardcoded table — `discrete`
    // is a fact about the range type, so two ranges over `integer` differ.
    assert_eq!(
        comparison_for("int4range", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Range {
            bound: Box::new(int_leaf()),
            discrete: Some(Discrete::Int4),
        }),
    );
    assert_eq!(
        comparison_for("int4multirange", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Multirange {
            bound: Box::new(int_leaf()),
            discrete: Some(Discrete::Int4),
        }),
    );
    assert!(matches!(
        comparison_for("numrange", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Range { discrete: None, .. })
    ));
    // A domain over a range is the range's own answer, through the same
    // recursion every domain takes.
    let rangedom = [types[1].clone(), ty("public.rangedom", TypeKind::domain("public.myrange"))];
    assert_eq!(
        comparison_for("public.rangedom", None, &rangedom, &[]),
        comparison_for("public.myrange", None, &types, &[]),
    );
    // A range whose DDL stated no subtype has no bound type to name a
    // refusal after, so the column is refused outright.
    let subtypeless = [ty(
        "public.opaquerange",
        TypeKind::Range { subtype: None, multirange_type_name: None, canonical: None },
    )];
    assert_eq!(
        comparison_for("public.opaquerange", None, &subtypeless, &[]),
        ComparisonPlan::Refused,
    );
    for declared in ["public.gtype", "public.forward", "public.nosuchtype"] {
        assert_eq!(
            comparison_for(declared, None, &types, &[]),
            ComparisonPlan::Refused,
            "{declared}"
        );
    }
}

/// A range type declaring a `canonical` function is the one answer that
/// is neither a comparison nor a bytewise fallback: PostgreSQL rewrites
/// every value through arbitrary server-side code before storing or
/// comparing one (I46), so both operator families would be *wrong* rather
/// than weak.
///
/// **Unanswerability propagates where comparability is inherited**: such a
/// range short-circuits out of every walk that reaches it — an array of
/// it, a composite holding one, its multirange companion, a domain over
/// it — and each answers for the column instead.
#[test]
fn a_range_declaring_a_canonical_function_answers_no_operator() {
    let types = [
        ty(
            "public.canonrange",
            TypeKind::Range {
                subtype: Some("integer".to_string()),
                multirange_type_name: Some("public.canonrange_multi".to_string()),
                canonical: Some("public.canonrange_canonical".to_string()),
            },
        ),
        ty(
            "public.holder",
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("id", "integer"),
                    ColumnDef::new("span", "public.canonrange"),
                ]),
            },
        ),
        ty("public.canondom", TypeKind::domain("public.canonrange")),
    ];
    // The refusal names the *range* type and its function, whichever
    // route reached it: the companion has no DDL of its own to name (I10)
    // and a position inside a container is not where the parameter sits.
    let unanswerable = || {
        ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
            range_type: "public.canonrange".to_string(),
            function: "public.canonrange_canonical".to_string(),
        })
    };
    for declared in [
        "public.canonrange",
        "public.canonrange_multi",
        "public.canonrange[]",
        "public.holder",
        "public.holder[]",
        "public.canondom",
        "public.canondom[]",
    ] {
        assert_eq!(comparison_for(declared, None, &types, &[]), unanswerable(), "{declared}");
    }
    // Not an order, which is what the four ordering operators ask, and
    // not a `Refused` either, which is what lets `=` be refused with it.
    assert!(!unanswerable().orders());
    assert_ne!(unanswerable(), ComparisonPlan::Refused);
    // The same range without the parameter is an ordinary bound-wise
    // comparison, so the answer turns on the DDL and on nothing else.
    let TypeKind::Range { subtype, multirange_type_name, .. } = types[0].kind.clone() else {
        unreachable!()
    };
    let plain = [
        ty("public.canonrange", TypeKind::Range { subtype, multirange_type_name, canonical: None }),
        types[1].clone(),
        types[2].clone(),
    ];
    assert!(comparison_for("public.canonrange", None, &plain, &[]).orders());
    assert!(comparison_for("public.holder[]", None, &plain, &[]).orders());
}

/// Comparability is inherited: a position whose declared type has no
/// order refuses the whole column, and the tree says which position and
/// which type so the refusal can name them.
///
/// **`json` is the case that is not obvious**: bytewise at top level, and
/// inside a container nothing at all, because `array_cmp` looks up the
/// element type's comparison proc and there is none.
#[test]
fn a_position_with_no_order_refuses_the_whole_column() {
    let types = [
        ty(
            "public.jsonpair",
            TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("ok", "integer"), ColumnDef::new("doc", "json")]),
            },
        ),
        ty("public.gtype", TypeKind::base()),
    ];
    for (declared, path, at, announces) in [
        // `json` takes the server's `=` away with its order, so the
        // column's bytewise fallback is an answer it does not have.
        ("json[]", "[]", "json", true),
        ("public.jsonpair", ".doc", "json", true),
        ("public.jsonpair[]", "[].doc", "json", true),
        // I22: an opaque element type, refused for the delimiter as much
        // as for the order.
        ("box[]", "[]", "box", false),
        ("\"box\"[]", "[]", "\"box\"", false),
        ("public.gtype[]", "[]", "public.gtype", false),
        // An element this walk cannot find, `types` declaring no
        // `public.intarr`: refused as an unknown type. The fixture's own
        // `public.intarr`, an array element that is itself an array
        // (I26), is the oracle walk's (`predicate.rs`'s `REFUSED`).
        ("public.intarr[]", "[]", "public.intarr", false),
    ] {
        let plan = comparison_for(declared, None, &types, &[]);
        assert!(!plan.orders(), "{declared}");
        let ComparisonPlan::Nested(tree) = plan else { panic!("{declared}: not nested") };
        let divergence = announces.then_some(ComparisonDivergence::AsText);
        assert_eq!(
            tree.uncomparable(),
            Some((path.to_string(), at.to_string(), divergence)),
            "{declared}"
        );
        // The three refused for a reason of *this build's* announce
        // nothing: PostgreSQL compares `box[]` element-wise, and this
        // build knows neither `public.gtype`'s order nor, here,
        // `public.intarr`.
        let expected: Vec<_> = announces
            .then(|| (path.to_string(), at.to_string(), ComparisonDivergence::AsText))
            .into_iter()
            .collect();
        assert_eq!(tree.divergences(), expected, "{declared}");
    }
}

/// Every diverging position, in walk order, each with the path and the
/// declared type its sentence needs — a composite on the database's
/// collation twice, once directly and once through an element.
#[test]
fn a_nested_column_announces_every_diverging_position() {
    let types = [ty(
        "public.tagged",
        TypeKind::Composite {
            fields: Some(vec![
                ColumnDef::new("label", "text"),
                ColumnDef::new("n", "integer"),
                ColumnDef::new("tags", "text[]"),
            ]),
        },
    )];
    let ComparisonPlan::Nested(tree) = comparison_for("public.tagged", None, &types, &[]) else {
        panic!("a composite compares structurally")
    };
    assert_eq!(
        tree.divergences(),
        [
            (".label".to_string(), "text".to_string(), ComparisonDivergence::UnknownCollation),
            (".tags[]".to_string(), "text".to_string(), ComparisonDivergence::UnknownCollation),
        ],
    );
    // A field's own clause is what the register reads, not the column's:
    // a composite is not collatable, so there is no column clause to
    // inherit.
    let types = [ty(
        "public.pair",
        TypeKind::Composite {
            fields: Some(vec![
                ColumnDef::new("plain", "text"),
                ColumnDef {
                    name: "c".to_string(),
                    declared_type: "text".to_string(),
                    collation: Some("pg_catalog.\"C\"".to_string()),
                    not_null: None,
                },
            ]),
        },
    )];
    let ComparisonPlan::Nested(tree) = comparison_for("public.pair", None, &types, &[]) else {
        panic!("a composite compares structurally")
    };
    assert_eq!(
        tree.divergences(),
        [(".plain".to_string(), "text".to_string(), ComparisonDivergence::UnknownCollation)],
    );
}

/// `int2vector` is the one built-in whose Arrow type is a container and
/// whose declaration is not spelled like one, so its three answers are
/// pinned together: the type, the literal form, and the comparison.
#[test]
fn int2vector_is_a_list_with_a_literal_form_and_an_order_of_its_own() {
    let TypeOutcome::Mapped(data_type, plan) = resolve_declared_type("int2vector", &[]) else {
        panic!("int2vector maps")
    };
    assert_eq!(data_type, list_of(DataType::Int16));
    assert_eq!(plan, NestedPlan::Int2Vector);
    // The same Arrow type through the array spelling, and a different
    // literal form — the reason the plan travels beside the type.
    let TypeOutcome::Mapped(array_type, array_plan) = resolve_declared_type("smallint[]", &[])
    else {
        panic!("smallint[] maps")
    };
    assert_eq!(array_type, data_type);
    assert_eq!(array_plan, NestedPlan::array(NestedPlan::Scalar));

    let comparison = comparison_for("int2vector", None, &[], &[]);
    assert_eq!(comparison, ComparisonPlan::Nested(NestedCompare::Int2Vector));
    assert!(comparison.orders());
    let ComparisonPlan::Nested(tree) = &comparison else { panic!("nested") };
    assert_eq!(tree.uncomparable(), None);
    assert_eq!(tree.divergences(), []);

    // An array *of* them composes like any other element type: the outer
    // literal is `array_out`'s and each element is a vector.
    let ComparisonPlan::Nested(outer) = comparison_for("int2vector[]", None, &[], &[]) else {
        panic!("int2vector[] compares structurally")
    };
    assert_eq!(
        outer,
        NestedCompare::Array(Box::new(Position::nullable(NestedCompare::Int2Vector)))
    );
}

/// A type name needing quotes resolves as an ordinary one does (I29): the
/// definition and the declaration are compared in one spelling, so every
/// column of I29's own dump finds its type, and case decides which of two
/// names differing only in it a spelling means.
#[test]
fn a_quoted_type_name_resolves_as_an_ordinary_one() {
    let enum_type = DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
    let types = [
        ty(r#"s."d[3]""#, TypeKind::domain("integer")),
        ty(r#"s."my type""#, TypeKind::domain("integer")),
        ty(r#"s."weird[]""#, TypeKind::Enum { labels: vec!["a".into(), "b".into()], exact: true }),
        ty(r#"s."x ARRAY""#, TypeKind::domain("integer")),
        ty(r#"s."Mood""#, TypeKind::domain("uuid")),
        ty("s.mood", TypeKind::domain("text")),
        // Written by hand, in a spelling other than the canonical one.
        ty(r#""s"."hand""#, TypeKind::domain("boolean")),
    ];
    let int = TypeOutcome::Mapped(DataType::Int32, NestedPlan::Scalar);
    let int_list =
        TypeOutcome::Mapped(list_of(DataType::Int32), NestedPlan::array(NestedPlan::Scalar));
    for (declared, outcome) in [
        (r#"s."weird[]""#, TypeOutcome::Mapped(enum_type.clone(), NestedPlan::Scalar)),
        (r#"s."my type""#, int.clone()),
        (r#"s."x ARRAY""#, int.clone()),
        (r#"s."d[3]""#, int.clone()),
        (
            r#"s."weird[]"[]"#,
            TypeOutcome::Mapped(list_of(enum_type), NestedPlan::array(NestedPlan::Scalar)),
        ),
        (r#"s."my type"[]"#, int_list.clone()),
        (r#"s."x ARRAY"[]"#, int_list),
        (r#"S."Mood""#, TypeOutcome::Mapped(DataType::FixedSizeBinary(16), NestedPlan::Scalar)),
        ("s.Mood", TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)),
        ("s.hand", TypeOutcome::Mapped(DataType::Boolean, NestedPlan::Scalar)),
        (r#"s."MOOD""#, TypeOutcome::Unknown),
    ] {
        assert_eq!(resolve_declared_type(declared, &types), outcome, "{declared}");
    }
    assert_eq!(
        comparison_for(r#"s."x ARRAY""#, None, &types, &[]),
        agrees(CompareKind::Int { bytes: 4 })
    );
    assert_eq!(extension_for(r#"s."Mood""#, &types), Some(CanonicalExtension::Uuid));
    assert_eq!(extension_for("s.mood", &types), None);

    // A multirange companion is found through its range under any spelling.
    let ranges = [ty(
        r#"s."R""#,
        TypeKind::Range {
            subtype: Some("integer".into()),
            multirange_type_name: Some(r#"s."R_multi""#.into()),
            canonical: None,
        },
    )];
    assert!(matches!(
        resolve_declared_type(r#""s"."R_multi""#, &ranges),
        TypeOutcome::Mapped(_, NestedPlan::Multirange(_))
    ));
}

/// **A domain's `NOT NULL` binds every domain over it, and no array of
/// it** (I76), found in any spelling of its name; a cycle ends.
#[test]
fn a_domain_not_null_binds_the_domains_over_it() {
    let not_null = |base: &str| TypeKind::Domain {
        base_type: base.into(),
        collation: None,
        not_null: true,
        check: false,
    };
    let types = [
        ty("public.nn", not_null("integer")),
        ty("public.over", TypeKind::domain("PUBLIC.nn")),
        ty("public.plain", TypeKind::domain("integer")),
        ty("public.a", TypeKind::domain("public.b")),
        ty("public.b", TypeKind::domain("public.a")),
    ];
    assert!(domain_not_null("public.nn", &types));
    assert!(domain_not_null("public.over", &types));
    assert!(domain_not_null(r#""public"."over""#, &types));
    assert!(!domain_not_null("public.nn[]", &types));
    assert!(!domain_not_null("public.plain", &types));
    assert!(!domain_not_null("integer", &types));
    assert!(!domain_not_null("public.a", &types));
}

/// **A `NOT NULL` domain's position refuses a NULL in both plans** — an
/// array's element and a composite's field, through a domain over it and
/// beneath another container — and a range's bound, which `range_in`
/// hands no infinite bound, does not (I76).
#[test]
fn a_not_null_domain_beneath_a_container_marks_its_position() {
    let not_null = |base: &str| TypeKind::Domain {
        base_type: base.into(),
        collation: None,
        not_null: true,
        check: false,
    };
    let types = [
        ty("public.nn", not_null("integer")),
        ty("public.over", TypeKind::domain("public.nn")),
        ty("public.nnbox", not_null("box")),
        ty("public.nncomp", not_null("public.comp")),
        ty(
            "public.comp",
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("a", "public.over"),
                    ColumnDef::new("b", "integer"),
                ]),
            },
        ),
        ty(
            "public.nnrange",
            TypeKind::Range {
                subtype: Some("public.nn".into()),
                multirange_type_name: None,
                canonical: None,
            },
        ),
    ];
    let plan = |declared| match resolve_declared_type(declared, &types) {
        TypeOutcome::Mapped(_, plan) => plan,
        other => panic!("`{declared}` is {other:?}"),
    };
    let compare = |declared| match comparison_for(declared, None, &types, &[]) {
        ComparisonPlan::Nested(tree) => tree,
        other => panic!("`{declared}` is {other:?}"),
    };
    let leaf = |declared: &str| NestedCompare::Leaf {
        declared: declared.into(),
        kind: CompareKind::Int { bytes: 4 },
        divergence: None,
    };
    fn refusing<T>(plan: T) -> Position<T> {
        Position { plan, not_null: true }
    }

    assert_eq!(plan("public.nn[]"), NestedPlan::Array(Box::new(refusing(NestedPlan::Scalar))));
    assert_eq!(plan("integer[]"), NestedPlan::array(NestedPlan::Scalar));
    let comp = NestedPlan::Record(vec![
        refusing(NestedPlan::Scalar),
        Position::nullable(NestedPlan::Scalar),
    ]);
    assert_eq!(plan("public.comp"), comp);
    assert_eq!(plan("public.nncomp[]"), NestedPlan::Array(Box::new(refusing(comp.clone()))));
    assert_eq!(plan("public.comp[]"), NestedPlan::array(comp));
    assert_eq!(plan("public.nnrange"), NestedPlan::Range(Box::new(NestedPlan::Scalar)));

    assert_eq!(compare("public.nn[]"), NestedCompare::Array(Box::new(refusing(leaf("public.nn")))));
    let comp = NestedCompare::Record(vec![
        ("a".to_string(), refusing(leaf("public.over"))),
        ("b".to_string(), Position::nullable(leaf("integer"))),
    ]);
    assert_eq!(compare("public.comp"), comp);
    assert_eq!(compare("public.nncomp[]"), NestedCompare::Array(Box::new(refusing(comp))));
    assert_eq!(
        compare("public.nnrange"),
        NestedCompare::Range { bound: Box::new(leaf("public.nn")), discrete: None }
    );
    assert_eq!(
        text_grammar("public.nnbox[]", &types),
        Some(TextGrammar::BoxArray { not_null: true })
    );
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
    let ComparisonPlan::Nested(NestedCompare::Array(element)) =
        comparison_for("xid8[]", None, &types, &[])
    else {
        panic!("an array of `xid8` is nested");
    };
    assert!(
        matches!(element.plan, NestedCompare::Uncomparable { grammar, .. } if grammar == nothing)
    );
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

/// Two spellings of one name are one definition, so a chain written with
/// both reaches its base; a cycle only case could make — a domain over
/// its own name spelled unquoted — is no cycle, the unquoted spelling
/// naming another type.
#[test]
fn spellings_of_one_name_are_one_definition() {
    let types = [
        ty(r#"public."D""#, TypeKind::domain("public.d")),
        ty("public.d", TypeKind::domain(r#""public"."E""#)),
        ty(r#"public."E""#, TypeKind::domain("uuid")),
    ];
    let uuid = TypeOutcome::Mapped(DataType::FixedSizeBinary(16), NestedPlan::Scalar);
    assert_eq!(resolve_declared_type(r#"public."D""#, &types), uuid);
    assert_eq!(resolve_declared_type("PUBLIC.D", &types), uuid);
    assert_eq!(comparison_for(r#"public."D""#, None, &types, &[]), agrees(CompareKind::Uuid));
}

/// A collation a dump declares under a quoted name is found by the
/// clause that states it, so its non-determinism is announced rather
/// than read as another non-bytewise order (I42) — and a declared
/// `public."C"` keeps an unqualified `"C"` from being called bytewise.
#[test]
fn a_quoted_collation_name_is_found_by_its_clause() {
    let declared = [CollationDef { name: r#"public."CI""#.to_string(), deterministic: false }];
    assert_eq!(
        comparison_for("text", Some(r#"public."CI""#), &[], &declared),
        ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::NonDeterministicCollation
        )
    );
    let c = [CollationDef { name: r#"public."C""#.to_string(), deterministic: true }];
    assert_ne!(
        comparison_for("text", Some(r#""C""#), &[], &c),
        agrees(CompareKind::Text { length: None })
    );
}
