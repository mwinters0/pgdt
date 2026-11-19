//! **The block tracker**: which blocks of at least a stated size the Rust
//! heap holds resident, live or freed, at each moment a held line's draw is
//! first admitted or returned (`pgdt_core::instrument::LineMoment`), each named
//! by the call stack that allocated it.
//!
//! What it answers is what a run holds past its `--memory` as the ledger
//! lends a returned draw's bytes to the next line: a block the program freed
//! is mimalloc's until a purge releases its pages, so a freed block's
//! resident pages are retention, and the stack names whose copy it was
//! (`docs/design/decisions.md`, "D165").
//!
//! **Armed only where [`FROM_VAR`] states a size**, in bytes: unset, the
//! counting allocator tracks nothing, the report carries no `blocks_*` line,
//! and every other reading of the build is what it was. Armed, it reports
//! `blocks_*` lines and, as prose, every event and every site.
//!
//! * **A stack captured at the allocation, not a label set around a known
//!   buffer**: a label names only the candidates someone thought of, where a
//!   stack names whatever holds the pages. It is resolved only at the report,
//!   so the run pays an unwind per tracked block and no symbolization.
//! * **Residency is the page table's** (`mincore`), read over each tracked
//!   range at each event, never inferred from the purge delay.
//! * **A freed range a later tracked block takes is cut from it**; one a
//!   smaller, untracked block takes is still counted as freed, so the tracker
//!   errs towards finding retention. A run at `MIMALLOC_PURGE_DELAY=0` tells
//!   the two apart: a freed range is purged at once there, so what stays
//!   resident is reuse. A range found with no resident page, or no longer
//!   mapped, is dropped.
//! * **What it costs**: a stack per tracked block, kept to the report and
//!   counted in `live_*`, and one `mincore` per tracked range per event, on
//!   the reader's thread. The build is never timed (`docs/design/roadmap.md`,
//!   "Attribution is introspective; only the gate is blind"), but a slower run lets mimalloc purge more of what it frees.
//! * **Blind to** blocks under the size, what C allocates, the held line's
//!   own mapping (`docs/design/decisions.md`, "D139"), and a `realloc` that
//!   fails, or shrinks a block in place under the size, whose block it counts
//!   as freed.
//!
//! **A reading** builds with line tables (`release.py build --features
//! introspect --profile profiling`), arms it, and takes each leg at the
//! shipped delay and at `MIMALLOC_PURGE_DELAY=0`: a site's share of what the
//! delay keeps is its shipped sum less its zero one.
//! `runs/p33.31-blocks-20261117/` (`run.sh`, `table.py`, `summarize.py`) is the
//! pattern.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use pgdt_core::instrument::{LineEvent, LineMoment};

/// The environment variable stating the smallest block tracked, in bytes:
/// unset, or zero, tracks nothing.
pub const FROM_VAR: &str = "PGDT_INTROSPECT_BLOCKS_FROM";

/// How many frames of a stack name its site, past the allocator's own.
const SITE_FRAMES: usize = 12;

/// A frame whose symbol starts with one of these is the standard library's or
/// the allocator shim's, skipped before a site's frames are counted where the
/// build names no file.
const RUNTIME_FRAMES: &[&str] = &[
    "std::",
    "core::",
    "alloc::",
    "<std::",
    "<core::",
    "<alloc::",
    "__rust",
    "__rdl",
    "__rg_",
    "mi_",
    "_mi_",
    "mimalloc",
    "<mimalloc",
];

/// The smallest block tracked, or zero while unarmed.
static FROM: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Whether this thread is inside the tracker, whose own allocations it
    /// passes over: `const` and without a destructor, so the allocator reads
    /// it allocating nothing, on a thread being torn down too.
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

static TABLE: Mutex<Table> = Mutex::new(Table::new());

