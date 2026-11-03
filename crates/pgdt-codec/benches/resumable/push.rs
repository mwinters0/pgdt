//! **The push form: hand-written state machines**, fed a field's runs a byte
//! at a time, as PostgreSQL's incremental JSON parser is
//! (`src/common/jsonapi.c`, `pg_parse_json_incremental`). Each container is
//! a frame holding where its grammar stands and the slot its current part is
//! read into: a leaf in place, a nested container boxed, built once per
//! value and reset per part. A byte that ends a part is handed back to its
//! container, which reads it again in the stage the part's end left it in.
//! Nothing looks ahead: the `NULL` test is a stage holding the letters read,
//! a whitespace run is carried as [`Blanks`], and a `"` inside a record's
//! quotes waits for the byte after it.

use super::{Answer, Blanks, IntState, MAXDIM, Plan, Refused, Walked, is_space};

/// A field fed as `runs`, read under `plan`.
pub(crate) fn read_field(plan: &Plan, runs: &[&[u8]]) -> Walked<Answer> {
    let mut answer = Answer::default();
    let mut machine = Machine::of(plan);
    for run in runs {
        for &byte in *run {
            machine.byte(byte, &mut answer)?;
        }
    }
    machine.end(&mut answer)?;
    Ok(answer)
}

/// A leaf's reader.
enum Leaf {
    Int { int: IntState, min: i64, max: i64 },
    Text,
}

impl Leaf {
    fn start(&mut self) {
        if let Leaf::Int { int, .. } = self {
            *int = IntState::new();
        }
    }

    fn byte(&mut self, byte: u8, answer: &mut Answer) -> Walked {
        match self {
            Leaf::Int { int, .. } => int.byte(byte),
            Leaf::Text => {
                answer.text_bytes += 1;
                Ok(())
            }
        }
    }

    fn end(&mut self, answer: &mut Answer) -> Walked {
        if let Leaf::Int { int, min, max } = self {
            answer.sum = answer.sum.wrapping_add(int.end(*min, *max)?);
        }
        answer.leaves += 1;
        Ok(())
    }
}

