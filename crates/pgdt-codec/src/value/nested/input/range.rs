//! `range_in`'s and `multirange_in`'s grammars as walks fed a byte at a
//! time.

use super::quoted::{Lexed, QuotedRun};
use crate::value::nested::{RangeLiteral, is_space};
use crate::value::source::{Refused, Walked, Word};

/// **A `range_in` literal's walk**, its bounds collected, since `range_in`
/// compares them: [`super::parse_range`]'s reading, looking at nothing past
/// the byte it is fed (`docs/design/decisions.md`, "D144"). `empty` is
/// matched case-insensitively and may carry whitespace on either side; the
/// bounds keep theirs.
#[derive(Debug, Clone)]
pub(crate) struct RangeWalk {
    stage: Stage,
    literal: RangeLiteral,
    /// The bound being read, its bytes so far.
    bound: Vec<u8>,
}

/// Where a [`RangeWalk`] stood at a mark ([`RangeWalk::mark`]): its stage,
/// and how much of the bound it was reading it had read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RangeMark {
    stage: Stage,
    bound: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Lead,
    /// Letters read of `empty`.
    Empty(Word),
    /// `empty` read.
    EmptyRead,
    /// At a bound's first byte, the upper where `upper`.
    BoundStart {
        upper: bool,
    },
    InBound {
        upper: bool,
        run: QuotedRun,
    },
    /// Past the closing bracket.
    Trailing,
}

impl Default for RangeWalk {
    fn default() -> Self {
        Self { stage: Stage::Lead, literal: RangeLiteral::empty(), bound: Vec::new() }
    }
}

impl RangeWalk {
    /// Ready the walk for another literal.
    pub(crate) fn start(&mut self) {
        self.stage = Stage::Lead;
        self.bound.clear();
    }

    /// Where the walk stands, for [`RangeWalk::rewind`] to return to past a
    /// whitespace run, which ends no bound and starts none of the literal's
    /// own: a bound's bytes, or skipped.
    pub(crate) fn mark(&self) -> RangeMark {
        RangeMark { stage: self.stage, bound: self.bound.len() }
    }

    /// Return to `mark`.
    pub(crate) fn rewind(&mut self, mark: RangeMark) {
        self.stage = mark.stage;
        self.bound.truncate(mark.bound);
    }

    /// Read the literal's next byte.
    // pg-refuses: I44 — every refusal this walk makes is `range_in`'s grammar's.
    pub(crate) fn byte(&mut self, byte: u8) -> Walked {
        match self.stage {
            Stage::Lead | Stage::EmptyRead | Stage::Trailing if is_space(byte) => Ok(()),
            Stage::Lead => {
                self.stage = match byte {
                    b'[' | b'(' => {
                        self.literal = RangeLiteral {
                            empty: false,
                            lower: None,
                            upper: None,
                            lower_inclusive: byte == b'[',
                            upper_inclusive: false,
                        };
                        Stage::BoundStart { upper: false }
                    }
                    _ => {
                        let mut word = Word::new(b"empty");
                        if !word.letter(byte) {
                            return Err(Refused);
                        }
                        Stage::Empty(word)
                    }
                };
                Ok(())
            }
            Stage::Empty(mut word) => {
                if !word.letter(byte) {
                    return Err(Refused);
                }
                self.stage = if word.done() {
                    self.literal = RangeLiteral::empty();
                    Stage::EmptyRead
                } else {
                    Stage::Empty(word)
                };
                Ok(())
            }
            Stage::EmptyRead | Stage::Trailing => Err(Refused),
            // Completely empty input is the unbounded bound, which is what
            // makes `(,a)` and `["",a)` different values.
            Stage::BoundStart { upper } if matches!(byte, b',' | b')' | b']') => {
                self.bound_end(upper, None, byte)
            }
            Stage::BoundStart { upper } => {
                self.bound.clear();
                self.stage = Stage::InBound { upper, run: QuotedRun::default() };
                self.byte(byte)
            }
            Stage::InBound { upper, mut run } => match run.byte(byte, b",)]") {
                Lexed::Byte(byte) => {
                    self.bound.push(byte);
                    self.stage = Stage::InBound { upper, run };
                    Ok(())
                }
                Lexed::Nothing => {
                    self.stage = Stage::InBound { upper, run };
                    Ok(())
                }
                Lexed::End => {
                    let text =
                        String::from_utf8(std::mem::take(&mut self.bound)).map_err(|_| Refused)?;
                    self.bound_end(upper, Some(text), byte)
                }
            },
        }
    }

    /// The literal has ended: what it read.
    pub(crate) fn end(&mut self) -> Walked<RangeLiteral> {
        match self.stage {
            Stage::EmptyRead | Stage::Trailing => {
                self.stage = Stage::Lead;
                Ok(std::mem::replace(&mut self.literal, RangeLiteral::empty()))
            }
            _ => Err(Refused),
        }
    }

    /// A bound read as `text`, ended at `byte`: a lower bound's at the `,`
    /// the upper's follows, the upper's at the closing bracket.
    fn bound_end(&mut self, upper: bool, text: Option<String>, byte: u8) -> Walked {
        self.stage = match (upper, byte) {
            (false, b',') => {
                self.literal.lower = text;
                Stage::BoundStart { upper: true }
            }
            (true, b']' | b')') => {
                self.literal.upper = text;
                self.literal.upper_inclusive = byte == b']';
                Stage::Trailing
            }
            _ => return Err(Refused),
        };
        Ok(())
    }
}

