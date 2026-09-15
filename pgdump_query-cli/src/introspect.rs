//! What the process can say about its own memory, when it is built to say it.
//!
//! **Off by default and never in a shipped binary.** The whole module is
//! behind the `introspect` Cargo feature; without it every item here is a
//! no-op and the binary is byte-for-byte the one that ships. With it, `pgdq`
//! installs a counting `#[global_allocator]` over `std::alloc::System` and
//! writes, on the way out, what the program held and what glibc was holding
//! for it.
//!
//! This is the introspective half of `docs/design/roadmap.md`, "Attribution is
//! introspective; only the gate is blind". What each instrument sees, what it
//! is blind to and what it costs is `docs/design/measurements.md`, "What an
//! instrument can see"; the mechanism's entry is
//! `docs/design/decisions.md`, "D13".
//!
//! # What it reports
//!
//! A snapshot at exit, and no sampler, because each quantity carries its own
//! high-water:
//!
//! * **`live_bytes` / `live_peak_bytes`** — exact bytes the *program* asked
//!   for and had not freed, and the largest that figure ever reached, kept by
//!   the counting allocator itself.
//! * **`mallinfo_*`** — glibc's own view at exit: `arena` (arena-backed bytes
//!   obtained from the OS), `hblkhd` (mmap-backed), `uordblks` (in use) and
//!   `fordblks` (freed, held, still resident). The gap between `uordblks` and
//!   `live_bytes` is allocator bookkeeping; the gap between `arena` and
//!   `uordblks` is retention.
//! * **`malloc_*`** — `malloc_info`'s document-level totals, plus the raw XML,
//!   which carries **each arena's own `system type="max"`** — the one number
//!   `mallinfo2` cannot give.
//! * **`statistics_*`** — where a `parse` ran: the library's statistics
//!   account as the pass returned it, beside the live bytes the counter
//!   attributed to statistics (`pgdump_query::instrument`) at the same moment,
//!   their peaks, and the worst shortfall any update of the account read
//!   (`docs/design/decisions.md`, "D81").
//!
//! **The two families do not cover the same memory**, so the report labels
//! each: `live_scope` and `glibc_scope`, with the note between them. The
//! counter sees what passes through Rust's `GlobalAlloc`; glibc sees the whole
//! process, C included — `liblzma` is the active `.xz` backend, so a reader's
//! `XZ_DECODE_FOOTPRINT` of decoder working set is invisible to one and fully
//! present in the other. Their difference is therefore not retention.
//!
//! All of it goes to **the file [`OUT_VAR`] names**, and nowhere at all when
//! that variable is unset — see [`report`].
//!
//! **Not a fourth allocator leg**, and **this build never times anything**:
//! `pgdq --version` names the instrument and `binary_allocator` in
//! `scripts/measure.py` refuses such a binary.
//!
//! The instrument must not move the plan it reports on, so `main.rs`'s
//! `the_instrument_build_resolves_what_the_default_build_resolves` pins the
//! resolved `jobs=`/`memory_bytes=` pair across every committed runtime root.
//! It is compiled into **both** configurations, so
//! `cargo test -p pgdump_query-cli --features introspect` re-runs that
//! assertion under the counting allocator.

/// The environment variable naming the file [`report`] writes to.
///
/// **Unset means no report at all**, which is the only state a default build
/// can be in. Shared in fact rather than in type with
/// `measure.INSTRUMENT_OUT_VAR`; `scripts/test_measure.py` holds the two to
/// each other.
///
/// Compiled into both configurations; only the feature build reads it.
#[cfg_attr(not(feature = "introspect"), allow(dead_code))]
pub const OUT_VAR: &str = "PGDQ_INTROSPECT_OUT";

/// Writes the report when it goes out of scope.
///
/// Held in `main`, so the report is emitted on the ordinary return **and** on
/// an error propagated out of it, while tokio's blocking pool threads are
/// still alive — a per-thread arena already torn down reports nothing. The one
/// exit that skips destructors, `std::process::exit` on the interrupt path,
/// calls [`report`] itself.
pub struct AtExit(());

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
pub fn at_exit() -> AtExit {
    AtExit(())
}

