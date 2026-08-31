//! Typed ordering predicates: `<`, `<=`, `>`, `>=` over the generated
//! `types` fixture (`docs/design/architecture.md`, "Predicates", the
//! ordering register).
//!
//! The library-level tests only — everything here drives `table_stream`
//! directly against real `pg_dump` output, so each column is compared
//! through the decoder its own DDL resolved to. The unit tests in
//! `src/predicate.rs` cover the comparison itself against hand-built
//! schemas; the flags are pinned in
//! `pgdump_query-cli/tests/query_ordering.rs`.

use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    Error, LocalFileSource, OrderingDivergence, Predicate, PredicateOp, QueryOptions, ScanOptions,
    SchemaMode, table_stream,
};

mod common;
use common::{rows_of, types_fixture};

fn term(column: &str, op: PredicateOp, value: &str) -> Predicate {
    Predicate { column: column.into(), op, value: Some(value.into()) }
}

/// Drain `table` from the 16 `types` fixture under `options`, returning the
/// decoded rows.
async fn drain(
    table: &str,
    options: QueryOptions,
) -> pgdump_query::Result<Vec<Vec<Option<String>>>> {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let mut stream =
        table_stream(&source, table, ScanOptions::default(), options, None, CacheMode::Disabled);
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await.transpose()? {
        rows.extend(rows_of(&batch));
    }
    Ok(rows)
}

/// The surviving values of the single projected column, in file order.
async fn kept(table: &str, column: &str, filters: Vec<Predicate>) -> Vec<Option<String>> {
    let options =
        QueryOptions { filters, projection: Some(vec![column.to_string()]), ..Default::default() };
    drain(table, options).await.unwrap().into_iter().map(|r| r[0].clone()).collect()
}

/// A spread of the register's agreeing rows, each through its own decoder
/// against the fixture's own values, and each a case a *text* comparison
/// would get wrong: the negative integer and the negative decimal both sort
/// last as text, `NaN` is PostgreSQL's largest float rather than an
/// incomparable one, a `uuid` compares as its 16 bytes rather than as its
/// hyphenated spelling, and `24:00:00` is a real boundary value.
#[tokio::test]
async fn each_agreeing_type_orders_by_its_own_decoder() {
    assert_eq!(
        kept("public.t_int", "v_bigint", vec![term("v_bigint", PredicateOp::Lt, "0")]).await,
        [Some("-9223372036854775808".to_string())]
    );
    assert_eq!(
        kept("public.t_numeric", "v_typed", vec![term("v_typed", PredicateOp::Lt, "0.0")]).await,
        [Some("-1.5000000000".to_string())]
    );
    assert_eq!(
        kept("public.t_float", "v_double", vec![term("v_double", PredicateOp::Gt, "Infinity")])
            .await,
        [Some("NaN".to_string())],
        "NaN is above every value including infinity, which is PostgreSQL's order, not Rust's"
    );
    assert_eq!(
        kept(
            "public.t_uuid",
            "v_uuid",
            vec![term("v_uuid", PredicateOp::Gt, "00000000-0000-0000-0000-000000000000")]
        )
        .await,
        [Some("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".to_string())]
    );
    assert_eq!(
        kept("public.t_time", "v_time", vec![term("v_time", PredicateOp::Ge, "24:00:00")]).await,
        [Some("24:00:00".to_string())],
        "`24:00:00` is a real boundary value, not an overflow"
    );
}

/// A `numeric(p,s)` literal is taken to the column's own scale before
/// comparing, so `-1.5` and the stored `-1.50` are one value — the property
/// that makes the `Decimal` register row unconditional.
#[tokio::test]
async fn a_decimal_literal_is_decoded_at_the_columns_scale() {
    assert_eq!(
        kept("public.t_numeric", "v_typed", vec![term("v_typed", PredicateOp::Ge, "-1.5")]).await,
        [
            Some("1234567890123456789012345678.1234567890".to_string()),
            Some("0.0000000000".to_string()),
            Some("-1.5000000000".to_string()),
        ]
    );
}

