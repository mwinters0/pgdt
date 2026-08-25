//! The nested literal codec against real `pg_dump` output — the I20
//! conformance test (`docs/design/architecture.md`, "Decoders and
//! render-back").
//!
//! `nested.rs`'s own unit tests are hand-written literals, which can encode a
//! misreading of the escaping rules twice. This file cannot: it reads every
//! nested column of `fixtures/*/types/default.sql` in `SchemaMode::Strings` —
//! the untyped path, byte-for-byte what `copy::decode_field` produced — and
//! requires `render_*(decode_*(value)) == value` for every one. A force-quote
//! predicate transcribed even slightly wrong from `arrayfuncs.c`/`rowtypes.c`/
//! `rangetypes.c` shows up here as an inequality, on values PostgreSQL itself
//! wrote.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use pgdump_query::cache::CacheMode;
use pgdump_query::nested::{
    decode_array, decode_multirange, decode_range, decode_record, render_array, render_multirange,
    render_range, render_record,
};
use pgdump_query::resolve::SchemaMode;
use pgdump_query::{
    BatchOptions, LocalFileSource, NestedPlan, ScanOptions, read_table, render_field,
};

/// Which codec a column's literals belong to. Resolution does not choose this
/// yet — that is 4.4's job — so this file names it per column.
#[derive(Clone, Copy)]
enum Kind {
    Array,
    Record,
    Range,
    Multirange,
}

/// Every nested column in the `types` fixture, by table and column name.
/// `t_multirange` exists only on PG14+ (I10), so it carries a minimum version.
const NESTED_COLUMNS: &[(&str, &str, Kind, u32)] = &[
    ("public.t_array", "v_empty", Kind::Array, 13),
    ("public.t_array", "v_with_null", Kind::Array, 13),
    ("public.t_array", "v_null_array", Kind::Array, 13),
    ("public.t_array", "v_text_special", Kind::Array, 13),
    ("public.t_array", "v_enum_array", Kind::Array, 13),
    ("public.t_array_shape", "v_multidim", Kind::Array, 13),
    ("public.t_array_shape", "v_mixed_dim", Kind::Array, 13),
    ("public.t_array_shape", "v_lbound", Kind::Array, 13),
    ("public.t_composite", "v_point", Kind::Record, 13),
    ("public.t_composite", "v_points", Kind::Array, 13),
    ("public.t_composite", "v_tagged", Kind::Record, 13),
    ("public.t_composite", "v_empty_comp", Kind::Record, 13),
    ("public.t_base_type", "v_mybase_array", Kind::Array, 13),
    ("public.t_range", "v_range", Kind::Range, 13),
    ("public.t_user_range", "v_myrange", Kind::Range, 13),
    ("public.t_text_range", "v_textrange", Kind::Range, 13),
    ("public.t_multirange", "v_int4multirange", Kind::Multirange, 14),
    ("public.t_multirange", "v_myrange_multi", Kind::Multirange, 14),
];

fn types_fixture(version: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(version.to_string())
        .join("types")
        .join("default.sql")
}

/// One column's non-NULL values, as decoded COPY text. `SchemaMode::Strings`
/// is deliberate: it never consults the DDL, so what comes back is exactly
/// what `copy::decode_field` produced, which is this codec's input.
async fn column_values(path: &Path, table: &str, column: &str) -> Vec<String> {
    let source = LocalFileSource::open(path).unwrap();
    let options = BatchOptions { schema_mode: SchemaMode::Strings, ..Default::default() };
    let mut out = Vec::new();
    read_table(
        &source,
        table,
        &ScanOptions::default(),
        &options,
        None,
        CacheMode::Disabled,
        |batch| {
            let index = batch.schema().index_of(column).expect("column is in the COPY header");
            for row in 0..batch.num_rows() {
                // `Strings` mode resolves every column `Utf8View`/`Scalar`,
                // which is the point: what comes back is the codec's input,
                // not something the typed path has already parsed.
                let column = batch.column(index).as_ref();
                if let Some(value) = render_field(column, row, &NestedPlan::Scalar) {
                    out.push(value);
                }
            }
            ControlFlow::Continue(())
        },
    )
    .await
    .unwrap();
    out
}

#[track_caller]
fn assert_round_trips(kind: Kind, value: &str, context: &str) {
    let rendered = match kind {
        Kind::Array => {
            render_array(&decode_array(value).unwrap_or_else(|| panic!("{context}: {value}")))
        }
        Kind::Record => {
            render_record(&decode_record(value).unwrap_or_else(|| panic!("{context}: {value}")))
        }
        Kind::Range => {
            render_range(&decode_range(value).unwrap_or_else(|| panic!("{context}: {value}")))
        }
        Kind::Multirange => render_multirange(
            &decode_multirange(value).unwrap_or_else(|| panic!("{context}: {value}")),
        ),
    };
    assert_eq!(rendered, value, "{context}");
}

#[tokio::test]
async fn every_nested_fixture_value_round_trips_byte_for_byte() {
    for version in [13, 14, 15, 16, 17, 18] {
        let path = types_fixture(version);
        let mut seen = 0;
        for (table, column, kind, min_version) in NESTED_COLUMNS {
            if version < *min_version {
                continue;
            }
            let values = column_values(&path, table, column).await;
            assert!(!values.is_empty(), "pg_dump {version}: {table}.{column} has no non-NULL rows");
            for value in &values {
                assert_round_trips(*kind, value, &format!("pg_dump {version}: {table}.{column}"));
            }
            seen += values.len();
        }
        assert!(seen > 30, "pg_dump {version}: only {seen} nested values reached the codec");
    }
}

