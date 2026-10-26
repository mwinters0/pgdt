//! Where a native decode writes: in 16-bit values while the history before
//! its start is unknown, in bytes once it no longer matters.
//!
//! **Each sink holds a window, not its output.** It keeps what was written
//! from the earliest position its caller still reads (`release_before`), and
//! never less than the last 32 KiB, which a back-reference may reach: one
//! buffer reused in place, its front dropped once the part released is at
//! least half of it, so a decode writing a whole chunk holds a few windows of
//! it rather than the chunk. A sink nobody releases keeps everything. Not a
//! ring of one window: a guess the limit or a refusal cuts stops at the last
//! boundary its decode reached, as much as a block back, and reads the 32 KiB
//! before that.

use crate::zlib::WINDOW_SIZE;

/// The first marker: a value `MARKER + j` stands for byte `j` of the 32 KiB
/// before the decode's start, `j = 0` the oldest; a value below 256 is the
/// byte itself.
pub(crate) const MARKER: u16 = 256;

/// What a decode writes into.
pub(crate) trait Sink {
    /// Plaintext written so far, history aside.
    fn produced(&self) -> usize;
    fn literal(&mut self, byte: u8);
    /// Copy `length` bytes from `distance` back; `false` where that reaches
    /// before the history a back-reference may use.
    fn copy(&mut self, distance: usize, length: usize) -> bool;
    fn bytes(&mut self, bytes: &[u8]);
    /// Whether a marked sink's last 32 KiB hold no marker, so the decode can go
    /// on in bytes; a sink holding no markers answers false, never stopping.
    fn is_clean(&self) -> bool {
        false
    }
}

/// How many positions of `len` held from `base` may be dropped, the caller
/// reading nothing before `keep` and a back-reference nothing before the last
/// 32 KiB: worth a move of the rest only where it is at least half of them.
fn droppable(base: usize, len: usize, keep: usize) -> usize {
    let drop = keep.min(base + len.saturating_sub(WINDOW_SIZE)).saturating_sub(base);
    if 2 * drop >= len { drop } else { 0 }
}

/// Output whose history is unknown: markers where it would be read.
pub(crate) struct Marked {
    /// The values written from position `base` on.
    out: Vec<u16>,
    base: usize,
    /// The position after the last marker written: zero for the unknown
    /// history itself.
    clean_from: usize,
    /// The earliest position the caller still reads.
    keep: usize,
}

impl Marked {
    pub(crate) fn new() -> Marked {
        Marked { out: Vec::new(), base: 0, clean_from: 0, keep: 0 }
    }

    /// The value written at position `at`, which must still be held.
    pub(crate) fn get(&self, at: usize) -> u16 {
        self.out[at - self.base]
    }

    /// The values held, from the first position still held on: every value
    /// written, where nothing was released.
    #[cfg(test)]
    pub(crate) fn values(&self) -> &[u16] {
        &self.out
    }

    /// The caller reads nothing before position `at` from here on.
    pub(crate) fn release_before(&mut self, at: usize) {
        self.keep = self.keep.max(at);
    }

    /// The position after the last marker written.
    #[cfg(test)]
    pub(crate) fn clean_from(&self) -> usize {
        self.clean_from
    }

    /// How many values the sink has room for.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.out.capacity()
    }

    /// The last 32 KiB written, as bytes: only once [`Sink::is_clean`].
    pub(crate) fn window(&self) -> Vec<u8> {
        debug_assert!(self.is_clean());
        self.out[self.out.len() - WINDOW_SIZE..].iter().map(|&v| v as u8).collect()
    }

    /// Room for `n` more values, the front dropped first where it may be.
    #[inline]
    fn make_room(&mut self, n: usize) {
        if self.out.len() + n > self.out.capacity() {
            let drop = droppable(self.base, self.out.len(), self.keep);
            if drop > 0 {
                self.out.drain(..drop);
                self.base += drop;
            }
            self.out.reserve(n.max(4 * WINDOW_SIZE));
        }
    }
}

impl Sink for Marked {
    fn produced(&self) -> usize {
        self.base + self.out.len()
    }

    #[inline]
    fn literal(&mut self, byte: u8) {
        self.make_room(1);
        self.out.push(u16::from(byte));
    }

