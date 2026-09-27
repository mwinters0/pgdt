# P27.2 — Receiving and translating: notes

What the slices after this one inherit. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Scope".
Nothing is pruned: every filter a scan holds is answered `No` and read by
nothing but its tests.

## What exists

- **`PgDumpExec` holds every filter the `Post` phase pushes**
  (`datafusion-pgdump/src/exec.rs`, `handle_child_pushdown_result`), none
  twice, answers `No` for each, visits them in `apply_expressions`, keeps them
  across `with_fetch`, and drops them in `reset_state`. `EXPLAIN` prints them
  in the scan's `predicate=`, after the static filter it answers, each
  `DynamicFilter [ … ]`, `empty` until its first update. A `Pre` phase filter
  is a `FilterExec`'s static one and is not held.
- **A join now computes its filter**, its probe side's scan visiting it. So a
  `dynamic-filter-join` "on" leg taken from here on pays the producer's cost —
  the bounds, the `IN` list or the hash table's lookup — with nothing consumed;
  the figures stand at `2f94f14`, where it paid none. `--stale` holds both
  dynamic-filter figures red on this slice's paths: the join's is owed a
  re-take, its leg having moved, and the TopK's leg runs nothing new, its
  producer having kept its filter up already.
- **`dynamic_filter::loosened`** reads a filter's state now into the
  library's `Expr` over the scan's own schema, keeping every row DataFusion's
  evaluation keeps: a part with no library term is `true` beneath an even
  number of `NOT`s and `false` beneath an odd one; a `CASE` is the `Or` of its
  branches (the `And` beneath an odd number); a `NULL` literal, or a comparison
  with one, is never the reason a row is kept; an `IN` is the `Or` of its `=`
  terms. It shares its leaves with the static translator
  (`pushdown::compared`, `null_term`, `boolean_term`).
- **The harness lost `Recording`**: `tests/dynamic_filters.rs` reads each
  query's shapes off the flags-on scan's `EXPLAIN` line after the query ran,
  so the session that holds the filters is the one whose answers are checked.

## Negative results

- **Parquet keeps its dynamic predicate across `reset_state`**
  (`datasource/src/source.rs` resets only the execution state), and
  `reset_plan_states` says a plan using dynamic filters cannot be re-run. We
  drop ours: `SortExec`'s reset makes a filter nobody pushes and
  `HashJoinExec`'s discards its own, so a held one would describe the rows of
  the run before — rows a recursive query's next iteration needs.
- **A float's `IN` is a set of bits** in DataFusion
  (`physical-expr/src/expressions/in_list/primitive_filter.rs`,
  `OrderedFloat64`): `-0` is not in a list holding `0` there, where the
  library's `=` equates them. So its membership is loosened where a row needs
  it true and has no term where a row needs it false.

## Tests

- `datafusion-pgdump/src/dynamic_filter/tests.rs`, **the generated check**:
  over every major's `types`, `statistics` (both flag sets) and `edge_cases`
  fixtures, gathered at 32-byte groups, generated filters of every shape the
  producers publish and of parts with no term, each evaluated by DataFusion
  over the table and translated for the replay a scan runs; no row DataFusion
  keeps is lost, an exact filter keeps exactly DataFusion's rows, and no
  translation is refused. Four mutations each fail it: `false` beneath an odd
  parity turned `true`, a float's membership loosened beneath one, a `CASE`'s
  branches joined the other way, and a `NULL`'s parity flipped.
- `tests/dynamic_filters.rs`: printing `empty` before a run and the final
  state after, the tree rendering; no static filter held; a second `Post` pass
  holding nothing twice and a fetch keeping what is held; a reset dropping
  every filter and the reset plan answering alike. Holding a filter twice, and
  keeping it across a reset, each fail one.

## For 27.3

- **`loosened` takes the scan's own `ResolvedSchema`**, the projected one
  `TablePartitions::resolved_schema` gives, and reads a physical column by
  index; `PgDumpExec` holds no schema and no partitions.
- **Nothing asks the library's plan whether a translated term resolves.** The
  static path asks `table_schema`, which reads every block, per filter; a
  dynamic filter's terms are values of the column's own type rendered by
  `render_field`, and the generated check met no refusal, but a consumer must
  read a term its plan refuses as the term's parity says, never as a refusal
  of the query.
- **What each shape translates to**: a partitioned join's `CASE` is the `Or`
  of every partition's bounds and list, so it prunes by the whole build side;
  a join past the list's limit (`hash_lookup`) and a multi-key one
  (`struct(…) IN`) by their bounds alone; a TopK's `false` is `Or([])`, which
  rules out every group left.
- **The manual says nothing of dynamic filters yet**: what `EXPLAIN` prints is
  worth a sentence once the metrics that say what a filter did exist.