/// The both-escape-conventions-at-once case, in both nesting orders, end to
/// end: peel one layer, decode the other, and put both back. This is the
/// shape a single-convention decoder passes every other fixture value on and
/// still gets wrong.
#[tokio::test]
async fn a_nested_layer_round_trips_through_the_other_conventions_codec() {
    for version in [13, 16, 18] {
        let path = types_fixture(version);

        // `public.point2d[]` — `array_out` (backslash) around `record_out`
        // (doubling).
        let mut composites_seen = 0;
        for value in column_values(&path, "public.t_composite", "v_points").await {
            let outer = decode_array(&value).unwrap();
            let elements: Vec<Option<String>> = outer
                .elements
                .iter()
                .map(|element| {
                    element.as_ref().map(|text| {
                        let record = decode_record(text).unwrap();
                        assert_eq!(record.fields.len(), 2, "point2d has two fields");
                        composites_seen += 1;
                        render_record(&record)
                    })
                })
                .collect();
            assert_eq!(elements, outer.elements, "pg_dump {version}: v_points");
            assert_eq!(render_array(&outer), value, "pg_dump {version}: v_points");
        }
        assert!(composites_seen >= 3, "pg_dump {version}: {composites_seen} nested composites");

        // `public.tagged` — `record_out` around `array_out`, the other way up.
        let mut arrays_seen = 0;
        for value in column_values(&path, "public.t_composite", "v_tagged").await {
            let outer = decode_record(&value).unwrap();
            let tags = outer.fields[1].as_ref().expect("v_tagged's array field is never NULL");
            let inner = decode_array(tags).unwrap();
            assert_eq!(&render_array(&inner), tags, "pg_dump {version}: v_tagged");
            arrays_seen += 1;
            assert_eq!(render_record(&outer), value, "pg_dump {version}: v_tagged");
        }
        assert!(arrays_seen >= 2, "pg_dump {version}: {arrays_seen} nested arrays");
    }
}

/// `()` is what `record_out` writes for a composite with no fields *and* for
/// a one-field composite holding SQL NULL, so the literal cannot say which it
/// is — this codec answers "one NULL field" because that is all the text
/// supports. The declared field list is the only discriminator, which is why
/// arity is the caller's join and not a check made here.
#[tokio::test]
async fn a_zero_field_composite_is_indistinguishable_from_a_one_field_null() {
    for version in [13, 16, 18] {
        let path = types_fixture(version);
        let values = column_values(&path, "public.t_composite", "v_empty_comp").await;
        assert_eq!(values, vec!["()".to_string(), "()".to_string()], "pg_dump {version}");
        for value in &values {
            let record = decode_record(value).unwrap();
            assert_eq!(record.fields, vec![None], "pg_dump {version}: {value}");
            assert_eq!(render_record(&record), *value, "pg_dump {version}: {value}");
        }
    }
}

/// The delimiter trap (I22), and why no round-trip test can catch it. `box`'s
/// `typdelim` is `;`, a domain over it inherits that, and this codec's
/// separator is a hardcoded `,` — so a two-element array comes apart into
/// seven elements whose commas are then *not* force-quoted, and re-renders
/// byte-for-byte identical. Wrong element boundaries, an exact round trip, no
/// error anywhere: the refusal has to happen at resolution, on the element
/// type after the domain walk, because nothing downstream of it can tell.
#[tokio::test]
async fn the_delimiter_trap_round_trips_while_splitting_on_the_wrong_character() {
    for version in [13, 16, 18] {
        let path = types_fixture(version);
        let values = column_values(&path, "public.t_delimiter", "v_box_domain_array").await;
        assert_eq!(
            values,
            vec!["{(1,1),(0,0);(3,3),(2,2)}".to_string()],
            "pg_dump {version}: two boxes, semicolon-separated"
        );
        let decoded = decode_array(&values[0]).unwrap();
        assert_eq!(decoded.elements.len(), 7, "pg_dump {version}: a comma split of two boxes");
        assert_eq!(render_array(&decoded), values[0], "pg_dump {version}");
    }
}

/// The shapes 4.5's census exists to find, read off real values rather than
/// asserted from the DDL — which cannot see them at all (I21).
#[tokio::test]
async fn the_fixture_carries_the_array_shapes_the_census_will_have_to_report() {
    for version in [13, 16, 18] {
        let path = types_fixture(version);
        let ndims = |values: &[String]| -> Vec<usize> {
            values.iter().map(|v| decode_array(v).unwrap().ndim()).collect()
        };

        let uniform = column_values(&path, "public.t_array_shape", "v_multidim").await;
        assert_eq!(ndims(&uniform), vec![2, 2], "pg_dump {version}: v_multidim");

        let mixed = column_values(&path, "public.t_array_shape", "v_mixed_dim").await;
        assert_eq!(ndims(&mixed), vec![1, 2], "pg_dump {version}: v_mixed_dim");

        let decorated = column_values(&path, "public.t_array_shape", "v_lbound").await;
        assert!(
            decorated.iter().all(|v| decode_array(v).unwrap().is_decorated()),
            "pg_dump {version}: v_lbound"
        );
        assert_eq!(
            decode_array(&decorated[0]).unwrap().lower_bounds,
            vec![0],
            "pg_dump {version}: v_lbound"
        );

        // And the ordinary case they are contrasted against.
        let plain = column_values(&path, "public.t_array", "v_text_special").await;
        assert!(
            plain.iter().all(|v| {
                let a = decode_array(v).unwrap();
                a.ndim() == 1 && !a.is_decorated()
            }),
            "pg_dump {version}: v_text_special"
        );
    }
}
