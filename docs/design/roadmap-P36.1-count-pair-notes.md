# P36.1 notes — the count pair

The instrument the hot-path slices are counted with is
`scripts/measure.py`'s `cmd_count_pair` and the helpers beside it, whose
docstrings say what each refuses and why; `CountPair` in
`scripts/test_measure.py` asserts it. Its first reading is below.

## The first reading: two builds of one source

`--count-pair HEAD~1` from `adf34a87`, which touched only `scripts/` and the
docs (`runs/count-pair-36.1-20261013-1814/log`, its `HANDOFF.md` beside it).
It exited 0: every shape's legs answered alike, both legs report the
`release` profile and mimalloc, and both sit under the glibc floor.

- **The change column is non-zero on every row, and inside one build's
  run-to-run spread on each row that has one.** The tree against the parent: `parse` at
  the metadata level +8,715, at the data level −12,752, `strings` −24,889,
  `typed` −12,922, `pgdt sql` −26,696, each under 0.001%. The fat leg's five
  reps at `ef442ad6` (`runs/measure-20261012T073914/raw.json`,
  `release-profile`'s `release-fat` counts) spread by 65,156 on the metadata
  `parse`, 230,128 on `strings` and 101,358 on `typed`. On `pgdt sql` they
  fall on two values ([`measurements.md`](measurements.md), "What link-time
  optimization buys the shipped build") and spread by 211,787 within the
  lower. So counts repeat to the million the register publishes, not to the
  instruction, and the spec's "repeat exactly"
  ([`roadmap-P36-embedder-api.md`](roadmap-P36-embedder-api.md), "What a
  hot-path slice owes") holds only at that precision. **The data level has
  no per-rep count anywhere**: `release-profile` does not run it, so 36.4's
  row has this pair's one difference and no spread.
- **Two builds of one source are not one binary.** mimalloc's C source
  embeds the build's `__TIME__` (`options.c`, `mi_options_print_out`), so
  each leg carries its own time string and places it differently in
  `.rodata`. What lies between the two placements moves by the string's
  length, and the `.text` addressing it moves too (sampled: rip-relative
  displacements). The files are the same size and differ in `.rodata`,
  `.text`, `.rela.dyn` and the build id. So the change column is two
  layouts' difference plus two runs'. **Which of the two carries it is
  unattributed**; counting each leg more than once would separate them.
- **A pair does not fit under ten minutes.** This one took 26.4 min, its two
  cold builds 12m 36s and 12m 29s and the counts the rest. No incremental
  pair is timed yet, but one fat leg built incrementally took 9m 37s at
  35.24 (`runs/release-profile-35.24-20261012-0735/log`), so two legs exceed
  the line either way. A hot-path slice therefore launches its pair
  detached (same section), logging under `runs/`, and the round after reads
  the log.
- **The log's two `nerdctl` broken-pipe warnings**, after the parent's glibc
  check, appear at the same point after a build in
  `runs/release-profile-35.9-20261010-1755/log`. Both legs' binaries ran
  every shape, so nothing the pair reads was lost. What closes the pipe is
  unattributed.

## What the next round inherits

- **"The parent" depends on the tree.** A tree carrying any change, untracked
  files included, is one change on `HEAD`. A clean tree is `HEAD` itself, one
  change on `HEAD~1`. So the slice rows' `--count-pair HEAD~1` is run after
  the slice is committed. An unattended round that leaves its work
  uncommitted runs `--count-pair HEAD` instead. Every other revision is
  refused.
- **No verdict is printed.** Whether a rise is beyond the instrument's noise
  is the slice's call. The noise is one build's run-to-run spread on each
  row (above), never zero. On `pgdt sql` it includes the step between its
  two values, which a single count per leg can land on either side of.
- **Two refusals beyond the spec's.** The pair is refused when the image's
  inputs (`release/Dockerfile`, `rust-toolchain.toml`, `mise.toml`) differ
  between the legs, because the two builds would then differ by compiler.
  A shape is also refused when its two legs answer differently, because its
  counts would then be two workloads'. Only `pgdt sql` prints an answer.
- **Where it leaves things.** The binaries go to
  `runs/pgdt-count-pair-{parent,tree}` and are overwritten by the next pair.
  Their cargo target directories are `count-pair-{parent,tree}` under the
  alloc-builds root's `image/`, beside the allocator legs', and are kept so
  the next pair builds incrementally. The parent's worktree under `runs/` is
  removed after its build, even when the build fails.

## Negative results

- **`Session.time_run` is not reused.** It stages binaries on tmpfs, records
  telemetry and keys readings per figure, and a count needs none of that.
  The pair reuses `_script`'s shapes and `counted_script`'s placement of the
  counter, so its rows are the register's workloads.
- **A shape wrapped in `peak-rss` cannot be counted.** `counted_script` puts
  the counter directly around the timed `/pgdt`, so
  `statistics-gathering`'s data leg gained an unwrapped spelling,
  `parse-statistics-data`. It runs under that figure's container limit,
  because the statistics allowance is discovered from the container (D85).
