# P23 inbox — facts filed for its grilling

Evidence that P23 (statistics coverage and the resident reserve) will need.
**This is a queue, not a document**: when P23 is grilled, walk every entry,
fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## mimalloc reads back every option by name

**Fact.** mimalloc 3.3.2 exports `mi_options_print_out(out, arg)`
(`src/options.c`), which writes each option's name and value as mimalloc holds
it, through a caller's output function. `libmimalloc-sys` 0.1.49 does not
declare it, nor v3's purge options; the symbol is linked regardless, as
`mi_option_set` is. An earlier instrument read one option back instead, by
an index into `mi_option_e` counted by hand, guarded by a test `mise run
check` never runs (`introspect`'s `MI_OPTION_PURGE_DELAY`, removed; `30f73fb6`
holds it).

**Why P23 cares.** If P23 confirms the unit mimalloc keeps by switching an
option off, or sets one as the remedy, the instrument has to show the option
reached mimalloc rather than assume it. Reading all of them by name needs no
index to drift at an upgrade; copying back that hand count is the obvious
route and the worse one.

**Origin.** 2026-10-06
([`../status/history/2026-10-06.md`](../status/history/2026-10-06.md),
"30.8's read-back goes with it"). *Contingent on* mimalloc still exporting
`mi_options_print_out`, and on `libmimalloc-sys` still not declaring it.

---

## The unit mimalloc keeps of a 128 MiB block

**Fact.** At 128 MiB blocks in `-m 1536m` and `-m 2g`, the instrument build
reads two terms above the charge. The program's live high-water exceeds
Rust's share of `Billed` (a reader's block slot and chunk buffer plus the
pool's list; the decoder footprint is C's) by one unit from two readers up,
on both builds (`KD111`). Peak RSS less Rust's live high-water and glibc's is,
on the shipped build in four reps of six, the process's non-heap baseline
plus about one unit, which the `system` twins never show (`KD34`): neither
term alone leaves the `-m 1536m` leg short of the margin. glibc's high-water
is within a few MiB of the decoder footprint, so it is not C. Readings:
`runs/measure-20261005T193210/` (`--alone`, not publishable; each instrument
rep's report under `instrument/`). Read in `libmimalloc-sys` 0.1.49's
mimalloc 3.3.2 (`c_src/mimalloc/v3/src/`), source and not a reading, the
candidates are two:

- **`purge_delay`** (1000 ms): a 128 MiB block is an arena's huge singleton
  page, not a direct mmap (only an object past `arena_max_object_size`, 2 GiB,
  is), retired at once on its owner's free and kept committed until a purge,
  which runs only from an allocator call, at most once per delay/10 — there
  is no timer. A purge decommits (`MADV_DONTNEED`), so resident drops at
  once; `0` purges inside the free, `-1` never.
- **A cross-thread free**: a block freed by a thread not owning its page
  waits on the page's `xthread_free` list until its owner collects, unless
  the page is abandoned (`free.c`, `mi_free_block_mt`); `purge_delay` does
  not reach it. `pgdt` frees cross-thread on every path — each read and each
  parse is its own `spawn_blocking` task, and a block's last `Arc` drops on
  whichever task evicts it, replaces it (`KD20`) or ends a scan holding its
  view (`io.rs`, `BlockCache::slot`, `retain`, `PooledBuffer`'s `Drop`); a
  straddling first read's chunk buffer, never pooled, likewise.

An option is set by `MIMALLOC_<NAME>` at init or `mi_option_set` at any time,
each purge reading the delay afresh; upstream DataFusion sets none. The
release build's statistics keep `purged` and `purge_calls` but no
abandonment or reclaim count (`pgdt/src/introspect.rs`, `readings_of`), so
neither candidate is confirmed by any reading the instrument takes. Peak RSS
less the live and glibc high-waters is the retention reading; mimalloc's
committed high-water is not resident at a moment, and `Billed` whole carries
C's footprint the counter cannot see.

**Why P23 cares.** It re-fits `MEMORY_RESERVE` and `MEMORY_UNPOOLED_BOUND`
under mimalloc and bills or removes `KD111`'s unit. A remedy for the retained
unit must first be confirmed by switching the candidate off with mimalloc's
own option, and is chosen in D13's order: a library change suiting both
allocators first, an option as a default the environment overrides second
([`decisions.md`](decisions.md), "D13").

**Origin.** The diagnostic sitting on the instrument build, 2026-10-05, at `27593d2c`
([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md),
"30.7's sitting"). *Contingent on* mimalloc 3.3.2 and the block cache's
eviction paths as they stand.
