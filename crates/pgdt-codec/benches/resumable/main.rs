//! **A resumable reader priced**: one reader written three ways and timed
//! over the same literals, so the form the input walks resume in is chosen
//! by its cost rather than argued
//! (`docs/design/roadmap-P33-line-bound.md`, "A pass holds a line only where
//! it decodes it").
//!
//! - **`pull`** ([`pull`]) — the input walks' shape at `0fd33bd3`
//!   (`value/source.rs`, `value/nested/input.rs` there), since written in the
//!   push form: a walk pulling bytes off a `dyn Source` with a
//!   peek and a fork, each part read through a `Part` and its `Lexer`. It
//!   reads one contiguous field; fed over runs it reads an iterator chaining
//!   them, which is what it would cost were the runs held, not a resumption.
//! - **`async`** ([`walk`]) — the same walks as `async fn`s, compiled by the
//!   compiler into the state machine a run boundary suspends: a source polls
//!   for its next byte and is `Pending` where the run it was fed has ended.
//! - **`push`** ([`push`]) — hand-written state machines, fed a run at a
//!   time, a frame for each container open and a lexer's state in each.
//!
//! **One grammar, three forms**: each reads `array_in`'s braces, separators,
//! quoted and unquoted elements, escapes, whitespace and `NULL`, `record_in`'s
//! fields, an integer as `pg_strtoint*` reads a decimal, and a text as a
//! count of its bytes, under a plan dispatched at run time as a
//! `ComparisonPlan`'s depth is. Neither resumable form looks further ahead
//! than a fixed bound: the two forks of the pull walk, the `NULL` test and an
//! unquoted element's whitespace run, become carried state — the letters
//! read so far, the run as its bytes' run lengths. **An `[m:n]=` decoration,
//! a radix prefix and the ranges and multiranges are not read**, by any of
//! the three; the grammar is the hot path's, not a port.
//!
//! **A form is asserted to answer as the others do before anything is
//! timed** ([`agreement`]): every literal timed, over each feed, and a list of
//! refused and accepted literals besides — a ragged array, a misquoted
//! element, a nested plan two deep.
//!
//! What the figure reads, and the criterion it decides by, is
//! `scripts/measure.py`'s `resumable-reader-micro`.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

mod pull;
mod push;
mod walk;

/// `pgdt`'s allocator, as `benches/decoders.rs` declares it: what a form
/// allocates — `pull`'s forks, an array's dimensions, a nested container's
/// boxed future or frame — is timed against the binary's heap.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// **What a reader is asked to read a field as**, chosen at run time.
#[derive(Debug, Clone)]
pub(crate) enum Plan {
    /// A decimal integer, `pg_strtoint*`'s, within `min..=max` — `int4`'s
    /// range for `int4`.
    Int { min: i64, max: i64 },
    /// A text, read to its end.
    Text,
    /// An `array_in` literal of elements read as the plan.
    Array(Box<Plan>),
    /// A `record_in` literal, a field per plan.
    Record(Vec<Plan>),
}

impl Plan {
    pub(crate) fn int4() -> Self {
        Plan::Int { min: i64::from(i32::MIN), max: i64::from(i32::MAX) }
    }

    pub(crate) fn int8() -> Self {
        Plan::Int { min: i64::MIN, max: i64::MAX }
    }
}

/// **A literal one of these grammars refuses.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refused;

pub(crate) type Walked<T = ()> = Result<T, Refused>;

/// **What every form answers of a field**: enough that no reading is dead
/// code, and that two forms disagreeing is seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Answer {
    /// The leaves read, a NULL among them.
    pub(crate) leaves: u64,
    pub(crate) nulls: u64,
    /// The integers read, summed, wrapping.
    pub(crate) sum: i64,
    /// The bytes of every text read.
    pub(crate) text_bytes: u64,
}

/// `array_isspace`, `scanner_isspace` and the C locale's `isspace`: one set.
pub(crate) fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// PostgreSQL's `MAXDIM`.
pub(crate) const MAXDIM: usize = 6;

/// **An integer read a byte at a time**, as each form's integer reader reads
/// one: blanks, a sign, decimal digits with an underscore between two, and
/// blanks. The resumable forms carry it between runs; the pull form keeps it
/// on its stack.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IntState {
    stage: IntStage,
    negative: bool,
    magnitude: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntStage {
    Leading,
    /// A sign read, no digit yet.
    Signed,
    Digits,
    /// An underscore read, a digit owed.
    Underscore,
    Trailing,
}

