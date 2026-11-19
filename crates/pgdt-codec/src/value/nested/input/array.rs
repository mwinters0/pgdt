//! `array_in`'s grammar as a walk fed a byte at a time.

use crate::value::nested::is_space;
use crate::value::source::{Parts, Refused, Walked, Word};

/// PostgreSQL's `MAXDIM`: the most dimensions an array value may have, which
/// the output walks bound their brace depth by too.
/// `pgdt_model::PG_ARRAY_MAX_DIMS` is the same number for the shape census;
/// it is a `u8` there and this is an index bound, and neither module is worth
/// a dependency edge on the other for one constant.
pub(in crate::value::nested) const MAXDIM: usize = 6;

/// An `array_in` literal's shape: one entry per dimension in each, both
/// empty for the zero-dimensional empty array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArrayShape {
    pub(crate) dims: Vec<usize>,
    pub(crate) lower_bounds: Vec<i32>,
}

/// **An `array_in` literal's walk**, its element type's `typdelim` being
/// `delim`, handing its caller every element in row-major order as it reads
/// it: what [`super::parse_array_delimited`] reads, with no element held
/// past its turn and nothing looked at past the byte it is fed
/// (`docs/design/decisions.md`, "D143", "D144"). A whitespace run whose end
/// decides whether it is an element's is fed on into the element and
/// rewound where it is not ([`Parts::mark`], "D145"), so the walk's state is
/// fixed.
#[derive(Debug, Clone)]
pub(crate) struct ArrayWalk {
    delim: u8,
    stage: Stage,
    /// Each dimension's length, declared by an `[m:n]=` decoration or found
    /// by the braces — the same array serving as both, which is what lets
    /// one walk check a declared shape and deduce an undeclared one.
    dim: [Option<usize>; MAXDIM],
    lower: [i32; MAXDIM],
    /// The dimensions an `[m:n]=` decoration declared.
    declared: usize,
    /// The dimensionality, `0` until a decoration or a brace says.
    ndim: usize,
    /// Once a dimensionality is declared, or an element has been seen, the
    /// nesting may not get deeper.
    frozen: bool,
    expect_delim: bool,
    nest_level: usize,
    nelems: [usize; MAXDIM],
    count: usize,
    /// An unquoted element's first letters, read as `NULL`'s.
    null: Word,
    held: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Before the decoration or the opening brace.
    Lead,
    /// Inside an `[m:n]` item: its upper bound where `upper`, and the
    /// number so far.
    Bound {
        upper: bool,
        number: Number,
        first: i32,
    },
    /// Past a decoration's `=`.
    Equals,
    /// Between elements and braces.
    Body,
    Quoted {
        escaped: bool,
    },
    /// Past a quoted element's closing quote.
    AfterQuoted,
    /// An unquoted element's first letters, so far `NULL`'s.
    Null,
    /// `NULL` read, and nothing after it.
    NullRead,
    /// `NULL` read and a whitespace run after it, fed on into an element
    /// begun past a mark, which the run met a refusal in where `refused`.
    NullBlanks {
        refused: bool,
    },
    /// An unquoted element, `run` naming a whitespace run it is in, fed on
    /// past a mark, by whether a refusal was met in it: a refusal that
    /// stands only where the run turns out interior.
    Unquoted {
        run: Option<bool>,
    },
    /// An unquoted element, past a `\`.
    UnquotedEscaped,
    /// Past the closing brace.
    Trailing,
}

/// One `[m:n]` bound. `ReadDimensionInt` refuses leading whitespace and
/// then hands the rest to `strtol`, so a sign is accepted and a bare sign
/// is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Number {
    Start,
    Signed {
        negative: bool,
    },
    /// As many digits as `strtol` reads, leading zeros among them, the
    /// magnitude held at one past `i32`'s.
    Digits {
        negative: bool,
        magnitude: i64,
    },
}

impl Number {
    const LIMIT: i64 = i32::MAX as i64 + 1;

