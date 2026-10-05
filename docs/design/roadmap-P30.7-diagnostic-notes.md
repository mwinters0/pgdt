# P30.7 notes — the diagnostic sitting

What the remedy's grilling and the phase's wrap inherit. The sitting is
`runs/measure-20261005T193210/` (`tables.md`, `raw.json`, `log.txt`, and each
instrument rep's own report under `instrument/`): `cd scripts && uv run
measure.py --figure reserve --alone` from `27593d2c`, exit 0, its handoff
`runs/30.7-diagnostic-sitting-20261005-1935/HANDOFF.md`. It is NOT
PUBLISHABLE (`--alone`, and `reserve` shares a reading with `peak-rss`), so
nothing from it is folded into [`measurements.md`](measurements.md), and its
readings are cited here by file rather than quoted.

## What landed

- **`reserve` declares two diagnostic legs**: `_reserve_diagnostic_specs` in
  `scripts/measure.py`, the flagless shape on the introspection build over
  `control_xz128` at `RESERVE_DIAGNOSTIC_LIMITS` (`1536m`, `2g`). The
  constant's comment says why those two and what reading would falsify each
  candidate heap.
- **They render as their own table**, after the 24 MiB instrument legs'
  check: each row the charge its own run was billed (`charge_bytes` at the
  count it resolved) beside every heap's reading, none subtracted, then the
  same instrument-against-shipped check the 24 MiB legs get. The two-heap
  cells and the check are `heap_cells` and `instrument_check` inside
  `run_reserve`, shared by both families.

## The attribution

**The rows describe the gate's run.** Both diagnostic legs resolved the
shipped build's arrangement (four readers, five), each inside
`INSTRUMENT_TOLERANCE_PCT` of its shipped leg, no rep killed on any build,
and the gate reproduces 30.6.1's verdict: 128 MiB blocks in `-m 1536m` fails
the margin on the shipped build alone, every other leg passes.

**Two of the criterion's three candidates carry excess, not one.**
`RESERVE_DIAGNOSTIC_LIMITS`' comment names one heap per outcome; the rows show
two. Each is read against the share of `Billed` its heap can hold: the
decoder's footprint (`XZ_DECODE_FOOTPRINT`, a reader) is C's, so Rust's share
is the block slot and chunk buffer a reader plus the pool's list.

- **The program holds one unit more than the charge bills, from two readers
  up.** Rust's live high-water exceeds Rust's share of the charge by one
  128 MiB unit and a few MiB, at both limits, identical across reps (one `2g`
  rep, `instrument/0009-…`, by two units). The 24 MiB instrument legs, in the
  same sitting, show the same step at that unit: about one block and a few MiB
  above Rust's share at every count from two readers to twenty-four, and a
  few MiB alone at one reader, where the black-box 128 MiB legs at one reader
  leave no room for an extra unit either. The term is held on both builds —
  it is most of what the `system` twins hold above the charge — and it is
  **unnamed in the code**: the account that would name it is `BlockCache::slot`,
  `retain` and `BufferPool`'s two lists under concurrent readers, whose
  rustdoc bills `(POOL_DEPTH.max(jobs) − 1) × unit` beside each reader's block.
  The 24 MiB counter line `measurements.md`'s `reserve` publishes fits
  straight across this step, which its fixed term and one-reader residual are
  consistent with.
- **mimalloc keeps about one unit the program has freed, on the shipped build
  alone.** Peak RSS less Rust's live high-water and glibc's is, per rep, either
  the process's non-heap baseline (a few tens of MiB, two reps of six) or that
  plus roughly one unit (four of six); the `system` twins are tight across
  reps, and each limit's worst shipped rep sits about one unit above its
  `system` twin's worst. That difference is what fails the margin. No reading
  names the mechanism. The candidate in source is mimalloc v3's `purge_delay`
  (1000 ms by default, `c_src/mimalloc/v3/src/options.c` in
  `libmimalloc-sys` 0.1.49), a freed allocation staying committed until
  purged, which a rep-to-rep spread is consistent with; a 128 MiB block on
  glibc is above its mmap threshold's ceiling and returned on free.
- **Not C.** glibc's high-water is within a few MiB of the decoder footprint
  the charge bills, at both limits.

**Neither term alone fails the margin**, by arithmetic on these readings: the
`system` twin holds the program's term and passes, and the worst shipped rep
less one unit would leave more than `MEMORY_MARGIN_PERCENT`. The gate's failure
is mimalloc's retention landing on a remainder the program's unbilled unit
already brings near `MEMORY_UNPOOLED_BOUND`.

## What the remedy's grilling inherits

- **The retention alone.** The program's term is held on both builds, so it is
  filed as a both-builds failure is and does not block: `KD111`, P23's
  ([`../status/deficiencies.md`](../status/deficiencies.md)). The split and
  why billing the unit is not P30's remedy are
  [`../status/history/2026-10-05.md`](../status/history/2026-10-05.md),
  "30.7's sitting".
- **The spec's remedies, against the retention**
  ([`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md), "What the move
  owes before a release"): an allocator option, once a sitting confirms the
  mechanism it reaches (below), a library change suiting glibc as well as
  mimalloc, or D13 reopened, which answers it by removing it.

