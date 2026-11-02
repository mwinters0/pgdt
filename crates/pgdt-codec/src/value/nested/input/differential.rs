//! **The walks read every literal as the scanners they replaced read it**
//! (`prior_shape.rs`): the same literal, or the same refusal, over a corpus
//! generated from the grammars' own bytes and over literals built to each
//! grammar and then damaged, so a disagreement on a literal nobody thought
//! to write down is a test failure.

use super::prior_shape;
use super::*;

/// A deterministic 64-bit LCG, so a failure is reproducible from the seed
/// alone and the corpus does not have to be committed.
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

    fn pick<'t>(&mut self, from: &[&'t str]) -> &'t str {
        from[self.below(from.len())]
    }
}

/// The pieces each grammar branches on, and a few it does not.
const TOKENS: &[&str] = &[
    "{",
    "}",
    "[",
    "]",
    "(",
    ")",
    ",",
    ";",
    ":",
    "=",
    "\"",
    "\"\"",
    "\\",
    " ",
    "\t",
    "\n",
    "1",
    "-",
    "+",
    "0",
    "42",
    "a",
    "b c",
    "x",
    "NULL",
    "null",
    "NuLl",
    "empty",
    "EMPTY",
    "é",
    "[1:2]=",
    "32768",
    "99999999999",
];

/// Every reading the walks replace, of one literal, by the walks and by the
/// scanners they replaced, answering how many of them read a value.
fn agree(literal: &str) -> usize {
    assert_eq!(parse_array(literal), prior_shape::parse_array(literal), "array {literal:?}");
    assert_eq!(
        parse_array_delimited(literal, b';'),
        prior_shape::parse_array_delimited(literal, b';'),
        "array split at ; {literal:?}"
    );
    for columns in 0..4 {
        assert_eq!(
            parse_record(literal, columns),
            prior_shape::parse_record(literal, columns),
            "record of {columns} {literal:?}"
        );
    }
    assert_eq!(parse_range(literal), prior_shape::parse_range(literal), "range {literal:?}");
    assert_eq!(
        parse_multirange(literal),
        prior_shape::parse_multirange(literal),
        "multirange {literal:?}"
    );
    assert_eq!(
        parse_int2vector(literal),
        prior_shape::parse_int2vector(literal),
        "int2vector {literal:?}"
    );
    usize::from(parse_array(literal).is_some())
        + usize::from(parse_array_delimited(literal, b';').is_some())
        + (0..4).filter(|&columns| parse_record(literal, columns).is_some()).count()
        + usize::from(parse_range(literal).is_some())
        + usize::from(parse_multirange(literal).is_some())
        + usize::from(parse_int2vector(literal).is_some())
}

#[test]
fn the_walks_read_free_form_literals_as_the_scanners_did() {
    let mut rng = Rng(0x5eed_0001);
    for _ in 0..60_000 {
        let n = rng.below(24) + 1;
        let literal: String = (0..n).map(|_| rng.pick(TOKENS)).collect();
        agree(&literal);
    }
}

/// Whitespace, sometimes, around a part.
fn blank(rng: &mut Rng) -> &'static str {
    ["", "", "", " ", "\t", "  "][rng.below(6)]
}

/// A scalar's text as one of the grammars quotes or escapes it, or leaves it
/// bare.
fn scalar(rng: &mut Rng) -> String {
    let word = rng.pick(&["1", "-7", "a", "b c", "NULL", "null", "", "x,y", "é", "1 2"]);
    match rng.below(5) {
        0 => format!("\"{}\"", word.replace('"', "\\\"")),
        1 => format!("\"{word}\\\"q\""),
        2 => word.replace(',', "\\,"),
        _ => word.to_string(),
    }
}

/// An array literal of `depth` levels at most, split at `delim`.
fn array(rng: &mut Rng, depth: usize, delim: char) -> String {
    let mut out = String::new();
    if rng.below(6) == 0 {
        out.push_str(rng.pick(&["[1:2]=", "[0:1]=", "[2]=", "[1:2][1:1]=", "[1:2] = "]));
    }
    out.push_str(blank(rng));
    out.push('{');
    let n = rng.below(4);
    for k in 0..n {
        if k > 0 {
            out.push(delim);
        }
        out.push_str(blank(rng));
        if depth > 0 && rng.below(3) == 0 {
            out.push_str(&array(rng, depth - 1, delim));
        } else {
            out.push_str(&scalar(rng));
        }
        out.push_str(blank(rng));
    }
    out.push('}');
    out.push_str(blank(rng));
    out
}

/// A record literal of up to three fields.
fn record(rng: &mut Rng) -> String {
    let fields: Vec<String> = (0..rng.below(4))
        .map(|_| match rng.below(4) {
            0 => String::new(),
            1 => format!("\"{}\"\"x\"", scalar(rng)),
            _ => format!("{}{}{}", blank(rng), scalar(rng), blank(rng)),
        })
        .collect();
    format!("{}({}){}", blank(rng), fields.join(","), blank(rng))
}

/// A range literal.
fn range(rng: &mut Rng) -> String {
    if rng.below(6) == 0 {
        return format!("{}{}{}", blank(rng), rng.pick(&["empty", "EmPtY"]), blank(rng));
    }
    let bound = |rng: &mut Rng| match rng.below(4) {
        0 => String::new(),
        1 => format!("\"{}\"", rng.pick(&["a,b", "c\"\"d", "", "e)f"])),
        2 => rng.pick(&["\\ ", "a\\)", "b\\ "]).to_string(),
        _ => scalar(rng),
    };
    format!(
        "{}{}{},{}{}{}",
        blank(rng),
        rng.pick(&["[", "("]),
        bound(rng),
        bound(rng),
        rng.pick(&["]", ")"]),
        blank(rng)
    )
}

/// A multirange literal.
fn multirange(rng: &mut Rng) -> String {
    let members: Vec<String> = (0..rng.below(4))
        .map(|_| if rng.below(5) == 0 { "empty".to_string() } else { range(rng) })
        .collect();
    format!("{}{{{}}}{}", blank(rng), members.join(","), blank(rng))
}

/// An `int2vector` literal.
fn int2vector(rng: &mut Rng) -> String {
    (0..rng.below(4))
        .map(|_| {
            let sep = rng.pick(&[" ", "  ", "\t"]);
            format!("{sep}{}", rng.pick(&["1", "+2", "-3", "007", "32767", "-32768", "32768"]))
        })
        .collect()
}

/// **Each grammar's own literals, two thirds well-formed and the rest
/// damaged at one byte**, which is where the accepting branches beside the
/// refusing ones are exercised.
#[test]
fn the_walks_read_built_literals_as_the_scanners_did() {
    let mut rng = Rng(0x5eed_0002);
    let mut read = 0;
    for _ in 0..60_000 {
        let literal = match rng.below(6) {
            0 => array(&mut rng, 2, ','),
            1 => array(&mut rng, 2, ';'),
            2 => record(&mut rng),
            3 => range(&mut rng),
            4 => multirange(&mut rng),
            _ => int2vector(&mut rng),
        };
        let literal = if rng.below(3) == 0 && !literal.is_empty() {
            let mut bytes: Vec<char> = literal.chars().collect();
            let at = rng.below(bytes.len());
            bytes[at] = rng.pick(TOKENS).chars().next().unwrap_or(' ');
            bytes.into_iter().collect()
        } else {
            literal
        };
        read += usize::from(agree(&literal) > 0);
    }
    // The corpus reaches the accepting branches, not only the refusals.
    assert!(read > 20_000, "{read} literals read as a value");
}
