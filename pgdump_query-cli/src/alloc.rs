//! Which allocator `pgdq` links against, and nothing else.
//!
//! The choice is the binary's, never the library's: see
//! `docs/design/decisions.md`, "D13". The consequence is that every figure in
//! `docs/design/measurements.md` is a **CLI** figure, taken under whatever
//! this module selects; an embedder inherits whatever their own binary chose.
//!
//! The default is the platform allocator, and the two features stay for the
//! re-take — `measurements.md`, "Which allocator a figure was taken under".
//!
//! [`VERSION`] is why this module is readable from outside the process:
//! `pgdq --version` names the allocator, so the measurement harness can *ask a
//! binary* which one it links against instead of trusting the flags it thinks
//! it passed.

/// Enabling both features asks for two `#[global_allocator]`s, which is a
/// `rustc` error whose message is about symbol collision rather than about the
/// build being wrong. Refuse it here instead, with a precedence rule
/// deliberately *not* chosen: a silent winner would let the harness label a
/// leg by the feature it passed rather than by the allocator it got.
#[cfg(all(feature = "jemalloc", feature = "mimalloc"))]
compile_error!(
    "`jemalloc` and `mimalloc` are one choice, not two: enable at most one. \
     They are measured legs of `measure.py --figure allocator`, not a matrix."
);

/// `introspect` is the third `#[global_allocator]` in this crate, and it is
/// refused beside the other two for more than the symbol collision: the
/// instrument counts allocations in front of `System` and reads glibc's
/// `mallinfo2`/`malloc_info` for that same heap, so beside `jemalloc` or
/// `mimalloc` it would count one heap and report another's. See
/// `src/introspect.rs`.
#[cfg(all(feature = "introspect", any(feature = "jemalloc", feature = "mimalloc")))]
compile_error!(
    "`introspect` counts allocations in front of the platform allocator and reads glibc's own \
     statistics for the same heap: it is an instrument, not a leg, and cannot be combined with \
     `jemalloc` or `mimalloc`."
);

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// What `pgdq --version` prints: the crate version and the allocator, in
/// `allocator: <name>` form.
///
/// `system` rather than `glibc` in the default arm: a feature-free build takes
/// whatever libc it was linked against and this crate cannot tell which one.
/// Naming the libc is the *measurement's* job (`measurements.md` records the
/// image); naming the choice is this one's.
///
/// Spelled as `#[cfg]` arms rather than a `cfg!` chain because `concat!` takes
/// literals only.
#[cfg(all(not(feature = "jemalloc"), not(feature = "mimalloc"), not(feature = "introspect")))]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: system)");
/// The instrumented build says so **beside** the allocator rather than in
/// place of it: it is still the platform allocator, with a counter in front of
/// it. `scripts/measure.py`'s `binary_allocator` refuses a binary whose
/// `--version` carries this marker, keeping an instrumented build out of every
/// timed table by construction.
#[cfg(all(not(feature = "jemalloc"), not(feature = "mimalloc"), feature = "introspect"))]
pub const VERSION: &str =
    concat!(env!("CARGO_PKG_VERSION"), " (allocator: system) (instrument: counting-allocator)");
#[cfg(feature = "jemalloc")]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: jemalloc)");
#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: mimalloc)");

#[cfg(test)]
mod tests {
    use super::VERSION;

    /// Every arm must be readable by the harness's `(allocator: <name>)`
    /// parse, and a default build must report `system`. `cargo test` sets no
    /// feature, so the live assertion here is the default arm; the other two
    /// are exercised by `measure.py --figure allocator`, which checks each
    /// build's `--version` before timing it.
    #[test]
    fn the_version_string_names_this_build_s_allocator() {
        let expected = if cfg!(feature = "jemalloc") {
            "jemalloc"
        } else if cfg!(feature = "mimalloc") {
            "mimalloc"
        } else {
            "system"
        };
        assert!(
            VERSION.contains(&format!("(allocator: {expected})")),
            "{VERSION} does not name {expected}"
        );
    }

    /// Only an instrumented build carries the instrument marker, in either
    /// direction: a default build that grew one is refused by the harness, and
    /// an instrumented build that lost one would be timed as the shipped
    /// binary.
    #[test]
    fn the_instrument_marker_is_present_exactly_when_the_feature_is() {
        assert_eq!(
            VERSION.contains("(instrument: counting-allocator)"),
            cfg!(feature = "introspect"),
            "{VERSION} disagrees with the `introspect` feature"
        );
    }
}
