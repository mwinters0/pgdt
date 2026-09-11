//! What the process can say about its own memory, when it is built to say it.
//!
//! **Off by default and never in a shipped binary.** The whole module is
//! behind the `introspect` Cargo feature; without it every item here is a
//! no-op and the binary is byte-for-byte the one that ships. With it, `pgdq`
//! installs a counting `#[global_allocator]` over `std::alloc::System` and
//! prints, on the way out, what the program held and what glibc was holding
//! for it.
//!
//! # Why it exists
//!
//! A peak-RSS reading is one scalar with no decomposition, so the only way to
//! take it apart is to vary something and subtract — and every subtraction is
//! another sitting carrying both legs' spreads. That is the right instrument
//! for *does the shipped rule survive a real allocation* and the wrong one for
//! *what is the resident set made of*, which is the split
//! `docs/design/roadmap.md`, "Attribution is introspective; only the gate is
//! blind", now makes a standing rule. This module is the introspective half:
//! the process reports its own terms instead of being differenced.
//!
//! What each instrument sees, what it is blind to and what it costs is
//! `docs/design/measurements.md`, "What an instrument can see"; this
//! mechanism's own section, with what each line means and what it refused, is
//! `docs/design/architecture.md`, "What the binary can report about itself".
//!
//! # What it reports, and why none of it needs a sampler
//!
//! A snapshot at exit reports the end state rather than the peak, so the
//! instinct is to poll. Both quantities this project keeps asking for carry
//! their own high-water instead:
//!
//! * **`live_bytes` / `live_peak_bytes`** — exact bytes the *program* asked
//!   for and had not freed, and the largest that figure ever reached. Kept by
//!   the counting allocator itself, so the high-water costs nothing beyond the
//!   atomics already taken.
//! * **`mallinfo_*`** — glibc's own view at exit: `arena` (arena-backed bytes
//!   obtained from the OS), `hblkhd` (mmap-backed), `uordblks` (in use) and
//!   `fordblks` (freed, held, still resident). The gap between `uordblks` and
//!   `live_bytes` is allocator bookkeeping; the gap between `arena` and
//!   `uordblks` is retention, which is the term four slices of this phase
//!   carried with no owner.
//! * **`malloc_*`** — `malloc_info`'s document-level totals, plus the raw XML,
//!   which carries **each arena's own `system type="max"`**. That per-arena
//!   high-water is the one number `mallinfo2` cannot give and the one the
//!   dynamic-mmap-threshold hypothesis is stated against.
//!
//! All of it goes to **stderr**, beside the status lines — see [`report`].
//!
//! # What it is not
//!
//! **Not a fourth allocator leg.** `ALLOCATOR_LEGS` is the `allocator`
//! figure's published table, and this build takes an atomic on every
//! allocation. It is stated rather than bounded: **this build never times
//! anything**, and `pgdq --version` says so — `binary_allocator` in
//! `scripts/measure.py` refuses a binary whose `--version` names an
//! instrument, so an instrumented build cannot be timed as a figure by
//! accident.
//!
//! # The check this instrument owes
//!
//! An instrument nobody can falsify is the trap a figure nobody can re-take
//! already is. What must not happen is that the instrument moves the plan it
//! reports on, so `main.rs`'s
//! `the_instrument_build_resolves_what_the_default_build_resolves` pins the
//! resolved `jobs=`/`memory_bytes=` pair across every committed runtime root.
//! It lives beside the other resolution tests because that is what it is
//! asserting about, and it is compiled into **both** configurations — so
//! `cargo test -p pgdump_query-cli --features introspect` is the same
//! assertion re-run under the counting allocator, and the two runs are the two
//! halves of the comparison.

/// Prints the report when it goes out of scope.
///
/// Held in `main`, so the report is emitted on the ordinary return **and** on
/// an error propagated out of it, while tokio's blocking pool threads are
/// still alive — which is the point, since a per-thread arena that has been
/// torn down reports nothing. The one exit that skips destructors,
/// `std::process::exit` on the interrupt path, calls [`report`] itself.
pub struct AtExit(());

/// Arm the report. A no-op without the `introspect` feature, where [`report`]
/// has nothing to print.
pub fn at_exit() -> AtExit {
    AtExit(())
}

impl Drop for AtExit {
    fn drop(&mut self) {
        report();
    }
}

/// Print the `key=value` lines this build can answer. Nothing without the
/// feature.
///
/// **To stderr, with the status lines, and not to stdout.** stdout is the
/// answer — rows, listings, JSON — and a diagnostic written into it corrupts
/// a pipe, which is the same argument `init_status_output` makes for the
/// status stream (`docs/design/architecture.md`, "Status output"). The
/// alternative was tried and is what `chunk_size.rs` refuses: two runs that
/// must agree byte for byte disagree on the instrument's own numbers, so the
/// instrumented build stops answering what the shipped one answers.
/// `measure.parse_reported` reads both streams for exactly this.
pub fn report() {
    #[cfg(feature = "introspect")]
    {
        eprint!("{}", enabled::report_text());
    }
}

