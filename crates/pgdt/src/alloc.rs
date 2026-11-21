//! Which allocator `pgdt` links against, the one default it sets of it,
//! mimalloc's purge delay, and what that delay keeps billed.
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
//! [`MARKERS`] is why this module is readable from outside the process:
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
/// beside `system` and `jemalloc` whether or not two would collide: the
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
///
/// deficiency: KD109 — linked without `override`, so `malloc` never sees a
/// Rust allocation, and no tool hooking it — heaptrack, or valgrind's memcheck,
/// Massif and DHAT — can attribute this heap to call stacks. A `system`
/// build's recording stands in, its requests being this build's. mimalloc's
/// `MI_TRACK_VALGRIND` reports each of its blocks to valgrind by client request
/// (`track.h`, `VALGRIND_MALLOCLIKE_BLOCK`), which is the remedy; but
/// `libmimalloc-sys` exposes no feature for it, so the define is injected by
/// hand, and the result is a build carrying those requests rather than the
/// shipped one. Neither route reaches what this heap retains after a `free`:
/// Massif's `--pages-as-heap` counts mapped pages, not resident ones, charges
/// them to whichever allocation grew an arena, and runs the threads serially.
/// That retention is read by the gate's RSS and the `introspect` build's
/// mimalloc statistics, and by its block tracker (`introspect/blocks.rs`),
/// which names a block of at least a size it is told by its allocating stack and
/// reads its resident pages once freed, at a held line's moments alone.
#[cfg(all(feature = "mimalloc", not(feature = "introspect")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// mimalloc's purge delay, set as `pgdt`'s default: how long it keeps a page
/// of freed blocks committed before it purges it. On a build linking mimalloc,
/// the default, and under `introspect`, whose heap is this one with a counter
/// in front of it. This crate's one `unsafe` site outside `introspect`, two
/// calls into mimalloc through `build.rs`'s shim (`docs/design/decisions.md`,
/// "D27").
#[cfg(all(
    any(feature = "mimalloc", feature = "introspect"),
    not(feature = "jemalloc"),
    not(feature = "system")
))]
mod purge {
    /// The delay in milliseconds where the environment states none
    /// (`MIMALLOC_PURGE_DELAY`, which overrides it): `pgdt`'s default, never a
    /// library's, an embedder linking mimalloc keeping its own
    /// (`docs/design/decisions.md`, "D166").
    pub const PURGE_DELAY_MS: std::ffi::c_long = 100;

    /// **What mimalloc keeps resident of what it frees under
    /// [`PURGE_DELAY_MS`]**, in bytes: one bound for the process, billed to
    /// every arrangement `pgdt` resolves and every scan `pgdt sql` draws as
    /// held beside its readers and its lines (`pgdt_io::ArrangementRequest::retained`,
    /// `docs/design/decisions.md`, "D165"). Read off one workload's grid of
    /// reader counts; a run keeping more over a zero delay than this moves it.
    /// It bounds this delay alone: a delay the environment states is billed
    /// this all the same.
    ///
    /// Bounded here rather than per buffer: a held line's freed copy is a
    /// small part of what the delay keeps at the peak, beside the view
    /// builder's blocks, the decoded-block buffers and DataFusion's own. So
    /// giving a draw's buffers back outside the allocator, or every buffer a
    /// scan frees, reaches little and never DataFusion's; charging it as a
    /// line's factor bills one allocator's habit to every line; and no library
    /// change suiting both allocators reaches it, the view builder's blocks
    /// leaving with their batch and a pooled buffer kept longer holding the
    /// same pages live.
    pub const RETAINED_BOUND: u64 = 320 << 20;

    // The shim `build.rs` compiles against mimalloc's own header
    // (`src/alloc/purge.c`), naming the option `libmimalloc-sys` declares no
    // constant for. Its setters write mimalloc's option table unsynchronized.
    // upstream: UF6
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn pgdt_purge_delay_set_default(ms: std::ffi::c_long);
        #[cfg(all(test, feature = "introspect"))]
        fn pgdt_purge_delay_set(ms: std::ffi::c_long);
        fn pgdt_purge_delay() -> std::ffi::c_long;
    }

    /// Sets [`PURGE_DELAY_MS`] as the purge delay unless the environment
    /// stated one, which mimalloc read as the process loaded and which this
    /// leaves in place (`mi_option_set_default`). Called first in `main`,
    /// while one thread runs, the option's setter not being thread-safe;
    /// mimalloc reads the delay afresh at each purge it schedules, so every
    /// page the commands free is purged under it (`docs/design/decisions.md`,
    /// "D166").
    pub fn configure() {
        // SAFETY: the setter takes any value, and runs here before a second
        // thread exists.
        #[allow(unsafe_code)]
        unsafe {
            pgdt_purge_delay_set_default(PURGE_DELAY_MS)
        };
    }

    /// Sets `ms` as the purge delay over whatever is set, for the block
    /// tracker's test: -1 never purges.
    ///
    /// # Safety
    ///
    /// No other thread may set a mimalloc option meanwhile.
    #[cfg(all(test, feature = "introspect"))]
    #[allow(unsafe_code)]
    pub unsafe fn set_for_test(ms: std::ffi::c_long) {
        // SAFETY: the caller's.
        unsafe { pgdt_purge_delay_set(ms) };
    }

    /// ` (purge delay: <ms> ms)`: the delay mimalloc runs under once
    /// [`configure`] has run, [`PURGE_DELAY_MS`] or what the environment
    /// stated, read back from mimalloc.
    pub fn marker() -> String {
        // SAFETY: the getter reads one option, set before a second thread.
        #[allow(unsafe_code)]
        let delay = unsafe { pgdt_purge_delay() };
        format!(" (purge delay: {delay} ms)")
    }
}

