# P35.19 notes — the predicate as one tree

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-query" tree for the predicate, its `moves` block applied whole and then
struck from that doc. Every body moved whole. `module_graph.py` finds no
cycle in `predicate/`: resolve reads notes, eval reads resolve, truths reads
both. The stream's `OWED` cycle never held a predicate module, so its entry
now names 35.20 alone.

## Where the cut fell

- **A method no move line names stayed with its type, in `resolve.rs`**:
  `ResolvedTerm`'s `comparison_notes`, `op`, `index` and `bounds_kind`,
  `ResolvedMembership::comparison_notes`, `ResolvedExpr`'s `reading`,
  `testing`, `invalid_values`, their setters and `comparison_notes`,
  `Lookup::of` and `Spellings`' methods.
- **`predicate.rs` re-exports what its readers named**: `ComparisonNote` and
  `column_divergences` for the crate root, and to the crate `ResolvedExpr`,
  `ResolvedTerm`, `resolve_expr`, `resolve_loosened` and `GroupStatistics`,
  so `stream/` and `prune.rs` kept their `crate::predicate::` paths.
- **What a sibling reads is `pub(super)`**: the types `resolve.rs` defines and
  their fields, `null_refused`, notes' refusal sentences and
  `compares_as_declared`, `ResolvedTerm::eval`, `ResolvedExpr::eval` and
  `wants_equal`, and `TruthSet::union`, which the shared test helpers read.
- **`stream/resolve.rs` keeps the block's state, the projection, the
  fingerprint, `row_text` and `render_candidate`**, all 35.20's to move.
- **A doc link from the predicate up into the stream is plain text now**
  (`plan_blocks`, `DynamicFilter`); notes' link to
  `ResolvedTerm::comparison_notes` runs against the edges and is a doc link
  only.

## Tests

- **The suite is the same 109 tests.** `eval/tests.rs` holds a row's verdict:
  each operator over a text term, Kleene's tables and the short-circuit, where
  a decode failure surfaces, a membership's answer and lookups, the row form,
  and the value path. `truths/tests.rs` holds a group's: the sets lifted from
  the tables, a group answered from its statistics and its unrepresentable
  count, a membership's group, and the keyed bounds.
- **The per-type comparison tests and the oracle stay in
  `predicate/tests.rs`**, as the design has it, the oracle's check of a
  group's truths against its rows among them; so no `resolve/tests.rs` or `notes/tests.rs` exists,
  every test of resolution or a note being a per-type one.
- **Helpers two files share are `predicate/test_support.rs`**, `#[cfg(test)]`,
  as 35.14 placed pgdt-io's; a helper one file uses went with it.

## What later cuts inherit

- **`measure.py`'s `PREDICATE` declares the four children** beside
  `predicate.rs`, the test files undeclared.
- **35.20's moves still name `stream::resolve`'s remaining items**, which this
  cut left where they were.

## Negative results

- **`tests/nested.rs` called `make_range` `predicate.rs`'s**: it has been
  pgdt-codec's since 35.17, and the comment now says so.
