//! **The geometric input functions as one walk fed a byte at a time** (I74,
//! `docs/design/decisions.md`, "D144"): the bytes are read into numbers and
//! punctuation, the blanks between them skipped as every `*_in` here skips
//! them, and the tokens handed to the type's grammar, which holds a point's
//! two coordinates at most beside the first two points of a `line`'s.
//!
//! **Two of the grammars ask whether a `(` is the last in the text**, which
//! no byte read so far answers: `path_decode` reads a lone `(` as its own
//! bracket only where it is, and `path_in` wraps its points in one only
//! then. There the walk reads on as both, a grammar each, and keeps the one
//! the text's last `(` agrees with once it ends — two at most, a grammar
//! asking it once, at its first token (`docs/design/decisions.md`, "D146").

use crate::grammar::input::Geometric;
use crate::value::geometric::{
    PATH_HEADER, POLYGON_HEADER, Point, fits_allocation, fp_zero, line_constructs, point_eq_point,
};
use crate::value::libc::{Strtod, is_c_space};
use crate::value::source::{Refused, Walked};

/// A number or a bracket, comma or the like.
#[derive(Debug, Clone, Copy)]
enum Token {
    Number(f64),
    /// The byte, and where in the text it lies.
    Mark(u8, u64),
}

impl Token {
    fn is(self, mark: u8) -> bool {
        matches!(self, Token::Mark(byte, _) if byte == mark)
    }

    /// Whether a point begins with it, as `pair_decode` reads one.
    fn opens_pair(self) -> bool {
        matches!(self, Token::Number(_)) || self.is(b'(')
    }
}

/// **A geometric field's walk**: [`GeometricWalk::byte`] each byte, then
/// [`GeometricWalk::end`] for whether its type's input function reads it.
#[derive(Debug, Clone)]
pub(crate) struct GeometricWalk {
    kind: Geometric,
    number: Strtod,
    /// Whether a number is being read.
    in_number: bool,
    /// The bytes read.
    at: u64,
    /// Where the last `(` lies.
    last_paren: Option<u64>,
    commas: u64,
    /// The grammars read on, each with what it assumes of the last `(`; none
    /// once every one has refused.
    reading: [Option<Reading>; 2],
}

/// A grammar, and where a `(` it took for the text's last, or for not the
/// last, lies.
#[derive(Debug, Clone, Copy)]
struct Reading {
    grammar: Grammar,
    assumes: Option<(u64, bool)>,
}

impl GeometricWalk {
    pub(crate) fn new(kind: Geometric) -> Self {
        let grammar = Grammar::new(kind);
        Self {
            kind,
            number: Strtod::default(),
            in_number: false,
            at: 0,
            last_paren: None,
            commas: 0,
            reading: [Some(Reading { grammar, assumes: None }), None],
        }
    }

    /// Ready the walk for another field, its number's buffer kept.
    pub(crate) fn start(&mut self) {
        let number = std::mem::take(&mut self.number);
        *self = Self { number, ..Self::new(self.kind) };
    }

    /// Read the field's next byte; `Err` once it is refused.
    pub(crate) fn byte(&mut self, byte: u8) -> Walked {
        let at = self.at;
        self.at += 1;
        if byte == b'(' {
            self.paren(at);
        }
        if self.in_number {
            if self.number.byte(byte)? {
                return Ok(());
            }
            self.in_number = false;
            let value = self.number.finish()?;
            self.token(Token::Number(value))?;
        }
        match byte {
            b'(' | b')' | b'[' | b']' | b'<' | b'>' | b'{' | b'}' | b',' => {
                self.commas += u64::from(byte == b',');
                self.token(Token::Mark(byte, at))
            }
            byte if is_c_space(byte) => Ok(()),
            byte if Strtod::starts(byte) => {
                self.in_number = true;
                self.number.byte(byte).map(drop)
            }
            _ => Err(Refused),
        }
    }