## The candidates in mimalloc's source

Read for the remedy's grilling in `libmimalloc-sys` 0.1.49, which compiles
mimalloc 3.3.2 (`build.rs` takes `v3` unless the `v2` feature is set); paths
are under its `c_src/mimalloc/v3/src/`. Source, not a reading.

- **A 128 MiB block is an arena's huge singleton page**, not a direct mmap:
  only an object over `arena_max_object_size` (2 GiB) goes to the OS and is
  unmapped on free (`arena.c`, `page.c`). An arena reserves `arena_reserve`,
  1 GiB, so four readers' blocks span two arenas (the 2050 MiB reserved
  high-water in the 30.7 rows).
- **Freed by its owning thread, it is retired at once and scheduled for purge**
  (`free.c`, `page.c`, `arena.c`'s `mi_arena_schedule_purge`): committed and
  reusable until `purge_delay` (1000 ms) expires. **There is no timer** — a
  purge runs only from an allocator call (a page free, every 10 000th generic
  allocation, `mi_collect`), at most once per delay/10. Purge decommits by
  default (`purge_decommits` = 1), which on Linux is `MADV_DONTNEED`, so a
  purge drops resident at once. `purge_delay` = 0 purges inside the free; −1
  never purges.
- **A second candidate: a cross-thread free.** A block freed by a thread that
  does not own its page goes on the page's `xthread_free` list (`free.c`,
  `mi_free_block_mt`): freed at once if the page is abandoned, otherwise held
  until its owner collects. A full page is normally abandoned (`page.c`), and
  a singleton page is full once allocated, so whether a block is held turns on
  whether its owner has abandoned the page yet; not traced further.
  `purge_delay` would not reach it. **`pgdt` frees cross-thread on every
  path**: each read and each parse is its own `spawn_blocking` task on
  tokio's pool, whose idle threads exit after 10 s, and a block's last `Arc`
  drops on whichever task evicts it, replaces it (`KD20`) or ends a scan
  holding its view (`pgdump_query/src/io.rs`, `BlockCache::slot`, `retain`,
  `PooledBuffer`'s `Drop`; `pgdump_query/src/leader.rs`). A straddling first
  read's chunk buffer is up to a unit, never pooled (`keeps` takes only
  chunk-sized buffers), so it is allocated fresh on the reading task and
  freed on the scanning one.
- **Setting an option**: `MIMALLOC_<NAME>` from the environment, read once at
  init; `mi_option_set` at any time, every purge reading the delay afresh
  (`options.c`). `libmimalloc-sys` declares `mi_option_set` only under
  `extended` and lacks v3's purge option constants; the symbol is linked
  regardless. No compile-time default for `purge_delay` exists. Upstream
  DataFusion sets no mimalloc option anywhere (`datafusion-cli`, `benchmarks`).
- **What the instrument's `mi_stats_json` can say.** The release build
  compiles `MI_STAT` 0 (`include/mimalloc/types.h`; `build.rs` defines
  none), which keeps `purged` and `purge_calls` but not the abandonment and
  reclaim counters: every 128 MiB rep of 30.7 reads `pages_reclaim_on_free`
  0 and a negative `pages_abandoned` current. Purges do run — a few hundred
  calls a rep, exit totals that say nothing of what was held at the peak.

## Negative results

- **mimalloc's committed high-water is not a resident reading at a moment.**
  In one rep (`instrument/0017-…`) it exceeds peak RSS less glibc's; peak RSS
  less live and glibc's high-waters is the reading for retention, a lower bound
  on what was not live when the process peaked.
- **`Billed` whole is the wrong yardstick for Rust's live high-water**: it
  carries C's decoder footprint, which the counter cannot see, so comparing the
  two understates the program's excess by that footprint.
- **The 128 MiB legs are not more limits on `RESERVE_INSTRUMENT_LIMITS`.**
  That family feeds the counter's own line, a fit at `RESERVE_MECHANISM_UNIT`
  whose pool subtraction and `reader_bytes` comparison are per unit, so a
  128 MiB leg there would put two units on one line.
- **`--render` of a sitting taken before 30.7's legs raises on `reserve`**:
  the renderer asks for the diagnostic legs and `get_rss` raises `KeyError` on
  a leg the sitting never took, as it does for any leg added after a sitting.
