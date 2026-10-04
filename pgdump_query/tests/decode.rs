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
    Error, Expr, LocalFileSource, Membership, NestedPlan, PostgresInvalidValues, Predicate,
    PredicateOp, QueryOptions, ScanOptions, StatisticsLevel, StatisticsRequest,
    StatisticsSelection, StatisticsTarget, UnrepresentableMode, map_file, read_table, render_field,
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

/// A hand-written dump whose `numeric(p,s)` fields are finer than their
/// scales, as `numeric_out` never writes them: typed at `(10,2)`, past the
/// precision at `(2,5)`, negative at `(3,-2)`, and held as text at `(100,2)`.
/// `t_past` and `t_past_array` each hold one field past its precision once
/// rounded, `0.001` at `(2,5)`: inside `Decimal128(5, 5)`'s own precision,
/// outside the column's.
fn typmod_dump(dir: &Path) -> PathBuf {
    let dump = dir.join("typmod.sql");
    let text = "CREATE TABLE public.t (\n    id integer,\n    v10_2 numeric(10,2),\n    \
                v2_5 numeric(2,5),\n    v3_m2 numeric(3,-2),\n    v100_2 numeric(100,2)\n);\n\n\
                CREATE TABLE public.t_past (\n    v2_5 numeric(2,5)\n);\n\n\
                CREATE TABLE public.t_past_array (\n    v2_5 numeric(2,5)[]\n);\n\n\
                COPY public.t (id, v10_2, v2_5, v3_m2, v100_2) FROM stdin;\n\
                1\t1.005\t0.000125\t1250\t1.005\n\
                2\t-0.004\t0.0000049\t-1249\t123.4449\n\
                3\t1.0\t.00001\t0\t-0.005\n\
                \\.\n\n\
                COPY public.t_past (v2_5) FROM stdin;\n0.001\n\\.\n\n\
                COPY public.t_past_array (v2_5) FROM stdin;\n{0.00012,0.001}\n\\.\n\n\
                SELECT 1;\n";
    std::fs::write(&dump, text).unwrap();
    dump
}

