//! The SQL lexer for everything outside a `COPY` block: which bytes of a line
//! are code, and which are inside a string, a quoted identifier, a comment or
//! a dollar-quoted body, carried from one line to the next.
//!
//! A plain-format dump is a psql script, so the rules are psql's
//! (`src/fe_utils/psqlscan.l`, which copies `scan.l`'s token rules and adds
//! meta-commands), as far as they decide where a quoted region begins and
//! ends: standard, `E''`, `U&''` and bit-string literals, a literal continued
//! across a newline, `"…"` identifiers, `--` and nested `/* */` comments,
//! `$tag$` bodies, an identifier that holds a `$`, and a backslash
//! meta-command running to the end of its line. A plain literal's backslash
//! escapes under `standard_conforming_strings = off`, which the dump states
//! itself (`docs/design/postgres-invariants.md`, I50). What it does not
//! model, no region boundary depends on: psql variables (`:name`, `:'name'`),
//! which `pg_dump` never writes, and what an escape *means*. One departure
//! moves a boundary, on text `pg_dump` never writes either: a `$` opening no
//! tag takes the identifier after it, so `$e'…'` opens a plain literal where
//! psql gives the `e` back and reads an escape string.
//!
//! [`crate::scan::CopyScanner`] lexes every line outside a block, so a `$$`
//! inside any of these opens no body (`docs/design/decisions.md`, "D23"), and
//! [`crate::preamble::StatementScan`] lexes a statement's lines with the same
//! rules, so a statement's end agrees with the scanner's regions.
//!
//! **Line at a time.** Every caller holds whole lines, and nothing in a
//! region boundary straddles a newline but the regions themselves, which is
//! what [`Lexer`] carries: a `''`, a `--`, a `/*` or a `$tag$` is always
//! inside one line.

/// Which kind of quoted literal is open, by what ends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quote {
    /// `'…'` with `''` as the only escape: a standard string under
    /// `standard_conforming_strings = on`, and `U&'…'` always.
    Plain,
    /// `'…'` in which a backslash escapes the next byte: `E'…'` always, and
    /// a standard string under `standard_conforming_strings = off`.
    Escape,
    /// `B'…'` and `X'…'`, which have no escape at all.
    Bits,
}

/// Where the lexer stands between two bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Region {
    /// Between tokens, or inside an unquoted one.
    Code,
    Quoted(Quote),
    /// A `"…"` identifier, `U&"…"` included.
    Identifier,
    /// A `/* */` comment, at this nesting depth.
    Comment(u32),
    /// A dollar-quoted body, closed by exactly this delimiter, both `$`s
    /// included.
    Dollar(Box<[u8]>),
}

/// What lexing one line found, beyond where it left the [`Lexer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineLex {
    /// The line began inside a dollar-quoted body, or opened one.
    pub dollar: bool,
    /// The line ends in a `--` comment, which the newline closes.
    pub line_comment: bool,
}

/// The lexer's state across lines. See the module docs.
#[derive(Debug, Clone)]
pub(crate) struct Lexer {
    region: Region,
    /// `standard_conforming_strings`, `on` until the dump says otherwise.
    standard_strings: bool,
    /// A literal just closed with nothing since but whitespace and `--`
    /// comments, so a `'` after a newline continues it as the same kind
    /// (`scan.l`'s `quotecontinue`); `.1` is whether a newline has passed.
    continuation: Option<(Quote, bool)>,
}

impl Default for Lexer {
    fn default() -> Self {
        Self::new()
    }
}

/// The bytes that can end a run of code: they open a region, or start a
/// token that might.
const STOP: [bool; 256] = {
    let mut table = [false; 256];
    let mut i = 0;
    let stops = b"'\"-/$\\";
    while i < stops.len() {
        table[stops[i] as usize] = true;
        i += 1;
    }
    table
};

/// `scan.l`'s `ident_start`, which is also `dolq_start`.
const fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

/// `scan.l`'s `dolq_cont`: an identifier byte other than `$`.
const fn dolq_cont(b: u8) -> bool {
    ident_start(b) || b.is_ascii_digit()
}

/// `scan.l`'s `ident_cont`. `pub(crate)` for [`crate::preamble::strip_kw`]'s
/// word boundary.
pub(crate) const fn ident_cont(b: u8) -> bool {
    dolq_cont(b) || b == b'$'
}

/// `scan.l`'s `space`, less the newline a line never holds.
const fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | 0x0b | 0x0c)
}

impl Lexer {
    pub(crate) const fn new() -> Self {
        Self { region: Region::Code, standard_strings: true, continuation: None }
    }

