use super::*;

fn some(v: &str) -> Option<String> {
    Some(v.to_string())
}

/// One array element. Borrowed, because `Cow` compares by its contents:
/// an assertion built with this passes against either arm, and which arm
/// a decode actually took is asserted on its own below.
fn elem(v: &str) -> Option<Cow<'_, str>> {
    Some(Cow::Borrowed(v))
}

/// The whole point of the module: decode then render must be the identity
/// on anything a `*_out` function can produce.
#[track_caller]
fn array_round_trips(literal: &str) -> ArrayLiteral<'_> {
    let decoded = decode_array(literal).unwrap_or_else(|| panic!("decode failed: {literal}"));
    assert_eq!(render_array(&decoded), literal);
    decoded
}

#[test]
fn one_dimensional_arrays_round_trip_with_every_null_and_quoting_case() {
    let a = array_round_trips("{1,2,3}");
    assert_eq!(a.elements, vec![elem("1"), elem("2"), elem("3")]);
    assert_eq!(a.dims, vec![3]);
    assert_eq!(a.ndim(), 1);

    // `{}`, `{NULL}` and a one-element array holding the *string* `NULL`
    // are three different values that look alike.
    assert_eq!(array_round_trips("{}").elements, Vec::<Option<Cow<str>>>::new());
    assert_eq!(array_round_trips("{NULL}").elements, vec![None]);
    assert_eq!(array_round_trips("{\"NULL\"}").elements, vec![elem("NULL")]);
    assert_eq!(array_round_trips("{\"\"}").elements, vec![elem("")]);
    assert_eq!(array_round_trips("{1,NULL,3}").elements, vec![elem("1"), None, elem("3")]);
}