/// **A `numeric(p,s)` field reads as `COPY` stores it** (I51): a typed read
/// renders each typed column's values rounded to the scale, as the koji
/// replica (PG16) stores the same `COPY`, the text-held column keeps the
/// file's text and compares as the rounded value, a data-level `parse` sums
/// the rounded values, and a field past its precision is refused naming it —
/// where the Arrow decimal's own precision would hold it, inside an array too
/// — failing a data-level `parse` keying it, as a restore fails, and a query
/// decoding it.
#[tokio::test]
async fn a_numeric_field_is_rounded_to_its_scale_and_refused_past_its_precision() {
    let dir = tempfile::tempdir().unwrap();
    let path = typmod_dump(dir.path());

    let (typed, _) = try_rows_in(&path, "public.t", QueryOptions::default()).await.unwrap();
    let stored = [
        ["1", "1.01", "0.00013", "1300", "1.005"],
        ["2", "0.00", "0.00000", "-1200", "123.4449"],
        ["3", "1.00", "0.00001", "0", "-0.005"],
    ];
    let stored: Vec<Vec<Option<String>>> =
        stored.iter().map(|row| row.iter().map(|v| Some(v.to_string())).collect()).collect();
    assert_eq!(typed, stored);

    let kept = |column: &str, op, value: &str| QueryOptions {
        filter: Expr::all([Predicate { column: column.into(), op, value: Some(value.into()) }]),
        projection: Some(vec!["id".into()]),
        ..Default::default()
    };
    for (column, op, value, ids) in [
        ("v100_2", PredicateOp::Eq, "1.01", &["1"][..]),
        ("v100_2", PredicateOp::Lt, "0", &["3"]),
        ("v100_2", PredicateOp::Eq, "123.44", &["2"]),
        ("v100_2", PredicateOp::Gt, "1.005", &["1", "2"]),
        ("v10_2", PredicateOp::Ge, "1.01", &["1"]),
        ("v2_5", PredicateOp::Gt, "0.00012", &["1"]),
        ("v3_m2", PredicateOp::Lt, "-1000", &["2"]),
    ] {
        let (rows, _) = try_rows_in(&path, "public.t", kept(column, op, value)).await.unwrap();
        let ids: Vec<Vec<Option<String>>> =
            ids.iter().map(|id| vec![Some(id.to_string())]).collect();
        assert_eq!(rows, ids, "{column} {op:?} {value}");
    }

    let source = LocalFileSource::open(&path).unwrap();
    let line_offset = std::fs::read_to_string(&path).unwrap().find("\n0.001\n").unwrap() + 1;
    let disabled = CacheMode::DISABLED;
    match map_file(&source, &ScanOptions::default(), &disabled, &StatisticsRequest::DATA).await {
        Err(Error::FieldRefused { table, column, declared_type, line, line_offset: at, value }) => {
            assert_eq!((table.as_str(), column.as_str()), ("public.t_past", "v2_5"));
            assert_eq!(
                (declared_type.as_str(), at, value.as_str()),
                ("numeric(2,5)", line_offset as u64, "0.001")
            );
            assert_eq!(line, copy_line(&std::fs::read(&path).unwrap(), at));
        }
        other => panic!("expected the parse to refuse public.t_past.v2_5, got {other:?}"),
    }
    // Only `t_past` fails it: `t_past_array`'s leaf is never keyed, so the
    // parse decodes it no further than its census does, and the query below
    // is what refuses it.
    let past_at_metadata = StatisticsRequest {
        selection: StatisticsSelection {
            overrides: vec![(
                StatisticsTarget::Table("public.t_past".into()),
                StatisticsLevel::Metadata,
            )],
            ..StatisticsSelection::DATA
        },
        ..StatisticsRequest::DATA
    };
    let mode = CacheMode::enabled(cache::colocated_path(&path));
    let index =
        map_file(&source, &ScanOptions::default(), &mode, &past_at_metadata).await.unwrap().index;
    let block = index.blocks().find(|b| b.header.table == "t").unwrap();
    let statistics = block.statistics.as_deref().unwrap();
    let sums = |at: usize| statistics.columns[at].as_ref().unwrap().sums.clone();
    assert_eq!(sums(1), Some(vec![201]), "v10_2 sums 1.01, 0.00 and 1.00");
    assert_eq!(sums(2), Some(vec![14]), "v2_5 sums 0.00013, 0 and 0.00001");

    for table in ["public.t_past", "public.t_past_array"] {
        match try_rows_in(&path, table, QueryOptions::default()).await.unwrap_err() {
            Error::FieldDecode { column, declared_type, .. } => {
                assert_eq!(column, "v2_5");
                assert!(declared_type.starts_with("numeric(2,5)"), "{declared_type}");
            }
            other => panic!("{table}: expected FieldDecode, got {other:?}"),
        }
    }
}

/// A dump of one table, `public.t (id integer, v <declared>)`, holding `id`
/// 1 to 3 with `v` the dump's spelling but on row 2, which holds `field`;
/// `public.pair` and `public.short` are a composite and a domain over a
/// `character varying(2)`. Hand-written: no `pg_dump` writes a field past its
/// length.
fn char_typmod_dump(dir: &Path, declared: &str, field: &str) -> PathBuf {
    let dump = dir.join("char_typmod.sql");
    let text = format!(
        "CREATE TYPE public.pair AS (\n\tx character varying(2)\n);\n\n\
         CREATE DOMAIN public.short AS character varying(2);\n\n\
         CREATE TABLE public.t (\n    id integer,\n    v {declared}\n);\n\n\
         COPY public.t (id, v) FROM stdin;\n1\t\\N\n2\t{field}\n3\t\\N\n\\.\n\nSELECT 1;\n"
    );
    std::fs::write(&dump, text).unwrap();
    dump
}

