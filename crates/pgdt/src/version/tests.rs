use super::*;

/// A build says it is unreleased exactly when the release variable is
/// absent, in the place the spec gives it: straight after the crate
/// version, the markers the harness reads following it unchanged.
#[test]
fn the_version_says_unreleased_unless_built_as_a_release() {
    let crate_version = env!("CARGO_PKG_VERSION");
    let unreleased = format!("{crate_version} (unreleased) {}", alloc::MARKERS);
    let released = format!("{crate_version} {}", alloc::MARKERS);
    let expected = if RELEASE_VARIABLE.is_none() { &unreleased } else { &released };
    assert!(version().starts_with(expected.as_str()), "{}", version());
    assert_eq!(RELEASED, RELEASE_VARIABLE.is_some());
}

/// The comparison the release variable is held to, which is `const` and
/// so hand-written.
#[test]
fn same_str_is_str_equality() {
    for (a, b) in
        [("0.1.0", "0.1.0"), ("0.1.0", "0.1.1"), ("0.1.0", "0.1.0 "), ("", "0.1.0"), ("", "")]
    {
        assert_eq!(same_str(a, b), a == b, "{a:?} against {b:?}");
    }
}
