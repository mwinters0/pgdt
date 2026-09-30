//! Typed ordering predicates: `<`, `<=`, `>`, `>=` over the generated
//! `types` fixture (`docs/design/decisions.md`, "Predicates", the
//! ordering register).
//!
//! The library-level tests only — everything here drives `table_stream`
//! directly against real `pg_dump` output, so each column is compared
//! through the decoder its own DDL resolved to. The unit tests in
//! `src/predicate.rs` cover the comparison itself against hand-built
//! schemas; the flags are pinned in `pgdt/tests/query_ordering.rs`.

use futures::StreamExt;
use pgdump_query::Finding;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ComparisonDivergence, ComparisonSemantics, Error, Expr, LocalFileSource, Predicate,
    PredicateOp, QueryOptions, ScanOptions, SchemaMode, UnrepresentableMode, table_stream,
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
        table_stream(&source, table, ScanOptions::default(), options, None, CacheMode::DISABLED);
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await.transpose()? {
        rows.extend(rows_of(&batch));
    }
    Ok(rows)
}

/// The surviving values of the single projected column, in file order.
async fn kept(table: &str, column: &str, filters: Vec<Predicate>) -> Vec<Option<String>> {
    kept_in(UnrepresentableMode::Null, table, column, filters).await
}

/// [`kept`], reading a value its column's type cannot hold as `mode` says:
/// the refuse mode compares it in PostgreSQL's order, where the null mode
/// compares it as NULL (`docs/design/decisions.md`, "D98").
async fn kept_in(
    mode: UnrepresentableMode,
    table: &str,
    column: &str,
    filters: Vec<Predicate>,
) -> Vec<Option<String>> {
    let options = QueryOptions {
        filter: Expr::all(filters),
        projection: Some(vec![column.to_string()]),
        unrepresentable: mode,
        ..Default::default()
    };
    drain(table, options).await.unwrap().into_iter().map(|r| r[0].clone()).collect()
}

/// The surviving rows' `id`s, for a filter over a column `render_field`
/// cannot print at `NestedPlan::Scalar` — every nested one. What the
/// assertion is about is which rows survived, so the projected column is the
/// key rather than the compared value.
async fn kept_ids(table: &str, filters: Vec<Predicate>) -> Vec<Option<String>> {
    kept(table, "id", filters).await
}

/// A spread of the register's agreeing rows, each through its own decoder
/// against the fixture's own values: a negative integer and a negative
/// decimal, `NaN` as PostgreSQL's largest float rather than an incomparable
/// one, a `uuid` as its 16 bytes, and `24:00:00`, a real boundary value
/// ordered where no column materializes it.
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
    // Its row's `id`, `Time64` holding no `24:00:00` to print: the refuse
    // mode's filter orders PostgreSQL's value, which the decoder refuses.
    let ge_end_of_day = || vec![term("v_time", PredicateOp::Ge, "24:00:00")];
    assert_eq!(
        kept_in(UnrepresentableMode::Refuse, "public.t_time", "id", ge_end_of_day()).await,
        [Some("1".to_string())],
        "`24:00:00` is a real boundary value, not an overflow"
    );
    assert_eq!(kept_ids("public.t_time", ge_end_of_day()).await, [], "and NULL to the typed mode");
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
        QueryOptions {
            filter: Expr::all([term("v_oid", PredicateOp::Lt, "-1")]),
            ..Default::default()
        },
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

