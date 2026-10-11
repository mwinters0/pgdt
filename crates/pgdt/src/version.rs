//! What `pgdt --version` says the binary is.

use crate::{alloc, profile};

/// What `pgdt --version` prints: the crate version, `(unreleased)` unless
/// this is a release's build ([`RELEASED`]), the allocator's markers
/// ([`alloc::MARKERS`]), the Cargo profile ([`profile::MARKER`]), and the
/// DataFusion release `sql` is, which `sql`'s
/// help also closes on, `sql` carrying no `--version` of its own. Formatted
/// once at run time: `DATAFUSION_VERSION` is a path, which `concat!` refuses.
pub(crate) fn version() -> &'static str {
    static VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "{}{} {} {} (datafusion: {})",
            env!("CARGO_PKG_VERSION"),
            if RELEASED { "" } else { " (unreleased)" },
            alloc::MARKERS,
            profile::MARKER,
            datafusion_cli_pgdump::DATAFUSION_VERSION
        )
    });
    &VERSION
}

/// The variable the release workflow builds with, naming the version it
/// releases, so a release says it is one and every other build says it is
/// not. Cargo rebuilds when it changes, as for any variable `option_env!`
/// reads. Rejected: a `build.rs` reading `git describe`, which re-runs at
/// every commit and names a dirty tree after a commit it does not match. A
/// version bump alone invalidates no cache, caches being keyed on
/// `CACHE_FORMAT_VERSION` alone.
const RELEASE_VARIABLE: Option<&str> = option_env!("PGDT_RELEASE_VERSION");

/// Whether this is a release's build: the variable names this crate's own
/// version. **One naming another fails the build**, an empty one included,
/// so a workflow that read the version from somewhere else, or not at all,
/// stops before it archives rather than shipping a binary that says it is
/// unreleased. The value is compared, never printed: what it guards is an
/// empty read in CI, which the smoke run's "no `(unreleased)`" would pass, and
/// the workflow reads the manifest's version anyway to name its archives.
/// Rejected: presence alone, marking that empty read a release; the check in
/// `release.py`, which a direct `cargo build` bypasses.
const RELEASED: bool = match RELEASE_VARIABLE {
    None => false,
    Some(v) if same_str(v, env!("CARGO_PKG_VERSION")) => true,
    Some(_) => panic!("PGDT_RELEASE_VERSION names another version than this crate's"),
};

/// `==` on `str`, which a `const` cannot call.
const fn same_str(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests;