/// Everything tracked, behind one lock.
struct Table {
    started: Option<Instant>,
    /// One stack per tracked allocation, by its index.
    traces: Vec<Backtrace>,
    /// Each live tracked block, by its start address.
    live: BTreeMap<usize, Live>,
    /// Each range freed and not taken since by a tracked block.
    freed: Vec<Freed>,
    events: Vec<Event>,
}

struct Live {
    len: usize,
    trace: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Freed {
    start: usize,
    end: usize,
    trace: usize,
    /// When, in milliseconds since the tracker was armed.
    at: u64,
}

/// The heap at one [`LineEvent`].
struct Event {
    line: LineEvent,
    /// When, in milliseconds since the tracker was armed.
    at: u64,
    /// The process's resident set, or `None` where `/proc` did not answer.
    rss: Option<u64>,
    /// What the counting allocator held live.
    live: usize,
    /// Each tracked block or freed range with a resident page.
    blocks: Vec<Resident>,
}

struct Resident {
    trace: usize,
    /// When it was freed, or `None` where it is live.
    freed_at: Option<u64>,
    len: usize,
    resident: usize,
}

impl Table {
    const fn new() -> Self {
        Self {
            started: None,
            traces: Vec::new(),
            live: BTreeMap::new(),
            freed: Vec::new(),
            events: Vec::new(),
        }
    }

    fn now(&self) -> u64 {
        self.started.map_or(0, |started| started.elapsed().as_millis() as u64)
    }

    /// A tracked block now holds `start..end`: cut it from every freed range.
    fn cut_freed(&mut self, start: usize, end: usize) {
        if !self.freed.iter().any(|f| f.start < end && start < f.end) {
            return;
        }
        let mut kept = Vec::with_capacity(self.freed.len() + 1);
        for f in self.freed.drain(..) {
            if f.end <= start || end <= f.start {
                kept.push(f);
                continue;
            }
            if f.start < start {
                kept.push(Freed { end: start, ..f });
            }
            if end < f.end {
                kept.push(Freed { start: end, ..f });
            }
        }
        self.freed = kept;
    }
}

/// Run `f` as the tracker, or not at all where this thread already is: an
/// allocation the tracker makes reaches the allocator, which calls back here.
fn guarded(f: impl FnOnce()) {
    INSIDE.with(|inside| {
        if inside.replace(true) {
            return;
        }
        f();
        inside.set(false);
    });
}

/// Arm the tracker where [`FROM_VAR`] states a size, `observer` hearing each
/// line moment and handing it to [`observed`] with the counter's live bytes,
/// and say on stderr where it states something else: a tracker silently
/// unarmed reads as one that found nothing.
pub fn arm(observer: fn(LineEvent)) {
    let Some(stated) = std::env::var_os(FROM_VAR) else { return };
    match stated.to_str().and_then(|s| s.parse::<usize>().ok()) {
        Some(from) => arm_from(from, observer),
        None => eprintln!("pgdt: {FROM_VAR} is not a size in bytes: {}", stated.display()),
    }
}

/// Track every block of at least `from` bytes from now on, `observer`
/// hearing every line moment; zero tracks nothing.
fn arm_from(from: usize, observer: fn(LineEvent)) {
    if from == 0 {
        return;
    }
    guarded(|| {
        if let Ok(mut table) = TABLE.lock() {
            table.started.get_or_insert_with(Instant::now);
        }
    });
    FROM.store(from, Ordering::Relaxed);
    pgdt_core::instrument::observe_lines(observer);
}

/// The counting allocator handed out `len` bytes at `ptr`.
#[inline]
pub fn allocated(ptr: *mut u8, len: usize) {
    let from = FROM.load(Ordering::Relaxed);
    if from == 0 || len < from {
        return;
    }
    guarded(|| {
        // Outside the lock: the capture allocates, and takes the standard
        // library's own.
        let trace = Backtrace::force_capture();
        let Ok(mut table) = TABLE.lock() else { return };
        let start = ptr as usize;
        table.cut_freed(start, start + len);
        let id = table.traces.len();
        table.traces.push(trace);
        table.live.insert(start, Live { len, trace: id });
    });
}

/// The counting allocator is about to free `len` bytes at `ptr`: recorded
/// before the free, so no other thread can be handed the range first.
#[inline]
pub fn freed(ptr: *mut u8, len: usize) {
    let from = FROM.load(Ordering::Relaxed);
    if from == 0 || len < from {
        return;
    }
    guarded(|| {
        let Ok(mut table) = TABLE.lock() else { return };
        let start = ptr as usize;
        if let Some(live) = table.live.remove(&start) {
            let at = table.now();
            table.freed.push(Freed { start, end: start + live.len, trace: live.trace, at });
        }
    });
}

/// Read the heap at `line`, the counting allocator holding `live` bytes.
pub fn observed(line: LineEvent, live: usize) {
    guarded(|| {
        let rss = rss_bytes();
        let Ok(mut guard) = TABLE.lock() else { return };
        let table = &mut *guard;
        let at = table.now();
        let mut blocks = Vec::new();
        for (&start, block) in &table.live {
            if let Some(resident) = resident(start, start + block.len).filter(|&r| r > 0) {
                blocks.push(Resident {
                    trace: block.trace,
                    freed_at: None,
                    len: block.len,
                    resident,
                });
            }
        }
        table.freed.retain(|f| match resident(f.start, f.end) {
            Some(resident) if resident > 0 => {
                let len = f.end - f.start;
                blocks.push(Resident { trace: f.trace, freed_at: Some(f.at), len, resident });
                true
            }
            _ => false,
        });
        table.events.push(Event { line, at, rss, live, blocks });
    });
}

fn page_size() -> usize {
    // SAFETY: `sysconf` reads a constant of the running system.
    #[allow(unsafe_code)]
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(page).ok().filter(|&p| p > 0).unwrap_or(4096)
}

/// The resident bytes of the pages `start..end` touches, or `None` where any
/// of them is not mapped.
fn resident(start: usize, end: usize) -> Option<usize> {
    let page = page_size();
    let from = start & !(page - 1);
    let to = end.div_ceil(page) * page;
    let mut pages = vec![0u8; (to - from) / page];
    // SAFETY: `mincore` reads the page table for `from..to`, page-aligned,
    // and writes one byte a page into `pages`, which holds that many; an
    // unmapped page fails the call with `ENOMEM` rather than faulting.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::mincore(from as *mut libc::c_void, to - from, pages.as_mut_ptr()) };
    (rc == 0).then(|| pages.iter().filter(|&&p| p & 1 != 0).count() * page)
}

/// The process's resident set, from `/proc/self/statm`'s second field.
fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * page_size() as u64)
}

