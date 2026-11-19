//! **What an input grammar is fed**: a text a byte at a time, as a field's
//! runs hand it over, with no byte after it to look at — an element, a field,
//! a bound handed on to its caller as the container's grammar reads it, so a
//! walk holds no copy of the text it reads, a range's two bounds aside, and
//! carries across a run's end only a fixed state of its own
//! (`docs/design/decisions.md`, "D143", "D144").
//!
//! It is fed, never pulling its source's next chunk itself: a codec walk
//! driving `pgdt-dump`'s reader would sit on its tab split, CR hold and chunk
//! retention (`docs/design/decisions.md`, "D74").

/// **A literal one of these grammars refuses**, wherever in it the walk
/// found that — or a part its caller read and refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refused;

/// What a reader answers: [`Refused`] stops the walk.
pub(crate) type Walked<T = ()> = Result<T, Refused>;

/// **What a container's walk hands its caller of each part**, `K` naming
/// which: a part's beginning, its own text a byte at a time as its
/// container's grammar reads it, and its end — or SQL NULL in its place.
///
/// **A whitespace run whose end decides whether it is the part's** — an
/// unquoted array element's, interior or trailing, or one after `NULL`,
/// which the byte after it makes a marker or a text — is fed on into the
/// part from a [`Parts::mark`] and rewound to it where it is not, so the
/// walk holds nothing of the run, and each implementor restores its own
/// state, or truncates what it collected, bounded by what it held at the
/// mark rather than by the run (`docs/design/decisions.md`, "D145").
pub(crate) trait Parts<K: Copy = ()> {
    /// Part `k` begins; its bytes follow.
    fn begin(&mut self, k: K) -> Walked;
    /// The current part's next byte.
    fn byte(&mut self, byte: u8) -> Walked;
    /// The current part has ended.
    fn end(&mut self) -> Walked;
    /// Part `k` is SQL NULL.
    fn null(&mut self, k: K) -> Walked;
    /// Remember where the current part stands — or, between parts, that
    /// none has begun — a whitespace run being fed on from here.
    fn mark(&mut self);
    /// Return to the last [`Parts::mark`], the run fed since turning out not
    /// to be the part's: what it fed forgotten, and a part begun since
    /// abandoned, a `null` following in its place. A refusal the run met
    /// goes with it.
    fn rewind(&mut self);
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
