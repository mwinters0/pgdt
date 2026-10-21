//! The memory allowance: the default budget, the reserve, the margin, the
//! statistics allowance, and the limit discovered from the cgroup hierarchy
//! and `/proc/meminfo`.

use std::path::{Path, PathBuf};

/// What a source may hold in pooled buffers when the caller has stated no
/// budget of its own — [`Parallelism::default`](crate::Parallelism::default)'s number, and what a `pgdt` run
/// falls back to where neither `--memory` nor a limit says otherwise.
///
/// **It is the serial path's budget and deliberately not the largest block
/// anyone might write**: [`POOL_DEPTH`](crate::source::pool::POOL_DEPTH) slots at the largest chunk size the
/// `chunk-size` figure measured, so no chunk size in that range loses a slot
/// to it. It is also the line a compressed source's whole-block decode is
/// refused above ([`BlockCache::affordable`](crate::source::xz::blocks::BlockCache::affordable)). See
/// `docs/design/decisions.md`, "D3".
pub const DEFAULT_MEMORY_BUDGET: u64 = 64 << 20;

/// What [`Arrangement::resolve`](crate::Arrangement::resolve) holds back from a memory allowance,
/// discovered or stated, in bytes: everything the process holds that the pools' budget
/// does not bound — the runtime's threads, the allocators' per-thread state, the
/// decoder state a compressed source keeps outside its pools, and the binary
/// itself.
///
/// **It is a subtraction rather than a fraction**
/// (`docs/design/decisions.md`, "D3").
///
/// **What it must satisfy: the worst observed rep leaves
/// [`MEMORY_MARGIN_PERCENT`] of the limit**, a cgroup's killer reading one
/// run's peak. This is the smallest candidate measured against that criterion
/// (`reserve`); what it covers is the remainder above
/// [`BlockCache::reader_bytes`](crate::source::xz::blocks::BlockCache::reader_bytes) and not a per-reader term, the block pool's
/// retention list being billed by [`WorkerMemory`](crate::WorkerMemory).
///
/// **It is the cap, and it is not the bound on that excess** — that is
/// [`MEMORY_UNPOOLED_BOUND`], which [`margin_allowance`] predicts with. This
/// one comes off the top of the allowance and decides what
/// [`BlockCache::affordable`](crate::source::xz::blocks::BlockCache::affordable) sees, so **raising it is also what declines the
/// block path**. One constant, taken from the compressed leg, a source's own
/// answer being downstream of recognition and so of I/O.
///
/// *Rejected: sizing it from glibc's arena count.* There is no getter for
/// `M_ARENA_MAX` and this crate does not set a cap
/// (`docs/design/decisions.md`, "D13").
///
/// **Below it the budget goes to zero rather than to a floor**, which the
/// floors already in the mechanism turn into one reader's worth on the
/// streaming path.
///
/// deficiency: KD34 — this value was fitted before statistics existed, and
/// what a run holds above its charge and its statistics account has since been
/// attributed at a worst 544 MiB on a compressed `query` — well past the
/// 384 MiB held back here, with every wide-text compressed `query` leg from a
/// 1 GiB limit up killed in every rep. That attribution was taken on the
/// `introspect` build, so it sizes the gap without standing in for the blind
/// gate a constant has to pass; P23 owns both. The `reserve` figure's stated
/// axis is a second reading: under a typed `--jobs` and `--memory` the margin
/// lowers only the budget, and the worst rep there holds less than this value
/// above the resolved budget on the shipped build, though more than the margin
/// leaves (`measurements.md`, `reserve`). **The choice of this value and the
/// attribution are the platform allocator's**
/// (`runs/19.16-reserve-constant-20260911-2210/readings.json`,
/// `runs/20.8-reserve-attribution-20260916-1857/`), and the shipped build's
/// Rust heap is mimalloc's, C's alone glibc's (`docs/design/decisions.md`,
/// "D13"), so neither reads what the shipped build holds. The shipped build's
/// flagless legs hold inside [`MEMORY_UNPOOLED_BOUND`], this value and
/// [`MEMORY_MARGIN_PERCENT`] at every leg, re-taken at `5629ecfa` in a sitting
/// of their own (`runs/measure-20261021T151351/`); the published figure, taken
/// at `3a34f062`, has the 128 MiB-block legs from four readers overrunning the
/// bound (`measurements.md`, `reserve`). P23 re-takes both readings under
/// mimalloc on the two-heap instrument and sets both constants from them.
pub const MEMORY_RESERVE: u64 = 384 << 20;