    /// Read `byte`: `Ok(None)` where it is the number's, `Ok(Some(value))`
    /// where it ends the number, which is then `byte`'s to follow.
    fn byte(&mut self, byte: u8) -> Walked<Option<i32>> {
        *self = match (*self, byte) {
            (Number::Start, b'-') => Number::Signed { negative: true },
            (Number::Start, b'+') => Number::Signed { negative: false },
            (Number::Start, b'0'..=b'9') => {
                Number::Digits { negative: false, magnitude: i64::from(byte - b'0') }
            }
            (Number::Signed { negative }, b'0'..=b'9') => {
                Number::Digits { negative, magnitude: i64::from(byte - b'0') }
            }
            (Number::Digits { negative, magnitude }, b'0'..=b'9') => Number::Digits {
                negative,
                magnitude: (magnitude * 10 + i64::from(byte - b'0')).min(Self::LIMIT + 1),
            },
            (Number::Digits { negative, magnitude }, _) => {
                let value = if negative { -magnitude } else { magnitude };
                return i32::try_from(value).map(Some).map_err(|_| Refused);
            }
            _ => return Err(Refused),
        };
        Ok(None)
    }
}

impl ArrayWalk {
    pub(crate) fn new(delim: u8) -> Self {
        Self {
            delim,
            stage: Stage::Lead,
            dim: [None; MAXDIM],
            lower: [1; MAXDIM],
            declared: 0,
            ndim: 0,
            frozen: false,
            expect_delim: false,
            nest_level: 0,
            nelems: [0; MAXDIM],
            count: 0,
            null: Word::new(b"NULL"),
            held: [0; 4],
        }
    }

    /// Ready the walk for another literal.
    pub(crate) fn start(&mut self) {
        *self = Self::new(self.delim);
    }

    fn terminates(&self, byte: u8) -> bool {
        byte == self.delim || byte == b'}'
    }

