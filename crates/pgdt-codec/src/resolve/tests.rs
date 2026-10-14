use super::*;
use pgdt_model::internal::DiagnosticExt;
use pgdt_model::{ColumnDef, DatabaseMetadata, TableDef, TypeDef, TypeKind};

fn one_db(tables: &[(&str, &[(&str, &str)])], types: Vec<TypeDef>) -> DumpMetadata {
    let mut db = DatabaseMetadata {
        name: None,
        preamble_complete: true,
        server_version: None,
        pg_dump_version: None,
        extensions: Vec::new(),
        types,
        collations: Vec::new(),
        tables: Default::default(),
    };
    for (name, cols) in tables {
        db.tables.insert(
            name.to_string(),
            TableDef::with_columns(cols.iter().map(|(c, t)| ColumnDef::new(*c, *t)).collect()),
        );
    }
    DumpMetadata { databases: vec![db] }
}

#[test]
fn mapped_and_not_declared_columns() {
    let meta = one_db(&[("public.t", &[("id", "integer"), ("name", "text")])], vec![]);
    let cols = vec!["id".to_string(), "name".to_string(), "extra".to_string()];
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert_eq!(
        resolved.columns,
        [ColumnResolution::Mapped, ColumnResolution::Mapped, ColumnResolution::NotDeclared,]
    );
    assert_eq!(resolved.schema.field(0).data_type(), &arrow_schema::DataType::Int32);
    assert_eq!(resolved.schema.field(1).data_type(), &arrow_schema::DataType::Utf8View);
    assert_eq!(resolved.schema.field(2).data_type(), &arrow_schema::DataType::Utf8View);
    assert!(resolved.schema.fields().iter().all(|f| f.is_nullable()));
    assert_eq!(resolved.unmapped_count(), 1);
}

#[test]
fn strings_mode_never_looks_up_ddl() {
    let meta = one_db(&[("public.t", &[("id", "integer")])], vec![]);
    let cols = vec!["id".to_string()];
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Strings, &[]);
    assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
    assert_eq!(resolved.schema.field(0).data_type(), &arrow_schema::DataType::Utf8View);
}

#[test]
fn no_metadata_at_all_is_every_column_not_declared() {
    let cols = vec!["id".to_string()];
    let resolved = resolve_columns("public.t", &cols, None, None, SchemaMode::Typed, &[]);
    assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
}

/// A block attributed to a database the metadata does not cover is
/// `MetadataNotScanned`, not `NotDeclared`: one is final, the other says
/// "finish the parse and ask again".
///
/// With *no* metadata at all there is no scan to finish, so `NotDeclared`
/// stands — the third case below.
#[test]
fn a_database_the_scan_never_reached_is_told_apart_from_undeclared_columns() {
    let mut first = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
    first.name = Some("first".to_string());
    let meta = DumpMetadata { databases: vec![first] };
    let cols = vec!["id".to_string()];

    let later =
        resolve_columns("public.t", &cols, Some(&meta), Some("second"), SchemaMode::Typed, &[]);
    assert_eq!(later.columns, [ColumnResolution::MetadataNotScanned]);
    assert_eq!(later.schema.field(0).data_type(), &arrow_schema::DataType::Utf8View);
    assert_eq!(later.notes[0].severity(), Severity::Warning);

    // The same metadata, asked about a table the database it *did* read
    // never declared: final, and says so.
    let undeclared =
        resolve_columns("public.absent", &cols, Some(&meta), Some("first"), SchemaMode::Typed, &[]);
    assert_eq!(undeclared.columns, [ColumnResolution::NotDeclared]);
}

