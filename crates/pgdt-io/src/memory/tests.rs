use crate::test_support::*;
use crate::*;

use super::*;

/// **The `0::` line is selected by its shape, and a dead cgroup's path is
/// not a directory name** (`RT1`). The hierarchy id is whatever
/// `idr_alloc_cyclic` handed out and the line order is unspecified, so
/// neither may be read; the ` (deleted)` suffix would otherwise be joined
/// onto the mount point.
#[test]
fn the_unified_line_is_found_by_shape_and_stripped_of_a_deleted_suffix() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "7:name=probe:/somewhere\n0::/leaf (deleted)\n").v2_limits(
        "/leaf",
        Some("536870912"),
        None,
    );
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(536870912));
}

/// **The controller field is a comma-separated list matched by
/// membership** (`RT6`): a v1 hierarchy carrying only a *name* reads
/// `name=memory` and holds no memory controller at all.
#[test]
fn a_hierarchy_merely_named_memory_is_not_the_memory_controller() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "3:name=memory:/decoy\n0::/leaf\n").v2_limits(
        "/leaf",
        Some("268435456"),
        None,
    );
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
}

/// **`max` is a sentinel for one level, not a statement that the process is
/// unlimited** (`RT2`), and an absent file is "no limit here" rather than an
/// error (`RT5`) — the hierarchy root carries neither file at all. So the
/// walk climbs past both and finds the ancestor that binds.
#[test]
fn an_ancestors_limit_binds_where_the_leaf_states_max() {
    let root = FakeRoot::new();
    root.v2("/pgdt-probe/child")
        .v2_limits("/pgdt-probe/child", Some("max"), Some("max"))
        .v2_limits("/pgdt-probe", Some("268435456"), None);
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
}

/// **`memory.high` and `memory.max` are minimised together, across levels
/// and across the two files** (`RT3`, `RT5`): nothing orders them, and the
/// throttle two levels up may be the smallest number in the walk.
#[test]
fn the_smallest_of_every_limit_at_every_level_is_what_binds() {
    let root = FakeRoot::new();
    root.v2("/a/b/c")
        .v2_limits("/a/b/c", Some("2147483648"), None)
        .v2_limits("/a/b", None, Some("134217728"))
        .v2_limits("/a", Some("1073741824"), None);
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(134217728));
}

/// **The file that stated the smallest limit comes back with it**:
/// `memory.high` throttles where `memory.max` kills (`RT3`) and either may
/// be an ancestor's (`RT5`), so a status line is only actionable beside the
/// file that cut the budget (`docs/design/decisions.md`, "D64").
#[test]
fn a_discovered_limit_names_the_file_that_stated_it() {
    let root = FakeRoot::new();
    root.v2("/a/b").v2_limits("/a/b", Some("2147483648"), None).v2_limits(
        "/a",
        None,
        Some("134217728"),
    );
    let limit = discover_memory_limit_in(root.path()).expect("a limit binds here");
    assert_eq!(limit.bytes, 134217728);
    assert_eq!(limit.read_from, root.path().join("sys/fs/cgroup/a/memory.high"));
}

/// **Nothing anywhere is `None`, and that is a complete statement**: no
/// limit found means no limit is being enforced. A tree with no cgroup
/// file at all reads the same way, which is the non-Linux case.
#[test]
fn an_unlimited_hierarchy_and_a_missing_one_both_read_as_no_limit() {
    let root = FakeRoot::new();
    root.v2("/leaf").v2_limits("/leaf", Some("max"), Some("max"));
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), None);
    assert_eq!(discover_memory_limit_in(FakeRoot::new().path()).map(|l| l.bytes), None);
}

/// **A v1 hierarchy is read from its own mount, located through
/// `mountinfo`** (`RT4`, `RT6`), and "unlimited" there is a *threshold*
/// near `LONG_MAX` rather than a sentinel string, the value being a
/// function of the page size and the word width.
///
/// This establishes that the reader handles the shape `RT4` describes. It
/// observes no kernel (`docs/design/decisions.md`, "D11").
#[test]
fn the_v1_arm_reads_its_own_mount_and_treats_a_huge_value_as_no_limit() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "5:memory:/svc\n0::/leaf\n")
        .write(
            "proc/self/mountinfo",
            "31 24 0:27 / /sys/fs/cgroup/memory rw,nosuid - cgroup cgroup rw,memory\n",
        )
        .write("sys/fs/cgroup/memory/svc/memory.limit_in_bytes", "536870912\n")
        // The v2 files are there and must not be consulted: a controller
        // lives in exactly one hierarchy, and this process's memory
        // controller is the v1 one.
        .v2_limits("/leaf", Some("104857600"), None);
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(536870912));

    let unset = FakeRoot::new();
    unset
        .write("proc/self/cgroup", "5:memory:/\n")
        .write(
            "proc/self/mountinfo",
            "31 24 0:27 / /sys/fs/cgroup/memory rw - cgroup cgroup rw,memory\n",
        )
        // 32-bit `PAGE_COUNTER_MAX × PAGE_SIZE`, the smallest shape
        // "unlimited" takes and so the one a threshold has to clear.
        .write("sys/fs/cgroup/memory/memory.limit_in_bytes", "8796093018112\n");
    assert_eq!(discover_memory_limit_in(unset.path()).map(|l| l.bytes), None);
}

