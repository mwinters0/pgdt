use super::*;

/// **`json_in` is read as the server reads it** (I72): each text below
/// was put to `pg_input_is_valid(…, 'json')` on PostgreSQL 16 and its
/// answer is beside it. The first five `jsonb_in` refuses.
#[test]
fn a_json_is_refused_only_where_json_in_refuses_it() {
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let cases: &[(&str, bool)] = &[
        (r#""\u0000""#, true),
        (r#""\ud800""#, true),
        (r#""\udc00""#, true),
        (r#""\ud800A""#, true),
        ("1e100000000", true),
        (&deep, true),
        ("-0", true),
        (r#"{"a":1,"a":2}"#, true),
        ("\"a\u{7f}b\"", true),
        ("[1] ", true),
        (" \t\n\r{}\r\n", true),
        ("\"\u{e9}\"", true),
        (r#""é""#, true),
        ("null", true),
        ("[true,false,null]", true),
        ("1E+2", true),
        ("-1.5e-3", true),
        (r#""\/""#, true),
        (r#"{"a": [1, {"b": {}}], "c": []}"#, true),
        ("[1,]", false),
        ("01", false),
        ("-01", false),
        ("", false),
        ("  ", false),
        (r#""\x41""#, false),
        ("\u{b}1", false),
        ("truex", false),
        ("tru", false),
        ("1x", false),
        ("1\u{e9}", false),
        ("\u{e9}", false),
        ("-", false),
        ("- 1", false),
        ("\"abc", false),
        ("1.", false),
        (".5", false),
        ("+1", false),
        ("NaN", false),
        ("1e", false),
        ("1e+", false),
        ("1_0", false),
        ("\"a\tb\"", false),
        ("1 2", false),
        ("{a:1}", false),
        (r#""\U0041""#, false),
        (r#""\u004""#, false),
        (r#"{"a" 1}"#, false),
        ("{,}", false),
        ("[,1]", false),
        (r#"{"a":1,}"#, false),
        ("[1]]", false),
        (r#"{"a":[}"#, false),
        (r#"{"a":1]"#, false),
        ("[1}", false),
    ];
    for (text, server) in cases {
        assert_eq!(json_in(text), *server, "{text:?}");
    }
}

/// **A `jsonb` walked off its bytes is read as its document is parsed**
/// (`JsonWalk`, `parse_jsonb`): the same refusal, or the same shortfall
/// past `JSONB_MAX_DEPTH`, over documents built from the grammar's tokens
/// and the refusals `jsonb_in` adds to `json_in`'s, and over nestings either
/// side of the depth with a refusal before, at and past it.
#[test]
fn a_jsonb_walked_is_read_as_its_document_is_parsed() {
    const TOKENS: &[&str] = &[
        "{",
        "}",
        "[",
        "]",
        ",",
        ":",
        "\"",
        "\"a\"",
        "\"\\u0000\"",
        "\"\\ud800\"",
        "\"\\udc00\"",
        "\"\\ud800\\udc00\"",
        "\"\\ud800\\n\"",
        "\"\\ud800x\"",
        "\"\\u0041\"",
        "\"\\x\"",
        "\"é\"",
        "\"\t\"",
        "\\",
        "1",
        "0",
        "-1",
        "01",
        "1.5",
        "1.",
        "1e5",
        "1e1073741824",
        "1e1073741823",
        "0e1073741823",
        "1e-16384",
        "1e99999999999999999999",
        "true",
        "fals",
        "null",
        " ",
        "\n",
        "x",
    ];
    let mut rng = 0x5eed_0003u64;
    let mut below = |n: usize| {
        rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((rng >> 11) % n as u64) as usize
    };
    let walked = |text: &str| walked(text, true);
    let mut read = 0;
    for _ in 0..60_000 {
        let n = below(12) + 1;
        let text: String = (0..n).map(|_| TOKENS[below(TOKENS.len())]).collect();
        let parsed = parse_jsonb(&text).map(drop);
        assert_eq!(walked(&text), parsed, "{text:?}");
        read += usize::from(parsed.is_ok());
    }
    assert!(read > 1_000, "{read} documents read");
    for depth in [998, 999, 1000, 1001, 2000] {
        for inner in ["", "1", "\"\\u0000\"", "01", "{\"\\u0000\":1}", "{\"a\":\"\\ud800\"}"] {
            let text = format!("{}{inner}{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(walked(&text), parse_jsonb(&text).map(drop), "{depth} {inner}");
        }
    }
    // Either side of the depth, a refusal is a refusal only where the
    // cursor reaches it.
    let nested =
        |depth: usize, inner: &str| format!("{}{inner}{}", "[".repeat(depth), "]".repeat(depth));
    assert_eq!(walked(&nested(999, "\"\\u0000\"")), Err(Unread::Refused));
    assert_eq!(walked(&nested(1000, "\"\\u0000\"")), Err(Unread::Unparsed));
    assert_eq!(walked(&nested(999, "{\"\\u0000\":1}")), Err(Unread::Refused));
    assert_eq!(walked(&nested(1000, "01")), Err(Unread::Refused));
}