/// A database entry that exists but is not `preamble_complete` is the same
/// case: the scan reached it and stopped inside it, so whatever DDL it
/// holds is a fragment rather than an answer.
#[test]
fn an_incomplete_database_entry_is_also_metadata_not_scanned() {
    let mut db = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
    db.preamble_complete = false;
    let meta = DumpMetadata { databases: vec![db] };
    let cols = vec!["id".to_string(), "other".to_string()];
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    // The column the fragment *does* declare still resolves; only the
    // unexplained one carries the "ask again later" outcome.
    assert_eq!(resolved.columns, [ColumnResolution::Mapped, ColumnResolution::MetadataNotScanned]);
}

/// `SchemaMode::Strings` never looks, so it never reaches this outcome
/// either — every column is `NotDeclared`, matching the untyped path
/// exactly.
#[test]
fn strings_mode_never_reports_metadata_not_scanned() {
    let mut first = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
    first.name = Some("first".to_string());
    let meta = DumpMetadata { databases: vec![first] };
    let cols = vec!["id".to_string()];
    let resolved =
        resolve_columns("public.t", &cols, Some(&meta), Some("second"), SchemaMode::Strings, &[]);
    assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
}

/// Every non-`Mapped` outcome `crate::pgtype` can produce but
/// `NestedArrayElement`, seen through the join — and the plan a `Utf8View` fallback carries, which is
/// `Scalar` whichever reason put it there.
#[test]
fn unknown_opaque_and_empty_enum_outcomes() {
    let types = vec![
        TypeDef { name: "public.gtype".to_string(), kind: TypeKind::base() },
        TypeDef {
            name: "public.mood".to_string(),
            kind: TypeKind::Enum { labels: vec![], exact: true },
        },
    ];
    let meta = one_db(
        &[(
            "public.t",
            &[("a", "money"), ("b", "public.gtype[]"), ("c", "public.gtype"), ("d", "public.mood")],
        )],
        types,
    );
    let cols = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert_eq!(
        resolved.columns,
        [
            ColumnResolution::UnknownType,
            ColumnResolution::OpaqueElementType,
            ColumnResolution::OpaqueBaseType,
            ColumnResolution::EmptyEnum,
        ]
    );
    assert!(
        resolved.schema.fields().iter().all(|f| *f.data_type() == arrow_schema::DataType::Utf8View),
        "every unmapped outcome falls back to text"
    );
    assert_eq!(resolved.plans, vec![NestedPlan::Scalar; 4]);
}

/// The `(DataType, NestedPlan)` pair reaches the schema positionally,
/// with one entry per column whether or not the column is nested — so no
/// consumer has to ask whether `plans` applies to it.
#[test]
fn a_nested_column_carries_its_plan_beside_its_type() {
    let types = vec![TypeDef {
        name: "public.point2d".to_string(),
        kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
    }];
    let meta = one_db(
        &[("public.t", &[("id", "integer"), ("v", "integer[]"), ("p", "public.point2d")])],
        types,
    );
    let cols = vec!["id".to_string(), "v".to_string(), "p".to_string()];
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped));
    assert_eq!(
        resolved.plans,
        [
            NestedPlan::Scalar,
            NestedPlan::array(NestedPlan::Scalar),
            NestedPlan::record(vec![NestedPlan::Scalar]),
        ]
    );
    assert_eq!(resolved.plans.len(), resolved.schema.fields().len());
}

fn shape(dims: Option<(u8, u8)>, lower_bound_prefix: bool) -> ArrayShape {
    ArrayShape { dims, lower_bound_prefix }
}

/// One table with one array column, resolved against `census`.
fn array_column(census: &[ArrayShape]) -> ResolvedSchema {
    let meta = one_db(&[("public.t", &[("v", "integer[]")])], vec![]);
    let cols = vec!["v".to_string()];
    resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, census)
}

fn list_of(child: arrow_schema::DataType) -> arrow_schema::DataType {
    arrow_schema::DataType::List(Arc::new(Field::new("item", child, true)))
}

