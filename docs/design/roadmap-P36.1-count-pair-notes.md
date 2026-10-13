# P36.1 notes — the count pair

The instrument the hot-path slices are counted with is
`scripts/measure.py`'s `cmd_count_pair` and the helpers beside it, whose
docstrings say what each refuses and why; `CountPair` in
`scripts/test_measure.py` asserts it. This slice built it and took no
reading.

## What the next round inherits

- **The first reading is still owed**, from this slice's commit:
  `cd scripts && uv run measure.py --count-pair HEAD~1`. That pair builds two
  identical Rust sources, since this slice touched only `scripts/` and the
  docs. So its change column is the instrument's floor between two builds of
  one source. Its closing line says whether a pair fits under `CLAUDE.md`'s
  ten-minute line, which is the question the spec gave this slice. If the
  pair does not fit, it is launched detached as `CLAUDE.md`, "Long-running
  processes" says, and the round after reads its log.
- **"The parent" depends on the tree.** A tree carrying any change, untracked
  files included, is one change on `HEAD`. A clean tree is `HEAD` itself, one
  change on `HEAD~1`. So the slice rows' `--count-pair HEAD~1` is run after
  the slice is committed. An unattended round that leaves its work
  uncommitted runs `--count-pair HEAD` instead. Every other revision is
  refused.
- **No verdict is printed.** Whether a rise is beyond the instrument's noise
  is the slice's call, made against the floor the first reading gives. The
  `pgdt sql` row is known not to repeat exactly between runs of one build
  ([`measurements.md`](measurements.md), "What link-time optimization buys
  the shipped build"), and one count per shape cannot show that spread.
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