#[cfg(feature = "introspect")]
mod enabled {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Bytes the program has asked for and not yet freed.
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    /// The largest [`LIVE`] ever reached — the high-water that makes a
    /// sampler unnecessary.
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    /// [`System`] with a counter in front of it.
    ///
    /// **Over `System` specifically**, not over whichever allocator the build
    /// selected: the glibc statistics below describe the allocator underneath,
    /// and counting jemalloc's allocations while reading glibc's idle main
    /// arena would be two instruments pointed at different heaps. That is why
    /// `alloc.rs`'s guard refuses `introspect` beside `jemalloc` or
    /// `mimalloc` rather than ordering them.
    pub struct Counting;

    /// `Relaxed` throughout, and `PEAK` is a high-water rather than a
    /// snapshot: the two atomics are read only after every thread that
    /// touched them has stopped, so nothing here orders anything else, and a
    /// concurrent allocation that lands between the `fetch_add` and the
    /// `fetch_max` can only make the recorded peak smaller than the true one.
    /// It is a lower bound on the live high-water, stated as one.
    fn took(bytes: usize) {
        let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK.fetch_max(live, Ordering::Relaxed);
    }

    fn gave_back(bytes: usize) {
        LIVE.fetch_sub(bytes, Ordering::Relaxed);
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

    /// The whole report, as text, so the formatting is testable without a
    /// process to run.
    ///
    /// **Bracketed, because this stream has two writers.** The measurement
    /// harness wraps every timed command in its own reporter, which prints
    /// `maxrss_kib=<n>` to stderr — a `key=value` line by the same grammar as
    /// these. A harness that read the whole stream would fold the wrapper's
    /// own per-rep reading into the dict of facts a run states about itself,
    /// where every other entry is identical across reps. The markers are what
    /// let `measure.parse_instrument` take this block and nothing else; they
    /// carry no `=` and are therefore invisible to the `key=value` parse.
    pub fn report_text() -> String {
        let mut out = String::from(BEGIN);
        out.push('\n');
        out.push_str("instrument=counting-allocator\n");
        out.push_str(&format!("live_bytes={}\n", LIVE.load(Ordering::Relaxed)));
        out.push_str(&format!("live_peak_bytes={}\n", PEAK.load(Ordering::Relaxed)));
        push_glibc(&mut out);
        out.push_str(END);
        out.push('\n');
        out
    }

    /// The opening marker. `measure.parse_instrument` keys on this exact
    /// string, so it is a shared constant in fact if not in type — changing it
    /// means changing that function in the same commit.
    const BEGIN: &str = "# pgdq-introspect";
    const END: &str = "# end pgdq-introspect";

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
                // Verbatim, after the keys. Every line of it fails
                // `measure.parse_reported`'s `key=value` match and is ignored
                // there, which is what lets the per-arena detail ride along on
                // the same stream the harness reads.
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
        // glibc half simply does not exist off glibc, and saying so beats
        // printing zeros that read like an instrument that works.
        out.push_str("# mallinfo2/malloc_info: this build is not linked against glibc\n");
    }

    /// `malloc_info`'s XML, captured through `open_memstream`.
    ///
    /// The buffer is glibc's own — allocated inside `open_memstream`, not
    /// through the global allocator — so it is released with `libc::free` and
    /// never reaches [`Counting`]'s counters. Freeing it any other way would
    /// both corrupt the heap and make the instrument's own allocations show up
    /// in its reading.
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
/// **Compiled whenever the tests are, not only under the feature.** The glibc
/// call above cannot exist in a default build, but its *parse* is ordinary
/// string work with a failure mode that would go unnoticed — a number read off
/// the wrong element still looks like a plausible byte count — so it stays
/// under `cargo test --workspace`'s cold review rather than only under the
/// build that can call it.
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
    /// them — so "the last one" *is* "the total", without a parser that has to
    /// know the element nesting. The per-arena values are not summed here:
    /// they are in the XML the report prints verbatim, which is where a
    /// per-arena question is answered.
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

#[cfg(test)]
mod tests {
    use super::xml::{Totals, totals_of};

    /// A two-arena document, shaped as glibc prints one: each `<heap>` carries
    /// its own `system` pair and the document-level pair follows the last of
    /// them. What is pinned is that the totals are the trailing pair and not
    /// the last heap's — the two differ here on purpose.
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
