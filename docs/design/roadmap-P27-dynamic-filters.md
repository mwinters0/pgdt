# P27 — DataFusion's dynamic filters

What this phase will do and why; how it lands is its slices'. Progress is
`STATUS.md`'s checklist, never this file. **Its first slice exists to produce
evidence** — the harness that proves no row is lost and the figures that price
what the phase buys — so slice numbers after it are allocation order as much
as schedule. Grilled 2026-09-26.

## Premise

**A hash join's build side, a TopK's heap and an ungrouped `MIN`/`MAX` each
publish a filter DataFusion 55 pushes into the scan below them at run time, and
`PgDumpExec` accepts none**, so a selective join reads all of its probe table
and an `ORDER BY … LIMIT` reads every group, though the statistics already held
would rule most of them out. **No new statistic is gathered**: the phase moves
pruning from the plan into the replay's stream, over the bounds, dictionaries
and row order a cache already holds. Where a filter's shape needs a statistic
we do not keep — a `hash_lookup`, an `IN` over a column with no dictionary —
it prunes nothing here; the membership statistic that would answer it is
P26's bloom filter (`roadmap.md`, "P26 — Statistics refused on their cost,
reconsidered").

## Scope

**The library's evaluator reads a dynamic filter, never `PruningPredicate`.**
A filter is translated into the library's `Expr` in Arrow semantics, as a
static one is (`decisions.md`, "D88"), and what does not translate stands as
`true` wherever that only loosens it. A wrongly pruned group is rows the join
or the TopK never sees, so one engine — the one already checked against
DataFusion — decides what is skipped, over the bound sets Arrow orders by
("D79"). `PruningPredicate` is refused: it would need every group's bounds
decoded to Arrow scalars and pruned in DataFusion's semantics rather than the
ones they were gathered under, and it prunes nothing on a partitioned join's
`CASE` or an `IN` list past 20, and stops no sorted block.

**Consuming a filter means three things**, the leaf answering `No` to all of
them, since every producer re-checks its own rows:

1. **Re-pruning between groups**: a replay re-reads the filter at each group
   boundary where its generation has moved, and skips the groups it now rules
   out.
2. **An early stop at a tightened bound**: a sorted block ends at a TopK's
   threshold as it ends at a static filter's (`prune::SortedStop`).
3. **Row-level evaluation before decode**: a row the filter rejects is dropped
   before its other projected columns are decoded. **It is priced by a figure
   before it is on by default** — an `IN` of up to 150 values evaluated per
   row is its cost — on inputs where group pruning helps nothing and where
   the filter rejects nothing (below).

**A dynamic filter enters the library as a trait, handed to the partitioned
replay alone.** L4 defines it and the provider implements it (`decisions.md`,
"D74"): it answers the current library `Expr` and a generation, the provider
snapshotting and translating only when DataFusion's generation moves, and the
library re-resolving per block as it resolves a static filter. **Its contract
is looser than a filter's and is stated on it alone**: a replay may drop any
row it rejects, at any moment, so what a stream emits lies between the static
filter's rows and those the filter's final state keeps, and depends on timing.
So it is not a `QueryOptions` field — every resumable entry point would have
to refuse it, a resume token counting rows being meaningless under it — and
`TablePartitions::stream`, which never resumes, is the one place it is taken
(the reasoning of "D77": no entry point handed an option it ignores).

**The byte cut is made at the first poll, not at `scan()`.** Planning — the
statistics DataFusion reads, the static filter's pruning, the plan notes —
stays at `scan()`; the cut into sub-streams is made once, for every partition,
when the first of them is polled, over the groups the static filter and the
dynamic filter's state at that moment both keep. A join's filter is complete
by then, so "D51"'s byte balance holds over what is actually read rather than
over groups a selective join on a clustered key leaves to one partition; a
TopK's is still `true`, so its cut is today's. What DataFusion was told at
planning stays true: rows and bytes are bounds ("D89"), and the filter only
narrows them.

**`decisions.md`'s "D53" stands.** An `IN` is an `Or` of `=`, as the static
translator builds it; a join's bounds and a TopK's chain are comparisons,
`And`, `Or` and `IS [NOT] NULL`; a partitioned join's `CASE` is the `Or` of
its branches; `hash_lookup` and `struct(…) IN` are `true`. Pruning needs no
new operator. The entry's **Reopens** is a set-membership term, should the
row-level figure show the per-row `Or` to be what costs.