/// How much of a memory allowance a resolved arrangement must leave unused, as
/// a percentage of that allowance — the criterion [`MEMORY_RESERVE`] was chosen
/// against, enforced on the worker count instead of being left to hold by
/// accident.
///
/// **A judgement rather than a derived number**, about the worst rep and not
/// the median (`docs/design/decisions.md`, "D3"). A constant reserve leaves a
/// shrinking *share* of the allowance as the allowance grows, so the cap alone
/// stops meeting the criterion at the top end; enforcing it on the count is
/// what makes the answer a property of the allocation rather than of the
/// host's width.
///
/// **Enforced once, against [`MEMORY_UNPOOLED_BOUND`]**: the predicted
/// resident a count is held to is `WorkerMemory::at(n)` plus that bound, not
/// plus [`MEMORY_RESERVE`], which would subtract the margin a second time. So
/// it binds above `5 × (MEMORY_RESERVE − MEMORY_UNPOOLED_BOUND)` and, but for
/// the few hundred bytes under it its rounding down reaches, nowhere below.
///
/// **It bounds the count, and the budget only by what a caller holds besides
/// its arrangements**: [`Parallelism::fit`](crate::Parallelism::fit) keeps one worker at whatever the
/// cap is and reports the budget that count spends, up to the cap, and
/// [`BlockCache::affordable`](crate::source::xz::blocks::BlockCache::affordable) reads the budget the cap leaves, lowered only by
/// the holdings [`Parallelism::within_shared`](crate::Parallelism::within_shared) cannot fit under the margin.
///
/// **What a gathering pass holds is inside it too**: the statistics account is
/// billed against this same margin, [`statistics_allowance`] handing a mapping
/// pass what the resolved arrangement leaves under it and a block that does not
/// fit declining (`docs/design/decisions.md`, "D85").
pub const MEMORY_MARGIN_PERCENT: u64 = 20;

/// What a scan holds resident **outside the pools the budget bills**, bounded:
/// the runtime's threads, the allocators' per-thread retention, the decoder
/// state a compressed source keeps beyond [`BlockCache::reader_bytes`](crate::source::xz::blocks::BlockCache::reader_bytes), and
/// the binary itself. [`margin_allowance`] predicts with it, and nothing else
/// reads it.
///
/// **A bound, not a term, and it is read off a grid rather than fitted**: the
/// smallest step covering the unnamed remainder `held − at(jobs)` over the
/// grid behind `reserve` (`docs/design/decisions.md`, "D3").
///
/// *Rejected: the next step down*, which halves the margin's crossover and
/// starts taking readers across the band the margin exists to leave alone.
/// *Rejected: a bound per block size*, which the margin cannot state before
/// the file is open.
///
/// **What it is, is attributed only in bulk**: on the platform allocator,
/// glibc's retained free memory at exit (`fordblks`) is most of it, and no term
/// table sums to it.
/// `scripts/measure.py`'s `charge_model_problem` faults a cell whose remainder
/// exceeds this, which is the finding that would move it, and on the shipped
/// build it has faulted (`KD34`, whose owner moves it). It is not
/// [`MEMORY_RESERVE`] because the reserve is what an allowance hands back
/// before anything is spent, where this is what the arrangement is predicted
/// to hold on top of what it spends.
pub const MEMORY_UNPOOLED_BOUND: u64 = 256 << 20;

/// The most a resolved arrangement may be **charged** under an `allowance` —
/// a discovered limit or a stated `--memory` — if its predicted resident is to
/// leave [`MEMORY_MARGIN_PERCENT`] of it unused: `(100 − margin)% of
/// allowance`, less [`MEMORY_UNPOOLED_BOUND`].
///
/// **The predicted resident is `charge + MEMORY_UNPOOLED_BOUND`**, that
/// constant being what bounds the one term the charge does not bill. Using
/// [`MEMORY_RESERVE`] here instead would apply [`MEMORY_MARGIN_PERCENT`] twice,
/// the reserve being the smallest constant meeting that criterion under the
/// cap rule (`docs/design/decisions.md`, "D3").
///
/// Integer arithmetic, rounding **down** the fraction of the allowance so the
/// ceiling errs small.
pub(super) fn margin_allowance(allowance: u64) -> u64 {
    (allowance / 100)
        .saturating_mul(100 - MEMORY_MARGIN_PERCENT)
        .saturating_sub(MEMORY_UNPOOLED_BOUND)
}

