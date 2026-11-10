//! **A strict check fed its field a run at a time**: a field of a column
//! whose plan [`walks`](super::check::walks), its bytes as `COPY` wrote them
//! unescaped as they come and handed through its type's input grammar — a
//! nested value's walked through each container's, every part checked as a
//! field of its own type in its turn — with nothing held past a run's end
//! but each reader's own state (`docs/design/decisions.md`, "D143",
//! "D144"), a type ordered by nothing by its [`TextGrammar`]'s walk among
//! them. Beside it, what reads a part whole: a range, by its bounds' keys
//! ([`checked_key`]).

use pgdt_core::internal::Unescape;

use crate::compare::{CompareKind, ComparisonDivergence, ComparisonPlan, Discrete, NestedCompare};
use crate::declared::Position;
use crate::decode::{Read, Unread};
use crate::grammar::input::{Geometric, TextGrammar};
use crate::key::order::{OrderKey, field_key};
use crate::nested::RangeLiteral;
use crate::value::bit::BitWalk;
use crate::value::bytea::ByteaFold;
use crate::value::geometric::GeometricWalk;
use crate::value::json::{JsonMark, JsonWalk, json_in};
use crate::value::nested;
use crate::value::nested::key::{
    NestedKey, RangeBoundKey, RangeKey, canonical_multirange, make_range,
};
use crate::value::nested::{
    ArrayWalk, Int2VectorWalk, MultirangeMark, MultirangeWalk, RangeMark, RangeWalk, RecordWalk,
};
use crate::value::source::{Parts, Refused, Walked};
use crate::value::text::TextFold;

/// **Whether a strict check refuses one field**, fed a run of its bytes at a
/// time — as `COPY` wrote them ([`FieldWalk::run`]) or already unescaped
/// ([`FieldWalk::text`]) — and answered once it ends ([`FieldWalk::finish`]):
/// [`field_refused`](super::check::field_refused)'s answer, with no copy of
/// the field and no tree of it held, but a scalar part's text and a range's
/// bounds, each in its turn.
#[derive(Debug)]
pub struct FieldWalk {
    escape: Unescape,
    check: Check,
    /// Whether the text is UTF-8: a field that is not is no value, which no
    /// `*_in` is handed.
    text: TextFold,
    refused: bool,
}

impl FieldWalk {
    /// A walk of a field of a column read by `grammar` in place of its plan,
    /// which orders it by nothing
    /// ([`column_grammar`](super::check::column_grammar)).
    pub fn grammar(grammar: TextGrammar) -> Self {
        FieldWalk::of(Check::grammar(grammar))
    }

    fn of(check: Check) -> Self {
        Self { escape: Unescape::default(), check, text: TextFold::default(), refused: false }
    }

    /// A walk of a field of a column compared by `plan`, whose plan
    /// [`walks`](super::check::walks).
    pub fn new(plan: &ComparisonPlan) -> Self {
        let check = match plan {
            // `AS_TEXT`'s one member is `json`.
            ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. } => {
                Check::Json { walk: JsonWalk::new(false), jsonb: false }
            }
            ComparisonPlan::Compared { kind, .. } => Check::leaf(kind),
            ComparisonPlan::Nested(nested) => Check::of(nested),
            ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_) => Check::Nothing,
        };
        Self::of(check)
    }

    /// Read the field's next run of bytes, as `COPY` wrote them.
    pub fn run(&mut self, escaped: &[u8]) {
        let mut escape = self.escape;
        escape.run(escaped, |text| self.text(text));
        self.escape = escape;
    }

    /// Read the field's next run of text, already unescaped.
    pub fn text(&mut self, text: &[u8]) {
        if matches!(self.check, Check::Nothing) {
            return;
        }
        // Read on past a refusal, which answers only of a text.
        self.text.push(text);
        if !self.refused {
            self.refused = self.check.run(text).is_err();
        }
    }

    /// The field has ended: whether its check refuses it, it being UTF-8.
    /// The walk is readied for its column's next field, so a reader keeps
    /// one per column, built once ([`Self::start`]).
    pub fn finish(&mut self) -> bool {
        let mut escape = self.escape;
        escape.finish(|text| self.text(text));
        let refused = !matches!(self.check, Check::Nothing)
            && (self.refused || self.check.end().is_err())
            && std::mem::take(&mut self.text).finish().is_some();
        self.start();
        refused
    }

    /// Ready the walk for another field, whatever it read of the last.
    pub fn start(&mut self) {
        self.escape = Unescape::default();
        self.check.start();
        self.text = TextFold::default();
        self.refused = false;
    }
}

