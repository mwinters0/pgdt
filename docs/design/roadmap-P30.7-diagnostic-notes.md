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

- **The spec's remedies, against the attribution** (the spec, "What the move
  owes before a release"): an allocator option reaches the shipped-alone term,
  whose mechanism is a candidate no reading has confirmed; re-setting the bound
  covers both terms without naming either (the sitting's own re-derivation is
  in its `tables.md`); D13 reopened answers the retention alone. **The
  program's term is a remedy the spec did not list**: billing the unit in the
  charge moves both builds and the flagless count, and is `KD34`'s territory
  as much as P30's.
- **Nothing is filed yet.** No `KD<k>` is opened and `KD34` is not rewritten:
  where the program's term is recorded, and against which phase, is the
  grilling's call.

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