impl Drop for AtExit {
    fn drop(&mut self) {
        report();
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
pub fn report() {
    #[cfg(feature = "introspect")]
    {
        enabled::write_report();
    }
}

#[cfg(feature = "introspect")]
mod enabled {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Bytes the program has asked for and not yet freed.
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    /// The largest [`LIVE`] ever reached.
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    /// [`System`] with a counter in front of it.
    ///
    /// **Over `System` specifically**: the glibc statistics below describe the
    /// allocator underneath, so counting another allocator's allocations would
    /// point two instruments at different heaps. Hence `alloc.rs`'s guard.
    pub struct Counting;

    /// `Relaxed` throughout: the atomics are read only after every thread
    /// that touched them has stopped, so nothing here orders anything else. A
    /// concurrent allocation landing between the `fetch_add` and the
    /// `fetch_max` can only understate the peak, so `PEAK` is a lower bound.
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

    /// The `statistics_*` lines, or none where no pass returned an account.
    fn push_statistics(out: &mut String) {
        let Some((held, reading)) = RETURNED.lock().ok().and_then(|r| *r) else { return };
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
        for (term, now, peak) in terms {
            out.push_str(&format!("statistics_account_{term}_bytes={now}\n"));
            out.push_str(&format!("statistics_account_{term}_peak_bytes={peak}\n"));
        }
        out.push_str(&format!("statistics_live_bytes={}\n", reading.live));
        out.push_str(&format!("statistics_live_peak_bytes={}\n", reading.live_peak));
        out.push_str(&format!("statistics_checks={}\n", reading.checks));
        out.push_str(&format!("statistics_worst_shortfall_bytes={}\n", reading.shortfall));
        out.push_str(&format!(
            "statistics_worst_shortfall_live_bytes={}\n",
            reading.shortfall_live
        ));
        out.push_str(&format!(
            "statistics_slack_per_mille={}\n",
            pgdump_query::instrument::STATISTICS_SLACK_PER_MILLE
        ));
        out.push_str(&format!(
            "statistics_worst_shortfall_past_slack_bytes={}\n",
            reading.shortfall_past_slack
        ));
    }

    // SAFETY: every method forwards to `System`, which satisfies the trait's
    // contract, and the counters are plain atomics that allocate nothing.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                took(layout.size());
            }
            ptr
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            if !ptr.is_null() {
                took(layout.size());
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            gave_back(layout.size());
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let out = unsafe { System.realloc(ptr, layout, new_size) };
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

    /// Write [`report_text`] to the file [`super::OUT_VAR`] names, or do
    /// nothing at all where the variable is unset.
    ///
    /// **The whole file is rewritten, and the last writer wins.** One process
    /// writes one report, at its own exit, so there is nothing to append to,
    /// and a run re-using a previous run's path cannot be read as that run's.
    pub fn write_report() {
        let Some(path) = std::env::var_os(super::OUT_VAR) else { return };
        let path = std::path::PathBuf::from(path);
        if let Err(err) = std::fs::write(&path, report_text()) {
            // Not the report — an error saying there is none.
            eprintln!(
                "pgdq: the introspection report could not be written to {}: {err}",
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
        out.push_str("glibc_scope=whole-process\n");
        push_glibc(&mut out);
        out
    }

    /// What the two scopes mean, in the report itself rather than only in the
    /// document that explains it. The number is `XZ_DECODE_FOOTPRINT`
    /// (`docs/design/decisions.md`, "D16").
    const SCOPE_NOTE: &str = concat!(
        "# `live_*` counts only what passed through Rust's `GlobalAlloc`.\n",
        "# `mallinfo_*` and `malloc_*` are glibc's view of the whole process, C\n",
        "# included: `liblzma` is the active `.xz` backend and allocates ~9.47 MB\n",
        "# a reader the counter cannot see. The two are not commensurable, and\n",
        "# their difference is not retention.\n",
    );

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

/// The report's own shape, which only the instrument build can produce.
///
/// Compiled under the feature alone — `report_text` does not exist without it
/// — so these run in `cargo test -p pgdump_query-cli --features introspect`.
#[cfg(all(test, feature = "introspect"))]
mod instrumented_tests {
    use super::enabled::report_text;

    /// The scope labels are the report's, not the reader's: a consumer
    /// differencing `live_peak_bytes` against `malloc_system_max` would
    /// subtract a Rust-only count from a whole-process one.
    #[test]
    fn every_quantity_states_which_memory_it_covers() {
        let text = report_text();
        assert!(text.contains("live_scope=rust-global-alloc\n"), "{text}");
        assert!(text.contains("glibc_scope=whole-process\n"), "{text}");
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

    #[test]
    fn a_document_with_no_system_elements_reads_zero_rather_than_panicking() {
        assert_eq!(
            totals_of("<malloc version=\"1\">\n</malloc>\n"),
            Totals { heaps: 0, system_current: 0, system_max: 0 }
        );
    }
}
