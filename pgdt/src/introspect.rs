//! What the process can say about its own memory, when it is built to say it.
//!
//! **Off by default and never in a shipped binary.** The whole module is
//! behind the `introspect` Cargo feature; without it every item here is a
//! no-op and the binary is byte-for-byte the one that ships. With it, `pgdt`
//! installs a counting `#[global_allocator]` over `mimalloc::MiMalloc` and
//! writes, on the way out, what the program held, what mimalloc was holding
//! for it, and what glibc was holding for the C code beside it.
//!
//! This is the introspective half of `docs/design/roadmap.md`, "Attribution is
//! introspective; only the gate is blind". What each instrument sees, what it
//! is blind to and what it costs is `docs/design/measurements.md`, "What an
//! instrument can see"; the mechanism's entry is
//! `docs/design/decisions.md`, "D13".
//!
//! # What it reports
//!
//! A snapshot at exit, and no sampler: a quantity whose peak is reported
//! carries its own high-water, and the rest are read as they stand at exit:
//!
//! * **`live_bytes` / `live_peak_bytes`** — exact bytes the *program* asked
//!   for and had not freed, and the largest that figure ever reached, kept by
//!   the counting allocator itself.
//! * **`mimalloc_*`** — mimalloc's own statistics at exit, for the heap the
//!   counter stands in front of: `committed` and `reserved`, each now and at
//!   its high-water, plus the raw JSON `mi_stats_get_json` returns. On an
//!   overcommitting kernel mimalloc counts an arena's slices as committed when
//!   it first hands them out and uncounts them when it purges, so the gap
//!   between `mimalloc_committed_peak_bytes` and `live_peak_bytes` is
//!   mimalloc's bookkeeping and retention, each a high-water of its own.
//!   Beside them, `mimalloc_purge_delay`: the option as mimalloc holds it,
//!   so a variable set in front of the process is read back rather than
//!   assumed to have arrived.
//! * **`mallinfo_*`** — glibc's own view at exit, of what reached C `malloc`:
//!   `arena` (arena-backed bytes obtained from the OS), `hblkhd`
//!   (mmap-backed), `uordblks` (in use) and `fordblks` (freed, held, still
//!   resident). The gap between `arena` and `uordblks` is retention.
//! * **`malloc_*`** — `malloc_info`'s document-level totals, plus the raw XML,
//!   which carries **each arena's own `system type="max"`** — the one number
//!   `mallinfo2` cannot give.
//! * **`statistics_*`** — where a `parse` ran: the library's statistics
//!   account as the pass returned it, beside the live bytes the counter
//!   attributed to statistics (`pgdump_query::instrument`) at the same moment,
//!   their peaks, and the worst difference either way any update of the
//!   account read (`docs/design/decisions.md`, "D81"). Where no pass returned
//!   an account — a `query`, which loads a cache's statistics and bills them
//!   only as held when its workers are carved, or a `parse --preamble-only`,
//!   which runs no mapping pass — the lines are `statistics_account=none`,
//!   `statistics_loaded_bytes`, the heap the cache handed the pass, which is
//!   the only statistics term a query has, and the counter's live bytes and
//!   their peak.
//!
//! **The process has two heaps, and the families do not cover the same
//! memory**, so the report labels each: `live_scope`, `mimalloc_scope` and
//! `glibc_scope`, with the note between them. The counter and mimalloc see
//! what passes through Rust's `GlobalAlloc`; glibc sees what reaches C
//! `malloc` — `liblzma`, the active `.xz` backend, whose share of a reader's
//! decoder working set (`xz_seek::Layout::decoder_bytes`) is there and
//! nowhere else, `aws-lc`, and libc itself. mimalloc is linked without
//! `override`, so C keeps the platform allocator: neither family is the whole
//! process, and only their sum is the heap.
//!
//! * **`evaluation_*`** — where `pgdt sql` ran: what
//!   `datafusion_cli_pgdump::introspection_section` timed of a dynamic
//!   filter's row evaluation, under its own `evaluation_scope`, after the
//!   allocator's sections.
//!
//! All of it goes to **the file [`OUT_VAR`] names**, and nowhere at all when
//! that variable is unset — see [`report`].
//!
//! **Not a fourth allocator leg**, and **this build never times anything**:
//! `pgdt --version` names the instrument and `binary_allocator` in
//! `scripts/measure.py` refuses such a binary.
//!
//! The instrument must not move the plan it reports on, so `main.rs`'s
//! `the_instrument_build_resolves_what_the_default_build_resolves` pins the
//! resolved `jobs=`/`memory_bytes=` pair across every committed runtime root.
//! It is compiled into **both** configurations, so
//! `cargo test -p pgdt --features introspect` re-runs that assertion under the
//! counting allocator.

