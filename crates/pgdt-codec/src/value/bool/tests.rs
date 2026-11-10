use super::*;

#[test]
fn bool_round_trips() {
    assert_eq!(decode_bool("t"), Some(true));
    assert_eq!(decode_bool("f"), Some(false));
    assert_eq!(decode_bool("x"), None);
    assert_eq!(render_bool(true), "t");
    assert_eq!(render_bool(false), "f");
}

/// **`boolin` is refused where the server refuses it, and what this build
/// reads is what the server reads** (I65): each spelling below was cast
/// on PostgreSQL 16, beside what it answered.
#[test]
fn a_boolean_is_refused_only_where_boolin_refuses_it() {
    let cases: &[(&str, Option<&str>)] = &[
        ("t", Some("true")),
        ("f", Some("false")),
        ("true", Some("true")),
        ("TRUE", Some("true")),
        ("tr", Some("true")),
        ("T", Some("true")),
        ("yes", Some("true")),
        ("y", Some("true")),
        ("YE", Some("true")),
        ("no", Some("false")),
        ("N", Some("false")),
        ("on", Some("true")),
        ("ON", Some("true")),
        ("of", Some("false")),
        ("off", Some("false")),
        ("OFF", Some("false")),
        ("o", None),
        ("O", None),
        ("1", Some("true")),
        ("0", Some("false")),
        ("10", None),
        ("01", None),
        ("", None),
        (" ", None),
        ("  t  ", Some("true")),
        ("\tyes\n", Some("true")),
        ("\u{b}f\u{c}", Some("false")),
        ("truex", None),
        ("tru e", None),
        ("yess", None),
        ("onn", None),
        ("offf", None),
        ("maybe", None),
        ("+1", None),
        ("-0", None),
        (" 1 ", Some("true")),
        ("t\u{a0}", None),
        ("falsE", Some("false")),
        ("fals", Some("false")),
        ("2", None),
    ];
    for (text, server) in cases {
        match decode_bool(text) {
            Some(read) => assert_eq!(Some(read), server.map(|v| v == "true"), "{text:?}"),
            None => {
                let want = if server.is_some() { Unread::Unparsed } else { Unread::Refused };
                assert_eq!(bool_unread(text), want, "{text:?}");
            }
        }
    }
}
