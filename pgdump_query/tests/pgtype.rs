//! Type resolution against real `pg_dump` output — `fixtures/*/types/default.sql`
//! (`docs/design/decisions.md`, "Type resolution and decoders"). Unlike
//! `pgdump_query/src/pgtype.rs`'s and `resolve.rs`'s unit tests (hand-written
//! `TypeDef`s), this exercises the whole "declared type string, as `pg_dump`
//! actually wrote it, resolved against that same dump's `CREATE TYPE` list"
//! path end to end, the same evidence `tests/preamble.rs` pins the DDL
//! grammar against.

use std::path::Path;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Fields};
use pgdump_query::resolve::{ColumnResolution, SchemaMode, resolve_columns};
use pgdump_query::{DumpMetadata, LocalFileSource, NestedPlan, ScanOptions, build_index};

mod common;
use common::{all_fixtures, multidb_fixture, types_fixture};

async fn metadata(path: &Path) -> DumpMetadata {
    let source = LocalFileSource::open(path).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    index.metadata.expect("build_index always populates metadata")
}

/// Resolve every declared column of `qualified`, in DDL order — this fixture
/// never drops or generates a column, so DDL order matches `COPY` header
/// order.
fn resolve_table(meta: &DumpMetadata, qualified: &str) -> pgdump_query::ResolvedSchema {
    let db = meta.databases.first().unwrap();
    let cols: Vec<String> =
        db.tables.get(qualified).unwrap().iter().map(|c| c.name.clone()).collect();
    // No census: this file pins what the *DDL alone* resolves to, which is
    // the optimistic type every array column starts from
    // (`docs/design/decisions.md`, "D35" — consuming
    // one is `tests/census.rs`'s subject).
    resolve_columns(qualified, &cols, Some(meta), db.name.as_deref(), SchemaMode::Typed, &[])
}

/// Just the resolutions, for a terse per-table assertion.
fn resolutions(meta: &DumpMetadata, qualified: &str) -> Vec<ColumnResolution> {
    resolve_table(meta, qualified).columns
}

fn list_of(child: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", child, true)))
}

/// The five-field range struct as `crate::pgtype` builds it — restated here
/// rather than exported, so a change to the field names or nullability has to
/// be made in two places deliberately.
fn range_struct(bound: DataType) -> DataType {
    DataType::Struct(Fields::from(vec![
        Field::new("lower", bound.clone(), true),
        Field::new("upper", bound, true),
        Field::new("lower_inclusive", DataType::Boolean, false),
        Field::new("upper_inclusive", DataType::Boolean, false),
        Field::new("empty", DataType::Boolean, false),
    ]))
}