/// **What a resolved arrangement leaves for a mapping pass's statistics**
/// ([`Arrangement::statistics_allowance`](crate::Arrangement::statistics_allowance)): the
/// bytes `pgdt_mapping`'s `StatisticsAccount` may hold under `allowance` —
/// a discovered limit or a stated `--memory` — once `budget`, the read-buffer
/// budget that arrangement resolved to, comes off [`margin_allowance`]'s
/// ceiling, which already stands [`MEMORY_UNPOOLED_BOUND`] below its fraction
/// of the allowance (`docs/design/decisions.md`, "D85").
///
/// **It is carved after the workers, never before them**
/// (`docs/design/decisions.md`, "D85"): the count is fixed before a byte is
/// read, while statistics are gathered only as they accumulate. Those a cache
/// hands the pass are known first and bill the workers' carving as `held`
/// ([`Parallelism::within_shared`](crate::Parallelism::within_shared)), so what is left here covers them too.
///
/// **[`MEMORY_RESERVE`] is not subtracted again here.** The reserve comes off
/// the top for the *cap* a count is solved against, where this is the *margin*
/// ceiling, which already stands [`MEMORY_UNPOOLED_BOUND`] below its fraction
/// of the allowance — subtracting both would take the margin twice, exactly as
/// [`margin_allowance`] refuses to.
///
/// **Which of the two bounds binds decides what is left, and the bands run the
/// other way round from the intuition.** [`Parallelism::fit`](crate::Parallelism::fit) solves the count
/// against `cap.min(ceiling)`, so below `5 × (MEMORY_RESERVE −
/// MEMORY_UNPOOLED_BOUND)` the *cap* binds and, above the zero line below,
/// what is left here is at least `ceiling − cap`, positive but in the few hundred bytes under the line the
/// ceiling's rounding reaches — an allowance in that band cannot starve
/// statistics. At and above it the *ceiling* binds, the count is solved right up
/// against the number this subtracts from, and statistics get only the slack one
/// worker's step leaves: a wide host at a high `--jobs` is where every block
/// declines, not a narrow one. At or below `MEMORY_UNPOOLED_BOUND × 100 /
/// (100 − MEMORY_MARGIN_PERCENT)` the ceiling is zero on its own and nothing is
/// ever gathered. Both lines are arithmetic between the constants and both are
/// pinned by this module's tests.
///
/// Saturating, so an arrangement whose budget already fills the margin leaves
/// zero rather than wrapping, and every block declines.
pub(crate) fn statistics_allowance(allowance: u64, budget: u64) -> u64 {
    margin_allowance(allowance).saturating_sub(budget)
}

/// A cgroup v1 `memory.limit_in_bytes` at or above this reads as *no limit*
/// (`docs/design/runtime-invariants.md`, `RT4`).
///
/// **A threshold, never an equality test.** The unset value is
/// `PAGE_COUNTER_MAX × PAGE_SIZE`, a function of the page size and the word
/// width, so it differs between 4 KiB, 16 KiB and 64 KiB pages and on a
/// 32-bit kernel; 4 TiB is under all of them while being more memory than a
/// cgroup is given.
///
/// It is applied to the v2 files too, where it is very nearly unreachable:
/// `memory.max` spells "no limit" as the string `max` (`RT2`), so a *stated*
/// v2 limit above 4 TiB is the only thing this could misread, and reading one
/// as unlimited falls back to the [`available_memory`] cap.
const NO_LIMIT_AT_OR_ABOVE: u64 = 1 << 42;

/// Which cgroup hierarchy states this process's memory limit, and where in it
/// the process sits (`docs/design/runtime-invariants.md`, `RT6`).
enum MemoryHierarchy<'a> {
    /// A v1 hierarchy carrying the `memory` controller, at this path within
    /// that hierarchy's own mount.
    V1(&'a str),
    /// The unified hierarchy, at this path under the v2 mount.
    V2(&'a str),
    /// `/proc/self/cgroup` named neither — no cgroup governs this process, or
    /// the file is not a Linux one.
    None,
}

/// Which of the two file shapes to read, from the body of
/// `/proc/self/cgroup`.
///
/// **A controller lives in exactly one hierarchy** (`RT6`), so a line whose
/// controller list contains `memory` settles it outright and the `0::` line is
/// only consulted when no such line exists. Three parse hazards from `RT1` are
/// handled here and nowhere else: the v2 line is selected **by its shape** —
/// an empty controller field, which is what `std` matches for the CPU quota —
/// and never by position or by hierarchy id, which `idr_alloc_cyclic` hands
/// out cyclically; the controller field is matched by **membership** in a
/// comma-separated list, since a v1 hierarchy carrying only a name reads
/// `name=memory` and holds no memory controller at all; and a dead cgroup's
/// path carries a ` (deleted)` suffix that would otherwise be joined onto the
/// mount point as part of a directory name.
fn memory_hierarchy(cgroups: &str) -> MemoryHierarchy<'_> {
    let mut unified = None;
    for line in cgroups.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(_id), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = path.strip_suffix(" (deleted)").unwrap_or(path);
        if controllers.is_empty() {
            unified.get_or_insert(path);
        } else if controllers.split(',').any(|c| c == "memory") {
            return MemoryHierarchy::V1(path);
        }
    }
    unified.map_or(MemoryHierarchy::None, MemoryHierarchy::V2)
}