**What a dynamic filter did is read off EXPLAIN and the plan node's
metrics**, never the plan-note sink, which hears what planning settles and
this all happens after it. `PgDumpExec` prints each filter it holds as
Parquet does, `DynamicFilter [ … ]`, `empty` before its first update; the
groups a dynamic filter pruned are counted apart from
`row_groups_pruned_statistics`, the rows row-level evaluation dropped have a
count of their own, and an early stop at a dynamic bound reports into
`bytes_unread_early_stop` per block as a static one does (`decisions.md`,
"D80").

**`KD45` is not this phase's.** Its premise, that a dynamic filter over an
enum needs bounds kept in label order, is refuted below; what remains is the
statistics hand-over, admitted as `M173` (`out-of-band.md`).

## Evidence

**DataFusion's own per-producer flags are the only lever**
(`enable_{join,topk}_dynamic_filter_pushdown`): one binary, off against on.
Row-level evaluation is priced on two kinds of input — **where it pays**, a
selective join on an unclustered key and a TopK over an unsorted column,
neither of which group pruning helps; and **where it costs**, a join whose
`IN` rejects no row, so the checks and the per-row `Or` are pure overhead. No
`pgdump.*` switch and no build made only to be measured: a loss on the costing
input reopens the question as a switch against "D53"'s set-membership term,
under "Two tunables fit pgdt to hardware".

**No row may be lost, and two checks say so before anything is on by
default**: every join and TopK query shape is run over the fixtures with the
flags on and off and the results compared — enums, floats and `NULL` keys, a
partitioned join's `CASE`, a multi-key join's `struct IN`, a build side past
the `IN` list's limit; and a generated check beside `tests/pruning.rs`'s
asserts that the translation only loosens — every row DataFusion's evaluation
of the physical expression keeps, the translated library `Expr` keeps.

**The figures are timed on the shipped `datafusion-cli-pgdump`**, run with
`-c "<sql>"` over generated dumps, warm: a probe table with a clustered and an
unclustered key, a small build table, and an unsorted column for the TopK.
`measure.py` gains that binary as a second timed program, with what it asserts
of it in `test_measure.py`; the figures, `dynamic-filter-join` and
`dynamic-filter-topk`, sit in a `measurements.md` section beside "What
row-group statistics buy a query". Until this phase no figure times a
DataFusion query.

## Slices

**Evidence first, then the receiving end, then each rework of a tested core
path on its own**, so each slice's mistakes show in the next one's checks:

1. **The evidence.** The result-equality harness over fixtures holding every
   shape above — an aggregate's included — and the two figures and the costing
   input, taken on today's code, whose "on" leg is DataFusion's own filtering
   with no help from the scan. No product code.
2. **Receiving and translating.** `PgDumpExec` holds every filter pushed to it
   from any producer, visits it in `apply_expressions`, resets it in
   `reset_state` and prints it in EXPLAIN; the physical-to-library translator
   lands with the generated check that it only loosens. Nothing is pruned yet.
3. **The library trait and re-pruning between groups**, with the early stop
   at a dynamic bound and the new metrics — a rework of the replay loop.
4. **The byte cut at the first poll** — a rework of `TablePartitions`'
   planning.
5. **Row-level evaluation**, the figures re-taken; they decide whether it
   ships on, and what they say is filed against "D53"'s **Reopens**.

## Facts found while grilling

Facts found while grilling, each checked against the code or DataFusion
55.1.0's source (the release the workspace pins); upstream paths are relative
to its tree.

**The hook, and what a leaf must do to be reached.**

- Dynamic filters are pushed only in the `Post` filter-pushdown phase. A leaf
  receives them as `parent_filters` in `handle_child_pushdown_result`, whose
  default declines all (`physical-plan/src/execution_plan.rs`, the default
  impls; `filter_pushdown.rs`, `ChildFilterPushdownResult::all`).
- **Whether the leaf answers `Yes` or `No` changes nothing a producer does in
  `Post`**: a hash join, a TopK and an aggregate always do their own work.
  Parquet holds the filter while answering `No` unless `pushdown_filters` is
  on (`datasource-parquet/src/source.rs`, `try_pushdown_filters`).