    /// Copied in bulk, as [`Plain`] copies: the values from `distance` back,
    /// markers standing for any part of it before the decode's start, then
    /// repeated to `length` where it overlaps what it writes. Only the part of
    /// the source before the last marker written is looked through, for the
    /// last marker the copy writes.
    fn copy(&mut self, distance: usize, length: usize) -> bool {
        let at = self.produced();
        if distance > at + WINDOW_SIZE {
            return false;
        }
        self.make_room(length);
        let start = self.out.len();
        let first = distance.min(length);
        // Positions before the start are markers; the rest of the source, from
        // `source` on, is held, the last 32 KiB never being dropped.
        let unknown = distance.saturating_sub(at).min(first);
        let oldest = WINDOW_SIZE + at - distance;
        self.out.extend((oldest..oldest + unknown).map(|j| MARKER + j as u16));
        let (source, known) = ((at + unknown).saturating_sub(distance), first - unknown);
        let from = source - self.base;
        self.out.extend_from_within(from..from + known);
        // The last marker among the copy's first `first` values: only those
        // read from before `clean_from` can be one.
        let dirty = self.clean_from.saturating_sub(source).min(known);
        let last = self.out[start + unknown..start + unknown + dirty]
            .iter()
            .rposition(|&v| v >= MARKER)
            .map(|i| unknown + i)
            .or(unknown.checked_sub(1));
        // An overlapping copy repeats the `distance` values before it: each
        // round copies all that is written since its start, doubling it.
        while self.out.len() - start < length {
            let n = (length - (self.out.len() - start)).min(self.out.len() - start);
            self.out.extend_from_within(start..start + n);
        }
        // The copy repeats its first `distance` values, so its last marker is
        // the last repetition of theirs.
        if let Some(m) = last {
            self.clean_from = at + m + distance * ((length - 1 - m) / distance) + 1;
        }
        true
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.make_room(bytes.len());
        self.out.extend(bytes.iter().map(|&b| u16::from(b)));
    }

    #[inline]
    fn is_clean(&self) -> bool {
        self.produced() - self.clean_from >= WINDOW_SIZE
    }
}

/// Output whose history is known, or not needed.
///
/// Positions count from the history's first byte, so a decode's plaintext
/// offset `i` is position `history + i`.
pub(crate) struct Plain {
    /// The bytes written from position `base` on, history first.
    out: Vec<u8>,
    base: usize,
    /// How many positions are history, not output.
    history: usize,
    /// The earliest position a back-reference may reach: the member's start
    /// where it began here, else the first.
    floor: usize,
    /// The earliest position the caller still reads.
    keep: usize,
}

impl Plain {
    /// Output beginning at a member's start, with no history.
    pub(crate) fn new() -> Plain {
        Plain { out: Vec::new(), base: 0, history: 0, floor: 0, keep: 0 }
    }

    /// Output whose member's last 32 KiB before it are `window`.
    pub(crate) fn after(window: Vec<u8>) -> Plain {
        Plain { history: window.len(), out: window, base: 0, floor: 0, keep: 0 }
    }

    /// A new member begins here: nothing before it is history.
    pub(crate) fn member_start(&mut self) {
        self.floor = self.base + self.out.len();
    }

    /// The byte at plaintext offset `i`, which must still be held.
    pub(crate) fn get(&self, i: usize) -> u8 {
        self.out[self.history + i - self.base]
    }

    /// The plaintext written: only where nothing was released.
    #[cfg(test)]
    pub(crate) fn written(&self) -> &[u8] {
        assert_eq!(self.base, 0, "a released sink");
        &self.out[self.history..]
    }

    /// The caller reads nothing before plaintext offset `i` from here on.
    pub(crate) fn release_before(&mut self, i: usize) {
        self.keep = self.keep.max(self.history + i);
    }

    /// How many bytes the sink has room for.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.out.capacity()
    }

    /// Room for `n` more bytes, the front dropped first where it may be.
    #[inline]
    fn make_room(&mut self, n: usize) {
        if self.out.len() + n > self.out.capacity() {
            let drop = droppable(self.base, self.out.len(), self.keep);
            if drop > 0 {
                self.out.drain(..drop);
                self.base += drop;
            }
            self.out.reserve(n.max(8 * WINDOW_SIZE));
        }
    }
}

impl Sink for Plain {
    fn produced(&self) -> usize {
        self.base + self.out.len() - self.history
    }

    #[inline]
    fn literal(&mut self, byte: u8) {
        self.make_room(1);
        self.out.push(byte);
    }

    #[inline]
    fn copy(&mut self, distance: usize, length: usize) -> bool {
        let at = self.base + self.out.len();
        if distance > at - self.floor {
            return false;
        }
        self.make_room(length);
        let from = at - distance - self.base;
        if distance >= length {
            self.out.extend_from_within(from..from + length);
            return true;
        }
        // An overlapping copy repeats the `distance` bytes before it: each
        // round copies all that is written since `from`, doubling it.
        let mut left = length;
        while left > 0 {
            let n = left.min(self.out.len() - from);
            self.out.extend_from_within(from..from + n);
            left -= n;
        }
        true
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.make_room(bytes.len());
        self.out.extend_from_slice(bytes);
    }
}
