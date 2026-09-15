# P20.1 — The statistics account: notes

The account is `statistics::StatisticsAccount`, read whole as
`MapRun::statistics`; why it is shaped that way is
[`decisions.md`](decisions.md), "D81", and the table layout it rests on is
[`runtime-invariants.md`](runtime-invariants.md), "RT11". The instrument that
checks it is `pgdump_query::instrument` behind the library's `introspect`
feature, which `pgdump_query-cli`'s own `introspect` feature turns on; its
readings are the `statistics_*` lines of that build's report.

## The reconciliation

`pgdump_query-cli/tests/statistics_account.rs` is the reading and the check at
once: two generated shapes, a table of reasonable width and one of sixty-four
text columns, each through a serial leg at a stated group size, the same split
across four workers, a back-fill of the first leg's cache, and a flagless leg.
Its tolerance is in its module doc and was written before the first run. Every
leg passes; the sitting's readings, and what the runs before them found, are
`runs/statistics-account-20260915/readings.txt`.

`pgdump_query/tests/statistics_heap.rs` holds the retained and loaded terms
exactly, per block, under `cargo test --workspace`: a block's reported heap is
what dropping it frees, gathered or decoded, and nothing is left charged to a
gathering term once a pass returns, split or not.

## Negative results

- **Charging a close's growth when the close ends fell short at the peak.** An
  interning map rehashes by allocating its larger table while the smaller is
  still live, and several columns' maps grow inside one close on a
  high-cardinality table: the reasonable shape's serial leg read the account's
  peak below the heap's past the tolerance. A map about to grow is now charged
  its larger table before the insert.
- **Updating once per group close left a split wide table near the floor**, and
  differently on two runs of the same input, each piece holding a whole
  close's growth uncharged. The account now moves column by column.
- **The instrument first read a decoded cache as live after it was freed.**
  `pgdq parse` decodes the whole cache once to claim it, before `map_file`
  loads it again, and dropped that copy outside any scope. A block's
  statistics now drop inside one wherever they are dropped. That claim-time
  decode is a real transient of every loaded statistic ahead of the pass, not
  concurrent with the pass's own load.
- **A back-fill at twice the group size gathered almost nothing** on the
  reasonable shape: a group twice as wide holds more distinct texts than a
  dictionary keeps. The leg re-reads at half the size instead.

## What the next slices inherit

- **The merge (20.3, 20.4) moves capacities the account reads.** Every charge
  is recomputed from the structures (`ColumnGatherer::held`,
  `Gatherer::charge_held`), so a merge that rebuilds a block's vectors is
  charged by the next update — but a merge allocating its coarser vectors
  beside the finer ones is a transient of both, which the account sees only if
  it is charged before the allocation, as the map growth is.
- **A block's interning maps can outweigh what the block retains.** On the
  reasonable shape the interned term's peak stood above the retained one
  (the readings' term peaks), so a gathering block holds its statistics plus
  its dictionaries' second copy until it finishes. The decline (20.7) reads
  gathering and interned together, never a retained estimate.
- **What the account does not see**, each named in its rustdoc: a column's open
  group before that column's first close, a row's decode scratch, a save's
  encode buffer and a load's file bytes. The spec's account a decline reads
  includes the save's buffer; nothing charges it yet.
- **Only a mapping pass keeps an account.** A `query` loads the cache whole and
  charges nothing, and the report's `statistics_*` lines follow only a `parse`;
  the reserve's `query` legs (20.8) need a term of their own, the walk being
  `stream::statistics_heap`.
- **What the account costs a gathering `parse` is unpriced.** Every column's
  close now walks its open group's distinct texts twice; `statistics-gathering`
  is re-taken in 20.9, and `measure.py --stale` names it red until then.
