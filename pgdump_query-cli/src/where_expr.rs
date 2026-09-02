//! `pgdq query --where` — the boolean expression grammar over filter terms.
//!
//! Parens group; `NOT` binds tighter than `AND`, which binds tighter than
//! `OR`; the three keywords are case-insensitive and recognised **only
//! outside quotes**. Everything that is not a paren or a keyword is a
//! **leaf**, handed to `parse_filter` — the same term grammar
//! `--filter` uses, unchanged (`docs/design/architecture.md`, "A filter term
//! is parsed for two audiences").
//!
//! The delegation keeps one term grammar rather than two, and
//! [`refuse_where_structure`] keeps the two flags meaning one thing: a
//! `--filter` term that does not tokenize to a single [`Token::Leaf`] is
//! refused, so this module's tokenizer *is* the definition of what the two
//! flags may disagree about rather than a rule restated beside one.
//!
//! This module is the CLI's alone. `Expr` is a plain public enum an embedder
//! fills in variant by variant, so nothing below L4 parses an expression any
//! more than it parses a term.

use anyhow::{Context, Result};
use pgdump_query::Expr;

/// Parse a `--where` argument into the expression tree the library evaluates.
///
/// Every fault — a malformed leaf included — is raised here, before the dump
/// is opened, exactly as a malformed `--filter` term is.
pub fn parse_where(spec: &str) -> Result<Expr> {
    let tokens = tokenize(spec);
    let mut parser = Parser { tokens: &tokens, pos: 0, spec };
    let expr = parser.parse_or()?;
    match parser.peek() {
        None => Ok(expr),
        Some(token) => {
            // A stray `(` is almost always a composite or range literal
            // written unquoted, which is the one collision between this
            // grammar and the term grammar's values — so it earns the remedy
            // rather than only the diagnosis.
            let hint = match token {
                Token::Open => " — a `(` groups here, so quote a value that holds one",
                _ => "",
            };
            Err(parser.fault(&format!(
                "{} is not expected after a complete expression{hint}",
                token.describe()
            )))
        }
    }
}

/// Refuse a `--filter` term that this grammar would read as structure, so
/// that no string means one thing under `--filter` and another under
/// `--where`.
///
/// **The refusal set is the tokenizer's, not a copy of it.** A term is
/// accepted only where [`tokenize`] gives back a single [`Token::Leaf`], which
/// makes the refused set *exactly* the disagreeing set by construction. A
/// second scan looking for the reserved spellings would be a duplicate of a
/// rule — free to drift the moment either grammar moves, and drift here is
/// silent again — and it would over-refuse today, since [`keyword_at`] wants
/// whitespace or a paren before a keyword and `--filter 'v_text=not a'` is
/// therefore one leaf under both flags.
///
/// **The check is on the `--filter` path alone.** A `--where` leaf is what
/// came *out* of this tokenizer, and text that is one leaf inside its
/// expression need not be one on its own — `--where 'x=(and b)'` cuts a leaf
/// `and b`, whose leading `and` had a paren before it there and nothing here.
///
/// The one string it reaches that was never returning wrong rows is a term
/// with no operator, such as `and is null`, which named a column `and` and is
/// a hard parse error under `--where`. It is refused anyway: a string
/// one flag accepts and the other rejects cannot be moved between them either,
/// and carving the exception would cost the property that makes the tokenizer
/// definition worth having. The remedy is the one the term grammar already
/// teaches — `--filter '"and" is null'`.
pub(crate) fn refuse_where_structure(spec: &str) -> Result<()> {
    let tokens = tokenize(spec);
    // Nothing at all is not structure: an empty or all-whitespace term holds
    // no reserved spelling to disagree about, and `parse_filter`'s usage
    // message is the accurate one. Neither flag accepts it, so the
    // single-meaning property is untouched by letting it through to there.
    if tokens.is_empty() || matches!(tokens.as_slice(), [Token::Leaf(_)]) {
        return Ok(());
    }
    // Leaves are only ever separated by structure, so more than one token
    // means at least one of them is not a leaf.
    let found = tokens
        .iter()
        .find(|token| !matches!(token, Token::Leaf(_)))
        .expect("a term of more than one token carries structure")
        .describe();
    anyhow::bail!(
        "--filter `{spec}`: {found} reads as structure under `--where`, so this term cannot mean the same thing under both flags — quote the part that holds it, or write the expression with `--where`"
    )
}

