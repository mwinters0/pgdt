//! `json_in`'s grammar, and with `jsonb` `jsonb_in`'s refusals beside it,
//! as a walk fed a byte at a time.

use crate::decode::{Read, Unread};
use crate::value::numeric::{NUMERIC_DSCALE_MAX, NUMERIC_INTEGER_DIGITS_MAX};
use crate::value::source::{Refused, Walked};

/// **A JSON document's walk**: whether `json_in` reads it, and with `jsonb`
/// whether `jsonb_in` does, as [`JsonCursor`](super::JsonCursor) reads it —
/// each of `jsonb_in`'s refusals counted only where the cursor would reach
/// it, before a value nested past [`JSONB_MAX_DEPTH`], which stops the
/// cursor where `json_in` reads on. No copy of the text is held and no byte
/// past the one it is fed is looked at (`docs/design/decisions.md`, "D143",
/// "D144").
///
/// Iterative, over a stack of the containers open, a bit each, so no depth
/// refuses: the server's recursion is bounded by its `max_stack_depth`, a
/// setting of the server restoring the dump (I72).
#[derive(Debug, Clone)]
pub(crate) struct JsonWalk {
    /// Whether `jsonb_in`'s refusals are read beside `json_in`'s grammar.
    jsonb: bool,
    /// Whether a value nested past [`JSONB_MAX_DEPTH`] has been reached,
    /// where [`JsonCursor`](super::JsonCursor) stops.
    deep: bool,
    /// The containers open around the value being read, innermost last, a
    /// bit each: set for an object.
    open: Vec<u64>,
    depth: usize,
    stage: Stage,
}

/// Where a [`JsonWalk`] stood at a mark ([`JsonWalk::mark`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct JsonMark {
    deep: bool,
    depth: usize,
    stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// A value expected, which the cursor counts its depth at.
    Value,
    /// Past an object's `{`: its `}`, or its first key.
    ObjectOpen,
    /// Past an array's `[`: its `]`, or its first value.
    ArrayOpen,
    /// Past a `,` in an object: its next key.
    Key,
    /// Past a key, its `:` owed; `opening` where it is its object's first.
    Colon {
        opening: bool,
    },
    /// Inside a string, a key's where `key` names its `opening`.
    String {
        key: Option<bool>,
        escape: Escape,
        high: bool,
    },
    Number(Number),
    /// A `true`, `false` or `null`, so many of its letters read.
    Keyword {
        word: &'static [u8],
        read: usize,
    },
    /// A value read: what closes or follows it.
    After,
}

/// Where a string stands inside an escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    Backslash,
    /// `\u` and so many hex digits read, their value so far.
    Unicode {
        digits: u8,
        code: u32,
    },
}

/// `json_lex_number`, `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`, a
/// byte at a time, with what [`numeric_stores`] reads of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Number {
    stage: NumberStage,
    /// The digits of its integer and fraction.
    count: usize,
    /// Where the first that is not a zero lies.
    first: Option<usize>,
    /// The integer part's digits.
    int: usize,
    negative_exponent: bool,
    exponent: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberStage {
    /// Past a `-`, a digit owed.
    Sign,
    /// A leading `0`, which no digit follows in the integer.
    Zero,
    Int,
    /// Past the `.`, a digit owed.
    Dot,
    Fraction,
    /// Past the `e`, a sign or digit owed.
    E,
    /// Past the exponent's sign, a digit owed.
    ExponentSign,
    Exponent,
}

impl Number {
    fn new() -> Self {
        Self {
            stage: NumberStage::Sign,
            count: 0,
            first: None,
            int: 0,
            negative_exponent: false,
            exponent: Some(0),
        }
    }

    fn digit(&mut self, digit: u8) {
        if digit != b'0' && self.first.is_none() {
            self.first = Some(self.count);
        }
        self.count += 1;
    }

