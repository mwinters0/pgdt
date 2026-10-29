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
//!
//! **Nor more than its limit asks.** A decode under a limit stops there,
//! having written at most one copy past it, so a sink made for that limit
//! grows to that room and no further, where a doubling would pass it.

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
    /// The plaintext position a decode under `limit` stops at, as things
    /// stand: the limit, or where a marked sink would come clean, which
    /// within a block only a copy moves.
    #[inline(always)]
    fn stop_at(&self, limit: usize) -> usize {
        limit
    }
    /// A block ended here: where a marked sink finds its last marker.
    #[inline(always)]
    fn block_ended(&mut self) {}
}

/// How many positions of `len` held from `base` may be dropped, the caller
/// reading nothing before `keep` and a back-reference nothing before the last
/// 32 KiB: worth a move of the rest only where it is at least half of them.
fn droppable(base: usize, len: usize, keep: usize) -> usize {
    let drop = keep.min(base + len.saturating_sub(WINDOW_SIZE)).saturating_sub(base);
    if 2 * drop >= len { drop } else { 0 }
}

/// The values a chunked copy may write past a copy's end, a whole chunk
/// being written at a time: room the buffer keeps beyond what it holds.
const SLACK: usize = 32;

/// The longest copy deflate codes: how far past its limit a decode writes,
/// a step beginning only short of it.
const LONGEST_COPY: usize = 258;

/// The room a sink keeps past its history and its decode's limit: a copy
/// begun just short of the limit, and its slack.
pub(crate) const ROOM_PAST_LIMIT: usize = LONGEST_COPY + SLACK;

/// A buffer of values written in place: `len` of them held, the rest room
/// ([`SLACK`] past any write), from position `base` on.
struct Held<T> {
    buf: Vec<T>,
    len: usize,
    base: usize,
    /// The earliest position the caller still reads.
    keep: usize,
    /// The most room it grows to: what its decode's limit asks.
    most: usize,
}

impl<T: Copy + Default> Held<T> {
    /// A buffer holding `history`, written on by a decode under `limit`: its
    /// room at most the history, the limit, a copy begun just short of it
    /// and its slack.
    fn new(history: Vec<T>, limit: usize) -> Held<T> {
        let len = history.len();
        let most = len.saturating_add(limit).saturating_add(ROOM_PAST_LIMIT);
        Held { buf: history, len, base: 0, keep: 0, most }
    }

    fn held(&self) -> &[T] {
        &self.buf[..self.len]
    }

    /// Room for `n` more values and a chunk past them.
    #[inline(always)]
    fn make_room(&mut self, n: usize, least: usize) {
        if self.len + n + SLACK > self.buf.len() {
            self.grow(n, least);
        }
    }

    /// The front dropped where it may be, then the buffer grown where that
    /// leaves too little room — at least `least` values, and doubling, but
    /// never past the room its limit asks, which no write under the limit
    /// passes.
    #[cold]
    fn grow(&mut self, n: usize, least: usize) {
        let drop = droppable(self.base, self.len, self.keep);
        if drop > 0 {
            self.buf.copy_within(drop..self.len, 0);
            self.len -= drop;
            self.base += drop;
        }
        let want = self.len + n + SLACK;
        debug_assert!(want <= self.most, "a write {want} past the limit's room {}", self.most);
        if want > self.buf.len() {
            let size = want.max((2 * self.buf.len()).max(least).min(self.most));
            self.buf.resize(size, T::default());
        }
    }

    /// Write `value` next, room being made for it.
    #[inline(always)]
    fn push(&mut self, value: T, least: usize) {
        self.make_room(1, least);
        self.buf[self.len] = value;
        self.len += 1;
    }
}

/// Copy `length` values from `distance` back to `out[start..]`, overlapping
/// as deflate's copies do — the source repeated where it is shorter than the
/// copy — in chunks of `N` where `distance` allows, writing up to a chunk
/// past the copy's end.
#[inline(always)]
fn copy_back<T: Copy, const N: usize>(out: &mut [T], start: usize, distance: usize, length: usize) {
    if distance >= N {
        // From the source's first value to the last chunk's end, so the
        // compiler sees every chunk inside it.
        let out = &mut out[start - distance..start + length + N];
        let mut i = 0;
        while i < length {
            let chunk: [T; N] = out[i..i + N].try_into().expect("a chunk");
            out[distance + i..distance + i + N].copy_from_slice(&chunk);
            i += N;
        }
    } else {
        for i in start..start + length {
            out[i] = out[i - distance];
        }
    }
}

