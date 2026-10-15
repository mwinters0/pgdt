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

## What the next round inherits

- **The count is owed, and is taken from the commit.** It runs longer than
  ten minutes, so it is launched detached
  ([36.1.1's notes](roadmap-P36.1.1-count-reps-notes.md)). An unrelated
  commit landed on top of this one first, so the count runs from a detached
  worktree at this commit, as that worktree's own `measure.py --count-pair
  HEAD~1` with `PGDT_MEASURE_OUT_DIR` set to the repo's `runs/`. That is
  the route for any count an unrelated commit displaces. The gatherer's
  per-field path changed in two ways: `ValueKey` now wraps a two-variant
  enum, with the nested arm boxed, and `stored_order` returns an `Option`.
  The data-level `parse` row is the one that prices both. A resolved rise
  refuses the slice. Its first suspect is the enum's check on each
  comparison, which the gatherer could avoid by holding `internal`'s
  `OrderKey` instead.
- **An embedder holds no kind.** `CompareKind` still reaches an embedder
  through `ComparisonPlan::bounds_kinds` and `bounds_read_by`, the stored
  sets' names (D79). No API path needs one to build or compare a key.