/// The notes a query over `table` raises for `column` under one `>=` term —
/// empty for a column whose comparison is PostgreSQL's own.
async fn notes_for(table: &str, column: &str, literal: &str) -> Vec<String> {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let mut stream = table_stream(
        &source,
        table,
        ScanOptions::default(),
        QueryOptions {
            filter: Expr::all([term(column, PredicateOp::Ge, literal)]),
            projection: Some(vec![column.to_string()]),
            ..Default::default()
        },
        None,
        CacheMode::DISABLED,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    stream.comparison_notes().iter().map(|n| n.message()).collect()
}

/// A bare `numeric` orders by decimal value, against the fixture's own
/// column. `t_numeric.v_untyped` holds `NaN`, `0`, `100.00` and `12345.6789`,
/// and the first two assertions below are each a case a bytewise comparison
/// of this row would get wrong: `100.00` sorts *below* `9` as text, and
/// `100.00` and `100` are one value written two ways. The third pins `NaN`
/// as the largest value.
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
/// that makes bytewise wrong here, since as text the three spellings do not
/// keep one row set. `t_interval` holds
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
    // `01:30:00` is the fixture's `1.5 hours` as the dump writes it; the
    // value whose text sorts the other way is the 423-day one.
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
///   unpadded literal and so a padded comparison would drop it — the row the
///   server keeps, and the direction a padded comparison would diverge in;
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
/// filters below are cases a bytewise comparison would get wrong, and they
/// are wrong in opposite directions: the first keeps a row that should go,
/// the second drops the only row that should stay.
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
/// `Eq`/`Ne` — unknown collapses to false at the root, which is what bounds
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

/// A range orders `empty` below everything, then by lower bound and then by
/// upper — and a bound settles infinity before value and value before
/// inclusivity (I46).
///
/// Each assertion is a case a bytewise comparison of the `range_out` text
/// gets wrong. `t_range` holds `[1,10)` (id 1), `empty` (id 2) and `(,5)`
/// (id 3): `empty` sorts *below* both where the text `empty` sorts above `[`
/// and `(`, and `(,5)` sorts below `[1,10)` where the text `(` is `0x28` and
/// `[` is `0x5B` — the same answer for the wrong reason, which is why the
/// unbounded row is asked against a bound it must be below rather than only
/// against its neighbour. `t_user_range` is a range over `double precision`
/// with no canonical function, so `[1.5,10.5)` and `[1.5,10.5]` are two
/// values and the exclusive upper is the lower of them.
#[tokio::test]
async fn a_range_orders_bound_wise_with_empty_below_everything() {
    assert_eq!(
        kept_ids("public.t_range", vec![term("v_range", PredicateOp::Lt, "[1,10)")]).await,
        [Some("2".to_string()), Some("3".to_string())],
        "`empty` is below every other range, and an unbounded lower is below a finite one"
    );
    assert_eq!(
        kept_ids("public.t_range", vec![term("v_range", PredicateOp::Gt, "empty")]).await,
        [Some("1".to_string()), Some("3".to_string())],
        "and `empty` is above nothing but itself"
    );
    assert_eq!(
        kept_ids("public.t_user_range", vec![term("v_myrange", PredicateOp::Lt, "[1.5,10.5]")])
            .await,
        [Some("1".to_string()), Some("2".to_string())],
        "an exclusive upper bound is below an inclusive one at the same value"
    );
}

/// A **discrete** range canonicalizes its bounds on the way in, so two
/// spellings of one value are equal and the register has to rewrite the
/// literal before it compares (I46).
///
/// `int4range` has a canonical function and `public.myrange`, over `double
/// precision`, has none — which is what makes this a property of the range
/// type rather than of its subtype. A literal whose bounds are out of order
/// is `22000` on the server, a fault the container grammar cannot see, and it
/// is refused here rather than compared.
#[tokio::test]
async fn a_discrete_range_canonicalizes_its_literal_before_comparing() {
    for literal in ["[1,10)", "[1,9]", "(0,10)", "(0,9]"] {
        assert_eq!(
            kept_ids("public.t_range", vec![term("v_range", PredicateOp::Eq, literal)]).await,
            [Some("1".to_string())],
            "{literal} is `[1,10)` once canonicalized"
        );
    }
    // The collapse to `empty` is part of the same rewrite: `(1,2)` holds no
    // integer, so the server stores it as the empty range.
    assert_eq!(
        kept_ids("public.t_range", vec![term("v_range", PredicateOp::Eq, "(1,2)")]).await,
        [Some("2".to_string())],
    );
    // A continuous range canonicalizes nothing, so the same two spellings are
    // two values there.
    assert_eq!(
        kept_ids("public.t_user_range", vec![term("v_myrange", PredicateOp::Eq, "[1.5,10.5)")])
            .await,
        [Some("1".to_string())],
    );
    assert!(
        kept_ids("public.t_user_range", vec![term("v_myrange", PredicateOp::Eq, "(1.5,10.5]")])
            .await
            .is_empty(),
    );
    let refused = drain(
        "public.t_range",
        QueryOptions {
            filter: Expr::all([term("v_range", PredicateOp::Gt, "[10,1)")]),
            projection: Some(vec!["id".to_string()]),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&refused, Error::PredicateValueDecode { column, .. } if column == "v_range"),
        "{refused:?}"
    );
}

/// A multirange is its members, sorted, coalesced and emptied out before
/// anything is compared — so `{[1,5),[5,10)}` and `{[1,10)}` are one value,
/// and a shorter multirange sorts below a longer one whose members agree
/// (I46).
///
/// `t_multirange` holds `{[1,10)}` (id 1) and `{}` (id 2). The companion
/// multirange of a *user* range is reached through the range's own DDL and
/// nowhere else (I10), which is why it is asked here beside the built-in.
#[tokio::test]
async fn a_multirange_is_compared_after_its_members_are_normalized() {
    for literal in ["{[1,10)}", "{[1,5),[5,10)}", "{[5,10),[1,5)}", "{[1,1),[1,10)}"] {
        assert_eq!(
            kept_ids(
                "public.t_multirange",
                vec![term("v_int4multirange", PredicateOp::Eq, literal)]
            )
            .await,
            [Some("1".to_string())],
            "{literal} normalizes to `{{[1,10)}}`"
        );
    }
    assert_eq!(
        kept_ids(
            "public.t_multirange",
            vec![term("v_int4multirange", PredicateOp::Lt, "{[1,10)}")]
        )
        .await,
        [Some("2".to_string())],
        "the empty multirange is below one with a member"
    );
    assert_eq!(
        kept_ids(
            "public.t_multirange",
            vec![term("v_myrange_multi", PredicateOp::Ge, "{[1.5,10.5)}")]
        )
        .await,
        [Some("1".to_string())],
    );
}

/// An array orders **element-wise first, and by its shape only afterwards**
/// (I45): the elements up to the shorter array's length, then the element
/// count, then the dimension count, then the dimensions and lower bounds.
///
/// The first and third assertions are each a case a bytewise comparison of
/// the `array_out` text gets wrong. `t_array` row 1 holds `{}` and row 2
/// `{1,2,3}`: `{}` sorts below on element count where the *text* `{}` sorts
/// above it (`}` is `0x7D`). An enum element is ordered by its declaration
/// position, so row 1's leading `sad` is below `ok`. The second pins that
/// row 1's `{NULL}` outranks row 2's `{1,NULL,3}`, a NULL element being above
/// every value.
#[tokio::test]
async fn an_array_orders_element_wise_then_by_shape() {
    assert_eq!(
        kept_ids("public.t_array", vec![term("v_empty", PredicateOp::Lt, "{1,2,3}")]).await,
        [Some("1".to_string())],
        "the shorter array is below when the elements it has agree"
    );
    assert_eq!(
        kept_ids("public.t_array", vec![term("v_with_null", PredicateOp::Gt, "{1,NULL,3}")]).await,
        [Some("1".to_string())],
        "a NULL element sorts above every value"
    );
    assert_eq!(
        kept_ids("public.t_array", vec![term("v_enum_array", PredicateOp::Lt, "{ok,ok}")]).await,
        [Some("1".to_string())],
        "an element is ordered by its own type's comparison — the enum's declaration order"
    );
    // A multi-dimensional array is one `Array` node whatever its
    // dimensionality: `array_out` writes the shape into the literal and the
    // elements are flattened row-major.
    assert_eq!(
        kept_ids(
            "public.t_array_shape",
            vec![term("v_multidim", PredicateOp::Lt, "{{5,6},{7,8}}")]
        )
        .await,
        [Some("1".to_string())]
    );
}

/// A composite orders field-wise in declaration order, with the same NULL
/// rule one level down: `record_cmp` calls two NULLs equal and ranks a NULL
/// above every value.
///
/// `t_composite.v_point` is `(x integer, y text)`, and its two non-NULL rows
/// are `(1,"a,b""c")` (id 1) and `(,"")` (id 3). A text comparison puts `(,`
/// *below* `(1,` because `,` is `0x2C`; the server puts it above, because the
/// first field is NULL.
#[tokio::test]
async fn a_composite_orders_field_wise_with_null_above_every_value() {
    assert_eq!(
        kept_ids("public.t_composite", vec![term("v_point", PredicateOp::Gt, "(1,a)")]).await,
        [Some("1".to_string()), Some("3".to_string())],
    );
    // The same walk one level further: an array of composites, where the
    // element literal carries `record_out`'s quoting inside `array_out`'s.
    // Row 1 leads with `(1,…)` and row 3 with a NULL element.
    assert_eq!(
        kept_ids("public.t_composite", vec![term("v_points", PredicateOp::Lt, "{NULL}")]).await,
        [Some("1".to_string())],
        "a leading non-NULL element is below a leading NULL one"
    );
}

/// Equality on a nested column reads the **input** grammar for the literal
/// and the strict output grammar for the field, so a spelling `array_in`
/// accepts and `array_out` never writes still matches the row the server
/// would match — where a byte comparison of this column would not.
#[tokio::test]
async fn a_nested_equality_reads_the_input_grammar() {
    for literal in ["{NULL,ok}", "{ null , ok }", "{NULL,\"ok\"}"] {
        assert_eq!(
            kept_ids("public.t_array", vec![term("v_enum_array", PredicateOp::Eq, literal)]).await,
            [Some("2".to_string())],
            "{literal}"
        );
    }
    // A composite literal keeps every byte its fields were written with,
    // which is `record_in`'s rule and not `array_in`'s — so the whitespace an
    // array drops is part of the field here, and a leaf is read in its own
    // type's output form and no wider.
    let refused = drain(
        "public.t_composite",
        QueryOptions {
            filter: Expr::all([term("v_point", PredicateOp::Eq, "( 1 , a )")]),
            projection: Some(vec!["id".to_string()]),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&refused, Error::PredicateValueDecode { column, .. } if column == "v_point"),
        "{refused:?}"
    );
}

/// A nested column inherits its positions' divergences, and announces **one
/// note per position** — which is what makes the note carry a path at all.
///
/// `public.tagged` is `(label text, tags text[])` and both positions are on
/// the database's own collation, one directly and one through an element; a
/// note naming only the column would have said half of it.
#[tokio::test]
async fn a_nested_column_announces_each_diverging_position() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let mut stream = table_stream(
        &source,
        "public.t_composite",
        ScanOptions::default(),
        QueryOptions {
            filter: Expr::all([term("v_tagged", PredicateOp::Ge, "(a,{})")]),
            projection: Some(vec!["id".to_string()]),
            ..Default::default()
        },
        None,
        CacheMode::DISABLED,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    let notes = stream.comparison_notes();
    assert_eq!(
        notes
            .iter()
            .map(|n| (n.path.clone(), n.declared_type.clone(), n.divergence))
            .collect::<Vec<_>>(),
        [
            (
                Some(".label".to_string()),
                "text".to_string(),
                ComparisonDivergence::UnknownCollation
            ),
            (
                Some(".tags[]".to_string()),
                "text".to_string(),
                ComparisonDivergence::UnknownCollation
            ),
        ],
    );
    assert!(notes[1].message().starts_with("`v_tagged.tags[]` (text)"), "{}", notes[1].message());

    // An `integer[]` column agrees with the server outright: the element has
    // no collation to be unsure about.
    assert!(notes_for("public.t_array", "v_empty", "{}").await.is_empty());
    // A `text[]` one diverges at its element, and the path says so.
    assert_eq!(
        notes_for("public.t_array", "v_text_special", "{}").await.len(),
        1,
        "one position, one note"
    );
}

/// A range type declaring a `canonical` function, end to end from the DDL
/// `pg_dump` writes to the refusal a filter gets — **the one path no fixture
/// can carry**. A canonical function must be declared against the shell type
/// and a SQL function cannot take one (`ERROR: SQL function cannot accept
/// shell type`), so the fixture schema would need a C or internal-language
/// function to hold this case; the dump text is written by hand instead, in
/// the shape `pg_dump` emits it (I10) — the shell declaration first (I11),
/// then the real one with `canonical` after `multirange_type_name`, which is
/// the order `dumpRangeType` appends the parameters in.
///
/// Every operator is refused, `=` and `!=` included: the server rewrites both
/// operands through that function before comparing them, so answering
/// bytewise would be a wrong answer rather than a weaker one.
#[tokio::test]
async fn a_range_declaring_a_canonical_function_refuses_every_operator() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("canonical-range.sql");
    std::fs::write(
        &path,
        [
            "SET client_encoding = 'UTF8';",
            "",
            "CREATE TYPE public.canonrange;",
            "",
            "CREATE TYPE public.canonrange AS RANGE (",
            "    subtype = integer,",
            "    multirange_type_name = public.canonrange_multi,",
            "    canonical = public.canonrange_canonical",
            ");",
            "",
            "CREATE TABLE public.t_canon (",
            "    id integer,",
            "    v_span public.canonrange",
            ");",
            "",
            "COPY public.t_canon (id, v_span) FROM stdin;",
            "1\t[1,11)",
            "\\.",
            "",
        ]
        .join("\n"),
    )
    .unwrap();
    let source = LocalFileSource::open(&path).unwrap();
    let refusal = |op, value: &str| {
        let options =
            QueryOptions { filter: Expr::all([term("v_span", op, value)]), ..Default::default() };
        table_stream(
            &source,
            "public.t_canon",
            ScanOptions::default(),
            options,
            None,
            CacheMode::DISABLED,
        )
    };
    for op in [PredicateOp::Lt, PredicateOp::Ge, PredicateOp::Eq, PredicateOp::Ne] {
        let err = refusal(op, "[1,10]").next().await.unwrap().unwrap_err();
        let Error::UncomparablePredicateColumn { reason, .. } = &err else {
            panic!("{op:?}: {err:?}")
        };
        assert!(reason.contains("public.canonrange_canonical"), "{op:?}: {reason}");
        // The whole message, word for word — `docs/manual/type-handling.md`
        // prints it for a user to recognise, so the wording is a contract
        // and not an implementation detail. Only the offset differs there,
        // which is illustration rather than a captured run.
        assert_eq!(
            err.to_string(),
            format!(
                "`{}` on column `v_span` in the COPY block at offset 306: the range type \
                 `public.canonrange` declares a canonical function \
                 (`public.canonrange_canonical`), which PostgreSQL applies to every value \
                 of it before storing or comparing one — arbitrary server-side code this \
                 build cannot run, so two spellings the server calls one value would be two \
                 values here; no operator can be answered for this column, `=` and `!=` \
                 included",
                op.symbol()
            )
        );
    }
    // `IS NOT NULL` reads no value and consults no plan, so the row still
    // comes back — the refusal is of comparisons, not of the column.
    let options = QueryOptions {
        filter: Expr::all([Predicate {
            column: "v_span".into(),
            op: PredicateOp::IsNotNull,
            value: None,
        }]),
        projection: Some(vec!["id".to_string()]),
        ..Default::default()
    };
    let mut stream = table_stream(
        &source,
        "public.t_canon",
        ScanOptions::default(),
        options,
        None,
        CacheMode::DISABLED,
    );
    let batch = stream.next().await.unwrap().unwrap();
    assert_eq!(rows_of(&batch), [[Some("1".to_string())]]);
}