/// **One part's check**, as its plan's position reads it.
#[derive(Debug)]
enum Check {
    /// A position whose check refuses nothing, which reads none of it.
    Nothing,
    /// A text held to its typmod by its characters.
    Typmod {
        length: u32,
        text: TextFold,
    },
    Bytea(ByteaFold),
    /// `json`'s grammar, and with `jsonb` `jsonb_in`'s refusals beside it.
    Json {
        walk: JsonWalk,
        jsonb: bool,
    },
    /// Any other scalar, collected for [`field_key`]: its own text, no
    /// longer than it.
    Scalar {
        kind: CompareKind,
        text: Vec<u8>,
    },
    /// A type ordered by nothing, read by its [`TextGrammar`]'s walk.
    Grammar(Grammar),
    /// A type whose input function refuses every value, or none.
    Refuses(bool),
    Array(Box<(ArrayWalk, Element)>),
    Int2Vector(Int2VectorWalk),
    Record(Box<(RecordWalk, Fields)>),
    /// A range, whose two bounds are what `range_in` compares.
    Range {
        walk: RangeWalk,
        bound: Box<NestedCompare>,
        discrete: Option<Discrete>,
    },
    /// `multirange_in` refuses nothing past its members' `range_in`.
    Multirange {
        walk: MultirangeWalk,
        bound: Box<NestedCompare>,
        discrete: Option<Discrete>,
    },
}

impl Check {
    /// A scalar position compared as `kind`: a text held to its typmod, a
    /// `bytea` and a `jsonb` read a byte at a time, and any other collected.
    fn leaf(kind: &CompareKind) -> Self {
        match kind {
            CompareKind::Text { length, .. } | CompareKind::PaddedText { length } => match length {
                Some(length) => Check::Typmod { length: *length, text: TextFold::default() },
                None => Check::Nothing,
            },
            CompareKind::Bytea => Check::Bytea(ByteaFold::default()),
            CompareKind::Jsonb => Check::Json { walk: JsonWalk::new(true), jsonb: true },
            kind => Check::Scalar { kind: kind.clone(), text: Vec::new() },
        }
    }

    /// A position planned as `plan`: a scalar's its kind's, and a
    /// container's its input grammar's walk, each of its parts read by its
    /// own position's check. A position reading no type this build reads by
    /// — another held as its text, an array of a delimiter this build does
    /// not split at — refuses nothing.
    fn of(plan: &NestedCompare) -> Self {
        match plan {
            NestedCompare::Leaf { kind, .. } => Check::leaf(kind),
            // `json`, the one position diverging `AsText` (`NestedCompare::Uncomparable`).
            NestedCompare::Uncomparable {
                divergence: Some(ComparisonDivergence::AsText), ..
            } => Check::Json { walk: JsonWalk::new(false), jsonb: false },
            NestedCompare::Uncomparable { grammar: Some(grammar), .. } => Check::grammar(*grammar),
            NestedCompare::Uncomparable { .. } => Check::Nothing,
            NestedCompare::Array { element, .. } => match array_delimiter(&element.plan) {
                Some(delimiter) => {
                    Check::Array(Box::new((ArrayWalk::new(delimiter), Element::of(element))))
                }
                None => Check::Nothing,
            },
            NestedCompare::Int2Vector { .. } => Check::Int2Vector(Int2VectorWalk::default()),
            NestedCompare::Record { fields, .. } => {
                let walk = RecordWalk::new(fields.len());
                let fields = fields.iter().map(|(_, position)| Element::of(position)).collect();
                Check::Record(Box::new((walk, Fields { fields, at: 0, marked: 0 })))
            }
            NestedCompare::Range { bound, discrete, .. } => {
                let bound = bound.clone();
                Check::Range { walk: RangeWalk::default(), bound, discrete: *discrete }
            }
            NestedCompare::Multirange { bound, discrete, .. } => {
                let bound = bound.clone();
                Check::Multirange { walk: MultirangeWalk::default(), bound, discrete: *discrete }
            }
        }
    }

