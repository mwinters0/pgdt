//! A bytewise value's head: all a stored bound and a row order's place for it
//! are taken from (`docs/design/decisions.md`, "D76").

use std::cmp::Ordering;

use pgdt_model::DICTIONARY_ENTRY_MAX_BYTES;

/// How much of a bytewise value is kept: past the cap by one character's
/// width, so a kept head longer than the cap says its value is too.
pub(super) const CLIP_BYTES: usize = DICTIONARY_ENTRY_MAX_BYTES + char::MAX.len_utf8();

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
    pub(super) fn of(text: &str) -> Self {
        let head = text_prefix(text, CLIP_BYTES);
        Self { head: head.to_owned(), whole: head.len() == text.len() }
    }

    /// Where `text` orders against the value this was clipped from, `None`
    /// when `text` begins with a head that is not the whole value. Two such
    /// values share every byte a stored bound reads, so either stands for
    /// the other in the group's bounds; only their row order is unknown.
    pub(super) fn locate(&self, text: &str) -> Option<Ordering> {
        self.locate_bytes(text.as_bytes(), text.len())
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