/// Uniform depth retypes the pair — both halves, in step
/// (`docs/design/decisions.md`, "D35").
#[test]
fn a_uniformly_deep_array_column_gains_a_list_level_per_dimension() {
    use arrow_schema::DataType::Int32;
    for (depth, expected) in
        [(1, list_of(Int32)), (2, list_of(list_of(Int32))), (3, list_of(list_of(list_of(Int32))))]
    {
        let resolved = array_column(&[shape(Some((depth, depth)), false)]);
        assert_eq!(resolved.schema.field(0).data_type(), &expected, "depth {depth}");
        assert_eq!(resolved.columns, [ColumnResolution::Mapped], "depth {depth}");
        let mut plan = NestedPlan::Scalar;
        for _ in 0..depth {
            plan = NestedPlan::array(plan);
        }
        assert_eq!(resolved.plans, [plan], "depth {depth}");
    }
}

/// The two shapes no `List` type is honest about, and the one that
/// constrains nothing at all.
#[test]
fn a_varying_or_decorated_array_column_becomes_text_and_says_why() {
    for (census, why) in [
        (shape(Some((1, 2)), false), "mixed dimensionality"),
        (shape(Some((1, 1)), true), "a lower-bound prefix, however uniform"),
    ] {
        let resolved = array_column(&[census]);
        assert_eq!(resolved.columns, [ColumnResolution::VaryingArrayShape], "{why}");
        assert_eq!(
            resolved.schema.field(0).data_type(),
            &arrow_schema::DataType::Utf8View,
            "{why}"
        );
        assert_eq!(resolved.plans, [NestedPlan::Scalar], "{why}");
        // Still a `Warning` on the shared scale, like every other
        // fallback to text.
        assert_eq!(resolved.notes[0].severity(), Severity::Warning, "{why}");
    }

    // Every value was NULL or `{}`: nothing was constrained, so the
    // optimistic type stands.
    let unconstrained = array_column(&[shape(None, false)]);
    assert_eq!(unconstrained.columns, [ColumnResolution::Mapped]);
    assert_eq!(unconstrained.schema.field(0).data_type(), &list_of(arrow_schema::DataType::Int32));
}

/// A leading brace run longer than `MAXDIM` did not come out of
/// `array_out` (I25), so it is not evidence about an array: the column
/// keeps its optimistic type and the offending row is a `FieldDecode`
/// like any other value contradicting its declared type.
#[test]
fn a_brace_run_past_the_dimension_limit_is_not_evidence() {
    for dims in [(7, 7), (200, 200), (1, 200)] {
        let resolved = array_column(&[shape(Some(dims), false)]);
        assert_eq!(resolved.columns, [ColumnResolution::Mapped], "{dims:?}");
        assert_eq!(
            resolved.schema.field(0).data_type(),
            &list_of(arrow_schema::DataType::Int32),
            "{dims:?}"
        );
    }
}

/// The census is type-blind and keyed by column, so it constrains exactly
/// one thing: a column the DDL resolved to an array. A composite and a
/// multirange (whose `List` is filled by `multirange_out`, not
/// `array_out`) read the same census entry and must ignore it.
///
/// The third column completes the list: a plan already nested — the one
/// shape whose census would have to be *disbelieved* rather than ignored
/// — cannot reach the transform, because an array-typed element is refused
/// at resolution (I26), and `integer[][]` is a spelling of `integer[]`
/// (I28) the census is free to deepen.
#[test]
fn the_census_only_speaks_for_a_column_the_ddl_resolved_to_an_array() {
    let types = vec![
        TypeDef {
            name: "public.point2d".to_string(),
            kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
        },
        TypeDef { name: "public.intarr".to_string(), kind: TypeKind::domain("integer[]") },
    ];
    let meta = one_db(
        &[(
            "public.t",
            &[("c", "public.point2d"), ("m", "int4multirange"), ("n", "public.intarr[]")],
        )],
        types,
    );
    let cols = vec!["c".to_string(), "m".to_string(), "n".to_string()];
    let census = vec![shape(Some((2, 2)), true); 3];
    let resolved =
        resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &census);
    assert_eq!(
        resolved.columns,
        [ColumnResolution::Mapped, ColumnResolution::Mapped, ColumnResolution::NestedArrayElement]
    );
    let untouched = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert_eq!(resolved.schema, untouched.schema);
    assert_eq!(resolved.plans, untouched.plans);
}