    /// The field has ended: `Ok` where its input function reads it.
    // pg-refuses: I74 — every refusal here is a geometric `*_in`'s.
    pub(crate) fn end(&mut self) -> Walked {
        if std::mem::take(&mut self.in_number) {
            let value = self.number.finish()?;
            self.token(Token::Number(value))?;
        }
        let (commas, last) = (self.commas, self.last_paren);
        let read = self.reading.iter_mut().flatten().any(|reading| {
            let holds = reading.assumes.is_none_or(|(at, is_last)| (last == Some(at)) == is_last);
            holds && reading.grammar.end(commas).is_ok()
        });
        if read { Ok(()) } else { Err(Refused) }
    }

    /// A `(` at `at`: a reading taking an earlier one for the last refused.
    fn paren(&mut self, at: u64) {
        self.last_paren = Some(at);
        for slot in &mut self.reading {
            if slot.is_some_and(|r| r.assumes.is_some_and(|(paren, last)| last && paren < at)) {
                *slot = None;
            }
        }
    }

    /// Hand `token` to every reading, forking one asking of a `(` before it.
    fn token(&mut self, token: Token) -> Walked {
        if !token.is(b'(') {
            self.fork();
        }
        for slot in &mut self.reading {
            if let Some(reading) = slot
                && reading.grammar.token(token).is_err()
            {
                *slot = None;
            }
        }
        if self.reading.iter().any(Option::is_some) { Ok(()) } else { Err(Refused) }
    }

    /// Read a reading whose grammar asks whether its `(` is the last on as
    /// both answers.
    fn fork(&mut self) {
        let [Some(reading), None] = &mut self.reading else { return };
        let Some(at) = reading.grammar.asks() else { return };
        let mut last = *reading;
        last.grammar.answer(true);
        last.assumes = Some((at, true));
        reading.grammar.answer(false);
        reading.assumes = Some((at, false));
        // Already refuted where a `(` came since.
        if self.last_paren.is_none_or(|paren| paren == at) {
            self.reading[1] = Some(last);
        }
    }
}

/// A point, `x,y`, optionally in parentheses, as `pair_decode` reads it.
#[derive(Debug, Clone, Copy, Default)]
struct Pair {
    stage: PairStage,
    delimited: bool,
    point: Point,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PairStage {
    #[default]
    Start,
    X,
    Comma,
    Y,
    /// `y` read in parentheses.
    Close,
}

impl Pair {
    /// Read `token`: the point once it is complete.
    fn token(&mut self, token: Token) -> Walked<Option<Point>> {
        self.stage = match (self.stage, token) {
            (PairStage::Start, Token::Mark(b'(', _)) => {
                self.delimited = true;
                PairStage::X
            }
            (PairStage::Start | PairStage::X, Token::Number(x)) => {
                self.point.x = x;
                PairStage::Comma
            }
            (PairStage::Comma, Token::Mark(b',', _)) => PairStage::Y,
            (PairStage::Y, Token::Number(y)) => {
                self.point.y = y;
                if !self.delimited {
                    return Ok(Some(std::mem::take(self).point));
                }
                PairStage::Close
            }
            (PairStage::Close, Token::Mark(b')', _)) => {
                return Ok(Some(std::mem::take(self).point));
            }
            _ => return Err(Refused),
        };
        Ok(None)
    }
}

/// **`path_decode`**: points, each followed by an optional `,`, opened by
/// `[` where `open` allows it, or by a `(` holding them all — the first of
/// two, or the text's last `(` — and closed to match. `count` is the number
/// of points it reads exactly, or, unstated, the number its caller makes of
/// the text's commas once it ends.
#[derive(Debug, Clone, Copy)]
struct Points {
    open: bool,
    count: Option<u64>,
    stage: PointsStage,
    /// Whether a bracket opened them, `[` where `square`.
    bracketed: bool,
    square: bool,
    read: u64,
    first: [Point; 2],
    pair: Pair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointsStage {
    Start,
    /// A `(` read first, at the place given: the first of two, or not.
    Paren(u64),
    /// A point must follow.
    Point,
    InPoint,
    /// A point read, a `,` not yet.
    Read,
    /// A point and its `,` read.
    Separated,
    /// The bracket closed; the token after is the caller's.
    Closed,
}

/// What [`Points::token`] made of a token.
#[derive(Debug, PartialEq, Eq)]
enum Fed {
    Taken,
    /// The points ended before it, which is the caller's.
    Ended,
}

impl Points {
    fn new(open: bool, count: Option<u64>) -> Self {
        Self {
            open,
            count,
            stage: PointsStage::Start,
            bracketed: false,
            square: false,
            read: 0,
            first: [Point::default(); 2],
            pair: Pair::default(),
        }
    }

