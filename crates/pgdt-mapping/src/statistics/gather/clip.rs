//! A bytewise value's head: all a stored bound and a row order's place for it
//! are taken from (`docs/design/decisions.md`, "D76").

use std::cmp::Ordering;

use pgdt_codec::internal::{TextFold, TextRead};
use pgdt_core::internal::Unescaped;
use pgdt_model::DICTIONARY_ENTRY_MAX_BYTES;

/// How much of a bytewise value is kept: past the cap by one character's
/// width, so a kept head longer than the cap says its value is too.
pub(super) const CLIP_BYTES: usize = DICTIONARY_ENTRY_MAX_BYTES + char::MAX.len_utf8();

/// **A bytewise value as its bounds read it**: its length, and a head of it
/// on a character boundary — the whole value, or at least [`CLIP_BYTES`] of
/// it, which is all a stored bound and a row order read
/// (`docs/design/decisions.md`, "D76", "D142").
#[derive(Clone, Copy)]
pub(super) struct Head<'a> {
    pub(super) text: &'a str,
    pub(super) len: usize,
}

impl<'a> Head<'a> {
    pub(super) fn whole(text: &'a str) -> Self {
        Self { text, len: text.len() }
    }
}

/// **A field's text as gathering reads it** (`docs/design/decisions.md`,
/// "D142"): whole, or, where it is long and escaped, read off its bytes.
#[derive(Clone, Copy)]
pub(super) enum Field<'a> {
    Whole(&'a str),
    /// Longer than [`LONG_FIELD_HEAD`], so longer than anything a statistic
    /// keeps whole.
    Long(&'a LongText<'a>),
}

impl Field<'_> {
    /// The text's length in bytes.
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Whole(text) => text.len(),
            Self::Long(long) => long.read.len,
        }
    }

    /// The text's bytes, read again.
    pub(super) fn bytes(&self) -> FieldBytes<'_> {
        match self {
            Self::Whole(text) => FieldBytes::Whole(text.bytes()),
            Self::Long(long) => FieldBytes::Long(long.bytes.clone()),
        }
    }
}

/// [`Field::bytes`].
pub(super) enum FieldBytes<'a> {
    Whole(std::str::Bytes<'a>),
    Long(Unescaped<'a>),
}

impl Iterator for FieldBytes<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        match self {
            Self::Whole(bytes) => bytes.next(),
            Self::Long(bytes) => bytes.next(),
        }
    }
}

/// How much of a long field [`LongText`] keeps: at least [`CLIP_BYTES`] on
/// a character boundary, a character being at most four bytes. An escaped
/// field longer than this is read as one ([`LongText::read`]).
pub(super) const LONG_FIELD_HEAD: usize = CLIP_BYTES + 3;

/// **A long escaped field read off its bytes, never unescaped whole**
/// (`docs/design/decisions.md`, "D142"): its head, and what a statistic or
/// a refusal reads of the rest ([`TextRead`]) — a `bytea`'s form, which only
/// its own kind reads, being read again from `bytes`.
pub(super) struct LongText<'a> {
    pub(super) bytes: Unescaped<'a>,
    /// Its first [`LONG_FIELD_HEAD`] bytes, cut back to a character boundary.
    pub(super) head: String,
    pub(super) read: TextRead,
}

impl<'a> LongText<'a> {
    /// `bytes` read once, `None` where they are not UTF-8.
    pub(super) fn read(bytes: Unescaped<'a>) -> Option<Self> {
        let mut head = Vec::with_capacity(LONG_FIELD_HEAD);
        let read = TextFold::read(bytes.clone(), |byte| {
            if head.len() < LONG_FIELD_HEAD {
                head.push(byte);
            }
        })?;
        if let Err(cut) = std::str::from_utf8(&head) {
            head.truncate(cut.valid_up_to());
        }
        let head = String::from_utf8(head).expect("a prefix of UTF-8 cut at a character");
        Some(Self { bytes, head, read })
    }

    /// The text, where its head is all of it.
    pub(super) fn whole(&self) -> Option<&str> {
        (self.head.len() == self.read.len).then_some(&self.head)
    }

    /// The text as its bounds read it.
    pub(super) fn head(&self) -> Head<'_> {
        Head { text: &self.head, len: self.read.len }
    }
}

/// A bytewise value's first [`CLIP_BYTES`], which is all a stored bound is
/// taken from (`docs/design/decisions.md`, "D76").
///
/// **Two values whose heads cannot be told apart store the same bound.** A
/// head that is not its whole value is at least `CLIP_BYTES` less one
/// character long, so two values it cannot place against each other share
/// more bytes than `bounds::clipped_bounds` reads of either. That is what lets a
/// group's bounds be joined from two halves' heads (`bounds::GroupBounds::absorb`)
/// and still store what one pass over the group's values stores: each keeps a
/// value no other value in its half is certainly beyond, and that value can
/// differ from the true extreme only by bytes no stored bound holds.
#[derive(Clone)]
pub(super) struct Clipped {
    pub(super) head: String,
    /// Whether `head` is the whole value.
    pub(super) whole: bool,
}

impl Clipped {
    pub(super) fn of(text: Head<'_>) -> Self {
        let head = text_prefix(text.text, CLIP_BYTES);
        Self { head: head.to_owned(), whole: head.len() == text.len }
    }

    /// Where `text` orders against the value this was clipped from, `None`
    /// when `text` runs past a head that is not the whole value and agrees
    /// with it. Two such
    /// values share every byte a stored bound reads, so either stands for
    /// the other in the group's bounds; only their row order is unknown.
    pub(super) fn locate(&self, text: Head<'_>) -> Option<Ordering> {
        self.locate_bytes(text.text.as_bytes(), text.len)
    }

    /// [`Self::locate`] for a text known by its length and its first
    /// [`CLIP_BYTES`] bytes (all of them, if it is shorter) — as much as the
    /// answer reads — so a piece's first value is placed exactly as a row is.
    pub(super) fn locate_bytes(&self, prefix: &[u8], len: usize) -> Option<Ordering> {
        let head = self.head.as_bytes();
        let shared = head.len().min(len);
        match prefix[..shared].cmp(&head[..shared]) {
            Ordering::Equal if len > head.len() => self.whole.then_some(Ordering::Greater),
            Ordering::Equal if len == head.len() && self.whole => Some(Ordering::Equal),
            Ordering::Equal => Some(Ordering::Less),
            unequal => Some(unequal),
        }
    }

    /// Where `other`'s value orders against this one's, known only by their
    /// heads: `None` exactly where the shorter head agrees with the longer and
    /// is not its whole value, which [`Clipped`] says stores the same bound.
    pub(super) fn order(&self, other: &Clipped) -> Option<Ordering> {
        let (mine, theirs) = (self.head.as_bytes(), other.head.as_bytes());
        let shared = mine.len().min(theirs.len());
        match theirs[..shared].cmp(&mine[..shared]) {
            Ordering::Equal => {}
            unequal => return Some(unequal),
        }
        match theirs.len().cmp(&mine.len()) {
            Ordering::Less => other.whole.then_some(Ordering::Less),
            Ordering::Greater => self.whole.then_some(Ordering::Greater),
            Ordering::Equal => match (self.whole, other.whole) {
                (true, true) => Some(Ordering::Equal),
                (true, false) => Some(Ordering::Greater),
                (false, true) => Some(Ordering::Less),
                (false, false) => None,
            },
        }
    }
}

/// The longest prefix of `text` no longer than `cap` bytes that ends on a
/// character boundary — a lower bound on `text` bytewise.
pub(super) fn text_prefix(text: &str, cap: usize) -> &str {
    let mut end = cap.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
