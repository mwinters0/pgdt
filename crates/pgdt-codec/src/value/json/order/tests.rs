//! **A `jsonb` field's positions order it as its document does**: over
//! seeds of every kind of value, and every one of them damaged a byte at a
//! time, read in place and as `COPY` escapes it, [`jsonb_order`] answers
//! what [`field_key`] and [`compare_keys`] answer — the same order, or the
//! same refusal.

use std::cmp::Ordering;

use pgdt_core::escape_field;

use crate::compare::CompareKind;
use crate::key::order::{OrderKey, compare_keys, field_key, jsonb_order};
use crate::value::nested::FieldText;

const SEEDS: &[&str] = &[
    r#"{"b": 1, "a": 2}"#,
    r#"{"a": 2, "b": 1}"#,
    r#"{"a":1,"a":2,"b":3}"#,
    r#"{"aa": 1, "b": [true, null]}"#,
    r#"{"é": 1, "e": 2, "a\"b": 3, "a\\b": 4}"#,
    r#"[1, "x", null, true, {"k": [1.0, 1e2]}]"#,
    r#""stré😀\n""#,
    "1",
    "-0.5e-3",
    "[]",
    "{}",
    "null",
    "true",
    r#"{"k": {"z": 1, "y": {"x": [false]}}, "j": "x\ty"}"#,
    " [ 1 , 2 ] ",
    r#"[[1, 2], [3], {"a": {}}]"#,
];

const BOUNDS: &[&str] = &[
    "{}",
    "[]",
    "1",
    "0.0005",
    r#""str""#,
    "\"stré😀\\n\"",
    r#"{"a": 2, "b": 1}"#,
    r#"{"a": 1}"#,
    r#"{"b": 1, "a": 3}"#,
    r#"[1, "x"]"#,
    r#"[1, "x", null, true, {"k": [1, 100]}]"#,
    r#"{"j": "x\ty", "k": {"y": {"x": [true]}, "z": 1}}"#,
    r#"[[1, 2], [3], {"a": {}}]"#,
    "null",
    "false",
    r#"{"e": 2, "a\"b": 3, "a\\b": 4, "é": 1}"#,
];

/// The bytes a byte-wise damage inserts, each a byte the grammar branches
/// on.
const INSERTED: &[u8] = b"{}[]\",:\\ 0-1.eEutnf";

/// Every field `seed` holds damaged once: a byte deleted, or one inserted,
/// at each place.
fn damaged(seed: &str) -> Vec<String> {
    let bytes = seed.as_bytes();
    let mut out = vec![seed.to_string()];
    for at in 0..=bytes.len() {
        let (head, tail) = bytes.split_at(at);
        if let Some((_, rest)) = tail.split_first() {
            out.extend(String::from_utf8([head, rest].concat()).ok());
        }
        for &byte in INSERTED {
            out.extend(String::from_utf8([head, &[byte], tail].concat()).ok());
        }
    }
    out
}

/// What the field's key answers against `bound`, or `None`.
fn keyed(field: &str, bound: &OrderKey) -> Option<Ordering> {
    field_key(&CompareKind::Jsonb, field).ok().map(|key| compare_keys(&key, bound))
}

#[test]
fn the_positions_order_a_field_as_its_document_does() {
    let bounds: Vec<OrderKey> =
        BOUNDS.iter().map(|bound| field_key(&CompareKind::Jsonb, bound).unwrap()).collect();
    let (mut read, mut orders) = (0, [0; 3]);
    for field in SEEDS.iter().flat_map(|seed| damaged(seed)) {
        let escaped = escape_field(Some(&field));
        let reads = field_key(&CompareKind::Jsonb, &field).is_ok();
        read += usize::from(reads);
        assert_eq!(jsonb_order(FieldText::plain(&field), None).is_some(), reads, "{field:?}");
        for bound in &bounds {
            let expected = keyed(&field, bound);
            if let Some(order) = expected {
                orders[(order as i8 + 1) as usize] += 1;
            }
            assert_eq!(
                jsonb_order(FieldText::plain(&field), Some(bound)),
                expected,
                "{field:?} against {bound:?}"
            );
            assert_eq!(
                jsonb_order(FieldText::escaped(&escaped), Some(bound)),
                expected,
                "{escaped:?} escaped, against {bound:?}"
            );
        }
    }
    // A corpus no field of which reads, or one every bound orders alike,
    // would agree vacuously.
    assert!(read > 300, "{read} fields read");
    assert!(orders.iter().all(|&n| n > 50), "{orders:?} less, equal and greater");
}

/// **A document past the depth the parser reads is refused as it is**, and
/// one at it ordered.
#[test]
fn a_document_past_its_depth_is_refused_alike() {
    let bound = field_key(&CompareKind::Jsonb, "[]").unwrap();
    for depth in [999, 1000, 1001, 5000] {
        let field = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let expected = keyed(&field, &bound);
        assert_eq!(jsonb_order(FieldText::plain(&field), Some(&bound)), expected, "{depth}");
    }
}