    /// A position read by `grammar`: a `box[]` an array of `box`, split at
    /// its `;` (I22).
    fn grammar(grammar: TextGrammar) -> Self {
        let walk = match grammar {
            TextGrammar::Bit { varying, length } => Grammar::Bit(BitWalk::new(varying, length)),
            TextGrammar::Geometric(kind) => Grammar::Geometric(Box::new(GeometricWalk::new(kind))),
            TextGrammar::BoxArray { not_null } => {
                let element = Check::grammar(TextGrammar::Geometric(Geometric::Box));
                let element = Element { check: element, not_null, mark: None };
                return Check::Array(Box::new((ArrayWalk::new(b';'), element)));
            }
            TextGrammar::RefusesAll => return Check::Refuses(true),
            TextGrammar::RefusesNothing => return Check::Refuses(false),
        };
        Check::Grammar(walk)
    }

    /// Ready the check for another part.
    fn start(&mut self) {
        match self {
            Check::Nothing => {}
            Check::Typmod { text, .. } => *text = TextFold::default(),
            Check::Bytea(fold) => *fold = ByteaFold::default(),
            Check::Json { walk, .. } => walk.start(),
            Check::Scalar { text, .. } => text.clear(),
            Check::Grammar(walk) => walk.start(),
            Check::Refuses(_) => {}
            Check::Array(array) => array.0.start(),
            Check::Int2Vector(walk) => walk.start(),
            Check::Record(record) => record.0.start(),
            Check::Range { walk, .. } => walk.start(),
            Check::Multirange { walk, .. } => walk.start(),
        }
    }

    /// What a whitespace run fed from here can move, for
    /// [`Check::rewind`]: bounded by the plan, a collected text by its
    /// length.
    fn mark(&self) -> Mark {
        match self {
            Check::Nothing => Mark::Nothing,
            Check::Typmod { text, .. } => Mark::Typmod(text.clone()),
            Check::Bytea(fold) => Mark::Bytea(*fold),
            Check::Json { walk, .. } => Mark::Json(walk.mark()),
            Check::Scalar { text, .. } => Mark::Collected(text.len()),
            Check::Grammar(walk) => Mark::Grammar(walk.clone()),
            Check::Refuses(_) => Mark::Nothing,
            Check::Array(array) => Mark::Array(Box::new((array.0.clone(), array.1.check.mark()))),
            Check::Int2Vector(walk) => Mark::Int2Vector(*walk),
            Check::Record(record) => {
                let Fields { fields, at, .. } = &record.1;
                let field = fields.get(*at).map(|field| field.check.mark());
                Mark::Record(Box::new((record.0.clone(), *at, field)))
            }
            Check::Range { walk, .. } => Mark::Range(walk.mark()),
            Check::Multirange { walk, .. } => Mark::Multirange(walk.mark()),
        }
    }

    /// Return to `mark`, taken of this check. A part a container begins
    /// inside the run is begun again after it and readied then, so only the
    /// part it was reading at the mark is restored; and a walk nested in
    /// that part is restored to whether it was itself in a run, so a mark
    /// it took inside this one is never rewound to.
    fn rewind(&mut self, mark: &Mark) {
        match (self, mark) {
            (Check::Nothing, Mark::Nothing) => {}
            (Check::Typmod { text, .. }, Mark::Typmod(at)) => text.clone_from(at),
            (Check::Bytea(fold), Mark::Bytea(at)) => *fold = *at,
            (Check::Json { walk, .. }, Mark::Json(at)) => walk.rewind(*at),
            (Check::Scalar { text, .. }, Mark::Collected(at)) => text.truncate(*at),
            (Check::Grammar(walk), Mark::Grammar(at)) => walk.clone_from(at),
            (Check::Refuses(_), Mark::Nothing) => {}
            (Check::Array(array), Mark::Array(at)) => {
                array.0.clone_from(&at.0);
                array.1.check.rewind(&at.1);
            }
            (Check::Int2Vector(walk), Mark::Int2Vector(at)) => *walk = *at,
            (Check::Record(record), Mark::Record(at)) => {
                let (walk, field, check) = &**at;
                record.0.clone_from(walk);
                record.1.at = *field;
                if let (Some(element), Some(check)) = (record.1.fields.get_mut(*field), check) {
                    element.check.rewind(check);
                }
            }
            (Check::Range { walk, .. }, Mark::Range(at)) => walk.rewind(*at),
            (Check::Multirange { walk, .. }, Mark::Multirange(at)) => walk.rewind(*at),
            _ => unreachable!("a mark is rewound into the check it was taken of"),
        }
    }