/// Where a `multirange_in` scan stands inside one member, which it reads as
/// a whole to hand to `range_in`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Member {
    /// Its opening bracket not yet read.
    Opening,
    InRange,
    InRangeEscaped,
    InRangeQuoted,
    InRangeQuotedEscaped,
    /// Inside quotes, a `"` read, whose meaning the byte after it decides.
    InRangeQuotedQuote,
    /// Its closing bracket read.
    Closed,
}

impl Member {
    /// The state past `byte`, as `multirange_in` finds the substring it
    /// hands to `range_in`: whitespace is skipped in every state, member text
    /// included — it is passed on all the same — so a member's own brackets,
    /// escapes and quoting are what delimit it.
    fn byte(self, byte: u8) -> Self {
        use Member::*;
        if is_space(byte) {
            // A `"` inside quotes that no second `"` follows ends them.
            return if self == InRangeQuotedQuote { InRange } else { self };
        }
        match (self, byte) {
            (Opening, _) => InRange,
            (InRange, b']' | b')') => Closed,
            (InRange, b'"') => InRangeQuoted,
            (InRange, b'\\') => InRangeEscaped,
            (InRange | InRangeEscaped, _) => InRange,
            (InRangeQuoted, b'"') => InRangeQuotedQuote,
            (InRangeQuoted, b'\\') => InRangeQuotedEscaped,
            (InRangeQuoted | InRangeQuotedEscaped, _) => InRangeQuoted,
            (InRangeQuotedQuote, b'"') => InRangeQuoted,
            (InRangeQuotedQuote, byte) => InRange.byte(byte),
            (Closed, _) => unreachable!("a closed member reads nothing"),
        }
    }
}

/// **A `multirange_in` literal's walk**, handing its caller every member
/// range as it reads it: what [`super::parse_multirange`] reads, holding one
/// member at a time and looking at nothing past the byte it is fed.
#[derive(Debug, Clone, Default)]
pub(crate) struct MultirangeWalk {
    stage: MultirangeStage,
    member: RangeWalk,
    /// The members read, `empty` among them.
    seen: usize,
}

/// Where a [`MultirangeWalk`] stood at a mark ([`MultirangeWalk::mark`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct MultirangeMark {
    stage: MultirangeStage,
    member: RangeMark,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum MultirangeStage {
    #[default]
    Lead,
    /// Past the opening brace or a separator, a member owed.
    Open,
    InMember(Member),
    /// Letters read of a member spelled `empty`.
    Empty(Word),
    /// Past a member.
    AfterMember,
    /// Past the closing brace.
    Trailing,
}

impl MultirangeWalk {
    /// Ready the walk for another literal.
    pub(crate) fn start(&mut self) {
        self.stage = MultirangeStage::Lead;
        self.seen = 0;
    }

    /// Where the walk stands, for [`MultirangeWalk::rewind`] to return to
    /// past a whitespace run, which ends no member and starts none.
    pub(crate) fn mark(&self) -> MultirangeMark {
        MultirangeMark { stage: self.stage, member: self.member.mark() }
    }

    /// Return to `mark`.
    pub(crate) fn rewind(&mut self, mark: MultirangeMark) {
        self.stage = mark.stage;
        self.member.rewind(mark.member);
    }

    /// Read the literal's next byte, handing `each` a member range it
    /// completes, but one that is empty.
    // pg-refuses: I44 — every refusal this walk makes is `multirange_in`'s grammar's.
    pub(crate) fn byte(
        &mut self,
        byte: u8,
        each: &mut dyn FnMut(RangeLiteral) -> Walked,
    ) -> Walked {
        use MultirangeStage::*;
        self.stage = match self.stage {
            Lead | Open | AfterMember | Trailing if is_space(byte) => self.stage,
            Lead if byte == b'{' => Open,
            Open if matches!(byte, b'[' | b'(') => {
                self.member.start();
                self.member.byte(byte)?;
                InMember(Member::Opening.byte(byte))
            }
            Open if byte == b'}' && self.seen == 0 => Trailing,
            Open => {
                let mut word = Word::new(b"empty");
                if !word.letter(byte) {
                    return Err(Refused);
                }
                Empty(word)
            }
            InMember(member) => {
                self.member.byte(byte)?;
                match member.byte(byte) {
                    Member::Closed => {
                        let range = self.member.end()?;
                        self.seen += 1;
                        if !range.empty {
                            each(range)?;
                        }
                        AfterMember
                    }
                    member => InMember(member),
                }
            }
            Empty(mut word) => {
                if !word.letter(byte) {
                    return Err(Refused);
                }
                if word.done() {
                    self.seen += 1;
                    AfterMember
                } else {
                    Empty(word)
                }
            }
            AfterMember if byte == b',' => Open,
            AfterMember if byte == b'}' => Trailing,
            Lead | AfterMember | Trailing => return Err(Refused),
        };
        Ok(())
    }

    /// The literal has ended: whether it was read whole.
    pub(crate) fn end(&self) -> Walked {
        if self.stage == MultirangeStage::Trailing { Ok(()) } else { Err(Refused) }
    }
}
