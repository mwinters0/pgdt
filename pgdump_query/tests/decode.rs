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
//! numeric); this file instead proves those same values surface correctly —
//! read as NULL, or refused — through the *real* pipeline — preamble parse, resolution, and
//! decode together — using `public.t_numeric`/`t_date`/`t_timestamp`'s own
//! real boundary rows rather than hand-built ones.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::datatypes::DataType;
use futures::StreamExt;
use pgdump_query::cache::{self, CacheMode};
use pgdump_query::resolve::{ColumnResolution, SchemaMode};
use pgdump_query::{
    Error, Expr, LocalFileSource, NestedPlan, Predicate, PredicateOp, QueryOptions, ScanOptions,
    StatisticsRequest, UnrepresentableMode, map_file, read_table, render_field, table_stream,
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
    let options = QueryOptions { schema_mode: mode, ..Default::default() };
    Ok(try_rows_in(path, table, options).await?.0)
}

/// [`try_rows`] under `options`, and the schema the rows came in.
async fn try_rows_in(
    path: &Path,
    table: &str,
    options: QueryOptions,
) -> pgdump_query::Result<(Vec<Vec<Option<String>>>, pgdump_query::ResolvedSchema)> {
    let source = LocalFileSource::open(path).unwrap();
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
    Ok((out, stream.resolved_schema()))
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
/// Hand-written, though the types fixture's `t_composite_matrix` holds the
/// same shape, so that it sits beside its control: `t_nested_ok` says the
/// literal below really is
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

/// **A value its column's type cannot hold, through the real pipeline**:
/// read as NULL by default — each row whose `column` the text spells `value`
/// is NULL there in the typed read — in the untyped mode as its text, that
/// column alone read as `Utf8View` and every row as `Strings` mode reads it,
/// and in the refuse mode `Error::Unrepresentable` before a row is read,
/// naming the table, the column, its declared type and how many values the
/// typed read nulls (`docs/design/decisions.md`, "D98", "D99"). `Strings`
/// mode never looks at the DDL, so the value passes through as its text.
async fn read_as_null_or_refused(table: &str, column: &str, declared: &str, value: &str) {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let typed = rows(&path, table, SchemaMode::Typed).await;
        let strings = rows(&path, table, SchemaMode::Strings).await;
        let index = resolved_schema(&path, table).await.schema.index_of(column).unwrap();
        let spelled: Vec<usize> = (0..strings.len())
            .filter(|&row| strings[row][index].as_deref() == Some(value))
            .collect();
        assert!(!spelled.is_empty(), "pg_dump {version}: {table} holds `{value}` as text");
        for row in spelled {
            assert_eq!(typed[row][index], None, "pg_dump {version}: {table} row {row}");
        }
        let nulled = (0..strings.len())
            .filter(|&row| strings[row][index].is_some() && typed[row][index].is_none())
            .count() as u64;

        let untyped =
            QueryOptions { unrepresentable: UnrepresentableMode::Text, ..Default::default() };
        let (text, widened) = try_rows_in(&path, table, untyped).await.unwrap();
        assert_eq!(text, strings, "pg_dump {version}: the untyped mode keeps every value");
        let declared_schema = resolved_schema(&path, table).await;
        for (i, field) in widened.schema.fields().iter().enumerate() {
            let holds_one =
                (0..strings.len()).any(|row| strings[row][i].is_some() && typed[row][i].is_none());
            let (resolution, data_type) = match holds_one {
                true => (ColumnResolution::UnrepresentableValues, &DataType::Utf8View),
                false => (
                    declared_schema.columns[i].clone(),
                    declared_schema.schema.field(i).data_type(),
                ),
            };
            assert_eq!(
                (&widened.columns[i], field.data_type()),
                (&resolution, data_type),
                "pg_dump {version}: {table}.{}, which the typed read nulls a value of: {holds_one}",
                field.name()
            );
        }

        let source = LocalFileSource::open(&path).unwrap();
        let refuse =
            QueryOptions { unrepresentable: UnrepresentableMode::Refuse, ..Default::default() };
        let err = read_table(
            &source,
            table,
            &ScanOptions::default(),
            &refuse,
            CacheMode::DISABLED,
            |_| ControlFlow::Continue(()),
        )
        .await
        .unwrap_err();
        match err {
            Error::Unrepresentable { table: named, column: at, declared_type, values } => {
                assert_eq!(named, table, "pg_dump {version}");
                assert_eq!(at, column, "pg_dump {version}");
                assert_eq!(declared_type, declared, "pg_dump {version}");
                assert_eq!(values, nulled, "pg_dump {version}");
            }
            other => panic!("pg_dump {version}: expected Unrepresentable, got {other:?}"),
        }
    }
}