/// The tracker's section of the report: nothing while unarmed.
pub fn section() -> String {
    let from = FROM.load(Ordering::Relaxed);
    if from == 0 {
        return String::new();
    }
    let mut out = String::new();
    guarded(|| {
        if let Ok(table) = TABLE.lock() {
            out = render(&table, from);
        }
    });
    out
}

/// One site's frames, as one string: the stack's frames past the counting
/// allocator's and then the standard library's, [`SITE_FRAMES`] of them, each
/// on its own line as its symbol, without the hash its name ends in, and
/// where it is, where known.
fn site_of(trace: &Backtrace) -> String {
    let text = trace.to_string();
    let frames = frames_of(&text);
    let allocator = |f: &Frame<'_>| {
        f.at.is_some_and(|at| at.contains("src/introspect.rs:"))
            || f.symbol.contains("introspect::enabled::Counting")
    };
    let past = frames.iter().rposition(allocator).map_or(0, |i| i + 1);
    let site: Vec<String> = frames[past..]
        .iter()
        .skip_while(|f| is_runtime(f))
        .take(SITE_FRAMES)
        .map(|f| match f.at {
            Some(at) => format!("{} at {}", f.symbol, short_path(at)),
            None => f.symbol.to_owned(),
        })
        .collect();
    if site.is_empty() { "<no frame past the allocator's>".to_owned() } else { site.join("\n") }
}

