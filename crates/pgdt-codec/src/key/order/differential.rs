//! **Each push machine answers what the slice reader it replaced answered**
//! (`prior_shape.rs`): every text up to a few bytes over the bytes its
//! grammar turns on, and built and damaged texts past them.

use std::sync::Arc;

use super::field_key;
use super::prior_shape;
use crate::compare::CompareKind;

/// Every text of at most `len` bytes over `alphabet`.
fn texts(alphabet: &[u8], len: usize) -> Vec<String> {
    let mut all = vec![String::new()];
    let mut last = vec![Vec::new()];
    for _ in 0..len {
        last = last
            .iter()
            .flat_map(|text: &Vec<u8>| {
                alphabet.iter().map(move |&byte| [text.as_slice(), &[byte]].concat())
            })
            .collect();
        all.extend(last.iter().map(|text| String::from_utf8(text.clone()).unwrap()));
    }
    all
}

fn agree(kind: &CompareKind, text: &str) {
    assert_eq!(field_key(kind, text), prior_shape::field_key(kind, text), "{kind:?} {text:?}");
}

/// Each text wrapped in blanks, zeros and signs, and cut at every byte.
fn damaged(text: &str) -> Vec<String> {
    let mut out = vec![text.to_string()];
    for wrap in [" ", "\t", "0", "00", "+", "-", "_", "x"] {
        out.push(format!("{wrap}{text}"));
        out.push(format!("{text}{wrap}"));
    }
    out.extend((0..text.len()).filter(|&at| text.is_char_boundary(at)).map(|at| {
        let (head, tail) = text.split_at(at);
        format!("{head}{}", tail.get(1..).unwrap_or(""))
    }));
    out
}

#[test]
fn an_integer_is_read_as_its_slice_reader_read_it() {
    let edges = [
        "32767",
        "-32768",
        "32768",
        "2147483647",
        "-2147483648",
        "2147483648",
        "9223372036854775807",
        "-9223372036854775808",
        "9223372036854775808",
        "170141183460469231731687303715884105728",
        "0x7FFF",
        "-0x8000",
        "0o17",
        "0b1_0",
        "1_000",
        "0x_1",
    ];
    for bytes in [2, 4, 8] {
        let kind = CompareKind::Int { bytes };
        for text in texts(b"0179+-_xob \t", 5) {
            agree(&kind, &text);
        }
        for text in edges.iter().flat_map(|edge| damaged(edge)) {
            agree(&kind, &text);
        }
    }
}

#[test]
fn an_oid_is_read_as_its_slice_reader_read_it() {
    let kind = CompareKind::UnsignedInt;
    for text in texts(b"0178+-xXbf \t", 5) {
        agree(&kind, &text);
    }
    for edge in [
        "4294967295",
        "4294967296",
        "18446744073709551615",
        "18446744073709551616",
        "18446744071562067968",
        "18446744071562067967",
        "-2147483648",
        "-2147483649",
        "0xFFFFFFFF",
        "0x100000000",
        "0777",
        "0b101",
    ] {
        for text in damaged(edge) {
            agree(&kind, &text);
        }
    }
}

#[test]
fn a_boolean_is_read_as_its_slice_reader_read_it() {
    for text in texts(b"tfTyYnoO10ers \t", 5) {
        agree(&CompareKind::Bool, &text);
    }
    for word in ["true", "false", "FALSE", "yes", "off", "t\u{a0}", "\u{e9}"] {
        for text in damaged(word) {
            agree(&CompareKind::Bool, &text);
        }
    }
}

#[test]
fn an_enum_is_read_as_its_slice_reader_read_it() {
    let longest = "x".repeat(63);
    let labels: Arc<[String]> = ["a", "ab", "\u{e9}", longest.as_str()].map(String::from).into();
    for exact in [false, true] {
        let kind = CompareKind::Enum { name: "e".into(), labels: labels.clone(), exact };
        for text in texts(b"abx", 3) {
            agree(&kind, &text);
        }
        for length in [62, 63, 64, 65, 200] {
            agree(&kind, &"x".repeat(length));
        }
        agree(&kind, "\u{e9}");
    }
}

#[test]
fn a_uuid_is_read_as_its_slice_reader_read_it() {
    let canonical = "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11";
    let bare = canonical.replace('-', "");
    let mut built = vec![
        canonical.to_string(),
        bare.clone(),
        format!("{{{canonical}}}"),
        format!("{{{bare}}}"),
        format!("{{{canonical}"),
        format!("{canonical}}}"),
        format!("{{{canonical}}}}}"),
        format!("{{{{{canonical}}}"),
        canonical.to_uppercase(),
        "{}".to_string(),
        "{".to_string(),
        String::new(),
    ];
    // A hyphen after every group of four, and one where none may stand.
    built.push(
        bare.as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("-"),
    );
    built.push(format!("-{bare}"));
    built.push(format!("{bare}-"));
    built.push(format!("{}--{}", &bare[..4], &bare[4..]));
    for text in &built {
        for text in damaged(text) {
            agree(&CompareKind::Uuid, &text);
            for byte in ["-", "{", "}", "g", "F"] {
                for at in (0..text.len()).filter(|&at| text.is_char_boundary(at)) {
                    agree(&CompareKind::Uuid, &format!("{}{byte}{}", &text[..at], &text[at..]));
                }
            }
        }
    }
}

