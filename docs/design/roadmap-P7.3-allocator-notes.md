# P7.3 — The allocator

What the rest of P7 inherits: an answer (the platform allocator stays), a
figure that re-takes it in five minutes, and one attribution that says *when*
to re-take it. The answer and its reasoning are filed by subject —
[`architecture.md`](architecture.md), "The allocator is the binary's choice",
and [`measurements.md`](measurements.md), "Which allocator a figure was taken
under" — and this doc holds the apparatus and what the numbers are not.

The library is untouched. What changed in shipped code is one new CLI module
and a `--version` string.

## Module map

| File | What it is |
|---|---|
| `pgdump_query-cli/src/alloc.rs` | the whole mechanism: two optional features, one `#[global_allocator]` behind each, `compile_error!` if both, and `VERSION` |
| `pgdump_query-cli/Cargo.toml`, `[features]` | `default = []`; `jemalloc` → `tikv-jemallocator` 0.6, `mimalloc` → `mimalloc` 0.1 |
| `pgdump_query-cli/src/main.rs` | `mod alloc;` and `version = alloc::VERSION` on the clap command — the only user-visible change |
| `scripts/measure.py`, `ALLOCATOR_LEGS` / `ensure_allocator_binary` / `binary_allocator` | the legs, their builds, and the interrogation |
| `scripts/measure.py`, `run_allocator` / `_ALLOCATOR_SHAPES` | the figure |
| `scripts/measure.py`, `_census_specs` | extracted, because two figures now borrow the census-on spec |
| `scripts/test_measure.py`, `Allocator` | eighteen assertions, each a way to publish a plausible table of the wrong comparison |

## The answer, and the two ways it could have been read wrong

`jemalloc` **1.87× / 1.19× / 1.07×** and `mimalloc` **0.98× / 0.96× / 0.96×**
on `parse`, a `strings` query and a typed one. The lever table's stake was a
factor — 1.8–2.4× between two stock libcs on everything that moves real bytes —
and what is actually on the table is 4%, in one direction, on two of three
shapes. The platform allocator stays; the rejected-alternative paragraph for
adopting `mimalloc` anyway is beside the mechanism.

**mimalloc's 4% is a real reading, not noise, and saying otherwise would have
been the easy mistake.** Its `typed` spread (9.75–9.79) does not overlap the
reference's (10.14–10.44), the sign reproduced in two independent sittings the
same day, and the one written floor that could have dismissed it — 7.2's "two
builds of one source can differ by ~10% from code layout alone" — was measured
on `insert_run` and explicitly *not* on these three shapes, where the alignment
flag moved nothing. So the case against adopting has to be about what 4% is
worth, and it is: the flip is one line and the cost is that every other table
in `measurements.md` becomes a figure of a binary nobody ships, with no
mechanical oracle, until the wrap's sweep pair.

**The other misreading would have been to treat jemalloc's `parse` number as
an allocator result.** It is not: it is 3,161 `madvise` calls against glibc's
50, all of the difference is system time, and it is `read_range`'s per-chunk
`vec![0u8; 1 MiB]` being handed back to the kernel once per chunk. See below.

## What 7.13 owes, and why this figure is not settled

**7.13 must re-take `--figure allocator`.** The ranking's largest number is an
artifact of the allocation 7.13 removes, so the post-7.13 ranking is a
different question and is the one that should decide adoption. Five minutes,
one figure, no sweep.

The evidence, all on the host and none of it a figure — the same class as 7.2's
`perf stat` and `strace` attributions:

| Warm host `parse`, 3.00 GiB control | `system` | `jemalloc` | `mimalloc` |
|---|---|---|---|
| user | 0.23 s | 0.19 s | 0.20 s |
| system | 0.27 s | **0.80 s** | 0.29 s |
| wall | 0.505 s | 0.981 s | 0.490 s |
| `instructions:u` | 1.882 G | 1.712 G | 1.891 G |
| `madvise` calls | 50 | **3,161** | 62 |
| `munmap` / `mmap` | 50 / 96 | 52 / 160 | 47 / 96 |

3,072 chunks of 1 MiB make up the file, which is where 3,161 comes from. Two
things follow that a later slice should not have to re-derive: the penalty is
*not* in user space, so a sampling profile at `perf_event_paranoid = 2` cannot
see it at all — the flat profiles of the two binaries are close to
indistinguishable; and glibc's advantage here is that its arena keeps the 1 MiB
region, so `vec![0u8; …]`'s `calloc` gets pages the kernel has already zeroed
and never memsets them.

