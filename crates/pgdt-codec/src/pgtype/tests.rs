use arrow_schema::TimeUnit;

use super::*;
use crate::compare::{TextGrammar, comparison_for};
use crate::declared::domain_not_null;
use crate::grammar::text_grammar;
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
    assert_eq!(
        comparison_for("public.d63", None, &chain, &[]),
        ComparisonPlan::agrees(CompareKind::Uuid)
    );
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
        ComparisonPlan::agrees(CompareKind::Int { bytes: 4 })
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
    assert_eq!(
        comparison_for(r#"public."D""#, None, &types, &[]),
        ComparisonPlan::agrees(CompareKind::Uuid)
    );
}