    /// Read `byte`: `false` where it is the number's, `true` where the
    /// number ended before it, which is then the next token's.
    fn byte(&mut self, byte: u8) -> Walked<bool> {
        use NumberStage::*;
        self.stage = match (self.stage, byte) {
            (Sign, b'0') => {
                self.digit(byte);
                Zero
            }
            (Sign, b'1'..=b'9') | (Int, b'0'..=b'9') => {
                self.digit(byte);
                Int
            }
            (Zero | Int, b'.') => {
                self.int = self.count;
                Dot
            }
            (Zero | Int, b'e' | b'E') => {
                self.int = self.count;
                E
            }
            (Dot | Fraction, b'0'..=b'9') => {
                self.digit(byte);
                Fraction
            }
            (Fraction, b'e' | b'E') => E,
            (E, b'-') => {
                self.negative_exponent = true;
                ExponentSign
            }
            (E, b'+') => ExponentSign,
            (E | ExponentSign | Exponent, b'0'..=b'9') => {
                self.exponent = self
                    .exponent
                    .and_then(|e| e.checked_mul(10)?.checked_add(i64::from(byte - b'0')));
                Exponent
            }
            (Zero | Int | Fraction | Exponent, _) => return Ok(true),
            (Sign | Dot | E | ExponentSign, _) => return Err(Refused),
        };
        Ok(false)
    }

    /// The number has ended: whether `numeric_in` stores it, in a `jsonb`.
    /// A letter, digit or `_` right after it would join the token in the
    /// server's lexer and fail it; here it fails as the next token, which no
    /// position takes.
    fn stores(&self) -> Walked<bool> {
        let int = match self.stage {
            NumberStage::Zero | NumberStage::Int => self.count,
            NumberStage::Fraction | NumberStage::Exponent => self.int,
            _ => return Err(Refused),
        };
        let frac = self.count - int;
        let exponent = self.exponent.map(|e| if self.negative_exponent { -e } else { e });
        Ok(exponent.is_some_and(|e| numeric_stores(int, frac, e, self.first)))
    }
}

/// `json_lex`'s blanks.
fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

impl JsonWalk {
    /// A walk of `json_in`'s grammar, with `jsonb` `jsonb_in`'s refusals
    /// beside it.
    pub(crate) fn new(jsonb: bool) -> Self {
        Self { jsonb, deep: false, open: Vec::new(), depth: 0, stage: Stage::Value }
    }

    /// Ready the walk for another document.
    pub(crate) fn start(&mut self) {
        self.deep = false;
        self.depth = 0;
        self.stage = Stage::Value;
    }

    /// Where the walk stands, for [`JsonWalk::rewind`] to return to past a
    /// whitespace run: a blank opens and closes no container, and a byte it
    /// refuses closes none, so the containers open beneath `depth` are as
    /// the mark found them.
    pub(crate) fn mark(&self) -> JsonMark {
        JsonMark { deep: self.deep, depth: self.depth, stage: self.stage }
    }

    /// Return to `mark`.
    pub(crate) fn rewind(&mut self, mark: JsonMark) {
        self.deep = mark.deep;
        self.depth = mark.depth;
        self.stage = mark.stage;
    }

    /// Whether a refusal of `jsonb_in`'s own is read here: in a `jsonb`, as
    /// far as [`JsonCursor`](super::JsonCursor) reads.
    fn marks(&self) -> bool {
        self.jsonb && !self.deep
    }

    fn push(&mut self, object: bool) {
        let (word, bit) = (self.depth / 64, self.depth % 64);
        if word == self.open.len() {
            self.open.push(0);
        }
        self.open[word] = self.open[word] & !(1 << bit) | u64::from(object) << bit;
        self.depth += 1;
    }

    /// The innermost container open, `true` for an object.
    fn top(&self) -> Option<bool> {
        let at = self.depth.checked_sub(1)?;
        Some(self.open[at / 64] >> (at % 64) & 1 == 1)
    }