## The reference column is the shipped binary, and that is the design

Three properties, and the middle one is the one a later figure should copy:

- **Not a fourth build of the same source.** Two builds of one source can
  differ ~10% by layout, which is larger than the effect. So the reference leg
  is `target/release/pgdq` — the binary every published figure was taken with —
  and the ratios are ratios against the doc.
- **Its readings are borrowed, not retaken**, from `census-brace-free` (parse)
  and `nested-end-to-end` (both `query` shapes), exactly as the warm throughput
  table's `COPY` row is borrowed. Run alone the figure measures them itself and
  says so in a "Partial sweep" note. This is why `_census_specs` was extracted:
  a borrow is a dictionary lookup that silently returns nothing when a key
  drifts, and two figures now depend on that key.
- **The reference's *name* is read off the binary**, so adopting a leg makes it
  the reference with no code change. `--no-default-features` on each leg's build
  is what keeps that true: without it, the `system` leg would follow the default
  and the figure would stop being re-takeable at the moment it mattered.

Each leg's build gets **its own `--target-dir`** (under scratch, per
`CLAUDE.local.md`; the binaries land in `runs/`). Not tidiness: a `--features`
build writes `target/release/pgdq`, so building a leg in the default directory
would silently replace `bin_pgdq` and every other figure in the same sweep
would be timed under the wrong allocator.

## What the landing commit owes

**An `acknowledged.py` entry, as a follow-up.** An entry cannot name its own
sha, so this lands the way `7.1`'s did. Six figures read stale on
`pgdump_query-cli/src/` — `nested-end-to-end`, `census-attribution`,
`cross-file-floor`, `map-only`, `projection-widths`, and `allocator` itself —
and the evidence is unusually strong for once: **measured, not reachability.**
This figure's reference column is a fresh reading of the three headline shapes
*on the changed binary*, and it reproduces the `ba2fc12` sweep to 2.5%, 0.7%
and 1.4% — inside the ~8% a warm absolute resolves to across sessions. The
change itself is `mod alloc;` plus a clap attribute, with no code on any timed
path.

**A new figure declaring an already-excused path has to be named in that
excuse.** `allocator` declares `NESTED`, so `7545dc6`'s comment-only `batch.rs`
retarget marked it stale — for a figure taken *after* that commit. The entry's
figure list gained `allocator`; nothing else about it changed. Worth knowing
because it is invisible: adding a figure can silently un-excuse a commit
somebody already read.

`session-drift` stays red for `scripts/measure.py`, which is a real new figure
function rather than an unreachable subcommand; the reachability oracle does
not stretch that far, and it is the wrap sweep's to clear.

## `QUERY_CLI` is now the directory

`measurements.md` already carried the rule — a declared path is matched by
prefix, so whatever splits the CLI widens `QUERY_CLI` to the directory in the
same change — and it had already been broken once by `where_expr.rs`. Adding a
third module is what made it two, so the declaration is now
`pgdump_query-cli/src/`. The consequence is that the query figures now go stale
on any CLI module rather than on `main.rs` alone, which is the honest state and
is why five of the six stale figures above are stale.

## Deliberately not done

- **No adoption, and no re-scoping of the slice to defer one.** The row asked
  for the measurement and for adoption *if it wins*; nothing clears the bar
  now, which the spec calls a delivered row. The reversal is one line and is
  filed under `STATUS.md`'s "Decisions worth another look" so it is a decision
  someone makes rather than one that got made by omission.
- **No sweep.** A stale figure obliges none, and the wrap's pair re-takes every
  table under whatever allocator is shipped by then.
- **No runtime allocator switch.** A `--global-allocator` flag would have to
  link all three and dispatch through a vtable per allocation, which prices the
  mechanism above what it measures. Rejected beside the mechanism.
- **No `tcmalloc` leg.** The spec named two replacements and both were measured;
  a third on the strength of the first two losing would be a fishing
  expedition, and the figure takes a new leg by one entry in `ALLOCATOR_LEGS`
  if anyone wants one.
- **No library-side reading.** Every number here is a `pgdq` number, and the
  library's own per-row budget is unmoved because no library code changed —
  which is what the phase's "a lever that lands re-reads that budget" rule
  asks, answered by there being nothing to re-read.
