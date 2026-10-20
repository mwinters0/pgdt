//! One filter term, `--filter`'s whole grammar and `--where`'s leaves.

use anyhow::Result;
use pgdt_query::{Expr, Predicate, PredicateOp};

/// A comparing operator before its value is read: the [`PredicateOp`]
/// variant it spells, as the constructor that takes the value.
pub(super) type Comparing = fn(String) -> PredicateOp;

/// The comparison spellings, in the order they are tried **at one position**
/// — longest first, so `>=` is never read as `>` followed by a stray `=`
/// (`docs/design/decisions.md`, "D60").
const FILTER_OPS: [(&str, Comparing); 6] = [
    ("!=", PredicateOp::Ne),
    (">=", PredicateOp::Ge),
    ("<=", PredicateOp::Le),
    ("=", PredicateOp::Eq),
    (">", PredicateOp::Gt),
    ("<", PredicateOp::Lt),
];

/// `word` at `i`, case-insensitively, and where it ends.
fn word_at(bytes: &[u8], i: usize, word: &str) -> Option<usize> {
    let end = i + word.len();
    (bytes.len() >= end && bytes[i..end].eq_ignore_ascii_case(word.as_bytes())).then_some(end)
}

/// Past the run of ASCII whitespace starting at `i` — `None` where there is
/// none, since every gap in the worded operators below must be a real one.
fn skip_spaces(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    (j > i).then_some(j)
}

/// `IS DISTINCT FROM` / `IS NOT DISTINCT FROM` starting at `i`, and how many
/// bytes it runs for — the two worded infix operators, candidates at the same
/// positions the punctuation spellings are, so **the earliest operator still
/// wins** (`docs/design/decisions.md`, "D60").
///
/// **Whitespace is required on both sides of the phrase**, so a column named
/// `is distinct from` is still askable as `is distinct from=x`. Any run of
/// whitespace separates the words, as in SQL, and the case is free.
fn distinct_from_at(bytes: &[u8], i: usize) -> Option<(usize, Comparing)> {
    if i == 0 || !bytes[i - 1].is_ascii_whitespace() {
        return None;
    }
    let mut j = skip_spaces(bytes, word_at(bytes, i, "is")?)?;
    let op: Comparing = match word_at(bytes, j, "not") {
        Some(after) => {
            j = skip_spaces(bytes, after)?;
            PredicateOp::IsNotDistinctFrom
        }
        None => PredicateOp::IsDistinctFrom,
    };
    j = skip_spaces(bytes, word_at(bytes, j, "distinct")?)?;
    let end = word_at(bytes, j, "from")?;
    // A value has to follow, and be separated from `FROM`: otherwise
    // `v is distinct from` alone splits into an empty value rather than
    // falling through to the usage message.
    if !bytes.get(end).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    Some((end - i, op))
}

/// `IN` starting at `i`, before the `(` its list opens with — the third
/// worded operator, a candidate where the other two are, so **the earliest
/// operator still wins**. Whitespace must precede it, as it must `IS`, and
/// any may sit between it and the paren; its length is the word's alone,
/// the list being the term's value.
fn in_at(bytes: &[u8], i: usize) -> Option<(usize, TermOp)> {
    if i == 0 || !bytes[i - 1].is_ascii_whitespace() {
        return None;
    }
    let end = word_at(bytes, i, "in")?;
    let open = end + bytes[end..].iter().take_while(|b| b.is_ascii_whitespace()).count();
    (bytes.get(open) == Some(&b'(')).then_some((end - i, TermOp::In))
}

/// A term's operator: one comparison, or `IN` before a list.
#[derive(Clone, Copy, Debug)]
pub(super) enum TermOp {
    Compare(Comparing),
    In,
}

/// What [`in_list`] found after an `IN`: each value's text, untrimmed and
/// still quoted, and the byte past the `)` that closes the list — `None`
/// where nothing closes it.
pub(super) struct InList<'a> {
    pub(super) values: Vec<&'a str>,
    pub(super) end: Option<usize>,
}

