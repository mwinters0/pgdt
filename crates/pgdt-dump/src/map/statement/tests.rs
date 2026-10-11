use super::*;

fn complete(buf: &str) -> bool {
    statement_complete(buf, true)
}

/// Guards against an apostrophe in comment prose opening a string that
/// never closes.
#[test]
fn statement_complete_is_not_confused_by_an_apostrophe_in_a_line_comment() {
    assert!(complete("CREATE TABLE t (id integer);\n-- it's here\nSELECT 1;"));
    // Not complete until a real `;` follows the comment.
    assert!(!complete("SELECT 1\n-- it's here"));
}

/// Guards against an apostrophe inside a double-quoted identifier
/// opening a string that never closes.
#[test]
fn statement_complete_is_not_confused_by_an_apostrophe_in_a_double_quoted_identifier() {
    assert!(complete(r#"CREATE TABLE public."it's" (id integer);"#));
}

#[test]
fn statement_complete_handles_doubled_double_quotes() {
    assert!(complete(r#"CREATE TABLE public."a""b" (id integer);"#));
    assert!(!complete(r#"CREATE TABLE public."a""b (id integer);"#));
}

#[test]
fn statement_complete_a_comment_run_to_end_of_buffer_is_never_complete() {
    // No trailing `\n` to close the comment — matches a statement scan
    // whose last fed line is itself a bare comment.
    assert!(!complete("SELECT 1;\n-- trailing comment, no newline after it"));
}

/// A statement ends where the scanner's lexer says its regions do
/// (`crate::lex`): an escape string's `\'`, a `;` or paren in a block
/// comment, and a plain string's backslash under the dump's own setting.
#[test]
fn statement_complete_lexes_as_the_scanner_does() {
    assert!(complete("COMMENT ON TABLE t IS E'it\\'s; here';"));
    assert!(!complete("COMMENT ON TABLE t IS E'it\\';"));
    assert!(complete("COMMENT ON TABLE t IS 'it\\';"));
    assert!(!statement_complete("COMMENT ON TABLE t IS 'it\\';", false));
    assert!(complete("SELECT 1 /* ( ; */;"));
    assert!(!complete("SELECT 1; /* a\n/* b */"));
    assert!(in_open_quote("SELECT 1; /* a\n/* b */", true));
    assert!(complete("SELECT 'a'\n'b' /* $$ */;"));
}

/// [`StatementScan::feed_line`] answers what [`statement_complete`] does
/// over the same lines joined by [`push_stmt_line`], so a buffer whose
/// last line is a bare comment stays open, which is what [`crate::map`]'s
/// dangling-close arm relies on.
#[test]
fn feeding_lines_matches_joining_them_with_newlines() {
    let cases: [&[&str]; 6] = [
        &["INSERT INTO t VALUES (1);"],
        &["INSERT INTO t VALUES (", "  'a''b'", ");"],
        &["SELECT 1;", "-- trailing comment"],
        &["INSERT INTO t VALUES ('a", "-- still string content", "');"],
        &["", "SELECT 1;"],
        &["INSERT INTO t VALUES (E'a'", "", "'b\\'', 1);"],
    ];
    for lines in cases {
        let mut scan = StatementScan::new(true);
        for line in lines {
            scan.feed_line(line.as_bytes());
        }
        let mut joined = String::new();
        for line in lines {
            push_stmt_line(&mut joined, line);
        }
        assert_eq!(scan.complete(), complete(&joined), "for {joined:?}");
        assert_eq!(scan.in_quote(), in_open_quote(&joined, true), "for {joined:?}");
    }
}

/// `is_empty` is what [`crate::map`]'s `INSERT` run reads as "a fresh
/// statement starts here", and it must survive a whitespace-only line
/// exactly as `buf.is_empty()` did: `push_stmt_line` appends such a line,
/// so the buffer stops being empty.
#[test]
fn a_statement_scan_is_empty_only_before_anything_is_fed() {
    let mut scan = StatementScan::new(true);
    assert!(scan.is_empty());
    scan.feed_line(b"");
    assert!(scan.is_empty(), "an empty line appends nothing, as `push_stmt_line` does not");
    scan.feed_line(b"   ");
    assert!(!scan.is_empty());
    scan.reset();
    assert!(scan.is_empty());
}

/// A reset keeps the dump's setting: the next statement of an `INSERT`
/// run is written under the same one.
#[test]
fn a_reset_keeps_the_standard_strings_setting() {
    let mut scan = StatementScan::new(false);
    scan.feed_line(b"SELECT 1;");
    scan.reset();
    scan.feed_line(br"INSERT INTO t VALUES ('a\');");
    assert!(scan.in_quote());
}