    /// Read the literal's next byte.
    // pg-refuses: I44 — every refusal this walk makes is `array_in`'s.
    pub(crate) fn byte(&mut self, byte: u8, parts: &mut dyn Parts) -> Walked {
        match self.stage {
            Stage::Lead | Stage::Equals if is_space(byte) => Ok(()),
            Stage::Lead if byte == b'[' => {
                if self.declared >= MAXDIM {
                    return Err(Refused);
                }
                self.stage = Stage::Bound { upper: false, number: Number::Start, first: 1 };
                Ok(())
            }
            Stage::Lead if self.declared > 0 && byte == b'=' => {
                self.stage = Stage::Equals;
                Ok(())
            }
            Stage::Lead if self.declared == 0 && byte == b'{' => self.open(byte, parts),
            Stage::Equals if byte == b'{' => self.open(byte, parts),
            Stage::Lead | Stage::Equals => Err(Refused),
            Stage::Bound { upper, mut number, first } => match number.byte(byte)? {
                None => {
                    self.stage = Stage::Bound { upper, number, first };
                    Ok(())
                }
                Some(value) => self.bound(upper, first, value, byte),
            },
            Stage::Body => self.body(byte, parts),
            Stage::Quoted { escaped: true } => {
                self.stage = Stage::Quoted { escaped: false };
                parts.byte(byte)
            }
            Stage::Quoted { escaped: false } => match byte {
                b'\\' => {
                    self.stage = Stage::Quoted { escaped: true };
                    Ok(())
                }
                b'"' => {
                    self.stage = Stage::AfterQuoted;
                    parts.end()
                }
                byte => parts.byte(byte),
            },
            // Nothing but whitespace may follow the closing quote before a
            // separator or a brace: `{"a"b}` is "incorrectly quoted", not a
            // two-part element.
            Stage::AfterQuoted if is_space(byte) => Ok(()),
            Stage::AfterQuoted if byte == b'{' || self.terminates(byte) => {
                self.element_done()?;
                self.body(byte, parts)
            }
            Stage::AfterQuoted => Err(Refused),
            Stage::Null if self.null.letter(byte) => {
                self.held[self.null.read() - 1] = byte;
                if self.null.done() {
                    self.stage = Stage::NullRead;
                }
                Ok(())
            }
            // The `NULL` marker: those four letters in any case, no escape
            // among them, so `{N\ULL}` is the *string* `NULL`, and nothing but
            // whitespace after them.
            Stage::NullRead if self.terminates(byte) => self.null_element(byte, parts),
            // Whether a run after `NULL` leaves it the marker only the byte
            // after the run says, so the run is fed on into an element begun
            // past a mark, which the marker rewinds.
            Stage::NullRead if is_space(byte) => {
                parts.mark();
                let refused = self.begin_unquoted(parts).and_then(|()| parts.byte(byte)).is_err();
                self.stage = Stage::NullBlanks { refused };
                Ok(())
            }
            Stage::Null | Stage::NullRead => {
                self.begin_unquoted(parts)?;
                self.byte(byte, parts)
            }
            Stage::NullBlanks { refused } if is_space(byte) => {
                let refused = refused || parts.byte(byte).is_err();
                self.stage = Stage::NullBlanks { refused };
                Ok(())
            }
            Stage::NullBlanks { .. } if self.terminates(byte) => {
                parts.rewind();
                self.null_element(byte, parts)
            }
            Stage::NullBlanks { refused } => {
                self.stage = Stage::Unquoted { run: Some(refused) };
                self.byte(byte, parts)
            }
            Stage::UnquotedEscaped => {
                self.stage = Stage::Unquoted { run: None };
                parts.byte(byte)
            }
            // Trailing whitespace is dropped and interior whitespace kept — a
            // run is interior where something but a separator or a brace
            // follows it — and a `\` escape counts as non-whitespace. A run
            // is fed on into the element as it comes, from a mark the
            // separator or brace rewinds to, and a refusal met in it stands
            // only where it turns out interior: `{1\v}` is a `json[]`, whose
            // grammar refuses a `\v` that `array_in` trims.
            Stage::Unquoted { run } => match byte {
                byte if self.terminates(byte) => {
                    if run.is_some() {
                        parts.rewind();
                    }
                    parts.end()?;
                    self.element_done()?;
                    self.body(byte, parts)
                }
                byte if is_space(byte) => {
                    let refused = match run {
                        None => {
                            parts.mark();
                            parts.byte(byte).is_err()
                        }
                        Some(refused) => refused || parts.byte(byte).is_err(),
                    };
                    self.stage = Stage::Unquoted { run: Some(refused) };
                    Ok(())
                }
                // An element is quoted in full or not at all, and a `{` here
                // is a brace where a value was expected.
                b'{' | b'"' => Err(Refused),
                _ if run == Some(true) => Err(Refused),
                b'\\' => {
                    self.stage = Stage::UnquotedEscaped;
                    Ok(())
                }
                byte => {
                    self.stage = Stage::Unquoted { run: None };
                    parts.byte(byte)
                }
            },
            Stage::Trailing if is_space(byte) => Ok(()),
            Stage::Trailing => Err(Refused),
        }
    }

    /// The literal has ended: whether it was read whole and its shape holds.
    pub(crate) fn end(&mut self) -> Walked {
        if self.stage != Stage::Trailing {
            return Err(Refused);
        }
        if self.count == 0 {
            return Ok(());
        }
        let mut product = 1usize;
        for dim in &self.dim[..self.ndim] {
            product = product.checked_mul(dim.ok_or(Refused)?).ok_or(Refused)?;
        }
        if product == self.count { Ok(()) } else { Err(Refused) }
    }

    /// The shape [`ArrayWalk::end`] read: both empty for the
    /// zero-dimensional empty array, which any literal with no elements is.
    pub(crate) fn shape(&self) -> ArrayShape {
        if self.count == 0 {
            return ArrayShape { dims: Vec::new(), lower_bounds: Vec::new() };
        }
        ArrayShape {
            dims: self.dim[..self.ndim].iter().map(|dim| dim.unwrap_or(0)).collect(),
            lower_bounds: self.lower[..self.ndim].to_vec(),
        }
    }

