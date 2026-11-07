//! **The `async` form: today's walk compiled into a state machine.** The
//! walks are [`super::pull`]'s, each an `async fn` awaiting its next byte; a
//! source is polled for it, and the field's runs are `Pending` once the run
//! it was fed is read, so the walk suspends where it stands and resumes on
//! the next run (`read_field`, driven by a no-op waker, there being nothing
//! to wake: the caller feeding a run is the wake).
//!
//! A source is `dyn`, as today's is — a part's source wraps its container's,
//! to a depth the plan sets at run time — so its methods poll rather than
//! being `async fn`s, which no trait object can hold; a [`Lexer`] is
//! resumable to match, carrying across a suspension what a byte left
//! pending — an escape, a quote awaiting its double, the `NULL` letters and
//! a whitespace run held. A container nested in
//! a container is read through a boxed future, the one recursion an `async
//! fn` cannot hold inline; a leaf is read in place. Neither fork is here: the
//! `NULL` test holds the letters it has read, and an unquoted element's
//! whitespace run is carried as [`Blanks`].

use std::cell::Cell;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::{Context, Poll, Waker, ready};

use super::{Answer, Blanks, IntState, MAXDIM, Plan, Refused, Walked, is_space};

/// The run a field's reader reads now, and whether the field has ended.
struct Feed<'r> {
    run: Cell<&'r [u8]>,
    ended: Cell<bool>,
}

/// A field fed as `runs`, read under `plan`: the walk polled once a run, and
/// once more past the last.
pub(crate) fn read_field(plan: &Plan, runs: &[&[u8]]) -> Walked<Answer> {
    let feed = Feed { run: Cell::new(&[]), ended: Cell::new(false) };
    let mut answer = Answer::default();
    let walked = {
        let mut root = Runs { feed: &feed };
        let mut walk = pin!(read(plan, &mut root, &mut answer));
        let mut cx = Context::from_waker(Waker::noop());
        let mut walked = None;
        for run in runs {
            feed.run.set(run);
            if let Poll::Ready(result) = walk.as_mut().poll(&mut cx) {
                walked = Some(result);
                break;
            }
            debug_assert!(feed.run.get().is_empty(), "a walk suspended inside its run");
        }
        walked.unwrap_or_else(|| {
            feed.ended.set(true);
            match walk.as_mut().poll(&mut cx) {
                Poll::Ready(result) => result,
                Poll::Pending => unreachable!("a walk suspended past the field's end"),
            }
        })
    };
    walked.map(|()| answer)
}

/// **A text read forward a byte at a time**, polled: `Pending` where the
/// byte is not yet fed.
trait Source {
    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<u8>>;
    fn poll_peek(&mut self, cx: &mut Context<'_>) -> Poll<Option<u8>>;
}

fn next(text: &mut dyn Source) -> impl Future<Output = Option<u8>> + '_ {
    poll_fn(move |cx| text.poll_next(cx))
}

fn peek(text: &mut dyn Source) -> impl Future<Output = Option<u8>> + '_ {
    poll_fn(move |cx| text.poll_peek(cx))
}

async fn eat(text: &mut dyn Source, byte: u8) -> bool {
    let found = peek(text).await == Some(byte);
    if found {
        next(text).await;
    }
    found
}

async fn skip_while(text: &mut dyn Source, keep: fn(u8) -> bool) {
    while peek(text).await.is_some_and(keep) {
        next(text).await;
    }
}

/// The field's own source: its runs as they are fed.
struct Runs<'f, 'r> {
    feed: &'f Feed<'r>,
}

impl Source for Runs<'_, '_> {
    fn poll_next(&mut self, _: &mut Context<'_>) -> Poll<Option<u8>> {
        match self.feed.run.get().split_first() {
            Some((&byte, rest)) => {
                self.feed.run.set(rest);
                Poll::Ready(Some(byte))
            }
            None if self.feed.ended.get() => Poll::Ready(None),
            None => Poll::Pending,
        }
    }

    fn poll_peek(&mut self, _: &mut Context<'_>) -> Poll<Option<u8>> {
        match self.feed.run.get().first() {
            Some(&byte) => Poll::Ready(Some(byte)),
            None if self.feed.ended.get() => Poll::Ready(None),
            None => Poll::Pending,
        }
    }
}

enum Step {
    Byte(u8),
    End,
    Fail,
}

/// One part's grammar inside its container's, resumable: what it has read
/// of a step is its own state.
trait Lexer {
    fn poll_step(&mut self, outer: &mut dyn Source, cx: &mut Context<'_>) -> Poll<Step>;
}

struct Part<'s, L> {
    outer: &'s mut dyn Source,
    lexer: L,
    peeked: Option<Option<u8>>,
    ended: Option<bool>,
}

impl<'s, L: Lexer> Part<'s, L> {
    fn new(outer: &'s mut dyn Source, lexer: L) -> Self {
        Self { outer, lexer, peeked: None, ended: None }
    }