/// One frame of a resolved stack: its symbol, and its file and line where
/// the build carries them.
struct Frame<'a> {
    symbol: &'a str,
    at: Option<&'a str>,
}

/// The frames of a `Backtrace`'s text: a numbered line a symbol, and an `at`
/// line after it where it is.
fn frames_of(text: &str) -> Vec<Frame<'_>> {
    let mut frames: Vec<Frame<'_>> = Vec::new();
    for line in text.lines().map(str::trim_start) {
        if let Some(at) = line.strip_prefix("at ") {
            if let Some(last) = frames.last_mut().filter(|f| f.at.is_none()) {
                last.at = Some(at.trim());
            }
        } else if let Some((index, symbol)) = line.split_once(": ")
            && !index.is_empty()
            && index.bytes().all(|b| b.is_ascii_digit())
        {
            frames.push(Frame { symbol: strip_hash(symbol.trim()), at: None });
        }
    }
    frames
}

/// Whether `frame` is the standard library's or the allocator shim's: its
/// file the toolchain's, or, where the build names no file, its symbol named
/// in [`RUNTIME_FRAMES`] or an impl of one of the standard library's traits,
/// `<u8 as alloc::vec::spec_from_elem::SpecFromElem>::from_elem` among them.
fn is_runtime(frame: &Frame<'_>) -> bool {
    let symbol = frame.symbol;
    match frame.at {
        Some(at) => at.contains("/rustc/") || at.contains("/library/"),
        None => {
            RUNTIME_FRAMES.iter().any(|p| symbol.starts_with(p))
                || symbol.starts_with('<')
                    && [" as alloc::", " as core::", " as std::"].iter().any(|t| symbol.contains(t))
        }
    }
}

/// `at` from the workspace's `crates/` or a registry crate's directory on.
fn short_path(at: &str) -> &str {
    if let Some(i) = at.find("/crates/") {
        return &at[i + 1..];
    }
    match at.find("/registry/src/") {
        Some(i) => {
            let rest = &at[i + "/registry/src/".len()..];
            rest.split_once('/').map_or(rest, |(_, crate_path)| crate_path)
        }
        None => at,
    }
}

/// `symbol` without a trailing `::h<16 hex digits>`.
fn strip_hash(symbol: &str) -> &str {
    match symbol.rsplit_once("::h") {
        Some((name, hash)) if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
            name
        }
        _ => symbol,
    }
}

/// The section's text: its `key=value` readings, then every event and every
/// site as prose, whose lines carry no `=`.
fn render(table: &Table, from: usize) -> String {
    // Each trace an event names, resolved once, and each distinct site
    // numbered in the order an event first names it.
    let mut sites: Vec<String> = Vec::new();
    let mut numbered: HashMap<String, usize> = HashMap::new();
    let mut site_of_trace: HashMap<usize, usize> = HashMap::new();
    for block in table.events.iter().flat_map(|e| &e.blocks) {
        if site_of_trace.contains_key(&block.trace) {
            continue;
        }
        let site = site_of(&table.traces[block.trace]);
        let number = match numbered.get(&site) {
            Some(&n) => n,
            None => {
                sites.push(site.clone());
                numbered.insert(site, sites.len() - 1);
                sites.len() - 1
            }
        };
        site_of_trace.insert(block.trace, number);
    }
    let freed_sum = |e: &Event| -> usize {
        e.blocks.iter().filter(|b| b.freed_at.is_some()).map(|b| b.resident).sum()
    };
    let rss_peak = table.events.iter().enumerate().filter_map(|(i, e)| Some((e.rss?, i))).max();

    let mut out = String::new();
    out.push_str("blocks_scope=rust-global-alloc\n");
    let _ = writeln!(out, "blocks_tracked_from_bytes={from}");
    let _ = writeln!(out, "blocks_tracked={}", table.traces.len());
    let _ = writeln!(out, "blocks_events={}", table.events.len());
    let freed_peak = table.events.iter().map(freed_sum).max().unwrap_or(0);
    let _ = writeln!(out, "blocks_freed_resident_peak_bytes={freed_peak}");
    match rss_peak {
        Some((rss, i)) => {
            let _ = writeln!(out, "blocks_rss_peak_bytes={rss}");
            let _ = writeln!(out, "blocks_rss_peak_event={i}");
            let freed = freed_sum(&table.events[i]);
            let _ = writeln!(out, "blocks_freed_resident_at_rss_peak_bytes={freed}");
        }
        None => out.push_str("blocks_rss_peak_bytes=none\n"),
    }
    out.push_str(BLOCKS_NOTE);
    for (i, event) in table.events.iter().enumerate() {
        push_event(&mut out, i, event, &site_of_trace);
    }
    for (number, site) in sites.iter().enumerate() {
        let _ = writeln!(out, "# site {number}:");
        for frame in site.lines() {
            let _ = writeln!(out, "#     {}", frame.replace('=', "_"));
        }
    }
    out.push_str("# end blocks\n");
    out
}

