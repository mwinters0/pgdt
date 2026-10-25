# P15.7 notes — the resident readings

**`gzip-resident` is taken at `511ca16b` and folded in.** Its section is
[`measurements.md`](measurements.md), "What a gzip scan holds against its
charge", which also holds the reading's interpretation. The sitting's output
is `runs/measure-20261025T173407/` and its log is
`runs/gzip-resident-20261025-1721/log`. The row's split is in
[`../status/history/2026-10-25.md`](../status/history/2026-10-25.md), "15.7
split: three reviews".

- **The hypothesis holds.** `MEMORY_UNPOOLED_BOUND`, read off `.xz` alone,
  bounds what gzip holds outside its charge on both legs. The BGZF block path
  meets its charge at every count. Nothing re-derives the bound, and D113's
  reopening on a gzip overrun has not fired.
- **The first pass is over-billed at every count.** That is the falsifier the
  instrument aimed at the task charge. On `control`'s 4.07 : 1 a worker holds
  under two thirds of `Chunking::task_bytes`, because the four plaintext terms
  priced at `SPECULATION_LIMIT` fill only at a chunk reaching the limit. Most
  of what a worker does hold is unattributed. Whether the worst-case charge
  stands is under STATUS's "Decisions worth another look".
- **The bills are the library's.** Each run's `scan started` budget equalled
  the mirror (`GzipResident.STATED_BILLS`), and each first pass ran as many
  workers as its row's count.

## What the next round inherits

- **A parallel first pass is slower than the serial one below about twelve
  workers.** The figure's own log shows it at four jobs. The hand probe beside
  it (`runs/gzip-resident-20261025-1721/jobs-probe/`) shows it at every count
  from two to eight, and at two jobs only one core is busy. CPU time stays
  near six serial inflates whatever the count, so wall clock is that work
  divided over the workers past the frontier. The evidence skill's fourth
  question says readings over a suspected defect price the defect, and 15.7.1
  is throughput readings of this pass. So whether a repair comes first is
  under "Decisions worth another look". The time a task costs is
  unattributed. A profile of a two-job first pass would name it
  (`measure.py --profile-recipe`).
- **KD141 is not promoted.** No task's remainder went over its charge on this
  file. Its promotion now names a resident reading in general, since the
  entry may not name a ticked slice.

## Negative results

- **No leg varies the limit or the chunk length.** The chunk-length legs are
  15.7.1's throughput figure, and the limit is priced only by this figure's
  secant (the spec's "Slice order"). That secant is a pricing at one
  compression ratio: a file whose chunks reach the limit would fill the
  terms this one leaves a quarter full.
- **No fetched leg.** No figure reads a remote first pass, and the download a
  fetched pass adds to each worker's charge is unmeasured here.