/// **A `character varying(n)` or `character(n)` field longer than `n`
/// characters is refused wherever it is read, but for trailing blanks**
/// (I75): a data-level `parse` keying it fails naming it, a query decoding it
/// fails, an ordering filter keying it fails, and a strict parse finds it
/// beneath an array, a composite or a domain, which a default one leaves to
/// the query; told to ignore it, a query reads its text. A field longer only
/// by blanks, or by none counted in characters, is read as the file holds
/// it, and a literal of any length is read, the server coercing it with no
/// typmod. Bare `character` is `character(1)` and `bpchar` has no length.
#[tokio::test]
async fn a_character_field_past_its_length_is_refused_wherever_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let disabled = CacheMode::DISABLED;
    let read = |path: PathBuf, options: QueryOptions| async move {
        try_rows_in(&path, "public.t", options).await.map(|(rows, _)| rows)
    };
    let row2 = |v: &str| {
        vec![
            vec![Some("1".to_string()), None],
            vec![Some("2".to_string()), Some(v.to_string())],
            vec![Some("3".to_string()), None],
        ]
    };
    // Every case put to `pg_input_is_valid` under its type on the koji
    // replica (PG16), the composite's, array's and domain's included.
    for (declared, field) in [
        ("character varying(3)", "abc"),
        ("character varying(3)", "ab    "),
        ("character varying(3)", "\u{e9}\u{e9}\u{e9}  "),
        ("character(3)", "abc  "),
        ("character(3)", "a"),
        ("character", "a "),
        ("bpchar", "abcdef"),
        ("character varying", "abcdefgh"),
        ("character varying(2)[]", "{ab,\"a   \"}"),
        ("public.pair", "(ab)"),
        ("public.short", "ab "),
    ] {
        let path = char_typmod_dump(dir.path(), declared, field);
        let source = LocalFileSource::open(&path).unwrap();
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
            let scan = ScanOptions { postgres_invalid_values: invalid, ..ScanOptions::default() };
            map_file(&source, &scan, &disabled, &StatisticsRequest::DATA)
                .await
                .unwrap_or_else(|e| panic!("{declared} `{field}`: {invalid:?} refused: {e}"));
        }
        let rows = read(path.clone(), QueryOptions::default()).await.unwrap();
        assert_eq!(rows, row2(field), "{declared} `{field}`");
    }

    let line_offset =
        |path: &Path| std::fs::read_to_string(path).unwrap().find("\n2\t").unwrap() as u64 + 1;
    for (declared, field) in [
        ("character varying(3)", "abcd"),
        ("character varying(3)", "abc\t"),
        ("character varying(3)", " abc"),
        ("character varying(3)", "\u{e9}\u{e9}\u{e9}\u{e9}"),
        ("character(3)", "abcd"),
        ("character(3)", "\u{e9}\u{e9}\u{e9}\u{e9} "),
        ("character", "ab"),
        ("public.short", "abc"),
    ] {
        let escaped = field.replace('\t', "\\t");
        let path = char_typmod_dump(dir.path(), declared, &escaped);
        let source = LocalFileSource::open(&path).unwrap();
        match map_file(&source, &ScanOptions::default(), &disabled, &StatisticsRequest::DATA).await
        {
            Err(Error::FieldRefused {
                column,
                declared_type,
                line,
                line_offset: at,
                value,
                ..
            }) => {
                assert_eq!(
                    (column.as_str(), declared_type.as_str(), line, value.as_str()),
                    ("v", declared, 2, field),
                    "{declared} `{field}`"
                );
                assert_eq!(at, line_offset(&path), "{declared} `{field}`");
            }
            other => panic!("{declared} `{field}`: expected the parse to refuse it, got {other:?}"),
        }
        match read(path.clone(), QueryOptions::default()).await {
            Err(Error::FieldDecode { column, value, .. }) => {
                assert_eq!((column.as_str(), value.as_str()), ("v", field), "{declared}");
            }
            other => panic!("{declared} `{field}`: expected the read to refuse it, got {other:?}"),
        }
        let ordered = QueryOptions {
            filter: Expr::all([Predicate {
                column: "v".into(),
                op: PredicateOp::Ge,
                value: Some("a".into()),
            }]),
            projection: Some(vec!["id".into()]),
            ..Default::default()
        };
        match read(path.clone(), ordered).await {
            Err(Error::FieldDecode { column, .. }) => assert_eq!(column, "v", "{declared}"),
            other => {
                panic!("{declared} `{field}`: expected the filter to refuse it, got {other:?}")
            }
        }
        let ignoring = QueryOptions {
            postgres_invalid_values: PostgresInvalidValues::Ignore,
            ..Default::default()
        };
        let rows = read(path.clone(), ignoring).await.unwrap();
        assert_eq!(rows, row2(field), "{declared} `{field}`: read as written when ignored");
    }

    // Beneath a container a default parse keys nothing; the query and a
    // strict parse refuse it.
    for (declared, field) in [
        ("character varying(2)[]", "{ab,abc}"),
        ("public.pair", "(abc)"),
        ("public.pair[]", "{(ab),(abc)}"),
    ] {
        let path = char_typmod_dump(dir.path(), declared, field);
        let source = LocalFileSource::open(&path).unwrap();
        map_file(&source, &ScanOptions::default(), &disabled, &StatisticsRequest::DATA)
            .await
            .unwrap_or_else(|e| panic!("{declared} `{field}`: a default parse refused: {e}"));
        let strict = ScanOptions {
            postgres_invalid_values: PostgresInvalidValues::Strict,
            ..ScanOptions::default()
        };
        match map_file(&source, &strict, &disabled, &StatisticsRequest::DATA).await {
            Err(Error::FieldRefused { column, line, value, .. }) => {
                assert_eq!((column.as_str(), line, value.as_str()), ("v", 2, field), "{declared}");
            }
            other => {
                panic!("{declared} `{field}`: expected a strict parse to refuse it, got {other:?}")
            }
        }
        match read(path.clone(), QueryOptions::default()).await {
            Err(Error::FieldDecode { column, .. }) => assert_eq!(column, "v", "{declared}"),
            other => panic!("{declared} `{field}`: expected the read to refuse it, got {other:?}"),
        }
    }

    // A literal past the length is read, in either semantics of order.
    let path = char_typmod_dump(dir.path(), "character varying(3)", "abc");
    for (op, ids) in [(PredicateOp::Eq, &[][..]), (PredicateOp::Lt, &["2"][..])] {
        let filtered = QueryOptions {
            filter: Expr::all([Predicate { column: "v".into(), op, value: Some("abcdef".into()) }]),
            projection: Some(vec!["id".into()]),
            ..Default::default()
        };
        let rows = read(path.clone(), filtered).await.unwrap();
        let ids: Vec<Vec<Option<String>>> =
            ids.iter().map(|id| vec![Some(id.to_string())]).collect();
        assert_eq!(rows, ids, "{op:?}");
    }
}