    fn token(&mut self, token: Token) -> Walked<Fed> {
        match (self.stage, token) {
            (PointsStage::Start, Token::Mark(b'[', _)) if self.open => {
                (self.bracketed, self.square) = (true, true);
                self.stage = PointsStage::Point;
            }
            (PointsStage::Start, Token::Mark(b'(', at)) => self.stage = PointsStage::Paren(at),
            // The first of two.
            (PointsStage::Paren(_), Token::Mark(b'(', _)) => {
                self.bracketed = true;
                self.point(token)?;
            }
            (PointsStage::Start | PointsStage::Point, _) => self.point(token)?,
            (PointsStage::InPoint, _) => {
                if let Some(point) = self.pair.token(token)? {
                    if let Some(slot) = self.first.get_mut(self.read as usize) {
                        *slot = point;
                    }
                    self.read += 1;
                    self.stage = PointsStage::Read;
                }
            }
            (PointsStage::Read, Token::Mark(b',', _)) => self.stage = PointsStage::Separated,
            (PointsStage::Read | PointsStage::Separated, token) if token.opens_pair() => {
                self.point(token)?;
            }
            (PointsStage::Read | PointsStage::Separated, Token::Mark(close, _))
                if self.bracketed && (close == b')' || (close == b']' && self.square)) =>
            {
                self.complete()?;
                self.stage = PointsStage::Closed;
            }
            (PointsStage::Read | PointsStage::Separated, _) if !self.bracketed => {
                self.complete()?;
                return Ok(Fed::Ended);
            }
            (PointsStage::Closed, _) => return Ok(Fed::Ended),
            _ => return Err(Refused),
        }
        Ok(Fed::Taken)
    }

    /// Whether the `(` read first is the text's last, a question asked once.
    fn asks(&self) -> Option<u64> {
        match self.stage {
            PointsStage::Paren(at) => Some(at),
            _ => None,
        }
    }

    /// The `(` read first is the text's last where `last`, so the points'
    /// bracket, or else the first point's own.
    fn answer(&mut self, last: bool) {
        if last {
            self.bracketed = true;
            self.stage = PointsStage::Point;
        } else {
            self.stage = PointsStage::InPoint;
            self.pair = Pair::default();
            let _ = self.pair.token(Token::Mark(b'(', 0));
        }
    }

    /// Begin a point with `token`, past the count where it is stated.
    fn point(&mut self, token: Token) -> Walked {
        if self.count.is_some_and(|count| self.read == count) {
            return Err(Refused);
        }
        self.pair = Pair::default();
        self.stage = PointsStage::InPoint;
        self.pair.token(token)?;
        Ok(())
    }

    /// The points have all been read, as many as stated.
    fn complete(&self) -> Walked {
        if self.count.is_some_and(|count| self.read != count) { Err(Refused) } else { Ok(()) }
    }

    /// The text ended here: `Ok` where the points are whole.
    fn end(&self) -> Walked {
        match self.stage {
            PointsStage::Closed => Ok(()),
            PointsStage::Read | PointsStage::Separated if !self.bracketed => self.complete(),
            _ => Err(Refused),
        }
    }