impl IntState {
    pub(crate) fn new() -> Self {
        Self { stage: IntStage::Leading, negative: false, magnitude: 0 }
    }

    pub(crate) fn byte(&mut self, byte: u8) -> Walked {
        use IntStage::*;
        self.stage = match (self.stage, byte) {
            (Leading, byte) if is_space(byte) => Leading,
            (Leading, b'-') => {
                self.negative = true;
                Signed
            }
            (Leading, b'+') => Signed,
            (Leading | Signed | Digits | Underscore, b'0'..=b'9') => {
                self.magnitude =
                    (self.magnitude * 10 + i128::from(byte - b'0')).min(i128::from(u64::MAX));
                Digits
            }
            (Digits, b'_') => Underscore,
            (Digits | Trailing, byte) if is_space(byte) => Trailing,
            _ => return Err(Refused),
        };
        Ok(())
    }

    /// The value read, once the text has ended, within `min..=max`.
    pub(crate) fn end(&self, min: i64, max: i64) -> Walked<i64> {
        if !matches!(self.stage, IntStage::Digits | IntStage::Trailing) {
            return Err(Refused);
        }
        let value = if self.negative { -self.magnitude } else { self.magnitude };
        if i128::from(min) <= value && value <= i128::from(max) {
            Ok(value as i64)
        } else {
            Err(Refused)
        }
    }
}

/// **An unquoted whitespace run carried as its bytes' run lengths**, where
/// the pull form forks to see past it: whether it is an element's interior or
/// its trailing blanks is known only at the byte after it.
#[derive(Debug, Default, Clone)]
pub(crate) struct Blanks {
    runs: Vec<(u8, u32)>,
}