/// **A value's reader**, from its first byte to its end.
enum Machine<'p> {
    Leaf(Leaf),
    Array(ArrayMachine<'p>),
    Record(RecordMachine<'p>),
}

impl<'p> Machine<'p> {
    fn of(plan: &'p Plan) -> Self {
        match plan {
            Plan::Int { min, max } => {
                Machine::Leaf(Leaf::Int { int: IntState::new(), min: *min, max: *max })
            }
            Plan::Text => Machine::Leaf(Leaf::Text),
            Plan::Array(element) => Machine::Array(ArrayMachine::new(element)),
            Plan::Record(fields) => Machine::Record(RecordMachine::new(fields)),
        }
    }

    fn start(&mut self) {
        match self {
            Machine::Leaf(leaf) => leaf.start(),
            Machine::Array(array) => array.start(),
            Machine::Record(record) => record.start(),
        }
    }

    fn byte(&mut self, byte: u8, answer: &mut Answer) -> Walked {
        match self {
            Machine::Leaf(leaf) => leaf.byte(byte, answer),
            Machine::Array(array) => array.byte(byte, answer),
            Machine::Record(record) => record.byte(byte, answer),
        }
    }

    fn end(&mut self, answer: &mut Answer) -> Walked {
        match self {
            Machine::Leaf(leaf) => leaf.end(answer),
            Machine::Array(array) => array.end(),
            Machine::Record(record) => record.end(),
        }
    }
}

/// Where a container reads its current part: a leaf in place, a nested
/// container boxed, the one indirection a recursive plan needs.
enum Slot<'p> {
    Leaf(Leaf),
    Nested(Box<Machine<'p>>),
}

impl<'p> Slot<'p> {
    fn of(plan: &'p Plan) -> Self {
        match Machine::of(plan) {
            Machine::Leaf(leaf) => Slot::Leaf(leaf),
            machine => Slot::Nested(Box::new(machine)),
        }
    }

    fn start(&mut self) {
        match self {
            Slot::Leaf(leaf) => leaf.start(),
            Slot::Nested(machine) => machine.start(),
        }
    }

    fn byte(&mut self, byte: u8, answer: &mut Answer) -> Walked {
        match self {
            Slot::Leaf(leaf) => leaf.byte(byte, answer),
            Slot::Nested(machine) => machine.byte(byte, answer),
        }
    }

    fn end(&mut self, answer: &mut Answer) -> Walked {
        match self {
            Slot::Leaf(leaf) => leaf.end(answer),
            Slot::Nested(machine) => machine.end(answer),
        }
    }
}

fn null(answer: &mut Answer) {
    answer.leaves += 1;
    answer.nulls += 1;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArrayStage {
    /// Before the opening brace.
    Lead,
    /// Between elements and braces.
    Body,
    Quoted {
        escaped: bool,
    },
    /// Past a quoted element's closing quote.
    AfterQuoted,
    /// Reading an unquoted element's first letters as `NULL`'s, so many read.
    Null {
        letters: usize,
    },
    /// `NULL` read, a whitespace run after it carried.
    NullBlanks,
    Unquoted {
        escaped: bool,
    },
    /// Past the closing brace.
    Trailing,
}

/// **An `array_in` literal's reader**: `array_body`'s state, and an
/// element's.
struct ArrayMachine<'p> {
    element: Slot<'p>,
    stage: ArrayStage,
    ndim: usize,
    frozen: bool,
    expect_delim: bool,
    nest_level: usize,
    nelems: [usize; MAXDIM],
    dim: [Option<usize>; MAXDIM],
    count: usize,
    held: [u8; 4],
    blanks: Blanks,
}

const DELIM: u8 = b',';

impl<'p> ArrayMachine<'p> {
    fn new(element: &'p Plan) -> Self {
        Self {
            element: Slot::of(element),
            stage: ArrayStage::Lead,
            ndim: 0,
            frozen: false,
            expect_delim: false,
            nest_level: 0,
            nelems: [0; MAXDIM],
            dim: [None; MAXDIM],
            count: 0,
            held: [0; 4],
            blanks: Blanks::default(),
        }
    }

    fn start(&mut self) {
        self.stage = ArrayStage::Lead;
        self.ndim = 0;
        self.frozen = false;
        self.expect_delim = false;
        self.nest_level = 0;
        self.dim = [None; MAXDIM];
        self.count = 0;
        self.blanks.clear();
    }

    fn terminates(byte: u8) -> bool {
        byte == DELIM || byte == b'}'
    }

    /// An element read: `array_body`'s bookkeeping after it.
    fn element_done(&mut self) -> Walked {
        self.frozen = true;
        if self.nest_level != self.ndim {
            return Err(Refused);
        }
        self.nelems[self.nest_level - 1] += 1;
        self.count += 1;
        self.expect_delim = true;
        self.stage = ArrayStage::Body;
        Ok(())
    }

    /// An unquoted element begun: the letters the `NULL` test held, and
    /// the run after them where it read one, handed to its reader.
    fn begin_unquoted(&mut self, letters: usize, answer: &mut Answer) -> Walked {
        self.element.start();
        for k in 0..letters {
            self.element.byte(self.held[k], answer)?;
        }
        self.flush_blanks(answer)?;
        self.stage = ArrayStage::Unquoted { escaped: false };
        Ok(())
    }

    /// A carried run handed on, being interior.
    fn flush_blanks(&mut self, answer: &mut Answer) -> Walked {
        while let Some(byte) = self.blanks.pop_front() {
            self.element.byte(byte, answer)?;
        }
        Ok(())
    }

    fn body(&mut self, byte: u8, answer: &mut Answer) -> Walked {
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
                    self.closed()?;
                }
            }
            DELIM => {
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
                    self.element.start();
                    self.stage = ArrayStage::Quoted { escaped: false };
                } else if first.eq_ignore_ascii_case(&b'N') {
                    self.held[0] = first;
                    self.stage = ArrayStage::Null { letters: 1 };
                } else {
                    self.begin_unquoted(0, answer)?;
                    return self.byte(first, answer);
                }
            }
        }
        Ok(())
    }

    /// The outermost brace closed: `walk_array`'s shape check.
    fn closed(&mut self) -> Walked {
        self.stage = ArrayStage::Trailing;
        if self.count == 0 {
            return Ok(());
        }
        let mut product = 1usize;
        for dim in &self.dim[..self.ndim] {
            product *= dim.ok_or(Refused)?;
        }
        if product == self.count { Ok(()) } else { Err(Refused) }
    }

    fn byte(&mut self, byte: u8, answer: &mut Answer) -> Walked {
        match self.stage {
            ArrayStage::Lead if is_space(byte) => Ok(()),
            ArrayStage::Lead if byte == b'{' => {
                self.stage = ArrayStage::Body;
                self.body(byte, answer)
            }
            ArrayStage::Lead | ArrayStage::Trailing if !is_space(byte) => Err(Refused),
            ArrayStage::Lead | ArrayStage::Trailing => Ok(()),
            ArrayStage::Body => self.body(byte, answer),
            ArrayStage::Quoted { escaped: true } => {
                self.stage = ArrayStage::Quoted { escaped: false };
                self.element.byte(byte, answer)
            }
            ArrayStage::Quoted { escaped: false } => match byte {
                b'\\' => {
                    self.stage = ArrayStage::Quoted { escaped: true };
                    Ok(())
                }
                b'"' => {
                    self.stage = ArrayStage::AfterQuoted;
                    self.element.end(answer)
                }
                byte => self.element.byte(byte, answer),
            },
            ArrayStage::AfterQuoted => match byte {
                byte if is_space(byte) => Ok(()),
                b'{' | b'}' | DELIM => {
                    self.element_done()?;
                    self.body(byte, answer)
                }
                _ => Err(Refused),
            },
            ArrayStage::Null { letters } if byte.eq_ignore_ascii_case(&b"NULL"[letters]) => {
                self.held[letters] = byte;
                self.stage = if letters == 3 {
                    ArrayStage::NullBlanks
                } else {
                    ArrayStage::Null { letters: letters + 1 }
                };
                Ok(())
            }
            ArrayStage::Null { letters } => {
                self.begin_unquoted(letters, answer)?;
                self.byte(byte, answer)
            }
            ArrayStage::NullBlanks if is_space(byte) => {
                self.blanks.push(byte);
                Ok(())
            }
            ArrayStage::NullBlanks if Self::terminates(byte) => {
                self.blanks.clear();
                null(answer);
                self.element_done()?;
                self.body(byte, answer)
            }
            ArrayStage::NullBlanks => {
                self.begin_unquoted(4, answer)?;
                self.byte(byte, answer)
            }
            ArrayStage::Unquoted { escaped: true } => {
                self.stage = ArrayStage::Unquoted { escaped: false };
                self.element.byte(byte, answer)
            }
            ArrayStage::Unquoted { escaped: false } => match byte {
                byte if Self::terminates(byte) => {
                    self.blanks.clear();
                    self.element.end(answer)?;
                    self.element_done()?;
                    self.body(byte, answer)
                }
                byte if is_space(byte) => {
                    self.blanks.push(byte);
                    Ok(())
                }
                b'{' | b'"' => Err(Refused),
                b'\\' => {
                    self.flush_blanks(answer)?;
                    self.stage = ArrayStage::Unquoted { escaped: true };
                    Ok(())
                }
                byte => {
                    self.flush_blanks(answer)?;
                    self.element.byte(byte, answer)
                }
            },
        }
    }

    fn end(&mut self) -> Walked {
        if self.stage == ArrayStage::Trailing { Ok(()) } else { Err(Refused) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordStage {
    Lead,
    /// At field `k`'s first byte, its separator read.
    FieldStart,
    InField {
        quoted: bool,
        escaped: bool,
        quote: bool,
    },
    /// A composite of no fields, its closing parenthesis owed.
    Close,
    /// Past the closing parenthesis.
    Trailing,
}

/// **A `record_in` literal's reader**: the field it stands in, and that
/// field's lexing.
struct RecordMachine<'p> {
    fields: &'p [Plan],
    k: usize,
    field: Option<Slot<'p>>,
    stage: RecordStage,
}

impl<'p> RecordMachine<'p> {
    fn new(fields: &'p [Plan]) -> Self {
        Self { fields, k: 0, field: None, stage: RecordStage::Lead }
    }

    fn start(&mut self) {
        self.k = 0;
        self.field = None;
        self.stage = RecordStage::Lead;
    }

    /// Field `k` ended at `byte`, a separator or the closing parenthesis,
    /// which must be the one its position calls for.
    fn separator(&mut self, byte: u8) -> Walked {
        self.k += 1;
        self.stage = match byte {
            b',' if self.k < self.fields.len() => RecordStage::FieldStart,
            b')' if self.k == self.fields.len() => RecordStage::Trailing,
            _ => return Err(Refused),
        };
        Ok(())
    }

    fn byte(&mut self, byte: u8, answer: &mut Answer) -> Walked {
        match self.stage {
            RecordStage::Lead if is_space(byte) => Ok(()),
            RecordStage::Lead if byte == b'(' => {
                self.stage = if self.fields.is_empty() {
                    RecordStage::Close
                } else {
                    RecordStage::FieldStart
                };
                Ok(())
            }
            RecordStage::Close if byte == b')' => {
                self.stage = RecordStage::Trailing;
                Ok(())
            }
            RecordStage::Close => Err(Refused),
            RecordStage::Lead | RecordStage::Trailing if !is_space(byte) => Err(Refused),
            RecordStage::Lead | RecordStage::Trailing => Ok(()),
            RecordStage::FieldStart if matches!(byte, b',' | b')') => {
                null(answer);
                self.separator(byte)
            }
            RecordStage::FieldStart => {
                let mut slot = Slot::of(&self.fields[self.k]);
                slot.start();
                self.field = Some(slot);
                self.stage = RecordStage::InField { quoted: false, escaped: false, quote: false };
                self.byte(byte, answer)
            }
            RecordStage::InField { quoted, escaped, quote } => {
                let field = self.field.as_mut().ok_or(Refused)?;
                if escaped {
                    self.stage = RecordStage::InField { quoted, escaped: false, quote };
                    return field.byte(byte, answer);
                }
                if quote {
                    if byte == b'"' {
                        self.stage = RecordStage::InField { quoted, escaped, quote: false };
                        return field.byte(b'"', answer);
                    }
                    self.stage = RecordStage::InField { quoted: false, escaped, quote: false };
                    return self.byte(byte, answer);
                }
                match byte {
                    b',' | b')' if !quoted => {
                        field.end(answer)?;
                        self.separator(byte)
                    }
                    b'\\' => {
                        self.stage = RecordStage::InField { quoted, escaped: true, quote };
                        Ok(())
                    }
                    b'"' => {
                        self.stage = RecordStage::InField { quoted: true, escaped, quote: quoted };
                        Ok(())
                    }
                    byte => field.byte(byte, answer),
                }
            }
        }
    }

    fn end(&mut self) -> Walked {
        if self.stage == RecordStage::Trailing { Ok(()) } else { Err(Refused) }
    }
}
