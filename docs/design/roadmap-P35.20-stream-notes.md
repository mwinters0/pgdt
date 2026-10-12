# P35.20 notes — options, render, batch and the stream

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-query" tree for everything but the predicate, its `moves` block
re-projected against this cut's parent with no cycle, applied whole, and then
struck from that doc. Every body moved whole. `module_graph.py` finds no cycle
in pgdt-query, so the stream's `OWED` entry is gone; doc links against the
edges fell from 206 to 204.

## Where the cut fell

- **A move line naming an item took its `impl` blocks and its doc**:
  `ResumeToken`'s `start`, `StreamShared`'s, `TableColumns`'s, `Cut` under
  `replay/dynamic.rs`. Items no line names stayed with their file:
  `block_runs` in `partitions.rs`, `record_unread` in `replay.rs`, the
  `#[cfg(test)]` `column_of` going with the builders it drives.
- **What the replay's children declare at `pub(super)` is now
  `pub(in crate::stream)`**, in all four of them: the scope each item had
  before, spelled from one level down. Nothing was widened past it.
- **Widened to compile, each to its module's parent**: `ResumeToken`'s
  `generation` and `InCopyResume`'s three private fields, which `snapshot`
  builds in `replay.rs`; `Cut`'s `groups` and `ruled_out`, which
  `DynamicPartitions` reads; the builders `batch.rs` and `render.rs` call,
  with `ListParts` and `StructParts`, which their variants carry; and
  `push_utf8view_field`.
- **`batch.rs` re-exports what its readers name** — the builders for
  `render.rs`, `RetainedChunks` for the replay, `column_of` for the predicate's
  tests — as `predicate.rs` does, so no reader names `batch::builder`.
- **`read_table` is the stream's**, beside `table_stream`, which it drains;
  the crate root's `pub use` lists are regrouped by module, the API unchanged.
- **Doc links name the defining module**, never a `crate::stream` re-export,
  so a link that ran with the edges still does; `rustdoc`'s broken links in
  pgdt-query are the five it had before, all in `batch.rs` and `predicate/`.

## Tests

- **The suite is the same 487 tests.** `batch/tests.rs` split along the cut:
  the builders' round trips to `batch/builder/tests.rs`, the defaults test to
  `options/tests.rs`, the batcher's own left; `prior_shape.rs` and
  `differential.rs` went to `render/`, whose walk they check.
- **`stream/tests.rs` was left whole** by this cut; the closing round split
  it along `replay/`, which narrowed six items it held wider
  ([`roadmap-P35.12-closing-seams-notes.md`](roadmap-P35.12-closing-seams-notes.md), B30).

## What later cuts inherit

- **`measure.py`'s `STREAM` names the replay's children, and `NESTED` the five
  files `batch.rs` became**, the test files undeclared.