/// `NaN` bypasses `numeric(p,s)`'s own precision/scale check and has no
/// `Decimal128`/`Decimal256` representation (`docs/design/decisions.md`,
/// "D42"); `public.t_numeric.v_small numeric(10,2)` carries it for exactly
/// this reason.
#[tokio::test]
async fn nan_numeric_is_read_as_null_or_refused_naming_its_context() {
    read_as_null_or_refused("public.t_numeric", "v_small", "numeric(10,2)", "NaN").await;
}

/// `infinity`/`-infinity` are real PostgreSQL date values with no `Date32`
/// sentinel.
#[tokio::test]
async fn date_infinity_is_read_as_null_or_refused_naming_its_context() {
    read_as_null_or_refused("public.t_date", "v_date", "date", "infinity").await;
}

/// **`time` `24:00:00` is one as the infinities are**: PostgreSQL's
/// inclusive bound, past the day Arrow's `Time64` holds.
#[tokio::test]
async fn time_24_00_00_is_read_as_null_or_refused_naming_its_context() {
    read_as_null_or_refused("public.t_time", "v_time", "time without time zone", "24:00:00").await;
}

/// Same as the date case, for `timestamp without time zone`.
#[tokio::test]
async fn timestamp_infinity_is_read_as_null_or_refused_naming_its_context() {
    read_as_null_or_refused(
        "public.t_timestamp",
        "v_ts",
        "timestamp without time zone",
        "infinity",
    )
    .await;
}

/// Columns whose scale PostgreSQL 15 and later admit and no fixture holds:
/// past the precision (`v2_5`), past `Decimal128`'s (`v1_40`), past every
/// Arrow decimal's (`v1_200`) and below `i8` (`v2_m129`), each value as
/// `numeric_out` writes it (I51) — row 1 positive, row 2 zero (`-0.00099` in
/// `v2_5`), row 4 negative (zero in `v2_5`).
fn wide_scale_dump(dir: &Path) -> PathBuf {
    let dump = dir.join("wide_scale.sql");
    let fraction =
        |digits: usize, tail: &str| format!("0.{}{tail}", "0".repeat(digits - tail.len()));
    let rows = [
        [
            "1",
            "0.00012",
            &fraction(40, "3"),
            &fraction(200, "5"),
            &format!("12{}", "0".repeat(129)),
        ],
        ["2", "-0.00099", &fraction(40, ""), &fraction(200, ""), "0"],
        ["3", "\\N", "\\N", "\\N", "\\N"],
        [
            "4",
            "0.00000",
            &format!("-{}", fraction(40, "3")),
            &format!("-{}", fraction(200, "5")),
            &format!("-12{}", "0".repeat(129)),
        ],
    ];
    let rows: String = rows.iter().map(|row| format!("{}\n", row.join("\t"))).collect();
    let text = format!(
        "CREATE TABLE public.t (\n    id integer,\n    v2_5 numeric(2,5),\n    \
         v1_40 numeric(1,40),\n    v1_200 numeric(1,200),\n    v2_m129 numeric(2,-129)\n);\n\n\
         COPY public.t (id, v2_5, v1_40, v1_200, v2_m129) FROM stdin;\n{rows}\\.\n\nSELECT 1;\n"
    );
    std::fs::write(&dump, text).unwrap();
    dump
}

/// **A scale Arrow's decimal can carry is typed at a precision widened to
/// it, and one it cannot is text compared by value**: a typed query renders
/// every value back as written, in the default and the refuse mode, a
/// data-level `parse` counts nothing unrepresentable, and `>`, `<` and `=`
/// against zero keep the rows PostgreSQL would.
#[tokio::test]
async fn a_scale_past_the_precision_or_arrows_reach_reads_every_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = wide_scale_dump(dir.path());

    let (typed, schema) = try_rows_in(&path, "public.t", QueryOptions::default()).await.unwrap();
    assert_eq!(typed, rows(&path, "public.t", SchemaMode::Strings).await);
    assert!(
        typed.iter().all(|row| row[0].as_deref() == Some("3") || row.iter().all(Option::is_some))
    );
    let types: Vec<&DataType> = schema.schema.fields().iter().map(|f| f.data_type()).collect();
    assert_eq!(
        types,
        [
            &DataType::Int32,
            &DataType::Decimal128(5, 5),
            &DataType::Decimal256(40, 40),
            &DataType::Utf8View,
            &DataType::Utf8View,
        ]
    );

    let refuse =
        QueryOptions { unrepresentable: UnrepresentableMode::Refuse, ..Default::default() };
    let (refused, _) = try_rows_in(&path, "public.t", refuse).await.unwrap();
    assert_eq!(refused, typed, "the refuse mode answers");

    let source = LocalFileSource::open(&path).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&path));
    let index = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA)
        .await
        .unwrap()
        .index;
    let mut blocks = 0;
    for block in index.blocks() {
        let counts = block.unrepresentable.as_deref().expect("a data-level block is counted");
        assert!(counts.iter().all(|c| c.is_zero()), "{counts:?}");
        blocks += 1;
    }
    assert_eq!(blocks, 1);

    let against_zero = |column: &str, op| QueryOptions {
        filter: Expr::all([Predicate { column: column.into(), op, value: Some("0".into()) }]),
        projection: Some(vec!["id".into()]),
        ..Default::default()
    };
    for (column, above, below, zero) in [
        ("v2_5", "1", "2", "4"),
        ("v1_40", "1", "4", "2"),
        ("v1_200", "1", "4", "2"),
        ("v2_m129", "1", "4", "2"),
    ] {
        for (op, id) in
            [(PredicateOp::Gt, above), (PredicateOp::Lt, below), (PredicateOp::Eq, zero)]
        {
            let (kept, _) = try_rows_in(&path, "public.t", against_zero(column, op)).await.unwrap();
            assert_eq!(kept, [vec![Some(id.to_string())]], "{column} {op:?} 0");
        }
    }
}