/// Each `numeric` column's reading of `text` held to the slice readers', a
/// field's key and refusal, a typmod's unscaled digits and a literal's key.
fn agree_numeric(kinds: &[CompareKind], text: &str) {
    use crate::value::numeric::{self, NumericKey, prior_shape};
    for kind in kinds {
        assert_eq!(field_key(kind, text), prior_shape::field_key(kind, text), "{kind:?} {text:?}");
    }
    for (precision, scale) in [(5, 2), (1, 0), (3, -2), (1, 1000), (1000, -1000)] {
        assert_eq!(
            numeric::typmod_unscaled_digits(text, precision, scale),
            prior_shape::typmod_unscaled_digits(text, precision, scale),
            "numeric({precision},{scale}) {text:?}"
        );
    }
    assert_eq!(NumericKey::parse(text), prior_shape::parse(text), "{text:?}");
}

fn numeric_kinds() -> Vec<CompareKind> {
    use crate::declared::NumericTypmod;
    let typmod = |precision, scale| CompareKind::Numeric {
        infinities: false,
        typmod: Some(NumericTypmod { precision, scale }),
    };
    vec![
        CompareKind::Decimal { precision: 5, scale: 2 },
        CompareKind::Decimal { precision: 38, scale: 0 },
        CompareKind::Decimal { precision: 4, scale: -2 },
        CompareKind::Decimal { precision: 3, scale: 5 },
        CompareKind::Numeric { infinities: true, typmod: None },
        CompareKind::Numeric { infinities: false, typmod: None },
        typmod(5, 2),
        typmod(1, 0),
        typmod(3, -2),
        typmod(2, 5),
        typmod(1, 1000),
        typmod(1000, -1000),
    ]
}

#[test]
fn a_numeric_is_read_as_its_slice_readers_read_it() {
    let kinds = numeric_kinds();
    for text in texts(b"019.-+eE_x ", 5) {
        agree_numeric(&kinds, &text);
    }
    for text in texts(b"05.-ebo_N", 4) {
        agree_numeric(&kinds, &text);
    }
    let zeros = |n: usize| "0".repeat(n);
    let built = [
        "NaN".to_string(),
        "nan".to_string(),
        "Infinity".to_string(),
        "-Infinity".to_string(),
        "+infinity".to_string(),
        "inf".to_string(),
        "-inf".to_string(),
        "infinit".to_string(),
        " NaN ".to_string(),
        "1e1073741822".to_string(),
        "1e1073741823".to_string(),
        "1e1073741824".to_string(),
        "1e-1073741823".to_string(),
        // A blank after the `e`, which only v14's `strtol` reads.
        "1e 1073741822".to_string(),
        "1e 1073741823".to_string(),
        "1e -1073741823".to_string(),
        "1e 5".to_string(),
        "1e +5".to_string(),
        "1e-5".to_string(),
        "1.e5".to_string(),
        ".5e-3".to_string(),
        "0x1F".to_string(),
        "-0X_1f".to_string(),
        "0b1_0".to_string(),
        "0o17".to_string(),
        "0x3E7".to_string(),
        "0x3E8".to_string(),
        "1_000.5_5".to_string(),
        "1_000.5_5e1_0".to_string(),
        "1._5".to_string(),
        "99999.995".to_string(),
        "999.995".to_string(),
        "-999.994".to_string(),
        "-0.005".to_string(),
        "0.0001".to_string(),
        "12345678901234567890123456789012345678".to_string(),
        "123456789012345678901234567890123456789".to_string(),
        format!("{}1", zeros(1000)),
        format!("0.{}5", zeros(999)),
        format!("0.{}5", zeros(1000)),
        format!("1{}", zeros(1000)),
        format!("5{}", zeros(999)),
        "1e131071".to_string(),
        "1e131072".to_string(),
        "1e-16383".to_string(),
        "1e-16384".to_string(),
        format!("{}e-16383", zeros(3)),
        format!("0x{}", "f".repeat(300)),
        format!("0x{}1", zeros(300)),
    ];
    for text in &built {
        for text in damaged(text) {
            agree_numeric(&kinds, &text);
        }
    }
    // Texts at I63's bounds, too long to damage at every byte: at its bound
    // in bits, where only the decimal length decides, 2^435411 is stored and
    // 2^435412 - 1 is not.
    let long = [
        format!("1{}", zeros(131071)),
        format!("1{}", zeros(131072)),
        format!("{}1{}", zeros(5), zeros(131071)),
        format!("0.{}1", zeros(16382)),
        format!("0.{}1", zeros(16383)),
        format!("1.{}", zeros(16384)),
        format!("0x8{}", zeros(108852)),
        format!("0x{}", "f".repeat(108853)),
        format!("0x1{}", zeros(108853)),
    ];
    for text in &long {
        for text in [text.clone(), format!("-{text}"), format!("{text} "), format!("{text}x")] {
            agree_numeric(&kinds, &text);
        }
    }
}
