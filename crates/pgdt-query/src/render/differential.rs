use arrow::datatypes::Field;

use std::sync::Arc;

use arrow::array::builder::{Int32Builder, StringViewBuilder};
use arrow::buffer::OffsetBuffer;
use arrow::datatypes::Fields;

use super::*;

/// The same deterministic 64-bit LCG `decode.rs`'s differential tests use,
/// so a failure is reproducible from the seed alone and the corpus does
/// not have to be committed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Every byte class the array grammar branches on: the force-quote set,
/// each whitespace character `array_isspace` knows, and ordinary text.
const ALPHABET: &[&str] = &[
    "a", "Z", "0", ",", "{", "}", "\"", "\\", " ", "\t", "\n", "\r", "\x0b", "\x0c", "é", "N", "U",
    "L",
];

fn element(rng: &mut Rng) -> Option<String> {
    match rng.below(12) {
        0 => return None,
        1 => return Some(String::new()),
        2 => return Some("NULL".to_string()),
        3 => return Some("null".to_string()),
        _ => {}
    }
    let len = rng.below(6);
    Some((0..len).map(|_| ALPHABET[rng.below(ALPHABET.len())]).collect())
}

fn utf8_list(rows: &[Vec<Option<String>>]) -> ArrayRef {
    let mut values = StringViewBuilder::new();
    for row in rows {
        for v in row {
            match v {
                Some(s) => values.append_value(s),
                None => values.append_null(),
            }
        }
    }
    Arc::new(ListArray::new(
        Arc::new(Field::new("item", DataType::Utf8View, true)),
        OffsetBuffer::from_lengths(rows.iter().map(Vec::len)),
        Arc::new(values.finish()) as ArrayRef,
        None,
    ))
}

/// Wrap a `List` in another `List`, `width` inner lists to each outer one
/// — the rectangular shape `append_typed` is the only thing that produces.
fn nest(inner: ArrayRef, width: usize) -> ArrayRef {
    let outer = inner.len() / width;
    let field = Arc::new(Field::new("item", inner.data_type().clone(), true));
    Arc::new(ListArray::new(
        field,
        OffsetBuffer::from_lengths((0..outer).map(|_| width)),
        inner,
        None,
    ))
}

#[track_caller]
fn agrees(column: &ArrayRef, plan: &NestedPlan, child: &NestedPlan) {
    for row in 0..column.len() {
        let expected = prior_shape::render_array(column.as_ref(), row, child)
            .expect("the oracle renders every corpus row");
        let actual = render_plan(column.as_ref(), row, plan)
            .expect("the walk renders every corpus row")
            .expect("no corpus row is a SQL NULL array");
        assert_eq!(actual, expected, "row {row}");
    }
}

/// One dimension, generated element text: the shape every registered
/// figure's array columns are, and where the quoting decision lives.
#[test]
fn a_one_dimensional_array_renders_as_the_literal_builder_did() {
    let mut rng = Rng(0x5eed_0717);
    let rows: Vec<Vec<Option<String>>> =
        (0..400).map(|_| (0..rng.below(7)).map(|_| element(&mut rng)).collect()).collect();
    assert!(rows.iter().any(Vec::is_empty), "the corpus must reach the empty array");
    let column = utf8_list(&rows);
    let child = NestedPlan::Scalar;
    agrees(&column, &NestedPlan::array(child.clone()), &child);
}

/// Two dimensions, including the all-empty shape `array_out` writes as
/// `{}` whatever its dimensionality (I20) — the one case where the walk
/// has to unwind the braces it has already written.
#[test]
fn a_two_dimensional_array_renders_as_the_literal_builder_did() {
    let mut rng = Rng(0x5eed_0718);
    for width in [1usize, 2, 3] {
        for inner_len in [0usize, 1, 4] {
            let rows: Vec<Vec<Option<String>>> = (0..(6 * width))
                .map(|_| (0..inner_len).map(|_| element(&mut rng)).collect())
                .collect();
            let column = nest(utf8_list(&rows), width);
            let child = NestedPlan::array(NestedPlan::Scalar);
            agrees(&column, &NestedPlan::array(child.clone()), &child);
        }
    }
}

/// Three dimensions, so the depth-keyed rectangularity check is exercised
/// below the level a two-dimensional value reaches.
#[test]
fn a_three_dimensional_array_renders_as_the_literal_builder_did() {
    let mut rng = Rng(0x5eed_0719);
    let rows: Vec<Vec<Option<String>>> =
        (0..24).map(|_| (0..2).map(|_| element(&mut rng)).collect()).collect();
    let column = nest(nest(utf8_list(&rows), 3), 2);
    let child = NestedPlan::array(NestedPlan::array(NestedPlan::Scalar));
    agrees(&column, &NestedPlan::array(child.clone()), &child);
}

/// A nested element that is not a scalar: the composite arm builds its
/// own literal and goes through the same quoting decision.
#[test]
fn an_array_of_composites_renders_as_the_literal_builder_did() {
    let mut rng = Rng(0x5eed_071a);
    let mut a = Int32Builder::new();
    let mut b = StringViewBuilder::new();
    let mut lengths = Vec::new();
    for _ in 0..60 {
        let len = rng.below(4);
        lengths.push(len);
        for _ in 0..len {
            match rng.below(5) {
                0 => a.append_null(),
                _ => a.append_value(rng.below(1000) as i32 - 500),
            }
            match element(&mut rng) {
                Some(s) => b.append_value(&s),
                None => b.append_null(),
            }
        }
    }
    let fields = Fields::from(vec![
        Field::new("a", DataType::Int32, true),
        Field::new("b", DataType::Utf8View, true),
    ]);
    let values = Arc::new(StructArray::new(
        fields.clone(),
        vec![Arc::new(a.finish()) as ArrayRef, Arc::new(b.finish()) as ArrayRef],
        None,
    )) as ArrayRef;
    let column = Arc::new(ListArray::new(
        Arc::new(Field::new("item", DataType::Struct(fields), true)),
        OffsetBuffer::from_lengths(lengths),
        values,
        None,
    )) as ArrayRef;
    let child = NestedPlan::record(vec![NestedPlan::Scalar, NestedPlan::Scalar]);
    agrees(&column, &NestedPlan::array(child.clone()), &child);
}
