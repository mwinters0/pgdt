# P15.21 notes — the default read as the incumbent

**The physical cap holds, and both readings are taken at the shipped chunk.**
`gzip-first-pass` and `gzip-resident` were re-taken at `055c8d27`, which
carries this verdict and the 4 MiB chunk, as one sitting
(`runs/measure-20261029T135551/`, launched from
`runs/gzip-first-pass-resident-20261029-1355/`). All 250 readings were taken,
none discarded for contention, and each count's resolved workers were checked.
`gzip-first-pass` reads every level-6 writer's file `level` and the level-1
file `no lift`, so no file reopens D2's gzip clause
([`decisions.md`](decisions.md), "D2"). `gzip-resident` finds no cell
under-billed against the 4 MiB chunk's charge
([`measurements.md`](measurements.md), "What a gzip scan holds against its
charge", "What a parallel gzip first pass buys").

## The bar, fixed before the reading

The bar is the harness's, so the table states the verdict
(`measure.gzpass_outcome`, `measure.gzpass_verdict`), by `gzpass_beats`'
pairs and margin, 7.1's bar:

- **`reopens`** is read first: the logical count or the logical count less
  one beats the physical count, on a file either lifts — the
  physical count beating serial, or that upper count beating serial itself.
  One such file reopens the default, whatever the others read.
- **Otherwise each lifted file names the cost an upper count adds**, by 7.1's
  falsifiers: `shared`, `oversubscription`, or **`level`**, the physical
  count level with the logical count. `level` replaces 7.1's `stands`, which
  named the logical default standing; it no longer ships. The verdict lists
  each lifted file's outcome, and the pair table every file's.
- `no lift` and `regresses` read the lift at the physical count and settle
  nothing.
- **`gzip-resident`**, by 15.18.3's bar: no cell under-billed against
  `Chunking::task_bytes`, an under-bill being a remainder past
  `MEMORY_UNPOOLED_BOUND` or the limit leg's secant past a task's charge
  (`measure.run_gzip_resident`).

## What 15.7.2 inherits

- **The cap stands on `level`, not on a cost.** At the 1 MiB chunk the
  physical count beat both upper counts on two level-6 files (15.20's
  sitting); at the shipped chunk the upper counts are level with it. So the
  record can say the logical count buys nothing over the physical count. It
  cannot say the logical count costs anything, and the P34 inbox's
  physical-core entry is rewritten to that.
- **D121 holds by its own `Reopens`, on a narrower margin.** The 1 MiB leg
  beats the shipped chunk on no file and is beaten on zlib's, level on the
  other two, and D121's statement of the evidence is rewritten to that.
- **At the shipped chunk `control_gz` fills a task's plaintext terms**, as
  the limit leg does: a 4 MiB chunk of a level-6 file inflates to about
  `SPECULATION_LIMIT`. The two first-pass legs' upper rows now hold about the
  same, both within the charge. At 1 MiB a chunk of that file inflated to a
  quarter of the limit.

## Negative results and what is unattributed

- **Part of 15.20's chunk margin may have been layout.** 15.20 read the
  4 MiB chunk as the leg build; here it is the shipped build and the 1 MiB
  the leg, and the 4 MiB chunk's advantage narrows. A pair cannot remove a
  layout difference, so the two sittings do not separate the chunk's share
  from the layout's.
- **Why the upper counts gained at the shipped chunk is unattributed.** The
  twelve-job rows sit where 15.20's 4 MiB leg read them; the twenty-three-
  and twenty-four-job rows are faster than at `ab7f630c`. Nothing here names
  why; a profile of the twenty-four-job pass at each chunk would.
- **The BGZF leg's per-reader secant is past its charge, unattributed**, as it
  was at 15.18.3. Every cell's remainder is within the bound, so it is not an
  under-bill by the bar above. A heaptrack pair of that leg at 24 and 4 jobs
  would name it.
- **The 15.19.1 level-1 remainder does not recur**: here the default is the
  faster in five pairs of seven, within the margin either way.
