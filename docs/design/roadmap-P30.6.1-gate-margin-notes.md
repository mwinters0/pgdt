# P30.6.1 notes — the gate reads the margin

What 30.7 and the phase's wrap inherit from correcting the gate's reading.
The margin is `measure.REPORTED_MARGIN_PERCENT`, read by `_margin_short` and
reported beside the verdict; since 30.9 a leg fails on a kill alone
([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md), "P30's
gate reads kills"). The re-rendered sitting is
`runs/measure-20261005T155330/tables.md`, and the rendering 30.6 folded is
kept beside it as `tables-as-taken.md`.

## What 30.7 inherits

- **One leg leaves less than the margin**: 128 MiB blocks in `-m 1536m`, on
  the shipped build alone ([`measurements.md`](measurements.md), `reserve`).
  Nothing is killed on either build, so it fails nothing; the shortfall is
  `KD34`'s. That leg is the arrangement 30.7's instrument legs take.
- **The margin is a per-figure registration**, `REPORTED_MARGIN_PERCENT`,
  keyed by figure id and holding `reserve` alone. Its value is
  `LIBRARY_MEMORY_MARGIN_PERCENT`, a mirror of `io::MEMORY_MARGIN_PERCENT`
  held to the library's source by the same test as the other mirrored
  constants, so a margin moved in the library fails that test before it can
  move the report silently.
- **The line is the library's**: a leg leaving exactly the margin meets it, as
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