#[tokio::test]
async fn every_column_family_resolves_as_the_mapping_table_says() {
    use ColumnResolution::Mapped;
    for version in [13, 16, 18] {
        let meta = metadata(&types_fixture(version, "default")).await;
        let m = |table: &str| resolutions(&meta, table);
        let v = version;

        // Integers, floats, numeric, text, date/time, uuid, bytea, json,
        // net -- every column here should be `Mapped` (id included).
        for table in [
            "public.t_int",
            "public.t_float",
            "public.t_text",
            "public.t_date",
            "public.t_time",
            "public.t_timestamp",
            // Every spelling `format_type` writes around a typmod, and its
            // `bpchar`: read as the grammar reads them, never `UnknownType`.
            "public.t_type_spelling",
            "public.t_uuid",
            "public.t_bytea",
        ] {
            assert!(m(table).iter().all(|r| *r == Mapped), "pg_dump {v}: {table} = {:?}", m(table));
        }

        // `numeric` picks Decimal128/256 by precision, Utf8View past 76 or
        // with no typmod at all -- all still `Mapped`, never `UnknownType`.
        assert!(m("public.t_numeric").iter().all(|r| *r == Mapped), "pg_dump {v}");

        // `json`/`jsonb`, `inet`/`cidr`/`macaddr`/`macaddr8` map to
        // `Utf8View` deliberately -- still `Mapped`, not `UnknownType`.
        assert!(m("public.t_json").iter().all(|r| *r == Mapped), "pg_dump {v}");
        assert!(m("public.t_net").iter().all(|r| *r == Mapped), "pg_dump {v}");

        // `interval` maps too, to its `MonthDayNano` struct.
        assert!(m("public.t_interval").iter().all(|r| *r == Mapped), "pg_dump {v}");

        // Arrays: every array column maps to a `List` of its element type,
        // whatever that element is (I21 — the declared type never says how
        // many dimensions the values have; that is the decoder's problem, not
        // resolution's).
        let array_res = m("public.t_array");
        assert!(array_res.iter().all(|r| *r == Mapped), "pg_dump {v}: t_array = {array_res:?}");

        // A built-in range type (`int4range`, no dot) resolves through the
        // hardcoded subtype table, since PostgreSQL keeps a built-in's
        // subtype in the catalog rather than in DDL text.
        let range_res = m("public.t_range");
        assert!(range_res.iter().all(|r| *r == Mapped), "pg_dump {v}: t_range = {range_res:?}");

        // A range over a text subtype maps the same way a numeric one does —
        // the subtype decides the bound type, not whether it resolves.
        assert_eq!(m("public.t_text_range")[1], Mapped, "pg_dump {v}: t_text_range.v_textrange");

        // Every column of the array-shape table maps from the declared type
        // alone — `integer[][]`, a column whose rows disagree about
        // dimensionality, and one carrying an `[lb:ub]=` prefix are
        // indistinguishable here (I21). Only the data separates them: the
        // block's census, which resolution reads once it has one (D43).
        let shape_res = m("public.t_array_shape");
        assert!(shape_res.iter().all(|r| *r == Mapped), "pg_dump {v}: t_array_shape");

        // Every spelling collapsed to `integer[]` before the file was
        // written (I28), so by the time resolution sees them there is nothing
        // left to tell apart — which is the point of the table, and the
        // reason the spellings need a unit test rather than a fixture.
        let spelling_res = m("public.t_array_spelling");
        assert!(
            spelling_res.iter().all(|r| *r == Mapped),
            "pg_dump {v}: t_array_spelling = {spelling_res:?}"
        );

        // A composite, an array *of* that composite, a composite with an
        // array *field*, and a zero-field composite (I23) — the recursion
        // composes in both nesting orders and bottoms out on an empty field
        // list without special-casing it.
        let composite_res = m("public.t_composite");
        assert!(composite_res.iter().all(|r| *r == Mapped), "pg_dump {v}: {composite_res:?}");

        // `mybase` is opaque on its own account; `mybase[]` is refused for
        // its *element*, and the two are told apart so `pgdt info` can say
        // which happened.
        let base_res = m("public.t_base_type");
        assert_eq!(base_res[1], ColumnResolution::OpaqueBaseType, "pg_dump {v}: v_mybase");
        assert_eq!(
            base_res[2],
            ColumnResolution::OpaqueElementType,
            "pg_dump {v}: t_base_type.v_mybase_array"
        );

        // The delimiter trap wearing a domain (I22). Resolution unwraps
        // `public.box_domain` to `box`, which this build's table has no entry
        // for, so the scalar is `UnknownType`. The array is refused for its
        // element — and the refusal has to survive the domain indirection,
        // since a domain's own DDL records nothing about the `;` separator it
        // inherited. Getting this wrong yields `List<Utf8View>` and a
        // literal split on the wrong character, which round-trips exactly and
        // is wrong.
        let delim_res = m("public.t_delimiter");
        assert_eq!(delim_res[0], Mapped, "pg_dump {v}: t_delimiter.id");
        assert_eq!(
            delim_res[1],
            ColumnResolution::UnknownType,
            "pg_dump {v}: t_delimiter.v_box_domain (domain over box)"
        );
        assert_eq!(
            delim_res[2],
            ColumnResolution::OpaqueElementType,
            "pg_dump {v}: t_delimiter.v_box_domain_array"
        );

        // Enum and domain-over-domain columns both resolve `Mapped` -- the
        // enum via `Dictionary`, the domain transitively through its base.
        // A label-less enum is the one `CREATE TYPE ... AS ENUM` form with
        // nothing to map to.
        let enum_domain_res = m("public.t_enum_domain");
        assert_eq!(
            enum_domain_res,
            [Mapped, Mapped, Mapped, ColumnResolution::EmptyEnum],
            "pg_dump {v}: t_enum_domain"
        );

        // An array whose element type is itself an array, through a domain
        // (I26): refused, and told apart from the opaque-element refusal
        // above because `integer[]` is understood, not opaque. Every other
        // column of this table is a shape that composes two mechanisms and
        // maps -- including the composite whose `arr` *field* carries the
        // refused type, which is a `Utf8View` field inside a mapped `Struct`
        // rather than a refusal of the whole column.
        let nested_res = m("public.t_nested_array");
        assert_eq!(
            nested_res,
            [
                Mapped,
                ColumnResolution::NestedArrayElement,
                Mapped,
                Mapped,
                Mapped,
                Mapped,
                Mapped,
                Mapped
            ],
            "pg_dump {v}: t_nested_array"
        );
    }
}