/// The environment variable naming the file [`report`] writes to.
///
/// **Unset means no report at all**, which is the only state a default build
/// can be in. Shared in fact rather than in type with
/// `measure.INSTRUMENT_OUT_VAR`; `scripts/test_measure.py` holds the two to
/// each other.
///
/// Compiled into both configurations; only the feature build reads it.
#[cfg_attr(not(feature = "introspect"), allow(dead_code))]
pub const OUT_VAR: &str = "PGDT_INTROSPECT_OUT";

/// Which sections a report carries: the allocator's always, and the SQL
/// shell's dynamic-filter timings where `pgdt sql` ran.
#[derive(Clone, Copy)]
pub enum Sections {
    Allocator,
    AllocatorAndEvaluation,
}

/// Writes the report when it goes out of scope.
///
/// Held in `commands`, so the report is emitted on the ordinary return **and**
/// on an error propagated out of it, while tokio's blocking pool threads are
/// still alive — a per-thread arena already torn down reports nothing. `sql`'s
/// is held in `main` around `datafusion_cli_pgdump::run`, whose runtime is its
/// own and is gone by then. The interrupt path's `std::process::exit` calls
/// [`report`] itself; a second signal's exit, and clap's own on a usage error,
/// `--help` or `--version`, write none.
pub struct AtExit(Sections);

/// Record what a mapping pass's statistics account held when it returned,
/// with the instrument's own count read at the same moment, for [`report`]
/// to print. A no-op without the `introspect` feature.
pub fn statistics_returned(held: &pgdump_query::StatisticsHeld) {
    #[cfg(feature = "introspect")]
    enabled::statistics_returned(held);
    #[cfg(not(feature = "introspect"))]
    let _ = held;
}

/// Arm the report. A no-op without the `introspect` feature, where [`report`]
/// has nothing to write.
pub fn at_exit(sections: Sections) -> AtExit {
    AtExit(sections)
}

impl Drop for AtExit {
    fn drop(&mut self) {
        report(self.0);
    }
}

/// Write the `key=value` lines this build can answer to the file [`OUT_VAR`]
/// names. Nothing without the feature, and nothing with the variable unset.
///
/// **A file, not a stream**: one writer by construction, carrying
/// `malloc_info`'s XML without riding a log. **An environment variable rather
/// than a flag**, so the command shape is identical to the one a sweep times.
///
/// A write that fails says so on stderr — an error, not the report: a silent
/// failure is the one outcome a reader cannot tell from a build without the
/// feature (`docs/design/decisions.md`, "D13").
pub fn report(sections: Sections) {
    #[cfg(feature = "introspect")]
    enabled::write_report(sections);
    #[cfg(not(feature = "introspect"))]
    let _ = sections;
}

#[cfg(feature = "introspect")]
mod enabled {
    use mimalloc::MiMalloc;
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Bytes the program has asked for and not yet freed.
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    /// The largest [`LIVE`] ever reached.
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    /// [`MiMalloc`] with a counter in front of it.
    ///
    /// **Over mimalloc specifically**: the `mimalloc_*` statistics below
    /// describe the allocator underneath, so counting another allocator's
    /// allocations would point two instruments at different heaps. Hence
    /// `alloc.rs`'s guard.
    pub struct Counting;

    /// `Relaxed` throughout: the atomics are read only after every thread
    /// that touched them has finished its work, so nothing here orders
    /// anything else. Every value `LIVE` takes is one `fetch_add`'s result,
    /// which that thread then hands to `fetch_max`, so `PEAK` is the
    /// counter's high-water however allocations interleave.
    fn took(bytes: usize) {
        let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK.fetch_max(live, Ordering::Relaxed);
        pgdump_query::instrument::allocated(bytes);
    }

    fn gave_back(bytes: usize) {
        LIVE.fetch_sub(bytes, Ordering::Relaxed);
        pgdump_query::instrument::freed(bytes);
    }