/// A build linking another allocator sets nothing and names nothing.
#[cfg(not(all(
    any(feature = "mimalloc", feature = "introspect"),
    not(feature = "jemalloc"),
    not(feature = "system")
)))]
mod purge {
    /// Nothing billed: no reading prices what this allocator keeps
    /// (`docs/design/decisions.md`, "D165").
    pub const RETAINED_BOUND: u64 = 0;

    pub fn configure() {}

    pub fn marker() -> String {
        String::new()
    }
}

/// For the block tracker's test, which tells mimalloc never to purge.
#[cfg(all(test, feature = "introspect", not(feature = "jemalloc"), not(feature = "system")))]
pub use purge::set_for_test as set_purge_delay_for_test;

/// Sets the allocator's defaults: mimalloc's purge delay (`purge`), first
/// thing in `main`.
pub use purge::configure;

/// What the allocator is billed for keeping of what it frees, beside every
/// arrangement's readers (`purge`'s `RETAINED_BOUND`).
pub use purge::RETAINED_BOUND;

/// What `pgdt --version` names of the purge delay mimalloc runs under, so the
/// harness asks the binary rather than trusting the variable it set
/// (`scripts/measurement/builds/leg_binaries.py`, `binary_purge_delay`); `MIMALLOC_VERBOSE` prints
/// the options as the process loads, before `pgdt` sets its default. Empty on
/// a build linking another allocator.
///
/// The one marker that moves with the environment rather than the build, and
/// in `--version` all the same: the delay a process runs under moves its
/// resident peak, so a memory report pasting `--version` should carry it, and
/// nothing reading the line compares it whole. Rejected: a flag or subcommand
/// of its own, surface the harness alone would read.
pub use purge::marker as purge_delay_marker;

/// What `pgdt --version` names of the allocator: `(allocator: <name>)`, and
/// the instrument's marker where there is one. `version.rs`'s `version` puts it
/// after the crate version and before DataFusion's.
///
/// `system` rather than `glibc` in that leg's arm: it takes whatever libc it
/// was linked against and this crate cannot tell which one. Naming the libc is
/// the *measurement's* job (`measurements.md` records the image); naming the
/// choice is this one's.
///
/// Spelled as `#[cfg]` arms rather than a `cfg!` chain so each is a constant.
/// The arms take a precedence — `jemalloc`, `system`, `introspect`, `mimalloc`
/// — so a refused combination never adds a second `MARKERS` to its
/// `compile_error!`; `jemalloc` beside `mimalloc` still names two `GLOBAL`s.
#[cfg(all(
    feature = "mimalloc",
    not(feature = "introspect"),
    not(feature = "jemalloc"),
    not(feature = "system")
))]
pub const MARKERS: &str = "(allocator: mimalloc)";
/// The instrumented build says so **beside** the allocator rather than in
/// place of it: it is mimalloc, with a counter in front of it.
/// `scripts/measurement/builds/leg_binaries.py`'s `binary_allocator` refuses a binary whose
/// `--version` carries this marker, keeping an instrumented build out of every
/// timed table by construction.
#[cfg(all(feature = "introspect", not(feature = "jemalloc"), not(feature = "system")))]
pub const MARKERS: &str = "(allocator: mimalloc) (instrument: counting-allocator)";
#[cfg(all(feature = "system", not(feature = "jemalloc")))]
pub const MARKERS: &str = "(allocator: system)";
#[cfg(feature = "jemalloc")]
pub const MARKERS: &str = "(allocator: jemalloc)";
/// Only to keep a refused build's error list to its `compile_error!`.
#[cfg(not(any(
    feature = "jemalloc",
    feature = "mimalloc",
    feature = "system",
    feature = "introspect"
)))]
pub const MARKERS: &str = "";

#[cfg(test)]
mod tests {
    use super::MARKERS;

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
            MARKERS.contains(&format!("(allocator: {expected})")),
            "{MARKERS} does not name {expected}"
        );
    }

    /// Only an instrumented build carries the instrument marker, in either
    /// direction: a default build that grew one is refused by the harness, and
    /// an instrumented build that lost one would be timed as the shipped
    /// binary.
    #[test]
    fn the_instrument_marker_is_present_exactly_when_the_feature_is() {
        assert_eq!(
            MARKERS.contains("(instrument: counting-allocator)"),
            cfg!(feature = "introspect"),
            "{MARKERS} disagrees with the `introspect` feature"
        );
    }
}
