//! A bytewise value's head: all a stored bound and a row order's place for it
//! are taken from (`docs/design/decisions.md`, "D76").

use std::cmp::Ordering;

use pgdt_codec::decode::Unread;
use pgdt_codec::internal::{ByteaRuns, TextFold, TextRead, bytea_unread};
use pgdt_core::internal::Unescape;
use pgdt_model::DICTIONARY_ENTRY_MAX_BYTES;

/// How much of a bytewise value is kept: past the cap by one character's
/// width, so a kept head longer than the cap says its value is too.
pub(super) const CLIP_BYTES: usize = DICTIONARY_ENTRY_MAX_BYTES + char::MAX.len_utf8();

/// How many of an `escape` value's bytes a `bytea`'s bounds renders: enough
/// that a value cut there renders past [`CLIP_BYTES`], so its head is the
/// whole value's and it reads as no whole value, as the value itself does
/// (`docs/design/decisions.md`, "D76").
pub(super) const BYTEA_ESCAPE_HEAD_BYTES: usize = CLIP_BYTES / 2;

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
/// "D142"): whole, or, where it is long, as its runs went by.
#[derive(Clone, Copy)]
pub(super) enum Field<'a> {
    Whole(&'a str),
    /// Longer than [`LONG_FIELD_HEAD`], so longer than anything a statistic
    /// keeps whole.
    Long(&'a LongText),
}

impl Field<'_> {
    /// The text's length in bytes.
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Whole(text) => text.len(),
            Self::Long(long) => long.read.len,
        }
    }

    /// Whether `byteain` refuses the text, a long one as its reading
    /// answered.
    pub(super) fn bytea_refused(&self) -> bool {
        match self {
            Self::Whole(text) => bytea_unread(text.bytes()) == Unread::Refused,
            Self::Long(long) => long.bytea().refused,
        }
    }
}

/// How much of a long field [`LongText`] keeps: at least [`CLIP_BYTES`] on
/// a character boundary, a character being at most four bytes. An escaped
/// field longer than this, of a column reading no field whole, is read as one,
/// never unescaped whole ([`LongRead`]).
pub(super) const LONG_FIELD_HEAD: usize = CLIP_BYTES + 3;

/// **A long field's text as its runs went by, never held whole**
/// (`docs/design/decisions.md`, "D142"): its head, and what a statistic or a
/// refusal reads of the rest ([`TextRead`]), and in a column whose bounds
/// read a `bytea`'s form, that form's reading ([`LongBytea`]).
pub(super) struct LongText {
    /// Its first [`LONG_FIELD_HEAD`] bytes, cut back to a character boundary.
    pub(super) head: String,
    pub(super) read: TextRead,
    bytea: Option<LongBytea>,
}

/// What a `bytea`'s bounds read of a long text ([`Canonical`](super::bounds)):
/// whether it is the canonical hex form, its `escape` form's head where that
/// form reads it, and whether `byteain` refuses it.
pub(super) struct LongBytea {
    pub(super) canonical_hex: bool,
    pub(super) escape_head: Option<Vec<u8>>,
    pub(super) refused: bool,
}

impl LongText {
    /// `field`, as `COPY` wrote it and not SQL `NULL`, read once in its
    /// plain stretches, its `bytea` form read where `bytea`; `None` where its
    /// text is not UTF-8.
    #[cfg(test)]
    pub(super) fn read(field: &[u8], bytea: bool) -> Option<Self> {
        let mut read = LongRead::new(bytea);
        read.run(field);
        read.finish()
    }

    /// The text, where its head is all of it.
    pub(super) fn whole(&self) -> Option<&str> {
        (self.head.len() == self.read.len).then_some(&self.head)
    }

    /// The text as its bounds read it.
    pub(super) fn head(&self) -> Head<'_> {
        Head { text: &self.head, len: self.read.len }
    }

    /// Its `bytea` form's reading, which a column whose bounds read one took.
    pub(super) fn bytea(&self) -> &LongBytea {
        self.bytea.as_ref().expect("a bytea column reads a long text's form")
    }
}

/// **A long field read a run at a time as `COPY` wrote it** — the one
/// reading of a [`LongText`], whether its runs are a held field's whole or a
/// streamed one's as they go by.
pub(super) struct LongRead {
    escape: Unescape,
    head: Vec<u8>,
    text: TextFold,
    bytea: Option<(CanonicalHex, ByteaRuns)>,
}

impl LongRead {
    /// Nothing read yet, a `bytea`'s form read beside the text where `bytea`.
    pub(super) fn new(bytea: bool) -> Self {
        let bytea =
            bytea.then(|| (CanonicalHex::default(), ByteaRuns::new(BYTEA_ESCAPE_HEAD_BYTES)));
        Self { escape: Unescape::default(), head: Vec::new(), text: TextFold::default(), bytea }
    }

    /// Read the field's next run, as `COPY` wrote it.
    pub(super) fn run(&mut self, escaped: &[u8]) {
        let Self { escape, head, text, bytea } = self;
        escape.run(escaped, |run| Self::text(head, text, bytea, run));
    }

    /// Read the next run of its text.
    fn text(
        head: &mut Vec<u8>,
        text: &mut TextFold,
        bytea: &mut Option<(CanonicalHex, ByteaRuns)>,
        run: &[u8],
    ) {
        let kept = LONG_FIELD_HEAD.saturating_sub(head.len()).min(run.len());
        head.extend_from_slice(&run[..kept]);
        text.push(run);
        if let Some((hex, form)) = bytea {
            hex.push(run);
            form.push(run);
        }
    }

    /// The field has ended: what was read, `None` where its text is not
    /// UTF-8.
    pub(super) fn finish(mut self) -> Option<LongText> {
        let Self { escape, head, text, bytea } = &mut self;
        escape.finish(|run| Self::text(head, text, bytea, run));
        let read = std::mem::take(text).finish()?;
        if let Err(cut) = std::str::from_utf8(head) {
            head.truncate(cut.valid_up_to());
        }
        let head =
            String::from_utf8(std::mem::take(head)).expect("a prefix of UTF-8 cut at a character");
        let bytea = bytea.take().map(|(hex, form)| {
            let (escape_head, refused) = form.finish();
            LongBytea { canonical_hex: hex.finish(), escape_head, refused }
        });
        Some(LongText { head, read, bytea })
    }
}

/// **Whether a text is `bytea`'s hex form as `byteaout` writes it**, which
/// orders as the bytes do — `\x` and lowercase hex pairs — read a run at a
/// time.
#[derive(Clone, Copy, Default)]
pub(super) struct CanonicalHex {
    /// The bytes read, as far as the prefix's two.
    read: u8,
    /// Whether every byte so far is the form's.
    holds: bool,
    /// Whether an odd number of digits has been read.
    odd: bool,
}

impl CanonicalHex {
    /// Whether `text` is the form, read whole.
    pub(super) fn of(text: &[u8]) -> bool {
        let mut hex = Self::default();
        hex.push(text);
        hex.finish()
    }

    /// Read the text's next run: the prefix a byte at a time, and the
    /// digits after it as a stretch.
    pub(super) fn push(&mut self, mut run: &[u8]) {
        while self.read < 2 {
            let Some((&byte, rest)) = run.split_first() else { return };
            match self.read {
                0 => self.holds = byte == b'\\',
                _ => self.holds &= byte == b'x',
            }
            self.read += 1;
            run = rest;
        }
        self.holds = self.holds && run.iter().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        self.odd ^= run.len() % 2 == 1;
    }

    pub(super) fn finish(self) -> bool {
        self.read == 2 && self.holds && !self.odd
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
