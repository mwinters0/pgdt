# P10.2 — Row-free comparison and plan-time filter resolution: notes

What the later P10 slices inherit from this one, and what it did not do. The
spec is [`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"The consumer ships in this phase". Nothing a query returns changed; the whole
suite passed unedited apart from the tests added.

## Row-free comparison (`predicate.rs`)

- **`ResolvedTerm::eval_value(Option<&str>) -> Option<Truth>` is the row
  path's whole comparison**, and `ResolvedTerm::eval` is now field extraction
  and unescaping in front of it. A stored bound or dictionary entry answered
  through it cannot be answered differently from the same value in a row.
  `None` back is exactly the row path's `Error::FieldDecode`; the NULL tests
  and the `Canonical`/`Trimmed` equalities never return it.
- **For the four ordering operators the answer is monotone in the value**, so
  a group's `min` and `max` through `eval_value` settle whether `True` and
  `False` are reachable. Equality over bounds is not reachable that way: it
  needs the literal's key placed between the bounds' keys, and
  `Comparison::Canonical`/`Trimmed` carry no `CompareKind`. The kind is the
  column's `ComparisonPlan` in the block's **unprojected** `ResolvedSchema`,
  which the plan does not keep today (below).
- **`order_key` and `compare_keys` are still private to `predicate.rs`**, and
  their row-freeness is now stated on them and tested rather than incidental:
  `the_key_is_a_total_order_over_every_committed_value` asserts reflexivity,
  antisymmetry and transitivity over every output value of every scalar-compared
  declared type in all six majors' `oracle/literals.tsv`. The slice that first
  calls them from outside the module widens them, and `OrderKey`'s variant
  payloads (`NumericKey`, `NetworkKey`, `Jsonb`) are private types, so that is
  more than one keyword.
- `a_value_outside_any_row_is_answered_as_the_row_holding_it_is` covers every
  `Comparison` arm and both NULL-counting families against the row path.

## Plan-time resolution (`stream.rs`)

- **`ReplayPlan::new` runs `plan_blocks`**, which resolves every matched
  block's schema, filter and projection into a `PlannedBlock` keyed by
  `header_offset`, before a segment is cut — in `table_stream` before the
  segment list, in `table_stream_partitions` before `plan_partitions`. That
  ordering is what the pruning consumer needs, and is kept deliberately.
- **`activate` is the one activation path**, the resumed token included
  (`resume_state` calls it). It takes the planned entry only when the header,
  field count and database match the activation, and resolves for itself
  otherwise; the entry's filter is an `Arc` shared by every piece of the block.
- **One kind of block has no entry, and may not be pruned**: a block whose
  header names no columns, whose field count only a row says, so reading it is
  what learns its field count or raises its refusal in place. Every other
  block's schema, filter or projection refusal refuses the whole plan, since
  `M106` and `M107`
  ([`../status/history/2026-09-14.md`](../status/history/2026-09-14.md), "A
  block's filter refusal moves to the plan").
- **A `PlannedBlock` holds the projected schema only.** The truth-set evaluator
  needs each term's column comparison from the unprojected one, so it adds that
  field (or the kind per term) rather than re-resolving.
- Pinned by `stream.rs`'s `the_filter_is_resolved_once_per_block_at_plan_time`
  (the entries, the shared `Arc`, the fallback) and `tests/stream.rs`'s
  `a_later_blocks_refusal_is_raised_before_any_row` (serial and partitioned).

## Negative results

- **A refusal is not raised from the plan** here, the slice being
  behaviour-preserving; reviewed, it moves there under `M106` and `M107`, ahead
  of the slices that extend `PlannedBlock`, so 10.3 inherits a plan that
  resolved every block with a column list or refused.
- **`Error` is not `Clone`**, so the plan cannot hold a refusal to hand out on
  activation; a refusing block is resolved a second time instead, resolution
  being a function of the block and the plan alone.
- **No `compare_texts` wrapper was added**: nothing outside a test calls one yet,
  and an unused crate-visible function is a lint failure.
- **Resolution now runs for every matched block up front**, including blocks
  before a resume token's offset and blocks a caller never polls. It is per
  block, not per row; no figure was taken.