/// The Arrow types and [`NestedPlan`]s the four container families resolve
/// to, against real `pg_dump` output rather than hand-built `TypeDef`s — the
/// mapping table's own statement, one level down from
/// `every_column_family_resolves_as_the_mapping_table_says`'s outcomes.
#[tokio::test]
async fn the_container_families_resolve_to_the_arrow_types_the_mapping_table_names() {
    for version in [13, 16, 18] {
        let meta = metadata(&types_fixture(version, "default")).await;
        let v = version;

        let arrays = resolve_table(&meta, "public.t_array");
        let field = |r: &pgdump_query::ResolvedSchema, name: &str| {
            let i = r.schema.index_of(name).unwrap();
            (r.schema.field(i).data_type().clone(), r.plans[i].clone())
        };
        let flat = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        assert_eq!(
            field(&arrays, "v_empty"),
            (list_of(DataType::Int32), flat.clone()),
            "pg_dump {v}: integer[]"
        );
        assert_eq!(
            field(&arrays, "v_text_special"),
            (list_of(DataType::Utf8View), flat.clone()),
            "pg_dump {v}: text[]"
        );
        assert_eq!(
            field(&arrays, "v_enum_array"),
            (
                list_of(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))),
                flat.clone()
            ),
            "pg_dump {v}: an enum element keeps its dictionary inside the list"
        );

        // A composite is a `Struct` in declaration order; an array of one and
        // a composite with an array field are the same recursion, two ways up.
        let composites = resolve_table(&meta, "public.t_composite");
        let point2d = DataType::Struct(Fields::from(vec![
            Field::new("x", DataType::Int32, true),
            Field::new("y", DataType::Utf8View, true),
        ]));
        let point_plan = NestedPlan::Record(vec![NestedPlan::Scalar, NestedPlan::Scalar]);
        assert_eq!(
            field(&composites, "v_point"),
            (point2d.clone(), point_plan.clone()),
            "pg_dump {v}: public.point2d"
        );
        assert_eq!(
            field(&composites, "v_points"),
            (list_of(point2d), NestedPlan::Array(Box::new(point_plan))),
            "pg_dump {v}: public.point2d[]"
        );
        assert_eq!(
            field(&composites, "v_tagged"),
            (
                DataType::Struct(Fields::from(vec![
                    Field::new("label", DataType::Utf8View, true),
                    Field::new("tags", list_of(DataType::Utf8View), true),
                ])),
                NestedPlan::Record(vec![NestedPlan::Scalar, flat.clone()])
            ),
            "pg_dump {v}: a composite with a text[] field"
        );
        // A zero-field composite is a zero-field `Struct` (I23), not a
        // refusal — the analogy with `EmptyEnum` does not hold, because `()`
        // is a real value that round-trips.
        assert_eq!(
            field(&composites, "v_empty_comp"),
            (DataType::Struct(Fields::empty()), NestedPlan::Record(Vec::new())),
            "pg_dump {v}: CREATE TYPE ... AS ()"
        );

        // A built-in range's subtype is hardcoded; a user-defined one's comes
        // from its own DDL.
        let bound_plan = NestedPlan::Range(Box::new(NestedPlan::Scalar));
        assert_eq!(
            field(&resolve_table(&meta, "public.t_range"), "v_range"),
            (range_struct(DataType::Int32), bound_plan.clone()),
            "pg_dump {v}: int4range"
        );
        assert_eq!(
            field(&resolve_table(&meta, "public.t_user_range"), "v_myrange"),
            (range_struct(DataType::Float64), bound_plan.clone()),
            "pg_dump {v}: CREATE TYPE ... AS RANGE (subtype = double precision)"
        );
        assert_eq!(
            field(&resolve_table(&meta, "public.t_text_range"), "v_textrange"),
            (range_struct(DataType::Utf8View), bound_plan),
            "pg_dump {v}: a range over text"
        );

        // Multiranges are PG14+ (I10). The companion of a user range has no
        // `CREATE TYPE` of its own anywhere in the file, so its bound type
        // can only come from the range that names it.
        if version >= 14 {
            let multi = resolve_table(&meta, "public.t_multirange");
            assert_eq!(
                field(&multi, "v_int4multirange"),
                (
                    list_of(range_struct(DataType::Int32)),
                    NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
                ),
                "pg_dump {v}: int4multirange"
            );
            assert_eq!(
                field(&multi, "v_myrange_multi"),
                (
                    list_of(range_struct(DataType::Float64)),
                    NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
                ),
                "pg_dump {v}: public.myrange_multi"
            );
        }

        // The two refusals stay `Utf8View`, with nothing nested behind them.
        for (table, column) in
            [("public.t_base_type", "v_mybase_array"), ("public.t_delimiter", "v_box_domain_array")]
        {
            let resolved = resolve_table(&meta, table);
            assert_eq!(
                field(&resolved, column),
                (DataType::Utf8View, NestedPlan::Scalar),
                "pg_dump {v}: {table}.{column}"
            );
        }
    }
}