/// What the tokenizer produces. A `Leaf` is the raw text between structure,
/// trimmed but otherwise untouched — `parse_filter` owns everything
/// inside it, quoting included.
#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Open,
    Close,
    And,
    Or,
    Not,
    Leaf(&'a str),
}

impl Token<'_> {
    /// How a message names this token.
    fn describe(&self) -> String {
        match self {
            Token::Open => "`(`".to_string(),
            Token::Close => "`)`".to_string(),
            Token::And => "`AND`".to_string(),
            Token::Or => "`OR`".to_string(),
            Token::Not => "`NOT`".to_string(),
            Token::Leaf(text) => format!("the term `{text}`"),
        }
    }
}

/// Whether `b` continues an identifier, and therefore cannot be the boundary
/// a keyword needs.
///
/// **Every byte above ASCII counts**, because a UTF-8 lead or continuation
/// byte is part of whatever character it belongs to and a column name may be
/// spelled in any of them: without this, `éand=1` would find a keyword one
/// byte into a character.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// The word ending at or before `i`, with any whitespace between skipped —
/// empty where a paren, an operator byte or the start of the string sits
/// there instead. It is what tells the expression's `NOT` from the one inside
/// a term.
fn preceding_word(bytes: &[u8], i: usize) -> &[u8] {
    let mut end = i;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_word_byte(bytes[start - 1]) {
        start -= 1;
    }
    &bytes[start..end]
}

/// A keyword is recognised only where structure could legitimately be —
/// against whitespace, a paren on the matching side, or the end of the
/// string.
///
/// This is stricter than a word boundary, and deliberately: with a bare word
/// boundary, `--where 'tag=and'` would tokenize as the term `tag=` followed
/// by `AND`, because `=` is not a word byte. Requiring whitespace or a paren
/// leaves that string a single leaf, which is the equality it reads as.
///
/// **`NOT` after the word `is` belongs to the term, not to the expression.**
/// `IS NOT NULL` and `IS NOT DISTINCT FROM` both carry one, and both are the
/// leaf grammar's — splitting them would make `--where 'v IS NOT NULL'` a
/// negation of the leaf `NULL`, which is the one way a boolean grammar over
/// this term grammar can silently mean something else.
fn keyword_at(bytes: &[u8], i: usize) -> Option<(usize, Token<'static>)> {
    let before_ok = i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b')';
    if !before_ok {
        return None;
    }
    for (word, token) in [("and", Token::And), ("or", Token::Or), ("not", Token::Not)] {
        let end = i + word.len();
        if bytes.len() < end || !bytes[i..end].eq_ignore_ascii_case(word.as_bytes()) {
            continue;
        }
        let after_ok = end == bytes.len()
            || bytes[end].is_ascii_whitespace()
            || bytes[end] == b'('
            || bytes[end] == b')';
        if !after_ok {
            continue;
        }
        if token == Token::Not && preceding_word(bytes, i).eq_ignore_ascii_case(b"is") {
            return None;
        }
        return Some((word.len(), token));
    }
    None
}