/// Output whose history is unknown: markers where it would be read.
pub(crate) struct Marked {
    out: Held<u16>,
    /// A position no marker is written at or after: at a block's end the
    /// position after the last marker, or no later than the start of the last
    /// 32 KiB where they hold none; inside a block the end of the last copy whose source
    /// began before it. So the sink comes clean no sooner than its values do
    /// and at most a block later. Zero for the unknown history itself.
    clean_from: usize,
}

impl Marked {
    /// A sink for a decode under `limit`.
    pub(crate) fn new(limit: usize) -> Marked {
        Marked { out: Held::new(Vec::new(), limit), clean_from: 0 }
    }

    /// The value written at position `at`, which must still be held.
    pub(crate) fn get(&self, at: usize) -> u16 {
        self.out.held()[at - self.out.base]
    }

    /// The values held, from the first position still held on: every value
    /// written, where nothing was released.
    #[cfg(test)]
    pub(crate) fn values(&self) -> &[u16] {
        self.out.held()
    }

    /// The caller reads nothing before position `at` from here on.
    pub(crate) fn release_before(&mut self, at: usize) {
        self.out.keep = self.out.keep.max(at);
    }

    /// The clean mark: no marker at or after it.
    #[cfg(test)]
    pub(crate) fn clean_from(&self) -> usize {
        self.clean_from
    }

    /// How many values the sink has room for.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.out.buf.len()
    }

    /// The last 32 KiB written, as bytes: only once [`Sink::is_clean`].
    pub(crate) fn window(&self) -> Vec<u8> {
        debug_assert!(self.is_clean());
        let held = self.out.held();
        held[held.len() - WINDOW_SIZE..].iter().map(|&v| v as u8).collect()
    }

    /// Copy `length` values from `distance` back where that reaches before
    /// the decode's start, `at`: a marker for each such position.
    #[cold]
    fn copy_from_before(&mut self, at: usize, distance: usize, length: usize) {
        let start = self.out.len;
        for i in 0..length {
            self.out.buf[start + i] = if i < distance && at + i < distance {
                MARKER + (WINDOW_SIZE + at + i - distance) as u16
            } else {
                self.out.buf[start + i - distance]
            };
        }
    }
}

impl Sink for Marked {
    #[inline(always)]
    fn produced(&self) -> usize {
        self.out.base + self.out.len
    }

    #[inline(always)]
    fn literal(&mut self, byte: u8) {
        self.out.push(u16::from(byte), 4 * WINDOW_SIZE);
    }

    /// Copied a chunk at a time, its values never looked at: a copy whose
    /// source begins before the clean mark may write a marker, so it moves
    /// the mark to its end, and a block's end finds the last one.
    #[inline(always)]
    fn copy(&mut self, distance: usize, length: usize) -> bool {
        let at = self.produced();
        if distance > at + WINDOW_SIZE {
            return false;
        }
        self.out.make_room(length, 4 * WINDOW_SIZE);
        let start = self.out.len;
        if distance >= 16 && distance <= at {
            copy_back::<u16, 16>(&mut self.out.buf, start, distance, length);
        } else if distance <= at {
            copy_back::<u16, 8>(&mut self.out.buf, start, distance, length);
        } else {
            self.copy_from_before(at, distance, length);
        }
        self.out.len += length;
        if at < self.clean_from + distance {
            self.clean_from = at + length;
        }
        true
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.out.make_room(bytes.len(), 4 * WINDOW_SIZE);
        let start = self.out.len;
        for (to, &b) in self.out.buf[start..start + bytes.len()].iter_mut().zip(bytes) {
            *to = u16::from(b);
        }
        self.out.len += bytes.len();
    }

    #[inline(always)]
    fn is_clean(&self) -> bool {
        self.produced() - self.clean_from >= WINDOW_SIZE
    }

