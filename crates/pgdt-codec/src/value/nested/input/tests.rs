use super::*;
use crate::value::nested::{
    decode_array, decode_int2vector, decode_multirange, decode_range, decode_record, render_array,
};

fn some(v: &str) -> Option<String> {
    Some(v.to_string())
}

// Every expectation below was taken from a real server before it was
// written down — `postgres:18.6-trixie`, one `SELECT textin(<typoutput>(
// $1::<type>))` per literal inside a PL/pgSQL block returning `'E' ||
// SQLSTATE` on failure, which is the same shape
// `scripts/comparison_oracle.py` uses. The committed oracle covers the
// family and the majors (`tests/nested.rs`); these cover the corners the
// oracle's case table has no row for.

#[track_caller]
fn array_elements(literal: &str) -> Vec<Option<String>> {
    parse_array(literal)
        .unwrap_or_else(|| panic!("should parse: {literal}"))
        .elements
        .into_iter()
        .map(|e| e.map(Cow::into_owned))
        .collect()
}

#[test]
fn array_input_drops_the_whitespace_and_the_quoting_that_output_never_needed() {
    // The whole point: a literal a person would type.
    assert_eq!(array_elements("{ a , b }"), vec![some("a"), some("b")]);
    assert_eq!(array_elements("{\"1\" , \"2\"}"), vec![some("1"), some("2")]);
    assert_eq!(array_elements(" {1,2} "), vec![some("1"), some("2")]);
    assert_eq!(array_elements("{\"a\" }"), vec![some("a")]);
    // Interior whitespace is part of the element; only the trailing run
    // goes.
    assert_eq!(array_elements("{a b}"), vec![some("a b")]);
    assert_eq!(array_elements("{\ta}"), vec![some("a")]);
    assert_eq!(array_elements("{\"a\tb\"}"), vec![some("a\tb")]);
    // A backslash escape is accepted anywhere, which `array_out` only
    // ever writes inside quotes.
    assert_eq!(array_elements(r"{a\,b}"), vec![some("a,b")]);
    assert_eq!(array_elements(r"{a\\b}"), vec![some(r"a\b")]);
    assert_eq!(array_elements("{\"a,b\"}"), vec![some("a,b")]);
}

#[test]
fn an_unquoted_null_is_sql_null_unless_an_escape_disqualifies_it() {
    assert_eq!(array_elements("{null}"), vec![None]);
    assert_eq!(array_elements("{NuLl}"), vec![None]);
    assert_eq!(array_elements("{\"null\"}"), vec![some("null")]);
    // The escape makes it a string even though the bytes still spell
    // `NULL`, which is what `escaped` is for.
    assert_eq!(array_elements(r"{N\ULL}"), vec![some("NULL")]);
    assert_eq!(render_array(&parse_array(r"{N\ULL}").unwrap()), "{\"NULL\"}");
}

#[test]
fn array_input_takes_the_dimension_forms_output_never_writes() {
    // `[n]` is the one-sided form, and a decoration whose bounds are all
    // 1 is dropped on the way out.
    for literal in ["[2]={1,2}", "[+1:+2]={1,2}", "[1:2] = {1,2}", "[1:2]={1,2}"] {
        let a = parse_array(literal).unwrap_or_else(|| panic!("{literal}"));
        assert_eq!(a.dims, vec![2], "{literal}");
        assert!(!a.is_decorated(), "{literal}");
        assert_eq!(render_array(&a), "{1,2}", "{literal}");
    }
    let decorated = parse_array("[0:1]={1,2}").unwrap();
    assert_eq!(decorated.lower_bounds, vec![0]);
    assert_eq!(render_array(&decorated), "[0:1]={1,2}");

    assert_eq!(parse_array("{ {1},{2} }").unwrap().dims, vec![2, 1]);
}

/// Every array literal with no elements is the zero-dimensional empty
/// array, whatever brace structure produced it. `{{},{}}` is the one
/// place two supported majors disagree about the *input* grammar: v13–v16
/// refuse it and v17+ accept it as `{}` (I44); the newer grammar is what
/// the union rule (I35) implements.
#[test]
fn an_element_less_array_literal_is_the_empty_array_however_it_was_written() {
    for literal in ["{}", "{ }", "{{},{}}"] {
        let a = parse_array(literal).unwrap_or_else(|| panic!("{literal}"));
        assert!(a.elements.is_empty(), "{literal}");
        assert_eq!(a.dims, Vec::<usize>::new(), "{literal}");
        assert_eq!(render_array(&a), "{}", "{literal}");
    }
}