/// Columns at four negative scales (PostgreSQL 15 and later; no fixture
/// holds one), written as `numeric_out` writes them (I51): zero as `0`
/// whatever the scale, every other value ending in the scale's zeros. `v40`
/// is past `Decimal128`'s precision.
fn negative_scale_dump(dir: &Path) -> PathBuf {
    let dump = dir.join("negative_scale.sql");
    let text = "CREATE TABLE public.t (\n    id integer,\n    v1 numeric(3,-1),\n    \
                v2 numeric(3,-2),\n    v5 numeric(4,-5),\n    v40 numeric(40,-3)\n);\n\n\
                COPY public.t (id, v1, v2, v5, v40) FROM stdin;\n\
                1\t0\t0\t0\t0\n\
                2\t120\t1200\t-100000\t1234000\n\
                3\t\\N\t\\N\t\\N\t\\N\n\
                4\t0\t-9900\t0\t0\n\
                \\.\n\nSELECT 1;\n";
    std::fs::write(&dump, text).unwrap();
    dump
}

/// **A negative scale's zero is a value of its column**: typed, it decodes
/// and renders back as the `0` the file holds, so the default null mode reads
/// no value of these columns as NULL, the refuse mode answers rather than
/// refusing, a data-level `parse` counts nothing unrepresentable, and `= 0`
/// keeps the zeros.
#[tokio::test]
async fn a_negative_scale_zero_reads_as_zero_in_every_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = negative_scale_dump(dir.path());

    let (typed, schema) = try_rows_in(&path, "public.t", QueryOptions::default()).await.unwrap();
    assert_eq!(typed, rows(&path, "public.t", SchemaMode::Strings).await);
    let zeros = ["1", "0", "0", "0", "0"].map(|v| Some(v.to_string()));
    assert_eq!(typed[0], zeros, "zero renders back as written");
    let types: Vec<&DataType> = schema.schema.fields().iter().map(|f| f.data_type()).collect();
    assert_eq!(
        types,
        [
            &DataType::Int32,
            &DataType::Decimal128(3, -1),
            &DataType::Decimal128(3, -2),
            &DataType::Decimal128(4, -5),
            &DataType::Decimal256(40, -3),
        ]
    );

    let refuse =
        QueryOptions { unrepresentable: UnrepresentableMode::Refuse, ..Default::default() };
    let (refused, _) = try_rows_in(&path, "public.t", refuse).await.unwrap();
    assert_eq!(refused, typed, "the refuse mode answers");

    let source = LocalFileSource::open(&path).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(&path));
    let index = map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA)
        .await
        .unwrap()
        .index;
    let mut blocks = 0;
    for block in index.blocks() {
        let counts = block.unrepresentable.as_deref().expect("a data-level block is counted");
        assert!(counts.iter().all(|c| c.is_zero()), "{counts:?}");
        blocks += 1;
    }
    assert_eq!(blocks, 1);

    let zero = |column: &str| QueryOptions {
        filter: Expr::all([Predicate {
            column: column.into(),
            op: PredicateOp::Eq,
            value: Some("0".into()),
        }]),
        projection: Some(vec!["id".into()]),
        ..Default::default()
    };
    for (column, ids) in [("v1", ["1", "4"]), ("v5", ["1", "4"]), ("v40", ["1", "4"])] {
        let (kept, _) = try_rows_in(&path, "public.t", zero(column)).await.unwrap();
        assert_eq!(kept, ids.map(|id| vec![Some(id.to_string())]), "{column} = 0");
    }
    let (kept, _) = try_rows_in(&path, "public.t", zero("v2")).await.unwrap();
    assert_eq!(kept, [vec![Some("1".to_string())]], "v2 = 0");
}