/// A NULL is excluded by every ordering operator, exactly as it is by
/// `Eq`/`Ne` — unknown collapses to false at each term, which is what bounds
/// the conjunction to `AND`.
#[tokio::test]
async fn a_null_survives_no_ordering_operator() {
    for op in [PredicateOp::Lt, PredicateOp::Le, PredicateOp::Gt, PredicateOp::Ge] {
        let rows = kept("public.t_int", "v_bigint", vec![term("v_bigint", op, "0")]).await;
        assert!(!rows.contains(&None), "a NULL survived {}", op.symbol());
    }
}

/// An ordering term composes with the rest of the query: it ANDs with the
/// other terms, and it may name a column the projection does not.
#[tokio::test]
async fn an_ordering_term_is_one_term_of_the_conjunction() {
    let rows = kept(
        "public.t_int",
        "id",
        vec![term("v_smallint", PredicateOp::Ge, "0"), term("v_integer", PredicateOp::Lt, "1")],
    )
    .await;
    assert_eq!(rows, [Some("3".to_string())], "only the all-zero row satisfies both");
}

/// The refusal fires where `UnknownPredicateColumn` fires — when the block's
/// schema resolves — so a nested column is refused before any row flows.
#[tokio::test]
async fn a_nested_column_refuses_an_ordering_operator() {
    for (table, column) in [
        ("public.t_range", "v_range"),
        ("public.t_composite", "v_point"),
        ("public.t_array", "v_with_null"),
        ("public.t_multirange", "v_int4multirange"),
    ] {
        let err = drain(
            table,
            QueryOptions {
                filters: vec![term(column, PredicateOp::Gt, "1")],
                projection: Some(vec!["id".to_string()]),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, Error::UnorderedPredicateColumn { column: c, .. } if c == column),
            "{table}.{column}: {err:?}"
        );
    }
}

/// `SchemaMode::Strings` resolves no column, so every ordering operator is
/// refused there. It falls out of the `Mapped` rule rather than needing a
/// case of its own — which is what this pins.
#[tokio::test]
async fn strings_mode_refuses_every_ordering_operator() {
    let err = drain(
        "public.t_int",
        QueryOptions {
            filters: vec![term("v_integer", PredicateOp::Gt, "0")],
            schema_mode: SchemaMode::Strings,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::UnorderedPredicateColumn { .. }), "{err:?}");
}

/// PostgreSQL's special values are ordered, against the fixture's own
/// `infinity`/`-infinity`/`NaN` — the values `pg_dump` writes from any
/// healthy database. `t_date` holds both infinities and `t_numeric.v_small`
/// (`numeric(10,2)`) holds a `NaN`, which the typmod does not exclude.
#[tokio::test]
async fn the_special_values_are_ordered_not_undecodable() {
    assert_eq!(
        kept("public.t_date", "id", vec![term("v_date", PredicateOp::Gt, "9999-12-31")]).await,
        [Some("1".to_string()), Some("6".to_string())],
        "`infinity` is above the largest finite date, and `10000-01-01` above the literal"
    );
    assert_eq!(
        kept("public.t_date", "id", vec![term("v_date", PredicateOp::Lt, "0001-01-01")]).await,
        [Some("2".to_string()), Some("5".to_string())],
        "`-infinity` is below every finite date, BC ones included"
    );
    assert_eq!(
        kept("public.t_numeric", "id", vec![term("v_small", PredicateOp::Gt, "0.00")]).await,
        [Some("3".to_string())],
        "`NaN` is the only `numeric` value above zero here, and it is above every value"
    );
}

/// **The filter is exact where the batch still cannot hold the value.** The
/// same `date` column that answers `>` above fails to *build*, because
/// `Date32` has no infinity — two paths with different powers, and the
/// asymmetry is deliberate.
#[tokio::test]
async fn a_selected_special_value_still_cannot_be_materialized() {
    let err = drain(
        "public.t_date",
        QueryOptions {
            filters: vec![term("v_date", PredicateOp::Gt, "9999-12-31")],
            projection: Some(vec!["v_date".to_string()]),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, Error::FieldDecode { column, value, .. }
            if column == "v_date" && value == "infinity"),
        "{err:?}"
    );
}

