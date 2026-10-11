use arrow_buffer::i256;

use super::*;

#[test]
fn decimal_numeric_and_boundary_scales() {
    // 38-digit precision, scale 10 (fits Decimal128).
    let text = "1234567890123456789012345678.1234567890";
    let unscaled = decimal_unscaled_digits(text, 10).unwrap();
    assert_eq!(unscaled.parse::<i128>().unwrap().to_string(), unscaled);
    assert_eq!(render_decimal(&unscaled, 10), text);

    // A short value read through `i256`, the unscaled type of a precision
    // past 38 digits (Decimal256), at scale 10.
    let text39 = "123456789.1234567890";
    let unscaled39 = decimal_unscaled_digits(text39, 10).unwrap();
    let as_i256 = i256::from_string(&unscaled39).unwrap();
    assert_eq!(render_decimal(&as_i256.to_string(), 10), text39);

    for (text, scale) in
        [("0.0000000000", 10), ("-1.5000000000", 10), ("0.00", 2), ("-1.50", 2), ("100.00", 2)]
    {
        let unscaled = decimal_unscaled_digits(text, scale).unwrap();
        assert_eq!(render_decimal(&unscaled, scale), text);
    }
}

/// **`numeric_in`'s two storage bounds, each at its edge** (I63): a
/// display scale of 16383 digits, trailing zeros counting, and 131072
/// integer digits, leading zeros not counting, whatever the sign.
#[test]
fn numeric_in_stores_up_to_its_display_scale_and_weight() {
    let zeros = |n| "0".repeat(n);
    for (text, stored) in [
        (format!("1.{}", zeros(16383)), true),
        (format!("1.{}", zeros(16384)), false),
        (format!("0.{}", zeros(16384)), false),
        (format!("-.{}1", zeros(16382)), true),
        (format!("1{}", zeros(131071)), true),
        (format!("1{}", zeros(131072)), false),
        (format!("-1{}", zeros(131072)), false),
        (format!("{}1.5", zeros(200_000)), true),
        ("0".to_string(), true),
    ] {
        assert_eq!(numeric_in_stores(&text), stored, "{}", text.len());
    }
}

#[test]
fn nan_numeric_has_no_decimal_representation() {
    assert_eq!(decimal_unscaled_digits("NaN", 2), None);
    assert_eq!(typmod_unscaled_digits("NaN", 10, 2), Err(Unread::Unparsed));
}

/// **A field is rounded to its scale half away from zero and refused past
/// its precision once rounded**, as `apply_typmod` stores it (I51): every
/// numeric case is a cast's reading on the koji replica (PG16), each
/// refusal its "numeric field overflow" — the carry raising the weight
/// past the precision included. Of the rest, `NaN` has no decimal, the
/// server refuses `Infinity` under a typmod (I34), and it reads `1e5`,
/// which this grammar does not (`docs/design/decisions.md`, "D55"), and
/// refuses `1e8` past the precision as it refuses `.` and the empty text
/// ([`numeric_unread`]).
#[test]
fn a_field_is_put_through_its_typmod_as_copy_puts_it() {
    let zeros = |n| "0".repeat(n);
    for (text, precision, scale, stored) in [
        ("1.005", 10, 2, Ok("101")),
        ("-1.005", 10, 2, Ok("-101")),
        ("1.004", 10, 2, Ok("100")),
        ("-0.004", 10, 2, Ok("0")),
        ("-0.005", 10, 2, Ok("-1")),
        ("1.0", 10, 2, Ok("100")),
        ("12345678.995", 10, 2, Ok("1234567900")),
        ("99999999.995", 10, 2, Err(Unread::Refused)),
        ("123456789012", 10, 2, Err(Unread::Refused)),
        ("99999999.99", 10, 2, Ok("9999999999")),
        ("0.000125", 2, 5, Ok("13")),
        ("0.0000049", 2, 5, Ok("0")),
        (".00001", 2, 5, Ok("1")),
        ("0.000995", 2, 5, Err(Unread::Refused)),
        ("0.001", 2, 5, Err(Unread::Refused)),
        ("1250", 3, -2, Ok("13")),
        ("-1249", 3, -2, Ok("-12")),
        ("99950", 3, -2, Err(Unread::Refused)),
        ("5", 1, -1, Ok("1")),
        ("5", 1, -2, Ok("0")),
        ("0.5", 1, 0, Ok("1")),
        ("9.5", 1, 0, Err(Unread::Refused)),
        ("NaN", 10, 2, Err(Unread::Unparsed)),
        ("1e5", 10, 2, Err(Unread::Unparsed)),
        ("1e8", 10, 2, Err(Unread::Refused)),
        ("Infinity", 10, 2, Err(Unread::Refused)),
        (".", 10, 2, Err(Unread::Refused)),
        ("", 10, 2, Err(Unread::Refused)),
    ] {
        assert_eq!(
            typmod_unscaled_digits(text, precision, scale).as_deref().map_err(|e| *e),
            stored.map_err(|e: Unread| e),
            "{text:?} as numeric({precision},{scale})"
        );
    }
    // `numerictypmodin`'s extreme scales, past every Arrow decimal's.
    let tiny = format!("0.{}5", zeros(999));
    assert_eq!(typmod_unscaled_digits(&tiny, 1, 1000), Ok("5".to_string()));
    assert_eq!(typmod_unscaled_digits(&tiny, 1, 999), Ok("1".to_string()));
    let huge = format!("-1{}", zeros(1000));
    assert_eq!(typmod_unscaled_digits(&huge, 1, -1000), Ok("-1".to_string()));
    assert_eq!(typmod_unscaled_digits(&huge, 1, -999), Err(Unread::Refused));
}