    /// Read the document's next byte.
    // pg-refuses: I72 — every refusal this walk makes is `json_in`'s, but those `jsonb`
    // adds, which are `jsonb_in`'s (I41, I63).
    pub(crate) fn byte(&mut self, byte: u8) -> Walked {
        self.stage = match self.stage {
            Stage::Value | Stage::ObjectOpen | Stage::Key | Stage::Colon { .. } | Stage::After
                if is_blank(byte) =>
            {
                self.stage
            }
            Stage::ArrayOpen if is_blank(byte) => Stage::ArrayOpen,
            Stage::Value => {
                if self.jsonb && self.depth >= JSONB_MAX_DEPTH {
                    self.deep = true;
                }
                match byte {
                    b'{' => Stage::ObjectOpen,
                    b'[' => Stage::ArrayOpen,
                    b'"' => Stage::String { key: None, escape: Escape::None, high: false },
                    b'-' => Stage::Number(Number::new()),
                    b'0'..=b'9' => {
                        let mut number = Number::new();
                        number.byte(byte)?;
                        Stage::Number(number)
                    }
                    b't' => Stage::Keyword { word: b"true", read: 1 },
                    b'f' => Stage::Keyword { word: b"false", read: 1 },
                    b'n' => Stage::Keyword { word: b"null", read: 1 },
                    _ => return Err(Refused),
                }
            }
            Stage::ObjectOpen => match byte {
                b'}' => Stage::After,
                b'"' => Stage::String { key: Some(true), escape: Escape::None, high: false },
                _ => return Err(Refused),
            },
            Stage::ArrayOpen if byte == b']' => Stage::After,
            Stage::ArrayOpen => {
                self.push(false);
                self.stage = Stage::Value;
                return self.byte(byte);
            }
            Stage::Key if byte == b'"' => {
                Stage::String { key: Some(false), escape: Escape::None, high: false }
            }
            Stage::Key => return Err(Refused),
            Stage::Colon { opening } if byte == b':' => {
                if opening {
                    self.push(true);
                }
                Stage::Value
            }
            Stage::Colon { .. } => return Err(Refused),
            Stage::String { key, escape, high } => self.string(key, escape, high, byte)?,
            Stage::Number(mut number) => {
                if !number.byte(byte)? {
                    Stage::Number(number)
                } else {
                    self.number_end(&number)?;
                    self.stage = Stage::After;
                    return self.byte(byte);
                }
            }
            Stage::Keyword { word, read } => {
                if word[read] != byte {
                    return Err(Refused);
                }
                if read + 1 == word.len() {
                    Stage::After
                } else {
                    Stage::Keyword { word, read: read + 1 }
                }
            }
            Stage::After => {
                let Some(object) = self.top() else { return Err(Refused) };
                match byte {
                    b',' if object => Stage::Key,
                    b',' => Stage::Value,
                    b'}' if object => {
                        self.depth -= 1;
                        Stage::After
                    }
                    b']' if !object => {
                        self.depth -= 1;
                        Stage::After
                    }
                    _ => return Err(Refused),
                }
            }
        };
        Ok(())
    }

    /// The document has ended: whether it was read whole, and with `jsonb`
    /// whether the cursor reads it, [`Unread::Unparsed`] past its depth.
    pub(crate) fn end(&mut self) -> Read<()> {
        let stage = std::mem::replace(&mut self.stage, Stage::Value);
        let read = match stage {
            Stage::After => true,
            Stage::Number(number) => self.number_end(&number).is_ok(),
            _ => false,
        };
        if !read || self.depth != 0 {
            return Err(Unread::Refused);
        }
        if self.deep { Err(Unread::Unparsed) } else { Ok(()) }
    }

    /// A number ended; in a `jsonb`, refused past `numeric`'s bounds.
    fn number_end(&self, number: &Number) -> Walked {
        let stores = number.stores()?;
        if !self.marks() || stores { Ok(()) } else { Err(Refused) }
    }

