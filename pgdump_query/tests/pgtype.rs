//! Type resolution against real `pg_dump` output — `fixtures/*/types/default.sql`
//! (`docs/design/roadmap-phase2-typed-columns.md`, "Type mapping"). Unlike
//! `pgdump_query/src/pgtype.rs`'s and `resolve.rs`'s unit tests (hand-written
//! `TypeDef`s), this exercises the whole "declared type string, as `pg_dump`
//! actually wrote it, resolved against that same dump's `CREATE TYPE` list"
//! path end to end, the same evidence `tests/preamble.rs` pins the DDL
//! grammar against.

use std::path::{Path, PathBuf};

use arrow::datatypes::DataType;
use pgdump_query::resolve::{ColumnResolution, SchemaMode, resolve_columns};
use pgdump_query::{DeferredKind, DumpMetadata, LocalFileSource, ScanOptions, build_index};

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

/// Resolve every declared column of `qualified` (in DDL order — this fixture
/// never drops or generates a column, so DDL order matches `COPY` header
/// order) and return just the resolutions, for a terse per-table assertion.
fn resolutions(meta: &DumpMetadata, qualified: &str) -> Vec<ColumnResolution> {
    let db = meta.databases.first().unwrap();
    let cols: Vec<String> =
        db.tables.get(qualified).unwrap().iter().map(|(n, _)| n.clone()).collect();
    resolve_columns(qualified, &cols, Some(meta), SchemaMode::Typed).columns
}

#[tokio::test]
async fn every_mapped_column_family_resolves_as_the_mapping_table_says() {
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

        // Arrays: `id` is mapped, every array column is `Deferred(Array)`
        // regardless of element type or declared dimensionality (I6).
        let array_res = m("public.t_array");
        assert_eq!(array_res[0], Mapped, "pg_dump {v}: t_array.id");
        for r in &array_res[1..] {
            assert_eq!(*r, ColumnResolution::Deferred { kind: DeferredKind::Array }, "pg_dump {v}");
        }

        // A built-in range type (`int4range`, no dot) must resolve the same
        // way a user-defined range would, not fall through to `UnknownType`.
        let range_res = m("public.t_range");
        assert_eq!(range_res[0], Mapped, "pg_dump {v}: t_range.id");
        assert_eq!(
            range_res[1],
            ColumnResolution::Deferred { kind: DeferredKind::Range },
            "pg_dump {v}: t_range.v_range (int4range)"
        );

        // A user-defined composite type's column.
        let composite_res = m("public.t_composite");
        assert_eq!(composite_res[0], Mapped, "pg_dump {v}: t_composite.id");
        assert_eq!(
            composite_res[1],
            ColumnResolution::Deferred { kind: DeferredKind::Composite },
            "pg_dump {v}: t_composite.v_point"
        );

        // Enum and domain-over-domain columns both resolve `Mapped` -- the
        // enum via `Dictionary`, the domain transitively through its base.
        let enum_domain_res = m("public.t_enum_domain");
        assert!(enum_domain_res.iter().all(|r| *r == Mapped), "pg_dump {v}: {enum_domain_res:?}");
    }
}

#[tokio::test]
async fn enum_column_maps_to_a_dictionary_and_domain_to_its_base_type() {
    for version in [13, 16, 18] {
        let meta = metadata(&types_fixture(version, "default")).await;
        let db = meta.databases.first().unwrap();
        let cols = vec!["v_mood".to_string(), "v_domain".to_string()];
        let resolved =
            resolve_columns("public.t_enum_domain", &cols, Some(&meta), SchemaMode::Typed);
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
/// `ambiguous_table_across_databases_resolves_against_the_first_match`,
/// which proves the first-match rule with deliberately differing types):
/// resolution must not error or degrade just because `metadata.databases`
/// has more than one entry.
#[tokio::test]
async fn resolution_still_works_against_metadata_with_more_than_one_database() {
    use ColumnResolution::Mapped;
    for version in [13, 16, 18] {
        let (_dir, path) = multidb_fixture(version);
        let meta = metadata(&path).await;
        assert_eq!(meta.databases.len(), 2, "pg_dump {version}");

        let cols: Vec<String> = meta.databases[0]
            .tables
            .get("public.widgets")
            .unwrap()
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        let resolved = resolve_columns("public.widgets", &cols, Some(&meta), SchemaMode::Typed);
        assert!(
            resolved.columns.iter().all(|r| *r == Mapped),
            "pg_dump {version}: {:?}",
            resolved.columns
        );
    }
}
