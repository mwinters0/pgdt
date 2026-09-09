# P19.1 — the runtime-invariants register

[`runtime-invariants.md`](runtime-invariants.md) exists, holding `RT1`–`RT7`,
with a read-trigger in `CLAUDE.md` beside the Postgres one and a row in
`README.md`'s doc list. No library code. This is the ordering the spec binds
first — *"the runtime-invariants register before anything reads
`/sys/fs/cgroup`"* — so what follows is what `19.7` inherits.

## What `discover_memory_limit` is now specified down to

The register is deliberately written as a parse contract, not as background.
Seven behaviours in it are the whole of what the reader must do, and each has an
entry to cite when the code looks arbitrary:

- **Select the v2 line by its shape** — `0::`, an empty controller field — never
  by position and never by the hierarchy id (`RT1`). Both were observed to move:
  the v2 line came *second* on a hybrid machine, and the scratch v1 hierarchy's
  id came back `1` on one run and `2` on the next, `idr_alloc_cyclic` being
  cyclic.
- **Strip a trailing ` (deleted)`** before joining the path (`RT1`).
- **Split the controller field on `,` and compare elements** (`RT6`). A
  substring test is wrong in a way that only shows up on a machine that has a
  *named* v1 hierarchy, and `name=memory` is a legal name.
- **Walk upward bounded by the mount point**, not by counting separators
  (`RT5`). Inside a cgroup namespace the mount *is* the namespace root, so the
  bound and the truncation are the same line of code.
- **A missing file means no limit at this level** (`RT5`). Two distinct causes
  produce it — the root cgroup has no `memory.*` files at all, and a child whose
  parent never enabled `+memory` in `cgroup.subtree_control` has none either —
  and neither is an error.
- **`max` is the v2 sentinel; v1 has none** (`RT2`, `RT4`). The v1 "unlimited"
  value is `PAGE_COUNTER_MAX × PAGE_SIZE`, which varies with page size and word
  width, so it is read as a **threshold**, never compared to a constant.
- **Minimise across both files at every level** (`RT3`, `RT5`). `memory.high`
  may be lower than `memory.max`, and either may be lowest several levels up.

## The one entry that is weaker than the others, and what it costs 19.7

`RT4` (v1 `memory.limit_in_bytes`) is proved from kernel source and **not
observed**. It cannot be observed here: the memory controller lives in exactly
one hierarchy (`RT6`), this machine is unified v2, and producing a v1 memory
controller means rebooting with `systemd.unified_cgroup_hierarchy=0`. The
register says so in its `Verified against` rather than implying a run happened.

The consequence is for `19.9`'s resolution tests rather than for `19.7`'s
design: **the reader wants a filesystem root it can be pointed at**, so that a
fixture tree can stand in for `/proc` and `/sys/fs/cgroup`. Without that seam
the v1 arm is untestable on any machine this project has, and the tests degrade
to exercising whichever hierarchy the host happens to run. The seam is cheap if
it is taken at the start and awkward to retrofit, which is why it is written
here and not discovered in `19.9`.

## The CPU side is closed, with a number behind it

`RT7` is not a promise about `std`; it is a reading. On this 24-CPU host,
`--cpus=2` gives `Ok(2)` and `--cpus=3.5` gives `Ok(3)` while `nproc` — which
consults only the affinity mask — answers 24 in both. So the spec's *"the CPU
side needs no work"* is now evidence rather than inference, and `19.8`'s `.xz`
override may call `available_parallelism()` directly. The rounding is **down**,
by integer division of `limit / period`, which matters for the clamp
arithmetic: a 3.5-CPU allocation buys three workers, not four.

## Where the evidence came from

The kernel is **not** checked out under `/mnt/wd12t/upstream/`, and this slice
did not add one. The quoted sources were read at
`https://raw.githubusercontent.com/torvalds/linux/v7.1/<path>` — `v7.1` matching
this machine's kernel line.

**A citation here is the path plus the tag, and the register's preamble now says
so**, along with why no worktree is kept: this register's walk is seven runs
rather than 47 greps, so it never needs a tree the way its Postgres sibling
does. The cost argument this slice originally gave for that — that a Linux clone
is hour-scale for six files — is only true of a *full* clone; `--depth 1
--filter=blob:none` is minutes. It is not what the answer rests on. Reasoning:
[2026-09-09](../status/history/2026-09-09.md), "The runtime register cites by
path and tag".

Every `Re-verify` block was run as written before the entry was filed; the
`+memory` line in `RT5`'s scratch hierarchy is there because the block failed
without it.

## The sigil

`RT<n>`, not `R<n>`. `R<n>` is already the `xz-seek` crate's requirements
register, and two phase inboxes in this tree cite it by number — so a bare `R3`
would be ambiguous between two documents that a session might reasonably be
holding at once. `docs/process.md`'s two-letters-rather-than-contend argument
under "Known deficiencies" applies verbatim; the reasoning is in the register's
own preamble so it is met where it is used.
