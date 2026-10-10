# P35.7 notes — the ordering keys lifted into the codec

What the next slices inherit from moving the ordering keys and the
field-grammar checks out of `predicate.rs`. The module is `src/key.rs`,
assigned to codec in `tests/layering.rs`, and the reasoning
[`decisions.md`](decisions.md), "D68"; the move is whole, every body as it
was, and nothing persisted changed.

## Where the cut fell

- **`key` holds everything that reads a text into a key or compares two**:
  `OrderKey` and `NestedKey` with the readers behind them (`field_key`,
  `order_key`, `read_key`, `literal_key`, `nested_key`, `checked_key`), the
  range canonicalization, `compare_keys` and `compare_nested`, `ValueKey`,
  and the strict checks `field_refused` and `grammar_refuses`.
- **The literal grammar's description went with it**: `accepted_form` and
  its helpers say what `literal_key` reads, and their rustdoc places them
  beside that grammar; `render_macaddr`, rendering through `macaddr_key`,
  went too.
- **`equality_comparison`, `Spellings` and `Comparison` stayed in
  `predicate`**: they build a filter term's comparison, which is query's.
  `key`'s rustdoc names `crate::predicate::equality_comparison` in prose,
  never as a link, so the carve breaks no doc link.
- **Seven tests moved to `key/tests.rs`**, the ones calling nothing but
  `key` and the codec: the strict-check and field-refusal tests and the two
  accepted-form clause tests. Every test driving a `Predicate` stayed in
  `predicate/tests.rs`, the oracle module included.

## What 35.4 inherits

- **What query reaches in `key` is its `internal` API**: `OrderKey`,
  `NestedKey`, `Side`, `ValueKey` and its methods, `order_key`, `read_key`,
  `literal_key`, `nested_key`, `compare_keys`, `compare_nested`,
  `special_order_key`, `int_in`, `int_range`, `render_macaddr`,
  `accepted_form`, `nested_accepted_form`, `field_refused` and
  `grammar_refuses`. `NumericKey`, `NetworkKey`, `Jsonb` and `RangeKey` are
  `pub(crate)` only because a variant of `OrderKey` or `NestedKey` names
  them (`private_interfaces`); the three structs' fields stay private, and
  nothing outside `key` matches a key's variants.
- **`predicate/tests.rs`'s oracle module reaches `key` too** (`order_key`,
  `compare_keys`, `OrderKey`, `field_refused`), through the same `internal`
  path.
- **`measure.py`'s `PREDICATE` names `key.rs`** beside `predicate.rs`, so
  `STATISTICS` and the `predicate-terms` figure declare it; the carve
  repoints both paths.

## Negative results

- **No cache byte moved**: a key is never persisted, a stored bound being
  its text (D74), so nothing here bumps `CACHE_FORMAT_VERSION`.
