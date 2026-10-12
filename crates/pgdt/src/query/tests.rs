use super::*;

/// The note a name that was not found earns when it looks quoted —
/// `--column` and `--table` take their names verbatim, so the quote marks
/// were part of what was looked for.
#[test]
fn a_quoted_looking_name_earns_the_verbatim_note() {
    let note = quoted_name_note("--column", "\"id\"").expect("a quoted-looking name");
    assert!(note.contains("--column"), "{note}");
    assert!(note.contains("matched literally"), "{note}");
    assert_eq!(quoted_name_note("--column", "id"), None);
    assert_eq!(quoted_name_note("--table", "\"id"), None, "one quote is not a pair");
    assert_eq!(quoted_name_note("--table", "\""), None, "one character is not a pair");
}
