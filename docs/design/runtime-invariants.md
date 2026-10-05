# Runtime-environment invariants we rely on

Every entry is a property of the environment the process is *given* — the
kernel's cgroup interface, and the standard library's reading of it — that some
design decision treats as guaranteed. Each records what the invariant is, the
source that proves it, the versions it was verified against, and how to
re-verify it.

**This is [`postgres-invariants.md`](postgres-invariants.md)'s sibling, and it
exists for the same reason**: a kernel release, a container runtime, or a
toolchain bump can quietly invalidate one of these, and the resulting bug
surfaces as a process that sized itself wrongly rather than as an error — an OOM
kill under an orchestrator, or a scan that took a fifth of the machine it was
given. The trigger to walk this file is therefore four-sided, and each side has
its own entries: a **kernel major**, a **container-runtime upgrade**, a **Rust
toolchain bump** (`RT7`, `RT11` and `RT12`, whose behaviour is `std`'s), a **glibc
release** — the host's or the figures' image's (`RT10` only) — a **bash
release** (`RT20`), and an
**`object_store` upgrade** (`RT13`–`RT17`, whose behaviour is that crate's and
whose re-verification is a test run against the oracle rather than a container;
`pgdt/tests/http_conformance.rs` reads the same claims against a real origin
wherever one is offered — [`../../CONTRIBUTING.md`](../../CONTRIBUTING.md),
"Building and testing").

**It is named for the runtime environment rather than for Linux or for
cgroups.** The mechanism these entries serve — a process discovering its own
allocation — has an answer on a machine with no container runtime and on an
operating system with no cgroups, and that answer belongs in the same file as
this one. Naming the file after the evidence it happens to hold today would send
the next such entry somewhere else.

**Identifiers are `RT<n>`**, allocated on discovery and never reused, for the
reason every register here numbers its entries: an entry gets cited, and a
citation that renumbers is a citation that lies. Two letters rather than one is
`docs/process.md`'s advice under "Known deficiencies", and here it is not
merely advice — `R<n>` is already spoken for in this tree by the `xz-seek`
crate's requirements register, which two phase inboxes cite by number
([`roadmap-P15-gzip-inbox.md`](roadmap-P15-gzip-inbox.md), "The seekable-xz
crate is xz-only on purpose, and generalizing it was rejected").

**`RT1`–`RT20` are allocated**, and nothing at or below `RT20` is reused.

**The `Re-verify` field is a container invocation, not a citation.** Reading the
kernel source proves what the kernel *does*; what a decision here rests on is
that the value a deployment asks for is the value this process reads back, and
only a run establishes that. That is the same ritual `postgres-invariants.md`
runs across six server images, and an entry without one is an entry nobody
checks. Where a claim cannot be produced on this machine — `RT4`, which needs a
host booted onto the v1 hierarchy — the entry says so in `Verified against`
rather than quietly resting on source alone.

Kernel line numbers and quotations below are from **v7.1** and are a starting
point, not an anchor — grep for the quoted code instead.

**A citation here is a repo-relative path against a stated version, and that is
the whole of it.** `mm/memcontrol-v1.c` at v7.1 names one file unambiguously
wherever that tree happens to live, so how you obtain it is not part of the
citation and no entry below names a location. Today the convenient way is
`https://raw.githubusercontent.com/torvalds/linux/v7.1/<path>`; a local checkout
serves identically.

**`RT7` is the exception worth naming, because its source is not Linux and
ships with the thing it is an invariant about.** `library/std/…` resolves under
rustup's `rust-src` component, at
`$(rustc --print sysroot)/lib/rustlib/src/rust/` — which means it is version-locked
to the toolchain whose behaviour `RT7` claims, and a toolchain bump moves the
file and the claim together. The component is **optional**: a walk that finds
the path missing should `rustup component add rust-src` rather than conclude the
file moved.

*Rejected: a local Linux clone at `/mnt/wd12t/upstream/linux/`, one worktree per
tag, mirroring how `postgres-invariants.md`'s checkouts are kept.* The sibling
needs its tree because its walk **is** the grep — 47 entries whose `Re-verify`
greps `src/bin/pg_dump/`, none of which can run without it. This register's walk
is seven *runs*, and its source proves the mechanism rather than checking the
claim, so a tree would be opened only when a run came back unexpected: a
diagnostic moment, with a known file to fetch and no hurry. Against that, a
worktree-per-tag convention costs a full clone of Linux on every kernel major,
which is the register's own walk trigger. The citations above are unaffected
either way — they name a path and a tag, not a location — so this judgement is
reversible at the cost of the one paragraph you are reading.

Container invocations are written with `docker`, which is this project's
convention for the container runtime; see `CLAUDE.local.md` for what it is on
this machine.

---

## RT1 — `/proc/self/cgroup` locates the process's own cgroup, and `/sys/fs/cgroup` is where it is mounted

**Claim.** A process can find the cgroup directory that governs it by reading
`/proc/self/cgroup` and joining the path it reports onto the cgroup filesystem's
mount point. The file holds one line per hierarchy, each
`<hierarchy-id>:<controllers>:<path>`; the cgroup v2 entry is **always**
`0::<path>`, with an empty controller field. The v2 mount point is
`/sys/fs/cgroup` by convention (`file-hierarchy(7)`), and `/proc/self/mountinfo`
is the authority where it is not.

**Proof.** `proc_cgroup_show()` in `kernel/cgroup/cgroup.c` prints
`root->hierarchy_id`, then a `:`, then the controller names — skipped entirely
for the default hierarchy (`if (root != &cgrp_dfl_root)`) — then a `:`, then the
path. `cgroup_setup_root(&cgrp_dfl_root, 0)` allocates the default root's id
from `cgroup_hierarchy_idr` starting at 0, which is why the v2 line reads `0::`.
`Documentation/admin-guide/cgroup-v2.rst` states it flatly: *"The entry for
cgroup v2 is always in the format `0::$PATH`."*

**Scope limit.** Three parse hazards:

- **The path is namespace-relative.** `proc_cgroup_show()` renders it through
  `cgroup_path_ns_locked(…, current->nsproxy->cgroup_ns)`, so a process inside
  a cgroup namespace sees its cgroupns root as `/`. The runtime mounts that
  same cgroup at `/sys/fs/cgroup`, so the join is correct, but the string is not
  a host path and must never be reported as one. Observed:
  `docker run --memory 512m` gives `0::/`, while the same container with
  `--cgroupns=host` gives
  `0::/system.slice/nerdctl-<id>.scope`.
- **A dead cgroup's path carries a ` (deleted)` suffix**, for a zombie on the
  default hierarchy (`if (cgroup_on_dfl(cgrp) && cgroup_is_dead(cgrp))
  seq_puts(m, " (deleted)\n")`). A reader that joins the raw path onto the mount
  point opens a path that does not exist.
- **Line order is not specified.** Where both hierarchies exist the v2 line may
  come last; observed here as `<n>:name=pgdtprobe:/` above
  `0::/user.slice/…`. Select the line by its shape, never by its position — and
  not by the hierarchy id either, which `idr_alloc_cyclic` hands out cyclically
  and which came back as `1` on one run of the check below and `2` on the next.

**Verified against:** kernel v7.1 (source); Linux 7.1.4-arch1-1, nerdctl 2.3.5 /
containerd v2.3.3, cgroup driver systemd, cgroup version 2 (observed).

**Relied on by:**
[`decisions.md`](decisions.md), "I/O, memory and parallelism" — `discover_memory_limit`
begins here.

**Re-verify.**

```sh
docker run --rm --memory 512m alpine:3 sh -c \
  'cat /proc/self/cgroup; grep -E " cgroup2? " /proc/self/mountinfo'
docker run --rm --memory 512m --cgroupns=host alpine:3 cat /proc/self/cgroup
```

The first prints `0::/` and one `cgroup2` mount at `/sys/fs/cgroup`; the second
prints the container's full host path under the same `0::` shape.

---

## RT2 — cgroup v2 `memory.max` is a byte count or the literal string `max`

**Claim.** `<cgroup>/memory.max` on a v2 hierarchy holds either a decimal byte
count or the four bytes `max`, and nothing else. `max` means *no hard limit set
at this level*. A byte count read back is the requested value floored to a whole
page, so it is never larger than what was asked for and is equal whenever the
request was page-aligned. The file exists on a **non-root** cgroup whose parent
has the memory controller enabled in its `cgroup.subtree_control`, and never on
the root.

**Proof.** `seq_puts_memcg_tunable()` in `mm/memcontrol.c` is the whole of the
read side:

```c
if (value == PAGE_COUNTER_MAX)
        seq_puts(m, "max\n");
else
        seq_printf(m, "%llu\n", (u64)value * PAGE_SIZE);
```

The write side is `page_counter_memparse(buf, "max", …)` in `mm/page_counter.c`,
which maps the literal string to `PAGE_COUNTER_MAX` and otherwise stores
`min(bytes / PAGE_SIZE, PAGE_COUNTER_MAX)` — an integer division, which is the
page floor. `Documentation/admin-guide/cgroup-v2.rst`: *"A read-write single
value file which exists on non-root cgroups. The default is `max`. … Memory
usage hard limit."*

**Scope limit.** `max` is a *sentinel for this level*, not a statement that the
process is unlimited — an ancestor may still bind (`RT5`). The floor is
observable: `--memory 100000001b` reads back `99999744`, which is
`floor(100000001 / 4096) × 4096`.

**Verified against:** kernel v7.1 (source); Linux 7.1.4-arch1-1, nerdctl 2.3.5 /
containerd v2.3.3 (observed: `--memory 512m` → `536870912`, exactly 512 MiB;
`--memory 100000001b` → `99999744`; the root cgroup has no `memory.max` file at
all).

**Relied on by:**
[`decisions.md`](decisions.md), "I/O, memory and parallelism".

**Re-verify.**

```sh
docker run --rm --memory 512m       alpine:3 cat /sys/fs/cgroup/memory.max
docker run --rm --memory 100000001b alpine:3 cat /sys/fs/cgroup/memory.max
docker run --rm                     alpine:3 cat /sys/fs/cgroup/memory.max
```

`536870912`, then `99999744`, then `max`.

---

## RT3 — cgroup v2 `memory.high` binds throughput where `memory.max` binds survival

**Claim.** `<cgroup>/memory.high` has exactly `RT2`'s shape — a byte count or
the literal `max`, page-floored, present on non-root cgroups only — and it may
be set **below** `memory.max`. Exceeding it never invokes the OOM killer;
instead the cgroup's processes are throttled and put under heavy reclaim. It is
therefore a limit that binds a scan even though it cannot end one.

**Proof.** `memory_high_show()` in `mm/memcontrol.c` reads through the same
`seq_puts_memcg_tunable()` as `memory.max`, and `memory_high_write()` through
the same `page_counter_memparse(buf, "max", …)`.
`Documentation/admin-guide/cgroup-v2.rst`: *"Memory usage throttle limit. If a
cgroup's usage goes over the high boundary, the processes of the cgroup are
throttled and put under heavy reclaim pressure. Going over the high limit never
invokes the OOM killer."*

**Scope limit.** `memory.high` throttles rather than kills, so a process that
ignores it finishes, slowly: sustained reclaim ends a scan's throughput as
surely as an OOM ends the run (D11). Nothing orders the two files; take the
minimum (`RT5`).

**Verified against:** kernel v7.1 (source); Linux 7.1.4-arch1-1, nerdctl 2.3.5 /
containerd v2.3.3 (observed: `--memory 512m --cgroup-conf
memory.high=268435456` → `memory.max` `536870912`, `memory.high` `268435456`).

**Relied on by:**
[`decisions.md`](decisions.md), "I/O, memory and parallelism" — the discovered limit
being the minimum over every limit that binds.

**Re-verify.**

```sh
docker run --rm --memory 512m --cgroup-conf memory.high=268435456 alpine:3 \
  sh -c 'echo max=$(cat /sys/fs/cgroup/memory.max) high=$(cat /sys/fs/cgroup/memory.high)'
```

`max=536870912 high=268435456`.

---

## RT4 — cgroup v1 `memory.limit_in_bytes` is always a number, and "unlimited" is a page-size-dependent value near `LONG_MAX`

**Claim.** On a v1 memory hierarchy, `<cgroup>/memory.limit_in_bytes` holds a
decimal byte count and **never** a sentinel string. An unset limit reads as
`PAGE_COUNTER_MAX × PAGE_SIZE`, which on 64-bit with 4 KiB pages is
**9223372036854771712**. The value is page-floored on write exactly as `RT2`'s
is.

**Proof.** The file's `read_u64` is `mem_cgroup_read_u64()` in
`mm/memcontrol-v1.c`, whose `RES_LIMIT` arm is `return (u64)counter->max *
PAGE_SIZE;` — no sentinel branch anywhere on the path. `counter->max` is
initialised to `PAGE_COUNTER_MAX` by `page_counter_init()`, and
`include/linux/page_counter.h` defines that as `LONG_MAX / PAGE_SIZE` on 64-bit
(and `LONG_MAX` on 32-bit). Writes go through the same
`page_counter_memparse()` as `RT2`.

**Scope limit.** **Do not test for equality with a constant.** The unset value
is a function of `PAGE_SIZE` and of `BITS_PER_LONG`: 9223372036854771712 at 4
KiB pages, 9223372036854759424 at 16 KiB, 9223372036854710272 at 64 KiB
(arm64's `CONFIG_ARM64_64K_PAGES`), and 8796093018112 on a 32-bit kernel. Read
it as a **threshold** — a value at or above what the machine could possibly have
is "no limit" — which is correct at every page size and on both word widths.

**Verified against:** kernel v7.1 (source) only. **Not observed here**: this
machine runs a pure v2 unified hierarchy, and the memory controller lives in
exactly one hierarchy at a time (`RT6`), so it cannot be mounted v1 alongside;
observing this entry needs a host booted with
`systemd.unified_cgroup_hierarchy=0`, which is what the `Re-verify` below runs.
`io.rs`'s unit tests cover our reader instead: a fixture tree shaped as this
entry claims, driven through `discover_memory_limit`'s filesystem-root seam
(D11). That observes no kernel and does not upgrade `Verified against`.

**Relied on by:**
[`decisions.md`](decisions.md), "I/O, memory and parallelism" — the v1 arm of
`discover_memory_limit`.

**Re-verify.** On a host booted with `systemd.unified_cgroup_hierarchy=0`:

```sh
docker run --rm --memory 512m alpine:3 sh -c \
  'cat /proc/self/cgroup;
   cat /sys/fs/cgroup/memory/memory.limit_in_bytes'
docker run --rm alpine:3 cat /sys/fs/cgroup/memory/memory.limit_in_bytes
```

`/proc/self/cgroup` carries a line whose controller list contains `memory`; the
limited container reads `536870912`, the unlimited one a value at or above
`9223372036854710272`. On a v2 host the second path does not exist, and that
absence is itself the check that `RT6`'s "exactly one hierarchy" still holds.

---

## RT5 — an ancestor's limit binds, and is invisible in the leaf's own file

**Claim.** A memory limit set on any ancestor cgroup constrains a process in a
descendant, and the descendant's own `memory.max` says nothing about it. The
**effective** limit is therefore the minimum over the process's own cgroup and
every ancestor up to the hierarchy root, and a reader that consults only the
leaf will believe an unlimited process is unlimited when it is not.

**Proof.** `Documentation/admin-guide/cgroup-v2.rst`: *"The limits and other
settings of all resource controllers are hierarchical and regardless of what
happens in the delegated sub-hierarchy, nothing can escape the resource
restrictions imposed by the parent."* The v1 documentation states the accounting
side of the same thing: *"all memory usage of e, is accounted to its ancestors
up until the root (i.e, c and root). If one of the ancestors goes over its
limit, the reclaim algorithm reclaims from the tasks in the ancestor…"*
Structurally it is `struct page_counter`'s `parent` pointer: a charge walks it
to the root.

Observed directly. With `/pgdt-probe` limited to 256 MiB and an unlimited child
under it, a process in the child reports:

```
0::/pgdt-probe/child
max                      # its own memory.max
268435456                # /pgdt-probe/memory.max — the limit that actually binds
```

**Scope limit.** Three:

- **The root cgroup has no `memory.max` or `memory.high` file at all**, so the
  walk must treat "file absent" as "no limit here" and not as an error.
  Observed: `/sys/fs/cgroup/memory.max` does not exist on this host.
- **A cgroup namespace truncates the walk**, and correctly so: inside one, the
  namespace root is what is mounted at `/sys/fs/cgroup`, so walking up from the
  `0::` path never leaves it. Limits set on cgroups *outside* the namespace
  still bind and are simply not readable, so the walk is bounded by the mount
  point rather than by counting `/`s.
- **`memory.high` and `memory.max` are minimised together**, across levels and
  across the two files: nothing orders them, and a `memory.high` two levels up
  may be the smallest number in the walk.

**Verified against:** kernel v7.1 (source); Linux 7.1.4-arch1-1 (observed, via
the scratch hierarchy in the `Re-verify` below).

**Relied on by:**
[`decisions.md`](decisions.md), "I/O, memory and parallelism" — every ancestor cgroup
read rather than the nearest.

**Re-verify.** The container form shows the walk; the scratch-hierarchy form
shows that an ancestor's limit is invisible at the leaf.

```sh
docker run --rm --cgroupns=host --memory 512m alpine:3 sh -c \
  'p=$(sed -n "s/^0:://p" /proc/self/cgroup)
   while [ -n "$p" ]; do echo "$p: $(cat /sys/fs/cgroup$p/memory.max 2>/dev/null)"; p=${p%/*}; done
   cat /sys/fs/cgroup/memory.max 2>&1'
```

The leaf reads `536870912`, its parent `max`, and the root reports no such file.

```sh
sudo sh -c '
  mkdir -p /sys/fs/cgroup/pgdt-probe
  echo "+memory" > /sys/fs/cgroup/pgdt-probe/cgroup.subtree_control
  echo 268435456 > /sys/fs/cgroup/pgdt-probe/memory.max
  mkdir -p /sys/fs/cgroup/pgdt-probe/child
  sh -c "echo \$\$ > /sys/fs/cgroup/pgdt-probe/child/cgroup.procs
         cat /proc/self/cgroup
         cat /sys/fs/cgroup/pgdt-probe/child/memory.max"'
sudo rmdir /sys/fs/cgroup/pgdt-probe/child /sys/fs/cgroup/pgdt-probe
```

The process reports `0::/pgdt-probe/child` and its own limit as `max`, while
256 MiB binds one level up. **Remove the scratch cgroups**; the `rmdir` is part
of the check, not cleanup after it. The `+memory` line is load-bearing: without
the controller enabled in the parent's `cgroup.subtree_control` the child has no
`memory.max` file at all — the first scope limit's "file absent, limit still
binding" case rather than an error.

---

## RT6 — a controller lives in exactly one hierarchy, so `/proc/self/cgroup` says which files to read

**Claim.** The `memory` controller is bound to the v2 hierarchy **or** to a v1
hierarchy, never to both at once. So `/proc/self/cgroup` decides, unambiguously,
which of `RT2`/`RT3` and `RT4` applies: if any line's controller field contains
`memory`, the limit is v1 and lives under that hierarchy's mount point;
otherwise the `0::` line's path under the v2 mount is where it is. A machine may
carry both hierarchies at once, so finding a `0::` line is not by itself
evidence that the memory limit is a v2 one.

**Proof.** `Documentation/admin-guide/cgroup-v2.rst`: *"All controllers which
support v2 and are not bound to a v1 hierarchy are automatically bound to the v2
hierarchy and show up at the root. Controllers which are not in active use in
the v2 hierarchy can be bound to other hierarchies. This allows mixing v2
hierarchy with the legacy v1 multiple hierarchies in a fully backward compatible
way."* And: *"A controller can be moved across hierarchies only after the
controller is no longer referenced in its current hierarchy."*

**Scope limit.** **The controller field is a comma-separated list and must be
matched by membership, never by substring.** A v1 hierarchy may carry *no*
controllers and only a name, in which case the field reads `name=<x>` — so a
hierarchy named `memory` would satisfy a substring test and hold no memory
controller at all. Observed here, alongside the live v2 line:

```
2:name=pgdtprobe:/
0::/user.slice/user-1000.slice/user@1000.service/…
```

This is also the rule `std` follows for the CPU quota (`RT7`), splitting on `,`
and comparing each element.

A second limit, from the same paragraph: moving a controller between hierarchies
is possible at runtime and *"strongly discouraged for production use"*, so a
process may outlive the arrangement it read. Discovery happens once at startup
and this is not defended against.

**Verified against:** kernel v7.1 (source); Linux 7.1.4-arch1-1 (observed: a
named v1 hierarchy mounted in a private mount namespace produces the two-line
file above, with the v2 line **second**).

**Relied on by:**
[`decisions.md`](decisions.md), "I/O, memory and parallelism" — which of the two file shapes
`discover_memory_limit` reads.

**Re-verify.** The hybrid shape, produced without touching the host:

```sh
sudo unshare -m sh -c '
  mkdir -p /tmp/cg1probe
  mount -t cgroup -o none,name=pgdtprobe cgroup /tmp/cg1probe
  cat /proc/self/cgroup
  umount /tmp/cg1probe'
```

Two lines: `<n>:name=pgdtprobe:/` — the id is whatever `idr_alloc_cyclic` next
hands out — and the machine's `0::` line. The mount lives in
a private mount namespace and leaves nothing behind. On this machine the v2
hierarchy owns `memory`, which the presence of `/sys/fs/cgroup/memory.max` on a
non-root cgroup and the *absence* of `/sys/fs/cgroup/memory/` together confirm.

---

## RT7 — `std::thread::available_parallelism` already reads the cgroup CPU quota

**Claim.** On Linux, `available_parallelism()` returns
`min(CPU_COUNT(sched_getaffinity), cgroup CPU quota)`, where the quota is
`cpu.max`'s `limit / period` on v2 or `cpu.cfs_quota_us / cpu.cfs_period_us` on
v1, minimised over every ancestor, rounded **down**, and floored at 1. An
unset quota (`max` in v2, an unparseable or absent value in v1) leaves the
affinity count alone. So the CPU half of "discover the allocation we were
given" needs no code here.

**Proof.** `library/std/src/sys/thread/unix.rs`. The entry point takes
`quota = cgroups::quota().max(1)` and then `CPU_COUNT(&set).min(quota)`, with
the `sysconf(_SC_NPROCESSORS_ONLN)` fallback also `.min(quota)`. `cgroups::quota()`
reads `/proc/self/cgroup`, distinguishes the hierarchies exactly as `RT6`
describes — `Some(b"") => Cgroup::V2`, otherwise a `,`-split membership test for
`"cpu"` — and dispatches to `quota_v2` or `quota_v1`. `quota_v2` walks upward
(`while path.starts_with(cgroup_mount)`) taking `quota.min(limit / period)` at
each level; `quota_v1` does the same over `cpu.cfs_quota_us` /
`cpu.cfs_period_us`. `limit / period` is integer division, which is the
round-down.

Observed, on a 24-CPU host where `nproc` (which reads only the affinity mask)
answers 24 throughout:

| run | `cpu.max` | `available_parallelism` |
|---|---|---|
| `--cpus=2` | `200000 100000` | `Ok(2)` |
| `--cpus=3.5` | `350000 100000` | `Ok(3)` |
| no flag | `max 100000` | `Ok(24)` |

**Scope limit.** `std`'s own module comment names two: *"cgroup v2 in
non-standard mountpoints"* (it hardcodes `/sys/fs/cgroup` for v2, falling back
to a `/proc/self/mountinfo` scan for v1 only) and *"paths containing control
characters or spaces, since those would be escaped in procfs output and we don't
unescape"*. Two more follow from the code: a v2 `cpu.max` of `max 100000` leaves
the quota unbounded because `"max".parse::<usize>()` simply fails, and the
result is a **quota**, not a share — `cpu.weight` and `cpuset.cpus.partition`
are not read, though `cpuset.cpus` is, through the affinity mask.

**Verified against:** Rust 1.98.0 (source and observed); Linux 7.1.4-arch1-1,
nerdctl 2.3.5 / containerd v2.3.3.

**Relied on by:**
[`decisions.md`](decisions.md), "I/O, memory and parallelism" — the source's own worker default, `.xz` taking
`available_parallelism()` clamped by the budget.

**Re-verify.** Build the one-line probe and run it under a quota:

```sh
cat > /tmp/ap.rs <<'EOF'
fn main() { println!("{:?}", std::thread::available_parallelism()); }
EOF
rustc -O /tmp/ap.rs -o /tmp/ap
for c in --cpus=2 --cpus=3.5; do
  docker run --rm $c -v /tmp/ap:/ap:ro debian:stable-slim \
    sh -c 'cat /sys/fs/cgroup/cpu.max; nproc; /ap'
done
```

`Ok(2)` then `Ok(3)`, with `nproc` answering the host's CPU count in both. Read
the implementation alongside it:

```sh
sed -n '/^mod cgroups/,/^}/p' \
  "$(rustc --print sysroot)/lib/rustlib/src/rust/library/std/src/sys/thread/unix.rs"
```

(`rustup component add rust-src` if that path is absent.)

## RT8 — `/proc/meminfo` is the host's, not the cgroup's, and `MemAvailable` is the only usable line

**Claim.** `/proc/meminfo` reports the **host's** memory to a containerised
process: `MemTotal`, `MemFree` and `MemAvailable` are unchanged by a cgroup
memory limit, and no line in that file reflects one. So a memory limit and free
memory are read from two different places that never agree, and free memory may
only be consulted once `RT1`–`RT6` have established that *no* limit binds.

Of the three lines, only `MemAvailable` is usable. `MemFree` excludes
reclaimable page cache, so on any machine that has read a large file it
under-reports drastically. `MemAvailable` is the kernel's own estimate of what
is obtainable without swapping.

**Proof.** Observed on this host, a 32 GiB machine, comparing the host with a
container given a 512 MiB limit:

| | `MemTotal` | `MemFree` | `MemAvailable` | `memory.max` |
|---|---|---|---|---|
| host | 32774304 kB | 2724748 kB | 20143720 kB | — |
| `nerdctl run -m 512m` | 32774304 kB | 2619956 kB | 20043708 kB | `536870912` |

The container's own limit is reported correctly by `/sys/fs/cgroup/memory.max`
and is invisible in `/proc/meminfo`; the two `MemFree` readings differ only by
ordinary drift between the two samples. The 7.4× gap between `MemFree` and
`MemAvailable` on an otherwise idle host is what rules `MemFree` out.

`lxcfs` and similar FUSE shims *can* overlay a cgroup-aware `/proc/meminfo`,
which is why the claim is about what the kernel provides rather than about what
is always mounted there — a shim makes the reading *more* conservative, never
less, so it does not break the use below.

**Scope limit.** This says nothing about how much memory pgdt may actually
obtain: `MemAvailable` is an estimate, it moves second to second, and two
processes reading it at once each see the whole of it. It is therefore usable
as a **ceiling** on what to plan for and never as a reservation or a target to
fill.

**Verified against:** Linux 7.1.4-arch1-1; nerdctl 2.3.5 / containerd v2.3.3;
`postgres:16`.

**Relied on by:** [`decisions.md`](decisions.md), "I/O, memory and parallelism" — the no-limit branch of
the budget default, which caps at half of `MemAvailable`, and the CLI's
statistics allowance (D85), which takes the same half where no limit is found.

**Re-verify:**

```sh
grep -E '^Mem(Total|Free|Available)' /proc/meminfo
sudo nerdctl run --rm -m 512m postgres:16 sh -c \
  'grep -E "^Mem(Total|Free|Available)" /proc/meminfo; cat /sys/fs/cgroup/memory.max'
```

---

## RT9 — cgroup v2 `memory.events` is the only thing that says a process was OOM-killed

**Claim.** `<cgroup>/memory.events` exists on every non-root v2 cgroup — a
container's own, whether or not it was given a limit — and carries a line
`oom_kill <n>`, a monotonic count of the processes the kernel's OOM killer
reaped in that cgroup. It is readable **from inside** the container by the
container's own processes, so it can be read after a command and before the
container is torn down.

A process reaped by the OOM killer dies by
`SIGKILL` and is indistinguishable, from its own exit status, from any other
signal death; where the harness runs the command under a wrapper that reports
its child's peak resident set, the wrapper exits with a *collapsed* status and
the distinction is gone entirely.

**Proof.** `Documentation/admin-guide/cgroup-v2.rst`, `memory.events`:
*"oom_kill — The number of processes belonging to this cgroup killed by any kind
of OOM killer."* Observed on this host, under `-m 64m --memory-swap 64m`: a
container whose command allocates past the limit reads `oom 1` / `oom_kill 1`,
one whose command merely exits non-zero reads `oom 0` / `oom_kill 0`, and both
containers exit **1** through the resident-set wrapper. An unlimited container
has the file and reads all zeros.

**Scope limit.** Four:

- **A missing file is not "nothing was killed".** On a v1 hierarchy, or where
  `/sys/fs/cgroup` is not the container's own, there is no counter and the
  answer is *unknown* — which has to stay distinguishable from zero, or an
  unreadable oracle silently becomes a clean bill of health.
- **`oom_kill` counts processes, `oom` counts events.** A cgroup that went OOM
  without reaping anything increments the second and not the first; what a
  caller asking "did my command die to the kernel" wants is the first.
- **It is hierarchical**, so a cgroup with descendants counts their kills too.
  Nothing here has descendants — a container's command tree is one cgroup — but
  a caller that acquired them would need `memory.events.local`.
- **It says the arrangement did not fit; it does not say `peak > limit` for any
  one process.** Clean page cache is *reclaimed* rather than killed for: a 3 GB
  file read through a 64 MiB cgroup hits the ceiling **13,798 times**
  (`memory.events`, `max`) and is never reaped, so a cgroup that does get a kill
  had a charge reclaim could not free. But the charge is the **cgroup's**, and
  the cgroup holds the wrapper, the shell and ~350 KB of slab alongside the one
  process a `ru_maxrss` fit measures — sub-MiB against the limits used here, and
  still not the same quantity. `memory.peak` is no escape: it reads exactly the
  limit in both cases, killed and survived, because any cgroup that touches its
  ceiling once reads `peak == limit` from then on.

**Verified against:** Linux 7.1.4-arch1-1; nerdctl 2.3.5 / containerd v2.3.3;
`alpine:3` and `postgres:16`.

**Relied on by:** [`measurements.md`](measurements.md), "The apparatus" — the
harness's kill oracle, which decides whether a killed leg is a censored reading
or an apparatus failure, and bars a figure from publication either way; and
`scripts/measure.py`, `_censored_constraint`, which states a killed leg as the
constraint it proves and takes its wording from the fourth scope limit above.

**Re-verify:**

```sh
docker run --rm -m 64m --memory-swap 64m alpine:3 sh -c \
  'tail /dev/zero >/dev/null 2>&1; cat /sys/fs/cgroup/memory.events'
docker run --rm -m 64m --memory-swap 64m alpine:3 sh -c \
  'false; cat /sys/fs/cgroup/memory.events'
docker run --rm alpine:3 cat /sys/fs/cgroup/memory.events
```

`oom_kill 1`, then `oom_kill 0`, then `oom_kill 0`.

For the fourth scope limit — clean page cache is reclaimed rather than killed
for, and `memory.peak` saturates either way:

```sh
docker run --rm -m 64m --memory-swap 64m alpine:3 sh -c \
  'dd if=/dev/zero of=/big bs=1M count=1024 2>/dev/null; sync; cat /big >/dev/null
   grep -E "^(max|oom|oom_kill) " /sys/fs/cgroup/memory.events
   cat /sys/fs/cgroup/memory.peak'
```

Exits **0** with `oom 0` / `oom_kill 0` and a `max` count in the thousands —
one per time reclaim was driven — and `memory.peak` equal to the limit,
67108864. The count scales with the bytes read and is not a fixed number: 9,249
here over 1 GiB, 13,798 over the 3 GB file the scope limit quotes.

---

## RT10 — glibc gives an allocating thread an arena up to a limit, and `M_ARENA_MAX` binds only before that limit latches

**Claim.** Under glibc's `malloc`, an allocation finding no free arena creates
one while the process's arena count is below a limit, and past it reuses an
existing one. The limit is `M_ARENA_MAX` if set; otherwise it is computed once,
the first time an arena is wanted with at least `arena_test` (8 on 64-bit)
already in existence — `8 × ncores` before glibc 2.44, `max(8, ncores)` from
2.44 on. **Once computed it is latched for the life of the process**, so a
`mallopt(M_ARENA_MAX, …)` after that changes nothing; one made before it bounds
every arena created afterwards and removes none that exist. There is no getter.

**Proof.** `malloc/arena.c`, `arena_get2`, at `glibc-2.44`:
`static size_t narenas_limit;` then `if (narenas_limit == 0)` →
`if (mp_.arena_max != 0) narenas_limit = mp_.arena_max;` `else if (narenas >=
mp_.arena_test)` → `narenas_limit = __get_nprocs ();` raised to `arena_test`.
Before `glibc-2.44` the same branch reads `NARENAS_FROM_NCORES (n)`; the change
is commit `93e6135`, "malloc: Reduce maximum arenas". `malloc/malloc.c`,
`do_set_arena_max`, writes `mp_.arena_max` and nothing else, which is also what
`MALLOC_ARENA_MAX` sets at startup. Observed, not proved: the `reserve` figure's
instrument legs read the readers plus one to three arenas from 1 to 24 readers,
under the `postgres:16` image's glibc — below its ceiling of 192 on this host.

**Scope limit.** glibc only: `jemalloc` and `mimalloc` (D13) have no such
arenas. `ncores` is a CPU count, which a CFS quota such as `--cpus` does not
lower. Because `mallopt` and `MALLOC_ARENA_MAX` write the same field, a binary
that calls it overrides the operator's setting unless it reads the variable
first. How much a retained arena holds is not part of the claim.

**Verified against:** glibc 2.44+r24 (Arch, the host; source read); glibc 2.41
(`postgres:16`, the figures' image; observed through the instrument).

**Relied on by:** [`decisions.md`](decisions.md), "D13" — the in-binary cap's
refusal, which is not mechanical because under D12's `current_thread` runtime
fewer than `arena_test` arenas exist before the arrangement resolves;
`io.rs`, `MEMORY_RESERVE`'s rejected sizing from the arena count; and
[`measurements.md`](measurements.md), "The apparatus" — the ceiling standing
behind every figure.

**Re-verify:**

```sh
getconf GNU_LIBC_VERSION                                   # the host's
docker run --rm "$(cd scripts && uv run python -c 'import measure; print(measure.Config().image)')" \
  getconf GNU_LIBC_VERSION                                 # the figures', as pinned
curl -sfL 'https://sourceware.org/git/?p=glibc.git;a=blob_plain;f=malloc/arena.c;hb=glibc-2.44' \
  | grep -n -A16 'static size_t narenas_limit'
```

Substitute the version either command reports for `glibc-2.44`: a branch reading
`__get_nprocs ()` is `max(8, ncores)`, one reading `NARENAS_FROM_NCORES` is
`8 × ncores`. No built instrument reads the arena count a `system` build's
scan reaches: the `introspect` build's `malloc_heaps` counts the arenas C's
allocations open, its Rust heap being mimalloc's (`pgdt/src/introspect.rs`).

## RT11 — `std`'s `HashMap` allocates one table sized by its capacity, and a full map grows into a table twice the buckets

**Claim.** A `std::collections::HashMap<K, V>` of nonzero `capacity()` holds one
allocation of `buckets × size_of::<(K, V)>()`, rounded up to
`max(align_of::<(K, V)>(), W)`, plus `buckets + W` control bytes, where `W` is
16 on x86 and x86-64 and 8 elsewhere and `buckets` is `capacity + 1` below a
capacity of 8 and `capacity × 8 / 7` from there. An insert into a map whose
length equals its capacity allocates the table sized for one more entry before
it frees the old one, so the two are live together for the rehash. A map made
`with_capacity(k)`, `k` nonzero, allocates that same table — the one a full map
of `k − 1` grows into. An empty map allocates nothing.

**Proof.** `std` re-exports the `hashbrown` crate, whose version is in
`library/Cargo.lock` under the toolchain's `rust-src`. `hashbrown` 0.17.1,
`src/raw.rs`: `bucket_mask_to_capacity`, `capacity_to_buckets` (the small-table
arm through `min_cap`) and `TableLayout::calculate_layout_for`, whose
`ctrl_offset` is the rounded slot bytes and whose `len` adds `buckets +
Group::WIDTH`; `reserve_rehash_inner` resizes to
`max(new_items, full_capacity + 1)` and frees the old allocation after moving
the entries; `with_capacity` sizes its table by the same `capacity_to_buckets`.

**Scope limit.** Nothing is claimed of a map that has had entries removed,
whose capacity no longer names its buckets — which is why a dictionary's
interning map is rebuilt rather than pruned — nor of any allocator's own
rounding: the sizes are what `GlobalAlloc` is asked for.

**Verified against:** Rust 1.98.0, `hashbrown` 0.17.1 (source read; observed
through the instrument build, `pgdt/tests/statistics_account.rs`).

**Relied on by:** [`decisions.md`](decisions.md), "D81" — the interned term of
the statistics account, `gather::map_heap` and `gather::grown_map_heap`.

**Re-verify:**

```sh
grep -A2 'name = "hashbrown"' "$(rustc --print sysroot)/lib/rustlib/src/rust/library/Cargo.lock"
cargo test -p pgdump_query --lib a_full_map_grows_into_the_table_it_is_charged
cargo test -p pgdump_query --lib a_map_made_to_hold_its_entries_allocates_the_table_it_is_charged
cargo test -p pgdt --features introspect --test statistics_account --target-dir <own>
```

The first names the version whose `src/raw.rs` the proof reads; the second and
third hold the capacities to the growth rule and to `with_capacity`; the last
holds the bytes to what the allocator was asked for.

## RT12 — `Vec::reserve_exact` leaves the capacity it was asked for, grown by one `realloc`

**Claim.** On a `Vec<T>` of nonzero-sized `T` whose capacity is below
`len + additional`, `reserve_exact(additional)` leaves `capacity()` exactly
`len + additional`, reached by one `realloc` of the old allocation to the new
size — one allocation of it where the vector held none. It never rounds the
capacity up to what the allocator returned.

**Proof.** `library/alloc/src/raw_vec/mod.rs`: `grow_exact` computes
`cap = len + additional`, `finish_grow` asks the allocator to `grow` the current
memory to `layout_array(cap)` (or to `allocate` it when there is none), and
`set_ptr_and_cap` stores `cap` itself, not the returned slice's length.
`library/alloc/src/alloc.rs`: `Global::grow` reaches `realloc` for a nonzero old
size.

**Scope limit.** Nothing is claimed of `reserve`, `push` or `extend` growing a
vector by themselves, whose amortized capacities are the toolchain's choice, nor
of any allocator's own rounding: the sizes are what `GlobalAlloc` is asked for.

**Verified against:** Rust 1.98.0 (source read; observed through the instrument
build, `pgdt/tests/statistics_account.rs`).

**Relied on by:** [`decisions.md`](decisions.md), "D81" — a vector charged
ahead of its growth, `gather::reserve_charged` and `gather::push_charged`.

**Re-verify:**

```sh
grep -n -A20 'fn grow_exact' "$(rustc --print sysroot)/lib/rustlib/src/rust/library/alloc/src/raw_vec/mod.rs"
cargo test -p pgdump_query --lib a_vector_grows_into_the_capacity_it_is_charged
cargo test -p pgdt --features introspect --test statistics_account --target-dir <own>
```

The first shows the capacity stored; the second holds the capacities to the
charge; the third holds the bytes to what the allocator was asked for.

## RT13 — a ranged GET's 206 carries the object's whole size, and `object_store` reports that rather than the slice's

**Claim.** `object_store` 0.14.2's HTTP backend, given `GetOptions::range`,
issues a `Range` request and requires a `206`; from that response's
`Content-Range` it takes the total after the `/` and **overwrites**
`ObjectMeta::size` with it, so the metadata returned beside a few bytes
describes the whole object. `ETag` and `Last-Modified` come back on the same
response. One ranged GET therefore answers stored size, weak identity and
leading bytes together.

**Proof.** `object_store` 0.14.2, `src/client/get.rs`: `get_range_meta` parses
`CONTENT_RANGE` into `ContentRange { range, size }` and assigns
`meta.size = value.size` under the comment "Update size to reflect the full
size of the object (#5272)"; `header_meta` reads the validators from the same
`response.headers`.

**Scope limit.** Nothing is claimed for an unranged GET, for a server that
answers `200` to a `Range` (which is `RT14`), or for any backend but this one.

**Verified against:** `object_store` 0.14.2 (source read; observed against the
oracle, `pgdt/tests/remote.rs`).

**Relied on by:** `io::RemoteObject::probe` ([`decisions.md`](decisions.md),
"D14") — the origin probe costs one round trip rather than a `HEAD` and a
`GET`.

**Re-verify:**

```sh
cargo test -p pgdt --test remote the_origin_probe_costs_one_round_trip
```

The assertion is on the request log, so a crate that started asking twice fails
here rather than merely getting slower.

## RT14 — a server that ignores `Range` is refused on the status line, before its body is read

**Claim.** Where a range was asked for and the response is not `206`,
`object_store` 0.14.2's HTTP backend raises `Error::NotSupported` carrying
`RangeNotSupported` **from the response's status line**, with the body left
undrained. There is no fallback to fetching the whole object and slicing it.

**Proof.** `object_store` 0.14.2, `src/http/client.rs`, `GetClient::get_request`
for the HTTP store: `if has_range && res.status() != StatusCode::PARTIAL_CONTENT`
returns `crate::Error::NotSupported` before `res` is consumed.

**Scope limit.** It is the status check that is claimed, not what any
particular server does. A server answering `206` with the wrong span is a
different case, and is `RT15`.

**Verified against:** `object_store` 0.14.2 (source read; observed against the
oracle's `ignoring_range` knob, `pgdt/tests/remote.rs`).

**Relied on by:** `io::RemoteSource` ([`decisions.md`](decisions.md), "D6") —
positioned reads are simply unavailable against such a server, and asking for
one does not download the object to find that out.

**Re-verify:**

```sh
cargo test -p pgdt --test remote a_range_ignoring_server_is_refused_rather_than_read_whole
```

## RT15 — a ranged GET delivers exactly the span asked for, or fails

**Claim.** `object_store` 0.14.2's HTTP backend compares the `Content-Range`
it received against the range it asked for — clamped to the object's size, so
a request running past the end yields the remainder rather than an error — and
raises `UnexpectedRange` on any other difference. `Content-Length` is required
unconditionally, and a body that ends before that length raises a transport
error rather than returning short. So a `get_opts` that returns `Ok` returns
every byte of the span, and a caller comparing the length it got against the
length it asked for is checking the *end of the object*, not the transport.

**Proof.** `object_store` 0.14.2, `src/client/get.rs`: `get_range_meta` calls
`GetRange::as_range(meta.size)` — `src/util.rs`, whose `Bounded` arm returns
`r.start..len` where `r.end > len` — and returns
`GetResultError::UnexpectedRange` where the actual range differs;
`src/client/mod.rs` disables response compression and the crate's own
`HeaderConfig` leaves `Content-Length` required, so a short body surfaces as an
error from the body stream.

**Scope limit.** Nothing is claimed about *which* servers do this; both
conditions are produced deliberately by the oracle
(`pgdt/tests/common/oracle.rs`), whose `short_range_after` and
`truncating_body_after` are the two halves.

**Verified against:** `object_store` 0.14.2 (source read; observed against both
oracle knobs, `pgdt/tests/remote.rs`).

**Relied on by:** `io::RemoteSource::read_range` ([`decisions.md`](decisions.md),
"D6") — it answers exactly its `len` or errors, as `ByteRangeSource` requires
of every source.

**Re-verify:**

```sh
cargo test -p pgdt --test remote a_short_206_is_a_fault_rather_than_a_short_read
cargo test -p pgdt --test remote a_body_that_stops_short_of_its_declared_length_is_a_failure
```

## RT16 — an absent `Last-Modified` is reported as the Unix epoch, not as absence

**Claim.** `ObjectMeta::last_modified` is a `DateTime<Utc>` rather than an
`Option`, and `object_store` 0.14.2 substitutes the Unix epoch where a response
carries no `Last-Modified` header. `ETag` is honestly `Option<String>`. So the
weak identity is asymmetric: one half can say "the server sent none" and the
other cannot.

**Proof.** `object_store` 0.14.2, `src/client/header.rs`: `header_meta` takes
`last_modified` from the header where `HeaderConfig::last_modified_required` is
false and falls back to `DateTime::default()`, which is the epoch; the HTTP
store sets that flag false in `src/http/client.rs`'s `HEADER_CONFIG`.

**Scope limit.** The substitution is claimed, not that no real server sends a
1970 date. A dump stamped at the epoch is indistinguishable from one with no
stamp at all, which is why the reading below is a deliberate choice rather than
a decoding.

**Verified against:** `object_store` 0.14.2 (source read; observed against the
oracle's `without_last_modified` knob).

**Relied on by:** `io::weak_identity` ([`decisions.md`](decisions.md), "D21") —
it reads the epoch as absence, so a server that says nothing is treated as
silent rather than as claiming a date; `RemoteObject::precondition` pins no read
by it, and a run over a server stating no strong tag either is refused unless
`none` (`RemoteObject::unpinned`).

**Re-verify:**

```sh
cargo test -p pgdt --test remote a_server_sending_no_modification_time_reads_as_silence
```

## RT17 — the HTTP backend runs on a `current_thread` runtime

**Claim.** Nothing in `object_store` 0.14.2's HTTP path, or in the `reqwest`
and `hyper` tree beneath it, enables `tokio/rt-multi-thread` or calls
`tokio::spawn` on the request path: a ranged GET completes on a
`current_thread` runtime. It does need tokio's IO and time drivers, which a
binary using it declares for itself.

**Proof.** `object_store` 0.14.2's `Cargo.toml` names `tokio` with no
`rt-multi-thread`, and its `tokio` feature adds only `dep:tokio` and
`dep:tracing`; `src/client/retry.rs` and `src/client/get.rs` await
`tokio::time::sleep` and the response body directly, with no spawn. Observed:
every async assertion in `pgdt/tests/remote.rs` runs under `#[tokio::test]`, whose
flavour is `current_thread`.

**Scope limit.** Multipart upload and the `list` path are not claimed; this
project calls neither.

**Verified against:** `object_store` 0.14.2, `reqwest` 0.13.5, `hyper` 1.11.1
(source and lockfile read; observed through the test suite).

**Relied on by:** [`decisions.md`](decisions.md), "D12" — `pgdt` runs one
`current_thread` runtime and would seed one glibc arena per visible CPU if a
dependency forced the multi-threaded flavour on it.

**Re-verify:**

```sh
cargo tree -p pgdt -e features -i tokio | grep -c rt-multi-thread
cargo test -p pgdt --test remote
```

The first must print `0` for the shipped feature set; the second exercises the
path on a `current_thread` runtime.

## RT18 — a `GetOptions` precondition is sent, and a 412 comes back as a terminal `Precondition`

**Claim.** `object_store` 0.14.2's HTTP backend sends `GetOptions::if_match` as
`If-Match` and `GetOptions::if_unmodified_since` as `If-Unmodified-Since` on
the ranged GET it makes, and a `412 Precondition Failed` arrives as
`Error::Precondition` — **not retried**, so it costs one round trip rather than
the retry schedule. `if_match` takes precedence over `if_unmodified_since`
where both are set.

**Proof.** `object_store` 0.14.2, `src/client/mod.rs`'s `GetBuilder::send`
writes both headers from the options; `src/client/retry.rs` maps
`StatusCode::PRECONDITION_FAILED` to `crate::Error::Precondition` in the
terminal-error conversion rather than in the retry arm. `check_preconditions`
in `src/lib.rs` documents the same precedence for the stores that evaluate it
client-side.

**Scope limit.** The *sending* is claimed, and the mapping of the status the
server returns. Whether a given server honours a conditional header is the
server's business — a store that ignores `If-Match` answers 200 and this
project cannot tell.

**Verified against:** `object_store` 0.14.2 (source read; observed against the
oracle's `etag_changing_after` and `without_etag` knobs).

**Relied on by:**
[`decisions.md`](decisions.md), "D21" —
the in-flight identity check on a remote source *is* this precondition, so a
header that was not sent would leave a remote run with no such check at all,
silently.

**Re-verify:**

```sh
cargo test -p pgdt --test remote every_ranged_get_after_the_probe_pins_the_object
cargo test -p pgdt --test remote an_object_rewritten_under_a_read_is_refused_by_the_server
```

## RT19 — a PID namespace's init is not killed by `SIGINT` or `SIGTERM` under the default action, whoever sends it

**Claim.** A process that is its PID namespace's init — PID 1 in a container,
as `exec` in the container's command makes `pgdt` — discards any signal but
`SIGKILL` and `SIGSTOP` whose disposition is `SIG_DFL`, **whoever sends it**:
`raise` from itself, a process in its namespace, and the host (`docker stop`,
a terminal's Ctrl-C) alike. `SIGINT` and `SIGTERM` do not end it; a handler it
installs still runs. It is init exactly when `getpid()` returns 1.

**Proof.** Linux v7.1, `kernel/fork.c`, `copy_process`: `is_child_reaper(pid)`
— number 1 at the new task's own namespace level, the number `getpid()`
returns — sets `p->signal->flags |= SIGNAL_UNKILLABLE`; `kernel/signal.c`,
`sig_task_ignored`, returns true for `SIGNAL_UNKILLABLE && handler == SIG_DFL
&& !(force && sig_kernel_only(sig))`, `force` being set only for a sender in
an ancestor namespace and `sig_kernel_only` true only of `SIGKILL`/`SIGSTOP`.
Observed: `pgdt parse` as PID 1, whose re-raise was discarded and whose
`signal-hook` fallback then `abort`ed, exited 139; a host-sent `SIGTERM` or
`SIGINT` to a default-action init left it to exit 7.

**Scope limit.** `SIGKILL` and `SIGSTOP` from an ancestor namespace do end it
— `docker stop`'s timeout is one. A fault's signal (`SIGILL`, `SIGTRAP`,
`SIGBUS`, `SIGFPE`, `SIGSEGV`, `SIGSYS`) is forced, and under `SIG_DFL` does end
init: `force_sig_info_to_task` clears `SIGNAL_UNKILLABLE` whenever the
disposition it delivers under is the default. That is how the observed 139
came about. The same signal *sent* to init is discarded like any other.

**Verified against:** Linux 7.1.4 (source read; observed under nerdctl with
`debian:stable-slim`, and under `unshare -Urpf` signalled from the host).

**Relied on by:** [`decisions.md`](decisions.md), "D26" — as its
namespace's init, each binary handles every signal that ends it elsewhere by
exiting `128 + n` but a fault's, which it leaves to the forced default
(`namespace-init/src/lib.rs`), and an interrupted `parse` exits `128 + n`
rather than re-raising. `pgdt/tests/namespace_init.rs` and
`datafusion-cli-pgdump/tests/namespace_init.rs` pin both under `unshare`.

**Re-verify:**

```sh
docker run --rm debian:stable-slim sh -c 'exec perl -e "kill INT => \$\$; sleep 1; exit 7"'; echo $?
docker run --rm debian:stable-slim sh -c 'perl -e "kill INT => \$\$; sleep 1; exit 7"; echo $?'
unshare -Urpf perl -e 'sleep 2; exit 7' & u=$!; sleep 0.5; kill -TERM $(cat /proc/$u/task/*/children); wait $u; echo $?
```

The first must print `7` (PID 1 survives its own `SIGINT`), the second `130`,
the third `7` (PID 1 survives the host's `SIGTERM`; run it from a script, an
interactive shell's job control aside).

## RT20 — bash runs a script on past a child that caught `SIGINT`, and stops past one `SIGINT` killed

**Claim.** When bash receives `SIGINT` while waiting on a foreground child —
Ctrl-C at a terminal, delivered to the whole process group — it acts on the
signal only if the child **died of** `SIGINT`; a child that exits, even with
`130`, is taken to have handled it, and the script continues with its next
command.

**Proof.** bash 5.3, `jobs.c`, `waitchld`: `child_caught_sigint` is set when
`wait_sigint_received && (WIFSIGNALED (status) == 0 || WTERMSIG (status) !=
SIGINT)` and cleared only for a child with `WTERMSIG (status) == SIGINT`;
`wait_sigint_cleanup` then kills the shell with `SIGINT` only where it is
clear. Observed, below.

**Scope limit.** Other shells are not claimed; this is bash's documented
"wait and cooperative exit". A script that traps `SIGINT` itself is not
claimed either.

**Verified against:** bash 5.3.15 (source read; observed on this machine).

**Relied on by:** [`decisions.md`](decisions.md), "D26" — `parse` re-raises
rather than exiting `128 + n`, so `pgdt parse …; pgdt query …` stops at the
Ctrl-C instead of querying a partial cache.

**Re-verify:**

```sh
python3 - <<'PY'
import os, signal, subprocess, time
for child in ['trap "exit 130" INT; sleep 5', 'sleep 5']:
    p = subprocess.Popen(['bash', '-c', f"sh -c '{child}'; echo continued"],
                         process_group=0, stdout=subprocess.PIPE, text=True)
    time.sleep(1); os.killpg(p.pid, signal.SIGINT)
    print(repr(child), '->', p.communicate()[0].strip() or 'stopped')
PY
```

The trapping child must print `continued`, the killed one `stopped`.

## RT21 — `arrow-cast` formats a `date` or timestamp only to `262142-12-31`

**Claim.** `arrow-cast`'s display, and its cast to `Utf8`, of a `Date32` or
`Timestamp` value after `262142-12-31 23:59:59.999999` fails, though the
value is a valid `i32` or `i64`; every value up to it formats.

**Proof.** `chrono` 0.4.45, `src/naive/date/mod.rs`: `MAX_YEAR = (i32::MAX >>
13) - 1`, the year packed into `NaiveDate`'s high 19 bits, and `NaiveDate::MAX`
its last day; `arrow-cast` converts through `chrono` to format. Observed by
`every_extreme_is_held_by_arrow_or_recorded`
(`datafusion-pgdump/tests/unrepresentable.rs`), `t_extremes` ids 14 and 15.

**Scope limit.** The upper end only: `chrono`'s lower end lies far below
PostgreSQL's 4713 BC. Other Arrow consumers are not claimed.

**Verified against:** `chrono` 0.4.45, `arrow-cast` 59.2.0.

**Relied on by:** [`decisions.md`](decisions.md), "D98" — the provider's
tier of unrepresentable values; and [`decisions.md`](decisions.md), "D96" — the count's engine tier, read off
`chrono::NaiveDate::MAX` by `pgdump_query::calendar_end`, which a cache
records and a build whose calendar ends elsewhere refuses where it holds a count.

**Re-verify:**

```sh
cargo nextest run -p datafusion-pgdump --test unrepresentable every_extreme_is_held_by_arrow_or_recorded
grep -n 'MAX_YEAR: i32' ~/.cargo/registry/src/*/chrono-$(cargo tree -i chrono -e normal --depth 0 | awk '{print substr($2,2)}')/src/naive/date/mod.rs
```

The test passes only while ids 14 and 15 sit either side of the bound.

## RT22 — A physical plan reports every expression it evaluates, and a rule appended last sees the plan that runs

**Claim.** `SessionState::create_physical_plan` hands the plan its
`QueryPlanner` builds to the session's physical optimizer rules in order and
runs what the last returns, a rule a `SessionStateBuilder` appends running
after every default one; every built-in `ExecutionPlan` reports through
`apply_expressions` each expression it evaluates, and a scalar subquery's
plan is a child of `ScalarSubqueryExec`. And a conjunct of a `WHERE` a
`TableProvider` answers `Exact` is moved into its `TableScan`'s `filters` and
left in no `Filter` above it, so no node DataFusion plans evaluates it.

**Proof.** DataFusion 55.1.0: `datafusion/core/src/physical_planner.rs`,
`DefaultPhysicalPlanner::create_physical_plan` then `optimize_physical_plan`,
looping over `session_state.physical_optimizers()`;
`datafusion/core/src/execution/session_state.rs`, `SessionStateBuilder::build`,
pushing `physical_optimizer_rules` after the defaults;
`datafusion/physical-plan/src/execution_plan.rs`, `apply_expressions`, a
required method; `datafusion/physical-plan/src/scalar_subquery.rs`,
`ScalarSubqueryExec::children`; `datafusion/optimizer/src/push_down_filter.rs`,
the `TableScan` arm, which keeps above the scan only the conjuncts answered
`Unsupported` or `Inexact`, and the volatile and subquery-holding ones.
Observed by `the_unrepresentable_function_finds_what_the_null_mode_nulls` and
`the_unrepresentable_function_refuses_where_datafusion_would_evaluate_it`
(`datafusion-pgdump/tests/unrepresentable.rs`), the first planning under the
guard every use a scan answers and the second refusing every other, a scalar
subquery's included.

**Scope limit.** Plans reaching physical planning through the session's own
`create_physical_plan`, which `DataFrame` and SQL use, and built-in nodes; a
caller driving a `PhysicalPlanner` by hand, a rule appended after the guard,
and an embedder's own `ExecutionPlan` reporting fewer expressions than it
evaluates are not claimed.

**Verified against:** DataFusion 55.1.0.

**Relied on by:** [`decisions.md`](decisions.md), "D101" — `pgdump_unrepresentable`
refused at planning by a physical optimizer rule the embedder appends, which
walks every node's expressions and finds the function in none a scan answers.

**Re-verify:**

```sh
cargo nextest run -p datafusion-pgdump --test unrepresentable unrepresentable_function
```

Both tests pass only while the function is planned where a scan answers it
and refused wherever another node holds it.