    /// A lexer at the start of a statement, under the dump's setting.
    pub(crate) const fn with_standard_strings(standard_strings: bool) -> Self {
        Self { region: Region::Code, standard_strings, continuation: None }
    }

    pub(crate) fn region(&self) -> &Region {
        &self.region
    }

    pub(crate) fn standard_strings(&self) -> bool {
        self.standard_strings
    }

    /// A `SET standard_conforming_strings` took effect.
    pub(crate) fn set_standard_strings(&mut self, on: bool) {
        self.standard_strings = on;
    }

    /// Lex one line, its newline stripped. `code` is handed every run of
    /// bytes outside a quoted region but the stops the lexer steps over — a
    /// lone `-` or `/`, a `$` token or an identifier's `$`, a `\;` or `\:`,
    /// and a meta-command from its backslash on — which is what a caller counting
    /// parentheses wants, a paren never being one, though not necessarily as
    /// one maximal run.
    pub(crate) fn line(&mut self, line: &[u8], mut code: impl FnMut(&[u8])) -> LineLex {
        let mut dollar = matches!(self.region, Region::Dollar(_));
        if let Some((_, newline)) = &mut self.continuation {
            *newline = true;
        }
        // Where the current run of code began: a byte before it belongs to a
        // region already closed, never to a token this one continues.
        let mut boundary = 0;
        let mut i = 0;
        while i < line.len() {
            match &mut self.region {
                Region::Code => {
                    let stop = line[i..]
                        .iter()
                        .position(|&b| STOP[b as usize])
                        .map_or(line.len(), |k| i + k);
                    if stop > i {
                        let run = &line[i..stop];
                        code(run);
                        if self.continuation.is_some() && run.iter().any(|&b| !is_space(b)) {
                            self.continuation = None;
                        }
                    }
                    if stop == line.len() {
                        break;
                    }
                    match line[stop] {
                        b'\'' => {
                            let quote = match self.continuation.take() {
                                Some((quote, true)) => quote,
                                _ => self.quote_at(line, boundary, stop),
                            };
                            self.region = Region::Quoted(quote);
                            i = stop + 1;
                        }
                        b'"' => {
                            self.continuation = None;
                            self.region = Region::Identifier;
                            i = stop + 1;
                        }
                        b'-' if line.get(stop + 1) == Some(&b'-') => {
                            // deficiency: KD103 — psql's `newline` is `[\n\r]`,
                            // so a CR inside the line ends this comment, the
                            // code after it running as a statement, and counts
                            // as the newline a literal's continuation needs
                            // (`quotecontinue`); here only the line's end does.
                            // `pg_dump` writes no CR outside a literal (I90).
                            return LineLex { dollar, line_comment: true };
                        }
                        b'/' if line.get(stop + 1) == Some(&b'*') => {
                            self.continuation = None;
                            self.region = Region::Comment(1);
                            i = stop + 2;
                        }
                        b'\\' => {
                            self.continuation = None;
                            // psql: `\;` and `\:` are the character itself;
                            // any other backslash starts a meta-command, whose
                            // arguments are the rest of the line.
                            match line.get(stop + 1) {
                                Some(b';' | b':') => i = stop + 2,
                                _ => break,
                            }
                        }
                        b'$' => {
                            self.continuation = None;
                            match dollar_token(line, boundary, stop) {
                                DollarToken::Continues => i = stop + 1,
                                DollarToken::Other(end) => {
                                    i = end;
                                    boundary = end;
                                }
                                DollarToken::Delimiter(end) => {
                                    self.region = Region::Dollar(line[stop..end].into());
                                    dollar = true;
                                    i = end;
                                }
                            }
                        }
                        // A lone `-` or `/`, an operator.
                        _ => {
                            self.continuation = None;
                            i = stop + 1;
                        }
                    }
                }
                Region::Quoted(quote) => {
                    let quote = *quote;
                    let found = match quote {
                        Quote::Escape => memchr::memchr2(b'\'', b'\\', &line[i..]),
                        Quote::Plain | Quote::Bits => memchr::memchr(b'\'', &line[i..]),
                    };
                    let Some(k) = found else { break };
                    let p = i + k;
                    if line[p] == b'\\' {
                        // Escapes the next byte, or the newline itself.
                        i = p + 2;
                    } else if quote != Quote::Bits && line.get(p + 1) == Some(&b'\'') {
                        i = p + 2;
                    } else {
                        self.region = Region::Code;
                        self.continuation = Some((quote, false));
                        i = p + 1;
                        boundary = i;
                    }
                }
                Region::Identifier => {
                    let Some(k) = memchr::memchr(b'"', &line[i..]) else { break };
                    let p = i + k;
                    if line.get(p + 1) == Some(&b'"') {
                        i = p + 2;
                    } else {
                        self.region = Region::Code;
                        i = p + 1;
                        boundary = i;
                    }
                }
                Region::Comment(depth) => {
                    let Some(k) = memchr::memchr2(b'/', b'*', &line[i..]) else { break };
                    let p = i + k;
                    match (line[p], line.get(p + 1)) {
                        (b'/', Some(b'*')) => {
                            *depth += 1;
                            i = p + 2;
                        }
                        (b'*', Some(b'/')) => {
                            *depth -= 1;
                            i = p + 2;
                            if *depth == 0 {
                                self.region = Region::Code;
                                boundary = i;
                            }
                        }
                        _ => i = p + 1,
                    }
                }
                Region::Dollar(tag) => {
                    let Some(k) = memchr::memmem::find(&line[i..], tag) else { break };
                    i += k + tag.len();
                    self.region = Region::Code;
                    boundary = i;
                }
            }
        }
        LineLex { dollar, line_comment: false }
    }

