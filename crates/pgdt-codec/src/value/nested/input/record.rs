//! `record_in`'s grammar as a walk fed a byte at a time.

use super::quoted::{Lexed, QuotedRun};
use crate::value::nested::is_space;
use crate::value::source::{Parts, Refused, Walked};

/// **A `record_in` literal's walk, for a composite of `columns` declared
/// fields**, handing its caller every field with its position as it reads
/// it: what [`super::parse_record`] reads, holding none of them past its
/// turn and looking at nothing past the byte it is fed
/// (`docs/design/decisions.md`, "D143", "D144").
#[derive(Debug, Clone)]
pub(crate) struct RecordWalk {
    columns: usize,
    stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Before the opening parenthesis.
    Lead,
    /// At field `k`'s first byte, its separator read.
    FieldStart {
        k: usize,
    },
    InField {
        k: usize,
        run: QuotedRun,
    },
    /// A composite of no fields, its closing parenthesis owed.
    Close,
    /// Past the closing parenthesis.
    Trailing,
}

impl RecordWalk {
    pub(crate) fn new(columns: usize) -> Self {
        Self { columns, stage: Stage::Lead }
    }

    /// Ready the walk for another literal.
    pub(crate) fn start(&mut self) {
        self.stage = Stage::Lead;
    }

    /// Read the literal's next byte.
    // pg-refuses: I44 — every refusal this walk makes is `record_in`'s, arity included.
    pub(crate) fn byte(&mut self, byte: u8, parts: &mut dyn Parts<usize>) -> Walked {
        match self.stage {
            Stage::Lead | Stage::Trailing if is_space(byte) => Ok(()),
            Stage::Lead if byte == b'(' => {
                self.stage =
                    if self.columns == 0 { Stage::Close } else { Stage::FieldStart { k: 0 } };
                Ok(())
            }
            Stage::Close if byte == b')' => {
                self.stage = Stage::Trailing;
                Ok(())
            }
            Stage::Lead | Stage::Close | Stage::Trailing => Err(Refused),
            // Nothing at all between the separators is SQL NULL; `""` is the
            // empty string.
            Stage::FieldStart { k } if matches!(byte, b',' | b')') => {
                parts.null(k)?;
                self.separator(k, byte)
            }
            Stage::FieldStart { k } => {
                parts.begin(k)?;
                self.stage = Stage::InField { k, run: QuotedRun::default() };
                self.byte(byte, parts)
            }
            Stage::InField { k, mut run } => match run.byte(byte, b",)") {
                Lexed::Byte(byte) => {
                    self.stage = Stage::InField { k, run };
                    parts.byte(byte)
                }
                Lexed::Nothing => {
                    self.stage = Stage::InField { k, run };
                    Ok(())
                }
                Lexed::End => {
                    parts.end()?;
                    self.separator(k, byte)
                }
            },
        }
    }

    /// The literal has ended: whether it was read whole.
    pub(crate) fn end(&self) -> Walked {
        if self.stage == Stage::Trailing { Ok(()) } else { Err(Refused) }
    }

    /// Field `k` ended at `byte`, a separator or the closing parenthesis,
    /// which must be the one its position calls for.
    fn separator(&mut self, k: usize, byte: u8) -> Walked {
        let k = k + 1;
        self.stage = match byte {
            b',' if k < self.columns => Stage::FieldStart { k },
            b')' if k == self.columns => Stage::Trailing,
            _ => return Err(Refused),
        };
        Ok(())
    }
}
