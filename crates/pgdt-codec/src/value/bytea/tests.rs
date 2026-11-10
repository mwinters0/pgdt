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
                assert_eq!(bytea_unread(text.bytes()), Unread::Refused, "{text:?}");
            }
            Some(value) => {
                let want = decode_bytea(value).unwrap();
                match decode_bytea(text) {
                    Some(read) => assert_eq!(read, want, "{text:?}"),
                    None => assert_eq!(bytea_unread(text.bytes()), Unread::Unparsed, "{text:?}"),
                }
            }
        }
    }
}

/// **A field read still escaped reads what its unescaped text reads**
/// (`decode_bytea_into`, `ByteaFold`), whichever way `COPY` spells it —
/// as `pg_dump` writes it, or each byte as an octal or hex escape, so the
/// hex form's slice path and the byte-at-a-time path both answer — and
/// appends after what `out` held, leaving it as it was where it reads none.
#[test]
fn a_field_read_escaped_reads_what_its_text_reads() {
    use pgdt_core::escape_field;
    use pgdt_core::internal::unescaped;

    let mut texts = vec![
        "\\x".to_string(),
        "\\xdeadbeef00ff".into(),
        "\\xDeAd".into(),
        "\\xabc".into(),
        "\\xgg".into(),
        "\\x ab".into(),
        "\\X41".into(),
        "abc".into(),
        "\\000\\\\\\177A".into(),
        "\\101".into(),
        "a\\".into(),
        "\\".into(),
        "".into(),
        "tab\there".into(),
    ];
    texts.push(render_bytea_escape(&(0..=u8::MAX).collect::<Vec<u8>>()));
    texts.push(render_bytea(&(0..=u8::MAX).collect::<Vec<u8>>()));
    for text in &texts {
        let want = decode_bytea(text);
        let pg_dump = escape_field(Some(text));
        let octal: Vec<u8> = text.bytes().flat_map(|b| format!("\\{b:03o}").into_bytes()).collect();
        // The hex escape takes up to two digits, so a byte spelled `\xNN`
        // stands alone whatever follows it.
        let hex: Vec<u8> = text.bytes().flat_map(|b| format!("\\x{b:02x}").into_bytes()).collect();
        for field in [pg_dump, octal, hex] {
            let shown = String::from_utf8_lossy(&field).into_owned();
            let mut out = b"kept".to_vec();
            let read = decode_bytea_into(unescaped(&field).unwrap(), &mut out);
            let mut fold = ByteaFold::default();
            for byte in unescaped(&field).unwrap() {
                fold.byte(byte, &mut |_| {});
            }
            let folded = fold.end();
            assert_eq!(folded.is_ok(), read, "{shown}");
            assert!(read || folded == Err(bytea_unread(text.bytes())), "{shown}");
            match &want {
                Some(want) => {
                    assert!(read, "{shown}");
                    assert_eq!(&out[..4], b"kept", "{shown}");
                    assert_eq!(&out[4..], &want[..], "{shown}");
                }
                None => {
                    assert!(!read, "{shown}");
                    assert_eq!(out, b"kept", "{shown}");
                }
            }
        }
    }
}

/// **A text read a run at a time answers what it answers read whole**
/// (`ByteaRuns`): its `escape` form's head and whether `byteain` refuses
/// it, fed whole, in runs of one to three bytes and cut at every byte.
#[test]
fn a_bytea_read_in_runs_answers_as_read_whole() {
    let mut texts: Vec<String> = [
        "\\x",
        "\\xdeadbeef",
        "\\x0",
        "\\xgg",
        "abc",
        "\\000\\\\\\177A",
        "\\101",
        "\\400",
        "a\\",
        "\\",
        "",
        "tab\there",
        "\\08",
        "\\1",
    ]
    .map(str::to_string)
    .into();
    texts.push(render_bytea_escape(&(0..=u8::MAX).collect::<Vec<u8>>()));
    for text in &texts {
        for limit in [0, 3, usize::MAX] {
            let want = (
                bytea_escape_head(text.bytes(), limit),
                bytea_unread(text.bytes()) == Unread::Refused,
            );
            let fed = |runs: &mut dyn Iterator<Item = &[u8]>| {
                let mut read = ByteaRuns::new(limit);
                runs.for_each(|run| read.push(run));
                read.finish()
            };
            let bytes = text.as_bytes();
            assert_eq!(fed(&mut std::iter::once(bytes)), want, "{text:?}");
            for width in 1..=3 {
                assert_eq!(fed(&mut bytes.chunks(width)), want, "{text:?} in runs of {width}");
            }
            for cut in 0..=bytes.len() {
                let (head, tail) = bytes.split_at(cut);
                assert_eq!(fed(&mut [head, tail].into_iter()), want, "{text:?} cut at {cut}");
            }
        }
    }
}
