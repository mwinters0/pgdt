//! Which allocator `pgdq` links against, and nothing else.
//!
//! **The choice is the binary's, never the library's.** A
//! `#[global_allocator]` in `pgdump_query` would impose one on every embedder,
//! and an embedder's own binary is where that call belongs — see
//! `docs/design/architecture.md`, "The allocator is the binary's
//! choice". The consequence is stated
//! rather than hidden: every figure in `docs/design/measurements.md` is a
//! **CLI** figure, taken under whatever this module selects, and an embedder
//! inherits whatever their own binary chose.
//!
//! The default is the platform allocator — glibc's `malloc` on the measured
//! apparatus — because that is what the measurement said, not because it was
//! the status quo: `measurements.md`, "Which allocator a figure was taken
//! under", carries the table. The two features stay for the re-take, since a
//! figure whose regeneration command is gone is not a figure.
//!
//! [`VERSION`] is why this module is readable from outside the process:
//! `pgdq --version` names the allocator, so the measurement harness can *ask a
//! binary* which one it links against instead of trusting the flags it thinks
//! it passed. That is the one hole a build-time constant alone leaves — the
//! day this file's default changes, `target/release/pgdq` becomes a different
//! binary and every apparatus line that still says otherwise is wrong with
//! nothing to notice.

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
/// whatever libc it was linked against, and this crate cannot tell which one
/// that is. Naming the libc is the *measurement's* job —
/// `measurements.md` records the image — and naming the choice is this one's.
///
/// Spelled as `#[cfg]` arms rather than a `cfg!` chain because `concat!` takes
/// literals only, and the point is a string a `--version` reader finds rather
/// than a value assembled at runtime.
#[cfg(all(not(feature = "jemalloc"), not(feature = "mimalloc")))]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: system)");
#[cfg(feature = "jemalloc")]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: jemalloc)");
#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (allocator: mimalloc)");

#[cfg(test)]
mod tests {
    use super::VERSION;

    /// Every arm must be readable by the harness's `(allocator: <name>)`
    /// parse, and a default build must report `system` — that is what every
    /// published figure was taken under. `cargo test` sets no feature, so the
    /// live assertion here is the default arm; the other two are exercised for
    /// real by `measure.py --figure allocator`, which builds all three and
    /// checks each one's `--version` before timing it.
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
            VERSION.ends_with(&format!("(allocator: {expected})")),
            "{VERSION} does not name {expected}"
        );
    }
}
