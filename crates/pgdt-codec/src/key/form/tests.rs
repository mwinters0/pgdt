use std::sync::Arc;

use super::*;
use crate::internal::NUMERIC_DSCALE_MAX;
use crate::internal::NUMERIC_INTEGER_DIGITS_MAX;

/// The enum clause names the labels themselves, quoted the way the dump
/// writes them and a `--filter` value reads them back, and stops at
/// [`ENUM_LABELS_SHOWN`] with a pointer at the output that carries the
/// rest.
#[test]
fn the_enum_clause_quotes_its_labels_and_caps_the_list() {
    let kind = |labels: &[&str]| CompareKind::Enum {
        name: "public.e".into(),
        labels: labels.iter().map(|l| (*l).to_string()).collect::<Arc<[String]>>(),
        exact: true,
    };
    assert_eq!(
        accepted_form(&kind(&["sad", "has space", "has'quote"])),
        "as one of the type's declared labels: 'sad', 'has space', 'has''quote'"
    );

    let many: Vec<String> = (0..ENUM_LABELS_SHOWN + 3).map(|i| format!("l{i}")).collect();
    let clause = accepted_form(&CompareKind::Enum {
        name: "public.e".into(),
        labels: many.iter().cloned().collect(),
        exact: true,
    });
    assert!(clause.contains("'l0'"), "{clause}");
    assert!(clause.contains(&format!("'l{}'", ENUM_LABELS_SHOWN - 1)), "{clause}");
    assert!(!clause.contains(&format!("'l{ENUM_LABELS_SHOWN}'")), "{clause}");
    assert!(clause.ends_with("and 3 more; see `info --detail`"), "{clause}");
}

/// The `numeric(p,s)` clause branches on the sign of the scale, as what
/// the column refuses does: a positive scale bounds the fraction, a zero
/// scale admits no fraction, and a negative one admits only multiples of
/// a power of ten.
#[test]
fn the_numeric_clause_branches_on_the_sign_of_the_scale() {
    for (scale, expected) in [
        (2i8, "as a number with at most 2 decimal places, or `NaN`"),
        (1, "as a number with at most 1 decimal place, or `NaN`"),
        (0, "as a whole number, or `NaN`"),
        (-2, "as a whole number that is a multiple of 100, or `NaN`"),
    ] {
        let kind = CompareKind::Decimal { precision: 10, scale };
        assert_eq!(accepted_form(&kind), expected, "scale {scale}");
    }
    // The scale-free arm keeps the scale-free clause: a `p > 76` column
    // is held as text and refuses a literal only past `numeric_in`'s
    // bounds (I63), which the clause states.
    assert!(NUMERIC_DIGITS.contains(&NUMERIC_INTEGER_DIGITS_MAX.to_string()));
    assert!(NUMERIC_DIGITS.contains(&NUMERIC_DSCALE_MAX.to_string()));
    assert_eq!(
        accepted_form(&CompareKind::Numeric {
            infinities: false,
            typmod: Some(crate::pgtype::NumericTypmod { precision: 77, scale: 0 }),
        }),
        "as a number of at most 131072 digits before the point and 16383 after it, or `NaN`"
    );
}