#[test]
fn an_array_backslash_escapes_inside_a_quoted_element() {
    let a = array_round_trips(r#"{"a,b","c{d}","e\"f","g\\h"}"#);
    assert_eq!(a.elements, vec![elem("a,b"), elem("c{d}"), elem(r#"e"f"#), elem(r"g\h")]);
}

/// An element is a slice of the literal unless it actually carried an
/// escape, and a *quoted* element with nothing to undo is still borrowed.
/// Asserted on the arm rather than on the text, since `Cow`'s own
/// equality cannot tell them apart.
#[test]
fn an_element_is_copied_only_where_the_literal_escaped_it() {
    let plain = decode_array(r#"{1,"a,b","c{d}",NULL,""}"#).unwrap();
    assert!(
        plain.elements.iter().flatten().all(|e| matches!(e, Cow::Borrowed(_))),
        "nothing here escapes: {:?}",
        plain.elements
    );

    // `\"` and `\\` are the only two escapes `array_out` writes, and each
    // one is what forces the copy.
    let escaped = decode_array(r#"{"e\"f","g\\h",plain}"#).unwrap();
    assert!(matches!(escaped.elements[0], Some(Cow::Owned(_))));
    assert!(matches!(escaped.elements[1], Some(Cow::Owned(_))));
    assert!(matches!(escaped.elements[2], Some(Cow::Borrowed(_))));

    // A record's doubled `""` is the other escape convention, and it is
    // the same split — `scan_token` serves all three forms.
    let (doubled, _) = scan_token(br#""a""b","#, 0, &RECORD, b")").unwrap();
    assert!(matches!(doubled, Some(Cow::Owned(_))));
}

#[test]
fn whitespace_and_the_delimiter_force_quotes_on_render() {
    let a = ArrayLiteral {
        elements: vec![elem("has space"), elem("has,comma"), elem("has'quote"), elem("plain")],
        dims: vec![4],
        lower_bounds: vec![1],
    };
    assert_eq!(render_array(&a), r#"{"has space","has,comma",has'quote,plain}"#);
    assert_eq!(decode_array(&render_array(&a)).unwrap(), a);
}

/// The bit set answers over the whole byte domain rather than over the
/// cases the round-trip tests happen to reach. Each syntax is checked
/// against the literal it was written as, so a typo in one of the three
/// `ByteSet::new` calls is a failure here rather than a token that
/// silently stops being quoted.
#[test]
fn the_force_quote_set_holds_exactly_the_bytes_each_syntax_names() {
    for (syntax, spelled) in
        [(&ARRAY, &b"\"\\{},"[..]), (&RECORD, b"\"\\(),"), (&RANGE_BOUND, b"\"\\()[],")]
    {
        for c in 0..=u8::MAX {
            assert_eq!(
                syntax.force_quote.contains(c),
                spelled.contains(&c),
                "byte {c:#04x} in {spelled:?}"
            );
        }
    }
}

#[test]
fn multi_dimensional_arrays_keep_their_shape_and_flatten_row_major() {
    let a = array_round_trips("{{1,2},{3,4}}");
    assert_eq!(a.dims, vec![2, 2]);
    assert_eq!(a.ndim(), 2);
    assert_eq!(a.elements, vec![elem("1"), elem("2"), elem("3"), elem("4")]);

    let deep = array_round_trips("{{{1},{2}},{{3},{4}}}");
    assert_eq!(deep.dims, vec![2, 2, 1]);
    assert_eq!(deep.elements, vec![elem("1"), elem("2"), elem("3"), elem("4")]);
}

#[test]
fn a_lower_bound_prefix_survives_the_round_trip() {
    let a = array_round_trips("[0:2]={7,8,9}");
    assert_eq!(a.lower_bounds, vec![0]);
    assert_eq!(a.dims, vec![3]);
    assert!(a.is_decorated());

    let negative = array_round_trips("[-1:0]={10,11}");
    assert_eq!(negative.lower_bounds, vec![-1]);

    let two_d = array_round_trips("[0:1][1:2]={{1,2},{3,4}}");
    assert_eq!(two_d.lower_bounds, vec![0, 1]);
    assert_eq!(two_d.dims, vec![2, 2]);

    // Every bound 1 means no prefix at all, which is how `array_out`
    // decides — so the flag is about the bounds, not about how the value
    // was built.
    assert!(!array_round_trips("{1,2}").is_decorated());
}

#[test]
fn ragged_and_mixed_depth_arrays_are_rejected_rather_than_guessed_at() {
    for bad in [
        "{{1},{2,3}}", // sibling lists of different lengths
        "{{1,2},3}",   // a list and a token at the same depth
        "{1,{2}}",     // the same, in the other order
        "{{},{}}",     // an inner empty list `array_out` cannot emit
        "{1,2",        // unterminated
        "{1,2}}",      // trailing junk
        "{ 1,2}",      // `array_in` padding, which `array_out` never writes
        "{a\"b}",      // a quote inside an unquoted element
        "[0:1]{1,2}",  // decoration with no `=`
        "[0:2]={1,2}", // decoration disagreeing with the element count
        "[0:-1]={}",   // `array_out` never decorates the empty array
        "{,}",         // an empty unquoted element
    ] {
        assert!(decode_array(bad).is_none(), "should not decode: {bad}");
    }
}

#[test]
fn records_double_their_quotes_and_spell_null_as_nothing() {
    for (literal, fields) in [
        (r#"(1,"a,b""c")"#, vec![some("1"), some(r#"a,b"c"#)]),
        ("()", vec![None]),
        (r#"(,"")"#, vec![None, some("")]),
        ("(3,)", vec![some("3"), None]),
        // A record does *not* force-quote the string `NULL` — it does not
        // have to, since its own NULL is empty rather than a word.
        ("(NULL)", vec![some("NULL")]),
        (r#"("a\\b")"#, vec![some(r"a\b")]),
    ] {
        let decoded = decode_record(literal).unwrap_or_else(|| panic!("{literal}"));
        assert_eq!(decoded.fields, fields, "{literal}");
        assert_eq!(render_record(&decoded), literal, "{literal}");
    }
}

#[test]
fn a_composite_inside_an_array_carries_both_escape_conventions_at_once() {
    // The array layer comes off first, exposing the record literal.
    let outer = array_round_trips(r#"{"(1,\"a,b\"\"c\")","(2,plain)"}"#);
    assert_eq!(outer.elements[0].as_deref(), Some(r#"(1,"a,b""c")"#));
    let inner = decode_record(outer.elements[0].as_deref().unwrap()).unwrap();
    assert_eq!(inner.fields, vec![some("1"), some(r#"a,b"c"#)]);
    assert_eq!(render_record(&inner), outer.elements[0].clone().unwrap());

    // And the other nesting order: a record whose field is an array.
    let literal = r#"("a,b","{""x\\""y"",""p q"",NULL}")"#;
    let record = decode_record(literal).unwrap();
    assert_eq!(record.fields[1].as_deref(), Some(r#"{"x\"y","p q",NULL}"#));
    let field = decode_array(record.fields[1].as_deref().unwrap()).unwrap();
    assert_eq!(field.elements, vec![elem(r#"x"y"#), elem("p q"), None]);
    assert_eq!(render_record(&record), literal);
}

#[test]
fn ranges_tell_an_absent_bound_from_an_empty_one() {
    for (literal, expected) in [
        (
            "[1,10)",
            RangeLiteral {
                empty: false,
                lower: some("1"),
                upper: some("10"),
                lower_inclusive: true,
                upper_inclusive: false,
            },
        ),
        ("empty", RangeLiteral::empty()),
        (
            "(,5)",
            RangeLiteral {
                empty: false,
                lower: None,
                upper: some("5"),
                lower_inclusive: false,
                upper_inclusive: false,
            },
        ),
        (
            r#"["",a)"#,
            RangeLiteral {
                empty: false,
                lower: some(""),
                upper: some("a"),
                lower_inclusive: true,
                upper_inclusive: false,
            },
        ),
        (
            r#"["a,b","c""d"]"#,
            RangeLiteral {
                empty: false,
                lower: some("a,b"),
                upper: some(r#"c"d"#),
                lower_inclusive: true,
                upper_inclusive: true,
            },
        ),
    ] {
        let decoded = decode_range(literal).unwrap_or_else(|| panic!("{literal}"));
        assert_eq!(decoded, expected, "{literal}");
        assert_eq!(render_range(&decoded), literal, "{literal}");
    }

    // `empty` and `(,)` both have absent bounds and differ only in the
    // flag — which is why the flag exists.
    assert_ne!(decode_range("empty").unwrap(), decode_range("(,)").unwrap());
}

#[test]
fn malformed_ranges_and_records_are_rejected() {
    for bad in ["[1,10", "1,10)", "[1,10)x", "[1)", "[1,2,3)", ""] {
        assert!(decode_range(bad).is_none(), "should not decode as a range: {bad}");
    }
    for bad in ["(1,2", "1,2)", "(1,2))", "(\"a)", ""] {
        assert!(decode_record(bad).is_none(), "should not decode as a record: {bad}");
    }
}

#[test]
fn multiranges_split_on_their_members_brackets_not_on_commas() {
    for (literal, count) in [("{}", 0), ("{[1,10)}", 1), ("{[1,2),[5,6)}", 2)] {
        let decoded = decode_multirange(literal).unwrap_or_else(|| panic!("{literal}"));
        assert_eq!(decoded.len(), count, "{literal}");
        assert_eq!(render_multirange(&decoded), literal, "{literal}");
    }

    // A member's own bound may contain the separator and a bracket, which
    // is exactly why the split honours quoting.
    let literal = r#"{["a,b","c)d"),[x,y]}"#;
    let decoded = decode_multirange(literal).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].upper.as_deref(), Some("c)d"));
    assert_eq!(render_multirange(&decoded), literal);

    for bad in ["{[1,2)", "[1,2)}", "{empty}", "{[1,2)x}"] {
        assert!(decode_multirange(bad).is_none(), "should not decode: {bad}");
    }
}

/// `int2vectorout`'s whole grammar, which is four rules long: decimal
/// `int16`s, one space between them, nothing else, and the empty string
/// for the empty vector.
#[test]
fn an_int2vector_round_trips_byte_for_byte() {
    for literal in ["", "0", "1 2 3", "-32768 32767", "42"] {
        let decoded =
            decode_int2vector(literal).unwrap_or_else(|| panic!("decode failed: {literal}"));
        assert_eq!(render_int2vector(&decoded), literal, "{literal}");
    }
    assert_eq!(decode_int2vector(""), Some(vec![]));
    assert_eq!(decode_int2vector("1 2 3"), Some(vec![1, 2, 3]));
}

/// The strict side refuses every spelling `pg_itoa` cannot write — which
/// is what makes it and [`render_int2vector`] inverses.
#[test]
fn a_non_canonical_int2vector_element_is_refused_by_the_decoder() {
    for literal in ["+1", "01", "-0", "1  2", " 1", "1 ", "1\t2", "{1,2}", "1,2", "32768"] {
        assert_eq!(decode_int2vector(literal), None, "{literal}");
    }
}