/// The list opening at the `(` at `open`, read as the term grammar reads a
/// value: **a quote opens a region only at the start of a value**, past
/// ASCII whitespace, and is data anywhere else; a `,` or `)` outside one
/// ends the value. `--where`'s tokenizer asks here too, so it ends the term
/// at the paren this ends the list at, and a paren or keyword inside the list
/// is never structure there.
pub(super) fn in_list(spec: &str, open: usize) -> InList<'_> {
    let bytes = spec.as_bytes();
    let mut values = Vec::new();
    let mut start = open + 1;
    let mut at_start = true;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if at_start && b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if at_start && (b == b'\'' || b == b'"') {
            at_start = false;
            i += 1;
            loop {
                match bytes.get(i) {
                    None => return InList { values, end: None },
                    Some(&q) if q == b && bytes.get(i + 1) == Some(&b) => i += 2,
                    Some(&q) if q == b => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            continue;
        }
        at_start = false;
        match b {
            b',' | b')' => {
                values.push(&spec[start..i]);
                if b == b')' {
                    return InList { values, end: Some(i + 1) };
                }
                start = i + 1;
                at_start = true;
            }
            _ => {}
        }
        i += 1;
    }
    InList { values, end: None }
}

/// The values of the list `rest` holds, `rest` being what follows `IN`:
/// each trimmed and unquoted as a comparison's value is
/// ([`filter_part`]). A list with no value, an empty value, an unquoted paren
/// inside one, a list nothing closes, and anything but whitespace after its
/// `)` are refused.
fn parse_in_list(rest: &str, spec: &str) -> Result<Vec<String>> {
    let open = rest.find('(').expect("`in_at` splits only before a paren");
    let list = in_list(rest, open);
    let Some(end) = list.end else {
        anyhow::bail!("--filter `{spec}`: the `IN` list's `(` is never closed")
    };
    if list.values.len() == 1 && list.values[0].trim().is_empty() {
        anyhow::bail!("--filter `{spec}`: an `IN` list needs at least one value")
    }
    let values = list
        .values
        .iter()
        .map(|value| {
            if value.trim().is_empty() {
                anyhow::bail!(
                    "--filter `{spec}`: an `IN` list holds an empty value — quote one to mean the empty string: `''`"
                )
            }
            let text = filter_part(value, "value", spec)?;
            if dequote(value.trim()).is_none() && text.contains('(') {
                anyhow::bail!(
                    "--filter `{spec}`: the `IN` value `{text}` holds a `(` — quote a value that holds a paren"
                )
            }
            Ok(text)
        })
        .collect::<Result<_>>()?;
    // After the values, so a paren left unquoted inside one is named as that
    // rather than as the text its `)` strands.
    if !rest[end..].trim().is_empty() {
        anyhow::bail!(
            "--filter `{spec}`: `{}` follows the `IN` list's closing `)`",
            rest[end..].trim()
        )
    }
    Ok(values)
}

/// Split `spec` at its operator.
///
/// **The earliest position wins, and the longest spelling at that position**
/// (`docs/design/decisions.md`, "D60") — scanning by position rather than by
/// operator is what keeps a value holding an operator byte from stealing the
/// split, the three worded operators ([`distinct_from_at`], [`in_at`])
/// included.
///
/// **The scan skips quoted regions**, so a column named `a=b` is askable as
/// `"a=b"=x`. A quote that never closes is its own outcome rather than "no
/// operator": reinterpreting the term without the operator it swallowed is
/// the silent-wrong-answer shape this grammar exists to remove.
fn split_filter_op(spec: &str) -> FilterSplit<'_> {
    let bytes = spec.as_bytes();
    // The scan walks *bytes*, and compares bytes: every character it looks
    // for is ASCII and no byte of a multi-byte UTF-8 character is, so a match
    // is always at a character boundary and the `spec[..i]` slices below are
    // safe. Matching through `str` would panic on the interior byte of a
    // multi-byte character, which trimming's Unicode whitespace admits.
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            // A doubled quote is an escaped one and keeps the region open.
            Some(q) if b == q => {
                if bytes.get(i + 1) == Some(&q) {
                    i += 2;
                } else {
                    quote = None;
                    i += 1;
                }
            }
            Some(_) => i += 1,
            None if b == b'\'' || b == b'"' => {
                quote = Some(b);
                i += 1;
            }
            None => {
                if let Some((len, op)) = filter_op_at(bytes, i) {
                    return FilterSplit::Op(&spec[..i], op, &spec[i + len..]);
                }
                i += 1;
            }
        }
    }
    match quote {
        // Whatever opened it sits left of any operator, since the scan
        // returns at the first operator it reaches outside a quote.
        Some(q) => FilterSplit::UnbalancedQuote(q as char),
        None => FilterSplit::NoOperator,
    }
}

