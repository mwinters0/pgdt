# P35.2 notes — tests beside their module

What the next slices inherit from moving the inline test modules out. The
mechanism is nothing new: each `#[cfg(test)] mod name { … }` of a file past a
limit became `#[cfg(test)] mod name;` with its body whole in
`<module>/name.rs`, or `src/name.rs` beside a crate root; indentation was
`cargo fmt`'s, and nothing in a body changed.

## What 35.3 and 35.6 inherit

- **The red files are red for their production code.** `predicate.rs`'s
  total-length red is gone with its tests; what remains past a hard limit —
  `stream.rs`, `predicate.rs` by code lines, `decode.rs`, `gather.rs`,
  `io.rs` by function count — is the split the seams round owes, and 35.3's
  cuts into `model` touch four of the soft ones (`map.rs`, `preamble.rs`,
  `statistics.rs`, `cache.rs`) on the way.
- **Three `#[cfg(test)]`-gated items stayed in production files** because
  they are not modules: `stream.rs`'s cache imports, `gather.rs`'s
  `one_group_bounds` and `batch.rs`'s `column_of`, helpers other modules'
  tests import. A crate carve that separates a helper from its importer
  moves it to a `test-support` feature or to the importer's own test file.
- **A test file beside a module is `super`'s child**, so `use super::*`
  inside it reads the module's private items as the inline body did, and a
  sibling test file (`decode/prior_shape.rs`, `decode/differential.rs`,
  `batch/…`) could reach another through `super::tests`, though none does. A
  crate carve moving a module moves its directory with it.
- **`crates/pgdt/src/tests.rs` is the binary's unit tests**, beside `alloc.rs` and
  the rest, not `crates/pgdt/tests/`, which holds the integration tests; a crate
  root's `mod tests;` can resolve nowhere else without `#[path]`.
- **What named a test by its file.** `scripts/oracle_register.py`'s three
  `Evidence` rows now point at `pgtype/tests.rs` and `predicate/tests.rs`,
  and `STATUS.md`'s oracle row cites `predicate/tests.rs`. Nothing else did:
  `tests/layering.rs` reads `src/*.rs` and cuts at the declaration,
  `emitter_register.py` cuts at the first `#[cfg(test)]`, and
  `floor_mapping.py` reads production anchors.

## Negative results

- **Dedenting the moved body by hand is wrong**: a multi-line string literal
  inside a test would lose four columns of content. The body is written as
  it was and `cargo fmt` re-indents the code, leaving literals alone.
