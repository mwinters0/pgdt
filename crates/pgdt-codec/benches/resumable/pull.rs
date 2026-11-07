//! **The pull form: the input walks** as `value/source.rs` and
//! `value/nested/input.rs` wrote them at `0fd33bd3` — `Source`, `Bytes`, `Part`, `Lexer` and
//! the array and record walks restated, the decoration and the ranges left
//! out, and each part read under its plan as a strict check's walk reads it.
//! Its two forks are theirs: the `NULL` test of every unquoted element, and
//! an unquoted element's whitespace run.

use super::{Answer, IntState, MAXDIM, Plan, Refused, Walked, is_space};

/// A field fed as `runs`, read under `plan`.
pub(crate) fn read_field(plan: &Plan, runs: &[&[u8]]) -> Walked<Answer> {
    let mut answer = Answer::default();
    match runs {
        [run] => read(plan, &mut Bytes(run.iter().copied()), &mut answer)?,
        runs => {
            read(plan, &mut Bytes(runs.iter().flat_map(|run| run.iter().copied())), &mut answer)?
        }
    }
    Ok(answer)
}

/// One part read under its plan, to its end.
fn read<'a>(plan: &Plan, text: &mut dyn Source<'a>, answer: &mut Answer) -> Walked {
    match plan {
        Plan::Int { min, max } => {
            let mut int = IntState::new();
            while let Some(byte) = text.next() {
                int.byte(byte)?;
            }
            answer.sum = answer.sum.wrapping_add(int.end(*min, *max)?);
            answer.leaves += 1;
        }
        Plan::Text => {
            while text.next().is_some() {
                answer.text_bytes += 1;
            }
            answer.leaves += 1;
        }
        Plan::Array(element) => {
            walk_array(text, b',', &mut |(), part| match part {
                Some(part) => read(element, part, answer),
                None => {
                    answer.leaves += 1;
                    answer.nulls += 1;
                    Ok(())
                }
            })?;
        }
        Plan::Record(fields) => walk_record(text, fields.len(), &mut |k, part| match part {
            Some(part) => read(&fields[k], part, answer),
            None => {
                answer.leaves += 1;
                answer.nulls += 1;
                Ok(())
            }
        })?,
    }
    Ok(())
}

trait Source<'a> {
    fn next(&mut self) -> Option<u8>;
    fn peek(&mut self) -> Option<u8>;
    fn fork(&self) -> Box<dyn Source<'a> + 'a>;

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.next();
        }
        found
    }

    fn skip_while(&mut self, keep: fn(u8) -> bool) {
        while self.peek().is_some_and(keep) {
            self.next();
        }
    }

    fn eat_word(&mut self, word: &[u8]) -> bool {
        let mut spelled = true;
        for expected in word {
            spelled &= self.next().is_some_and(|byte| byte.eq_ignore_ascii_case(expected));
        }
        spelled
    }
}

#[derive(Clone)]
struct Bytes<I>(I);

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

enum Step {
    Byte(u8),
    End,
    Fail,
}

trait Lexer: Clone {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step;
}

trait Outer<'a> {
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

struct Part<O, L> {
    outer: O,
    lexer: L,
    peeked: Option<Option<u8>>,
    ended: Option<bool>,
}

impl<'a, O: Outer<'a>, L: Lexer + 'a> Part<O, L> {
    fn new(outer: O, lexer: L) -> Self {
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

    fn finish(&mut self) -> bool {
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

type Each<'w, 'a, T> = dyn FnMut(T, Option<&mut dyn Source<'a>>) -> Walked + 'w;

fn walk_array<'a>(text: &mut dyn Source<'a>, delim: u8, each: &mut Each<'_, 'a, ()>) -> Walked {
    let mut dim = [None; MAXDIM];
    text.skip_while(is_space);
    if text.peek() != Some(b'{') {
        return Err(Refused);
    }
    let (ndim, count) = array_body(text, 0, &mut dim, delim, each)?;
    text.skip_while(is_space);
    if text.next().is_some() {
        return Err(Refused);
    }
    if count == 0 {
        return Ok(());
    }
    let dims = dim[..ndim].iter().copied().collect::<Option<Vec<usize>>>().ok_or(Refused)?;
    if dims.iter().product::<usize>() != count {
        return Err(Refused);
    }
    Ok(())
}

fn array_body<'a>(
    text: &mut dyn Source<'a>,
    mut ndim: usize,
    dim: &mut [Option<usize>; MAXDIM],
    delim: u8,
    each: &mut Each<'_, 'a, ()>,
) -> Walked<(usize, usize)> {
    let mut frozen = ndim != 0;
    let mut expect_delim = false;
    let mut nest_level = 0usize;
    let mut nelems = [0usize; MAXDIM];
    let mut count = 0usize;
    loop {
        text.skip_while(is_space);
        match text.peek().ok_or(Refused)? {
            b'{' => {
                text.next();
                if expect_delim || nest_level >= MAXDIM {
                    return Err(Refused);
                }
                nelems[nest_level] = 0;
                nest_level += 1;
                if nest_level > ndim {
                    if frozen {
                        return Err(Refused);
                    }
                    ndim = nest_level;
                }
            }
            b'}' => {
                text.next();
                if nest_level == 0 {
                    return Err(Refused);
                }
                if nelems[nest_level - 1] > 0 && !expect_delim {
                    return Err(Refused);
                }
                nest_level -= 1;
                if nest_level > 0 {
                    nelems[nest_level - 1] += 1;
                }
                match dim[nest_level] {
                    None => dim[nest_level] = Some(nelems[nest_level]),
                    Some(n) if n == nelems[nest_level] => {}
                    Some(_) => return Err(Refused),
                }
                expect_delim = true;
            }
            byte if byte == delim => {
                text.next();
                if !expect_delim {
                    return Err(Refused);
                }
                expect_delim = false;
            }
            first => {
                if expect_delim || nest_level == 0 {
                    return Err(Refused);
                }
                array_element(text, first, delim, each)?;
                frozen = true;
                if nest_level != ndim {
                    return Err(Refused);
                }
                nelems[nest_level - 1] += 1;
                count += 1;
                expect_delim = true;
            }
        }
        if nest_level == 0 {
            return Ok((ndim, count));
        }
    }
}

fn array_element<'a>(
    text: &mut dyn Source<'a>,
    first: u8,
    delim: u8,
    each: &mut Each<'_, 'a, ()>,
) -> Walked {
    if first == b'"' {
        text.next();
        let mut element = Part::new(&mut *text, QuotedElement);
        each((), Some(&mut element))?;
        if !element.finish() {
            return Err(Refused);
        }
        text.skip_while(is_space);
        return match text.peek() {
            Some(b'{' | b'}') => Ok(()),
            Some(byte) if byte == delim => Ok(()),
            _ => Err(Refused),
        };
    }
    if unquoted_null(text, delim) {
        text.eat_word(b"NULL");
        return each((), None);
    }
    let mut element = Part::new(&mut *text, UnquotedElement { delim, interior: 0 });
    each((), Some(&mut element))?;
    if element.finish() { Ok(()) } else { Err(Refused) }
}

