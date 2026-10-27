# P15.18 notes — the resident readings again

**`gzip-resident` is re-taken at `c4ec490d` and folded in, with a heaptrack
recording of the limit leg beside it; the charge is 15.18.1's.** The section
is [`measurements.md`](measurements.md), "What a gzip scan holds against its
charge", which holds the reading and the attribution. The sitting's output is
`runs/measure-20261027T182426/`, its log `runs/gzip-resident-20261027-1824/log`.
The recording is `runs/heaptrack-parse-gzrss-jobs-{24,4}-repeated_gz.{gz,txt}`
and `runs/heaptrack-first-pass-diff.txt`, run by the scripts beside
`runs/p15.18/heaptrack/` (each a verbatim excerpt of `--heaptrack-recipe`).
The row's split is in
[`../status/history/2026-10-27.md`](../status/history/2026-10-27.md), "15.18's
split".

- **No under-bill.** On the limit leg each added worker held 45.7 MiB against
  the 82.9 MiB a task is charged, and no cell's remainder reached
  `MEMORY_UNPOOLED_BOUND` on any leg. Each run's `scan started` bill equalled
  `GzipResident.STATED_BILLS`, and each first pass ran its row's count.
- **The charge is a bound, and on this file two of its terms hold almost
  nothing.** At the `malloc` boundary an added worker asks for about 25 MiB:
  one and a half chunk-decode buffers of `SPECULATION_LIMIT`, and about 1 MiB
  for everything its speculations hold. `Chunking::task_bytes` charges the
  speculation's plaintext twice, two limits, for a block as long as the limit
  ([`roadmap-P15.17.1-window-notes.md`](roadmap-P15.17.1-window-notes.md));
  15.17.1's sink keeps only the window its guess reads, and this file's
  writer ends no block that long.
- **About 20.7 MiB a worker is resident and not heap.** heaptrack cannot see
  it. It is unattributed; the candidate is a freed decode buffer an
  allocator keeps, which both allocators show.

## What 15.18.1 inherits

- **Which terms the charge keeps is an account, not this secant.** The
  secant says the sum is high on the one file that fills every term; the
  recording says which terms hold nothing. Before a term is cut, the remainder
  past the heap needs a name, because a charge cut to the heap's terms hands
  that remainder to `MEMORY_UNPOOLED_BOUND`, which is one constant for the
  process, while the remainder grows by a worker.
- **The decodes held, one and a half a worker, are a peak over twenty
  workers' phases.** Whether two can be held at once is the code's to say
  (`Ahead::top_up`, `DECODES_A_TASK`), not the recording's.
- **`KD141`'s term is still read from the code.** At the limit leg's ratio
  the compressed bytes past a chunk are a small fraction of a limit, so no
  reading here exercises it.

## The recipe

`--heaptrack-recipe` records a second pair, the first pass's
(`HEAPTRACK_FIRST_PASS_AXIS`): `gzip-resident`'s 24- and 4-job shapes at the
limit leg, read as a difference. Three defects in the recipe surfaced the
first time it ran in the image, and are fixed:

- **The container rule** asked eight times a run's allowance, about 27 GiB
  for this pair. It now asks eight times what a run may hold
  (`heaptrack_held_bound`), a first pass's bill; STATUS's "Decisions worth
  another look" carries the call.
- **The image's heaptrack writes `.gz`**, being built without zstd, so a
  recipe naming `.zst` printed empty reports (`HEAPTRACK_SUFFIX`).
- **A report piped from one container into another loses its tail**: a
  container's output reaches the host through the runtime's logger, which
  drops what is unwritten when the container exits. Each report now runs its
  pipe and writes its file inside one `sh -c`.

`reserve`'s step pair has not been recorded under the repaired recipe.

## Negative results

- **The account written before the sitting misread the code.** It summed a
  worker's named terms to about four limits, counting the speculation's
  plaintext twice as filled wherever a chunk inflates past the limit. Since
  15.17.1 the speculation's sink keeps only the window its guess reads, so
  those terms fill only with a block as long as the limit. The reading came
  in far under the predicted sum, and the recording shows why.
- **`REPEAT` is eight.** Deflating a sample of `control`'s rows at level 6
  gave a ratio past the limit's only from six repeats, and eight clears
  `GZIP_LIMIT_ROOM` (`runs/p15.18/generate.log`).
- **A plain `repeated` input was not registered.** No figure would stage it,
  and `test_every_defined_input_is_consumed` refuses an input no figure
  takes. The generator streams the repeated rows into the deflater instead,
  and the ratio check reads `control`'s size.
- **The pair is not recorded at a lower top count.** A pair at twelve jobs
  would fit a smaller container, but it is not a shape `gzip-resident`
  times, which the recording must be (`test_a_recorded_shape_is_the_shape_the_sweep_times`).