    fn poll_advance(&mut self, cx: &mut Context<'_>) -> Poll<Option<u8>> {
        if self.ended.is_some() {
            return Poll::Ready(None);
        }
        Poll::Ready(match ready!(self.lexer.poll_step(self.outer, cx)) {
            Step::Byte(byte) => Some(byte),
            Step::End => {
                self.ended = Some(false);
                None
            }
            Step::Fail => {
                self.ended = Some(true);
                None
            }
        })
    }

    async fn finish(&mut self) -> bool {
        while next(self).await.is_some() {}
        self.ended == Some(false)
    }
}

impl<L: Lexer> Source for Part<'_, L> {
    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<u8>> {
        match self.peeked.take() {
            Some(byte) => Poll::Ready(byte),
            None => self.poll_advance(cx),
        }
    }

    fn poll_peek(&mut self, cx: &mut Context<'_>) -> Poll<Option<u8>> {
        if self.peeked.is_none() {
            self.peeked = Some(ready!(self.poll_advance(cx)));
        }
        Poll::Ready(self.peeked.flatten())
    }
}

/// One part read under its plan, to its end: a leaf in place, a container
/// through a boxed future.
async fn read_part(plan: &Plan, text: &mut dyn Source, answer: &mut Answer) -> Walked {
    match plan {
        Plan::Int { .. } | Plan::Text => read_leaf(plan, text, answer).await,
        Plan::Array(_) | Plan::Record(_) => Box::pin(read(plan, text, answer)).await,
    }
}

async fn read(plan: &Plan, text: &mut dyn Source, answer: &mut Answer) -> Walked {
    match plan {
        Plan::Int { .. } | Plan::Text => read_leaf(plan, text, answer).await,
        Plan::Array(element) => walk_array(text, b',', element, answer).await,
        Plan::Record(fields) => walk_record(text, fields, answer).await,
    }
}

async fn read_leaf(plan: &Plan, text: &mut dyn Source, answer: &mut Answer) -> Walked {
    match plan {
        Plan::Int { min, max } => {
            let mut int = IntState::new();
            while let Some(byte) = next(text).await {
                int.byte(byte)?;
            }
            answer.sum = answer.sum.wrapping_add(int.end(*min, *max)?);
        }
        _ => {
            while next(text).await.is_some() {
                answer.text_bytes += 1;
            }
        }
    }
    answer.leaves += 1;
    Ok(())
}

fn null(answer: &mut Answer) {
    answer.leaves += 1;
    answer.nulls += 1;
}