/// **What `numeric_out` writes is stored unchanged**: a value at its
/// column's scale reads as the exact literal reader reads it, at every
/// scale either side of zero, so the typmod moves no value a `pg_dump`
/// wrote.
#[test]
fn a_field_numeric_out_wrote_reads_as_its_literal() {
    for scale in [-3i8, -1, 0, 1, 2, 6] {
        for unscaled in ["0", "7", "-7", "120", "-98765", "1234567"] {
            let text = render_decimal(unscaled, scale);
            assert_eq!(
                typmod_unscaled_digits(&text, 7, i16::from(scale)).ok(),
                decimal_unscaled_digits(&text, scale),
                "{text:?} at scale {scale}"
            );
            assert_eq!(decimal_unscaled_digits(&text, scale).as_deref(), Some(unscaled));
        }
    }
}

#[test]
fn negative_scale_numeric() {
    // PG15+ negative-scale numerics print with no fractional digits at
    // all — not exercised by the fixtures (no negative-scale column
    // there), so pinned here from the type's own documented semantics.
    let unscaled = decimal_unscaled_digits("1200", -2).unwrap();
    assert_eq!(unscaled, "12");
    assert_eq!(render_decimal(&unscaled, -2), "1200");
}

/// **Zero is `0` at every negative scale**, which is how `numeric_out`
/// prints it whatever the typmod, though every other value of the column
/// carries the scale's trailing zeros: it decodes, to `0`, and renders
/// back as `0`. A short digit string that is not zero stays refused, it
/// being no multiple of the scale's power of ten; and `i8::MIN` renders,
/// its magnitude being past `i8`.
#[test]
fn a_negative_scale_zero_is_zero_and_renders_as_written() {
    for scale in i8::MIN..0 {
        let zeros = usize::from(scale.unsigned_abs());
        for text in ["0", "-0", "0.0", "00"] {
            assert_eq!(
                decimal_unscaled_digits(text, scale).as_deref(),
                Some("0"),
                "{text:?} at scale {scale}"
            );
        }
        assert_eq!(render_decimal("0", scale), "0", "at scale {scale}");

        let one = format!("1{}", "0".repeat(zeros));
        assert_eq!(decimal_unscaled_digits(&one, scale).as_deref(), Some("1"), "at {scale}");
        assert_eq!(render_decimal("1", scale), one, "at scale {scale}");
        let short = format!("1{}", "0".repeat(zeros - 1));
        assert_eq!(decimal_unscaled_digits(&short, scale), None, "{short:?} at {scale}");
    }
}
