//! **The walk reads every literal as the scanners it replaced read it**
//! (`prior_shape.rs`): the same answer from each of the seven input
//! functions, over a corpus generated from the grammars' own tokens and over
//! literals built to each grammar and then damaged, so a disagreement on a
//! literal nobody thought to write down is a test failure.

use super::prior_shape;
use super::tests::{box_in, circle_in, line_in, lseg_in, path_in, point_in, poly_in};

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
    "(",
    ")",
    "[",
    "]",
    "<",
    ">",
    "{",
    "}",
    ",",
    ",",
    " ",
    "\t",
    "\u{b}",
    "1",
    "0",
    "-1",
    "+2.5",
    ".5",
    "1.",
    ".",
    "1e3",
    "1e",
    "1e+",
    "2E-2",
    "1e400",
    "1e-400",
    "0e-400",
    "0x1p3",
    "0x",
    "0x.8",
    "0x1p",
    "0x1p1024",
    "inf",
    "-Infinity",
    "infinit",
    "nan",
    "NaN(a_1)",
    "nan(",
    "nan()",
    "nan(x",
    "x",
    "e",
    "_",
    "00",
    "007",
    "1_0",
    "é",
];

/// A number, as a field might spell one.
fn number(rng: &mut Rng) -> String {
    let n = rng.pick(&[
        "1",
        "-2",
        "0",
        "3.5",
        "1e-310",
        "1e308",
        "-1e308",
        "1.7976931348623157e308",
        "1.7976931348623159e308",
        "2e-324",
        "3e-324",
        "1e-6",
        "2e-6",
        "NaN",
        "inf",
        "-inf",
        "0x1p-1074",
        "0x1.fffffffffffff8p1023",
        "00000000000000000001",
        "1e400",
        "-0",
    ]);
    format!("{}{n}{}", blank(rng), blank(rng))
}

/// Whitespace, sometimes, around a part.
fn blank(rng: &mut Rng) -> &'static str {
    ["", "", "", " ", "\t", "  "][rng.below(6)]
}

/// A point, in parentheses or bare.
fn point(rng: &mut Rng) -> String {
    let (x, y) = (number(rng), number(rng));
    if rng.below(2) == 0 { format!("({x},{y})") } else { format!("{x},{y}") }
}

/// A literal built to one of the grammars: some points, separated or not,
/// in some bracket or none, or a `line`'s or a `circle`'s own form.
fn built(rng: &mut Rng) -> String {
    match rng.below(4) {
        0 => format!("{{{},{},{}}}", number(rng), number(rng), number(rng)),
        1 => {
            let open = rng.pick(&["<", "(", "((", ""]);
            let close = rng.pick(&[">", ")", ""]);
            format!("{open}{},{}{close}", point(rng), number(rng))
        }
        _ => {
            let n = rng.below(5) + 1;
            let points: Vec<String> = (0..n).map(|_| point(rng)).collect();
            let sep = rng.pick(&[",", ",", " ", ", "]);
            let (open, close) =
                [("[", "]"), ("(", ")"), ("", ""), ("((", "))"), ("([", "])")][rng.below(5)];
            format!("{}{open}{}{close}{}", blank(rng), points.join(sep), blank(rng))
        }
    }
}