/// One event's lines: the moment and the heap, then each site's live and
/// each site's freed blocks summed, the youngest free's age beside them.
fn push_event(out: &mut String, i: usize, event: &Event, site_of_trace: &HashMap<usize, usize>) {
    let moment = match event.line.moment {
        LineMoment::Admitted => "admitted",
        LineMoment::Returned => "returned",
    };
    let ledger = event.line.drawn.map_or("no account".to_owned(), |d| format!("{d} drawn"));
    let rss = event.rss.map_or("unread".to_owned(), |r| r.to_string());
    let sum = |freed: bool| -> usize {
        event.blocks.iter().filter(|b| b.freed_at.is_some() == freed).map(|b| b.resident).sum()
    };
    let _ = writeln!(
        out,
        "# event {i}, {} ms: line at {} {moment} holding {}, ledger {ledger}; rss {rss}, live {}, \
         tracked live resident {}, freed resident {}",
        event.at,
        event.line.start,
        event.line.charge,
        event.live,
        sum(false),
        sum(true),
    );
    let mut by_site: BTreeMap<(usize, bool), Summed> = BTreeMap::new();
    for block in &event.blocks {
        let site = site_of_trace.get(&block.trace).copied().unwrap_or(usize::MAX);
        let summed = by_site.entry((site, block.freed_at.is_some())).or_default();
        summed.blocks += 1;
        summed.bytes += block.len;
        summed.resident += block.resident;
        if let Some(freed_at) = block.freed_at {
            let age = event.at.saturating_sub(freed_at);
            summed.youngest = Some(summed.youngest.map_or(age, |a| a.min(age)));
        }
    }
    for ((site, _), Summed { blocks, bytes, resident, youngest }) in by_site {
        let state =
            youngest.map_or("live".to_owned(), |a| format!("freed, youngest {a} ms before"));
        let _ = writeln!(
            out,
            "#   site {site}, {state}: {blocks} blocks of {bytes} bytes, {resident} resident"
        );
    }
}

/// One site's live, or freed, blocks at one event, summed.
#[derive(Default)]
struct Summed {
    blocks: usize,
    bytes: usize,
    resident: usize,
    /// The youngest free's age, in milliseconds, where they are freed.
    youngest: Option<u64>,
}

/// What the section's readings mean, in the report itself.
const BLOCKS_NOTE: &str = concat!(
    "# `blocks_*` read each Rust heap block of at least the tracked size,\n",
    "# live or freed, whose pages `mincore` finds resident, at each moment a\n",
    "# held line's draw is first admitted or returned. A freed block's\n",
    "# resident pages are mimalloc's until a purge, or a smaller block's\n",
    "# that took them, which a run at a zero purge delay tells apart. Each\n",
    "# site is the stack that allocated it, past the allocator's frames.\n",
);

#[cfg(test)]
mod tests {
    use super::*;