    /// `json_lex_string` without de-escaping; in a `jsonb`,
    /// [`JsonCursor::unicode_escape`](super::JsonCursor)'s refusals beside it.
    /// `high` says a high surrogate was read, which a low one must follow at
    /// once.
    fn string(&mut self, key: Option<bool>, escape: Escape, high: bool, byte: u8) -> Walked<Stage> {
        let string = |escape, high| Stage::String { key, escape, high };
        Ok(match escape {
            Escape::None => {
                if high && byte != b'\\' && self.marks() {
                    return Err(Refused);
                }
                match byte {
                    b'"' => match key {
                        Some(opening) => Stage::Colon { opening },
                        None => Stage::After,
                    },
                    b'\\' => string(Escape::Backslash, high),
                    0x00..=0x1f => return Err(Refused),
                    _ => string(Escape::None, high),
                }
            }
            Escape::Backslash => match byte {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                    if high && self.marks() {
                        return Err(Refused);
                    }
                    string(Escape::None, false)
                }
                b'u' => string(Escape::Unicode { digits: 0, code: 0 }, high),
                _ => return Err(Refused),
            },
            Escape::Unicode { digits, code } => {
                let digit = char::from(byte).to_digit(16).ok_or(Refused)?;
                let code = code << 4 | digit;
                if digits < 3 {
                    return Ok(string(Escape::Unicode { digits: digits + 1, code }, high));
                }
                let mut high = high;
                if self.marks() && !unicode_escape_read(code, &mut high) {
                    return Err(Refused);
                }
                string(Escape::None, high)
            }
        })
    }
}

/// How deep [`parse_jsonb`](super::parse_jsonb) will descend before reading no value
/// ([`Unread::Unparsed`]) where `json_in` reads the text; a fixed number
/// rather than a stack-depth check (`docs/design/decisions.md`, "D55").
/// Nothing a `jsonb_out` field of a real dump holds comes near it.
pub(super) const JSONB_MAX_DEPTH: usize = 1000;

/// The largest exponent `numeric_in` reads in a `jsonb` number, either sign:
/// `PG_INT32_MAX / 2` (I63).
const NUMERIC_EXPONENT_MAX: i64 = i32::MAX as i64 / 2;

/// Whether a `jsonb` reads a `\u` escape naming `code`, `high` saying a high
/// surrogate came just before it and set where `code` is one
/// ([`JsonCursor::unicode_escape`](super::JsonCursor)).
// pg-refuses: I41 — `\u0000`, and a surrogate out of its pair.
fn unicode_escape_read(code: u32, high: &mut bool) -> bool {
    let low = (0xdc00..0xe000).contains(&code);
    if std::mem::take(high) {
        return low;
    }
    *high = (0xd800..0xdc00).contains(&code);
    code != 0 && !low
}

/// **Whether `numeric_in` stores a JSON number** whose integer part has `int`
/// digits, its fraction `frac`, its exponent `exponent`, and `first` its first
/// digit that is not a zero, counted over both parts: the exponent and the
/// display scale it leaves within the storage format's, and the weight
/// counted from that first digit.
// pg-refuses: I63 — an exponent past `PG_INT32_MAX / 2`, or a display
// scale or a weight past the storage format's.
pub(super) fn numeric_stores(int: usize, frac: usize, exponent: i64, first: Option<usize>) -> bool {
    let (Ok(int), Ok(frac)) = (i64::try_from(int), i64::try_from(frac)) else { return false };
    if exponent.abs() > NUMERIC_EXPONENT_MAX || frac - exponent > NUMERIC_DSCALE_MAX as i64 {
        return false;
    }
    // A zero has no weight, so no weight bounds it.
    let Some(first) = first.and_then(|first| i64::try_from(first).ok()) else { return true };
    int + exponent - first <= NUMERIC_INTEGER_DIGITS_MAX as i64
}
