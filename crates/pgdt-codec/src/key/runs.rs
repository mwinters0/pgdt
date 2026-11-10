//! A value of a bytewise kind — a text, a `character`, a `bytea` — ordered
//! against a bound as its text's runs come, holding none of it: what a
//! filter's term reads an escaped field by, off `Unescape`, and a whole text
//! by as one run (`docs/design/decisions.md`, "D150").

use std::cmp::Ordering;

use crate::compare::CompareKind;
use crate::key::order::OrderKey;
use crate::value::bytea::ByteaFold;
use crate::value::text::{TextFold, char_typmod_refuses};

/// **A text read a run at a time, ordered bytewise against `bound` as it
/// goes**: its first byte that differs, or which of the two is a prefix of
/// the other. Where it is `trimmed`, the text's trailing blanks (`0x20`)
/// are no part of it, as `bcTruelen` reads a `character` (I38), and `bound`
/// holds none.
#[derive(Debug, Clone)]
pub struct BytewiseRuns<'b> {
    bound: &'b [u8],
    /// How much of `bound` the text has matched.
    at: usize,
    /// `Equal` while every byte read matches `bound`'s; the answer once one
    /// does not.
    order: Ordering,
    trimmed: bool,
    /// Blanks read and not yet compared, where `trimmed`: a later non-blank
    /// compares them, and the text's end drops them.
    blanks: usize,
}

impl<'b> BytewiseRuns<'b> {
    /// A text ordered against `bound`, every byte of it read.
    pub fn new(bound: &'b [u8]) -> Self {
        Self { bound, at: 0, order: Ordering::Equal, trimmed: false, blanks: 0 }
    }

    /// A text ordered against `bound` without its trailing blanks.
    pub fn trimmed(bound: &'b [u8]) -> Self {
        Self { trimmed: true, ..Self::new(bound) }
    }

    /// Read the text's next run.
    pub fn push(&mut self, run: &[u8]) {
        if self.order.is_ne() {
            return;
        }
        if !self.trimmed {
            return self.compare(run);
        }
        match run.iter().rposition(|&byte| byte != b' ') {
            None => self.blanks += run.len(),
            Some(last) => {
                self.compare_blanks();
                self.compare(&run[..=last]);
                self.blanks = run.len() - last - 1;
            }
        }
    }

    /// Compare the blanks held back, a non-blank having followed them: no
    /// more of them than `bound` has left, one past it deciding.
    fn compare_blanks(&mut self) {
        while self.blanks > 0 && self.order.is_eq() {
            self.order = match self.bound.get(self.at) {
                Some(&byte) => b' '.cmp(&byte),
                None => Ordering::Greater,
            };
            self.at += 1;
            self.blanks -= 1;
        }
        self.blanks = 0;
    }

    fn compare(&mut self, run: &[u8]) {
        if self.order.is_ne() {
            return;
        }
        let rest = &self.bound[self.at..];
        let shared = run.len().min(rest.len());
        self.order = run[..shared].cmp(&rest[..shared]);
        self.at += shared;
        if self.order.is_eq() && run.len() > shared {
            self.order = Ordering::Greater;
        }
    }

    /// The text has ended: how it orders against `bound`.
    pub fn finish(self) -> Ordering {
        if self.order.is_eq() && self.at < self.bound.len() { Ordering::Less } else { self.order }
    }
}

/// **A field of a bytewise kind ordered against its bound key as its
/// unescaped text's runs come**: what [`read_key`](super::order::read_key)
/// and [`compare_keys`](super::order::compare_keys) answer of it, with no
/// key of it built — a `text` or a `character` under its typmod, and a
/// `bytea` as either `bytea_output` form decodes.
#[derive(Debug, Clone)]
pub struct FieldOrder<'b> {
    order: BytewiseRuns<'b>,
    read: Reading,
}

#[derive(Debug, Clone)]
enum Reading {
    /// A text, held to its typmod's `length` where it has one.
    Text { length: Option<u32>, fold: TextFold },
    /// A `bytea`'s text, decoded as it is read, its bytes ordered.
    Bytea(ByteaFold),
}

/// How many decoded `bytea` bytes are ordered at once.
const DECODED_RUN: usize = 256;

impl<'b> FieldOrder<'b> {
    /// A field of `kind` ordered against `bound`, that kind's key, or `None`
    /// where `kind` is no bytewise kind.
    pub fn new(kind: &CompareKind, bound: &'b OrderKey) -> Option<Self> {
        let text = |length: Option<u32>| Reading::Text { length, fold: TextFold::default() };
        let (bound, trimmed, read) = match (kind, bound) {
            (CompareKind::Text { length, .. }, OrderKey::Text(bound)) => {
                (bound.as_bytes(), false, text(*length))
            }
            (CompareKind::PaddedText { length }, OrderKey::Text(bound)) => {
                (bound.as_bytes(), true, text(*length))
            }
            (CompareKind::Bytea, OrderKey::Bytes(bound)) => {
                (bound.as_slice(), false, Reading::Bytea(ByteaFold::default()))
            }
            _ => return None,
        };
        let order = if trimmed { BytewiseRuns::trimmed(bound) } else { BytewiseRuns::new(bound) };
        Some(Self { order, read })
    }

    /// Read the field's text's next run.
    pub fn push(&mut self, run: &[u8]) {
        let Self { order, read } = self;
        match read {
            Reading::Text { length, fold } => {
                if length.is_some() {
                    fold.push(run);
                }
                order.push(run);
            }
            Reading::Bytea(fold) => {
                let mut decoded = [0u8; DECODED_RUN];
                let mut at = 0;
                for &byte in run {
                    fold.byte(byte, &mut |byte| {
                        decoded[at] = byte;
                        at += 1;
                    });
                    if at == DECODED_RUN {
                        order.push(&decoded);
                        at = 0;
                    }
                }
                order.push(&decoded[..at]);
            }
        }
    }

    /// The text has ended: how the field orders against the bound, or
    /// `None` where it is no value its key reads — a text past its typmod,
    /// a `bytea` neither form decodes.
    pub fn finish(self) -> Option<Ordering> {
        let Self { order, read } = self;
        let read = match read {
            Reading::Text { length: Some(length), fold } => {
                !fold.finish().is_some_and(|text| text.typmod_refuses(length))
            }
            Reading::Text { length: None, .. } => true,
            Reading::Bytea(mut fold) => fold.end().is_ok(),
        };
        read.then(|| order.finish())
    }

    /// [`Self::push`] and [`Self::finish`] over a whole text, a typmod's
    /// characters counted only where the text is longer in bytes.
    pub fn of(mut self, text: &str) -> Option<Ordering> {
        if let Reading::Text { length, .. } = &mut self.read {
            if length.is_some_and(|length| char_typmod_refuses(text, length)) {
                return None;
            }
            *length = None;
        }
        self.push(text.as_bytes());
        self.finish()
    }
}

#[cfg(test)]
mod tests;