    fn freed(start: usize, end: usize) -> Freed {
        Freed { start, end, trace: 0, at: 0 }
    }

    #[test]
    fn a_tracked_block_cuts_each_freed_range_it_takes() {
        let mut table = Table::new();
        table.freed = vec![freed(0, 100), freed(200, 300), freed(400, 500)];
        table.cut_freed(50, 250);
        assert_eq!(table.freed, [freed(0, 50), freed(250, 300), freed(400, 500)]);
        table.cut_freed(420, 450);
        assert_eq!(table.freed, [freed(0, 50), freed(250, 300), freed(400, 420), freed(450, 500)]);
        table.cut_freed(0, 1000);
        assert!(table.freed.is_empty());
    }

    #[test]
    fn a_symbol_s_hash_is_stripped_and_nothing_else() {
        assert_eq!(
            strip_hash("pgdt_query::batch::push::h0123456789abcdef"),
            "pgdt_query::batch::push"
        );
        assert_eq!(strip_hash("pgdt_query::batch::hash"), "pgdt_query::batch::hash");
        assert_eq!(strip_hash("a::h0123"), "a::h0123");
    }

    /// One process-wide tracker, so one test walks it: armed, a block past
    /// the size is live and named by this test's stack at one moment, and
    /// freed and resident at the next, mimalloc told never to purge.
    #[test]
    fn a_block_is_read_live_then_freed_and_resident_at_line_moments() {
        // SAFETY: an option's setter takes any value; -1 never purges.
        // upstream: UF6
        #[allow(unsafe_code)]
        unsafe {
            libmimalloc_sys::mi_option_set(crate::alloc::MI_OPTION_PURGE_DELAY, -1)
        };
        const LEN: usize = 48 << 20;
        arm_from(32 << 20, |line| observed(line, 0));
        let block = std::hint::black_box(vec![1u8; LEN]);
        let start = block.as_ptr() as usize;
        let event = LineEvent { moment: LineMoment::Admitted, start: 1, charge: 2, drawn: Some(2) };
        observed(event, 0);
        drop(block);
        observed(LineEvent { moment: LineMoment::Returned, ..event }, 0);

        let table = TABLE.lock().unwrap();
        let [.., held, freed] = &table.events[..] else { panic!("two events were read") };
        fn named<'a>(table: &Table, event: &'a Event, freed: bool) -> Option<&'a Resident> {
            event.blocks.iter().find(|b| {
                b.freed_at.is_some() == freed
                    && b.len == LEN
                    && site_of(&table.traces[b.trace])
                        .contains("a_block_is_read_live_then_freed_and_resident_at_line_moments")
            })
        }
        let seen = |event: &Event| -> Vec<(bool, usize, usize, String)> {
            let site = |b: &Resident| site_of(&table.traces[b.trace]);
            event
                .blocks
                .iter()
                .map(|b| (b.freed_at.is_some(), b.len, b.resident, site(b)))
                .collect()
        };
        let live = named(&table, held, false).unwrap_or_else(|| {
            panic!("the block is live and named at the first: {:?}", seen(held))
        });
        assert!(live.resident >= LEN, "{} of {LEN}", live.resident);
        let gone = named(&table, freed, true).unwrap_or_else(|| {
            panic!("the block is freed and resident at the second: {:?}", seen(freed))
        });
        assert!(gone.resident >= LEN - page_size(), "{} of {LEN}", gone.resident);
        assert!(!table.live.contains_key(&start));
        drop(table);

        let text = section();
        for line in text.lines() {
            if line.starts_with('#') {
                assert!(!line.contains('='), "a prose line parses as a reading: {line}");
            } else {
                let (key, value) = line.split_once('=').expect("a reading");
                assert!(key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{line}");
                assert!(!value.is_empty() && !value.contains(char::is_whitespace), "{line}");
            }
        }
        assert!(text.contains("blocks_tracked_from_bytes=33554432\n"), "{text}");
    }
}