    /// Read the part's next run.
    fn run(&mut self, run: &[u8]) -> Walked {
        match self {
            Check::Nothing => Ok(()),
            Check::Typmod { text, .. } => {
                text.push(run);
                Ok(())
            }
            Check::Scalar { text: collected, .. } => {
                collected.extend_from_slice(run);
                Ok(())
            }
            check => run.iter().try_for_each(|&byte| check.byte(byte)),
        }
    }

    /// Read the part's next byte.
    fn byte(&mut self, byte: u8) -> Walked {
        match self {
            Check::Nothing => Ok(()),
            Check::Typmod { text, .. } => {
                text.push(&[byte]);
                Ok(())
            }
            Check::Bytea(fold) => {
                fold.byte(byte, &mut |_| {});
                Ok(())
            }
            Check::Json { walk, .. } => walk.byte(byte),
            Check::Scalar { text, .. } => {
                text.push(byte);
                Ok(())
            }
            Check::Grammar(walk) => walk.byte(byte),
            Check::Refuses(_) => Ok(()),
            Check::Array(array) => {
                let (walk, element) = &mut **array;
                walk.byte(byte, element)
            }
            Check::Int2Vector(walk) => walk.byte(byte, &mut |_| Ok(())),
            Check::Record(record) => {
                let (walk, fields) = &mut **record;
                walk.byte(byte, fields)
            }
            Check::Range { walk, .. } => walk.byte(byte),
            Check::Multirange { walk, bound, discrete } => {
                let (bound, discrete) = (&**bound, *discrete);
                walk.byte(byte, &mut |member| range_refused(bound, &member, discrete))
            }
        }
    }

    /// The part has ended: whether its check refuses it.
    fn end(&mut self) -> Walked {
        let refused = match self {
            Check::Nothing => false,
            Check::Typmod { length, text } => {
                std::mem::take(text).finish().is_some_and(|read| read.typmod_refuses(*length))
            }
            Check::Bytea(fold) => fold.end() == Err(Unread::Refused),
            Check::Json { walk, jsonb: false } => walk.end().is_err(),
            Check::Json { walk, jsonb: true } => walk.end() == Err(Unread::Refused),
            Check::Scalar { kind, text } => {
                let value = std::str::from_utf8(text).map_err(|_| Refused)?;
                matches!(field_key(kind, value), Err(Unread::Refused))
            }
            Check::Grammar(walk) => walk.end().is_err(),
            Check::Refuses(refused) => *refused,
            Check::Array(array) => return array.0.end(),
            Check::Int2Vector(walk) => return walk.end(&mut |_| Ok(())),
            Check::Record(record) => return record.0.end(),
            Check::Range { walk, bound, discrete } => {
                return range_refused(bound, &walk.end()?, *discrete);
            }
            Check::Multirange { walk, .. } => return walk.end(),
        };
        if refused { Err(Refused) } else { Ok(()) }
    }
}

/// **What a whitespace run can move of a [`Check`]**, saved where the run
/// starts ([`Parts::mark`]): each reader's own fixed state, and a collected
/// text's length, none of it growing with the run.
#[derive(Debug)]
enum Mark {
    Nothing,
    Typmod(TextFold),
    Bytea(ByteaFold),
    Json(JsonMark),
    /// How much text was collected.
    Collected(usize),
    Grammar(Grammar),
    /// The walk, and its element's check.
    Array(Box<(ArrayWalk, Mark)>),
    Int2Vector(Int2VectorWalk),
    /// The walk, the field it was reading and that field's check.
    Record(Box<(RecordWalk, usize, Option<Mark>)>),
    Range(RangeMark),
    Multirange(MultirangeMark),
}

/// Whether a range's two bounds, as `range_in` compares them, refuse it.
fn range_refused(
    bound: &NestedCompare,
    literal: &RangeLiteral,
    discrete: Option<Discrete>,
) -> Walked {
    checked_range(bound, literal, discrete).map(drop).map_err(|_| Refused)
}

/// **One position's parts as its container hands them over**: each checked
/// in its turn by the one check, ready again for the next, and a NULL
/// refused where the position is `NOT NULL`.
#[derive(Debug)]
struct Element {
    check: Check,
    not_null: bool,
    /// Where the check stood at the container's last mark.
    mark: Option<Mark>,
}

