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