/// Where the v1 hierarchy carrying the `memory` controller is mounted, read
/// from `/proc/self/mountinfo` — the authority when the conventional
/// `/sys/fs/cgroup/memory` is not where it is (`RT1`).
///
/// A line's mount point is its fifth space-separated field; the filesystem
/// type and the super options follow the ` - ` separator, and the options are
/// where a v1 cgroup mount names its controllers. Falls back to the
/// convention when the file cannot be read or lists no such mount, which is
/// also what a fixture tree that states only the cgroup path gets.
///
/// **Mountinfo's `\040` escaping of spaces is not undone**, which is the same
/// limit `std` states for its own reading of this file: a cgroup mounted under
/// a path containing a space is not found, and the fallback applies.
fn v1_memory_mount(root: &Path) -> PathBuf {
    let conventional = root.join("sys/fs/cgroup/memory");
    let Ok(text) = std::fs::read_to_string(root.join("proc/self/mountinfo")) else {
        return conventional;
    };
    for line in text.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let mut tail = after.split_whitespace();
        if tail.next() != Some("cgroup") {
            continue;
        }
        let _source = tail.next();
        if !tail.next().unwrap_or("").split(',').any(|o| o == "memory") {
            continue;
        }
        if let Some(point) = before.split_whitespace().nth(4) {
            return root.join(point.trim_start_matches('/'));
        }
    }
    conventional
}

/// One limit file's value in bytes, or `None` where it states no limit — the
/// literal `max` of a v2 file (`RT2`, `RT3`), a v1 value at
/// [`NO_LIMIT_AT_OR_ABOVE`] (`RT4`), or a file that is not there at all.
///
/// **Absent is "no limit here", not an error** (`RT5`): the hierarchy root
/// carries no `memory.max` at all, and a cgroup whose parent has not enabled
/// the controller in `cgroup.subtree_control` carries none either.
fn read_limit_file(path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = text.trim().parse::<u64>().ok()?;
    (value < NO_LIMIT_AT_OR_ABOVE).then_some(value)
}

/// The smallest limit any of `files` states at `cgroup_path` or at any
/// ancestor of it up to `mount` (`docs/design/runtime-invariants.md`, `RT5`).
///
/// **Every ancestor, and the two v2 files minimised together.** A limit set on
/// an ancestor binds a process in a descendant and is invisible in the
/// descendant's own file, and nothing orders `memory.high` against
/// `memory.max`, across levels or within one. `memory.high` is read at all
/// because it throttles where `memory.max` kills (`RT3`).
///
/// **The walk is bounded by the mount point, not by counting separators**:
/// inside a cgroup namespace the namespace root is what is mounted, so walking
/// up from the `0::` path never leaves it. Limits set outside the namespace
/// still bind and are simply not readable.
fn smallest_limit(mount: &Path, cgroup_path: &str, files: &[&str]) -> Option<MemoryLimit> {
    let mut smallest: Option<MemoryLimit> = None;
    let mut rel = cgroup_path.trim_start_matches('/');
    loop {
        let dir = if rel.is_empty() { mount.to_path_buf() } else { mount.join(rel) };
        for file in files {
            let at = dir.join(file);
            if let Some(bytes) = read_limit_file(&at)
                && smallest.as_ref().is_none_or(|seen| bytes < seen.bytes)
            {
                smallest = Some(MemoryLimit { bytes, read_from: at });
            }
        }
        match rel.rfind('/') {
            Some(at) => rel = &rel[..at],
            None if rel.is_empty() => break,
            None => rel = "",
        }
    }
    smallest
}

