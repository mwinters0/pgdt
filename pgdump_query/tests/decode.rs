//! End-to-end round-trip over real `pg_dump` output
//! (`docs/design/decisions.md`, "D73"):
//! every mapped, always-decodable column family must render back to exactly
//! what `SchemaMode::Strings` (the untyped, byte-for-byte path)
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
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::resolve::SchemaMode;
use pgdump_query::{
    Error, LocalFileSource, NestedPlan, QueryOptions, ScanOptions, read_table, render_field,
    table_stream,
};

mod common;
use common::types_fixture;

/// Every row of `table`, rendered back to PostgreSQL text.
///
/// Pull mode, not `read_table`: rendering a nested column needs the stream's
/// own `NestedPlan`s, which push mode only hands back once the whole stream
/// has been drained. The scan underneath is the same one — `read_table`
/// drains this stream internally.
async fn rows(path: &Path, table: &str, mode: SchemaMode) -> Vec<Vec<Option<String>>> {
    try_rows(path, table, mode).await.unwrap()
}

/// The schema one query commits to — the census-retyped one, since
/// `table_stream` fixes it after mapping and before its first batch.
async fn resolved_schema(path: &Path, table: &str) -> pgdump_query::ResolvedSchema {
    let source = LocalFileSource::open(path).unwrap();
    let mut stream = table_stream(
        &source,
        table,
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        CacheMode::DISABLED,
    );
    while let Some(batch) = stream.next().await {
        batch.unwrap();
    }
    stream.resolved_schema()
}

async fn try_rows(
    path: &Path,
    table: &str,
    mode: SchemaMode,
) -> pgdump_query::Result<Vec<Vec<Option<String>>>> {
    let source = LocalFileSource::open(path).unwrap();
    let options = QueryOptions { schema_mode: mode, ..Default::default() };
    let mut stream =
        table_stream(&source, table, ScanOptions::default(), options, None, CacheMode::DISABLED);
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await.transpose()? {
        let plans = stream.resolved_schema().plans;
        for row in 0..batch.num_rows() {
            out.push(
                batch
                    .columns()
                    .iter()
                    .enumerate()
                    .map(|(col, c)| {
                        render_field(c.as_ref(), row, plans.get(col).unwrap_or(&NestedPlan::Scalar))
                            .expect("every fixture value renders back")
                    })
                    .collect(),
            );
        }
    }
    Ok(out)
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
            "public.t_oid",
            "public.t_float",
            "public.t_text",
            "public.t_uuid",
            "public.t_bytea",
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

/// The four container families, end to end on the optimistic path: decoded
/// into `List`/`Struct` columns and rendered back, against `SchemaMode::
/// Strings`'s byte-for-byte text for the same fields.
///
/// `tests/nested.rs` proves the codec is its own inverse; this proves the
/// *typed* path is — that resolution picked the plan the values are actually
/// written in, that the builders filled the type resolution promised, and
/// that nothing was lost between them. A plan/type disagreement (an array of
/// ranges read as a multirange, say) shows up here and nowhere else.
#[tokio::test]
async fn nested_columns_round_trip_against_strings_mode() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let mut tables = vec![
            "public.t_array",
            "public.t_composite",
            "public.t_range",
            "public.t_user_range",
            "public.t_text_range",
            // The two refusals: still text, and still identical text.
            "public.t_base_type",
            "public.t_delimiter",
            // The multi-hop shapes: five that map (a domain over a
            // composite, an array of that domain, a composite whose field is
            // a composite, an array of a user range, a domain over a range)
            // beside the third refusal, an array whose element type is itself
            // an array (I26). The refusal is what keeps `v_nested_array` from
            // a `FieldDecode` on every row: the literal is one brace deep and
            // the type it resolves to is two `List`s deep, and the table
            // round-tripping is what says the column comes back as the text
            // it always was.
            "public.t_nested_array",
            // Four array-declaration spellings that reached `pg_dump` as
            // `integer[]` and are indistinguishable here (I28). The table is
            // ordinary once written, which is exactly its claim.
            "public.t_array_spelling",
        ];
        if version >= 14 {
            tables.push("public.t_multirange");
        }
        for table in tables {
            let typed = rows(&path, table, SchemaMode::Typed).await;
            let strings = rows(&path, table, SchemaMode::Strings).await;
            assert_eq!(typed, strings, "pg_dump {version}: {table}");
            assert!(!typed.is_empty(), "pg_dump {version}: {table} unexpectedly empty");
        }
    }
}