fn unquoted_null(text: &mut dyn Source<'_>, delim: u8) -> bool {
    let mut ahead = text.fork();
    if !ahead.eat_word(b"NULL") {
        return false;
    }
    ahead.skip_while(is_space);
    ahead.peek().is_some_and(|byte| byte == delim || byte == b'}')
}

#[derive(Clone)]
struct QuotedElement;

impl Lexer for QuotedElement {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step {
        match outer.next() {
            Some(b'\\') => outer.next().map_or(Step::Fail, Step::Byte),
            Some(b'"') => Step::End,
            Some(byte) => Step::Byte(byte),
            None => Step::Fail,
        }
    }
}

#[derive(Clone)]
struct UnquotedElement {
    delim: u8,
    interior: usize,
}

impl Lexer for UnquotedElement {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step {
        if self.interior > 0 {
            self.interior -= 1;
            return outer.next().map_or(Step::Fail, Step::Byte);
        }
        let terminates = |byte: u8, delim: u8| byte == delim || byte == b'}';
        match outer.peek() {
            None | Some(b'{' | b'"') => Step::Fail,
            Some(b'\\') => {
                outer.next();
                outer.next().map_or(Step::Fail, Step::Byte)
            }
            Some(byte) if terminates(byte, self.delim) => Step::End,
            Some(byte) if is_space(byte) => {
                let mut ahead = outer.fork();
                let mut run = 0usize;
                while ahead.peek().is_some_and(is_space) {
                    ahead.next();
                    run += 1;
                }
                if ahead.peek().is_some_and(|byte| terminates(byte, self.delim)) {
                    return Step::End;
                }
                self.interior = run - 1;
                outer.next().map_or(Step::Fail, Step::Byte)
            }
            Some(byte) => {
                outer.next();
                Step::Byte(byte)
            }
        }
    }
}

#[derive(Clone)]
struct QuotedRun {
    terminators: &'static [u8],
    quoted: bool,
}

impl Lexer for QuotedRun {
    fn step(&mut self, outer: &mut dyn Source<'_>) -> Step {
        loop {
            let Some(byte) = outer.peek() else { return Step::Fail };
            if !self.quoted && self.terminators.contains(&byte) {
                return Step::End;
            }
            outer.next();
            match byte {
                b'\\' => return outer.next().map_or(Step::Fail, Step::Byte),
                b'"' if !self.quoted => self.quoted = true,
                b'"' if outer.eat(b'"') => return Step::Byte(b'"'),
                b'"' => self.quoted = false,
                byte => return Step::Byte(byte),
            }
        }
    }
}

fn walk_record<'a>(
    text: &mut dyn Source<'a>,
    columns: usize,
    each: &mut Each<'_, 'a, usize>,
) -> Walked {
    text.skip_while(is_space);
    if !text.eat(b'(') {
        return Err(Refused);
    }
    for k in 0..columns {
        if k > 0 && !text.eat(b',') {
            return Err(Refused);
        }
        if matches!(text.peek(), Some(b',' | b')')) {
            each(k, None)?;
            continue;
        }
        let mut field = Part::new(&mut *text, QuotedRun { terminators: b",)", quoted: false });
        each(k, Some(&mut field))?;
        if !field.finish() {
            return Err(Refused);
        }
    }
    if !text.eat(b')') {
        return Err(Refused);
    }
    text.skip_while(is_space);
    if text.next().is_some() { Err(Refused) } else { Ok(()) }
}
