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