#[tokio::test]
async fn enum_column_maps_to_a_dictionary_and_domain_to_its_base_type() {
    for version in [13, 16, 18] {
        let meta = metadata(&types_fixture(version, "default")).await;
        let db = meta.databases.first().unwrap();
        let cols = vec!["v_mood".to_string(), "v_domain".to_string()];
        let resolved = resolve_columns(
            "public.t_enum_domain",
            &cols,
            Some(&meta),
            db.name.as_deref(),
            SchemaMode::Typed,
            &[],
        );
        assert_eq!(
            resolved.schema.field(0).data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            "pg_dump {version}"
        );
        // `derived_domain` is a domain over `base_domain`, itself a domain
        // over `integer` -- transitive resolution must reach `Int32`.
        assert_eq!(resolved.schema.field(1).data_type(), &DataType::Int32, "pg_dump {version}");
        assert!(db.tables.contains_key("public.t_enum_domain"), "pg_dump {version}");
    }
}

/// `oid` against real `pg_dump` output, at the boundary a signed reading gets
/// wrong: `UInt32`, and the file holding 2147483648 and 4294967295 as plain
/// unsigned digits.
#[tokio::test]
async fn an_oid_column_resolves_unsigned() {
    for version in [13, 16, 18] {
        let meta = metadata(&types_fixture(version, "default")).await;
        let resolved = resolve_table(&meta, "public.t_oid");
        assert_eq!(resolved.schema.field(1).name(), "v_oid", "pg_dump {version}");
        assert_eq!(resolved.schema.field(1).data_type(), &DataType::UInt32, "pg_dump {version}");
    }
}

/// The two canonical extension names, on the fields of real columns
/// (`docs/design/decisions.md`, "D37"). What a consumer reads
/// is the metadata, so that is what is asserted — and `arrow.json`'s empty
/// metadata value is part of the spelling, not an accident.
///
/// The `id` column beside each is the negative half: a name is a claim about
/// the column it sits on, so an `Int32` must carry none.
#[tokio::test]
async fn the_canonical_extension_names_land_on_the_columns_that_claim_them() {
    const NAME: &str = "ARROW:extension:name";
    for version in [13, 16, 18] {
        let meta = metadata(&types_fixture(version, "default")).await;

        let uuid = resolve_table(&meta, "public.t_uuid");
        assert_eq!(uuid.schema.field(0).metadata().get(NAME), None, "pg_dump {version}: id");
        assert_eq!(
            uuid.schema.field(1).metadata().get(NAME).map(String::as_str),
            Some("arrow.uuid"),
            "pg_dump {version}: v_uuid"
        );

        let json = resolve_table(&meta, "public.t_json");
        for column in [1, 2] {
            let field = json.schema.field(column);
            assert_eq!(
                field.metadata().get(NAME).map(String::as_str),
                Some("arrow.json"),
                "pg_dump {version}: {}",
                field.name()
            );
            assert_eq!(
                field.metadata().get("ARROW:extension:metadata").map(String::as_str),
                Some(""),
                "pg_dump {version}: {}",
                field.name()
            );
        }

        // A `text` column reaches the same `Utf8View` and claims nothing.
        let text = resolve_table(&meta, "public.t_text");
        assert!(text.schema.fields().iter().all(|f| f.metadata().get(NAME).is_none()));
    }
}