/// A field that is genuinely undecodable for its mapped type is still the
/// fault the typed build path reports, and projecting the column away does
/// **not** escape it: the filter named it. `t_timestamp` holds PostgreSQL's
/// own documented maximum, which overflows `i64` micros counted from the Unix
/// epoch — a representation limit, unlike an infinity, with no order to fall
/// back on.
#[tokio::test]
async fn a_field_that_does_not_decode_is_a_field_decode_error() {
    let err = drain(
        "public.t_timestamp",
        QueryOptions {
            filters: vec![term("v_ts", PredicateOp::Gt, "2000-01-01 00:00:00")],
            projection: Some(vec!["id".to_string()]),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, Error::FieldDecode { column, value, .. }
            if column == "v_ts" && value == "294276-12-31 23:59:59.999999"),
        "{err:?}"
    );
}

/// The literal is decoded once, when the block's schema resolves, so a
/// literal of the wrong type is a fault reported before any row is read
/// rather than a filter that quietly matches nothing.
#[tokio::test]
async fn a_literal_of_the_wrong_type_is_refused_before_any_row() {
    let err = drain(
        "public.t_int",
        QueryOptions {
            filters: vec![term("v_integer", PredicateOp::Gt, "twelve")],
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, Error::PredicateValueDecode { column, value, declared_type, .. }
            if column == "v_integer" && value == "twelve" && declared_type == "integer"),
        "{err:?}"
    );
}

/// The divergent register rows a real dump can show, reported through the
/// stream's own channel — per-column *and* conditional on the predicate,
/// which is why it is neither a `Diagnostic` nor a `ColumnNote`.
///
/// Every text column in the fixture tree carries no `COLLATE` clause, which
/// is why `UnknownCollation` is the text row here and the *agreeing* halves
/// of the collation rule — an explicit `COLLATE "C"`, and a bare `name`
/// column, whose type default is `C` — have unit tests rather than a fixture
/// behind them.
#[tokio::test]
async fn a_divergent_comparison_is_reported_by_the_stream() {
    for (table, column, divergence, marker) in [
        ("public.t_text", "v_text", OrderingDivergence::UnknownCollation, "no COLLATE clause"),
        ("public.t_text", "v_varchar", OrderingDivergence::UnknownCollation, "no COLLATE clause"),
        // `character(n)` diverges for a reason collation cannot fix: the
        // dump writes its values blank-padded and `bpcharcmp` trims (I38).
        ("public.t_text", "v_char", OrderingDivergence::BlankPadded, "blank-padded"),
        ("public.t_numeric", "v_untyped", OrderingDivergence::AsText, "unconstrained"),
        ("public.t_enum_domain", "v_mood", OrderingDivergence::EnumLabels, "declaration order"),
    ] {
        let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
        let mut stream = table_stream(
            &source,
            table,
            ScanOptions::default(),
            QueryOptions {
                filters: vec![term(column, PredicateOp::Ge, "a")],
                projection: Some(vec![column.to_string()]),
                ..Default::default()
            },
            None,
            CacheMode::Disabled,
        );
        while stream.next().await.transpose().unwrap().is_some() {}
        let notes = stream.ordering_notes();
        assert_eq!(notes.len(), 1, "{table}.{column}: {notes:?}");
        assert_eq!(notes[0].column, column);
        assert_eq!(notes[0].divergence, divergence);
        assert!(notes[0].message().contains(marker), "{}", notes[0].message());
    }
}