async fn walk_array(
    text: &mut dyn Source,
    delim: u8,
    element: &Plan,
    answer: &mut Answer,
) -> Walked {
    let mut dim = [None; MAXDIM];
    skip_while(text, is_space).await;
    if peek(text).await != Some(b'{') {
        return Err(Refused);
    }
    let (ndim, count) = array_body(text, &mut dim, delim, element, answer).await?;
    skip_while(text, is_space).await;
    if next(text).await.is_some() {
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

async fn array_body(
    text: &mut dyn Source,
    dim: &mut [Option<usize>; MAXDIM],
    delim: u8,
    element: &Plan,
    answer: &mut Answer,
) -> Walked<(usize, usize)> {
    let mut ndim = 0usize;
    let mut frozen = false;
    let mut expect_delim = false;
    let mut nest_level = 0usize;
    let mut nelems = [0usize; MAXDIM];
    let mut count = 0usize;
    loop {
        skip_while(text, is_space).await;
        match peek(text).await.ok_or(Refused)? {
            b'{' => {
                next(text).await;
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
                next(text).await;
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
                next(text).await;
                if !expect_delim {
                    return Err(Refused);
                }
                expect_delim = false;
            }
            first => {
                if expect_delim || nest_level == 0 {
                    return Err(Refused);
                }
                array_element(text, first, delim, element, answer).await?;
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

async fn array_element(
    text: &mut dyn Source,
    first: u8,
    delim: u8,
    element: &Plan,
    answer: &mut Answer,
) -> Walked {
    if first == b'"' {
        next(text).await;
        let mut part = Part::new(&mut *text, QuotedElement { escaped: false });
        read_part(element, &mut part, answer).await?;
        if !part.finish().await {
            return Err(Refused);
        }
        skip_while(text, is_space).await;
        return match peek(text).await {
            Some(b'{' | b'}') => Ok(()),
            Some(byte) if byte == delim => Ok(()),
            _ => Err(Refused),
        };
    }
    // The `NULL` test, its letters held rather than forked past: they are
    // the element's first bytes where it turns out to be none.
    let mut held = [0u8; 4];
    let mut letters = 0;
    while letters < 4 {
        match peek(text).await {
            Some(byte) if byte.eq_ignore_ascii_case(&b"NULL"[letters]) => {
                next(text).await;
                held[letters] = byte;
                letters += 1;
            }
            _ => break,
        }
    }
    let mut blanks = Blanks::default();
    if letters == 4 {
        while let Some(byte) = peek(text).await.filter(|&byte| is_space(byte)) {
            next(text).await;
            blanks.push(byte);
        }
        if peek(text).await.is_some_and(|byte| byte == delim || byte == b'}') {
            null(answer);
            return Ok(());
        }
    }
    let lexer = UnquotedElement {
        delim,
        held,
        letters,
        replayed: 0,
        blanks,
        draining: true,
        escaped: false,
    };
    let mut part = Part::new(&mut *text, lexer);
    read_part(element, &mut part, answer).await?;
    if part.finish().await { Ok(()) } else { Err(Refused) }
}

struct QuotedElement {
    escaped: bool,
}

impl Lexer for QuotedElement {
    fn poll_step(&mut self, outer: &mut dyn Source, cx: &mut Context<'_>) -> Poll<Step> {
        loop {
            let byte = ready!(outer.poll_next(cx));
            if self.escaped {
                self.escaped = false;
                return Poll::Ready(byte.map_or(Step::Fail, Step::Byte));
            }
            return Poll::Ready(match byte {
                Some(b'\\') => {
                    self.escaped = true;
                    continue;
                }
                Some(b'"') => Step::End,
                Some(byte) => Step::Byte(byte),
                None => Step::Fail,
            });
        }
    }
}

/// An unquoted element: the `NULL` test's letters replayed, then its bytes
/// to the separator or brace after it, a whitespace run carried until the
/// byte after it says whether it was interior.
struct UnquotedElement {
    delim: u8,
    held: [u8; 4],
    letters: usize,
    replayed: usize,
    blanks: Blanks,
    /// Whether the carried run is being handed on, being interior.
    draining: bool,
    escaped: bool,
}

impl Lexer for UnquotedElement {
    fn poll_step(&mut self, outer: &mut dyn Source, cx: &mut Context<'_>) -> Poll<Step> {
        if self.replayed < self.letters {
            self.replayed += 1;
            return Poll::Ready(Step::Byte(self.held[self.replayed - 1]));
        }
        if self.draining {
            match self.blanks.pop_front() {
                Some(byte) => return Poll::Ready(Step::Byte(byte)),
                None => self.draining = false,
            }
        }
        loop {
            if self.escaped {
                let byte = ready!(outer.poll_next(cx));
                self.escaped = false;
                return Poll::Ready(byte.map_or(Step::Fail, Step::Byte));
            }
            let byte = ready!(outer.poll_peek(cx));
            match byte {
                Some(byte) if byte == self.delim || byte == b'}' => {
                    self.blanks.clear();
                    return Poll::Ready(Step::End);
                }
                Some(byte) if is_space(byte) => {
                    let _ = outer.poll_next(cx);
                    self.blanks.push(byte);
                }
                _ if !self.blanks.is_empty() => {
                    self.draining = true;
                    return Poll::Ready(Step::Byte(self.blanks.pop_front().unwrap_or(b' ')));
                }
                None | Some(b'{' | b'"') => return Poll::Ready(Step::Fail),
                Some(b'\\') => {
                    let _ = outer.poll_next(cx);
                    self.escaped = true;
                }
                Some(byte) => {
                    let _ = outer.poll_next(cx);
                    return Poll::Ready(Step::Byte(byte));
                }
            }
        }
    }
}

/// A `record_in` field to its terminator, a `"` inside quotes carried until
/// the byte after it says whether it was doubled.
struct QuotedRun {
    quoted: bool,
    escaped: bool,
    quote: bool,
}

impl Lexer for QuotedRun {
    fn poll_step(&mut self, outer: &mut dyn Source, cx: &mut Context<'_>) -> Poll<Step> {
        loop {
            if self.escaped {
                let byte = ready!(outer.poll_next(cx));
                self.escaped = false;
                return Poll::Ready(byte.map_or(Step::Fail, Step::Byte));
            }
            if self.quote {
                let doubled = ready!(outer.poll_peek(cx)) == Some(b'"');
                self.quote = false;
                if doubled {
                    let _ = outer.poll_next(cx);
                    return Poll::Ready(Step::Byte(b'"'));
                }
                self.quoted = false;
            }
            let Some(byte) = ready!(outer.poll_peek(cx)) else { return Poll::Ready(Step::Fail) };
            if !self.quoted && matches!(byte, b',' | b')') {
                return Poll::Ready(Step::End);
            }
            let _ = outer.poll_next(cx);
            match byte {
                b'\\' => self.escaped = true,
                b'"' if !self.quoted => self.quoted = true,
                b'"' => self.quote = true,
                byte => return Poll::Ready(Step::Byte(byte)),
            }
        }
    }
}

async fn walk_record(text: &mut dyn Source, fields: &[Plan], answer: &mut Answer) -> Walked {
    skip_while(text, is_space).await;
    if !eat(text, b'(').await {
        return Err(Refused);
    }
    for (k, plan) in fields.iter().enumerate() {
        if k > 0 && !eat(text, b',').await {
            return Err(Refused);
        }
        if matches!(peek(text).await, Some(b',' | b')')) {
            null(answer);
            continue;
        }
        let mut field =
            Part::new(&mut *text, QuotedRun { quoted: false, escaped: false, quote: false });
        read_part(plan, &mut field, answer).await?;
        if !field.finish().await {
            return Err(Refused);
        }
    }
    if !eat(text, b')').await {
        return Err(Refused);
    }
    skip_while(text, is_space).await;
    if next(text).await.is_some() { Err(Refused) } else { Ok(()) }
}
