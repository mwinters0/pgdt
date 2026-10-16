# P36.4 notes — a key for every value

`pgdt_codec::key::ValueKey` is read from a column's `ComparisonPlan` in a
`ComparisonSemantics`: `ValueKey::of` for a field, `ValueKey::literal` for a
filter's literal. A nested value has a key, and two keys that do not share a
representation compare as `None`. The dispatch beneath it lives in the
`pub(crate)` child `key::order`, and only `internal` re-exports it, so
`KD116` and `KD117` are struck. Each item's rustdoc says what it answers.
`key/tests.rs` asserts the API. The dispatch's own tests moved with it, to
`key/order/tests.rs`.

## What landed beside the spec's words

- **The kind a plan is keyed by in a semantics is the kind a filter's term
  compares by.** PostgreSQL's semantics uses the register's kind, a
  divergent one included. DataFusion's uses `datafusion_order`, and a plan
  the register gives no order is read as its text. A nested plan is keyed
  only in PostgreSQL's semantics, and only where every position is ordered
  (`ComparisonPlan::orders`). Everything else has no key. This matches what
  `predicate/resolve.rs` builds a term from, so a summary's extremes and a
  filter order alike.
- **A field is read as `PostgresInvalidValues::Default` reads it**: through
  the typmod, and refused where `*_in` refuses. A nested field is read in
  the strict `*_out` grammar, so `{ 1 , 2 }` is a literal and not a field.
- **"Of different kinds" means of different representations.** A key does
  not carry the plan it was read from. Two keys whose plans share a
  representation, such as a `date` and an `integer` (both `Int`), compare as
  that representation orders, and the infinities and `NaN` compare against
  any key by rank. That falls short of the spec, as `KD118`, which 36.4.1
  closes.
- **`stored_order` answers `Option` too**, since it refines `compare`. The
  gatherer's row order reads a `None` step as a lost order, as its bytewise
  path already does. Two keys of one set never produce one.
- **The gatherer keys by its sets' kinds** through `internal`'s
  `ValueKeyExt` (`keyed_by`, `field_keyed_by`), not through `ValueKey::of`,
  which D79's Rejected line explains. `pgdt-query`'s summary and
  `partition_orders` key through the plan.
- **A nested key's comparability is checked before it is compared**
  (`value::nested::key::comparable`), so the range algebra and
  `compare_nested` are unchanged, and the predicate still calls
  `compare_nested` with its precondition. The scalar comparison has one
  match, `key::order::order_keys`, which answers `None`. `compare_keys` is
  that match with the precondition asserted.
- **`NestedKey::heap_bytes`** counts each vector's capacity and what its
  slots hold, the same way `OrderKey`'s count does. `ValueKey::heap_bytes`
  adds the box.
- `scripts/measure.py`'s `PREDICATE` mechanism declares `key/order.rs`
  beside `key.rs`.

## The count: a rise on two rows, refusing the slice

`measure.py --count-pair HEAD~1` from a detached worktree at `0588c9e8`,
`a7b6ef0e` its parent (`runs/count-pair-36.4-20261015-2048/log`, its
`HANDOFF.md` beside it). It exited 0 and printed **2 of 5 rows resolved,
both rises**, so the slice is refused (spec, "What a hot-path slice
owes"):

| Row | parent, fewest–most | tree, fewest–most | median change |
|---|---|---|---|
| `parse`, data level | 38,505,290,290–38,505,354,688 | 38,998,654,072–38,998,764,578 | +493,303,174 (+1.281%) |
| `pgdt sql` | 21,976,066,601–21,976,371,012 | 21,982,318,314–21,982,413,268 | +6,263,374 (+0.029%) |

`parse` at the metadata level and both `query` rows are unresolved, and
flat to within 60,000. Both rises reproduce outside the container: the
pair's two binaries, run on the host under `perf stat`, differ by
+493.3 M and +6.0 M, each to within 0.5 M across two reps. The data-level
pair's wall clocks are equal.

**The data-level rise is 36.4's own code**, in the gatherer functions
its key code is inlined into, the accounting's `heap_bytes` above all
([36.4.3's notes](roadmap-P36.4.3-callgrind-notes.md), an exact count). A
sampled per-symbol profile cannot place a rise of this size: one taken
here (`runs/profile-36.4-count/`) put most of it in unchanged decode code,
which was sampling error
([36.4.2's notes](roadmap-P36.4.2-attribution-notes.md)). The `pgdt sql`
row's 6 M is 60 samples, below what a sampled profile resolves; nothing it
plans or scans builds a `ValueKey` per row, so it is unattributed.

## What the next round inherits

- **The slice is refused, and the rise is its own code**
  ([36.4.3's notes](roadmap-P36.4.3-callgrind-notes.md)). It is reworked
  where a shape as clean recovers it and accepted where none does (spec,
  "What a hot-path slice owes").
- **A count an unrelated commit displaces is taken from a worktree** at the
  slice's commit, as that worktree's own `measure.py --count-pair HEAD~1`
  with `PGDT_MEASURE_OUT_DIR` set to the repo's `runs/`.
- **An embedder holds no kind.** `CompareKind` still reaches an embedder
  through `ComparisonPlan::bounds_kinds` and `bounds_read_by`, the stored
  sets' names (D79). No API path needs one to build or compare a key.