    /// The kind of literal a `'` at `at` opens, read off the prefix before
    /// it: `E`, `B`, `X` or `U&` when that prefix starts its token.
    fn quote_at(&self, line: &[u8], boundary: usize, at: usize) -> Quote {
        // Whether `line[k]` is a byte of the token running into the quote.
        let token = |k: Option<usize>| k.is_some_and(|k| k >= boundary && ident_cont(line[k]));
        let before = |n: usize| at.checked_sub(n).filter(|&k| k >= boundary);
        match before(1).map(|k| line[k]) {
            Some(b'e' | b'E') if !token(at.checked_sub(2)) => Quote::Escape,
            Some(b'b' | b'B' | b'x' | b'X') if !token(at.checked_sub(2)) => Quote::Bits,
            Some(b'&')
                if before(2).is_some_and(|k| matches!(line[k], b'u' | b'U'))
                    && !token(at.checked_sub(3)) =>
            {
                Quote::Plain
            }
            _ if self.standard_strings => Quote::Plain,
            _ => Quote::Escape,
        }
    }
}

/// What a `$` in code is.
enum DollarToken {
    /// A byte of the identifier before it (`ident_cont` holds `$`).
    Continues,
    /// A parameter (`$1`), or a `$` that opens nothing; the token ends here.
    Other(usize),
    /// A `$tag$` delimiter, ending here.
    Delimiter(usize),
}

fn dollar_token(line: &[u8], boundary: usize, at: usize) -> DollarToken {
    // Inside an identifier only where the run of identifier bytes before it
    // starts as one does, rather than as a number.
    let mut start = at;
    while start > boundary && ident_cont(line[start - 1]) {
        start -= 1;
    }
    if start < at && ident_start(line[start]) {
        return DollarToken::Continues;
    }
    let mut end = at + 1;
    match line.get(end) {
        Some(b'$') => DollarToken::Delimiter(end + 1),
        Some(&b) if ident_start(b) => {
            while line.get(end).is_some_and(|&b| dolq_cont(b)) {
                end += 1;
            }
            if line.get(end) == Some(&b'$') {
                DollarToken::Delimiter(end + 1)
            } else {
                DollarToken::Other(end)
            }
        }
        _ => {
            while line.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            DollarToken::Other(end)
        }
    }
}