/// **What the census buys, end to end.** `t_array_shape` is the column set
/// the optimistic path refuses — a uniformly 2-D column, one that mixes 1-D
/// and 2-D across rows, and one carrying `[lb:ub]=` prefixes. The schema
/// commits against what the mapping pass actually saw, so the uniform column
/// becomes `List(List(Int32))` and the two that no Arrow list type is honest
/// about come back as text — decided before the first batch, not at row 40
/// million (`docs/design/decisions.md`, "D35").
///
/// It round-trips against `Strings` mode like every other table, which is
/// what says the retyped column is still exact: `{{1,2},{3,4}}` in, the same
/// bytes back out.
#[tokio::test]
async fn the_census_retypes_an_array_column_before_the_schema_commits() {
    use arrow::datatypes::{DataType, Field};
    use pgdump_query::ColumnResolution;

    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let typed = rows(&path, "public.t_array_shape", SchemaMode::Typed).await;
        let strings = rows(&path, "public.t_array_shape", SchemaMode::Strings).await;
        assert_eq!(typed, strings, "pg_dump {version}");
        assert!(!typed.is_empty(), "pg_dump {version}");

        let resolved = resolved_schema(&path, "public.t_array_shape").await;
        let list_of_list = DataType::List(Arc::new(Field::new(
            "item",
            DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
            true,
        )));
        assert_eq!(
            resolved.schema.field(1).data_type(),
            &list_of_list,
            "pg_dump {version}: v_multidim is uniformly 2-D"
        );
        assert_eq!(
            &resolved.columns[1..],
            &[
                ColumnResolution::Mapped,
                ColumnResolution::VaryingArrayShape,
                ColumnResolution::VaryingArrayShape,
            ],
            "pg_dump {version}: v_mixed_dim varies, v_lbound is decorated"
        );
        for i in [2, 3] {
            assert_eq!(
                resolved.schema.field(i).data_type(),
                &DataType::Utf8View,
                "pg_dump {version}: column {i}"
            );
        }
    }
}

/// **The spellings `pg_dump` cannot write, through the whole pipeline.**
/// PostgreSQL accepts six ways of declaring an array-typed column and every
/// one is the same type (I28); `format_type` keeps only the first, so a
/// generated fixture can never carry the other five and this dump is built by
/// hand. `t_array_spelling` in `fixtures/*/types/default.sql` is the other
/// half of the evidence — it proves the collapse in `pg_dump`'s own bytes.
///
/// The seam this crosses is resolution → census → `retype_from_census`, which
/// no unit test reaches: `v_2d` is declared `integer[][]` and holds uniformly
/// 2-D values, so it must resolve to `List(Int32)` — one array level, per the
/// normalization — and then be *deepened by the census* to `List(List(Int32))`
/// exactly as a column spelled `integer[]` would be, rather than refused
/// outright as an array of arrays.
#[tokio::test]
async fn an_array_declaration_pg_dump_never_writes_resolves_and_takes_its_census() {
    use arrow::datatypes::{DataType, Field};
    use pgdump_query::ColumnResolution;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spellings.sql");
    std::fs::write(
        &path,
        "--\n\
         -- Name: t_spelling; Type: TABLE; Schema: public; Owner: postgres\n\
         --\n\
         \n\
         CREATE TABLE public.t_spelling (\n\
         \x20   id integer,\n\
         \x20   v_2d integer[][],\n\
         \x20   v_kw integer ARRAY[4],\n\
         \x20   v_bounded integer[3]\n\
         );\n\
         \n\
         \n\
         --\n\
         -- Data for Name: t_spelling; Type: TABLE DATA; Schema: public; Owner: postgres\n\
         --\n\
         \n\
         COPY public.t_spelling (id, v_2d, v_kw, v_bounded) FROM stdin;\n\
         1\t{{1,2},{3,4}}\t{5,6}\t{7}\n\
         2\t{{5,6},{7,8}}\t\\N\t{}\n\
         \\.\n\
         \n",
    )
    .unwrap();

    // Exact, in both directions: what the typed path renders back is what the
    // untyped path read, for a declaration no fixture can hold.
    assert_eq!(
        rows(&path, "public.t_spelling", SchemaMode::Typed).await,
        rows(&path, "public.t_spelling", SchemaMode::Strings).await,
    );

    let list_of = |inner| DataType::List(Arc::new(Field::new("item", inner, true)));
    let resolved = resolved_schema(&path, "public.t_spelling").await;
    assert_eq!(
        resolved.schema.field(1).data_type(),
        &list_of(list_of(DataType::Int32)),
        "`integer[][]` is one array level from the DDL, deepened by the census"
    );
    assert_eq!(
        resolved.schema.field(2).data_type(),
        &list_of(DataType::Int32),
        "`integer ARRAY[4]`"
    );
    assert_eq!(resolved.schema.field(3).data_type(), &list_of(DataType::Int32), "`integer[3]`");
    assert!(
        resolved.columns.iter().all(|r| *r == ColumnResolution::Mapped),
        "no spelling is a refusal: {:?}",
        resolved.columns
    );
}

