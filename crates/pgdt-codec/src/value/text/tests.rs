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

/// **A text folded a run at a time reads as it does whole**
/// (`docs/design/decisions.md`, "D142"): its length, where its trailing
/// blanks begin and its characters before them, whichever bytes the runs
/// end between — a character's included — and a text that is not UTF-8
/// found as one across any cut.
#[test]
fn a_text_folded_in_runs_reads_as_it_does_whole() {
    let folded = |runs: &[&[u8]]| {
        let mut fold = TextFold::default();
        for run in runs {
            fold.push(run);
        }
        fold.finish()
    };
    for text in ["", "abc", "abc   ", "\u{e9}\u{e9} \u{e9}  ", "\u{10FFFF}a ", "  ", " \u{e9}"] {
        let trimmed = text.trim_end_matches(' ');
        let whole = TextRead {
            len: text.len(),
            trimmed: trimmed.len(),
            trimmed_chars: trimmed.chars().count(),
        };
        let b = text.as_bytes();
        for first in 0..=b.len() {
            for second in first..=b.len() {
                let runs = [&b[..first], &b[first..second], &b[second..]];
                assert_eq!(folded(&runs), Some(whole), "{text:?} cut at {first} and {second}");
            }
        }
    }
    for bad in [&[0xc3u8, 0x28][..], &[b'a', 0xff, b'b'], &[b'a', 0xe2, 0x82]] {
        for cut in 0..=bad.len() {
            assert_eq!(folded(&[&bad[..cut], &bad[cut..]]), None, "{bad:?} cut at {cut}");
        }
    }
}

/// **A text that is not UTF-8 is valid up to where `str::from_utf8` says**,
/// whichever bytes the runs end between: the error a field's unescape
/// raises is the fold's to name too.
#[test]
fn a_text_folded_in_runs_is_valid_up_to_where_it_is_whole() {
    for bad in [
        &[0xc3u8, 0x28][..],
        &[b'a', 0xff, b'b'],
        &[b'a', 0xe2, 0x82],
        "\u{e9}a\u{20ac}".as_bytes(),
        &[0xe2, 0x82, 0xac, b'x', 0xe2, 0x82, b'y'],
    ] {
        let whole = std::str::from_utf8(bad).err().map_or(bad.len(), |e| e.valid_up_to());
        for cut in 0..=bad.len() {
            let mut fold = TextFold::default();
            fold.push(&bad[..cut]);
            fold.push(&bad[cut..]);
            assert_eq!(fold.valid_up_to(), whole, "{bad:?} cut at {cut}");
        }
    }
}