/// A dump whose `public.t` holds `id` 1 to 3 and `v` 5, NULL and 7, the
/// table declared by `ddl`, which ends in `;`, after two domains: `public.nn`,
/// an `integer` declared `NOT NULL`, and `public.over_nn` over it.
/// Hand-written: no `pg_dump` writes a NULL into a `NOT NULL` column.
fn not_null_dump(dir: &Path, ddl: &str) -> PathBuf {
    let dump = dir.join("not_null.sql");
    let text = format!(
        "CREATE DOMAIN public.nn AS integer NOT NULL;\n\n\
         CREATE DOMAIN public.over_nn AS public.nn;\n\n\
         {ddl}\n\n\
         COPY public.t (id, v) FROM stdin;\n1\t5\n2\t\\N\n3\t7\n\\.\n\nSELECT 1;\n"
    );
    std::fs::write(&dump, text).unwrap();
    dump
}

/// **A NULL in a column its declaration makes `NOT NULL` is refused wherever
/// it is read** (I76) — on the column, at the table, by a primary key, by
/// `SET NOT NULL`, through a parent, as a typed table's option, through a
/// domain or a domain over one, whatever the column's type resolves to: a
/// data-level `parse` keying it fails naming it by its `COPY` line, as a
/// strict one does, and so does a metadata-level strict one, which a default
/// one leaves to the query; a query reading it fails, a filter term naming
/// it — `IS NOT NULL` included — fails, and the strings schema mode, which
/// reads no declaration, reads it. Told to ignore it, a query reads the NULL,
/// and a parse goes past it and records it, so a default parse over the cache
/// fails with it. A `NOT NULL` that does not bind the column — a parent's `NO
/// INHERIT`, a `CHECK` — refuses nothing.
#[tokio::test]
async fn a_null_in_a_not_null_column_is_refused_wherever_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let disabled = CacheMode::DISABLED;
    let read = |path: PathBuf, options: QueryOptions| async move {
        try_rows_in(&path, "public.t", options).await.map(|(rows, _)| rows)
    };
    let rows = |v: &[Option<&str>]| -> Vec<Vec<Option<String>>> {
        (1..=3).zip(v).map(|(id, v)| vec![Some(id.to_string()), v.map(str::to_string)]).collect()
    };
    let with_null = rows(&[Some("5"), None, Some("7")]);
    let line_offset =
        |path: &Path| std::fs::read_to_string(path).unwrap().find("\n2\t").unwrap() as u64 + 1;
    let strict = ScanOptions {
        postgres_invalid_values: PostgresInvalidValues::Strict,
        ..Default::default()
    };
    let ignoring = ScanOptions {
        postgres_invalid_values: PostgresInvalidValues::Ignore,
        ..Default::default()
    };

    for ddl in [
        "CREATE TABLE public.t (\n    id integer,\n    v integer NOT NULL\n);",
        "CREATE TABLE public.t (\n    id integer,\n    v integer CONSTRAINT t_v NOT NULL NO INHERIT\n);",
        "CREATE TABLE public.t (\n    id integer,\n    v integer,\n    NOT NULL v\n);",
        "CREATE TABLE public.t (\n    id integer,\n    v integer,\n    PRIMARY KEY (v)\n);",
        "CREATE TABLE public.t (\n    id integer,\n    v integer\n);\n\
         ALTER TABLE ONLY public.t ALTER COLUMN v SET NOT NULL;",
        "CREATE TABLE public.p (\n    v integer NOT NULL\n);\n\n\
         CREATE TABLE public.t (\n    id integer\n)\nINHERITS (public.p);",
        "CREATE TYPE public.pair AS (\n\tid integer,\n\tv integer\n);\n\n\
         CREATE TABLE public.t OF public.pair (\n    v NOT NULL\n);",
        "CREATE TABLE public.t (\n    id integer,\n    v public.nn\n);",
        "CREATE TABLE public.t (\n    id integer,\n    v public.over_nn\n);",
        "CREATE TYPE public.opaque;\n\n\
         CREATE TABLE public.t (\n    id integer,\n    v public.opaque NOT NULL\n);",
    ] {
        let path = not_null_dump(dir.path(), ddl);
        let source = LocalFileSource::open(&path).unwrap();
        for (scan, request) in [
            (ScanOptions::default(), StatisticsRequest::DATA),
            (strict.clone(), StatisticsRequest::DATA),
            (strict.clone(), StatisticsRequest::METADATA),
        ] {
            match map_file(&source, &scan, &disabled, &request).await {
                Err(Error::NullRefused { table, column, line, line_offset: at }) => {
                    assert_eq!(
                        (table.as_str(), column.as_str(), line, at),
                        ("public.t", "v", Some(2), line_offset(&path)),
                        "{ddl}"
                    );
                }
                other => panic!("{ddl}: expected the parse to refuse the NULL, got {other:?}"),
            }
        }
        map_file(&source, &ScanOptions::default(), &disabled, &StatisticsRequest::METADATA)
            .await
            .unwrap_or_else(|e| panic!("{ddl}: a metadata-level parse refused: {e}"));
        match read(path.clone(), QueryOptions::default()).await {
            Err(Error::NullRefused { column, line: None, line_offset: at, .. }) => {
                assert_eq!((column.as_str(), at), ("v", line_offset(&path)), "{ddl}");
            }
            other => panic!("{ddl}: expected the read to refuse the NULL, got {other:?}"),
        }
        let filtered = QueryOptions {
            filter: Expr::all([Predicate {
                column: "v".into(),
                op: PredicateOp::IsNotNull,
                value: None,
            }]),
            projection: Some(vec!["id".into()]),
            ..Default::default()
        };
        match read(path.clone(), filtered).await {
            Err(Error::NullRefused { column, .. }) => assert_eq!(column, "v", "{ddl}"),
            other => panic!("{ddl}: expected the filter to refuse the NULL, got {other:?}"),
        }
        // Projected: the query emits an inherited column ahead of the
        // table's own.
        let both = || Some(vec!["id".to_string(), "v".to_string()]);
        let untyped = QueryOptions {
            schema_mode: SchemaMode::Strings,
            projection: both(),
            ..Default::default()
        };
        assert_eq!(read(path.clone(), untyped).await.unwrap(), with_null, "{ddl}: strings");
        let ignored = QueryOptions {
            postgres_invalid_values: PostgresInvalidValues::Ignore,
            projection: both(),
            ..Default::default()
        };
        assert_eq!(read(path.clone(), ignored).await.unwrap(), with_null, "{ddl}: ignored");

        let mode = CacheMode::enabled(cache::colocated_path(&path));
        let _ = std::fs::remove_file(cache::colocated_path(&path));
        let mapped = map_file(&source, &ignoring, &mode, &StatisticsRequest::DATA).await.unwrap();
        let block = mapped.index.blocks().find(|b| b.header.table == "t").unwrap();
        let recorded = block.ignored_refusals.as_ref().expect("the NULL is recorded");
        assert_eq!(
            (recorded.columns[0].first.value.as_deref(), recorded.columns[0].count),
            (None, 1)
        );
        match map_file(&source, &ScanOptions::default(), &mode, &StatisticsRequest::DATA).await {
            Err(Error::FieldRefusedRecorded { refused, .. }) => {
                assert!(matches!(*refused, Error::NullRefused { line: Some(2), .. }), "{ddl}");
            }
            other => panic!("{ddl}: expected the recorded refusal, got {other:?}"),
        }
        let _ = std::fs::remove_file(cache::colocated_path(&path));
    }

    for ddl in [
        "CREATE TABLE public.t (\n    id integer,\n    v integer\n);",
        "CREATE TABLE public.p (\n    v integer NOT NULL NO INHERIT\n);\n\n\
         CREATE TABLE public.t (\n    id integer\n)\nINHERITS (public.p);",
        "CREATE TABLE public.t (\n    id integer,\n    v integer CHECK ((v IS NOT NULL))\n);",
    ] {
        let path = not_null_dump(dir.path(), ddl);
        let source = LocalFileSource::open(&path).unwrap();
        for scan in [ScanOptions::default(), strict.clone()] {
            map_file(&source, &scan, &disabled, &StatisticsRequest::DATA)
                .await
                .unwrap_or_else(|e| panic!("{ddl}: refused: {e}"));
        }
        let both = QueryOptions {
            projection: Some(vec!["id".to_string(), "v".to_string()]),
            ..Default::default()
        };
        assert_eq!(read(path, both).await.unwrap(), with_null, "{ddl}");
    }
}