/// A memory limit this process is running under, and the file that states it
/// (`docs/design/decisions.md`, "D11").
///
/// **The path is carried because the two v2 files do different things and the
/// walk minimises over both**: `memory.high` throttles where `memory.max`
/// kills (`RT3`), and either may be stated on an ancestor rather than on the
/// leaf (`RT5`), so a cut budget is only actionable beside the file whose
/// number did the cutting. It is a display fact: nothing in the library
/// branches on it, and what reads it is a message — `pgdt`'s status line, the
/// DataFusion provider's budget note
/// (`docs/design/decisions.md`, "D64").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLimit {
    /// The smallest limit that binds, in bytes — the minimum over every file
    /// and every ancestor cgroup.
    pub bytes: u64,
    /// The file that stated it, as reached: under
    /// [`discover_memory_limit_in`]'s root rather than absolute, so a fixture
    /// tree's answer names the fixture.
    pub read_from: PathBuf,
}

/// The memory limit this process is actually running under, in bytes, or
/// `None` where nothing limits it (`docs/design/decisions.md`, "D11").
///
/// **A primitive, not a policy**: what a caller may usefully take from a
/// limit is [`Allowance::discover`](crate::Allowance::discover) (`docs/design/decisions.md`, "D11").
///
/// **`None` means no limit is being *enforced*, which is a complete and
/// checkable statement** — unlike "are we in a container", which no reading of
/// `/.dockerenv` or `/proc/self/cgroup` answers on every runtime.
///
/// The reading is `RT1`–`RT6` of
/// [`docs/design/runtime-invariants.md`](../../../docs/design/runtime-invariants.md):
/// which hierarchy owns the `memory` controller, that hierarchy's files
/// (`memory.max` and `memory.high` on v2, `memory.limit_in_bytes` on v1), and
/// the minimum over every ancestor cgroup rather than the nearest one.
pub fn discover_memory_limit() -> Option<MemoryLimit> {
    discover_memory_limit_in(Path::new("/"))
}

/// [`discover_memory_limit`] against an arbitrary filesystem root.
///
/// **The seam the arms a test must drive need, no machine having more than
/// one of them** (`docs/design/decisions.md`, "D11"): `internal`'s, as are
/// [`available_memory_in`] and
/// [`discover_allowance_in`](crate::arrangement::discover_allowance_in) beside it, so the resolution can
/// be pinned against a v1 hierarchy, an unlimited one and a below-reserve one
/// (`crates/pgdt-io/tests/data/runtime/`). The memory controller lives in exactly one
/// hierarchy at a time (`RT6`), so a v1 shape cannot be produced beside a v2
/// one on the same host.
///
/// It is not a chroot facility: paths are joined onto the root, so a root of
/// `/` is the real reading and anything else is a tree somebody built. What a
/// fixture tree establishes is that this reader handles the shape `RT4`
/// describes, not that a kernel still produces it.
pub fn discover_memory_limit_in(root: &Path) -> Option<MemoryLimit> {
    let cgroups = std::fs::read_to_string(root.join("proc/self/cgroup")).ok()?;
    match memory_hierarchy(&cgroups) {
        MemoryHierarchy::V1(path) => {
            smallest_limit(&v1_memory_mount(root), path, &["memory.limit_in_bytes"])
        }
        MemoryHierarchy::V2(path) => {
            smallest_limit(&root.join("sys/fs/cgroup"), path, &["memory.max", "memory.high"])
        }
        MemoryHierarchy::None => None,
    }
}

/// The kernel's own estimate of the memory obtainable without swapping, in
/// bytes — `/proc/meminfo`'s `MemAvailable`
/// (`docs/design/runtime-invariants.md`, `RT8`).
///
/// **It is the host's number even inside a container**, so it is meaningful
/// only once [`discover_memory_limit`] has answered `None`.
///
/// **`MemAvailable` and not `MemFree`**, which excludes reclaimable page cache
/// and would throttle a scan for memory the kernel would hand straight back.
///
/// **A ceiling to plan under, never a reservation.** It is an estimate, it
/// moves second to second, and two processes reading it at once each see the
/// whole of it — which is why the caller takes half.
pub fn available_memory() -> Option<u64> {
    available_memory_in(Path::new("/"))
}

/// [`available_memory`] against an arbitrary filesystem root, for the same
/// reason [`discover_memory_limit_in`] takes one: the no-limit branch is
/// otherwise only exercisable on a machine that has no limit.
pub fn available_memory_in(root: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(root.join("proc/meminfo")).ok()?;
    let line = text.lines().find_map(|line| line.strip_prefix("MemAvailable:"))?;
    let mut fields = line.split_whitespace();
    let value = fields.next()?.parse::<u64>().ok()?;
    match fields.next() {
        // `meminfo_proc_show` prints every size in kB and the unit is part of
        // the line; a bare number is accepted so that a fixture need not
        // repeat it.
        Some("kB") => Some(value.saturating_mul(1024)),
        None => Some(value),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests;