/// One literal damaged: a byte dropped, doubled or replaced by a token.
fn damaged(rng: &mut Rng, literal: &str) -> String {
    let mut bytes = literal.as_bytes().to_vec();
    if bytes.is_empty() {
        return literal.to_string();
    }
    let at = rng.below(bytes.len());
    match rng.below(3) {
        0 => {
            bytes.remove(at);
        }
        1 => bytes.insert(at, bytes[at]),
        _ => {
            let token = rng.pick(TOKENS).as_bytes().to_vec();
            bytes.splice(at..=at, token);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Each of the seven input functions, by the walk and by the scanner it
/// replaced, answering how many read `literal`.
fn agree(literal: &str) -> usize {
    type Reader = fn(&str) -> bool;
    let readers: [(&str, Reader, Reader); 7] = [
        ("point", point_in, prior_shape::point_in),
        ("lseg", lseg_in, prior_shape::lseg_in),
        ("box", box_in, prior_shape::box_in),
        ("path", path_in, prior_shape::path_in),
        ("polygon", poly_in, prior_shape::poly_in),
        ("circle", circle_in, prior_shape::circle_in),
        ("line", line_in, prior_shape::line_in),
    ];
    readers
        .iter()
        .filter(|(name, walk, prior)| {
            let read = walk(literal);
            assert_eq!(read, prior(literal), "{name} {literal:?}");
            read
        })
        .count()
}

#[test]
fn the_walk_reads_free_form_literals_as_the_scanners_did() {
    let mut rng = Rng(0x5eed_0002);
    for _ in 0..60_000 {
        let n = rng.below(16) + 1;
        let literal: String = (0..n).map(|_| rng.pick(TOKENS)).collect();
        agree(&literal);
    }
}

#[test]
fn the_walk_reads_built_and_damaged_literals_as_the_scanners_did() {
    let mut rng = Rng(0x5eed_0003);
    let mut read = 0;
    for _ in 0..40_000 {
        let literal = built(&mut rng);
        read += agree(&literal);
        agree(&damaged(&mut rng, &literal));
    }
    // The corpus reaches values, not only refusals.
    assert!(read > 10_000, "{read} read");
}

/// **A number of any length is read as `strtod` reads it**: its digits past
/// those that decide its rounding, its leading zeros and its blanks change
/// nothing but where they are.
#[test]
fn a_long_number_is_read_as_the_scanners_did() {
    let long = |piece: &str, count: usize| piece.repeat(count);
    for literal in [
        format!("({}1,2)", long("0", 5000)),
        format!("(0.{}1,2)", long("0", 400)),
        format!("(1{},2)", long("0", 400)),
        format!("(1.{}1e-400,2)", long("0", 900)),
        // A tie past the kept digits, broken by a digit far after it.
        format!("(9007199254740993{},2)", long("0", 900)),
        format!("(9007199254740993{}1,2)", long("0", 900)),
        format!("({}1,2)", long(" ", 5000)),
        format!("[{}1,1]", long("1,1,", 2000)),
        format!("({}(1,1))", long("(1,1),", 2000)),
    ] {
        agree(&literal);
    }
    // Halfway past the largest double, `(2^54 - 1) × 2^970`, rounds to an
    // infinity, ties to even, and a unit below it does not; halfway to the
    // least subnormal, `5^1075 × 10^-1075`, rounds to zero, and a digit
    // past its last does not. Each is decided by its last digit, hundreds
    // of digits on.
    let overflow = digits(2, 970, (1 << 54) - 1);
    let mut below = overflow.clone();
    let last = below.iter().position(|&d| d != 0).unwrap();
    below[last] -= 1;
    let underflow = digits(5, 1075, 1);
    for (text, read) in [
        (spelled(&overflow), false),
        (spelled(&below), true),
        (format!("{}e-1075", spelled(&underflow)), false),
        (format!("{}1e-1076", spelled(&underflow)), true),
    ] {
        assert_eq!(agree(&format!("({text},1)")) > 0, read, "{text}");
    }
}

/// `base^power × times` in decimal digits, least significant first.
fn digits(base: u64, power: u32, times: u64) -> Vec<u8> {
    let mut out = vec![1u8];
    let mut multiply = |by: u64| {
        let mut carry = 0u128;
        for digit in &mut out {
            let n = u128::from(*digit) * u128::from(by) + carry;
            *digit = (n % 10) as u8;
            carry = n / 10;
        }
        while carry > 0 {
            out.push((carry % 10) as u8);
            carry /= 10;
        }
    };
    for _ in 0..power {
        multiply(base);
    }
    multiply(times);
    out
}

/// Digits, least significant first, as text.
fn spelled(digits: &[u8]) -> String {
    digits.iter().rev().map(|d| char::from(b'0' + d)).collect()
}