impl Element {
    fn of(position: &Position<NestedCompare>) -> Self {
        Self { check: Check::of(&position.plan), not_null: position.not_null, mark: None }
    }
}

impl Parts for Element {
    fn begin(&mut self, (): ()) -> Walked {
        self.check.start();
        Ok(())
    }

    fn byte(&mut self, byte: u8) -> Walked {
        self.check.byte(byte)
    }

    fn end(&mut self) -> Walked {
        self.check.end()
    }

    fn null(&mut self, (): ()) -> Walked {
        if self.not_null { Err(Refused) } else { Ok(()) }
    }

    fn mark(&mut self) {
        self.mark = Some(self.check.mark());
    }

    fn rewind(&mut self) {
        if let Some(mark) = &self.mark {
            self.check.rewind(mark);
        }
    }
}

/// A composite's fields, each its own position's.
#[derive(Debug)]
struct Fields {
    fields: Vec<Element>,
    /// The field being read last.
    at: usize,
    /// The field being read at the last mark.
    marked: usize,
}

impl Parts<usize> for Fields {
    fn begin(&mut self, k: usize) -> Walked {
        self.at = k;
        self.fields[k].begin(())
    }

    fn byte(&mut self, byte: u8) -> Walked {
        self.fields[self.at].byte(byte)
    }

    fn end(&mut self) -> Walked {
        self.fields[self.at].end()
    }

    fn null(&mut self, k: usize) -> Walked {
        self.fields[k].null(())
    }

    fn mark(&mut self) {
        self.marked = self.at;
        if let Some(field) = self.fields.get_mut(self.at) {
            field.mark();
        }
    }

    fn rewind(&mut self) {
        self.at = self.marked;
        if let Some(field) = self.fields.get_mut(self.at) {
            field.rewind();
        }
    }
}

/// Whether PostgreSQL's input function refuses `text` as a field read by
/// `grammar`, of a type this build holds as its text and orders by nothing:
/// [`FieldWalk::grammar`]'s answer, fed it whole.
pub fn grammar_refuses(grammar: TextGrammar, text: &str) -> bool {
    let mut walk = FieldWalk::grammar(grammar);
    walk.text(text.as_bytes());
    walk.finish()
}

/// **A [`TextGrammar`]'s walk** over one part, fed a byte at a time
/// (`docs/design/decisions.md`, "D144"), its state fixed.
#[derive(Debug, Clone)]
enum Grammar {
    Bit(BitWalk),
    Geometric(Box<GeometricWalk>),
}

impl Grammar {
    fn start(&mut self) {
        match self {
            Grammar::Bit(walk) => walk.start(),
            Grammar::Geometric(walk) => walk.start(),
        }
    }

    fn byte(&mut self, byte: u8) -> Walked {
        match self {
            Grammar::Bit(walk) => {
                walk.byte(byte);
                Ok(())
            }
            Grammar::Geometric(walk) => walk.byte(byte),
        }
    }

    fn end(&mut self) -> Walked {
        match self {
            Grammar::Bit(walk) if walk.end() => Ok(()),
            Grammar::Bit(_) => Err(Refused),
            Grammar::Geometric(walk) => walk.end(),
        }
    }
}

/// The `typdelim` an array of `element` is split at, which the plan carries
/// off the preamble (I22) — `None` where the split is not this build's to
/// make: a delimiter the preamble could not read, and one `array_in` reads
/// otherwise than [`nested::parse_array_delimited`] models at some supported
/// major, which is any byte but a visible ASCII one, the space not among them,
/// other than a brace, a double quote or a backslash.
fn array_delimiter(element: &NestedCompare) -> Option<u8> {
    let delimiter = match element {
        NestedCompare::Uncomparable { delimiter, .. } => (*delimiter)?,
        _ => b',',
    };
    (delimiter.is_ascii_graphic() && !b"{}\"\\".contains(&delimiter)).then_some(delimiter)
}

