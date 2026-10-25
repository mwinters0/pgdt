//! Where a native decode writes: in 16-bit values while the history before
//! its start is unknown, in bytes once it no longer matters.

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

/// Output whose history is unknown: markers where it would be read.
pub(crate) struct Marked {
    pub(crate) out: Vec<u16>,
    /// The position after the last marker written: zero for the unknown
    /// history itself.
    clean_from: usize,
}

impl Marked {
    pub(crate) fn new() -> Marked {
        Marked { out: Vec::new(), clean_from: 0 }
    }

    /// The last 32 KiB written, as bytes: only once [`Sink::is_clean`].
    pub(crate) fn window(&self) -> Vec<u8> {
        debug_assert!(self.is_clean());
        self.out[self.out.len() - WINDOW_SIZE..].iter().map(|&v| v as u8).collect()
    }
}

impl Sink for Marked {
    fn produced(&self) -> usize {
        self.out.len()
    }

    #[inline]
    fn literal(&mut self, byte: u8) {
        self.out.push(u16::from(byte));
    }

    fn copy(&mut self, distance: usize, length: usize) -> bool {
        let at = self.out.len();
        if distance > at + WINDOW_SIZE {
            return false;
        }
        for i in 0..length {
            let value = match (at + i).checked_sub(distance) {
                Some(from) => self.out[from],
                None => MARKER + (WINDOW_SIZE + at + i - distance) as u16,
            };
            if value >= MARKER {
                self.clean_from = at + i + 1;
            }
            self.out.push(value);
        }
        true
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.out.extend(bytes.iter().map(|&b| u16::from(b)));
    }

    #[inline]
    fn is_clean(&self) -> bool {
        self.out.len() - self.clean_from >= WINDOW_SIZE
    }
}

/// Output whose history is known, or not needed.
pub(crate) struct Plain {
    pub(crate) out: Vec<u8>,
    /// How many of `out`'s first bytes are history, not output.
    pub(crate) history: usize,
    /// The earliest position in `out` a back-reference may reach: the
    /// member's start where it began in `out`, else `out`'s first byte.
    floor: usize,
}

impl Plain {
    /// Output beginning at a member's start, with no history.
    pub(crate) fn new() -> Plain {
        Plain { out: Vec::new(), history: 0, floor: 0 }
    }

    /// Output whose member's last 32 KiB before it are `window`.
    pub(crate) fn after(window: Vec<u8>) -> Plain {
        Plain { history: window.len(), out: window, floor: 0 }
    }

    /// A new member begins here: nothing before it is history.
    pub(crate) fn member_start(&mut self) {
        self.floor = self.out.len();
    }
}

impl Sink for Plain {
    fn produced(&self) -> usize {
        self.out.len() - self.history
    }

    #[inline]
    fn literal(&mut self, byte: u8) {
        self.out.push(byte);
    }

    #[inline]
    fn copy(&mut self, distance: usize, length: usize) -> bool {
        let at = self.out.len();
        if distance > at - self.floor {
            return false;
        }
        let from = at - distance;
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
        self.out.extend_from_slice(bytes);
    }
}
