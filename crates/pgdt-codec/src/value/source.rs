//! A text read forward a byte at a time, as a walk over a literal reads one:
//! a field's bytes as `COPY`'s escapes stand for them, or one part of a
//! literal as its container's grammar reads it — an element, a field, a
//! bound — so a walk holds no copy of the text it reads, a range's two
//! bounds aside
//! (`docs/design/decisions.md`, "D143").

/// **A text read forward a byte at a time**, with one byte of lookahead and
/// a fork for more.
pub(crate) trait Source<'a> {
    /// The next byte, read; `None` past the last.
    fn next(&mut self) -> Option<u8>;

    /// The next byte, left to be read.
    fn peek(&mut self) -> Option<u8>;

    /// A source reading on from here apart from this one, which it leaves
    /// where it is.
    fn fork(&self) -> Box<dyn Source<'a> + 'a>;

    /// Read the next byte where it is `byte`.
    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.next();
        }
        found
    }

    /// Read every byte up to the next one `keep` does not hold.
    fn skip_while(&mut self, keep: fn(u8) -> bool) {
        while self.peek().is_some_and(keep) {
            self.next();
        }
    }

    /// Read the next `word.len()` bytes, answering whether they spell `word`
    /// in any ASCII case.
    fn eat_word(&mut self, word: &[u8]) -> bool {
        let mut spelled = true;
        for expected in word {
            spelled &= self.next().is_some_and(|byte| byte.eq_ignore_ascii_case(expected));
        }
        spelled
    }
}

/// The bytes a source has left, read as an iterator.
pub(crate) fn bytes<'s, 'a>(source: &'s mut dyn Source<'a>) -> impl Iterator<Item = u8> + 's {
    std::iter::from_fn(move || source.next())
}

/// A text's bytes, every one read into a `String` — `None` where they are
/// not UTF-8.
pub(crate) fn collect(source: &mut dyn Source<'_>) -> Option<String> {
    String::from_utf8(bytes(source).collect()).ok()
}

/// **A byte iterator as a source**: a `&str`'s bytes, or a field's as
/// `pgdt_core::Unescaped` reads them, peeked and forked by cloning it.
#[derive(Clone)]
pub(crate) struct Bytes<I>(pub(crate) I);

impl<'a, I: Iterator<Item = u8> + Clone + 'a> Source<'a> for Bytes<I> {
    fn next(&mut self) -> Option<u8> {
        self.0.next()
    }

    fn peek(&mut self) -> Option<u8> {
        self.0.clone().next()
    }

    fn fork(&self) -> Box<dyn Source<'a> + 'a> {
        Box::new(self.clone())
    }
}

/// What a [`Lexer`] reads next of its part.
pub(crate) enum Step {
    Byte(u8),
    /// The part has ended where its grammar ends it.
    End,
    /// The text ended, or held a byte its grammar refuses, inside the part.
    Fail,
}

/// **One part's grammar inside its container's**: which bytes of the
/// container's text are the part's, and what each stands for.
pub(crate) trait Lexer: Clone {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step;
}

/// **The source a [`Part`] reads its container's text off**: borrowed by
/// the walk reading the container, or owned by a fork of the part.
pub(crate) trait Outer<'a> {
    fn source(&mut self) -> &mut dyn Source<'a>;
    fn fork(&self) -> Box<dyn Source<'a> + 'a>;
}

impl<'a> Outer<'a> for &mut (dyn Source<'a> + '_) {
    fn source(&mut self) -> &mut dyn Source<'a> {
        &mut **self
    }

    fn fork(&self) -> Box<dyn Source<'a> + 'a> {
        (**self).fork()
    }
}

impl<'a> Outer<'a> for Box<dyn Source<'a> + 'a> {
    fn source(&mut self) -> &mut dyn Source<'a> {
        &mut **self
    }

    fn fork(&self) -> Box<dyn Source<'a> + 'a> {
        (**self).fork()
    }
}

/// **One part of a literal, read as its container's grammar reads it**, off
/// the container's own source, which it reads on past the part only as far
/// as its [`Lexer`] looks.
pub(crate) struct Part<O, L> {
    outer: O,
    lexer: L,
    peeked: Option<Option<u8>>,
    /// Where the part ended, `true` where it failed ([`Step::Fail`]).
    ended: Option<bool>,
}

impl<'a, O: Outer<'a>, L: Lexer + 'a> Part<O, L> {
    pub(crate) fn new(outer: O, lexer: L) -> Self {
        Self { outer, lexer, peeked: None, ended: None }
    }

    fn advance(&mut self) -> Option<u8> {
        if self.ended.is_some() {
            return None;
        }
        match self.lexer.step(self.outer.source()) {
            Step::Byte(byte) => Some(byte),
            Step::End => {
                self.ended = Some(false);
                None
            }
            Step::Fail => {
                self.ended = Some(true);
                None
            }
        }
    }

    /// Read the part to its end, answering whether its grammar read it
    /// there: the container's source is then just past it.
    pub(crate) fn finish(&mut self) -> bool {
        while Source::next(self).is_some() {}
        self.ended == Some(false)
    }
}

impl<'a, O: Outer<'a>, L: Lexer + 'a> Source<'a> for Part<O, L> {
    fn next(&mut self) -> Option<u8> {
        match self.peeked.take() {
            Some(byte) => byte,
            None => self.advance(),
        }
    }

    fn peek(&mut self) -> Option<u8> {
        if self.peeked.is_none() {
            self.peeked = Some(self.advance());
        }
        self.peeked.flatten()
    }

    fn fork(&self) -> Box<dyn Source<'a> + 'a> {
        Box::new(Part::<Box<dyn Source<'a> + 'a>, L> {
            outer: self.outer.fork(),
            lexer: self.lexer.clone(),
            peeked: self.peeked,
            ended: self.ended,
        })
    }
}
