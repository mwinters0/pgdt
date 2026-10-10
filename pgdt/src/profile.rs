//! Which Cargo profile built this binary, as `pgdt --version` names it.
//!
//! [`MARKER`] is read from outside the process as `alloc::MARKERS` is: the
//! `release-profile` figure builds one leg per profile and asks each binary
//! which it is before timing it, rather than trusting the flag it passed
//! (`scripts/measure.py`, `binary_profile`). The name comes from
//! `build.rs`, which says how it is found.

/// `(profile: <name>)`: `release` for the shipped build, a leg's own profile
/// for each `release-profile` leg, `dev` for a debug or test build.
pub const MARKER: &str = concat!("(profile: ", env!("PGDT_PROFILE"), ")");

#[cfg(test)]
mod tests {
    use super::MARKER;

    /// The marker names a profile, not `unknown`: a Cargo that moved its
    /// target layout fails here, at the toolchain upgrade, instead of in a
    /// sitting whose legs all read alike.
    #[test]
    fn the_version_marker_names_a_profile() {
        let name = MARKER
            .strip_prefix("(profile: ")
            .and_then(|m| m.strip_suffix(')'))
            .expect("the marker's shape is what the harness parses");
        assert_ne!(name, "unknown", "build.rs could not read the profile out of OUT_DIR");
        assert!(
            !name.is_empty()
                && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{name:?} is not what the harness's `(profile: [a-z0-9-]+)` reads"
        );
    }
}