    /// What [`super::statistics_returned`] recorded: the account and the
    /// instrument's reading, taken together.
    static RETURNED: std::sync::Mutex<
        Option<(pgdump_query::StatisticsHeld, pgdump_query::instrument::StatisticsReading)>,
    > = std::sync::Mutex::new(None);

    pub fn statistics_returned(held: &pgdump_query::StatisticsHeld) {
        let reading = pgdump_query::instrument::statistics_reading();
        if let Ok(mut returned) = RETURNED.lock() {
            *returned = Some((*held, reading));
        }
    }

    /// The `statistics_*` lines.
    ///
    /// **Two shapes, because only a mapping pass returns an account**, whose
    /// terms are the reading; a `parse --preamble-only` runs none, and a
    /// `query` returns none — it loads the cache, bills it only as held when its
    /// workers are carved, and prints no `statistics held` line. The second shape says `statistics_account=none`
    /// and leans on `statistics_loaded_bytes`, the heap the cache handed the
    /// pass, beside the scope counter's live bytes and their peak.
    fn push_statistics(out: &mut String) {
        let Some((held, reading)) = RETURNED.lock().ok().and_then(|r| *r) else {
            let reading = pgdump_query::instrument::statistics_reading();
            out.push_str("statistics_scope=library-statistics-scope\n");
            out.push_str("statistics_account=none\n");
            out.push_str(&format!("statistics_loaded_bytes={}\n", reading.loaded));
            out.push_str(&format!("statistics_live_bytes={}\n", reading.live));
            out.push_str(&format!("statistics_live_peak_bytes={}\n", reading.live_peak));
            return;
        };
        let terms = [
            ("retained", held.now.retained, held.term_peaks.retained),
            ("loaded", held.now.loaded, held.term_peaks.loaded),
            ("gathering", held.now.gathering, held.term_peaks.gathering),
            ("pieces", held.now.pieces, held.term_peaks.pieces),
            ("interned", held.now.interned, held.term_peaks.interned),
        ];
        out.push_str("statistics_scope=library-statistics-scope\n");
        out.push_str(&format!("statistics_account_bytes={}\n", held.now.total()));
        out.push_str(&format!("statistics_account_peak_bytes={}\n", held.peak));
        out.push_str(&format!("statistics_loaded_bytes={}\n", reading.loaded));
        for (term, now, peak) in terms {
            out.push_str(&format!("statistics_account_{term}_bytes={now}\n"));
            out.push_str(&format!("statistics_account_{term}_peak_bytes={peak}\n"));
        }
        out.push_str(&format!("statistics_live_bytes={}\n", reading.live));
        out.push_str(&format!("statistics_live_peak_bytes={}\n", reading.live_peak));
        out.push_str(&format!("statistics_checks={}\n", reading.checks));
        out.push_str(&format!("statistics_allowance_peak_bytes={}\n", reading.allowance_peak));
        out.push_str(&format!("statistics_worst_short_bytes={}\n", reading.short));
        out.push_str(&format!(
            "statistics_worst_short_past_allowance_bytes={}\n",
            reading.short_past_allowance
        ));
        out.push_str(&format!("statistics_worst_over_bytes={}\n", reading.over));
        out.push_str(&format!(
            "statistics_worst_over_past_allowance_bytes={}\n",
            reading.over_past_allowance
        ));
    }

    // SAFETY: every method forwards to `MiMalloc`, which satisfies the trait's
    // contract, and the counters are plain atomics that allocate nothing.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { MiMalloc.alloc(layout) };
            if !ptr.is_null() {
                took(layout.size());
            }
            ptr
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { MiMalloc.alloc_zeroed(layout) };
            if !ptr.is_null() {
                took(layout.size());
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            gave_back(layout.size());
            unsafe { MiMalloc.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let out = unsafe { MiMalloc.realloc(ptr, layout, new_size) };
            if !out.is_null() {
                // Only the difference: a `realloc` that grows a block never
                // holds both sizes at once from the program's point of view,
                // so charging the new size and crediting the old would put a
                // step in the high-water that no allocation made.
                if new_size >= layout.size() {
                    took(new_size - layout.size());
                } else {
                    gave_back(layout.size() - new_size);
                }
            }
            out
        }
    }

    #[global_allocator]
    static GLOBAL: Counting = Counting;

