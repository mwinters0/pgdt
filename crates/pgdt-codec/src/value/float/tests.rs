use super::*;

#[test]
fn float_special_values_round_trip() {
    for (text, v) in
        [("NaN", f64::NAN), ("Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY)]
    {
        let decoded = decode_f64(text).unwrap();
        assert_eq!(decoded.is_nan(), v.is_nan());
        if !v.is_nan() {
            assert_eq!(decoded, v);
        }
        assert_eq!(render_f64(decoded), text);
    }
    assert_eq!(render_f64(decode_f64("-0").unwrap()), "-0");
}

/// **A float past its type's range is refused, as `float8in` and
/// `float4in` refuse it** (I59): every spelling `float8out` and
/// `float4out` round the largest finite value past at an
/// `--extra-float-digits` short enough (I57), down to `-15`'s one digit,
/// and a nonzero one read as zero. A spelling this build does not read,
/// which `strtod` does, is unparsed rather than refused.
#[test]
fn a_float_past_its_range_is_refused_and_one_in_it_read() {
    for text in [
        "1.79769313486232e+308",
        "1.7976931349e+308",
        "1.797693135e+308",
        "1.7977e+308",
        "1.798e+308",
        "1.8e+308",
        "2e+308",
        "1e+309",
        "1e-400",
    ] {
        assert_eq!(float_in::<f64>(text), Err(Unread::Refused), "{text}");
        assert_eq!(float_in::<f64>(&format!("-{text}")), Err(Unread::Refused), "-{text}");
    }
    for text in ["3.403e+38", "3.4e+39", "4e+38", "1e-46"] {
        assert_eq!(float_in::<f32>(text), Err(Unread::Refused), "{text}");
        assert_eq!(float_in::<f32>(&format!("-{text}")), Err(Unread::Refused), "-{text}");
    }
    for text in [" 1.5", "1.5 ", "0x1p3", "nan(1)"] {
        assert_eq!(float_in::<f64>(text), Err(Unread::Unparsed), "{text:?}");
    }
    for text in ["1_0", "", " 1e400", "0x1p1024"] {
        assert_eq!(float_in::<f64>(text), Err(Unread::Refused), "{text:?}");
    }
    assert_eq!(decode_f64("1.79769313486232e+308"), None);
    assert_eq!(decode_f64("1.7976931348623157e+308"), Some(f64::MAX));
    assert_eq!(decode_f32("3.40282e+38"), Some(3.40282e38));
    assert_eq!(decode_f64("Infinity"), Some(f64::INFINITY));
    assert_eq!(decode_f64("-Infinity"), Some(f64::NEG_INFINITY));
    assert_eq!(decode_f64("1e-310"), Some(1e-310));
    assert_eq!(decode_f64("0e-999"), Some(0.0));
}

/// **Told to ignore the refusal, a float field past its type's range is
/// read as the parse rounds it**: the largest finite value of its sign past
/// it, zero of its sign below it; a spelling this build does not read stays
/// unread, and told nothing, every one of them is.
#[test]
fn a_float_field_past_its_range_is_read_only_when_ignored() {
    use pgdt_model::PostgresInvalidValues::{Default, Ignore};
    for (text, read) in [
        ("1.79769313486232e+308", f64::MAX),
        ("-1.79769313486232e+308", -f64::MAX),
        ("1e+309", f64::MAX),
        ("1e-400", 0.0),
    ] {
        assert_eq!(float_field::<f64>(text, Default), None, "{text}");
        assert_eq!(float_field::<f64>(text, Ignore), Some(read), "{text}");
    }
    assert!(float_field::<f64>("-1e-400", Ignore).unwrap().is_sign_negative());
    assert_eq!(float_field::<f32>("3.403e+38", Ignore), Some(f32::MAX));
    assert_eq!(float_field::<f32>("-3.403e+38", Ignore), Some(-f32::MAX));
    for text in ["1.5", "Infinity", "NaN"] {
        let (default, ignored) = (float_field::<f64>(text, Default), float_field(text, Ignore));
        assert_eq!(default.map(f64::to_bits), ignored.map(f64::to_bits), "{text}");
        assert!(default.is_some(), "{text}");
    }
    assert_eq!(float_field::<f64>("0x1p3", Ignore), None);
}

/// Ground truth from a live server under `extra_float_digits = 3` —
/// exercises the fixed/scientific switch at both ends and at both
/// `FLT_DIG`/`DBL_DIG` thresholds, which a fixture round-trip alone
/// can't be relied on to hit.
#[test]
fn float_formatting_matches_postgresql_exactly() {
    let cases: &[(&str, f64)] = &[
        ("1e+100", 1e100),
        ("1e+20", 1e20),
        ("1e+21", 1e21),
        ("1.2345678901234568e+20", 1.2345678901234568e20),
        ("0.00015", 1.5e-4),
        ("1.5e-05", 1.5e-5),
        ("0.0001", 0.0001),
        ("-1e+100", -1e100),
        ("1e-300", 1e-300),
        ("0", 0.0),
        ("1.234567890123456e+15", 1234567890123456.0),
        ("100000000000000", 1e14),
        ("1e+15", 1e15),
        ("123456789012345", 123456789012345.0),
    ];
    for (text, v) in cases {
        assert_eq!(render_f64(*v), *text, "value {v}");
        assert_eq!(decode_f64(text), Some(*v), "text {text}");
    }
    let f32_cases: &[(&str, f32)] = &[
        ("1e+06", 1e6),
        ("100000", 1e5),
        ("1.2345678e+07", 1.234_567_8e7),
        ("1.1754944e-38", 1.175_494_4e-38),
    ];
    for (text, v) in f32_cases {
        assert_eq!(render_f32(*v), *text, "value {v}");
        assert_eq!(decode_f32(text), Some(*v), "text {text}");
    }
}