    /// An `[m:n]` item's bound read as `value`, `byte` after it.
    fn bound(&mut self, upper: bool, first: i32, value: i32, byte: u8) -> Walked {
        match (upper, byte) {
            (false, b':') => {
                self.stage = Stage::Bound { upper: true, number: Number::Start, first: value };
                return Ok(());
            }
            (_, b']') => {}
            _ => return Err(Refused),
        }
        let (lb, ub) = if upper { (first, value) } else { (1, value) };
        // A zero-length dimension is refused rather than collapsed, and an
        // upper bound of `INT_MAX` is refused outright.
        if ub < lb || ub == i32::MAX {
            return Err(Refused);
        }
        self.dim[self.declared] =
            Some(usize::try_from(i64::from(ub) - i64::from(lb) + 1).map_err(|_| Refused)?);
        self.lower[self.declared] = lb;
        self.declared += 1;
        self.ndim = self.declared;
        self.frozen = true;
        self.stage = Stage::Lead;
        Ok(())
    }

    /// The opening brace, `byte`.
    fn open(&mut self, byte: u8, parts: &mut dyn Parts) -> Walked {
        self.stage = Stage::Body;
        self.body(byte, parts)
    }

    /// An element read: the brace structure's bookkeeping after it. Every
    /// element sits at the same depth, or the literal is ragged.
    fn element_done(&mut self) -> Walked {
        self.frozen = true;
        if self.nest_level != self.ndim {
            return Err(Refused);
        }
        self.nelems[self.nest_level - 1] += 1;
        self.count += 1;
        self.expect_delim = true;
        self.stage = Stage::Body;
        Ok(())
    }

    /// An unquoted element begun: the letters the `NULL` test held handed
    /// on.
    fn begin_unquoted(&mut self, parts: &mut dyn Parts) -> Walked {
        let read = self.null.read();
        self.null = Word::new(b"NULL");
        self.stage = Stage::Unquoted { run: None };
        parts.begin(())?;
        self.held[..read].iter().try_for_each(|&byte| parts.byte(byte))
    }

    /// The `NULL` marker read, `byte` the separator or brace after it.
    fn null_element(&mut self, byte: u8, parts: &mut dyn Parts) -> Walked {
        self.null = Word::new(b"NULL");
        parts.null(())?;
        self.element_done()?;
        self.body(byte, parts)
    }

    /// The brace structure, as `ReadArrayToken` classifies its tokens.
    fn body(&mut self, byte: u8, parts: &mut dyn Parts) -> Walked {
        match byte {
            byte if is_space(byte) => {}
            b'{' => {
                if self.expect_delim || self.nest_level >= MAXDIM {
                    return Err(Refused);
                }
                self.nelems[self.nest_level] = 0;
                self.nest_level += 1;
                if self.nest_level > self.ndim {
                    if self.frozen {
                        return Err(Refused);
                    }
                    self.ndim = self.nest_level;
                }
            }
            b'}' => {
                if self.nest_level == 0 {
                    return Err(Refused);
                }
                // A `}` may close an empty sub-array; otherwise it stands
                // where a separator would have.
                if self.nelems[self.nest_level - 1] > 0 && !self.expect_delim {
                    return Err(Refused);
                }
                self.nest_level -= 1;
                let level = self.nest_level;
                if level > 0 {
                    self.nelems[level - 1] += 1;
                }
                match self.dim[level] {
                    None => self.dim[level] = Some(self.nelems[level]),
                    Some(n) if n == self.nelems[level] => {}
                    Some(_) => return Err(Refused),
                }
                self.expect_delim = true;
                if level == 0 {
                    self.stage = Stage::Trailing;
                }
            }
            byte if byte == self.delim => {
                if !self.expect_delim {
                    return Err(Refused);
                }
                self.expect_delim = false;
            }
            first => {
                if self.expect_delim || self.nest_level == 0 {
                    return Err(Refused);
                }
                if first == b'"' {
                    parts.begin(())?;
                    self.stage = Stage::Quoted { escaped: false };
                } else if self.null.letter(first) {
                    self.held[0] = first;
                    self.stage = Stage::Null;
                } else {
                    self.begin_unquoted(parts)?;
                    return self.byte(first, parts);
                }
            }
        }
        Ok(())
    }
}