/// A range bound's key, for a check to compare a range's two by: a
/// nested field's key, built as
/// [`nested_key`](crate::value::nested::key::nested_key) builds a literal's —
/// each container read by its input grammar, the one `COPY` reads a field by —
/// with every element read as a field: `Err` where a marked check on the way
/// refuses it, and `Ok(None)` where it is read but some part of it has no key
/// here, a position this build does not order or a spelling it does not read.
/// **Every element is read whatever an earlier one answered**, so a refusal
/// anywhere in the value is found.
pub(crate) fn checked_key(plan: &NestedCompare, text: &str) -> Read<Option<NestedKey>> {
    let refused = Unread::Refused;
    Ok(match plan {
        NestedCompare::Leaf { kind, .. } => match field_key(kind, text) {
            Ok(key) => Some(NestedKey::Leaf(key)),
            Err(Unread::Refused) => return Err(refused),
            Err(Unread::Unparsed) => None,
        },
        // `json`, the one position diverging `AsText` (`NestedCompare::Uncomparable`).
        NestedCompare::Uncomparable { divergence: Some(ComparisonDivergence::AsText), .. }
            if !json_in(text) =>
        {
            return Err(refused);
        }
        NestedCompare::Uncomparable { grammar: Some(grammar), .. }
            if grammar_refuses(*grammar, text) =>
        {
            return Err(refused);
        }
        NestedCompare::Uncomparable { .. } => None,
        NestedCompare::Array { element, .. } => {
            let Some(delimiter) = array_delimiter(&element.plan) else { return Ok(None) };
            let literal = nested::parse_array_delimited(text, delimiter).ok_or(refused)?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            let mut keyed = true;
            for value in &literal.elements {
                match value {
                    Some(value) => match checked_key(&element.plan, value)? {
                        Some(key) => elements.push(Some(key)),
                        None => keyed = false,
                    },
                    None if element.not_null => return Err(refused),
                    None => elements.push(None),
                }
            }
            keyed.then_some(NestedKey::Array {
                elements,
                dims: literal.dims,
                lower_bounds: literal.lower_bounds,
            })
        }
        NestedCompare::Int2Vector { .. } => {
            let values = nested::parse_int2vector(text).ok_or(refused)?;
            Some(NestedKey::Array {
                dims: vec![values.len()],
                elements: values
                    .into_iter()
                    .map(|v| Some(NestedKey::Leaf(OrderKey::Int(i64::from(v)))))
                    .collect(),
                lower_bounds: vec![0],
            })
        }
        NestedCompare::Record { fields: plans, .. } => {
            let fields = nested::parse_record(text, plans.len()).ok_or(refused)?.fields;
            let mut out = Vec::with_capacity(plans.len());
            let mut keyed = true;
            for ((_, field), value) in plans.iter().zip(fields) {
                match value {
                    Some(value) => match checked_key(&field.plan, &value)? {
                        Some(key) => out.push(Some(key)),
                        None => keyed = false,
                    },
                    None if field.not_null => return Err(refused),
                    None => out.push(None),
                }
            }
            keyed.then_some(NestedKey::Record(out))
        }
        NestedCompare::Range { bound, discrete, .. } => {
            let literal = nested::parse_range(text).ok_or(refused)?;
            checked_range(bound, &literal, *discrete)?
                .map(|range| NestedKey::Range(Box::new(range)))
        }
        NestedCompare::Multirange { bound, discrete, .. } => {
            let literals = nested::parse_multirange(text).ok_or(refused)?;
            let mut members = Vec::with_capacity(literals.len());
            let mut keyed = true;
            for literal in &literals {
                match checked_range(bound, literal, *discrete)? {
                    Some(member) => members.push(member),
                    None => keyed = false,
                }
            }
            // `multirange_in` refuses nothing past its members' `range_in`.
            keyed
                .then(|| canonical_multirange(members, *discrete))
                .flatten()
                .map(NestedKey::Multirange)
        }
    })
}

/// The nested keys' `range_key` for [`checked_key`]: both bounds read, then
/// [`make_range`]'s refusals where both have a key.
fn checked_range(
    bound: &NestedCompare,
    literal: &nested::RangeLiteral,
    discrete: Option<Discrete>,
) -> Read<Option<RangeKey>> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        let value = match text {
            Some(text) => match checked_key(bound, text)? {
                Some(key) => Some(key),
                None => return Ok(None),
            },
            None => None,
        };
        Ok(Some(RangeBoundKey { value, inclusive, lower }))
    };
    let lower = side(&literal.lower, literal.lower_inclusive, true)?;
    let upper = side(&literal.upper, literal.upper_inclusive, false)?;
    let (Some(lower), Some(upper)) = (lower, upper) else { return Ok(None) };
    make_range(lower, upper, literal.empty, discrete).map(Some).ok_or(Unread::Refused)
}