/// A census shorter than the column list — one ending at the highest
/// field that ever held a brace — leaves the columns
/// past its end exactly as the DDL resolved them.
#[test]
fn a_census_shorter_than_the_column_list_constrains_only_what_it_covers() {
    let meta = one_db(&[("public.t", &[("a", "integer[]"), ("b", "integer[]")])], vec![]);
    let cols = vec!["a".to_string(), "b".to_string()];
    let resolved = resolve_columns(
        "public.t",
        &cols,
        Some(&meta),
        None,
        SchemaMode::Typed,
        &[shape(Some((1, 2)), false)],
    );
    assert_eq!(resolved.columns, [ColumnResolution::VaryingArrayShape, ColumnResolution::Mapped]);
    assert_eq!(resolved.schema.field(1).data_type(), &list_of(arrow_schema::DataType::Int32));
}

/// The comparison plan is a fourth positional vector, one entry per column
/// whether or not the column has an order — so no consumer has to ask
/// whether it applies — and its value is the register's own answer.
#[test]
fn every_column_carries_a_comparison_plan() {
    use crate::compare::{CompareKind, ComparisonPlan};

    let types = vec![TypeDef {
        name: "public.mood".to_string(),
        kind: TypeKind::Enum { labels: vec!["sad".to_string()], exact: true },
    }];
    let meta = one_db(
        &[(
            "public.t",
            &[
                ("i", "integer"),
                ("s", "text"),
                ("m", "public.mood"),
                ("v", "integer[]"),
                ("u", "money"),
            ],
        )],
        types,
    );
    let cols: Vec<String> = ["i", "s", "m", "v", "u"].iter().map(ToString::to_string).collect();
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert_eq!(resolved.comparisons.len(), resolved.schema.fields().len());
    assert_eq!(
        resolved.comparisons,
        [
            ComparisonPlan::Compared { kind: CompareKind::Int { bytes: 4 }, divergence: None },
            ComparisonPlan::diverging(
                CompareKind::Text { length: None },
                crate::compare::ComparisonDivergence::UnknownCollation,
            ),
            ComparisonPlan::Compared {
                kind: CompareKind::Enum {
                    labels: ["sad".to_string()].into_iter().collect(),
                    exact: true
                },
                divergence: None,
            },
            // Nested: compared structurally, one node per level.
            ComparisonPlan::Nested(crate::compare::NestedCompare::Array(Box::new(
                crate::pgtype::Position::nullable(crate::compare::NestedCompare::Leaf {
                    declared: "integer".to_string(),
                    kind: CompareKind::Int { bytes: 4 },
                    divergence: None,
                },)
            ))),
            // A type this build never mapped has no order here.
            ComparisonPlan::Refused,
        ]
    );

    // `SchemaMode::Strings` resolves nothing, so it claims no order for
    // any column — which is what makes an ordering operator refuse.
    let strings = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Strings, &[]);
    assert_eq!(strings.comparisons, vec![ComparisonPlan::Refused; 5]);
}

/// The register is consulted **per column**, not per declared type: two
/// `text` columns of one table get different verdicts, because the
/// `COLLATE` clause is half the question and is a fact about the column.
#[test]
fn two_text_columns_of_one_table_can_compare_differently() {
    use crate::compare::{CompareKind, ComparisonDivergence};

    let mut meta = one_db(&[("public.t", &[("plain", "text"), ("bytewise", "text")])], vec![]);
    meta.databases[0].tables.get_mut("public.t").unwrap().columns[1].collation =
        Some("pg_catalog.\"C\"".to_string());

    let cols = vec!["plain".to_string(), "bytewise".to_string()];
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert_eq!(
        resolved.comparisons,
        [
            ComparisonPlan::diverging(
                CompareKind::Text { length: None },
                ComparisonDivergence::UnknownCollation
            ),
            ComparisonPlan::Compared { kind: CompareKind::Text { length: None }, divergence: None },
        ]
    );
    // Both are still the same Arrow type and the same comparison — only
    // the verdict moved.
    assert_eq!(resolved.schema.field(0).data_type(), resolved.schema.field(1).data_type());
}