/// I29's own dump, as `pg_dump 16.14` wrote it: every type name needs quoting,
/// and each column resolves to the type it names, the arrays included — the
/// definition and the declaration compared in one spelling.
#[tokio::test]
async fn a_type_name_needing_quotes_resolves_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quoted.sql");
    std::fs::write(
        &path,
        [
            "CREATE SCHEMA s;",
            r#"CREATE DOMAIN s."d[3]" AS integer;"#,
            r#"CREATE DOMAIN s."my type" AS integer;"#,
            r#"CREATE TYPE s."weird[]" AS ENUM ("#,
            "    'a',",
            "    'b'",
            ");",
            r#"CREATE DOMAIN s."x ARRAY" AS integer;"#,
            "CREATE TABLE s.t (",
            r#"    a s."weird[]","#,
            r#"    b s."my type","#,
            r#"    c s."x ARRAY","#,
            r#"    d s."d[3]","#,
            r#"    e s."weird[]"[],"#,
            r#"    f s."my type"[],"#,
            r#"    g s."x ARRAY"[]"#,
            ");",
            "COPY s.t (a, b, c, d, e, f, g) FROM stdin;",
            "a\t1\t2\t3\t{b}\t{4}\t{5}",
            "\\.",
            "",
        ]
        .join("\n"),
    )
    .unwrap();
    let meta = metadata(&path).await;
    let resolved = resolve_table(&meta, "s.t");
    assert!(resolved.columns.iter().all(|r| *r == ColumnResolution::Mapped), "{resolved:?}");
    let label = DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
    let types: Vec<&DataType> = resolved.schema.fields().iter().map(|f| f.data_type()).collect();
    assert_eq!(
        types,
        [
            &label,
            &DataType::Int32,
            &DataType::Int32,
            &DataType::Int32,
            &list_of(label.clone()),
            &list_of(DataType::Int32),
            &list_of(DataType::Int32),
        ]
    );
}

/// Real-shape smoke test for `resolve_columns` against genuine multi-database
/// `DumpMetadata` (as opposed to `resolve.rs`'s hand-built
/// `database_selects_by_attributed_name_not_by_first_match`, which proves
/// exact-name selection with deliberately differing types): resolution must
/// not error or degrade just because `metadata.databases` has more than one
/// entry, and selecting each database explicitly by name must reach that
/// same database's own declared types.
#[tokio::test]
async fn resolution_still_works_against_metadata_with_more_than_one_database() {
    use ColumnResolution::Mapped;
    for version in [13, 16, 18] {
        let (_dir, path) = multidb_fixture(version);
        let meta = metadata(&path).await;
        assert_eq!(meta.databases.len(), 2, "pg_dump {version}");

        for db in &meta.databases {
            let cols: Vec<String> =
                db.tables.get("public.widgets").unwrap().iter().map(|c| c.name.clone()).collect();
            let resolved = resolve_columns(
                "public.widgets",
                &cols,
                Some(&meta),
                db.name.as_deref(),
                SchemaMode::Typed,
                &[],
            );
            assert!(
                resolved.columns.iter().all(|r| *r == Mapped),
                "pg_dump {version}, database {:?}: {:?}",
                db.name,
                resolved.columns
            );
        }
    }
}