/// The operator starting at byte `i` of the term `bytes`, and how many bytes
/// it runs for: the longest punctuation spelling there, else a worded one.
///
/// `bytes` must start where the term does, since [`distinct_from_at`] and
/// [`in_at`] read the term's start as no boundary. [`split_filter_op`] and
/// `--where`'s tokenizer both ask here, so the tokenizer knows where a term's
/// value begins by the rule that splits it.
pub(super) fn filter_op_at(bytes: &[u8], i: usize) -> Option<(usize, TermOp)> {
    FILTER_OPS
        .into_iter()
        .find(|(symbol, _)| bytes[i..].starts_with(symbol.as_bytes()))
        .map(|(symbol, op)| (symbol.len(), op))
        .or_else(|| distinct_from_at(bytes, i))
        .map(|(len, op)| (len, TermOp::Compare(op)))
        .or_else(|| in_at(bytes, i))
}

/// What [`split_filter_op`] found. `NoOperator` is the worded `IS` tests' cue,
/// not a fault: they are the fallback, tried only on a term with no operator
/// outside quotes.
enum FilterSplit<'a> {
    Op(&'a str, TermOp, &'a str),
    NoOperator,
    UnbalancedQuote(char),
}

/// The text a quoted part holds: the outer pair stripped and every doubled
/// interior quote collapsed to one, SQL's own escape.
///
/// `None` — the part does not open with a quote, so it is data exactly as
/// written. `Some(Err(quote))` — it opens with one and what follows is not a
/// well-formed quoted string; an unterminated quote and text after the closing
/// one are deliberately the *same* fault, falling back to the unquoted reading
/// handing a user who mistyped one quote a value nobody meant.
fn dequote(part: &str) -> Option<Result<String, char>> {
    let quote = part.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut out = String::new();
    let mut rest = &part[quote.len_utf8()..];
    loop {
        let Some(at) = rest.find(quote) else { return Some(Err(quote)) };
        out.push_str(&rest[..at]);
        rest = &rest[at + quote.len_utf8()..];
        if let Some(after) = rest.strip_prefix(quote) {
            out.push(quote);
            rest = after;
        } else if rest.is_empty() {
            return Some(Ok(out));
        } else {
            return Some(Err(quote));
        }
    }
}

/// One side of a filter term as the [`Predicate`] should carry it: whitespace
/// outside the quotes trimmed off, and a quoted part taken exactly as written.
/// `what` names the side for the error message. Trimming is `str::trim`,
/// Unicode's whitespace; the worded operators, the `IS` tests ([`is_test`])
/// and the keyword boundaries separate words by ASCII whitespace alone.
fn filter_part(part: &str, what: &str, spec: &str) -> Result<String> {
    let part = part.trim();
    match dequote(part) {
        None => Ok(part.to_string()),
        Some(Ok(text)) => Ok(text),
        Some(Err(quote)) => Err(unbalanced_quote(what, quote, spec)),
    }
}

/// The message a malformed quote in a column name or value earns, wherever
/// it was found — but a quote inside an `IN` list that never closes, which
/// leaves the list unclosed and earns that refusal instead ([`in_list`]).
fn unbalanced_quote(what: &str, quote: char, spec: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "--filter `{spec}`: unbalanced `{quote}` quote in the {what} — a quoted {what} closes with the matching `{quote}` at its very end, and any `{quote}` inside it is doubled"
    )
}