/// The other half of the same join: the register also reads the database's
/// own `CREATE COLLATION` list, because a `COLLATE` clause naming a
/// collation the dump declares `deterministic = false` is the one equality
/// divergence a plain dump states outright (I42). What moves is what the
/// whole dump said about that name, not the clause.
#[test]
fn the_dumps_own_collation_list_decides_whether_a_clause_is_deterministic() {
    use crate::compare::{CompareKind, ComparisonDivergence};
    use pgdt_model::CollationDef;

    let build = |deterministic: bool| {
        let mut meta = one_db(&[("public.t", &[("v", "text")])], vec![]);
        meta.databases[0].tables.get_mut("public.t").unwrap().columns[0].collation =
            Some("public.icu_ci".to_string());
        meta.databases[0].collations =
            vec![CollationDef { name: "public.icu_ci".to_string(), deterministic }];
        let cols = vec!["v".to_string()];
        resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[])
    };

    assert_eq!(
        build(true).comparisons,
        [ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::NonBytewiseCollation
        )]
    );
    assert_eq!(
        build(false).comparisons,
        [ComparisonPlan::diverging(
            CompareKind::Text { length: None },
            ComparisonDivergence::NonDeterministicCollation
        )]
    );
}

/// The census speaks after the declared type and can take a column out of
/// `Mapped` once the plan has been read — so the plan is re-answered
/// against the outcome that survived.
#[test]
fn a_census_refused_column_claims_no_order() {
    use crate::compare::ComparisonPlan;

    let meta = one_db(&[("public.t", &[("v", "text")])], vec![]);
    let cols = vec!["v".to_string()];
    let text = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert_eq!(
        text.comparisons,
        [ComparisonPlan::diverging(
            crate::compare::CompareKind::Text { length: None },
            crate::compare::ComparisonDivergence::UnknownCollation,
        )]
    );

    // The same column declared as an array, whose values do not share one
    // list shape: `VaryingArrayShape` holds *array* literals as text, and
    // comparing them as text is not an order this build defines.
    let varying = array_column(&[shape(Some((1, 2)), false)]);
    assert_eq!(varying.columns, [ColumnResolution::VaryingArrayShape]);
    assert_eq!(varying.comparisons, [ComparisonPlan::Refused]);
}

#[test]
fn diagnostic_carries_the_declared_string_when_present() {
    let meta = one_db(&[("public.t", &[("id", "integer")])], vec![]);
    let cols = vec!["id".to_string(), "extra".to_string()];
    let resolved = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    assert_eq!(resolved.notes[0].declared.as_deref(), Some("integer"));
    assert_eq!(resolved.notes[1].declared, None);
}

/// `database_for_name` selects by the attributed database's *name*, not by
/// which database's DDL mentions the table first — asserted by giving two
/// databases different declared types for the same qualified table name.
/// The caller knows which database applies from the matched `CopyBlock`'s
/// own attribution.
#[test]
fn database_selects_by_attributed_name_not_by_first_match() {
    let mut a = one_db(&[("public.t", &[("id", "text")])], vec![]).databases.remove(0);
    a.name = Some("a".to_string());
    let mut b = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
    b.name = Some("b".to_string());
    let meta = DumpMetadata { databases: vec![a, b] };
    let cols = vec!["id".to_string()];

    let resolved_a =
        resolve_columns("public.t", &cols, Some(&meta), Some("a"), SchemaMode::Typed, &[]);
    assert_eq!(resolved_a.schema.field(0).data_type(), &arrow_schema::DataType::Utf8View);

    let resolved_b =
        resolve_columns("public.t", &cols, Some(&meta), Some("b"), SchemaMode::Typed, &[]);
    assert_eq!(resolved_b.schema.field(0).data_type(), &arrow_schema::DataType::Int32);
}

