# P30.7 notes — the diagnostic sitting

What the round taking 30.7's readings inherits. The instrument is in the
tree; the readings are not, because a figure's legs are read from the commit
that carries them ([`measurements.md`](measurements.md), "A figure may be
published outside the sweep").

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
  cells and the check are now `heap_cells` and `instrument_check` inside
  `run_reserve`, shared by both families; re-rendering the 30.6 sitting's
  `reserve` with the new legs stubbed out gives the old output exactly.

## What the readings round does

- **The command** is `cd scripts && uv run measure.py --figure reserve
  --alone`, run from the commit carrying this slice. `--alone` is required:
  `reserve` shares a reading with `peak-rss`, so the sitting is a diagnostic
  one, marked NOT PUBLISHABLE, and nothing from it is folded into
  `measurements.md`. It takes the whole figure, black-box legs and `system`
  twins included, because the check reads the shipped leg from the same
  sitting. `reserve` alone took about half an hour of the 30.6 sweep
  (`--- reserve done` in `runs/measure-20261005T155330/log.txt`), before the
  builds and preflight, so the sitting is launched detached per
  [`gosub`'s handoff](../../.claude/skills/gosub/handoff.md).
- **What it reads**: the diagnostic table's rows against `Billed`, by the
  criterion in `RESERVE_DIAGNOSTIC_LIMITS`' comment, and the two check lines;
  a check outside the tolerance or a different resolved arrangement means the
  rows describe another run. The attribution is written into this file and
  the slice stops; the remedy is grilled from it (the spec, "What the move
  owes before a release").

## Negative results

- **The 128 MiB legs are not more limits on `RESERVE_INSTRUMENT_LIMITS`.**
  That family feeds the counter's own line, a fit at `RESERVE_MECHANISM_UNIT`
  whose pool subtraction and `reader_bytes` comparison are per unit, so a
  128 MiB leg there would put two units on one line.
- **`--render` of a sitting taken before this slice now raises on
  `reserve`**: the renderer asks for the diagnostic legs and `get_rss` raises
  `KeyError` on a leg the sitting never took, as it does for any leg added
  after a sitting.
