# P30.6.1 notes — the gate reads the margin

What 30.7 and the phase's wrap inherit from correcting the gate's reading.
The criterion is `measure.GATE_MARGIN_PERCENT`, read by `_gate_cell`; the
re-rendered sitting is `runs/measure-20261005T155330/tables.md`, and the
rendering 30.6 folded is kept beside it as `tables-as-taken.md`.

## What 30.7 inherits

- **The gate blocks on one leg**: 128 MiB blocks in `-m 1536m`, on the
  shipped build alone ([`measurements.md`](measurements.md), `reserve`). No
  leg fails on both builds, so nothing is filed against `KD34` from the gate.
  That leg is the failing arrangement 30.7's instrument legs take.
- **The margin is a per-figure registration**, `GATE_MARGIN_PERCENT`, keyed
  by figure id and holding `reserve` alone; a figure absent from it fails a
  leg on a kill alone. Its value is `LIBRARY_MEMORY_MARGIN_PERCENT`, a mirror
  of `io::MEMORY_MARGIN_PERCENT` held to the library's source by the same
  test as the other mirrored constants, so a margin moved in the library
  fails that test before it can move the gate silently.
- **The line is the library's**: a leg leaving exactly the margin passes, as
  `margin_allowance`'s ceiling does, and the comparison is in bytes, so the
  head the cell rounds to a tenth cannot decide it.

## Negative results

- **Only the two gate sections and `reserve`'s sentence on the gate were
  re-folded.** The re-render moved three unrelated figures, and none of
  it is a reading of this sitting: `--render` runs `nested-decode-micro`'s
  `cargo bench`, `per-block-quadratic`'s `strace`d `parse` and
  `preamble-prepass`'s `grep` for the first `COPY` header against the sparse
  stand-ins `ReplaySession` stages, so those cells are today's host or a
  zero-filled file's
  ([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md),
  "`--render` runs three renderers' host steps"). Diff `tables.md` against
  `tables-as-taken.md` to see them.
- **`parallel-peak-rss`'s gate verdicts are unchanged**: its legs' heads are
  context, its container being the contract, so its section gained only the
  sentence naming its criterion.
