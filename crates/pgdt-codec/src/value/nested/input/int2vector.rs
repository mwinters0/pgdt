//! `int2vectorin`'s grammar as a walk fed a byte at a time.

use crate::value::nested::is_space;
use crate::value::source::{Refused, Walked};

/// **An `int2vectorin` literal's walk**, handing its caller every element as
/// it reads it (I47), looking at nothing past the byte it is fed
/// (`docs/design/decisions.md`, "D144").
///
/// `int2vectorin` skips any run of whitespace before an element, hands what
/// follows to `strtol` — so a leading `+` and a leading zero are taken —
/// range-checks the result against `int16`, and then requires the byte
/// *after* the number to be a space or the end of the string. That last rule
/// is the one worth reproducing rather than smoothing into "any whitespace
/// separates": `\t1 2` is accepted and `1\t2` is not.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Int2VectorWalk {
    stage: Stage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Stage {
    /// Before an element, whitespace skipped.
    #[default]
    Lead,
    Signed {
        negative: bool,
    },
    /// The magnitude held at one past `int16`'s.
    Digits {
        negative: bool,
        magnitude: i32,
    },
}

impl Int2VectorWalk {
    const LIMIT: i32 = i16::MAX as i32 + 1;

    /// Ready the walk for another literal.
    pub(crate) fn start(&mut self) {
        self.stage = Stage::Lead;
    }

    /// Read the literal's next byte, handing `each` an element it ends.
    // pg-refuses: I47 — every refusal this walk makes is `int2vectorin`'s, the `int16` bound included.
    pub(crate) fn byte(&mut self, byte: u8, each: &mut dyn FnMut(i16) -> Walked) -> Walked {
        self.stage = match (self.stage, byte) {
            (Stage::Lead, byte) if is_space(byte) => Stage::Lead,
            (Stage::Lead, b'-') => Stage::Signed { negative: true },
            (Stage::Lead, b'+') => Stage::Signed { negative: false },
            (Stage::Lead, b'0'..=b'9') => {
                Stage::Digits { negative: false, magnitude: i32::from(byte - b'0') }
            }
            (Stage::Signed { negative }, b'0'..=b'9') => {
                Stage::Digits { negative, magnitude: i32::from(byte - b'0') }
            }
            (Stage::Digits { negative, magnitude }, b'0'..=b'9') => Stage::Digits {
                negative,
                magnitude: (magnitude * 10 + i32::from(byte - b'0')).min(Self::LIMIT + 1),
            },
            (Stage::Digits { negative, magnitude }, b' ') => {
                each(Self::element(negative, magnitude)?)?;
                Stage::Lead
            }
            _ => return Err(Refused),
        };
        Ok(())
    }

    /// The literal has ended, handing `each` an element it ends.
    pub(crate) fn end(&mut self, each: &mut dyn FnMut(i16) -> Walked) -> Walked {
        match std::mem::take(&mut self.stage) {
            Stage::Lead => Ok(()),
            Stage::Digits { negative, magnitude } => each(Self::element(negative, magnitude)?),
            Stage::Signed { .. } => Err(Refused),
        }
    }

    /// `strtol`'s own `ERANGE`, then `int2vectorin`'s explicit
    /// `SHRT_MIN`/`SHRT_MAX` check — one refusal here, since both say the
    /// literal is not a value of this type.
    fn element(negative: bool, magnitude: i32) -> Walked<i16> {
        i16::try_from(if negative { -magnitude } else { magnitude }).map_err(|_| Refused)
    }
}