/// Split `spec` into parens, keywords and leaves.
///
/// **Quoted regions are skipped whole**, with a doubled quote an escaped one
/// — the same scan `split_filter_op` makes, for the same reason: a
/// paren or the word `and` inside a quoted value is data. A quote that never
/// closes swallows the rest of the string into one leaf, where
/// `parse_filter` refuses it with the message every malformed quote
/// earns.
fn tokenize(spec: &str) -> Vec<Token<'_>> {
    let bytes = spec.as_bytes();
    let mut tokens = Vec::new();
    // The byte the pending leaf starts at, set at its first non-whitespace
    // byte so that a leaf never opens on the space before it.
    let mut leaf: Option<usize> = None;
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
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
                leaf.get_or_insert(i);
                quote = Some(b);
                i += 1;
            }
            None if b == b'(' || b == b')' => {
                flush(&mut tokens, spec, leaf.take(), i);
                tokens.push(if b == b'(' { Token::Open } else { Token::Close });
                i += 1;
            }
            None => {
                if let Some((len, keyword)) = keyword_at(bytes, i) {
                    flush(&mut tokens, spec, leaf.take(), i);
                    tokens.push(keyword);
                    i += len;
                } else {
                    if !b.is_ascii_whitespace() {
                        leaf.get_or_insert(i);
                    }
                    i += 1;
                }
            }
        }
    }
    flush(&mut tokens, spec, leaf, bytes.len());
    tokens
}

/// Push `spec[start..end]`, trimmed, as a leaf — unless there is no pending
/// leaf, or it is all whitespace.
fn flush<'a>(tokens: &mut Vec<Token<'a>>, spec: &'a str, start: Option<usize>, end: usize) {
    if let Some(start) = start {
        let text = spec[start..end].trim();
        if !text.is_empty() {
            tokens.push(Token::Leaf(text));
        }
    }
}

/// Recursive descent over the token list. `spec` is carried only so that
/// every message can quote the whole expression back.
struct Parser<'a> {
    tokens: &'a [Token<'a>],
    pos: usize,
    spec: &'a str,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Token<'a>> {
        self.tokens.get(self.pos)
    }

    /// Consume the next token if it is `want`.
    fn eat(&mut self, want: &Token<'_>) -> bool {
        if self.peek() == Some(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// The one message shape every structural fault takes: the whole
    /// expression, then what went wrong with it.
    fn fault(&self, what: &str) -> anyhow::Error {
        anyhow::anyhow!("--where `{}`: {what}", self.spec)
    }

    /// `or := and ( OR and )*` — the loosest binding, so it is the entry
    /// point.
    ///
    /// Neither this nor [`Self::parse_and`] wraps its operand's failure in a
    /// "needs an operand on its right" line: the operand is usually *there*
    /// and merely not a term, and that line then contradicts the cause under
    /// it. What is missing is already said by the fault the operand raises.
    fn parse_or(&mut self) -> Result<Expr> {
        let mut children = vec![self.parse_and()?];
        while self.eat(&Token::Or) {
            children.push(self.parse_and()?);
        }
        Ok(nary(Expr::Or, children))
    }

    /// `and := not ( AND not )*`.
    fn parse_and(&mut self) -> Result<Expr> {
        let mut children = vec![self.parse_not()?];
        while self.eat(&Token::And) {
            children.push(self.parse_not()?);
        }
        Ok(nary(Expr::And, children))
    }

    /// `not := NOT not | primary` — right-associative, so `NOT NOT a` is two
    /// negations rather than a fault.
    fn parse_not(&mut self) -> Result<Expr> {
        if self.eat(&Token::Not) {
            return Ok(Expr::Not(Box::new(self.parse_not()?)));
        }
        self.parse_primary()
    }

    /// `primary := '(' or ')' | leaf`.
    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek() {
            Some(Token::Open) => {
                self.pos += 1;
                let inner = self.parse_or()?;
                if !self.eat(&Token::Close) {
                    return Err(self.fault("a `(` is never closed"));
                }
                Ok(inner)
            }
            Some(Token::Leaf(text)) => {
                self.pos += 1;
                // The leaf's own faults keep `parse_filter`'s wording — one
                // term grammar, so one set of messages — under a line saying
                // which flag and which term they came from.
                let term = crate::parse_filter(text)
                    .with_context(|| format!("--where `{}`: in the term `{text}`", self.spec))?;
                Ok(Expr::Term(term))
            }
            Some(token) => {
                let described = token.describe();
                Err(self.fault(&format!("expected a term, found {described}")))
            }
            None if self.tokens.is_empty() => Err(self.fault("there is no expression here")),
            None => Err(self.fault("the expression ends where a term was expected")),
        }
    }
}