#[test]
fn malformed_array_input_is_refused_rather_than_guessed_at() {
    for bad in [
        "{1,2}x",      // junk after the closing brace
        "{1,{2}}",     // a token and a list at one depth
        "{{1},{2,3}}", // sibling lists of different lengths
        "[1:3]={1,2}", // the decoration disagrees with the contents
        "[1:0]={}",    // a zero-length dimension
        "[0:1]{1,2}",  // a decoration with no `=`
        "{a,}",        // an empty unquoted element
        "{,a}",
        "{\"a\"b}", // quoted in part
        "{a\"b\"}",
        "{\"a\" \"b\"}",
        "{\"a\"{1}}",
        "{\"a\"\"b\"}", // an array escapes `\"`, it does not double
        "{1,2",         // unterminated
        "",
    ] {
        assert!(parse_array(bad).is_none(), "should not parse: {bad}");
    }
}

/// The headline disagreement between the three grammars, in one place:
/// `array_in` drops unquoted whitespace around an element and `record_in`
/// and `range_in` keep every byte of it. A parser written once against the
/// array rules and reused eats a composite field's blanks and matches
/// nothing.
#[test]
fn only_the_array_grammar_strips_whitespace_around_a_part() {
    assert_eq!(array_elements("{ 1 , a }"), vec![some("1"), some("a")]);
    assert_eq!(parse_record("( 1 , a )", 2).unwrap().fields, vec![some(" 1 "), some(" a ")]);
    let range = parse_range("[ 1 , a )").unwrap();
    assert_eq!((range.lower, range.upper), (some(" 1 "), some(" a ")));
}

#[test]
fn record_input_reads_exactly_the_declared_field_count() {
    assert_eq!(parse_record(" (1,a) ", 2).unwrap().fields, vec![some("1"), some("a")]);
    assert_eq!(parse_record("(1,)", 2).unwrap().fields, vec![some("1"), None]);
    assert_eq!(parse_record("(,)", 2).unwrap().fields, vec![None, None]);
    assert_eq!(parse_record("(1,\"\")", 2).unwrap().fields, vec![some("1"), some("")]);
    // Quoting may start and stop mid-field, and `""` inside quotes is one
    // quote — the doubling convention, where an array backslash-escapes.
    assert_eq!(parse_record("(1,\"a\"b)", 2).unwrap().fields, vec![some("1"), some("ab")]);
    assert_eq!(parse_record("(1,\"a\"\"b\")", 2).unwrap().fields, vec![some("1"), some("a\"b")]);
    assert_eq!(parse_record(r"(1,a\,b)", 2).unwrap().fields, vec![some("1"), some("a,b")]);

    // Arity is the check, and it is why `()` is a value for a zero-field
    // composite and a fault for a two-field one.
    assert_eq!(parse_record("()", 0).unwrap().fields, Vec::<Option<String>>::new());
    assert_eq!(parse_record("()", 1).unwrap().fields, vec![None]);
    for (bad, columns) in [
        ("()", 2),      // too few columns
        ("(1,a,b)", 2), // too many
        ("(1)", 0),
        ("( )", 0), // a zero-field composite has no room for the blank
        ("()x", 0),
        ("(1,a) x", 2),
        ("(1,a", 2),
        ("1,a)", 2),
        ("", 1),
    ] {
        assert!(parse_record(bad, columns).is_none(), "should not parse: {bad} as {columns}");
    }
}

#[test]
fn range_input_takes_empty_in_any_case_and_keeps_its_bounds_verbatim() {
    for literal in ["empty", "EMPTY", " empty "] {
        assert_eq!(parse_range(literal).unwrap(), RangeLiteral::empty(), "{literal}");
    }
    assert_eq!(parse_range(" [1,10) ").unwrap(), decode_range("[1,10)").unwrap());
    // `["",a)` has an empty-string lower bound and `(,a)` has none, which
    // is the distinction `decode_range` also turns on.
    assert_eq!(parse_range("[\"\",a)").unwrap().lower, some(""));
    assert_eq!(parse_range("(,a)").unwrap().lower, None);
    assert_eq!(parse_range("[\"a\"b,c)").unwrap().lower, some("ab"));
    assert_eq!(parse_range(r"[a\,b,c)").unwrap().lower, some("a,b"));
    assert_eq!(parse_range("[\"a\"\"b\",c)").unwrap().lower, some("a\"b"));

    // Not canonicalized: a discrete subtype's `[1,10]` is `[1,11)` on the
    // server and stays inclusive here.
    assert!(parse_range("[1,10]").unwrap().upper_inclusive);

    for bad in ["empty x", "[1,10)x", "[1,10", "[1,2,3)", "1,10)", "[1)", ""] {
        assert!(parse_range(bad).is_none(), "should not parse: {bad}");
    }
}

