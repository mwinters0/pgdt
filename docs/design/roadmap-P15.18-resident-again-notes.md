# P15.18 notes — the resident readings again

**The instrument is landed; the readings, the heaptrack recording and the
charge remain.** `gzip-resident` is re-taken from the commit carrying it
([`measurements.md`](measurements.md), "A figure may be published outside
the sweep"):

    cd scripts && uv run measure.py --figure gzip-resident

It is `pgdt parse` under the peak-resident wrapper over three files, at
`--jobs` 1, 4 and 24 (`GZIP_RSS_JOBS`), each beside its `system` twin, three
reps a leg. `measure.run_gzip_resident`'s docstring holds the falsification
criteria, written before any reading.

## What changed in the figure

- **A third leg, the falsifier: `repeated_gz`**, a single member whose
  chunks reach `SPECULATION_LIMIT` (`generate_repeated_rows.py`). It is
  `control`'s rows, each kept row written `REPEAT` times in a run and the
  block thinned to its source's length, deflated by `control_gz`'s zlib at
  level 6 behind `pg_dump`'s header. Its plaintext is `control`'s length, and
  it packs several times tighter. The figure refuses it before reading any
  leg unless its mean ratio is past the limit's by `GZIP_LIMIT_ROOM`
  (`gzip_limit_ratio_problem`), which reads `control`'s size. So a re-render
  needs that size too, and `recorded_input_sizes` now records a derived
  input's sources.
- **Every leg states `GZPASS_BUDGET` in `GZPASS_MEMORY`**, through a family
  of its own (`GZRSS_FAMILY`), which is `parse-rss-jobs-<n>` but for the
  budget. Under `PARALLEL_BUDGET` the twenty-four-job first pass resolved
  twenty, because 15.17.4 moved a task's charge. 15.17.4's notes left the
  count or the allowance to this slice. The count stays, twenty-four being
  the guard (`roadmap.md`, "Four workers is the optimization baseline;
  twenty-four is the guard"), and `gzip-first-pass` had already sized a
  budget for it. The BGZF leg's bill does not move with the budget: its pool
  is depth-bound at both, which the trial runs below confirm.
- **The secant table says, leg by leg, whether each added worker held past
  its charge.** On the limit leg that is the under-bill that admits workers a
  container then kills.

## What the readings round inherits

- **The bills a run states**, read off `scan started` at this tree by hand
  and held by `test_measure.py`'s `GzipResident.STATED_BILLS`
  (`runs/p15.18/trial-bills.sh`, each run's log beside it). Every leg
  resolved its row's count under the new budget, twenty-four included, and
  each first pass ran that many workers. The two single members bill alike
  at every count, both having chunks past their first. The BGZF bills are
  the ones `PARALLEL_BUDGET` stated.
- **What the arithmetic predicts, before the sitting** (the evidence skill,
  rule 1). A task's charge is the chunk, five speculation limits, five
  guesses' histories and an input chunk (`Chunking::task_bytes`). On the
  limit leg a chunk inflates past the limit, so the four plaintext terms
  fill. The fifth limit is the compressed bytes past the chunk, which at this
  ratio is under a megabyte. So a worker's named terms sum to about four
  limits and three input-sized buffers, about one limit under its charge.
  **The charge is refuted as a bound where a worker's unnamed remainder on
  this file is larger than one limit.** At 15.7, on `control_gz` and before
  15.17's line, each worker held more than a limit beyond its named terms,
  unattributed ([`measurements.md`](measurements.md), "What a gzip scan holds
  against its charge"). Were that remainder to survive the line and not
  shrink with the ratio, this leg would be under-billed.
- **`KD141` is not what the limit leg reads.** That entry is the compressed
  bytes past a chunk, up to nine eighths of a limit where a fixed-code block
  carries literals. At the limit leg's ratio those bytes are a small fraction
  of a limit. Settling `task_bytes` from this figure settles its plaintext
  terms; `KD141`'s term is read from the code, the same arithmetic the entry
  states, and a reading of it would need a file of incompressible blocks.
- **The heaptrack recording is not built.** `--heaptrack-recipe` records
  `reserve`'s xz step pair alone (`HEAPTRACK_AXIS`). A gzip pair would be
  the limit leg's shape at four jobs and at twenty-four, read as a
  difference. It conflicts with a rule the harness asserts.
  `test_every_binary_runs_in_a_container_no_recording_meets` holds the
  recording's container to eight times the allowance the recorded run
  states. Every `gzip-resident` shape states `GZPASS_BUDGET`'s allowance, so
  that rule asks for about 27 GiB, more than this machine has. The readings
  round decides which: the rule restated for a first pass, whose resident is
  its bill and not its allowance; or a gzip shape of its own under a smaller
  allowance, which breaks the reconciliation with the timed shape. Either is
  a call for "Decisions worth another look".

## Negative results

- **`REPEAT` is eight.** Deflating a sample of `control`'s rows at level 6
  gave a ratio past the limit's only from six repeats, and eight clears
  `GZIP_LIMIT_ROOM`. The generated file's ratio is in its generator's log,
  `runs/p15.18/generate.log`.
- **A plain `repeated` input was not registered.** No figure would stage it,
  and `test_every_defined_input_is_consumed` refuses an input no figure
  takes. The generator streams the repeated rows into the deflater instead,
  and the ratio check reads `control`'s size.
