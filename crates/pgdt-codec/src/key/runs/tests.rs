use pgdt_model::PostgresInvalidValues;

use super::*;
use crate::key::order::{compare_keys, literal_key, read_key};

/// What the key a field's text reads as answers against `bound`.
fn keyed(kind: &CompareKind, bound: &OrderKey, text: &str) -> Option<Ordering> {
    let key = read_key(kind, text, PostgresInvalidValues::Default)?;
    Some(compare_keys(&key, bound))
}

/// The kinds a [`FieldOrder`] reads, a typmod's among them.
fn kinds() -> Vec<CompareKind> {
    vec![
        CompareKind::Text { length: None, varying: true },
        CompareKind::Text { length: Some(3), varying: true },
        CompareKind::PaddedText { length: None },
        CompareKind::PaddedText { length: Some(3) },
        CompareKind::Bytea,
    ]
}

/// **A field ordered a run at a time answers as its key does**: over every
/// kind, every bound and every text, the text cut into three runs at every
/// pair of places — a character, a blank run and a `bytea`'s escape split
/// among them — the order its key compares by, and no order where it reads
/// no key: a text past its typmod, a `bytea` no form decodes.
#[test]
fn a_field_ordered_in_runs_answers_as_its_key() {
    let literals = ["", "ab", "abc", "ab ", "b", "\u{e9}", "\\x6162", "\\x61", "ab\\\\", "\\x"];
    let texts = [
        "",
        "a",
        "ab",
        "ab ",
        "ab  ",
        "abc",
        "abcd",
        "abc  ",
        "a b",
        " ",
        "  a",
        "\u{e9}\u{e9}\u{e9} ",
        "\u{e9}\u{e9}\u{e9}\u{e9}",
        "\\x",
        "\\x61",
        "\\x6162",
        "\\x616263",
        "\\x6",
        "\\x6g",
        "\\x61 62",
        "ab\\\\",
        "ab\\134",
        "a\\001b",
        "a\\",
        "\\\\x61",
    ];
    for kind in kinds() {
        for literal in literals {
            let Some(bound) =
                literal_key(&kind, literal, crate::compare::ComparisonSemantics::Postgres)
            else {
                continue;
            };
            for text in texts {
                let want = keyed(&kind, &bound, text);
                let order = || FieldOrder::new(&kind, &bound).expect("a bytewise kind");
                assert_eq!(order().of(text), want, "{kind:?} {literal:?} {text:?} whole");
                let b = text.as_bytes();
                for first in 0..=b.len() {
                    for second in first..=b.len() {
                        let mut runs = order();
                        for run in [&b[..first], &b[first..second], &b[second..]] {
                            runs.push(run);
                        }
                        assert_eq!(
                            runs.finish(),
                            want,
                            "{kind:?} {literal:?} {text:?} cut at {first} and {second}"
                        );
                    }
                }
            }
        }
    }
}

/// **A long `bytea` is ordered past the run its decoded bytes are compared
/// in**: a difference at its last byte and a length one past the bound's
/// both found, the bytes ordered across many decoded runs.
#[test]
fn a_long_bytea_orders_across_its_decoded_runs() {
    let value: Vec<u8> = (0..4 * DECODED_RUN + 3).map(|i| (i % 251) as u8).collect();
    let hex = |bytes: &[u8]| {
        let mut text = String::from("\\x");
        for byte in bytes {
            text.push_str(&format!("{byte:02x}"));
        }
        text
    };
    let bound = OrderKey::Bytes(value.clone());
    let mut later = value.clone();
    *later.last_mut().unwrap() += 1;
    let longer = [value.as_slice(), &[0]].concat();
    for (bytes, want) in [
        (value.as_slice(), Ordering::Equal),
        (&later, Ordering::Greater),
        (&longer, Ordering::Greater),
        (&value[..value.len() - 1], Ordering::Less),
    ] {
        let order = FieldOrder::new(&CompareKind::Bytea, &bound).unwrap();
        assert_eq!(order.of(&hex(bytes)), Some(want), "{} bytes", bytes.len());
    }
}

/// **Only a bytewise kind is ordered here**, and only against its own key.
#[test]
fn only_a_bytewise_kind_is_ordered_in_runs() {
    let text = OrderKey::Text("a".into());
    assert!(FieldOrder::new(&CompareKind::Int { bytes: 4 }, &OrderKey::Int(1)).is_none());
    assert!(FieldOrder::new(&CompareKind::Bytea, &text).is_none());
    assert!(FieldOrder::new(&CompareKind::Text { length: None, varying: true }, &text).is_some());
}

/// **A trimmed text drops only its trailing blanks**: a run of blanks a
/// non-blank follows is compared, however long, and one at the end is not,
/// across any cut.
#[test]
fn a_trimmed_text_compares_its_inner_blanks_and_drops_its_trailing_ones() {
    let blanks = " ".repeat(10_000);
    for (text, bound, want) in [
        (format!("a{blanks}"), "a", Ordering::Equal),
        (format!("a{blanks}b"), "a", Ordering::Greater),
        (format!("a{blanks}b"), "a  ", Ordering::Greater),
        (format!("a{blanks}"), "a b", Ordering::Less),
        (blanks.clone(), "", Ordering::Equal),
        (format!("{blanks}!"), "a", Ordering::Less),
    ] {
        let b = text.as_bytes();
        for cut in [0, 1, 2, b.len() / 2, b.len() - 1, b.len()] {
            let mut runs = BytewiseRuns::trimmed(bound.trim_end_matches(' ').as_bytes());
            runs.push(&b[..cut]);
            runs.push(&b[cut..]);
            let whole =
                text.trim_end_matches(' ').as_bytes().cmp(bound.trim_end_matches(' ').as_bytes());
            assert_eq!(runs.finish(), whole, "{bound:?} cut at {cut}");
            assert_eq!(whole, want, "{bound:?}");
        }
    }
}
