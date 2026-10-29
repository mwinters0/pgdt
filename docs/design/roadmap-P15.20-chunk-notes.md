# P15.20 notes — the chunk priced at the default count

**The 4 MiB chunk ships.** `gzip-first-pass`, re-taken at `ab7f630c` with
the chunk leg at the physical count (`runs/measure-20261029T015226/`,
launched from `runs/gzip-first-pass-20261029-0143/`), reads rapidgzip's
4 MiB beating the shipped 1 MiB on all three level-6 writers' files and
level on the level-1 file. All 196 readings were taken, none discarded for
contention, and each count's resolved workers were checked. That meets the
bar fixed before the sitting, so `ahead.rs`'s `CHUNK_BYTES` is 4 MiB and
D121 is rewritten to it ([`measurements.md`](measurements.md), "What a
parallel gzip first pass buys").

## The bar, fixed before the reading

The bar is the harness's, `measure.gzpass_chunk_verdict`, so the table states
the verdict rather than leaving it to whoever renders it. The leg's chunk
replaces the shipped one where it beats it, by `gzpass_beats` at
`GZPASS_CHUNK_JOBS`, on at least `GZPASS_CHUNK_WINS` of
`GZPASS_CHUNK_WRITERS` (the three level-6 files) and is beaten on none of
the four. A win on the level-1 file counts toward nothing. The bar is
symmetric, so it is D121's `Reopens` with the two lengths swapped.

## What the switch moved

- **The leg is inverted, not retired**: `gzip-1mib-chunks` cuts the input
  chunk, the length D121 rejects, so a re-take can still meet its
  `Reopens`. The feature, `GZPASS_CHUNK_LEG`, the `--version` marker
  `(gzip chunk: 1 MiB)` and the lint build follow.
- **The harness's shipped chunk is its own constant**, `measure.GZIP_CHUNK`,
  split from `GZIP_INPUT_CHUNK`. A decode's input buffer and a local task's
  reads stay at the input chunk. A task's charge, a file's chunk count, the
  shipped binary's check and the limit leg's ratio check read the chunk.
- **Every task is charged 6 MiB more** (`Chunking::task_bytes`: the chunk
  and its feed each grow by 3 MiB). `GzipResident.STATED_BILLS` are re-read
  off trial runs of this tree's release-image build
  (`runs/p15.20/trial-bills.sh`, its logs beside it), and `GZPASS_BUDGET`
  still affords the logical count at either chunk.
- **`control_gz` is no longer clear of the speculation limit.** A 4 MiB
  chunk reaches the limit at about 4 : 1, which is `control`'s own mean
  ratio, so a first pass over `control_gz` fills the plaintext terms on the
  chunks that pack tighter than its mean. `repeated_gz` is still the only leg
  filling them on every chunk, with room (`gzip_limit_ratio_problem`).
  `gzip-resident`'s first-pass cells were read against the 1 MiB charge and
  are stale until it is re-taken.
- **Two `pgdt` fixtures grew to hold their workers' chunks**, a first pass
  running no more workers than its file has chunks (D2):
  `a_gzip_first_pass_says_whether_it_inflates_in_parallel`'s file and the
  remote first-pass tests' `scattered_gzip_dump`. Each was two chunks at
  4 MiB.
- **`gz-seek`'s `spec_cost` example** cuts 4 MiB chunks, as `pgdt-io` does.

## What the reading cannot say

The leg is a separate build, so its layout is a second difference no pair
removes (`measurements.md`, "Two builds of one source can differ by
layout"). And the figure affords every count its memory, so it never shows
the longer chunk's second cost, every task charged it twice: the bar prices
time alone. Under a budget that seats fewer workers than the default count,
the larger charge can seat fewer still. Nothing here reads that case.

## Unsettled: the default count

The same sitting reads the default `shared` on zlib's and zlib-ng-compat's
files and `stands` on GNU gzip's, where the physical count is faster than
both upper counts by its medians but in five pairs of seven, one short of
the bar. The three sittings before read `shared` on every level-6 file. D2
is unchanged and the chunk was priced at the shipped default. Whether this
reading reopens D2's gzip clause is the maintainer's (STATUS, "Decisions
worth another look"). The P34 inbox's physical-core entry is contingent on
the same verdict.
