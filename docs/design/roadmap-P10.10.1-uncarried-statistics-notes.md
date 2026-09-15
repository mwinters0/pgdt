# P10.10.1 — Carrying statistics, priced: notes

What the phase wrap inherits. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Measurements", `statistics-pruning`; the instrument this extends is
[`roadmap-P10.10-figures-notes.md`](roadmap-P10.10-figures-notes.md)'s. The
instrument landed in one commit and the readings were taken from it, as a
sitting of their own: the table is in [`measurements.md`](measurements.md),
"What row-group statistics buy a query", its marker carrying the commit, and
every number below is there.

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
  after all — or returned a row count other than the row's other legs'. The
  sitting passed it: the leg printed no note and returned the row's count.
- **The table gains two columns on the third row**, `—` on the first two:
  "Statistics used, none in the cache", the leg's wall, and "Δ carrying them",
  the `all` leg's wall against it. Its per-rep readings are listed under the
  third filter's, and the generated paragraph says what the leg is.
- **`perf_generator_fidelity.rs`** runs the figure's third filter, stating
  `--statistics all`, over a second cache `parse --statistics none` wrote, and
  asserts no statistics note, no stop note and the same closing count — the
  property the refusal above depends on, at the figure's group size.

## What the readings mean

- **Carrying statistics costs a query nothing this table resolves** at this
  input's group count. The leg's spread overlaps both other legs' of the third
  row, its median is slower than the carrying cache's rather than faster, and
  paired rep by rep the difference alternates in sign.
- **The decode is unresolved, not zero, and not read out of the Δs.** "Δ
  carrying them" is decoding and consulting together, the third row's own Δ
  consulting alone, but both are subtractions of medians of one build whose
  difference here is a negative cost: that is the spreads, not a term
  (`.claude/skills/evidence/SKILL.md`, rule 6).
- **What bounds the decode is the pruned legs, not this leg.** They decode the
  same carrying cache whole inside their fixed floor, so the statistics' decode
  at this group count is at most that floor, which is far under what the
  unnarrowed legs' spreads can see. The leg therefore adds a check that nothing
  large hides there, not a price.
- **Pricing the decode takes the process timing its own cache load**, or an
  input with enough groups that the floor stops bounding it tightly, not
  another subtraction ([`roadmap.md`](roadmap.md), "Attribution is
  introspective; only the gate is blind"). Where the group count grows by
  orders of magnitude — a dump the size of koji — this reading says nothing.
- **Against a bare cache the plan looks for statistics and finds none**
  (`prune_blocks`, `prune_block` returning `None`), so the leg is not free of
  the lookup; it is per block, and `pruning` is one block.

## Negative results

- **`--statistics none` over the bare cache was not the leg**: it prints no
  note whatever the cache holds, so a builder that silently gathered would
  publish a Δ of nothing against nothing with no refusal able to fire.
