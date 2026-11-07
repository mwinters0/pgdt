//! **What an input grammar is fed**: a text a byte at a time, as a field's
//! runs hand it over, with no byte after it to look at — an element, a field,
//! a bound handed on to its caller as the container's grammar reads it, so a
//! walk holds no copy of the text it reads, a range's two bounds aside, and
//! carries across a run's end only a fixed state of its own
//! (`docs/design/decisions.md`, "D143", "D144").

/// **A literal one of these grammars refuses**, wherever in it the walk
/// found that — or a part its caller read and refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refused;

/// What a reader answers: [`Refused`] stops the walk.
pub(crate) type Walked<T = ()> = Result<T, Refused>;

/// **What a container's walk hands its caller of each part**, `K` naming
/// which: a part's beginning, its own text a byte at a time as its
/// container's grammar reads it, and its end — or SQL NULL in its place.
pub(crate) trait Parts<K: Copy = ()> {
    /// Part `k` begins; its bytes follow.
    fn begin(&mut self, k: K) -> Walked;
    /// The current part's next byte.
    fn byte(&mut self, byte: u8) -> Walked;
    /// The current part has ended.
    fn end(&mut self) -> Walked;
    /// Part `k` is SQL NULL.
    fn null(&mut self, k: K) -> Walked;
}

/// **A whitespace run carried as its bytes' run lengths**, where a reader
/// learns whether it is a part's interior or its trailing blanks only at the
/// byte after it: a run of one byte is one count however long.
// deficiency: KD167 — a run alternating its bytes is a count per change, so a
// field written as one unquoted element whose whitespace alternates `' '`
// and `'\t'` carries state growing with it: no `pg_dump` writes one, `array_out`
// quoting every element holding whitespace. Bounded by reading the run on
// into the part and rewinding it where it turns out trailing, which every
// part's reader would offer. **(c) unowned**; promoted by such a field met in use.
#[derive(Debug, Default, Clone)]
pub(crate) struct Blanks {
    runs: Vec<(u8, u32)>,
}

impl Blanks {
    pub(crate) fn push(&mut self, byte: u8) {
        match self.runs.last_mut() {
            Some((last, count)) if *last == byte && *count < u32::MAX => *count += 1,
            _ => self.runs.push((byte, 1)),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.runs.clear();
    }

    /// Hand the run to `to`, in order, being a part's interior.
    pub(crate) fn flush(&mut self, to: &mut dyn FnMut(u8) -> Walked) -> Walked {
        for (byte, count) in self.runs.drain(..) {
            for _ in 0..count {
                to(byte)?;
            }
        }
        Ok(())
    }
}

/// **Letters read against a keyword in any ASCII case**, as `NULL`, `empty`
/// and the like are matched: so many read, all matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Word {
    word: &'static [u8],
    read: usize,
}

impl Word {
    pub(crate) fn new(word: &'static [u8]) -> Self {
        Self { word, read: 0 }
    }

    /// Read `byte` as the keyword's next letter, answering whether it is.
    pub(crate) fn letter(&mut self, byte: u8) -> bool {
        let matches = self.word.get(self.read).is_some_and(|l| byte.eq_ignore_ascii_case(l));
        self.read += usize::from(matches);
        matches
    }

    /// Whether every letter has been read.
    pub(crate) fn done(&self) -> bool {
        self.read == self.word.len()
    }

    /// How many letters have been read.
    pub(crate) fn read(&self) -> usize {
        self.read
    }
}