    /// Write [`report_text`], and `sql`'s section after it where `sections`
    /// asks, to the file [`super::OUT_VAR`] names, or do nothing at all where
    /// the variable is unset.
    ///
    /// **The whole file is rewritten, and the last writer wins.** One process
    /// writes one report, at its own exit, so there is nothing to append to,
    /// and a run re-using a previous run's path cannot be read as that run's.
    pub fn write_report(sections: super::Sections) {
        let Some(path) = std::env::var_os(super::OUT_VAR) else { return };
        let path = std::path::PathBuf::from(path);
        let mut text = report_text();
        if let super::Sections::AllocatorAndEvaluation = sections {
            text.push_str(&datafusion_cli_pgdump::introspection_section());
        }
        if let Err(err) = std::fs::write(&path, text) {
            // Not the report — an error saying there is none.
            eprintln!(
                "pgdt: the introspection report could not be written to {}: {err}",
                path.display()
            );
        }
    }

    /// The whole report, as text, so the formatting is testable without a
    /// process to run. Every quantity carries its scope, and [`SCOPE_NOTE`]
    /// sits between the two families; the note's lines carry no `=`, so
    /// `measure.parse_reported` ignores them as it ignores the XML below.
    pub fn report_text() -> String {
        let mut out = String::from("instrument=counting-allocator\n");
        out.push_str("live_scope=rust-global-alloc\n");
        out.push_str(&format!("live_bytes={}\n", LIVE.load(Ordering::Relaxed)));
        out.push_str(&format!("live_peak_bytes={}\n", PEAK.load(Ordering::Relaxed)));
        push_statistics(&mut out);
        out.push_str(SCOPE_NOTE);
        out.push_str("mimalloc_scope=rust-heap\n");
        push_mimalloc(&mut out);
        out.push_str("glibc_scope=c-malloc\n");
        push_glibc(&mut out);
        out
    }

    /// What the three scopes mean, in the report itself rather than only in
    /// the document that explains it. What it names is
    /// `xz_seek::Layout::decoder_bytes`, not `decode_footprint`: xz-seek's
    /// input chunk passes through `GlobalAlloc` (`docs/design/decisions.md`, "D15").
    const SCOPE_NOTE: &str = concat!(
        "# `live_*` counts only what passed through Rust's `GlobalAlloc`, and\n",
        "# `mimalloc_*` is the heap that serves it: their gap is mimalloc's\n",
        "# bookkeeping and retention. `mallinfo_*` and `malloc_*` are glibc's\n",
        "# view of what reached C `malloc` and nothing else: `liblzma` is the\n",
        "# active `.xz` backend and allocates its dictionary and state\n",
        "# (`xz_seek::Layout::decoder_bytes`) a reader there, which the counter\n",
        "# cannot see. The process has two heaps; neither family is all of it.\n",
    );

    /// The `mimalloc_*` lines, then `mi_stats_get_json`'s document verbatim.
    ///
    /// **Read through mimalloc's own JSON rather than its struct**, because
    /// the struct's layout is versioned (`MI_STAT_VERSION`) and the JSON
    /// carries the version beside the fields; a field the document lacks
    /// prints nothing rather than a zero that reads like a reading.
    fn push_mimalloc(out: &mut String) {
        out.push_str(&format!("mimalloc_purge_delay={}\n", purge_delay()));
        let Some(json) = mimalloc_stats_json() else {
            out.push_str("# mi_stats_get_json: mimalloc returned no document\n");
            return;
        };
        for (key, value) in super::mimalloc_json::readings_of(&json) {
            out.push_str(&format!("{key}={value}\n"));
        }
        // Verbatim, after the keys: no line of it is `key=value`, so
        // `measure.parse_reported` reads none of it.
        out.push_str("# mi_stats_json\n");
        out.push_str(&json);
        if !json.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("# end mi_stats_json\n");
    }

    /// `mi_option_purge_delay`'s index in mimalloc 3.3.2's `mi_option_e`
    /// (`include/mimalloc.h`), counted by hand: `libmimalloc-sys` 0.1.49
    /// declares the options either side of it and not it.
    /// `the_purge_delay_read_back_is_purge_delay_s` holds the count.
    const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;

