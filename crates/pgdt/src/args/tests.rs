use super::*;

/// The `--strict-identity` grammar, both ways: what a selection binds, and
/// that `advisory` and `none` are exclusive rather than overrides — naming
/// either beside another term is a contradiction, and a contradiction is
/// refused rather than resolved (`docs/design/decisions.md`, "D60" is the
/// precedent for refusing at the flag). Every selection reads back as it
/// is written.
#[test]
fn the_strict_identity_selection_is_read_and_its_contradictions_refused() {
    let binds = |text: &str| parse_strict_identity(text).unwrap();
    assert_eq!(binds("time"), StrictIdentity::binding(true, false));
    assert_eq!(binds("location"), StrictIdentity::binding(false, true));
    assert_eq!(binds("time,location"), StrictIdentity::binding(true, true));
    assert_eq!(binds("location, time"), StrictIdentity::binding(true, true));
    assert_eq!(binds("none"), StrictIdentity::NONE);
    assert_eq!(binds("advisory"), StrictIdentity::ADVISORY);
    // The in-flight check is on under every selection but `none`.
    assert!(binds("time").in_flight() && binds("location").in_flight());
    assert!(binds("advisory").in_flight());
    assert!(!binds("none").in_flight());
    for text in ["time", "location", "time,location", "advisory", "none"] {
        assert_eq!(binds(text).to_string(), text, "a selection reads back as written");
    }
    // What the bare flag means, spelled as clap substitutes it (the test
    // below reads the attribute).
    assert_eq!(binds("time,location"), StrictIdentity::binding(true, true));

    assert!(parse_strict_identity("none,time").is_err(), "`none` is exclusive");
    assert!(parse_strict_identity("advisory,time").is_err(), "`advisory` is exclusive");
    assert!(parse_strict_identity("advisory,none").is_err(), "and so is the pair");
    let unknown = parse_strict_identity("path").unwrap_err();
    assert!(unknown.contains("`advisory`"), "an unknown term names the four: {unknown}");
    assert!(parse_strict_identity("time,").is_err(), "an empty term is not silence");
    assert!(parse_strict_identity("").is_err());
}