/// The line `COPY` numbers the row at `line_offset` in `text`, a restore
/// counting from 1 at its block's first data line (`copyfrom.c`'s
/// `CopyFromErrorCallback`): read off the file, not off the parse.
fn copy_line(text: &[u8], line_offset: u64) -> u64 {
    let before = &text[..line_offset as usize];
    let header = b" FROM stdin;\n";
    let data = before.windows(header.len()).rposition(|w| w == header).unwrap() + header.len();
    before[data..].iter().filter(|&&b| b == b'\n').count() as u64 + 1
}

/// **Every fixture holding a field PostgreSQL refuses fails a data-level
/// parse there, at every major** (`common::REFUSED_FIELDS`), naming the table,
/// the column and the line the field is on, by `COPY`'s number and by its
/// offset, as a restore of it fails that table's `COPY`; mapped with that column at the metadata level, as the
/// sweeps map it (`common::sweep_request`), it parses.
#[tokio::test]
async fn every_refused_field_fails_a_data_level_parse_reading_it() {
    let mut copies = 0;
    for path in common::all_fixtures() {
        let Some(refused) = common::refused_field(&path) else { continue };
        copies += 1;
        let source = LocalFileSource::open(&path).unwrap();
        let (options, mode) = (ScanOptions::default(), CacheMode::DISABLED);
        match map_file(&source, &options, &mode, &StatisticsRequest::DATA).await {
            Err(Error::FieldRefused { table, column, line, line_offset, value, .. }) => {
                assert_eq!((table.as_str(), column.as_str()), (refused.table, refused.column));
                let text = std::fs::read(&path).unwrap();
                assert_eq!(line, copy_line(&text, line_offset), "{}", path.display());
                let line = &text[line_offset as usize..];
                let line = &line[..line.iter().position(|&b| b == b'\n').unwrap()];
                let line = String::from_utf8_lossy(line);
                assert!(line.split('\t').any(|field| field == value), "{line:?} holds {value:?}");
            }
            other => panic!("{}: expected the parse to refuse, got {other:?}", path.display()),
        }
        let run = map_file(&source, &options, &mode, &common::sweep_request(&path)).await;
        run.unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    assert_eq!(copies, common::REFUSED_FIELDS.len() * common::VERSIONS.len());
}

/// **A strict parse of every fixture refuses no field PostgreSQL wrote but
/// those [`common::REFUSED_FIELDS`] lists**, which it refuses as a default
/// parse does, at the data level and at the metadata level alike — so every
/// field a `pg_dump` writes, nested elements and range bounds among them, is
/// one its check reads as the server does — and records every block checked
/// in full.
#[tokio::test]
async fn a_strict_parse_refuses_no_fixture_field_but_the_listed_ones() {
    let strict = ScanOptions {
        postgres_invalid_values: PostgresInvalidValues::Strict,
        ..Default::default()
    };
    for path in common::all_fixtures() {
        let source = LocalFileSource::open(&path).unwrap();
        for request in [StatisticsRequest::DATA, StatisticsRequest::METADATA] {
            let run = map_file(&source, &strict, &CacheMode::DISABLED, &request).await;
            match (common::refused_field(&path), run) {
                (Some(refused), Err(Error::FieldRefused { table, column, .. })) => {
                    assert_eq!((table.as_str(), column.as_str()), (refused.table, refused.column));
                }
                (None, Ok(run)) => {
                    let unchecked: Vec<_> = run
                        .index
                        .blocks()
                        .filter(|b| !b.checked_in_full)
                        .map(|b| b.header.qualified_name())
                        .collect();
                    assert!(unchecked.is_empty(), "{}: {unchecked:?}", path.display());
                }
                (_, other) => panic!("{}: {other:?}", path.display()),
            }
        }
    }
}

/// **Told to ignore them, a parse goes on past a float PostgreSQL refuses,
/// and a query reads it as `float8out` meant it** (I57), at every major: the
/// largest finite value of its sign, printed and filtered alike, a cache's
/// statistics ruling out no group holding it; told nothing, the query
/// refuses it, and a literal spelled as the field is refused either way
/// (`docs/design/decisions.md`, "D103").
#[tokio::test]
async fn a_float_past_its_range_is_read_as_the_largest_only_when_told_to_ignore_it() {
    let ignore = PostgresInvalidValues::Ignore;
    let options = |filter: Expr, invalid| QueryOptions {
        projection: Some(vec!["id".into(), "v_double".into()]),
        filter,
        postgres_invalid_values: invalid,
        ..Default::default()
    };
    let term = |op, value: &str| {
        Expr::Term(Predicate { column: "v_double".into(), op, value: Some(value.into()) })
    };
    let row = |id: &str, v: &str| vec![Some(id.to_string()), Some(v.to_string())];
    let (largest, lowest) =
        (row("2", "1.7976931348623157e+308"), row("1", "-1.7976931348623157e+308"));
    let filters = [
        (term(PredicateOp::Eq, "1.7976931348623157e+308"), vec![largest.clone()]),
        (term(PredicateOp::Lt, "-1e308"), vec![lowest.clone(), row("3", "-Infinity")]),
        (
            Expr::In(Membership {
                column: "v_double".into(),
                values: vec![Some("-1.7976931348623157e+308".into()), Some("0".into())],
            }),
            vec![lowest.clone()],
        ),
    ];
    for version in common::VERSIONS {
        let fixture = types_fixture(version, "extra-float-digits-0");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql");
        std::fs::copy(&fixture, &path).unwrap();
        let source = LocalFileSource::open(&path).unwrap();
        let scan = ScanOptions { postgres_invalid_values: ignore, ..ScanOptions::default() };
        let cached = CacheMode::enabled(cache::colocated_path(&path));
        map_file(&source, &scan, &cached, &StatisticsRequest::DATA)
            .await
            .unwrap_or_else(|e| panic!("{version}: an ignoring parse refused: {e}"));

        let all = Expr::And(Vec::new());
        match try_rows_in(&path, "public.t_extremes", options(all.clone(), Default::default()))
            .await
        {
            Err(Error::FieldDecode { column, value, .. }) => {
                assert_eq!(column, "v_double", "{version}");
                assert!(value.ends_with("1.79769313486232e+308"), "{version}: {value}");
            }
            other => panic!("{version}: expected the read to refuse, got {other:?}"),
        }
        let (read, _) =
            try_rows_in(&path, "public.t_extremes", options(all, ignore)).await.unwrap();
        assert!(read.contains(&largest) && read.contains(&lowest), "{version}: {read:?}");
        for (filter, want) in &filters {
            for mode in [CacheMode::DISABLED, cached.clone()] {
                let mut stream = table_stream(
                    &source,
                    "public.t_extremes",
                    ScanOptions::default(),
                    options(filter.clone(), ignore),
                    None,
                    mode,
                );
                let mut kept = Vec::new();
                while let Some(batch) = stream.next().await.transpose().unwrap() {
                    for r in 0..batch.num_rows() {
                        kept.push(
                            batch
                                .columns()
                                .iter()
                                .map(|c| render_field(c.as_ref(), r, &NestedPlan::Scalar).unwrap())
                                .collect::<Vec<_>>(),
                        );
                    }
                }
                assert_eq!(&kept, want, "{version}: {filter:?}");
            }
        }
        let spelled = options(term(PredicateOp::Eq, "1.79769313486232e+308"), ignore);
        match try_rows_in(&path, "public.t_extremes", spelled).await {
            Err(Error::PredicateValueDecode { .. }) => {}
            other => panic!("{version}: expected the literal refused, got {other:?}"),
        }
    }
}

/// **A `bytea_output = escape` dump reads as its `hex` twin does** (I4,
/// I56): a typed read of `t_bytea` renders the rows `default`'s does, and
/// every filter over `v_bytea` — each value's `=`, `!=`, `<` and `>=`, and an
/// `IN` of every value, each written in either form — keeps the rows it keeps
/// there.
#[tokio::test]
async fn an_escape_output_dump_reads_and_filters_as_its_hex_twin() {
    async fn kept(path: &Path, filter: &Expr) -> Vec<Vec<Option<String>>> {
        let options = QueryOptions { filter: filter.clone(), ..Default::default() };
        try_rows_in(path, "public.t_bytea", options).await.unwrap().0
    }
    let table = "public.t_bytea";
    let mut matched = 0;
    for version in common::VERSIONS {
        let hex = types_fixture(version, "default");
        let escape = types_fixture(version, "bytea-output-escape");
        let typed = rows(&hex, table, SchemaMode::Typed).await;
        assert_eq!(rows(&escape, table, SchemaMode::Typed).await, typed, "{version}");
        let spelled = |rows: Vec<Vec<Option<String>>>| {
            rows.into_iter().filter_map(|row| row[1].clone()).collect::<Vec<_>>()
        };
        let mut literals = spelled(rows(&hex, table, SchemaMode::Strings).await);
        let escaped = spelled(rows(&escape, table, SchemaMode::Strings).await);
        assert_ne!(literals, escaped, "{version}: the fixture holds the escape form");
        literals.extend(escaped);
        let term = |op, value: &str| {
            Expr::Term(Predicate { column: "v_bytea".into(), op, value: Some(value.into()) })
        };
        let mut filters: Vec<Expr> = literals
            .iter()
            .flat_map(|literal| {
                [PredicateOp::Eq, PredicateOp::Ne, PredicateOp::Lt, PredicateOp::Ge]
                    .map(|op| term(op, literal))
            })
            .collect();
        filters.push(Expr::In(Membership {
            column: "v_bytea".into(),
            values: literals.iter().cloned().map(Some).collect(),
        }));
        for filter in &filters {
            let want = kept(&hex, filter).await;
            assert_eq!(kept(&escape, filter).await, want, "{version}: {filter:?}");
            if matches!(filter, Expr::Term(Predicate { op: PredicateOp::Eq, .. })) {
                matched += want.len();
            }
        }
    }
    // Each of the three values, in both spellings, at six majors.
    assert_eq!(matched, 3 * 2 * 6);
}