    /// How long mimalloc waits before purging a freed page, in milliseconds,
    /// as it holds it: its default, or what `MIMALLOC_PURGE_DELAY` set at
    /// init.
    fn purge_delay() -> std::ffi::c_long {
        // SAFETY: reads one option by a valid index, initialising it from the
        // environment on first use, which mimalloc does under its own lock.
        unsafe { libmimalloc_sys::mi_option_get(MI_OPTION_PURGE_DELAY) }
    }

    /// `mi_stats_get_json`'s document, statistics merged over the process's
    /// heaps.
    ///
    /// The buffer is mimalloc's own — allocated inside the call, not through
    /// the global allocator — so it is released with `mi_free` and never
    /// reaches [`Counting`]'s counters.
    fn mimalloc_stats_json() -> Option<String> {
        // SAFETY: a zero size and a null buffer ask mimalloc to allocate the
        // document itself, which it returns NUL-terminated or as null.
        let buf = unsafe { libmimalloc_sys::mi_stats_get_json(0, std::ptr::null_mut()) };
        if buf.is_null() {
            return None;
        }
        // SAFETY: non-null, NUL-terminated, and live until the `mi_free` below.
        let text = unsafe { std::ffi::CStr::from_ptr(buf) }.to_string_lossy().into_owned();
        unsafe { libmimalloc_sys::mi_free(buf.cast()) };
        Some(text)
    }

    #[cfg(target_env = "gnu")]
    fn push_glibc(out: &mut String) {
        // SAFETY: `mallinfo2` takes nothing, returns a plain struct of
        // `size_t`s, and is safe to call from any thread.
        let info = unsafe { libc::mallinfo2() };
        out.push_str(&format!("mallinfo_arena={}\n", info.arena));
        out.push_str(&format!("mallinfo_hblkhd={}\n", info.hblkhd));
        out.push_str(&format!("mallinfo_uordblks={}\n", info.uordblks));
        out.push_str(&format!("mallinfo_fordblks={}\n", info.fordblks));
        match malloc_info_xml() {
            Some(xml) => {
                let totals = super::xml::totals_of(&xml);
                out.push_str(&format!("malloc_heaps={}\n", totals.heaps));
                out.push_str(&format!("malloc_system_current={}\n", totals.system_current));
                out.push_str(&format!("malloc_system_max={}\n", totals.system_max));
                // Verbatim, after the keys: every line fails
                // `measure.parse_reported`'s `key=value` match.
                out.push_str("# malloc_info\n");
                out.push_str(&xml);
                if !xml.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("# end malloc_info\n");
            }
            None => out.push_str("# malloc_info: the stream could not be opened\n"),
        }
    }

    #[cfg(not(target_env = "gnu"))]
    fn push_glibc(out: &mut String) {
        // The counting half is allocator-independent and still exact; the
        // glibc half does not exist off glibc, and saying so beats zeros that
        // read like an instrument that works.
        out.push_str("# mallinfo2/malloc_info: this build is not linked against glibc\n");
    }

    /// `malloc_info`'s XML, captured through `open_memstream`.
    ///
    /// The buffer is glibc's own — allocated inside `open_memstream`, not
    /// through the global allocator — so it must be released with `libc::free`
    /// and never reaches [`Counting`]'s counters.
    #[cfg(target_env = "gnu")]
    fn malloc_info_xml() -> Option<String> {
        let mut buf: *mut libc::c_char = std::ptr::null_mut();
        let mut len: libc::size_t = 0;
        // SAFETY: both out-parameters are live for the call, and the stream is
        // closed exactly once below. `fclose` is what flushes `buf`/`len`.
        let stream = unsafe { libc::open_memstream(&mut buf, &mut len) };
        if stream.is_null() {
            return None;
        }
        let rc = unsafe { libc::malloc_info(0, stream) };
        unsafe { libc::fclose(stream) };
        let text = if rc == 0 && !buf.is_null() {
            let bytes = unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), len) };
            Some(String::from_utf8_lossy(bytes).into_owned())
        } else {
            None
        };
        if !buf.is_null() {
            unsafe { libc::free(buf.cast()) };
        }
        text
    }
}

/// The parse over `malloc_info`'s XML, and nothing else.
///
/// Compiled whenever the tests are, not only under the feature: the parse is
/// ordinary string work whose failure mode goes unnoticed — a number read off
/// the wrong element still looks like a plausible byte count.
#[cfg(any(feature = "introspect", test))]
mod xml {
    /// `malloc_info`'s document-level totals — the sums over every arena,
    /// which glibc prints after the last `<heap>` element.
    #[derive(Debug, PartialEq, Eq)]
    pub struct Totals {
        pub heaps: usize,
        pub system_current: u64,
        pub system_max: u64,
    }

