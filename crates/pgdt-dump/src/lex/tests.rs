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
    for line in ["SELECT $1$;", "SELECT a$$b$ FROM t;", "SELECT a$b $1 FROM t;", "SELECT é$$;"] {
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