/// `SET standard_conforming_strings = on|off;`, the line `pg_dump` and
/// `pg_dumpall` write to state the literal syntax that follows (I50), as
/// `on`. Keywords in any case, `=` or `TO`, the value bare or quoted; any
/// other line is `None`.
pub(crate) fn standard_conforming_strings(line: &[u8]) -> Option<bool> {
    let line = std::str::from_utf8(line).ok()?.trim();
    let rest = strip_word(line, "SET")?;
    let rest = strip_word(rest, "standard_conforming_strings")?;
    let rest = rest
        .strip_prefix('=')
        .or_else(|| strip_word(rest, "TO").map(|r| r.trim_start()))?
        .trim_start();
    let value = rest.strip_suffix(';')?.trim_end();
    let value = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')).unwrap_or(value);
    match value.to_ascii_lowercase().as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// `text` with the keyword `word` taken off its front, case-insensitively,
/// and the space after it; `None` unless `word` ends at a non-identifier byte.
fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let head = text.get(..word.len())?;
    if !head.eq_ignore_ascii_case(word) {
        return None;
    }
    let rest = &text[word.len()..];
    if rest.bytes().next().is_some_and(ident_cont) {
        return None;
    }
    Some(rest.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lex `lines` from a fresh lexer under `standard`, returning where each
    /// line left it and what it found.
    fn lex(standard: bool, lines: &[&str]) -> Vec<(Region, LineLex)> {
        let mut lexer = Lexer::with_standard_strings(standard);
        lines
            .iter()
            .map(|line| {
                let found = lexer.line(line.as_bytes(), |_| {});
                (lexer.region().clone(), found)
            })
            .collect()
    }

    fn ends(standard: bool, lines: &[&str]) -> Region {
        lex(standard, lines).pop().unwrap().0
    }

    fn dollar(tag: &str) -> Region {
        Region::Dollar(tag.as_bytes().into())
    }

    #[test]
    fn a_dollar_inside_a_string_identifier_or_comment_opens_nothing() {
        for line in [
            "COMMENT ON TABLE public.a IS 'costs $$ here';",
            "COMMENT ON TABLE public.a IS E'costs \\' $$ here';",
            "CREATE TABLE public.\"x$$\" (id integer);",
            "SELECT 1; -- $$ in a comment",
            "SELECT /* $x$ */ 1;",
            "-- Name: f$$; Type: FUNCTION; Schema: public; Owner: -",
        ] {
            let found = lex(true, &[line]);
            assert_eq!(found[0].0, Region::Code, "{line}");
            assert!(!found[0].1.dollar, "{line}");
        }
    }

    #[test]
    fn a_dollar_quote_opens_and_closes_across_lines() {
        let found = lex(true, &["    AS $$", "COPY public.t (a) FROM stdin;", "$$;"]);
        assert_eq!(found[0].0, dollar("$$"));
        assert!(found[0].1.dollar);
        assert_eq!(found[1].0, dollar("$$"));
        assert!(found[1].1.dollar, "a line inside a body reports it");
        assert_eq!(found[2].0, Region::Code);
        assert!(found[2].1.dollar, "so does the line closing it");
    }

    #[test]
    fn only_the_opening_tag_closes_a_body() {
        let found = lex(true, &["AS $func$", "SELECT 'x $$ y' AS note;", "$func$;"]);
        assert_eq!(found[1].0, dollar("$func$"));
        assert_eq!(found[2].0, Region::Code);
        // A tag that starts like the open one, and one that ends like it.
        assert_eq!(ends(true, &["AS $ab$ $abc$ $b$ $zab$"]), dollar("$ab$"));
        assert_eq!(ends(true, &["AS $ab$ x$ab$"]), Region::Code);
    }

    #[test]
    fn a_dollar_quote_can_open_and_close_on_one_line() {
        let found = lex(true, &["SELECT $$literal$$ AS x;"]);
        assert_eq!(found[0].0, Region::Code);
        assert!(found[0].1.dollar);
    }

    #[test]
    fn a_dollar_in_an_identifier_or_parameter_opens_nothing() {
        for line in ["SELECT $1$;", "SELECT a$$b$ FROM t;", "SELECT a$b $1 FROM t;", "SELECT é$$;"]
        {
            assert_eq!(ends(true, &[line]), Region::Code, "{line}");
        }
        // After a number, a parameter or a closed region, a `$` starts a token.
        assert_eq!(ends(true, &["SELECT 1$$"]), dollar("$$"));
        assert_eq!(ends(true, &["SELECT $1$$"]), dollar("$$"));
        assert_eq!(ends(true, &["SELECT $a$ x $a$$b$"]), dollar("$b$"));
        assert_eq!(ends(true, &["SELECT \"a\"$é$"]), dollar("$é$"));
    }

    #[test]
    fn a_string_is_carried_across_lines() {
        let found = lex(true, &["COMMENT ON TABLE t IS 'first", "COPY public.t FROM stdin;", "';"]);
        assert_eq!(found[0].0, Region::Quoted(Quote::Plain));
        assert_eq!(found[1].0, Region::Quoted(Quote::Plain));
        assert_eq!(found[2].0, Region::Code);
    }

    #[test]
    fn a_backslash_escapes_only_where_the_literal_says_it_does() {
        // `standard_conforming_strings = on`: a plain string's backslash is a
        // byte, so `'a\'` closes, and an `E''` string's escapes the quote.
        assert_eq!(ends(true, &["SELECT 'a\\';"]), Region::Code);
        assert_eq!(ends(true, &["SELECT E'a\\';"]), Region::Quoted(Quote::Escape));
        assert_eq!(ends(true, &["SELECT e'a\\'b';"]), Region::Code);
        // `off`: the plain string escapes too, and `U&''` and bit strings never do.
        assert_eq!(ends(false, &["SELECT 'a\\';"]), Region::Quoted(Quote::Escape));
        assert_eq!(ends(false, &["SELECT U&'a\\';"]), Region::Code);
        assert_eq!(ends(false, &["SELECT X'a\\';"]), Region::Code);
        // A prefix that ends an identifier is not one.
        assert_eq!(ends(true, &["SELECT xe'a\\';"]), Region::Code);
        assert_eq!(ends(true, &["SELECT nu&'a\\';"]), Region::Code);
        // An escaped newline leaves the next line's first quote to close it.
        assert_eq!(ends(true, &["SELECT E'a\\", "';"]), Region::Code);
    }

    #[test]
    fn a_doubled_quote_stays_inside_except_in_a_bit_string() {
        assert_eq!(ends(true, &["SELECT 'it''s"]), Region::Quoted(Quote::Plain));
        assert_eq!(ends(true, &["SELECT E'it''s"]), Region::Quoted(Quote::Escape));
        // `B'0''1'` is `B'0'` and a standard string after it.
        assert_eq!(ends(false, &["SELECT B'0''1\\'"]), Region::Quoted(Quote::Escape));
        assert_eq!(ends(true, &["SELECT \"a\"\"b"]), Region::Identifier);
        assert_eq!(ends(true, &["SELECT \"a\"\"b\" $$"]), dollar("$$"));
    }

    #[test]
    fn a_literal_continued_after_a_newline_keeps_its_kind() {
        // `E'…'` continued by a plain-looking `'…'` is still an escape string.
        assert_eq!(
            ends(true, &["SELECT E'a'", "  -- between", "", "'b\\';"]),
            Region::Quoted(Quote::Escape)
        );
        // Without a newline between them, the second is a literal of its own.
        assert_eq!(ends(true, &["SELECT E'a' 'b\\';"]), Region::Code);
        // Nor across anything but whitespace and `--` comments.
        assert_eq!(ends(true, &["SELECT E'a',", "'b\\';"]), Region::Code);
        assert_eq!(ends(true, &["SELECT E'a' /* c */", "'b\\';"]), Region::Code);
    }

    #[test]
    fn block_comments_nest_and_line_comments_end_the_line() {
        assert_eq!(ends(true, &["/* a /* b */ $$"]), Region::Comment(1));
        assert_eq!(ends(true, &["/* a /* b */ $$", "*/ $$"]), dollar("$$"));
        assert_eq!(ends(true, &["SELECT 1 /**/ $$"]), dollar("$$"));
        let found = lex(true, &["SELECT 1; -- it's /* here"]);
        assert_eq!(found[0].0, Region::Code);
        assert!(found[0].1.line_comment);
        // A single `-` or `/` is an operator.
        assert_eq!(ends(true, &["SELECT 1 - -2 / 3, 'x"]), Region::Quoted(Quote::Plain));
    }

    #[test]
    fn a_meta_command_runs_to_the_end_of_its_line() {
        let found = lex(true, &["\\connect -reuse-previous=on \"dbname='a$$b'"]);
        assert_eq!(found[0].0, Region::Code);
        assert!(!found[0].1.dollar);
        assert_eq!(ends(true, &["SELECT 1 \\; SELECT 'a"]), Region::Quoted(Quote::Plain));
    }

    #[test]
    fn code_runs_hold_every_parenthesis_outside_a_region() {
        let mut lexer = Lexer::new();
        let mut depth = 0;
        lexer.line(b"INSERT INTO t VALUES (1, '(', \"(\", $$($$, E'\\'(') -- (", |run| {
            for &b in run {
                depth += match b {
                    b'(' => 1,
                    b')' => -1,
                    _ => 0,
                };
            }
        });
        assert_eq!(depth, 0);
    }

    #[test]
    fn the_setting_line_parses_as_pg_dump_writes_it() {
        let parse = |s: &str| standard_conforming_strings(s.as_bytes());
        assert_eq!(parse("SET standard_conforming_strings = on;"), Some(true));
        assert_eq!(parse("SET standard_conforming_strings = off;"), Some(false));
        assert_eq!(parse("set Standard_Conforming_Strings TO 'off' ;"), Some(false));
        assert_eq!(parse("SET standard_conforming_strings_x = off;"), None);
        assert_eq!(parse("SET standard_conforming_strings = maybe;"), None);
        assert_eq!(parse("SET standard_conforming_strings = off"), None);
        assert_eq!(parse("SET client_encoding = 'UTF8';"), None);
    }
}