#[test]
fn multirange_input_drops_a_member_spelled_empty_and_keeps_the_written_order() {
    assert_eq!(parse_multirange("{ }").unwrap().len(), 0);
    assert_eq!(parse_multirange("{empty}").unwrap().len(), 0);
    assert_eq!(parse_multirange("{EMPTY,[1,2)}").unwrap().len(), 1);
    assert_eq!(parse_multirange("{ [1,2) , [5,6) }").unwrap().len(), 2);
    // The server sorts and coalesces its members and drops one that is
    // *equivalent* to empty; both need the subtype's own order, so a
    // parsed multirange is the members as written.
    assert_eq!(parse_multirange("{[1,1)}").unwrap().len(), 1);
    let unsorted = parse_multirange("{[5,6),[1,2)}").unwrap();
    assert_eq!(unsorted[0].lower, some("5"));

    // A member's own bound may carry the separator and a bracket.
    let quoted = parse_multirange("{[\"a,b\",\"c)d\"),[x,y]}").unwrap();
    assert_eq!(quoted.len(), 2);
    assert_eq!(quoted[0].upper, some("c)d"));

    for bad in ["{[1,2),}", "{[1,2)", "[1,2)}", "{[1,2)x}", "{,}", ""] {
        assert!(parse_multirange(bad).is_none(), "should not parse: {bad}");
    }
}

/// The input grammars are supersets of the output ones, so everything
/// `*_out` writes the permissive parser reads as the strict decoder does —
/// which is what lets a filter compare a literal against a field at all.
#[test]
fn every_output_form_parses_as_itself_through_the_input_grammar() {
    for literal in [
        "{}",
        "{1,2,3}",
        "{NULL}",
        "{\"NULL\"}",
        "{\"\"}",
        "{{1,2},{3,4}}",
        // An array whose elements are themselves array literals (I26).
        // The parser reads it; nothing will hand it one, because such a
        // column resolves to text (`KD3`).
        "{\"{1,2}\",\"{3}\"}",
    ] {
        assert_eq!(parse_array(literal), decode_array(literal), "{literal}");
    }
    assert_eq!(parse_array("[0:2]={7,8,9}"), decode_array("[0:2]={7,8,9}"));
    for (literal, columns) in [("(1,\"a,b\"\"c\")", 2), ("(3,)", 2), ("(NULL)", 1), ("()", 1)] {
        assert_eq!(parse_record(literal, columns), decode_record(literal), "{literal}");
    }
    for literal in ["empty", "[1,10)", "(,5)", "[\"\",a)", "[\"a,b\",\"c\"\"d\"]"] {
        assert_eq!(parse_range(literal), decode_range(literal), "{literal}");
    }
    for literal in ["{}", "{[1,10)}", "{[1,2),[5,6)}"] {
        assert_eq!(parse_multirange(literal), decode_multirange(literal), "{literal}");
    }
    for literal in ["", "0", "1 2 3", "-32768 32767"] {
        assert_eq!(parse_int2vector(literal), decode_int2vector(literal), "{literal}");
    }
}

/// The permissive side is `int2vectorin`, and the rule worth having is
/// the asymmetric one: whitespace *before* a number is skipped, and the
/// byte *after* one must be a space or the end.
#[test]
fn the_int2vector_input_grammar_is_int2vectorin() {
    assert_eq!(parse_int2vector("  1   2  "), Some(vec![1, 2]));
    assert_eq!(parse_int2vector("+1 01"), Some(vec![1, 1]));
    assert_eq!(parse_int2vector("\t\n1 2"), Some(vec![1, 2]));
    assert_eq!(parse_int2vector("   "), Some(vec![]));
    // The byte after a number is neither a space nor the end.
    assert_eq!(parse_int2vector("1\t2"), None);
    assert_eq!(parse_int2vector("1,2"), None);
    assert_eq!(parse_int2vector("1x"), None);
    // `int2vectorin`'s own range check, and `strtol`'s `ERANGE` beyond it.
    assert_eq!(parse_int2vector("32768"), None);
    assert_eq!(parse_int2vector("-32769"), None);
    assert_eq!(parse_int2vector("99999999999999999999999"), None);
    // Not a number at all, and the array spelling this type does not take.
    assert_eq!(parse_int2vector("{1,2}"), None);
    assert_eq!(parse_int2vector("-"), None);
}