- **A leaf holding a filter must visit it in `apply_expressions`**, now a
  required method: `HashJoinExec` computes its filter at `execute` only if its
  `expression_id` is found in the probe subtree that way
  (`joins/hash_join/exec.rs`, `plan_contains_expression_id`), and an
  aggregate drops its filter at optimization otherwise. `PgDumpExec`'s visits
  nothing today (`datafusion-pgdump/src/exec.rs`, `apply_expressions`).
- **The leaf's `execute` runs before a join's filter exists**: the join calls
  the probe's `execute` first, and polls it only once every build partition
  has reported, at which point it calls `update` and `mark_complete`. So a
  join's filter is final at the probe's first poll.
- Each producer has its own flag, all `true` by default:
  `optimizer.enable_{topk,join,aggregate}_dynamic_filter_pushdown`.

**What each producer publishes.**

- **Hash join**: `bounds AND membership` over the probe keys. The bounds are
  `k >= min AND k <= max` per key. The membership is an `IN` list when the
  build side has at most `hash_join_inlist_pushdown_max_distinct_values` (150)
  distinct keys within `hash_join_inlist_pushdown_max_size` (128 KiB) per
  partition, and a `hash_lookup` otherwise. Several keys give
  `struct(k1, k2) IN (…)`. A `Partitioned` join publishes one
  `CASE hash_repartition(keys) % N WHEN p THEN <partition p's filter> … END`;
  a `CollectLeft` join publishes the bare conjunction. A null-aware or
  null-equal join prepends `k IS NULL OR …`
  (`joins/hash_join/shared_bounds.rs`).
- **TopK** (`SortExec` with a fetch): the lexicographic
  `c1 OP v1 OR (c1 = v1 AND c2 OP v2) OR …`, strict, `OP` being `<` ascending
  and `>` descending, with `IS NULL`/`IS NOT NULL` arms placed as the sort's
  NULL ordering requires (`topk/mod.rs`, the filter builder). It starts at
  `true`, **tightens while the scan streams**, and is complete only once the
  input is exhausted.
- **Aggregate**: `c < cur_min OR c > cur_max` for a `MIN`/`MAX` over a bare
  column with no `GROUP BY`, tightening per batch and never marked complete.
  A dictionary column's argument is coerced to its value type, so an enum
  gets none (`functions-aggregate/src/min_max.rs`).

**Reading one.** `DynamicFilterPhysicalExpr::current()` (or
`snapshot_physical_expr` over a tree) gives the expression now;
`snapshot_generation` changes on each update, and
`DynamicFilterTracking::classify(…).changed()` polls for change without
blocking. Waiting for completion can hang forever — a TopK completes after the
scan, an aggregate never does.

**Parquet's consumer is the model.** It re-prunes files at open, row groups at
each row-group boundary when the filter changed, and ends a file early after
any batch once the filter rules the rest out — rebuilding its
`PruningPredicate` only on a changed generation
(`datasource-parquet/src/opener/mod.rs`, `push_decoder.rs`,
`pruning/src/file_pruner.rs`).

**Translating one.** No converter from a `PhysicalExpr` back to a logical
`Expr` exists upstream. `PruningPredicateBuilder` runs over any
`PruningStatistics` and snapshots dynamic filters itself, but treats `CASE`,
`hash_lookup`, `struct(…) IN` and an `IN` list longer than
`max_in_list_size` (20) as `true` — so it prunes a partitioned join on
nothing, and a `CollectLeft` one on its bounds alone past 20 build keys
(`pruning/src/pruning_predicate.rs`, `build_predicate_expression`).

**A dictionary column is compared by value** in every filter above: a join's
bounds are `Utf8` literals against it, a TopK's threshold a
`ScalarValue::Dictionary`, and arrow's comparison kernels unwrap either side.

**An enum's label-order bounds already exist per group.** Gathering keeps
Arrow's order beside a register order it differs from (`decisions.md`, "D79";
`pgtype.rs`, `ComparisonPlan::bounds_kinds`), and an enum's Arrow order is its
label text (`CompareKind::arrow_order`). Pruning in Arrow semantics reads that
set, and the provider's table summary is computed in Arrow semantics
(`datafusion-pgdump/src/statistics.rs`, `table_statistics`). So **a dynamic
filter over an enum prunes on the bounds already kept**, and what `KD45`
describes is only the hand-over of its `MIN`/`MAX`.

**EXPLAIN shows a held filter only if the leaf prints it**: Parquet appends
`predicate=… DynamicFilter [ … ]` in its `fmt_as`.
