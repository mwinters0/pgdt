//! End-to-end round-trip over real `pg_dump` output (Phase 2.4, "Testing the
//! mapping's correctness" in `docs/design/roadmap-phase2-typed-columns.md`):
//! every mapped, always-decodable column family must render back to exactly
//! what `SchemaMode::Strings` (Phase 1's untouched, byte-for-byte behavior)
//! already decoded for that same field — that's what "the original bytes"
//! means once `SchemaMode::Strings` is available as a trustworthy oracle,
//! rather than re-deriving expected values by hand.
//!
//! `decode.rs`'s own unit tests already cover the boundary values a round
//! trip through a small fixture can't be relied on to hit (`NaN`,
//! `±Infinity`, `infinity`/`-infinity` dates/timestamps, 38-vs-39-digit
//! numeric); this file instead proves those same failure modes surface
//! correctly through the *real* pipeline — preamble parse, resolution, and
//! decode together — using `public.t_numeric`/`t_date`/`t_timestamp`'s own
//! real boundary rows rather than hand-built ones.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use pgdump_query::cache::CacheMode;
use pgdump_query::resolve::SchemaMode;
use pgdump_query::{BatchOptions, Error, LocalFileSource, ScanOptions, read_table, render_field};

fn types_fixture(version: u32, flag_set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("types")
        .join(format!("{flag_set}.sql"))
}

async fn rows(path: &Path, table: &str, mode: SchemaMode) -> Vec<Vec<Option<String>>> {
    let source = LocalFileSource::open(path).unwrap();
    let options = BatchOptions { schema_mode: mode, ..Default::default() };
    let mut out = Vec::new();
    read_table(
        &source,
        table,
        &ScanOptions::default(),
        &options,
        None,
        CacheMode::Disabled,
        |batch| {
            for row in 0..batch.num_rows() {
                out.push(batch.columns().iter().map(|c| render_field(c.as_ref(), row)).collect());
            }
            ControlFlow::Continue(())
        },
    )
    .await
    .unwrap();
    out
}

/// Every type family that always decodes successfully on this fixture (no
/// `NaN`/infinity boundary row): typed render-back must equal
/// `SchemaMode::Strings`'s own text for every field, row for row.
#[tokio::test]
async fn round_trip_matches_strings_mode_for_every_always_decodable_table() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        for table in [
            "public.t_int",
            "public.t_float",
            "public.t_text",
            "public.t_uuid",
            "public.t_bytea",
            "public.t_time",
            "public.t_json",
            "public.t_net",
            "public.t_interval",
            "public.t_enum_domain",
        ] {
            let typed = rows(&path, table, SchemaMode::Typed).await;
            let strings = rows(&path, table, SchemaMode::Strings).await;
            assert_eq!(typed, strings, "pg_dump {version}: {table}");
            assert!(!typed.is_empty(), "pg_dump {version}: {table} unexpectedly empty");
        }
    }
}

/// `NaN` bypasses `numeric(p,s)`'s own precision/scale check and has no
/// `Decimal128`/`Decimal256` representation, so it's a genuine decode
/// failure, not a bug — `public.t_numeric.v_small numeric(10,2)` carries it
/// for exactly this reason ("Type mapping" in the phase doc). Checks the
/// error names the right table/column/declared type/value rather than
/// merely failing.
#[tokio::test]
async fn nan_numeric_is_a_field_decode_error_naming_its_context() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();
        let err = read_table(
            &source,
            "public.t_numeric",
            &ScanOptions::default(),
            &BatchOptions::default(),
            None,
            CacheMode::Disabled,
            |_| ControlFlow::Continue(()),
        )
        .await
        .unwrap_err();
        match err {
            Error::FieldDecode { table, column, declared_type, value, .. } => {
                assert_eq!(table, "public.t_numeric", "pg_dump {version}");
                assert_eq!(column, "v_small", "pg_dump {version}");
                assert_eq!(declared_type, "numeric(10,2)", "pg_dump {version}");
                assert_eq!(value, "NaN", "pg_dump {version}");
            }
            other => panic!("pg_dump {version}: expected FieldDecode, got {other:?}"),
        }
        // The escape hatch: `Strings` mode never looks at the DDL, so `NaN`
        // passes through as plain Utf8View text with no error at all.
        let strings = rows(&path, "public.t_numeric", SchemaMode::Strings).await;
        assert!(
            strings.iter().any(|row| row.iter().any(|f| f.as_deref() == Some("NaN"))),
            "pg_dump {version}: Strings mode should still show NaN as text"
        );
    }
}

/// `infinity`/`-infinity` are real PostgreSQL date values with no `Date32`
/// sentinel, so this is a genuine, expected `FieldDecode` (same reasoning as
/// `NaN` above), not a bug.
#[tokio::test]
async fn date_infinity_is_a_field_decode_error_naming_its_context() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();
        let err = read_table(
            &source,
            "public.t_date",
            &ScanOptions::default(),
            &BatchOptions::default(),
            None,
            CacheMode::Disabled,
            |_| ControlFlow::Continue(()),
        )
        .await
        .unwrap_err();
        match err {
            Error::FieldDecode { table, column, declared_type, value, .. } => {
                assert_eq!(table, "public.t_date", "pg_dump {version}");
                assert_eq!(column, "v_date", "pg_dump {version}");
                assert_eq!(declared_type, "date", "pg_dump {version}");
                assert_eq!(value, "infinity", "pg_dump {version}");
            }
            other => panic!("pg_dump {version}: expected FieldDecode, got {other:?}"),
        }
    }
}

/// Same as the date case, for `timestamp without time zone`.
#[tokio::test]
async fn timestamp_infinity_is_a_field_decode_error_naming_its_context() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();
        let err = read_table(
            &source,
            "public.t_timestamp",
            &ScanOptions::default(),
            &BatchOptions::default(),
            None,
            CacheMode::Disabled,
            |_| ControlFlow::Continue(()),
        )
        .await
        .unwrap_err();
        match err {
            Error::FieldDecode { table, column, declared_type, value, .. } => {
                assert_eq!(table, "public.t_timestamp", "pg_dump {version}");
                assert_eq!(column, "v_ts", "pg_dump {version}");
                assert_eq!(declared_type, "timestamp without time zone", "pg_dump {version}");
                assert_eq!(value, "infinity", "pg_dump {version}");
            }
            other => panic!("pg_dump {version}: expected FieldDecode, got {other:?}"),
        }
    }
}
