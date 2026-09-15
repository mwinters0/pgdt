# P10.10.1 — Carrying statistics, priced: notes

What the re-take and the phase wrap inherit. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Measurements", `statistics-pruning`; the instrument this extends is
[`roadmap-P10.10-figures-notes.md`](roadmap-P10.10-figures-notes.md)'s. **The
instrument landed and no reading was taken**: the table in
[`measurements.md`](measurements.md), "What row-group statistics buy a query",
is still `312af13`'s, and `cd scripts && uv run measure.py --figure
statistics-pruning` from the instrument's commit is what re-takes it.

## What exists

- **One leg, `query-pruning-unnarrowed-uncarried`** (`measure.PRUNING_UNCARRIED`),
  run for the third filter alone. Its untimed builder states
  `measure.NO_STATISTICS` where the other legs' state
  `GATHER_STATISTICS`; its timed query states `--statistics all`, so its argv
  is the `unnarrowed-all` leg's with the builder's request swapped and nothing
  else, which `test_measure.py` asserts by substitution.
- **The query asks for statistics rather than refusing them**, for the reason
  on the constant: its note is the check. `pruning_problems` refuses the leg if
  it printed a pruning or stop note — the builder's cache carried statistics
  after all — or returned a row count other than the row's other legs'.
- **The table gains two columns on the third row**, `—` on the first two:
  "Statistics used, none in the cache", the leg's wall, and "Δ carrying them",
  the `all` leg's wall against it. Its per-rep readings are listed under the
  third filter's, and the generated paragraph says what the leg is.
- **`perf_generator_fidelity.rs`** runs the figure's third filter, stating
  `--statistics all`, over a second cache `parse --statistics none` wrote, and
  asserts no statistics note, no stop note and the same closing count — the
  property the refusal above depends on, at the figure's group size.

## What a reading will mean

- **"Δ carrying them" is decoding and consulting together**: the two queries
  state the same flags and differ by the cache. The third row's existing Δ,
  `none` against `all` over the same carrying cache, is consulting alone, so
  the decode is read as the difference of the two. Both are subtractions of
  medians of one build, not an attribution by instrument; a residual inside
  the spreads is reported as unresolved, not as zero.
- **Against a bare cache the plan looks for statistics and finds none**
  (`prune_blocks`, `prune_block` returning `None`), so the leg is not free of
  the lookup; it is per block, and `pruning` is one block.

## For the re-take

- **The fold-in rewrites the hand prose under the table** that says no leg
  prices a cache carrying statistics against one that does not
  (`measurements.md`, the paragraph beginning "Both pruned legs read a floor"),
  and the second half of [`roadmap-P20-statistics-memory-inbox.md`](roadmap-P20-statistics-memory-inbox.md),
  "What the statistics figures leave unpriced", is struck with it; then the
  box is ticked.

## Negative results

- **`--statistics none` over the bare cache was not the leg**: it prints no
  note whatever the cache holds, so a builder that silently gathered would
  publish a Δ of nothing against nothing with no refusal able to fire.