/// Parse one filter term into an [`Expr`] leaf — one term of the conjunction
/// a repeated `--filter` builds, and equally the **leaf** of a `--where`
/// expression ([`expr`](super::expr)): `column<op>value` for any of the six
/// comparison spellings, `column IS [NOT] DISTINCT FROM value`,
/// `column IN (value, …)`, `column IS NULL` / `column IS NOT NULL`, or
/// `column IS UNREPRESENTABLE` / `column IS NOT UNREPRESENTABLE`, the worded
/// forms matched case-insensitively (`docs/design/decisions.md`, "D60",
/// "D101").
///
/// **`IN` has no NULL**: its values are read as `=`'s are, so `null` in the
/// list is the text `null`, and `c in (a, b)` means `c=a or c=b` exactly —
/// answered as one [`PredicateOp::In`] term (`docs/design/decisions.md`,
/// "D53").
///
/// **The `IS` forms are the fallback, not the first test**: an operator
/// outside quotes is looked for first and [`is_test`] asked only of a term
/// that has none, so `note=this is null` is an equality against
/// `this is null`.
pub(super) fn parse_filter(spec: &str) -> Result<Expr> {
    match split_filter_op(spec) {
        FilterSplit::Op(column, TermOp::Compare(op), value) => Ok(Expr::Term(Predicate {
            column: filter_part(column, "column name", spec)?,
            op: op(filter_part(value, "value", spec)?),
        })),
        FilterSplit::Op(column, TermOp::In, list) => Ok(Expr::Term(Predicate {
            column: filter_part(column, "column name", spec)?,
            op: PredicateOp::In(parse_in_list(list, spec)?.into_iter().map(Some).collect()),
        })),
        FilterSplit::UnbalancedQuote(quote) => Err(unbalanced_quote("column name", quote, spec)),
        FilterSplit::NoOperator => {
            if let Some((column, op)) = is_test(spec.trim()) {
                return Ok(Expr::Term(Predicate {
                    column: filter_part(column, "column name", spec)?,
                    op,
                }));
            }
            anyhow::bail!(
                "--filter must be `column=value` (or `!=`, `<`, `<=`, `>`, `>=`), `column IS DISTINCT FROM value`, `column IS NOT DISTINCT FROM value`, `column IN (value, …)`, `column IS [NOT] NULL`, or `column IS [NOT] UNREPRESENTABLE`, got `{spec}`"
            )
        }
    }
}

/// The worded `IS` test `term` ends in, and the column before it:
/// `IS [NOT] NULL` or `IS [NOT] UNREPRESENTABLE`, matched **as words** under
/// the rule `docs/design/decisions.md`, "D60" states for every keyword —
/// case-insensitively, any run of ASCII whitespace between them, and
/// whitespace before `IS` as before the other worded operators, so `xis null`
/// names no column `x` and a column is required.
///
/// `term` is trimmed. Every cut is at an ASCII whitespace byte, which no
/// multi-byte character holds, so each slice is at a character boundary.
fn is_test(term: &str) -> Option<(&str, PredicateOp)> {
    let (head, last) = last_word(term)?;
    let (head, word) = last_word(head)?;
    let negated = word.eq_ignore_ascii_case("not");
    let (column, is) = if negated { last_word(head)? } else { (head, word) };
    if !is.eq_ignore_ascii_case("is") {
        return None;
    }
    let op = if last.eq_ignore_ascii_case("null") {
        if negated { PredicateOp::IsNotNull } else { PredicateOp::IsNull }
    } else if last.eq_ignore_ascii_case("unrepresentable") {
        if negated { PredicateOp::IsNotUnrepresentable } else { PredicateOp::IsUnrepresentable }
    } else {
        return None;
    };
    Some((column, op))
}

/// `s` split before its last word: what precedes it, trailing whitespace
/// dropped, and the word — `None` where no ASCII whitespace precedes one,
/// since every word an `IS` test names needs a boundary before it.
fn last_word(s: &str) -> Option<(&str, &str)> {
    let at = s.bytes().rposition(|b| b.is_ascii_whitespace())?;
    Some((s[..at].trim_end_matches(|c: char| c.is_ascii_whitespace()), &s[at + 1..]))
}

#[cfg(test)]
mod tests;