    /// Read the totals out of `malloc_info`'s XML.
    ///
    /// **The last occurrence wins, and that is the whole parse.** Each
    /// `<heap>` carries its own `<system type="current"/>` and `<system
    /// type="max"/>`, and glibc prints the document-level pair after all of
    /// them, so "the last one" *is* "the total". Per-arena values are not
    /// summed here; they are in the XML the report prints verbatim.
    pub fn totals_of(xml: &str) -> Totals {
        Totals {
            heaps: xml.matches("<heap nr=").count(),
            system_current: last_size(xml, "<system type=\"current\" size=\"").unwrap_or(0),
            system_max: last_size(xml, "<system type=\"max\" size=\"").unwrap_or(0),
        }
    }

    /// The number following the last occurrence of `prefix`, or `None` where
    /// the document does not carry one — which is what a glibc too old for
    /// this element, or a truncated capture, looks like.
    fn last_size(xml: &str, prefix: &str) -> Option<u64> {
        let tail = xml.rfind(prefix)? + prefix.len();
        let digits: String = xml[tail..].chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }
}

/// The read over `mi_stats_get_json`'s document, and nothing else.
///
/// Compiled whenever the tests are, not only under the feature, for `xml`'s
/// reason: a number read off the wrong field still looks like a byte count.
#[cfg(any(feature = "introspect", test))]
mod mimalloc_json {
    /// The `mimalloc_*` lines' keys and values, in the order they print.
    ///
    /// `committed` and `reserved` are each a `{ total, peak, current }` count
    /// in mimalloc's statistics; `total` is cumulative and is not read. A
    /// document that does not parse, or lacks a field, yields nothing for it.
    pub fn readings_of(json: &str) -> Vec<(String, i64)> {
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(json) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(v) = doc.get("mimalloc_version").and_then(serde_json::Value::as_i64) {
            out.push(("mimalloc_version".to_owned(), v));
        }
        for stat in ["committed", "reserved"] {
            for (field, suffix) in [("current", "bytes"), ("peak", "peak_bytes")] {
                let value = doc.get(stat).and_then(|s| s.get(field)).and_then(|v| v.as_i64());
                if let Some(v) = value {
                    out.push((format!("mimalloc_{stat}_{suffix}"), v));
                }
            }
        }
        out
    }
}

/// The report's own shape, which only the instrument build can produce.
///
/// Compiled under the feature alone — `report_text` does not exist without it
/// — so these run in `cargo test -p pgdt --features introspect`.
#[cfg(all(test, feature = "introspect"))]
mod instrumented_tests {
    use super::enabled::report_text;

    /// The scope labels are the report's, not the reader's: a consumer
    /// differencing `live_peak_bytes` against `malloc_system_max` would
    /// subtract the Rust heap's count from C's heap, which never held it.
    #[test]
    fn every_quantity_states_which_memory_it_covers() {
        let text = report_text();
        assert!(text.contains("live_scope=rust-global-alloc\n"), "{text}");
        assert!(text.contains("mimalloc_scope=rust-heap\n"), "{text}");
        assert!(text.contains("glibc_scope=c-malloc\n"), "{text}");
    }

    /// mimalloc is the heap behind the counter, so it must report one: a test
    /// process has allocated through it before this runs, and a committed
    /// high-water of zero would be an instrument reading the wrong heap.
    #[test]
    fn mimalloc_reports_the_heap_the_counter_stands_in_front_of() {
        let text = report_text();
        let committed_peak = text
            .lines()
            .find_map(|l| l.strip_prefix("mimalloc_committed_peak_bytes="))
            .and_then(|v| v.parse::<u64>().ok());
        assert!(committed_peak.is_some_and(|b| b > 0), "{text}");
    }

    /// The option's index is counted by hand, so its default is what proves
    /// the count: 1000 ms is `purge_delay`'s and none of its neighbours'
    /// (`src/options.c`). mimalloc matches the variable's name in any case.
    #[test]
    fn the_purge_delay_read_back_is_purge_delay_s() {
        let want = std::env::vars()
            .find(|(k, _)| k.eq_ignore_ascii_case("MIMALLOC_PURGE_DELAY"))
            .map_or_else(|| "1000".to_owned(), |(_, v)| v);
        let text = report_text();
        assert!(text.contains(&format!("mimalloc_purge_delay={want}\n")), "{text}");
    }

