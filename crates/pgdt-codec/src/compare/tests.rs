use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;

fn ty(name: &str, kind: TypeKind) -> TypeDef {
    TypeDef { name: name.to_string(), kind }
}

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
        ("text", unknown(K::Text { length: None, varying: false })),
        ("character varying(10)", unknown(K::Text { length: Some(10), varying: true })),
        ("character varying", unknown(K::Text { length: None, varying: true })),
        ("character(10)", unknown(K::PaddedText { length: Some(10) })),
        ("character", unknown(K::PaddedText { length: Some(1) })),
        ("bpchar", unknown(K::PaddedText { length: None })),
        ("name", agrees(K::Text { length: None, varying: false })),
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
            name: "public.mood".into(),
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
            CompareKind::Text { length: None, varying: false },
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
    let text = || Some(CompareKind::Text { length: None, varying: false });
    let read = |plan: &ComparisonPlan| {
        (plan.bounds_kinds(), plan.bounds_in(Postgres), plan.bounds_in(DataFusion))
    };
    assert_eq!(
        read(&ComparisonPlan::agrees(CompareKind::Text { length: None, varying: false })),
        ([text(), None], Some(P), Some(P))
    );
    for divergence in [AsText, UnknownCollation, NonBytewiseCollation, NonDeterministicCollation] {
        let plan = ComparisonPlan::diverging(
            CompareKind::Text { length: None, varying: false },
            divergence,
        );
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
        CompareKind::Enum { name: "public.e".into(), labels, exact: true },
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
    let text = CompareKind::Text { length: None, varying: false };
    let refused = ComparisonPlan::Refused.bounds_read_by(ComparisonSemantics::DataFusion);
    assert_eq!(refused.as_ref(), Some(&text));
    assert_eq!(ComparisonPlan::Refused.bounds_read_by(ComparisonSemantics::Postgres), None);
    let labels: Arc<[String]> = Arc::from(vec!["b".to_string()]);
    for (declared, stored, want) in [
        ("text", [Some(text.clone()), None], Some(P)),
        (
            "an enum",
            [
                Some(CompareKind::Enum { name: "public.e".into(), labels, exact: true }),
                Some(text.clone()),
            ],
            Some(F),
        ),
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
    let agrees_text = || ComparisonPlan::agrees(CompareKind::Text { length: None, varying: false });
    let unknown = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None, varying: false },
            ComparisonDivergence::UnknownCollation,
        )
    };
    let named = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None, varying: false },
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
                CompareKind::Text { length: Some(10), varying: true },
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
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, named)
    );
    // Declared deterministic moves nothing either.
    assert_eq!(
        comparison_for("text", Some("public.icu_ci"), &[], &[coll("public.icu_ci", true)]),
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, named)
    );
    // Declared non-deterministic: same comparison, stronger verdict.
    let declared = [coll("public.other", true), coll("public.icu_ci", false)];
    assert_eq!(
        comparison_for("text", Some("public.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, nd)
    );
    // `character(n)` and `character varying` reach the same branch, and
    // `character(n)` keeps its own trim (I38).
    assert_eq!(
        comparison_for("character(10)", Some("public.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::PaddedText { length: Some(10) }, nd)
    );
    assert_eq!(
        comparison_for("character varying(10)", Some("public.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: Some(10), varying: true }, nd)
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
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, nd)
    );
    // A column of a *different* collation is untouched by the entry.
    assert_eq!(
        comparison_for("text", Some("public.other"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, named)
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
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, nd)
    );
    // An unqualified reference matches on the name alone — the
    // announcing direction, since a file's search path is not read.
    assert_eq!(
        comparison_for("text", Some("icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, nd)
    );
    // ... but a *qualified* reference must agree on the schema.
    assert_eq!(
        comparison_for("text", Some("elsewhere.icu_ci"), &[], &declared),
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, named)
    );
    // An unqualified `"C"` is the built-in unless the dump declares one of
    // that name, which a path naming `pg_catalog` after it puts first (`KD48`):
    // then it is the weaker verdict, and never both branches at once.
    let c = ComparisonPlan::agrees(K::Text { length: None, varying: false });
    assert_eq!(comparison_for("text", Some("\"C\""), &[], &declared), c);
    assert_eq!(
        comparison_for("text", Some("pg_catalog.\"C\""), &[], &[coll("public.\"C\"", true)]),
        c
    );
    assert_eq!(
        comparison_for("text", Some("\"C\""), &[], &[coll("public.\"C\"", true)]),
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, named)
    );
    assert_eq!(
        comparison_for("text", Some("\"POSIX\""), &[], &[coll("public.\"POSIX\"", false)]),
        ComparisonPlan::diverging(K::Text { length: None, varying: false }, nd)
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
    let agrees_text = || ComparisonPlan::agrees(CompareKind::Text { length: None, varying: false });
    let unknown = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None, varying: false },
            ComparisonDivergence::UnknownCollation,
        )
    };
    let named = || {
        ComparisonPlan::diverging(
            CompareKind::Text { length: None, varying: false },
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
            CompareKind::Text { length: None, varying: false },
            ComparisonDivergence::UnknownCollation
        ),
    );
    // A domain over a nested type is that type's nested comparison,
    // through the same recursion.
    assert_eq!(
        comparison_for("public.darr", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Array {
            declared: "integer[]".to_string(),
            element: Box::new(Position::nullable(NestedCompare::Leaf {
                declared: "integer".to_string(),
                kind: CompareKind::Int { bytes: 4 },
                divergence: None,
            })),
        }),
    );
    // A domain over something with no order here has none either.
    assert_eq!(comparison_for("public.dmoney", None, &types, &[]), ComparisonPlan::Refused);
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
    let by_declaration = agrees(CompareKind::Enum {
        name: "public.mood".into(),
        labels: labels.iter().cloned().collect(),
        exact: true,
    });
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
    let array = |declared: &str, element| NestedCompare::Array {
        declared: declared.to_string(),
        element: Box::new(Position::nullable(element)),
    };
    for declared in ["integer[]", "integer ARRAY"] {
        assert_eq!(
            comparison_for(declared, None, &types, &[]),
            ComparisonPlan::Nested(array(declared, int_leaf())),
            "{declared}"
        );
    }
    // The element carries the column's own clause, because an array type
    // is not collatable and the clause is about its elements (I37).
    assert_eq!(
        comparison_for("text[]", None, &types, &[]),
        ComparisonPlan::Nested(array(
            "text[]",
            NestedCompare::Leaf {
                declared: "text".to_string(),
                kind: CompareKind::Text { length: None, varying: false },
                divergence: Some(ComparisonDivergence::UnknownCollation),
            }
        )),
    );
    assert_eq!(
        comparison_for("text[]", Some("pg_catalog.\"C\""), &types, &[]),
        ComparisonPlan::Nested(array(
            "text[]",
            NestedCompare::Leaf {
                declared: "text".to_string(),
                kind: CompareKind::Text { length: None, varying: false },
                divergence: None,
            }
        )),
    );
    let point2d = || {
        ComparisonPlan::Nested(NestedCompare::Record {
            declared: "public.point2d".to_string(),
            fields: vec![("x".to_string(), Position::nullable(int_leaf()))],
        })
    };
    assert_eq!(comparison_for("public.point2d", None, &types, &[]), point2d());
    // Nesting composes with no special case: an array of composites is
    // the composite's own answer one level down.
    let ComparisonPlan::Nested(record) = point2d() else { unreachable!() };
    assert_eq!(
        comparison_for("public.point2d[]", None, &types, &[]),
        ComparisonPlan::Nested(array("public.point2d[]", record)),
    );
    // A user-defined range and the companion multirange `pg_dump` writes
    // no `CREATE TYPE` for (I10) reach the same bound through two
    // different lookups, and neither canonicalizes.
    assert_eq!(
        comparison_for("public.myrange", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Range {
            declared: "public.myrange".to_string(),
            bound: Box::new(int_leaf()),
            discrete: None,
        }),
    );
    assert_eq!(
        comparison_for("public.myrange_multi", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Multirange {
            declared: "public.myrange_multi".to_string(),
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
            declared: "int4range".to_string(),
            bound: Box::new(int_leaf()),
            discrete: Some(Discrete::Int4),
        }),
    );
    assert_eq!(
        comparison_for("int4multirange", None, &types, &[]),
        ComparisonPlan::Nested(NestedCompare::Multirange {
            declared: "int4multirange".to_string(),
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
        // (I26), is the oracle walk's (pgdt-query's `predicate/tests.rs`, `REFUSED`).
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
            CompareKind::Text { length: None, varying: false },
            ComparisonDivergence::NonDeterministicCollation
        )
    );
    let c = [CollationDef { name: r#"public."C""#.to_string(), deterministic: true }];
    assert_ne!(
        comparison_for("text", Some(r#""C""#), &[], &c),
        agrees(CompareKind::Text { length: None, varying: false })
    );
}