/// **The conventional mount is the fallback, not the authority** — a tree
/// that lists no cgroup mount still reads `/sys/fs/cgroup/memory`, which is
/// where `file-hierarchy(7)` puts it.
#[test]
fn the_v1_arm_falls_back_to_the_conventional_mount() {
    let root = FakeRoot::new();
    root.write("proc/self/cgroup", "5:cpu,memory,cpuacct:/svc\n")
        .write("sys/fs/cgroup/memory/svc/memory.limit_in_bytes", "268435456\n");
    assert_eq!(discover_memory_limit_in(root.path()).map(|l| l.bytes), Some(268435456));
}

/// **`MemAvailable`, in kB, and nothing else** (`RT8`). `MemFree` is on
/// the same file and is not read: it excludes reclaimable page cache.
#[test]
fn available_memory_reads_memavailable_and_converts_from_kb() {
    let root = FakeRoot::new();
    root.write(
        "proc/meminfo",
        "MemTotal:       32774304 kB\nMemFree:         1489828 kB\n\
             MemAvailable:   20002184 kB\n",
    );
    assert_eq!(available_memory_in(root.path()), Some(20002184 * 1024));
    assert_eq!(available_memory_in(FakeRoot::new().path()), None);
}

/// **What a mapping pass's statistics are left has two bands, and its
/// floor is not the margin's** (`docs/design/decisions.md`, "D85"). Both
/// lines are arithmetic between the three constants rather than readings,
/// and nothing else would notice one of them moving past another.
///
/// **Its own zero line is `MEMORY_UNPOOLED_BOUND × 100 / (100 −
/// MEMORY_MARGIN_PERCENT)`** — at and below it the ceiling this subtracts
/// a budget from is zero on its own, so no allowance there gathers
/// anything at any budget, and the line is reached rather than merely
/// approached.
///
/// **The swap is `the_margin_is_the_tighter_condition_only_above_the_crossover`'s
/// crossover read the other way round.** Below it a count is solved
/// against the cap, which stands under the ceiling, so `ceiling − cap`
/// survives whatever the source charges; above it the count is solved
/// against the ceiling itself and a source whose step divides it takes the
/// statistics to nothing — a wide allowance, not a narrow one.
#[test]
fn statistics_are_starved_below_their_own_line_and_unfloored_above_the_crossover() {
    let zero_line = MEMORY_UNPOOLED_BOUND * 100 / (100 - MEMORY_MARGIN_PERCENT);
    assert_eq!(zero_line, 320 << 20);
    assert_eq!(statistics_allowance(zero_line, 0), 0, "at the line, not only under it");
    assert_eq!(statistics_allowance(zero_line - (1 << 20), 0), 0);
    assert!(statistics_allowance(zero_line + (1 << 20), 0) > 0, "and positive above it");

    let crossover = 5 * (MEMORY_RESERVE - MEMORY_UNPOOLED_BOUND);
    let below = crossover - (64 << 20);
    let above = crossover + (64 << 20);

    // Below the crossover no budget a fit reports can reach the ceiling,
    // the cap being the tighter of the two: `ceiling − cap` is left at
    // every per-worker charge, including one too large for a single worker.
    let floor = margin_allowance(below) - (below - MEMORY_RESERVE);
    assert!(floor > 0, "an allowance in this band cannot starve statistics");
    for per_worker in [1u64 << 20, 17 << 20, 64 << 20, 96 << 20, 1 << 30] {
        let fitted = Parallelism::within(24, Some(WorkerMemory::per_worker(per_worker)), below);
        let left = statistics_allowance(below, fitted.memory_bytes().unwrap());
        assert!(left >= floor, "{per_worker} left {left}, under the cap's own floor {floor}");
    }

    // Above it the ceiling binds instead, so the count is solved right up
    // against the number this subtracts from and there is no floor left.
    let ceiling = margin_allowance(above);
    assert!(ceiling < above - MEMORY_RESERVE, "the ceiling binds here, not the cap");
    let fitted = Parallelism::within(24, Some(WorkerMemory::per_worker(ceiling / 4)), above);
    assert_eq!(fitted.jobs(), 4, "four workers is what the ceiling exactly affords");
    assert_eq!(statistics_allowance(above, fitted.memory_bytes().unwrap()), 0);
}