/// **The one cause the refusal still has.** The census is keyed by column, so
/// it records the shape of a whole field and has nowhere to put the shape of
/// an array nested *inside* a composite. Such a column therefore stays on the
/// optimistic path however much of the file has been scanned, and a
/// multi-dimensional value there is a `FieldDecode` — with
/// `--schema-mode strings`, which the message names, as its only remedy.
///
/// Hand-written rather than a fixture: `pg_dump` writes what it is given, and
/// no fixture schema inserts a 2-D array into a composite field.
/// `t_nested_ok` is the control that says the literal below really is
/// `record_out`'s form — it round-trips through the typed path unchanged, so
/// the failure on `t_nested_bad` is the nested array's and not a misquoted
/// composite's.
#[tokio::test]
async fn an_array_inside_a_composite_keeps_the_optimistic_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.sql");
    std::fs::write(
        &path,
        "--\n\
         -- Name: boxed; Type: TYPE; Schema: public; Owner: postgres\n\
         --\n\
         \n\
         CREATE TYPE public.boxed AS (\n\
         \tlabel text,\n\
         \tgrid integer[]\n\
         );\n\
         \n\
         \n\
         --\n\
         -- Name: t_nested_ok; Type: TABLE; Schema: public; Owner: postgres\n\
         --\n\
         \n\
         CREATE TABLE public.t_nested_ok (\n\
         \x20   id integer,\n\
         \x20   v public.boxed\n\
         );\n\
         \n\
         \n\
         --\n\
         -- Name: t_nested_bad; Type: TABLE; Schema: public; Owner: postgres\n\
         --\n\
         \n\
         CREATE TABLE public.t_nested_bad (\n\
         \x20   id integer,\n\
         \x20   v public.boxed\n\
         );\n\
         \n\
         \n\
         --\n\
         -- Data for Name: t_nested_ok; Type: TABLE DATA; Schema: public; Owner: postgres\n\
         --\n\
         \n\
         COPY public.t_nested_ok (id, v) FROM stdin;\n\
         1\t(a,\"{1,2}\")\n\
         \\.\n\
         \n\
         \n\
         --\n\
         -- Data for Name: t_nested_bad; Type: TABLE DATA; Schema: public; Owner: postgres\n\
         --\n\
         \n\
         COPY public.t_nested_bad (id, v) FROM stdin;\n\
         1\t(a,\"{{1,2},{3,4}}\")\n\
         \\.\n\
         \n",
    )
    .unwrap();

    // The control: a 1-D array in the same composite field decodes and
    // renders back byte for byte.
    assert_eq!(
        rows(&path, "public.t_nested_ok", SchemaMode::Typed).await,
        rows(&path, "public.t_nested_ok", SchemaMode::Strings).await,
    );

    let err = try_rows(&path, "public.t_nested_bad", SchemaMode::Typed).await.unwrap_err();
    // The message names the remedy, and the remedy works.
    assert!(
        format!("{err}").contains("--schema-mode strings"),
        "the error names its one remaining remedy: {err}"
    );
    match err {
        Error::FieldDecode { table, column, declared_type, value, .. } => {
            assert_eq!(table, "public.t_nested_bad");
            assert_eq!(column, "v");
            assert_eq!(declared_type, "public.boxed");
            // The whole field, not the fragment that tripped it.
            assert_eq!(value, r#"(a,"{{1,2},{3,4}}")"#);
        }
        other => panic!("expected FieldDecode, got {other:?}"),
    }
    let strings = try_rows(&path, "public.t_nested_bad", SchemaMode::Strings).await.unwrap();
    assert_eq!(strings[0][1].as_deref(), Some(r#"(a,"{{1,2},{3,4}}")"#));
}

/// `NaN` bypasses `numeric(p,s)`'s own precision/scale check and has no
/// `Decimal128`/`Decimal256` representation, so it's a genuine decode
/// failure, not a bug — `public.t_numeric.v_small numeric(10,2)` carries it
/// for exactly this reason (`docs/design/decisions.md`, "D42"). Checks the
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
            &QueryOptions::default(),
            CacheMode::DISABLED,
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
            &QueryOptions::default(),
            CacheMode::DISABLED,
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

/// **`time` `24:00:00` is refused as the infinities are**: PostgreSQL's
/// inclusive bound, past the day Arrow's `Time64` holds, so the decoder
/// names it rather than writing a value Arrow forbids.
#[tokio::test]
async fn time_24_00_00_is_a_field_decode_error_naming_its_context() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let source = LocalFileSource::open(&path).unwrap();
        let err = read_table(
            &source,
            "public.t_time",
            &ScanOptions::default(),
            &QueryOptions::default(),
            CacheMode::DISABLED,
            |_| ControlFlow::Continue(()),
        )
        .await
        .unwrap_err();
        match err {
            Error::FieldDecode { column, declared_type, value, .. } => {
                assert_eq!(column, "v_time", "pg_dump {version}");
                assert_eq!(declared_type, "time without time zone", "pg_dump {version}");
                assert_eq!(value, "24:00:00", "pg_dump {version}");
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
            &QueryOptions::default(),
            CacheMode::DISABLED,
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