    /// `pair_count`: the points `commas` say a path or polygon holds, half
    /// of them rounded up, where they are odd — and so many read.
    fn counted(&self, commas: u64, header: usize) -> Walked {
        let count = (commas % 2 == 1).then(|| commas.div_ceil(2)).ok_or(Refused)?;
        let fits = usize::try_from(count).is_ok_and(|count| fits_allocation(header, count));
        if fits && self.read == count { Ok(()) } else { Err(Refused) }
    }
}

/// One type's grammar over the tokens.
#[derive(Debug, Clone, Copy)]
enum Grammar {
    Point {
        pair: Pair,
        read: bool,
    },
    /// `lseg_in`, `box_in`, `poly_in`, and `line_in`'s two points: points
    /// and nothing after them.
    Points {
        points: Points,
        kind: Geometric,
    },
    /// `path_in`: its points, wrapped in one more `(…)` where `wrap` says.
    Path {
        points: Points,
        wrap: Wrap,
    },
    Circle(Circle),
    /// `line_in` before its first token says which form.
    Line,
    /// `line_in`'s `{A,B,C}`: so many coefficients read, and whether closed.
    Coefficients {
        read: usize,
        values: [f64; 3],
        comma: bool,
        closed: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrap {
    Start,
    /// A `(` first, at the place given.
    Paren(u64),
    Unwrapped,
    Wrapped,
    Closed,
}

impl Grammar {
    fn new(kind: Geometric) -> Self {
        let points = |open, count| Grammar::Points { points: Points::new(open, count), kind };
        match kind {
            Geometric::Point => Grammar::Point { pair: Pair::default(), read: false },
            Geometric::Lseg => points(true, Some(2)),
            Geometric::Box => points(false, Some(2)),
            Geometric::Polygon => points(false, None),
            Geometric::Path => Grammar::Path { points: Points::new(true, None), wrap: Wrap::Start },
            Geometric::Circle => Grammar::Circle(Circle::default()),
            Geometric::Line => Grammar::Line,
        }
    }

    fn token(&mut self, token: Token) -> Walked {
        match self {
            Grammar::Point { read: true, .. } => return Err(Refused),
            Grammar::Point { pair, read } => *read = pair.token(token)?.is_some(),
            Grammar::Points { points, .. } => {
                if points.token(token)? == Fed::Ended {
                    return Err(Refused);
                }
            }
            Grammar::Path { points, wrap } => match (*wrap, token) {
                (Wrap::Start, Token::Mark(b'(', at)) => *wrap = Wrap::Paren(at),
                // Not the last `(`, so the points' own.
                (Wrap::Paren(at), Token::Mark(b'(', _)) => {
                    *wrap = Wrap::Unwrapped;
                    points.token(Token::Mark(b'(', at))?;
                    points.token(token)?;
                }
                (Wrap::Start, _) => {
                    *wrap = Wrap::Unwrapped;
                    points.token(token)?;
                }
                (Wrap::Unwrapped, _) => {
                    if points.token(token)? == Fed::Ended {
                        return Err(Refused);
                    }
                }
                (Wrap::Wrapped, _) => {
                    if points.token(token)? == Fed::Ended {
                        if !token.is(b')') {
                            return Err(Refused);
                        }
                        *wrap = Wrap::Closed;
                    }
                }
                _ => return Err(Refused),
            },
            Grammar::Circle(circle) => circle.token(token)?,
            Grammar::Line if token.is(b'{') => {
                *self = Grammar::Coefficients {
                    read: 0,
                    values: [0.0; 3],
                    comma: false,
                    closed: false,
                };
            }
            Grammar::Line => {
                *self =
                    Grammar::Points { points: Points::new(true, Some(2)), kind: Geometric::Line };
                return self.token(token);
            }
            Grammar::Coefficients { read, values, comma, closed } => match token {
                _ if *closed => return Err(Refused),
                Token::Number(value) if *read < 3 && (*read == 0 || *comma) => {
                    values[*read] = value;
                    *read += 1;
                    *comma = false;
                }
                Token::Mark(b',', _) if (1..3).contains(read) && !*comma => *comma = true,
                Token::Mark(b'}', _) if *read == 3 => *closed = true,
                _ => return Err(Refused),
            },
        }
        Ok(())
    }

    /// Where a `(` this grammar read first lies, while it asks whether it is
    /// the text's last.
    fn asks(&self) -> Option<u64> {
        match self {
            Grammar::Points { points, .. } => points.asks(),
            Grammar::Path { wrap: Wrap::Paren(at), .. } => Some(*at),
            _ => None,
        }
    }

    /// Its `(` is the text's last where `last`.
    fn answer(&mut self, last: bool) {
        match self {
            Grammar::Points { points, .. } => points.answer(last),
            Grammar::Path { points, wrap } => {
                let Wrap::Paren(at) = *wrap else { return };
                if last {
                    *wrap = Wrap::Wrapped;
                } else {
                    *wrap = Wrap::Unwrapped;
                    // The points' `(`, then not the last: the first point's.
                    let _ = points.token(Token::Mark(b'(', at));
                    points.answer(false);
                }
            }
            _ => {}
        }
    }

    /// The text has ended, holding `commas` commas: `Ok` where the type's
    /// input function reads it.
    fn end(&self, commas: u64) -> Walked {
        let read = match self {
            Grammar::Point { read, .. } => *read,
            Grammar::Points { points, kind } => {
                points.end()?;
                match kind {
                    Geometric::Polygon => points.counted(commas, POLYGON_HEADER).is_ok(),
                    Geometric::Line => {
                        let [p, q] = points.first;
                        !point_eq_point(p, q)
                            && (line_constructs(p, q, false) || line_constructs(p, q, true))
                    }
                    _ => true,
                }
            }
            Grammar::Path { points, wrap } => {
                let closed = match wrap {
                    Wrap::Unwrapped => points.end().is_ok(),
                    Wrap::Closed => true,
                    _ => false,
                };
                closed && points.counted(commas, PATH_HEADER).is_ok()
            }
            Grammar::Circle(circle) => circle.end(),
            Grammar::Line => false,
            Grammar::Coefficients { values, closed, .. } => {
                *closed && !(fp_zero(values[0]) && fp_zero(values[1]))
            }
        };
        if read { Ok(()) } else { Err(Refused) }
    }
}

/// `circle_in`: a center and a radius, in `<…>`, `(…)` — the first of two
/// `(` — or bare.
#[derive(Debug, Clone, Copy, Default)]
struct Circle {
    stage: CircleStage,
    bracketed: bool,
    pair: Pair,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum CircleStage {
    #[default]
    Start,
    Paren,
    Center,
    /// The center read; an optional `,`, then the radius.
    Centered,
    Radius,
    /// The radius read: the closing bracket, where one opened.
    Closing,
    Closed,
}

impl Circle {
    fn token(&mut self, token: Token) -> Walked {
        self.stage = match (self.stage, token) {
            (CircleStage::Start, Token::Mark(b'<', _)) => {
                self.bracketed = true;
                CircleStage::Center
            }
            (CircleStage::Start, Token::Mark(b'(', _)) => CircleStage::Paren,
            (CircleStage::Paren, token) => {
                // The first of two is the circle's; a lone one the center's.
                self.bracketed = token.is(b'(');
                self.pair.token(Token::Mark(b'(', 0))?;
                if !self.bracketed {
                    self.pair.token(token)?;
                }
                CircleStage::Center
            }
            (CircleStage::Start | CircleStage::Center, token) => {
                if self.pair.token(token)?.is_some() {
                    CircleStage::Centered
                } else {
                    CircleStage::Center
                }
            }
            (CircleStage::Centered, Token::Mark(b',', _)) => CircleStage::Radius,
            (CircleStage::Centered | CircleStage::Radius, Token::Number(radius)) => {
                // A NaN radius is read.
                if radius < 0.0 {
                    return Err(Refused);
                }
                CircleStage::Closing
            }
            (CircleStage::Closing, Token::Mark(b')' | b'>', _)) if self.bracketed => {
                CircleStage::Closed
            }
            _ => return Err(Refused),
        };
        Ok(())
    }

    fn end(&self) -> bool {
        match self.stage {
            CircleStage::Closing => !self.bracketed,
            CircleStage::Closed => true,
            _ => false,
        }
    }
}
