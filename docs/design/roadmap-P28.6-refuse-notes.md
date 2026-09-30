# P28.6 — The refuse mode: notes

What the slices after this one inherit. The spec is
[`roadmap-P28-unrepresentable-values.md`](roadmap-P28-unrepresentable-values.md),
"Scope"; the decision is `decisions.md`, "D99", and the key's is "D97" with
`postgres-invariants.md`, "I49".

## What exists

- **`ReplayPlan::new` refuses under `UnrepresentableMode::Refuse`**, after the
  blocks resolve and before any is pruned, naming the first materialized
  column whose blocks count a value in the query's tiers
  (`stream::materialized_unrepresentable`, which the null mode's
  `ReadAsNull` notes are now built from too). Every plan — `pgdt query`, the
  library's `table_stream*`, the provider's `TablePartitions::plan` — passes
  through it, so the provider refuses in `TableProvider::scan`, at physical
  planning.
- **`Error::Unrepresentable` is the planning refusal**: `{ table, column,
  declared_type, values }`, the count the map's over every block in the
  tiers read. It names the null mode and leaving the column unmaterialized,
  **not the untyped mode, which does not exist yet**: 28.7 adds it to the
  sentence (`error.rs`) and to `pgdt`'s and the shell's help.
- **Nothing refuses a value of the type at read time any more.** Under the
  refuse mode `unrepresentable_reads` hands every column `None`, so the
  batcher and a filter leaf raise only `Error::FieldDecode`, on text that does
  not parse; `UnrepresentableRead` lost its `null` flag and `past`.
- **A timestamp keys from PostgreSQL's epoch**
  (`decode::timestamp_postgres_micros`, `render_timestamp_postgres_micros`
  for an equality's canonical text), so a literal or field up to
  `294276-12-31 23:59:59.999999` compares, and every value's view bounds a
  group holding one. No `CACHE_FORMAT_VERSION` bump: a cache gathered before
  leaves those groups unbounded and the column's every-value order
  `Unsorted`, which prunes less and answers the same.

## What the next slices inherit

- **The harness's refuse mode is green in every case**; the untyped mode still
  opens it, so `COUNT(*) … WHERE v_tstz < …` now passes under `Text` too and
  was struck from its record. 28.7, opening the real untyped mode, may find
  it failing again and re-records it.
- **The generated pruning check asserts that no query over a fixture raises**
  (`pgdump_query/tests/pruning.rs`, `check`), its error leg gone with its
  population until M184 restores it over `KD2`'s shape.
- **The untyped mode's widening is a plan fact beside this refusal**: the same
  per-column count over every block decides which columns widen, and
  `materialized_unrepresentable` is where both read it.

## Negative results

- **A read-time refusal kept as a fallback buys nothing**: the count is the
  map's exact record (D96), every planned block carries one, and the one path
  resolving a block the plan does not hold (`activate`'s, a header the map
  does not match) is a source that changed under its map.