impl Blanks {
    pub(crate) fn push(&mut self, byte: u8) {
        match self.runs.last_mut() {
            Some((last, count)) if *last == byte => *count += 1,
            _ => self.runs.push((byte, 1)),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.runs.clear();
    }

    /// The next byte of the run, read off its front.
    pub(crate) fn pop_front(&mut self) -> Option<u8> {
        let (byte, count) = self.runs.first_mut()?;
        let byte = *byte;
        *count -= 1;
        if *count == 0 {
            self.runs.remove(0);
        }
        Some(byte)
    }
}

/// Every timed literal: an id, its plan and its text — `nested-decode-micro`'s
/// three, and a scalar beside them.
fn literals() -> Vec<(&'static str, Plan, String)> {
    vec![
        ("int8", Plan::int8(), "-1234567890".to_owned()),
        ("array_4", Plan::Array(Box::new(Plan::int4())), int_array_literal(4)),
        ("array_50", Plan::Array(Box::new(Plan::int4())), int_array_literal(50)),
        (
            "record_2",
            Plan::Record(vec![Plan::int4(), Plan::Text]),
            "(-1234567890,\"lorem ipsum dolor sit amet\")".to_owned(),
        ),
    ]
}

/// `benches/decoders.rs`'s literal of `n` constant-width `int4`s.
fn int_array_literal(n: usize) -> String {
    let mut out = String::from("{");
    for i in 0..n {
        if i > 0 {
            out.push(',');
        }
        out.push_str("-1234567890");
    }
    out.push('}');
    out
}

/// The feeds: the field as one run, and as one run a byte, every byte a
/// resumption — the bound from above on what a run boundary costs.
fn feeds(text: &str) -> [(&'static str, Vec<&[u8]>); 2] {
    let bytes = text.as_bytes();
    [("whole", vec![bytes]), ("bytes", bytes.chunks(1).collect())]
}

/// A form's reader: a field, fed as its runs, read under a plan.
type Reader = fn(&Plan, &[&[u8]]) -> Walked<Answer>;

/// Each form's reader, named as the bench ids name it.
fn reader(form: &str) -> Reader {
    match form {
        "pull" | "pull_again" => pull::read_field,
        "async" => walk::read_field,
        "push" => push::read_field,
        _ => unreachable!("no form {form}"),
    }
}

/// Each form's reading of a field fed as `runs`.
fn read(form: &str, plan: &Plan, runs: &[&[u8]]) -> Walked<Answer> {
    reader(form)(plan, runs)
}

const FORMS: [&str; 3] = ["pull", "async", "push"];

/// **Every form answers as the others do**, over every feed — and as the
/// literal's author says, for the hand-written cases — or the bench refuses
/// to time anything.
fn agreement() {
    let text = Plan::Text;
    let int4 = Plan::int4();
    let ints = Plan::Array(Box::new(Plan::int4()));
    let texts = Plan::Array(Box::new(Plan::Text));
    let pair = Plan::Record(vec![Plan::int4(), Plan::Text]);
    let answer = |leaves, nulls, sum, text_bytes| Ok(Answer { leaves, nulls, sum, text_bytes });
    let cases: Vec<(&Plan, &str, Walked<Answer>)> = vec![
        (&int4, " -12_3 ", answer(1, 0, -123, 0)),
        (&int4, "2147483648", Err(Refused)),
        (&int4, "-", Err(Refused)),
        (&int4, "1_", Err(Refused)),
        (&int4, "1 2", Err(Refused)),
        (&text, "abc", answer(1, 0, 0, 3)),
        (&ints, "{1,2,3}", answer(3, 0, 6, 0)),
        (&ints, " { 1 , 2 } ", answer(2, 0, 3, 0)),
        (&ints, "{{1,2},{3,4}}", answer(4, 0, 10, 0)),
        (&ints, "{{1,2},{3}}", Err(Refused)),
        (&ints, "{{1},2}", Err(Refused)),
        (&ints, "{1,,2}", Err(Refused)),
        (&ints, "{1 2}", Err(Refused)),
        (&ints, "{}", answer(0, 0, 0, 0)),
        (&ints, "{1}x", Err(Refused)),
        (&ints, "{NULL, null ,\"7\"}", answer(3, 2, 7, 0)),
        (&texts, "{a b , \"c\\\"d\" ,N\\ULL,NULLx,NULL y}", answer(5, 0, 0, 3 + 3 + 4 + 5 + 6)),
        (&texts, "{\"a\"b}", Err(Refused)),
        (&texts, "{a\"b}", Err(Refused)),
        (&texts, "{\"a}", Err(Refused)),
        (&texts, "{\t}", answer(0, 0, 0, 0)),
        (&pair, "( 1 ,x)", answer(2, 0, 1, 1)),
        (&pair, "(1,\"a\"\"b\"c)", answer(2, 0, 1, 4)),
        (&pair, "(,)", answer(2, 2, 0, 0)),
        (&pair, "(1)", Err(Refused)),
        (&pair, "(1,a,b)", Err(Refused)),
        (&pair, " (1,\"\") ", answer(2, 0, 1, 0)),
        (&pair, "(1,a", Err(Refused)),
    ];
    let deep = Plan::Record(vec![Plan::int4(), Plan::Array(Box::new(Plan::Text))]);
    let composites = Plan::Array(Box::new(Plan::Record(vec![Plan::int4(), Plan::Text])));
    let mut cases = cases;
    cases.push((&deep, "(1,\"{a,\\\"b c\\\"}\")", answer(3, 0, 1, 4)));
    cases.push((&deep, "(1,{x})", answer(2, 0, 1, 1)));
    cases.push((&deep, "(1,\"{a,{b}}\")", Err(Refused)));
    cases.push((&composites, "{\"(1,a)\",\"(2,bc)\"}", answer(4, 0, 3, 3)));
    cases.push((&composites, "{\"(1,a\"}", Err(Refused)));
    for (plan, literal, expected) in &cases {
        for (feed, runs) in feeds(literal) {
            for form in FORMS {
                let got = read(form, plan, &runs);
                assert_eq!(&got, expected, "{form} over {feed} runs of {literal:?}");
            }
        }
    }
    for (id, plan, literal) in literals() {
        let whole = read("pull", &plan, &[literal.as_bytes()]);
        assert!(whole.is_ok(), "{id} is refused");
        for (feed, runs) in feeds(&literal) {
            for form in FORMS {
                assert_eq!(read(form, &plan, &runs), whole, "{form} over {feed} runs of {id}");
            }
        }
    }
}

fn resumable(c: &mut Criterion) {
    agreement();
    let literals = literals();
    let mut g = c.benchmark_group("resumable");
    // `pull` timed first and again last: what separates two timings of one
    // reader in one sitting is the noise the others are read against.
    for form in ["pull", "async", "push", "pull_again"] {
        let read = reader(form);
        for (id, plan, literal) in &literals {
            for (feed, runs) in feeds(literal) {
                g.bench_function(format!("{id}/{form}/{feed}"), |b| {
                    b.iter(|| read(black_box(plan), black_box(&runs)))
                });
            }
        }
    }
    g.finish();
}

criterion_group!(benches, resumable);
criterion_main!(benches);