/// **Every [`ColumnResolution`] variant, written once**, either as an outcome a
/// fixture column must produce or as the written exemption, and from that one
/// list: `outcome_name`, whose match has no wildcard arm and denies an
/// unreachable one, so a new variant stops this file compiling until it is
/// placed, and a variant placed twice does too; `EVERY_OUTCOME`, the outcomes
/// a fixture must produce; and `EXEMPT`, those it cannot.
macro_rules! resolution_outcomes {
    (fixture: [$($covered:ident),* $(,)?], exempt: [$($exempt:ident),* $(,)?] $(,)?) => {
        /// One resolution outcome's name.
        #[deny(unreachable_patterns)]
        fn outcome_name(resolution: &ColumnResolution) -> &'static str {
            match resolution {
                $(ColumnResolution::$covered => stringify!($covered),)*
                $(ColumnResolution::$exempt => stringify!($exempt),)*
            }
        }

        /// Every outcome that must have a fixture column behind it.
        const EVERY_OUTCOME: &[ColumnResolution] = &[$(ColumnResolution::$covered),*];

        /// The outcomes no fixture can produce, which the test below holds to.
        const EXEMPT: &[ColumnResolution] = &[$(ColumnResolution::$exempt),*];
    };
}

resolution_outcomes! {
    fixture: [
        Mapped,
        UnknownType,
        NotDeclared,
        OpaqueElementType,
        NestedArrayElement,
        VaryingArrayShape,
        OpaqueBaseType,
        EmptyEnum,
    ],
    exempt: [MetadataNotScanned],
}

/// **`roadmap.md`'s fixture rule, made mechanical.** "A shape observed to work
/// is not covered until a fixture holds it" is a rule with nothing enforcing
/// it, so: resolve every column of every `COPY` block of every generated
/// fixture, and require each `ColumnResolution` variant to be produced by at
/// least one of them.
///
/// The census is the block's own, so an outcome only the census can produce
/// (`VaryingArrayShape`) is reachable here — this walk sees the same evidence
/// `pgdt info` does on a fully scanned file.
///
/// **The check is partial and says so.** A shape that resolves to an outcome
/// already covered and merely *works* still passes without a fixture; that
/// half stays a judgement call, and naming the limit beats a check that
/// implies coverage it does not have.
///
/// **An outcome no fixture *can* produce is listed under `EXEMPT`, and that is
/// the only legitimate exemption.** It applies to an outcome that is a
/// property of how much of the file was read rather than of a declared type.
/// `ColumnResolution::MetadataNotScanned` is the one instance: no mapping scan
/// leaves a banked block whose database's DDL it never read (the variant's
/// own docs), so no fixture reaches it, and `pgdt/tests/partial_reporting.rs`
/// pins it against a hand-built truncated cache instead.
/// `resolution_outcomes!` forces the exemption to be written down, and a
/// fixture column producing an exempt outcome fails the test, the exemption's
/// premise then being false.
#[tokio::test]
async fn every_resolution_outcome_is_produced_by_a_real_fixture_column() {
    use std::collections::BTreeMap;

    let mut witnesses: BTreeMap<&'static str, String> = BTreeMap::new();
    for path in all_fixtures() {
        let source = LocalFileSource::open(&path).unwrap();
        let index = build_index(&source, &ScanOptions::default()).await.unwrap();
        for block in index.blocks() {
            if block.header.columns.is_empty() {
                continue;
            }
            let qualified = block.header.qualified_name();
            let resolved = resolve_columns(
                &qualified,
                &block.header.columns,
                index.metadata.as_ref(),
                block.database.as_deref(),
                SchemaMode::Typed,
                &block.array_shapes,
            );
            for note in &resolved.notes {
                witnesses
                    .entry(outcome_name(&note.resolution))
                    .or_insert_with(|| format!("{}: {qualified}.{}", path.display(), note.column));
            }
        }
    }

    let missing: Vec<&str> = EVERY_OUTCOME
        .iter()
        .map(outcome_name)
        .filter(|name| !witnesses.contains_key(name))
        .collect();
    assert!(
        missing.is_empty(),
        "no fixture column produces {missing:?} — add one to scripts/fixture_schema_*.sql and \
         regenerate, rather than exempting the outcome (docs/design/roadmap.md, \"Expand the \
         generated fixtures freely\").\nCovered: {witnesses:#?}"
    );
    let reached: Vec<(&str, &String)> = EXEMPT
        .iter()
        .map(outcome_name)
        .filter_map(|name| witnesses.get(name).map(|witness| (name, witness)))
        .collect();
    assert!(
        reached.is_empty(),
        "an exempt outcome has a fixture column, so is not exempt: {reached:?}"
    );
}