/// **The untyped mode widens a marked column still `Mapped` and no
/// other**: to `Utf8View` at a scalar plan, keeping the declared type's
/// comparison in PostgreSQL's semantics and none in DataFusion's, which
/// compares its text; a column already text holds every value, so is
/// left as it is, and an unmarked one keeps its type.
#[test]
fn the_untyped_mode_widens_a_marked_column_and_compares_it_per_semantics() {
    use arrow_schema::DataType;

    let meta = one_db(
        &[("public.t", &[("id", "integer"), ("d", "date"), ("n", "text"), ("a", "date[]")])],
        vec![],
    );
    let cols: Vec<String> = ["id", "d", "n", "a"].iter().map(|c| c.to_string()).collect();
    let declared = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
    for semantics in [ComparisonSemantics::Postgres, ComparisonSemantics::DataFusion] {
        let mut widened = declared.clone();
        read_as_text(&mut widened, &[false, true, true, true], semantics);
        let types: Vec<&DataType> = widened.schema.fields().iter().map(|f| f.data_type()).collect();
        assert_eq!(
            types,
            [&DataType::Int32, &DataType::Utf8View, &DataType::Utf8View, &DataType::Utf8View]
        );
        use ColumnResolution::{Mapped, UnrepresentableValues};
        assert_eq!(widened.columns, [Mapped, UnrepresentableValues, Mapped, UnrepresentableValues]);
        assert_eq!(widened.notes[1].resolution, UnrepresentableValues);
        assert_eq!(widened.plans[3], NestedPlan::Scalar);
        let expected = |i: usize| match semantics {
            ComparisonSemantics::Postgres => declared.comparisons[i].clone(),
            ComparisonSemantics::DataFusion => ComparisonPlan::Refused,
        };
        assert_eq!(widened.comparisons[1], expected(1), "{semantics:?}");
        assert_eq!(widened.comparisons[3], expected(3), "{semantics:?}");
        assert_eq!(widened.comparisons[..1], declared.comparisons[..1]);
        assert_eq!(widened.comparisons[2], declared.comparisons[2]);
    }
}

/// A [`ColumnNote`]'s severity is derived from its resolution, never
/// stored — so it cannot drift out of agreement with the outcome it
/// describes.
#[test]
fn column_note_severity_follows_its_resolution() {
    let note = |resolution| ColumnNote { column: "c".to_string(), declared: None, resolution };
    assert_eq!(note(ColumnResolution::Mapped).severity(), Severity::Info);
    for fell_back in [
        ColumnResolution::UnknownType,
        ColumnResolution::NotDeclared,
        ColumnResolution::MetadataNotScanned,
        ColumnResolution::OpaqueBaseType,
        ColumnResolution::EmptyEnum,
        ColumnResolution::UnrepresentableValues,
    ] {
        assert_eq!(note(fell_back.clone()).severity(), Severity::Warning, "{fell_back:?}");
    }
}

/// The two channels share one scale, which is the whole point of keeping
/// them separate types: a caller can filter both by the same threshold
/// without knowing which produced what.
#[test]
fn both_channels_order_on_one_severity_scale() {
    use pgdt_model::{Diagnostic, DiagnosticKind};

    let file = Diagnostic::tiling_broken(vec![pgdt_model::TilingIssue::Empty { index: 0 }]);
    let column = ColumnNote {
        column: "c".to_string(),
        declared: None,
        resolution: ColumnResolution::UnknownType,
    };
    assert!(file.severity() > column.severity());
    assert!(matches!(file.kind(), DiagnosticKind::TilingBroken { .. }));
}
