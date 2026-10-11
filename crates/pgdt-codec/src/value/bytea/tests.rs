use super::*;

#[test]
fn bytea_round_trips_including_empty() {
    for text in ["\\x", "\\xdeadbeef00ff", "\\x5c6261636b736c617368"] {
        let bytes = decode_bytea(text).expect(text);
        assert_eq!(render_bytea(&bytes), text);
    }
    assert_eq!(decode_bytea("\\xnope"), None);
}

/// `byteaout`'s `escape` form, every byte through it and back, beside
/// what the server writes for a few (`select '\x005c7f41'::bytea` under
/// `bytea_output = escape` is `\000\\\177A`).
#[test]
fn bytea_escape_form_round_trips_every_byte() {
    let every_byte: Vec<u8> = (0..=u8::MAX).collect();
    let text = render_bytea_escape(&every_byte);
    assert_eq!(decode_bytea(&text), Some(every_byte.clone()));
    assert_eq!(decode_bytea_escape(&text, 3), Some(every_byte[..3].to_vec()));
    for b in 0..=u8::MAX {
        assert_eq!(decode_bytea(&render_bytea_escape(&[b])), Some(vec![b]), "{b}");
    }
    assert_eq!(render_bytea_escape(&[0x00, 0x5c, 0x7f, 0x41]), "\\000\\\\\\177A");
    assert_eq!(decode_bytea("\\000\\\\\\177A"), Some(vec![0x00, 0x5c, 0x7f, 0x41]));
    // The empty value is the empty text, and no escape spelling opens
    // `\x` (I56).
    assert_eq!(decode_bytea(""), Some(Vec::new()));
    assert!(!text.starts_with("\\x"));
}

/// What `byteaout` never writes is not read, a shortfall where `byteain`
/// reads it (`docs/design/decisions.md`, "D55"): a printable byte in octal,
/// an octal escape past a byte, one cut short, a lone backslash, a byte
/// outside space to `~` written bare.
#[test]
fn bytea_escape_form_refuses_what_byteaout_never_writes() {
    for text in ["\\101", "\\134", "\\400", "\\08", "\\00", "a\\", "\\y", "tab\there", "é"] {
        assert_eq!(decode_bytea(text), None, "{text:?}");
    }
}

/// **`byteain` is refused where the server refuses it, and what this
/// build reads is what the server reads** (I69): each spelling below was
/// cast on PostgreSQL 16, beside the value it answered.
#[test]
fn a_bytea_is_refused_only_where_byteain_refuses_it() {
    let cases: &[(&str, Option<&str>)] = &[
        ("\\x", Some("\\x")),
        ("\\x00", Some("\\x00")),
        ("\\xAB", Some("\\xab")),
        ("\\x a b", None),
        ("\\x0", None),
        ("\\xab c", None),
        ("\\x ab \n\tcd\r", Some("\\xabcd")),
        ("\\xgg", None),
        ("\\x\u{e9}", None),
        ("\\X41", None),
        ("abc", Some("\\x616263")),
        ("a\\\\b", Some("\\x615c62")),
        ("\\000", Some("\\x00")),
        ("\\377", Some("\\xff")),
        ("\\400", None),
        ("\\08", None),
        ("\\1", None),
        ("\\x", Some("\\x")),
        ("\\", None),
        ("a\\", None),
        ("\\101", Some("\\x41")),
        ("\u{e9}", Some("\\xc3a9")),
        ("tab\there", Some("\\x7461620968657265")),
        ("\\\\x41", Some("\\x5c783431")),
        ("\\xab\u{b}", None),
        ("\\x ab\u{c}", None),
    ];
    for (text, server) in cases {
        match server {
            None => {
                assert_eq!(decode_bytea(text), None, "{text:?}");
                assert_eq!(bytea_unread(text), Unread::Refused, "{text:?}");
            }
            Some(value) => {
                let want = decode_bytea(value).unwrap();
                match decode_bytea(text) {
                    Some(read) => assert_eq!(read, want, "{text:?}"),
                    None => assert_eq!(bytea_unread(text), Unread::Unparsed, "{text:?}"),
                }
            }
        }
    }
}
