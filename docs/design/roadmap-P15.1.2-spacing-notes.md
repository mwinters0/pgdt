# P15.1.2 notes — the spacing figure

**`pgdt-io` passes `gz-seek` the block `xz -T0` writes by default**
([`decisions.md`](decisions.md), "D113"), read off `gzseek-spacing`
([`measurements.md`](measurements.md), "What a checkpoint's spacing costs
gzip"), taken at `4691536e`, the commit carrying the instrument. Within the
range the figure reads, only the window section moves with the spacing among
gzip's own costs; the first pass is flat, and a resume's fixed cost is below
what the figure resolves at every spacing, so the column bounds it and does
not price it. What chose the value is what a unit shares with xz, read off
the code and xz's figures.

- **The constant is `CHECKPOINT_SPACING`**, in a new
  `crates/pgdt-io/src/source/compressed/deflate.rs`, the gzip adapter's home
  under the spec's placement; nothing reads it yet, its `dead_code`
  expectation lifting when the codec passes it to `gz_seek`, the expectation
  then failing until struck. A test holds it on D3's side of the default
  budget's block-decode line, with no decoder charged.
- **The value is not a row of the figure.** It interpolates exactly: the
  window section is 32 KiB a checkpoint, and neither the first pass nor a
  resume's fixed cost moves between the rows on either side of it.
- **The instrument** is `measure.run_gzseek_spacing` over
  `crates/gz-seek/examples/gz_read.rs`, its falsifying results in that
  function's docstring, none of which appeared. The figure now sits in
  `measure.FIGURES`, so a sweep re-takes it.
- **The sitting**: `runs/measure-20261023T201434/` (`tables.md`,
  `raw.json`) and its log, `runs/gzseek-spacing-sitting.log`.

## What the next slices inherit

- **15.5 passes `CHECKPOINT_SPACING` to `gz_seek::Forward::new`** and
  removes the `dead_code` expectation; 15.6 passes it to `gz_seek::Walk`
  to coalesce members. A single member's unit runs from one checkpoint to the
  first block boundary past the next multiple, so it overshoots the spacing
  by up to a deflate block — the whole member where its writer ends none
  (`KD129`).
- **The resident readings 15.7 takes on gzip** are what D113's `Reopens`
  names: a gzip run overrunning `MEMORY_UNPOOLED_BOUND` at this unit would
  reopen the spacing beside the bound.

## Negative results

- **No row resolves a resume's fixed cost.** Its `starts` row less its
  `forward` row lies inside both rows' spreads at every spacing, of either
  sign; the 1 MiB row bounds it, not prices it. A figure that prices it needs
  either far more resumes than 3072 in one run or a direct instrument timing
  the begin alone, and nothing now asks for either.
- **The `units` rows at 64 and 128 MiB sit highest, unresolved** and
  unattributed. A candidate is the instrument's one-unit output buffer
  outgrowing the last-level cache, which a profile at both ends would name.
