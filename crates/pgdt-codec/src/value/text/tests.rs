use super::*;

/// **A `character varying(n)` or `character(n)` field is refused exactly
/// where `varchar_input` or `bpchar_input` refuses it** (I75): past `n`
/// characters, counted in UTF-8, with anything but `0x20` past them —
/// every case put to `pg_input_is_valid` under `character varying(3)` and
/// `character(3)`, which agree on each, on a PostgreSQL 16 server.
#[test]
fn a_character_field_is_refused_only_past_its_length_but_for_blanks() {
    // `(text, the server reads it)`, at a length of 3.
    let cases: &[(&str, bool)] = &[
        ("abc", true),
        ("a", true),
        ("", true),
        ("abcd", false),
        ("abc   ", true),
        ("ab    ", true),
        ("abc\t", false),
        ("abc\n", false),
        (" abc", false),
        ("\u{e9}\u{e9}\u{e9}", true),
        ("\u{e9}\u{e9}\u{e9}  ", true),
        ("\u{e9}\u{e9}\u{e9}\u{e9}", false),
        ("\u{e9}\u{e9}\u{e9}x", false),
        ("\u{e9}\u{e9}\u{e9}\u{e9} ", false),
    ];
    for &(text, server) in cases {
        assert_eq!(!char_typmod_refuses(text, 3), server, "{text:?}");
    }
    assert!(char_typmod_refuses("ab", 1) && !char_typmod_refuses("a ", 1));
}
