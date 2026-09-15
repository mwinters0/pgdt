# P10 — Per-row-group column statistics: notes

What the phase leaves that neither the code nor the register holds: its
**negative results**, and the facts aimed at the phases after it. What it built
is `statistics.rs`, `gather.rs`, `prune.rs`, `stream.rs`'s plan and
`pgdump_query-cli/src/info_statistics.rs`, and why each has its shape is in
[`decisions.md`](decisions.md); the spec it was built against is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md). Every
number the phase measured is in [`measurements.md`](measurements.md), "What
gathering row-group statistics costs a parse" and "What row-group statistics
buy a query". The slices' own notes docs were consolidated here at the wrap.

## For the phases after

- **P20.** Its inbox holds what the phase filed: per-partition statistics
  resident during a parallel gathering `parse`, and what the two figures leave
  unpriced. Beside those, three costs nothing measures: the `info --json`
  export, which grows with group count times tracked columns as the cache does,
  most at the tiny group sizes the correctness checks gather at; a back-fill,
  a second block-by-block read of the dump; and the plan resolving every
  matched block up front — before a resume token's offset, and for blocks a
  caller never polls — which is per block, not per row.
- **Whoever extends the `statistics` fixture.** It holds no `numeric`
  `Infinity` (PostgreSQL 13 cannot), and no `bytea`, `varchar` or `character`
  column carrying bounds, so the file-side assertion
  (`tests/statistics_fixture.rs`) never reaches one; the `character` dictionary
  is pinned against a hand-built column instead. Add a column rather than reason
  about bytes ([`roadmap.md`](roadmap.md), "Expand the generated fixtures
  freely").
- **At the 1 MiB default nearly every fixture block is one group** (the long
  value's is not), so a test that wants several groups states a size. A row
  wider than the group — most rows at the correctness checks' 32 bytes — starts
  in a group of its own, whose bounds
  already skip it, so few generated comparisons fire an early stop; what the stop
  report covers is taken by the hand tests at groups wider than a row.

## Negative results

- **The long value stays past the shipped read chunk on every major**, reviewed:
  the empty groups, truncation and piece joins need only a stated group size, and
  a shorter value would have hidden the quadratic carry scan that this fixture
  found in `determinism.rs`'s 64-byte-chunk leg (since repaired; `scan.rs`'s
  `a_line_many_chunks_long_is_scanned_once`).
- **`Error` is not `Clone`**, so the plan cannot hold a block's refusal to hand
  out on activation; a refusing block is resolved a second time, resolution
  being a function of the block and the plan alone.
- **No property-testing crate.** The random tests (`truths` against the row
  evaluator, the gather join, the pruned-equals-unpruned check) seed SplitMix64,
  so a failure reproduces.
- **Over `pg_dump` output a lost order step at a gather join rarely changes a
  block's order**: of the join mutations also run against the fixture sweep, the
  dropped row count failed it and the dropped step did not. The random join test
  in `gather.rs` is the one that guards the step. Two mutations passed as
  equivalent — a bytewise tie taking the later head, and `Clipped::order`'s
  `None` read as an order — both storing the same bounds, which `Clipped`'s
  rustdoc claims and a test pins.
- **A back-fill's save on an interrupt is not pinned apart from the throttle's**:
  the throttle starts due, so the first re-read block is banked either way.
- **The `scan arrangement` shortfall line is once per pass**, so a `--jobs` a
  budget cannot pay prints it for the scan and again for the back-fill.
- **A group of NULLs counts as a group without bounds** in `--detail`, so an
  all-NULL column reads bounds in none of its groups; `rows - null_count` cannot
  tell a value-less group from a short row observing nothing. A zero-row block
  contributes a block and no group.
- **Stopping on "not `True`" rather than `False`** passed the generated check,
  being equivalent: the two differ only on a value that does not decode, which
  no sorted block holds. **Skipping a stopped block's later segments** in one
  sub-stream was not kept: pruning already skips a later run, and it would save
  only the read before a later piece's first row, which no test sees.
- **A test's parallel leg can silently run as one sub-stream.** The recording
  source once forwarded no `ByteRangeSource::partitions`, and the CLI's
  `--jobs 3` query legs ran serially until they stated `--parallel-memory`, the
  default budget affording one query sub-stream; both passed while doing so,
  which is why each now states what makes it split.
- **The generated pruning check is slow in a debug build**, most of it loading
  the 32-byte gathered cache once per query; per-major threads, the plain cache
  for the unpruned leg and for resolving terms, and two literals per operator
  keep it inside a test run.
- **The figures' input.** `v_bool` cannot be the dictionary leg — every group
  holds both values — and a label drawn per row fails the same way, hence runs.
  A `--dqcache none` query prunes nothing, so the pruning legs build a cache.
  Text under no stated collation gets a dictionary and no bounds, which makes
  the equality leg the dictionary's alone. A text column was not the unnarrowed
  filter: with no bounds and past the cap no dictionary, it consults only NULL
  counts. A group of at most a dictionary's cap in distinct values keeps a
  dictionary and can skip, so the unnarrowed filter needs groups wider than
  that, and a sitting ending on a short group is refused (`pruning_problems`).
- **`--statistics none` over the bare cache was not the uncarried leg**: it
  prints no note whatever the cache holds, so a builder that silently gathered
  would publish a difference of nothing against nothing with no refusal able to
  fire.