/// `SchemaMode::Strings` resolves no column, so every ordering operator is
/// refused there. It falls out of the `Mapped` rule rather than needing a
/// case of its own — which is what this pins.
#[tokio::test]
async fn strings_mode_refuses_every_ordering_operator() {
    let err = drain(
        "public.t_int",
        QueryOptions {
            filter: Expr::all([term("v_integer", PredicateOp::Gt, "0")]),
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
/// healthy database — by the refuse mode's filter, which compares a value its
/// column cannot hold in PostgreSQL's order. `t_date` holds both infinities
/// and `t_numeric.v_small` (`numeric(10,2)`) holds a `NaN`, which the typmod
/// does not exclude.
#[tokio::test]
async fn the_special_values_are_ordered_not_undecodable() {
    let refuse = UnrepresentableMode::Refuse;
    assert_eq!(
        kept_in(refuse, "public.t_date", "id", vec![term("v_date", PredicateOp::Gt, "9999-12-31")])
            .await,
        [Some("1".to_string()), Some("6".to_string())],
        "`infinity` is above the largest finite date, and `10000-01-01` above the literal"
    );
    assert_eq!(
        kept_in(refuse, "public.t_date", "id", vec![term("v_date", PredicateOp::Lt, "0001-01-01")])
            .await,
        [Some("2".to_string()), Some("5".to_string())],
        "`-infinity` is below every finite date, BC ones included"
    );
    assert_eq!(
        kept_in(refuse, "public.t_numeric", "id", vec![term("v_small", PredicateOp::Gt, "0.00")])
            .await,
        [Some("3".to_string())],
        "`NaN` is the only `numeric` value above zero here, and it is above every value"
    );
}

/// **The typed mode's filter compares such a value as NULL**, as its batch
/// holds it: no ordering term keeps it, and `IS NULL` does.
#[tokio::test]
async fn the_typed_mode_compares_a_special_value_as_null() {
    assert_eq!(
        kept_ids("public.t_date", vec![term("v_date", PredicateOp::Gt, "9999-12-31")]).await,
        [Some("6".to_string())]
    );
    assert_eq!(
        kept_ids("public.t_date", vec![term("v_date", PredicateOp::Lt, "0001-01-01")]).await,
        [Some("5".to_string())]
    );
    assert_eq!(
        kept_ids("public.t_numeric", vec![term("v_small", PredicateOp::Gt, "0.00")]).await,
        []
    );
    let is_null = Predicate { column: "v_date".into(), op: PredicateOp::IsNull, value: None };
    assert_eq!(
        kept_ids("public.t_date", vec![is_null]).await,
        [Some("1".to_string()), Some("2".to_string()), Some("7".to_string())]
    );
}

/// **The untyped mode's filter compares a column it reads as text in its
/// semantics' order**: in PostgreSQL's, the declared type's, special values
/// ranked, where the batch holds each value's text; in DataFusion's, that
/// text bytewise, as DataFusion compares a `Utf8View`
/// (`docs/design/decisions.md`, "D56"; `ColumnResolution::UnrepresentableValues`).
#[tokio::test]
async fn the_untyped_mode_compares_its_text_column_in_each_semantics_order() {
    let kept = |semantics, op, literal: &str| QueryOptions {
        filter: Expr::all([term("v_date", op, literal)]),
        projection: Some(vec!["v_date".to_string()]),
        unrepresentable: UnrepresentableMode::Text,
        semantics,
        ..Default::default()
    };
    let values = |rows: Vec<Vec<Option<String>>>| -> Vec<String> {
        rows.into_iter().map(|row| row[0].clone().unwrap()).collect()
    };
    let postgres = ComparisonSemantics::Postgres;
    let after = kept(postgres, PredicateOp::Gt, "9999-12-31");
    assert_eq!(values(drain("public.t_date", after).await.unwrap()), ["infinity", "10000-01-01"]);
    let before = kept(postgres, PredicateOp::Lt, "0001-01-01");
    assert_eq!(
        values(drain("public.t_date", before).await.unwrap()),
        ["-infinity", "0044-01-01 BC"]
    );
    let bytewise = kept(ComparisonSemantics::DataFusion, PredicateOp::Gt, "9999-12-31");
    assert_eq!(values(drain("public.t_date", bytewise).await.unwrap()), ["infinity"]);
}

/// **The refuse mode's filter is exact where the batch still cannot hold the
/// value.** The same `date` column that answers `>` above refuses to be
/// *materialized*, because `Date32` has no infinity — refused at planning,
/// from the map's count of both, whatever the filter keeps: a filter keeping
/// no row holding one, or none at all, refuses as surely
/// (`docs/design/decisions.md`, "D99").
#[tokio::test]
async fn a_selected_special_value_still_cannot_be_materialized() {
    for keeps in [
        term("v_date", PredicateOp::Gt, "9999-12-31"),
        term("v_date", PredicateOp::Eq, "0001-01-01"),
        term("id", PredicateOp::Gt, "100"),
    ] {
        let err = drain(
            "public.t_date",
            QueryOptions {
                filter: Expr::all([keeps.clone()]),
                projection: Some(vec!["v_date".to_string()]),
                unrepresentable: UnrepresentableMode::Refuse,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, Error::Unrepresentable { column, values: 2, .. } if column == "v_date"),
            "{keeps:?}: {err:?}"
        );
    }
}

/// **A timestamp past what `i64` counts from 1970 is keyed**, from
/// PostgreSQL's epoch as the server stores it (I49), so the refuse mode's
/// filter orders `t_timestamp`'s greatest — PostgreSQL's own documented
/// maximum — above every finite value and below `infinity`, as the server
/// does, where the column is not materialized. The typed mode reads it as
/// NULL, which no ordering term keeps.
#[tokio::test]
async fn a_timestamp_past_i64_is_ordered_by_the_refuse_mode_s_filter() {
    let options = |mode, op, literal| QueryOptions {
        filter: Expr::all([term("v_ts", op, literal)]),
        projection: Some(vec!["id".to_string()]),
        unrepresentable: mode,
        ..Default::default()
    };
    let ids = |rows: Vec<Vec<Option<String>>>| -> Vec<String> {
        rows.into_iter().map(|row| row[0].clone().unwrap()).collect()
    };
    let refuse = UnrepresentableMode::Refuse;
    let after = options(refuse, PredicateOp::Gt, "2000-01-01 00:00:00");
    assert_eq!(ids(drain("public.t_timestamp", after).await.unwrap()), ["1", "3", "4", "7"]);
    let last = options(refuse, PredicateOp::Ge, "294276-12-31 23:59:59.999999");
    assert_eq!(ids(drain("public.t_timestamp", last).await.unwrap()), ["1", "7"]);
    let equal = options(refuse, PredicateOp::Eq, "294276-12-31 23:59:59.999999");
    assert_eq!(ids(drain("public.t_timestamp", equal).await.unwrap()), ["7"]);
    let null = options(UnrepresentableMode::Null, PredicateOp::Gt, "2000-01-01 00:00:00");
    assert_eq!(ids(drain("public.t_timestamp", null).await.unwrap()), ["3", "4"]);
}

/// The literal is decoded once, when the block's schema resolves, so a
/// literal of the wrong type is a fault reported before any row is read
/// rather than a filter that quietly matches nothing.
#[tokio::test]
async fn a_literal_of_the_wrong_type_is_refused_before_any_row() {
    let err = drain(
        "public.t_int",
        QueryOptions {
            filter: Expr::all([term("v_integer", PredicateOp::Gt, "twelve")]),
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
        // `json` is what `AsText` covers: the enum, the bare `numeric`, and
        // the other types with their own decoder all order by their own
        // values instead, leaving `json` as the text-held case.
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
                filter: Expr::all([term(column, PredicateOp::Ge, literal)]),
                projection: Some(vec![column.to_string()]),
                ..Default::default()
            },
            None,
            CacheMode::DISABLED,
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
/// Two of the four *silences* are the half no unit test can
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
                filter: Expr::all([term(column, PredicateOp::Ge, "a")]),
                projection: Some(vec![column.to_string()]),
                ..Default::default()
            },
            None,
            CacheMode::DISABLED,
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
/// the note says PostgreSQL would order differently. `a` and `ax` surviving
/// `> B` are the divergence made concrete — lower case is above `B` in ASCII,
/// where glibc's primary level ranks `a` below it; `_x` survives under both,
/// its underscore ignored there. `v_nd`
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
            filter: Expr::all([term("v_integer", PredicateOp::Gt, "0")]),
            ..Default::default()
        },
        None,
        CacheMode::DISABLED,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    assert!(stream.comparison_notes().is_empty());
}

/// The resume fingerprint covers each term's operator, so a token taken from
/// a `>` stream cannot be handed to a `>=` one — the ordering operators are
/// hashed like any other, not silently equal to each other or to `=`/`!=`.
#[tokio::test]
async fn a_resume_token_does_not_cross_two_ordering_operators() {
    let source = LocalFileSource::open(types_fixture(16, "default")).unwrap();
    let options = |op| QueryOptions {
        filter: Expr::all([term("v_integer", op, "0")]),
        max_rows: 1,
        ..Default::default()
    };
    let mut stream = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options(PredicateOp::Gt),
        None,
        CacheMode::DISABLED,
    );
    stream.next().await.transpose().unwrap();
    let token = stream.resume_token();

    let mut resumed = table_stream(
        &source,
        "public.t_int",
        ScanOptions::default(),
        options(PredicateOp::Ge),
        Some(token),
        CacheMode::DISABLED,
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
            filter: Expr::all([term("v_small", PredicateOp::Eq, "not-a-number")]),
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
                filter: Expr::all([term(column, PredicateOp::Eq, "a")]),
                projection: Some(vec![column.to_string()]),
                ..Default::default()
            },
            None,
            CacheMode::DISABLED,
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
            filter: Expr::all([term("v_box_domain", PredicateOp::Eq, "(1,1),(0,0)")]),
            projection: Some(vec!["v_box_domain".to_string()]),
            ..Default::default()
        },
        None,
        CacheMode::DISABLED,
    );
    while stream.next().await.transpose().unwrap().is_some() {}
    let notes = stream.comparison_notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].divergence, ComparisonDivergence::UnmodelledType);
    assert!(notes[0].message().contains("models no comparison"), "{}", notes[0].message());
}
