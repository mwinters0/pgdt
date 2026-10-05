//! Which allocator `pgdt` links against, and nothing else.
//!
//! The choice is the binary's, never the library's: see
//! `docs/design/decisions.md`, "D13". The consequence is that every figure in
//! `docs/design/measurements.md` is a **CLI** figure, taken under whatever
//! this module selects; an embedder inherits whatever their own binary chose.
//!
//! The default is mimalloc, the allocator DataFusion's own CLI links; `system`
//! and `jemalloc` are the opt-in legs `measurements.md`, "Which allocator a figure was taken
//! under" times against it.
//!
//! [`VERSION`] is why this module is readable from outside the process:
//! `pgdt --version` names the allocator, so the measurement harness can *ask a
//! binary* which one it links against instead of trusting the flags it thinks
//! it passed.

/// Two allocator features ask for two `#[global_allocator]`s — or, with
/// `system`, for one the build does not name — which `rustc` reports as a
/// symbol collision or not at all. Refuse every pair here instead, with a
/// precedence rule deliberately *not* chosen: a silent winner would let the
/// harness label a leg by the feature it passed rather than by the allocator
/// it got. A leg is built with `--no-default-features` for this reason.
#[cfg(any(
    all(feature = "jemalloc", feature = "mimalloc"),
    all(feature = "jemalloc", feature = "system"),
    all(feature = "mimalloc", feature = "system"),
))]
compile_error!(
    "`mimalloc`, `system` and `jemalloc` are one choice, not several: enable exactly one, \
     building a leg with `--no-default-features --features <leg>`. The two besides the \
     default are measured legs of `measure.py --figure allocator`, not a matrix."
);

/// And none is refused too, rather than falling back to the platform
/// allocator: that fallback is the `system` leg under a name nobody chose, and
/// `--no-default-features` alone is exactly the build that would reach it.
#[cfg(not(any(
    feature = "jemalloc",
    feature = "mimalloc",
    feature = "system",
    feature = "introspect"
)))]
compile_error!(
    "no allocator named: enable one of `mimalloc` (the default), `system` or `jemalloc`."
);

/// `introspect` is a `#[global_allocator]` of its own, and it is refused
/// beside `system` and `jemalloc` for more than the symbol collision: the
/// instrument counts allocations in front of mimalloc and reads mimalloc's own
/// statistics for that same heap, so beside either it would report a heap the
/// build does not name. Beside `mimalloc` — the default — it is the same
/// build, the counter's heap *being* mimalloc, so that pair is accepted and the
/// instrument is the allocator. See `src/introspect.rs`.
#[cfg(all(feature = "introspect", any(feature = "jemalloc", feature = "system")))]
compile_error!(
    "`introspect` counts allocations in front of mimalloc and reads mimalloc's own statistics \
     for the same heap: it is an instrument, not a leg, and cannot be combined with `system` \
     or `jemalloc`."
);

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Not under `introspect`, whose counting allocator is mimalloc with a counter
/// in front of it and is installed by `src/introspect.rs` instead.
#[cfg(all(feature = "mimalloc", not(feature = "introspect")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// What `pgdt --version` prints: the crate version and the allocator, in
/// `allocator: <name>` form.
///
/// `system` rather than `glibc` in that leg's arm: it takes whatever libc it
/// was linked against and this crate cannot tell which one. Naming the libc is
/// the *measurement's* job (`measurements.md` records the image); naming the
/// choice is this one's.
///
/// Spelled as `#[cfg]` arms rather than a `cfg!` chain because `concat!` takes
/// literals only. Each arm is guarded by the others' absence, so a refused
/// combination fails on its `compile_error!` alone rather than on a second
/// `VERSION` beside it.
#[cfg(all(
    feature = "mimalloc",
    not(feature = "introspect"),
    not(feature = "jemalloc"),
    not(feature = "system")
))]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: mimalloc)");
/// The instrumented build says so **beside** the allocator rather than in
/// place of it: it is mimalloc, with a counter in front of it.
/// `scripts/measure.py`'s `binary_allocator` refuses a binary whose
/// `--version` carries this marker, keeping an instrumented build out of every
/// timed table by construction.
#[cfg(all(feature = "introspect", not(feature = "jemalloc"), not(feature = "system")))]
pub const VERSION: &str =
    concat!(env!("CARGO_PKG_VERSION"), " (allocator: mimalloc) (instrument: counting-allocator)");
#[cfg(all(feature = "system", not(feature = "jemalloc")))]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: system)");
#[cfg(feature = "jemalloc")]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: jemalloc)");
/// Only to keep a refused build's error list to its `compile_error!`.
#[cfg(not(any(
    feature = "jemalloc",
    feature = "mimalloc",
    feature = "system",
    feature = "introspect"
)))]
pub const VERSION: &str = "";

#[cfg(test)]
mod tests {
    use super::VERSION;

    /// Every arm must be readable by the harness's `(allocator: <name>)`
    /// parse, and a default build must report `mimalloc`. `cargo test` builds
    /// the default features, so the live assertion here is the default arm;
    /// the other two are exercised by `measure.py --figure allocator`, which
    /// checks each build's `--version` before timing it, and the instrument's
    /// by `cargo test -p pgdt --features introspect`, which reports `mimalloc`.
    #[test]
    fn the_version_string_names_this_build_s_allocator() {
        let expected = if cfg!(feature = "jemalloc") {
            "jemalloc"
        } else if cfg!(feature = "system") {
            "system"
        } else {
            "mimalloc"
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