    /// Every line that is not one of the report's own readings must be
    /// invisible to `measure.parse_reported`, whose grammar is
    /// `^[a-z_]+=\S+$`: the verbatim JSON and XML included.
    #[test]
    fn only_the_readings_parse_as_readings() {
        let text = report_text();
        let verbatim = text
            .split("# mi_stats_json\n")
            .nth(1)
            .and_then(|rest| rest.split("# end mi_stats_json\n").next())
            .expect("the report carries mimalloc's document");
        for line in verbatim.lines() {
            let trimmed = line.trim();
            let key = trimmed.split_once('=').map(|(k, _)| k);
            assert!(
                !key.is_some_and(
                    |k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                ),
                "a line of mimalloc's document parses as a reading: {line}"
            );
        }
    }

    /// The note explaining the two scopes must not itself parse as a reading:
    /// `measure.parse_reported` takes every `key=value` line in the file, so a
    /// note line carrying one would enter the report as a fact.
    #[test]
    fn the_prose_lines_are_invisible_to_the_key_value_parse() {
        for line in report_text().lines().filter(|l| l.starts_with('#')) {
            assert!(!line.contains('='), "a comment line parses as a reading: {line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::xml::{Totals, totals_of};

    /// A two-arena document, shaped as glibc prints one: each `<heap>` carries
    /// its own `system` pair and the document-level pair follows the last of
    /// them. The totals must be the trailing pair and not the last heap's —
    /// the two differ here on purpose.
    const XML: &str = concat!(
        "<malloc version=\"1\">\n",
        "<heap nr=\"0\">\n<sizes>\n</sizes>\n",
        "<total type=\"rest\" count=\"0\" size=\"1000\"/>\n",
        "<system type=\"current\" size=\"135168\"/>\n",
        "<system type=\"max\" size=\"135168\"/>\n</heap>\n",
        "<heap nr=\"1\">\n<sizes>\n</sizes>\n",
        "<system type=\"current\" size=\"25305088\"/>\n",
        "<system type=\"max\" size=\"25305088\"/>\n</heap>\n",
        "<total type=\"mmap\" count=\"0\" size=\"0\"/>\n",
        "<system type=\"current\" size=\"25440256\"/>\n",
        "<system type=\"max\" size=\"25440256\"/>\n",
        "</malloc>\n",
    );

    #[test]
    fn the_totals_are_the_trailing_pair_and_not_the_last_heaps() {
        assert_eq!(
            totals_of(XML),
            Totals { heaps: 2, system_current: 25_440_256, system_max: 25_440_256 }
        );
    }

    /// mimalloc's document, cut to the fields the read takes and one it must
    /// not: `total` is cumulative and differs from `peak` here on purpose.
    const JSON: &str = concat!(
        "{\n",
        "  \"stat_version\": 5,\n",
        "  \"mimalloc_version\": 316,\n",
        "  \"reserved\": { \"total\": 9000, \"peak\": 2048, \"current\": 1024 },\n",
        "  \"committed\": { \"total\": 7000, \"peak\": 512, \"current\": 256 },\n",
        "  \"malloc_bins\": [\n  ]\n",
        "}\n",
    );

    #[test]
    fn mimalloc_s_readings_are_current_and_peak_never_total() {
        let got = super::mimalloc_json::readings_of(JSON);
        let want: Vec<(String, i64)> = [
            ("mimalloc_version", 316),
            ("mimalloc_committed_bytes", 256),
            ("mimalloc_committed_peak_bytes", 512),
            ("mimalloc_reserved_bytes", 1024),
            ("mimalloc_reserved_peak_bytes", 2048),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn a_document_that_does_not_parse_yields_no_reading() {
        assert!(super::mimalloc_json::readings_of("{ truncated").is_empty());
    }

    #[test]
    fn a_document_with_no_system_elements_reads_zero_rather_than_panicking() {
        assert_eq!(
            totals_of("<malloc version=\"1\">\n</malloc>\n"),
            Totals { heaps: 0, system_current: 0, system_max: 0 }
        );
    }
}