/// One child is that child; more than one is the n-ary node. Flattening the
/// single case keeps `--where 'a=1'` the same tree `--filter 'a=1'` builds
/// one level down, rather than a conjunction of one.
fn nary(node: fn(Vec<Expr>) -> Expr, mut children: Vec<Expr>) -> Expr {
    if children.len() == 1 { children.pop().expect("just checked") } else { node(children) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgdump_query::PredicateOp;

    /// A tree rendered as a parenthesised string, so a test can assert shape
    /// and leaves in one comparison.
    fn show(expr: &Expr) -> String {
        match expr {
            Expr::Term(term) => {
                // A worded operator gets spaces around it and a punctuation
                // one does not, so the rendering reads the way the term was
                // written.
                let symbol = term.op.symbol();
                match &term.value {
                    Some(value) if symbol.starts_with(|c: char| c.is_ascii_alphabetic()) => {
                        format!("{} {symbol} {value}", term.column)
                    }
                    Some(value) => format!("{}{symbol}{value}", term.column),
                    None => format!("{} {symbol}", term.column),
                }
            }
            Expr::And(children) => joined("and", children),
            Expr::Or(children) => joined("or", children),
            Expr::Not(inner) => format!("not({})", show(inner)),
        }
    }

    fn joined(label: &str, children: &[Expr]) -> String {
        let parts: Vec<String> = children.iter().map(show).collect();
        format!("{label}({})", parts.join(", "))
    }

    fn ok(spec: &str) -> String {
        show(&parse_where(spec).unwrap_or_else(|e| panic!("`{spec}` should parse: {e:#}")))
    }

    fn err(spec: &str) -> String {
        match parse_where(spec) {
            Err(e) => format!("{e:#}"),
            Ok(tree) => panic!("`{spec}` should be refused, parsed as {}", show(&tree)),
        }
    }

    /// One term is one term: no wrapper node, so the simplest `--where` is
    /// the same tree the same string builds under `--filter`.
    #[test]
    fn a_single_term_is_the_term_itself() {
        assert_eq!(ok("name=alpha"), "name=alpha");
        assert_eq!(ok("  name = alpha  "), "name=alpha");
        assert_eq!(ok("created_at IS NULL"), "created_at IS NULL");
    }

    /// `AND` and `OR` are n-ary rather than nested pairs, and the keywords
    /// are case-insensitive.
    #[test]
    fn conjunction_and_disjunction_are_flat() {
        assert_eq!(ok("a=1 and b=2 and c=3"), "and(a=1, b=2, c=3)");
        assert_eq!(ok("a=1 OR b=2 or c=3"), "or(a=1, b=2, c=3)");
        assert_eq!(ok("a=1 And b=2"), "and(a=1, b=2)");
    }

    /// **`NOT` binds tighter than `AND`, which binds tighter than `OR`.**
    /// Asserted through the rendered shape rather than argued: the `OR` is
    /// the root and the negation reaches one term.
    #[test]
    fn precedence_is_not_then_and_then_or() {
        assert_eq!(ok("a=1 or b=2 and c=3"), "or(a=1, and(b=2, c=3))");
        assert_eq!(ok("not a=1 and b=2"), "and(not(a=1), b=2)");
        assert_eq!(ok("not a=1 or b=2"), "or(not(a=1), b=2)");
    }

    /// Parens override it, and nest.
    #[test]
    fn parens_group() {
        assert_eq!(ok("(a=1 or b=2) and c=3"), "and(or(a=1, b=2), c=3)");
        assert_eq!(ok("not (a=1 or b=2)"), "not(or(a=1, b=2))");
        assert_eq!(ok("((a=1))"), "a=1");
        assert_eq!(ok("(a=1)and(b=2)"), "and(a=1, b=2)", "a paren is its own boundary");
    }

    /// `NOT NOT` is two negations, not a fault — the rule is `not := NOT not`
    /// and nothing is folded.
    #[test]
    fn negation_stacks() {
        assert_eq!(ok("not not a=1"), "not(not(a=1))");
    }

    /// **A keyword is recognised only against whitespace or a paren**, so a
    /// value that happens to be `and` stays a value and a column whose name
    /// contains one stays a column.
    #[test]
    fn a_keyword_needs_structure_around_it() {
        assert_eq!(ok("tag=and"), "tag=and");
        assert_eq!(ok("v_and=1"), "v_and=1");
        assert_eq!(ok("nota=1"), "nota=1");
        assert_eq!(ok("name=android"), "name=android");
        assert_eq!(ok("\u{e9}and=1"), "\u{e9}and=1", "a keyword never starts inside a character");
    }

    /// **The `NOT` inside a term is the term's.** Both `IS NOT NULL` and
    /// `IS NOT DISTINCT FROM` carry one, and reading either as the
    /// expression's negation is the one way this grammar could silently mean
    /// something other than it says — `v IS NOT NULL` would become a
    /// negation of the leaf `NULL`.
    #[test]
    fn a_not_after_is_belongs_to_the_term() {
        assert_eq!(ok("created_at IS NOT NULL"), "created_at IS NOT NULL");
        assert_eq!(ok("a=1 and b is not null"), "and(a=1, b IS NOT NULL)");
        assert_eq!(ok("not b is not null"), "not(b IS NOT NULL)");
        assert_eq!(
            ok("v is  not  distinct  from  1"),
            "v IS NOT DISTINCT FROM 1",
            "any run of whitespace, as in the term grammar"
        );
    }

    /// A keyword inside quotes is data — the tokenizer's scan skips a quoted
    /// region whole, exactly as the term grammar's does.
    #[test]
    fn a_quoted_keyword_is_data() {
        assert_eq!(ok("name='a and b'"), "name=a and b");
        assert_eq!(ok("name=\"(x)\""), "name=(x)");
        assert_eq!(ok("\"a and b\"=x"), "a and b=x");
    }

    /// **The hazard the flag split exists for.** A widened `--filter` would
    /// have read this as a disjunction; here the leaf `b` reaches
    /// `parse_filter`, which refuses it, and the message says which term.
    #[test]
    fn a_leaf_that_is_not_a_term_is_refused_loudly() {
        let message = err("note=a and b");
        assert!(message.contains("--where"), "{message}");
        assert!(message.contains("`b`"), "{message}");
        assert!(message.contains("--filter must be"), "{message}");
    }

    /// Structural faults name the whole expression and what is wrong with it.
    #[test]
    fn structural_faults_are_refused() {
        for (spec, wanted) in [
            ("", "there is no expression here"),
            ("   ", "there is no expression here"),
            ("(a=1", "a `(` is never closed"),
            ("a=1)", "`)` is not expected after a complete expression"),
            ("a=1 and", "ends where a term was expected"),
            ("or a=1", "expected a term, found `OR`"),
            ("(a=1) b=2", "the term `b=2` is not expected"),
            ("()", "expected a term, found `)`"),
            ("not", "ends where a term was expected"),
        ] {
            let message = err(spec);
            assert!(message.contains(wanted), "`{spec}`: {message}");
        }
    }

    /// **Two terms with no keyword between them are one leaf**, because only
    /// a paren or a keyword ends one — so the term grammar's earliest-operator
    /// rule applies and `a=1 b=2` is `a` equal to `1 b=2`, exactly as the same
    /// string means under `--filter`. Juxtaposition is not an implicit `AND`,
    /// and inventing one here would be the second grammar this flag exists to
    /// avoid.
    #[test]
    fn juxtaposition_is_not_an_implicit_and() {
        assert_eq!(ok("a=1 b=2"), "a=1 b=2");
    }

    /// A quote that never closes swallows the rest of the expression into one
    /// leaf, which `parse_filter` refuses with the message every malformed
    /// quote earns — never a fallback to the unquoted reading.
    #[test]
    fn an_unclosed_quote_is_refused_by_the_leaf_grammar() {
        let message = err("name='alpha and b=2");
        assert!(message.contains("unbalanced"), "{message}");
    }

    /// The message a `--filter` term is refused with, or `None` where the
    /// term holds no structure.
    fn refused(spec: &str) -> Option<String> {
        refuse_where_structure(spec).err().map(|e| format!("{e:#}"))
    }

    /// **What 11.13 buys**: a term holding a reserved spelling is refused
    /// rather than read one way here and another under `--where`, and the
    /// message names both remedies.
    #[test]
    fn a_term_that_reads_as_structure_is_refused() {
        let message = refused("note=a and b").expect("a keyword is structure");
        assert!(message.contains("--filter `note=a and b`"), "{message}");
        assert!(message.contains("`AND`"), "{message}");
        assert!(message.contains("quote the part that holds it"), "{message}");
        assert!(message.contains("--where"), "{message}");
        for spec in ["a=1 or b=2", "not a=1", "v=(1,a)", "a=1)"] {
            assert!(refused(spec).is_some(), "`{spec}` should be refused");
        }
    }

    /// The first structural token is the one named, even where a leaf comes
    /// before it.
    #[test]
    fn the_refusal_names_what_it_found() {
        for (spec, wanted) in
            [("v=(1,a)", "`(`"), ("a=1 or b=2", "`OR`"), ("not a=1", "`NOT`"), ("a=1)", "`)`")]
        {
            let message = refused(spec).expect("structure");
            assert!(message.contains(wanted), "`{spec}`: {message}");
        }
    }

    /// **The refusal is exactly the tokenizer's boundary rule**, so every
    /// spelling that was one leaf stays one: a keyword needs whitespace or a
    /// paren before it, and a term-level `NOT` is claimed by its `is`.
    #[test]
    fn a_term_that_is_one_leaf_is_untouched() {
        for spec in [
            "note=a b",
            "v_text=not a",
            "tag=and",
            "v_and=1",
            "nota=1",
            "name=android",
            "created_at is not null",
            "v is not distinct from 1",
            "name='a and b'",
            "\"a and b\"=x",
            "name='alpha and b=2",
        ] {
            assert_eq!(refused(spec), None, "`{spec}` should be accepted");
        }
    }

    /// A term with nothing in it holds no structure to disagree about, so it
    /// falls through to the term grammar's own usage message. Neither flag
    /// accepts it either way.
    #[test]
    fn an_empty_term_is_left_to_the_term_grammar() {
        assert_eq!(refused(""), None);
        assert_eq!(refused("   "), None);
    }

    /// The two `IS DISTINCT FROM` forms come through the leaf grammar like
    /// every other operator, so `--where` needs no table of its own.
    #[test]
    fn the_leaf_grammar_carries_every_operator() {
        let Expr::Or(children) =
            parse_where("a is distinct from 1 or b IS NOT DISTINCT FROM 2").expect("parses")
        else {
            panic!("an OR at the root");
        };
        let ops: Vec<PredicateOp> = children
            .iter()
            .map(|child| match child {
                Expr::Term(term) => term.op,
                other => panic!("a term, got {}", show(other)),
            })
            .collect();
        assert_eq!(ops, [PredicateOp::IsDistinctFrom, PredicateOp::IsNotDistinctFrom]);
    }
}
