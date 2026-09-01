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
    ComparisonDivergence, Error, LocalFileSource, Predicate, PredicateOp, QueryOptions,
    ScanOptions, SchemaMode, table_stream,
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

/// An `oid` orders over its whole unsigned range, against the fixture's own
/// values: the two above 2^31 are the pair an `Int32` reading turns negative,
/// which would put them *below* zero instead of above it. A signed literal is
/// refused rather than wrapped the way `oidin` wraps it, which is the one
/// place this row is weaker than the server rather than equal to it.
#[tokio::test]
async fn an_oid_orders_unsigned_and_refuses_a_signed_literal() {
    assert_eq!(
        kept("public.t_oid", "v_oid", vec![term("v_oid", PredicateOp::Gt, "2147483647")]).await,
        [Some("2147483648".to_string()), Some("4294967295".to_string())]
    );
    let err = drain(
        "public.t_oid",
        QueryOptions { filters: vec![term("v_oid", PredicateOp::Lt, "-1")], ..Default::default() },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v_oid"),
        "{err:?}"
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

/// The notes a query over `table` raises for `column` under one `>` term —
/// empty for a column whose comparison is PostgreSQL's own.
async fn notes_for(table: &str, column: &str, literal: &str) -> Vec<String> {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let mut stream = table_stream(
        &source,
        table,
        ScanOptions::default(),
        QueryOptions {
            filters: vec![term(column, PredicateOp::Ge, literal)],
            projection: Some(vec![column.to_string()]),
            ..Default::default()
        },
        None,
        CacheMode::Disabled,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    stream.comparison_notes().iter().map(|n| n.message()).collect()
}

/// A bare `numeric` orders by decimal value, against the fixture's own
/// column. `t_numeric.v_untyped` holds `NaN`, `0`, `100.00` and `12345.6789`,
/// and the three assertions below are each a case the bytewise comparison
/// this row used to make would get wrong: `100.00` sorts *below* `9` as text,
/// `100.00` and `100` are one value written two ways, and `NaN` is the
/// largest value rather than a letter.
#[tokio::test]
async fn a_bare_numeric_orders_by_decimal_value() {
    assert_eq!(
        kept("public.t_numeric", "v_untyped", vec![term("v_untyped", PredicateOp::Gt, "9")]).await,
        [Some("NaN".to_string()), Some("100.00".to_string()), Some("12345.6789".to_string()),],
        "as text `100.00` would sort below `9` and be dropped"
    );
    assert_eq!(
        kept("public.t_numeric", "v_untyped", vec![term("v_untyped", PredicateOp::Gt, "100")])
            .await,
        [Some("NaN".to_string()), Some("12345.6789".to_string())],
        "`100.00` equals `100`: the trailing zeros are display scale, not value"
    );
    assert_eq!(
        kept("public.t_numeric", "v_untyped", vec![term("v_untyped", PredicateOp::Lt, "0.5")])
            .await,
        [Some("0".to_string())],
        "`NaN` is above every value, so `< 0.5` keeps only the zero"
    );
    assert!(notes_for("public.t_numeric", "v_untyped", "0").await.is_empty());
}

/// An enum orders by the declaration order the dump carries verbatim, which
/// is the reverse of the label text for these values: `public.mood` declares
/// `sad` first, and every other label in `t_enum_domain` sorts *below* it
/// bytewise.
#[tokio::test]
async fn an_enum_orders_by_declaration_order() {
    assert_eq!(
        kept("public.t_enum_domain", "v_mood", vec![term("v_mood", PredicateOp::Gt, "sad")]).await,
        [
            Some("has space".to_string()),
            Some("has,comma".to_string()),
            Some("has'quote".to_string()),
        ],
        "bytewise every one of these is below `sad`, so a text comparison keeps none"
    );
    assert_eq!(
        kept("public.t_enum_domain", "v_mood", vec![term("v_mood", PredicateOp::Lt, "has,comma")])
            .await,
        [Some("sad".to_string()), Some("has space".to_string())]
    );
    assert!(notes_for("public.t_enum_domain", "v_mood", "sad").await.is_empty());
}

/// An `interval` orders by the span PostgreSQL computes, so `1 mon`,
/// `30 days` and `720:00:00` are one bound written three ways — the property
/// that makes bytewise wrong here in *both* directions, since the three
/// spellings keep three different row sets as text. `t_interval` holds
/// `1 year 2 mons 3 days 04:05:06` (423 days and change), `-1 days`,
/// `00:00:00` and `01:30:00`.
#[tokio::test]
async fn an_interval_orders_by_span_whatever_the_bound_is_spelled() {
    for bound in ["1 mon", "30 days", "720:00:00"] {
        assert_eq!(
            kept(
                "public.t_interval",
                "v_interval",
                vec![term("v_interval", PredicateOp::Ge, bound)]
            )
            .await,
            [Some("1 year 2 mons 3 days 04:05:06".to_string())],
            "bound spelled {bound}"
        );
    }
    // `01:30:00` is the fixture's `1.5 hours` as the dump writes it, and it
    // is below 60 days where its text is above.
    assert_eq!(
        kept(
            "public.t_interval",
            "v_interval",
            vec![term("v_interval", PredicateOp::Lt, "60 days")]
        )
        .await,
        [Some("-1 days".to_string()), Some("00:00:00".to_string()), Some("01:30:00".to_string())],
        "bytewise the 423-day value sorts below `60 days` and would survive"
    );
    assert!(notes_for("public.t_interval", "v_interval", "00:00:00").await.is_empty());
}

/// A `time with time zone` compares as the UTC instant, so
/// `00:00:00.000001-05` is five hours after midnight and survives a bound its
/// text sorts below.
#[tokio::test]
async fn a_timetz_orders_by_its_utc_instant() {
    assert_eq!(
        kept("public.t_time", "v_timetz", vec![term("v_timetz", PredicateOp::Gt, "01:00:00+00")])
            .await,
        [Some("24:00:00+00".to_string()), Some("00:00:00.000001-05".to_string())],
        "bytewise `00:00:00.000001-05` sorts below the bound and would be dropped"
    );
    assert!(notes_for("public.t_time", "v_timetz", "00:00:00+00").await.is_empty());
}

/// The four network types compare by their own orders: an IPv4 address sorts
/// below every IPv6 one whatever the text says, and `192.168.1.1` is above
/// `9.0.0.0` where its first character is below.
#[tokio::test]
async fn the_network_types_order_by_address_not_by_text() {
    assert_eq!(
        kept("public.t_net", "v_inet", vec![term("v_inet", PredicateOp::Ge, "9.0.0.0")]).await,
        [Some("192.168.1.1".to_string()), Some("::1".to_string())],
        "bytewise `192.168.1.1` sorts below `9.0.0.0` and would be dropped"
    );
    assert_eq!(
        kept("public.t_net", "v_cidr", vec![term("v_cidr", PredicateOp::Ge, "9.0.0.0/8")]).await,
        [Some("192.168.1.0/24".to_string()), Some("::/0".to_string())]
    );
    assert_eq!(
        kept(
            "public.t_net",
            "v_macaddr",
            vec![term("v_macaddr", PredicateOp::Gt, "08:00:2b:01:02:02")]
        )
        .await,
        [Some("08:00:2b:01:02:03".to_string())]
    );
    for (column, literal) in [
        ("v_inet", "9.0.0.0"),
        ("v_cidr", "9.0.0.0/8"),
        ("v_macaddr", "08:00:2b:01:02:02"),
        ("v_macaddr8", "08:00:2b:01:02:03:04:04"),
    ] {
        assert!(notes_for("public.t_net", column, literal).await.is_empty(), "{column}");
    }
}

/// A `character(n)` column compares with the blank padding gone from **both**
/// sides, which is `bpcharcmp` calling `bcTruelen` on its two operands before
/// anything else (I38). `t_text.v_char` holds ten blanks and `hi` + eight,
/// and both assertions below are cases the padded comparison gets wrong:
///
/// - `<= hi` keeps `hi` + eight blanks, which padded sorts *above* the
///   unpadded literal and so used to be dropped — the row the server keeps
///   and the direction the divergence used to run in;
/// - a literal carrying padding of its own is the same value, so `>= hi` and
///   `>= hi` + three blanks keep the same rows.
///
/// The blank is `0x20` alone: a tab is a value byte, and it is the byte that
/// separates trim-and-compare from pad-and-compare, which is why the oracle's
/// `character(10)` cases carry one.
#[tokio::test]
async fn a_char_column_compares_with_the_padding_off_both_sides() {
    assert_eq!(
        kept("public.t_text", "v_char", vec![term("v_char", PredicateOp::Le, "hi")]).await,
        [Some("          ".to_string()), Some("hi        ".to_string())]
    );
    for literal in ["hi", "hi   "] {
        assert_eq!(
            kept("public.t_text", "v_char", vec![term("v_char", PredicateOp::Ge, literal)]).await,
            [Some("hi        ".to_string())],
            "{literal:?}"
        );
    }
}

/// `json` is what the text-held row has left, so it is the column that still
/// announces itself that way — the regression guard on the four rows above,
/// since a note that stopped being raised at all would pass every assertion
/// there. Its sentence says which way the difference runs: PostgreSQL has no
/// comparison for `json`, so bytewise is more than the server offers.
#[tokio::test]
async fn json_is_still_announced_as_compared_bytewise() {
    let notes = notes_for("public.t_json", "v_json", "1").await;
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("no comparison"), "{}", notes[0]);
}

/// `jsonb` compares as a container, against the fixture's own two values —
/// an object (`{"a": 1, "b": [1, 2, 3]}`) and the JSON scalar `null`. Both
/// filters below are cases the bytewise comparison this replaced gets wrong,
/// and they are wrong in opposite directions: the first keeps a row that
/// should go, the second drops the only row that should stay.
///
/// A `jsonb` string sorts *below* every number, array and object and above
/// JSON `null`, so `> "zzz"` keeps the object alone — where bytewise the
/// literal starts `0x22` and both rows are above it. And a top-level scalar
/// sorts below a one-element array, so `< [1]` keeps `null` alone — where
/// bytewise `[` is `0x5b` and neither row is below it.
#[tokio::test]
async fn jsonb_compares_as_a_container_not_as_its_text() {
    assert_eq!(
        kept("public.t_json", "v_jsonb", vec![term("v_jsonb", PredicateOp::Gt, "\"zzz\"")]).await,
        [Some(r#"{"a": 1, "b": [1, 2, 3]}"#.to_string())]
    );
    assert_eq!(
        kept("public.t_json", "v_jsonb", vec![term("v_jsonb", PredicateOp::Lt, "[1]")]).await,
        [Some("null".to_string())]
    );
}

/// A `jsonb` column announces the one thing its comparison cannot reach: the
/// string leaves and object keys, which the server orders by the database's
/// collation and a plain dump does not record (I32, I41). The sentence is not
/// the text-held one — the structure *is* compared PostgreSQL's way.
#[tokio::test]
async fn jsonb_announces_its_string_leaves() {
    let notes = notes_for("public.t_json", "v_jsonb", "1").await;
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("structurally"), "{}", notes[0]);
    assert!(notes[0].contains("object key"), "{}", notes[0]);
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
    for (table, column, literal, divergence, marker) in [
        (
            "public.t_text",
            "v_text",
            "a",
            ComparisonDivergence::UnknownCollation,
            "no COLLATE clause",
        ),
        (
            "public.t_text",
            "v_varchar",
            "a",
            ComparisonDivergence::UnknownCollation,
            "no COLLATE clause",
        ),
        // `character(n)` is the same collation row: the dump's blank padding
        // is trimmed off both sides (I38), and what is left is a bare column
        // whose collation the file does not carry.
        (
            "public.t_text",
            "v_char",
            "a",
            ComparisonDivergence::UnknownCollation,
            "no COLLATE clause",
        ),
        // `json` is what `AsText` covers now: the enum, the bare `numeric`,
        // the four types the text-held row lost and `jsonb` all order by
        // their own values.
        ("public.t_json", "v_json", "a", ComparisonDivergence::AsText, "no comparison"),
        // `jsonb` is compared structurally and diverges only at a string
        // leaf, which is a different sentence for a different reason. Its
        // literal has to be a JSON document, which is the same row's other
        // half: `a` is refused where every column above takes it.
        (
            "public.t_json",
            "v_jsonb",
            "1",
            ComparisonDivergence::JsonbStringCollation,
            "structurally",
        ),
    ] {
        let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
        let mut stream = table_stream(
            &source,
            table,
            ScanOptions::default(),
            QueryOptions {
                filters: vec![term(column, PredicateOp::Ge, literal)],
                projection: Some(vec![column.to_string()]),
                ..Default::default()
            },
            None,
            CacheMode::Disabled,
        );
        while stream.next().await.transpose().unwrap().is_some() {}
        let notes = stream.comparison_notes();
        assert_eq!(notes.len(), 1, "{table}.{column}: {notes:?}");
        assert_eq!(notes[0].column, column);
        assert_eq!(notes[0].divergence, divergence);
        assert!(notes[0].message().contains(marker), "{}", notes[0].message());
    }
}

/// One assertion per reachable column of `public.t_collate`, which is the
/// whole of what a real dump can say about the collation rule. `v_gen_nn` is
/// the one column of that table absent from here: it is `STORED` generated,
/// so the dump omits it from `COPY` and no stream can reach it — its
/// assertion is in `tests/preamble.rs`, over `DatabaseMetadata`.
///
/// Two of the seven are *silences*, and they are the half no unit test can
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
///
/// `v_text_def` is the *placement* case: its clause is written after the
/// `DEFAULT` rather than beside the type (I37), so a register that read only
/// the token following the type words would call it unknown. `v_user` is the
/// conservative answer made concrete — the same dump declares
/// `public.c_collation` as `locale = 'C'`, and it is still reported divergent,
/// because the collation is not in `pg_catalog` and nothing stops a user
/// defining a non-bytewise one named `"C"` of their own.
///
/// `v_nd` is the one verdict read off a *statement* rather than off a name:
/// its clause is spelled exactly as `v_user`'s is — schema-qualified,
/// unquoted, outside `pg_catalog` — and the two answer differently only
/// because the same dump declares `public.nd_collation` with
/// `deterministic = false` (I42). It is also the only column here whose
/// divergence reaches `=`; see
/// [`a_collation_note_is_raised_for_ordering_and_not_for_equality`].
#[tokio::test]
async fn a_collated_column_is_judged_by_its_clause() {
    for (column, divergence, marker) in [
        ("v_text_c", None, ""),
        ("v_name", None, ""),
        ("v_domain_c", None, ""),
        ("v_text_def", None, ""),
        ("v_text_locale", Some(ComparisonDivergence::NonBytewiseCollation), "other than C/POSIX"),
        ("v_text_ucs", Some(ComparisonDivergence::NonBytewiseCollation), "other than C/POSIX"),
        ("v_user", Some(ComparisonDivergence::NonBytewiseCollation), "other than C/POSIX"),
        ("v_nd", Some(ComparisonDivergence::NonDeterministicCollation), "non-deterministic"),
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
        let notes = stream.comparison_notes();
        match divergence {
            None => assert!(notes.is_empty(), "{column}: {notes:?}"),
            Some(expected) => {
                assert_eq!(notes.len(), 1, "{column}: {notes:?}");
                assert_eq!(notes[0].column, column);
                assert_eq!(notes[0].divergence, expected);
                assert!(notes[0].message().contains(marker), "{}", notes[0].message());
            }
        }
    }
}

/// The eight reachable columns hold one alphabet, so the note is the only
/// thing that separates them: every one answers bytewise, including the four
/// the note says PostgreSQL would order differently. `_x` surviving `> B` is
/// the divergence made concrete — underscore is above `B` in ASCII and is
/// ignored at glibc's primary level, where the server ranks it below. `v_nd`
/// is bytewise here too, and its ICU collation would order it differently
/// again; the register says so and does not act on it.
#[tokio::test]
async fn every_collation_answers_the_same_bytewise_row_set() {
    let expected: Vec<Option<String>> =
        ["a", "é", "f", "_x", "ax"].iter().map(|v| Some(v.to_string())).collect();
    for column in [
        "v_text_c",
        "v_text_locale",
        "v_text_ucs",
        "v_name",
        "v_domain_c",
        "v_text_def",
        "v_user",
        "v_nd",
    ] {
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
    assert!(stream.comparison_notes().is_empty());
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

/// Typed `=` against the fixture's own values, each a case the untyped
/// comparison answered with an empty result that read like an answer.
///
/// `v_small` is `numeric(10,2)` holding `-1.50`, so the literal `-1.5` a
/// person types is the same value; `v_char` is `char(10)` holding `hi`
/// blank-padded to ten, so `hi` is the same value; `v_untyped` is a bare
/// `numeric` holding `100.00`, so `100` is the same value and no rendering
/// of the literal could have made it a byte comparison.
#[tokio::test]
async fn typed_equality_matches_a_value_written_another_way() {
    assert_eq!(
        kept("public.t_numeric", "v_small", vec![term("v_small", PredicateOp::Eq, "-1.5")]).await,
        [Some("-1.50".to_string())]
    );
    assert_eq!(
        kept("public.t_text", "v_char", vec![term("v_char", PredicateOp::Eq, "hi")]).await,
        [Some("hi        ".to_string())]
    );
    assert_eq!(
        kept("public.t_numeric", "v_untyped", vec![term("v_untyped", PredicateOp::Eq, "100")])
            .await,
        [Some("100.00".to_string())]
    );
    // An `interval` collapses months to 30 days and days to 86400 s, so the
    // fixture's `1 year 2 mons 3 days 04:05:06` is the same value as the 423
    // days it comes to — which no rendering of the literal could have made a
    // byte comparison.
    assert_eq!(
        kept(
            "public.t_interval",
            "v_interval",
            vec![term("v_interval", PredicateOp::Eq, "423 days 04:05:06")]
        )
        .await,
        [Some("1 year 2 mons 3 days 04:05:06".to_string())]
    );
    // `inet_out` drops a full-width netmask, so the literal carrying one is
    // the same address.
    assert_eq!(
        kept("public.t_net", "v_inet", vec![term("v_inet", PredicateOp::Eq, "192.168.1.1/32")])
            .await,
        [Some("192.168.1.1".to_string())]
    );
}

/// A literal that is not a value of the column's type is refused before a row
/// is read, under `=` exactly as under `<` — the answer it replaces is an
/// empty result a user reads as "no such row".
#[tokio::test]
async fn an_equality_literal_of_the_wrong_type_is_refused() {
    let err = drain(
        "public.t_numeric",
        QueryOptions {
            filters: vec![term("v_small", PredicateOp::Eq, "not-a-number")],
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, Error::PredicateValueDecode { value, op, .. }
            if value == "not-a-number" && *op == "="),
        "{err}"
    );
}

/// The collation divergences do not reach `=`: every libc collation is
/// deterministic, so `texteq` is a byte comparison whatever the collation is
/// and the same column that warns under `>=` is silent under `=`.
///
/// **`v_nd` is the exception, and it is why the operator dimension exists at
/// all.** Its clause names a collation the same dump declares
/// `deterministic = false` (I42), which is the one thing a plain dump states
/// about equality, so it warns under both. Every column in the loop above it
/// is `libc`, and the server refuses to make a `libc` collation
/// non-deterministic — so this is not a stronger version of their divergence
/// but a different one, read off a statement rather than off a name.
#[tokio::test]
async fn a_collation_note_is_raised_for_ordering_and_not_for_equality() {
    let equality_notes = |column: &'static str| async move {
        let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
        let mut stream = table_stream(
            &source,
            "public.t_collate",
            ScanOptions::default(),
            QueryOptions {
                filters: vec![term(column, PredicateOp::Eq, "a")],
                projection: Some(vec![column.to_string()]),
                ..Default::default()
            },
            None,
            CacheMode::Disabled,
        );
        while stream.next().await.transpose().unwrap().is_some() {}
        stream.comparison_notes()
    };
    for column in ["v_text_locale", "v_text_ucs", "v_user"] {
        assert_eq!(notes_for("public.t_collate", column, "a").await.len(), 1, "{column} under >=");
        assert!(equality_notes(column).await.is_empty(), "{column} under =");
    }

    assert_eq!(notes_for("public.t_collate", "v_nd", "a").await.len(), 1, "v_nd under >=");
    let nd = equality_notes("v_nd").await;
    assert_eq!(nd.len(), 1, "v_nd under =: {nd:?}");
    assert_eq!(nd[0].divergence, ComparisonDivergence::NonDeterministicCollation);
    assert!(nd[0].message().contains("non-deterministic"), "{}", nd[0].message());
}

/// A `box` column has no comparison in the register: the four ordering
/// operators are refused on it, `=` falls back to text, and the note says so
/// — because `box_eq` compares areas and a byte comparison does not (`KD10`).
#[tokio::test]
async fn a_column_with_no_registered_comparison_announces_its_equality() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let mut stream = table_stream(
        &source,
        "public.t_delimiter",
        ScanOptions::default(),
        QueryOptions {
            filters: vec![term("v_box_domain", PredicateOp::Eq, "(1,1),(0,0)")],
            projection: Some(vec!["v_box_domain".to_string()]),
            ..Default::default()
        },
        None,
        CacheMode::Disabled,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    let notes = stream.comparison_notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].divergence, ComparisonDivergence::UnmodelledType);
    assert!(notes[0].message().contains("models no comparison"), "{}", notes[0].message());
}
