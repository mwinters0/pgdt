//! Type resolution against real `pg_dump` output — `fixtures/*/types/default.sql`
//! (`docs/design/architecture.md`, "Type resolution"). Unlike
//! `pgdump_query/src/pgtype.rs`'s and `resolve.rs`'s unit tests (hand-written
//! `TypeDef`s), this exercises the whole "declared type string, as `pg_dump`
//! actually wrote it, resolved against that same dump's `CREATE TYPE` list"
//! path end to end, the same evidence `tests/preamble.rs` pins the DDL
//! grammar against.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Fields};
use pgdump_query::resolve::{ColumnResolution, SchemaMode, resolve_columns};
use pgdump_query::{DumpMetadata, LocalFileSource, NestedPlan, ScanOptions, build_index};

fn types_fixture(version: u32, flag_set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("types")
        .join(format!("{flag_set}.sql"))
}

/// Two copies of `edge_cases/create.sql`, concatenated into one real
/// `\connect`-delimited multi-database dump — see `tests/preamble.rs`'s
/// `multidb_fixture` for the full rationale (duplicated here since each
/// `tests/*.rs` file is its own crate with no shared support module).
fn multidb_fixture(version: u32) -> (tempfile::TempDir, PathBuf) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("edge_cases")
        .join("create.sql");
    let content = std::fs::read_to_string(path).unwrap();
    let renamed = content.replace("pgdq_fixture", "pgdq_fixture_2");
    let dir = tempfile::tempdir().unwrap();
    let combined = dir.path().join("multidb.sql");
    std::fs::write(&combined, format!("{content}{renamed}")).unwrap();
    (dir, combined)
}

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
        db.tables.get(qualified).unwrap().iter().map(|(n, _)| n.clone()).collect();
    // No census: this file pins what the *DDL alone* resolves to, which is
    // the optimistic type every array column starts from
    // (`docs/design/architecture.md`, "The array shape census" — consuming
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

        // `interval` maps to `Utf8View` deliberately too.
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
        // indistinguishable here (I21). Only the data separates them, and it
        // does so at decode time, not here.
        let shape_res = m("public.t_array_shape");
        assert!(shape_res.iter().all(|r| *r == Mapped), "pg_dump {v}: t_array_shape");

        // A composite, an array *of* that composite, a composite with an
        // array *field*, and a zero-field composite (I23) — the recursion
        // composes in both nesting orders and bottoms out on an empty field
        // list without special-casing it.
        let composite_res = m("public.t_composite");
        assert!(composite_res.iter().all(|r| *r == Mapped), "pg_dump {v}: {composite_res:?}");

        // `mybase` is opaque on its own account; `mybase[]` is refused for
        // its *element*, and the two are told apart so `pgdq info` can say
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
        let enum_domain_res = m("public.t_enum_domain");
        assert!(enum_domain_res.iter().all(|r| *r == Mapped), "pg_dump {v}: {enum_domain_res:?}");
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
                db.tables.get("public.widgets").unwrap().iter().map(|(n, _)| n.clone()).collect();
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
