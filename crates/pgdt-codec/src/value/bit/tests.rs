use super::*;

/// **A bit string is refused exactly where `bit_in` or `varbit_in`
/// refuses it** (I73): every case below was put to `pg_input_is_valid`
/// under the type beside it on a PostgreSQL 16 server, `bit` being
/// `bit(1)` and `"bit"` the type with no length.
#[test]
fn a_bit_string_is_refused_only_where_bit_in_refuses_it() {
    // `(text, varying, length, the server reads it)`.
    let cases: &[(&str, bool, Option<u32>, bool)] = &[
        ("101", false, Some(3), true),
        ("10", false, Some(3), false),
        ("1010", false, Some(3), false),
        ("B101", false, Some(3), true),
        ("b101", false, Some(3), true),
        ("x5", false, Some(3), false),
        ("x5", false, Some(4), true),
        ("XA", false, Some(4), true),
        ("xg", false, Some(4), false),
        ("102", false, Some(3), false),
        (" 101", false, Some(3), false),
        ("101 ", false, Some(3), false),
        ("", false, Some(3), false),
        ("1", false, Some(1), true),
        ("11", false, Some(1), false),
        ("11", false, None, true),
        ("", false, None, true),
        ("b", false, None, true),
        ("B", false, None, true),
        ("x", false, None, true),
        ("xFfA0", false, None, true),
        ("xFfA0g", false, None, false),
        ("2", false, None, false),
        ("bb1", false, None, false),
        ("bx1", false, None, false),
        ("xx1", false, None, false),
        ("\u{e9}", false, None, false),
        ("101", true, None, true),
        ("", true, None, true),
        ("x", true, None, true),
        ("xab", true, None, true),
        ("1_0", true, None, false),
        ("+1", true, None, false),
        ("0x1", true, None, false),
        ("b 1", true, None, false),
        ("101", true, Some(3), true),
        ("10", true, Some(3), true),
        ("", true, Some(3), true),
        ("1010", true, Some(3), false),
        ("xA", true, Some(4), true),
        ("xAB", true, Some(4), false),
        ("xA", true, Some(3), false),
        ("b11", true, Some(2), true),
        ("b111", true, Some(2), false),
    ];
    for &(text, varying, length, server) in cases {
        assert_eq!(bit_in(text, varying, length), server, "{text:?} {varying} {length:?}");
    }
}