/// One assertion per column of `public.t_collate`, which is the whole of
/// what a real dump can say about the collation rule.
///
/// Two of the five are *silences*, and they are the half no unit test can
/// stand in for: `pg_dump` writes a `COLLATE` clause only where the column's
/// collation differs from its type's default (I37), so `v_name` and
/// `v_domain_c` carry none — the first because `name`'s type default is `C`,
/// the second because the domain's own `COLLATE "C"` is the default it would
/// have to differ from. A register that read the absence as "unknown" would
/// warn on both.
///
/// `v_text_ucs` is the asymmetry, pinned by a dump rather than by a sentence:
/// `ucs_basic` really is bytewise (`collcollate = C`) and is not named `C`, so
/// the register must still call it divergent. `pg_dump` writes it unquoted —
/// `pg_catalog.ucs_basic` — which is also the shape `collation_is_bytewise`
/// must not fold to `"C"`.
#[tokio::test]
async fn a_collated_column_is_judged_by_its_clause() {
    for (column, divergence) in [
        ("v_text_c", None),
        ("v_name", None),
        ("v_domain_c", None),
        ("v_text_locale", Some(OrderingDivergence::NonBytewiseCollation)),
        ("v_text_ucs", Some(OrderingDivergence::NonBytewiseCollation)),
    ] {
        let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
        let mut stream = table_stream(
            &source,
            "public.t_collate",
            ScanOptions::default(),
            QueryOptions {
                filters: vec![term(column, PredicateOp::Ge, "a")],
                projection: Some(vec![column.to_string()]),
                ..Default::default()
            },
            None,
            CacheMode::Disabled,
        );
        while stream.next().await.transpose().unwrap().is_some() {}
        let notes = stream.ordering_notes();
        match divergence {
            None => assert!(notes.is_empty(), "{column}: {notes:?}"),
            Some(expected) => {
                assert_eq!(notes.len(), 1, "{column}: {notes:?}");
                assert_eq!(notes[0].column, column);
                assert_eq!(notes[0].divergence, expected);
                assert!(
                    notes[0].message().contains("other than C/POSIX"),
                    "{}",
                    notes[0].message()
                );
            }
        }
    }
}

/// The five columns hold one alphabet, so the note is the only thing that
/// separates them: every one answers bytewise, including the two the note
/// says PostgreSQL would order differently. `_x` surviving `> B` is the
/// divergence made concrete — underscore is above `B` in ASCII and is ignored
/// at glibc's primary level, where the server ranks it below.
#[tokio::test]
async fn every_collation_answers_the_same_bytewise_row_set() {
    let expected: Vec<Option<String>> =
        ["a", "é", "f", "_x", "ax"].iter().map(|v| Some(v.to_string())).collect();
    for column in ["v_text_c", "v_text_locale", "v_text_ucs", "v_name", "v_domain_c"] {
        assert_eq!(
            kept("public.t_collate", column, vec![term(column, PredicateOp::Gt, "B")]).await,
            expected,
            "{column}"
        );
    }
}

/// A comparison that agrees with PostgreSQL reports nothing, and neither
/// does a query with no ordering term at all: the channel is a divergence
/// report, not a record of which operators were used.
#[tokio::test]
async fn an_agreeing_comparison_reports_nothing() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        QueryOptions {
            filters: vec![term("v_integer", PredicateOp::Gt, "0")],
            ..Default::default()
        },
        None,
        CacheMode::Disabled,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    assert!(stream.ordering_notes().is_empty());
}

/// The resume fingerprint covers each term's operator, so a token taken from
/// a `>` stream cannot be handed to a `>=` one — the four new operators are
/// hashed, not silently equal to the ones they were added beside.
#[tokio::test]
async fn a_resume_token_does_not_cross_two_ordering_operators() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let options = |op| QueryOptions {
        filters: vec![term("v_integer", op, "0")],
        max_rows: 1,
        ..Default::default()
    };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options(PredicateOp::Gt),
        None,
        CacheMode::Disabled,
    );
    stream.next().await.transpose().unwrap();
    let token = stream.resume_token();

    let mut resumed = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options(PredicateOp::Ge),
        Some(token),
        CacheMode::Disabled,
    );
    let err = resumed.next().await.unwrap().unwrap_err();
    assert!(matches!(err, Error::ResumeQueryMismatch), "{err:?}");
}