    #[inline(always)]
    fn stop_at(&self, limit: usize) -> usize {
        limit.min(self.clean_from + WINDOW_SIZE)
    }

    /// The clean mark brought back to the position after the last marker
    /// among the last 32 KiB, or where they hold none to no later than their
    /// start: no
    /// marker is at or after the mark, so the last is before it. Looked for
    /// from the mark back a chunk at a time, so the compiler folds a chunk's
    /// values at once.
    fn block_ended(&mut self) {
        const CHUNK: usize = 16;
        let held = self.out.held();
        let window = held.len().saturating_sub(WINDOW_SIZE);
        let upto = self.clean_from.saturating_sub(self.out.base).min(held.len());
        if upto <= window {
            return;
        }
        let values = &held[window..upto];
        let chunks = values.rchunks_exact(CHUNK);
        let head = chunks.remainder().len();
        let mut last = None;
        for (k, chunk) in chunks.enumerate() {
            if chunk.iter().fold(0, |acc, &v| acc | v) >= MARKER {
                let i = chunk.iter().rposition(|&v| v >= MARKER).expect("a marker");
                last = Some(values.len() - (k + 1) * CHUNK + i);
                break;
            }
        }
        let last = last.or_else(|| values[..head].iter().rposition(|&v| v >= MARKER));
        self.clean_from = self.out.base + window + last.map_or(0, |i| i + 1);
    }
}

/// Output whose history is known, or not needed.
///
/// Positions count from the history's first byte, so a decode's plaintext
/// offset `i` is position `history + i`.
pub(crate) struct Plain {
    /// The bytes written, history first.
    out: Held<u8>,
    /// How many positions are history, not output.
    history: usize,
    /// The earliest position a back-reference may reach: the member's start
    /// where it began here, else the first.
    floor: usize,
}

impl Plain {
    /// Output beginning at a member's start, with no history, of a decode
    /// under `limit`.
    pub(crate) fn new(limit: usize) -> Plain {
        Plain { out: Held::new(Vec::new(), limit), history: 0, floor: 0 }
    }

    /// Output whose member's last 32 KiB before it are `window`, of a decode
    /// under `limit`.
    pub(crate) fn after(window: Vec<u8>, limit: usize) -> Plain {
        Plain { history: window.len(), out: Held::new(window, limit), floor: 0 }
    }

    /// A new member begins here: nothing before it is history.
    pub(crate) fn member_start(&mut self) {
        self.floor = self.out.base + self.out.len;
    }

    /// The byte at plaintext offset `i`, which must still be held.
    pub(crate) fn get(&self, i: usize) -> u8 {
        self.out.held()[self.history + i - self.out.base]
    }

    /// The plaintext written: only where nothing was released.
    #[cfg(test)]
    pub(crate) fn written(&self) -> &[u8] {
        assert_eq!(self.out.base, 0, "a released sink");
        &self.out.held()[self.history..]
    }

    /// The caller reads nothing before plaintext offset `i` from here on.
    pub(crate) fn release_before(&mut self, i: usize) {
        self.out.keep = self.out.keep.max(self.history + i);
    }

    /// How many bytes the sink has room for.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.out.buf.len()
    }
}

impl Sink for Plain {
    #[inline(always)]
    fn produced(&self) -> usize {
        self.out.base + self.out.len - self.history
    }

    #[inline(always)]
    fn literal(&mut self, byte: u8) {
        self.out.push(byte, 8 * WINDOW_SIZE);
    }

    #[inline(always)]
    fn copy(&mut self, distance: usize, length: usize) -> bool {
        let at = self.out.base + self.out.len;
        if distance > at - self.floor {
            return false;
        }
        self.out.make_room(length, 8 * WINDOW_SIZE);
        copy_back::<u8, 16>(&mut self.out.buf, self.out.len, distance, length);
        self.out.len += length;
        true
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.out.make_room(bytes.len(), 8 * WINDOW_SIZE);
        let start = self.out.len;
        self.out.buf[start..start + bytes.len()].copy_from_slice(bytes);
        self.out.len += bytes.len();
    }
}
