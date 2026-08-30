# P5.2 — The source-span flush trigger: notes

What the next slices inherit from bounding what an in-flight batch pins. The
spec is [`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md); the mechanism now
lives in [`architecture.md`](architecture.md), "Three flush triggers, and only
one of them bounds memory"; what has landed is
[`../status/STATUS.md`](../status/STATUS.md).

## What is there now

`BatchOptions` carries a third trigger, `max_source_span: Option<usize>`,
defaulting to `Some(64 << 20)`. `RowBatcher` gains one field —
`span: Option<(u64, u64)>` — opened by the first `push_row` of a batch,
extended to `row_offset + raw.len()` by each later one, and cleared by
`flush`. `should_flush` ORs a third condition onto the two that were there.

That is the whole change to the library. The stream loop is untouched: the new
trigger goes through the same `should_flush` → `flush` → `invalidate_block_cache`
sequence the other two already do, which is what the spec required of anything
adding a trigger.

## Calls made here, and why

**The span is measured between *selected* rows, not against the scanner.** A
row the predicate rejected never reaches `push_row`, so it neither opens a span
nor extends one. Measuring against the scanner position instead would flush an
in-flight batch while the scan crossed a stretch matching nothing — a stretch
that pins nothing — turning a hard filter into one tiny batch per cap's worth
of file, which is the opposite of the fix. The rejected alternative is written
beside the mechanism in `architecture.md`.

**The span is a bound on pinning, not a measurement of it.** What is actually
pinned is the set of read chunks the builder holds a `Buffer` clone of, which
is at most the span rounded out to chunk boundaries and can be much less — two
selected rows a gigabyte apart pin two chunks and span a gigabyte. The cap is
deliberately the conservative side of that: it is the quantity a caller can
reason about without knowing the chunk size or the selectivity, and the spec
named it.

**The trigger is checked after the row, like the other two**, so a batch may
overshoot its cap by one row. Unlike the other two it cannot fire on an empty
batch whatever its cap, because `span` stays `None` until a row lands — a
`max_rows` of 0 would flush an empty batch, and `max_source_span: Some(0)`
does not.

**No CLI flag.** `pgdq query` exposes neither `max_rows` nor
`ScanOptions::chunk_size`, and the spec asks for a library option, not a
surface. The default is what protects the CLI, which is the only shipped
caller.

## Tests, and what each holds

Three unit tests in `batch.rs`, driving `RowBatcher` directly at chosen file
offsets — the triggers are arithmetic over offsets and row lengths, and
reaching a 64 MiB span through a fixture would mean a 64 MiB fixture:

- `the_source_span_trigger_fires_on_the_distance_between_selected_rows` pins
  the arithmetic, including that a flush reopens the span at the *next* row
  rather than carrying the flushed batch's end forward.
- `rows_that_were_never_pushed_do_not_widen_the_span` pins the call above.
- `a_none_source_span_leaves_a_batch_unbounded` pins the escape hatch.

Two integration tests in `tests/batch.rs`, over the 132-row
`public.escapes` fixture with the other two triggers disabled, so the split
points are the span's alone. `max_source_span_splits_batches` sweeps five caps
against three chunk sizes and asserts the decoded rows never move — the small
`chunk_size` is the point, because it forces a flush to land between two views
into the same chunk, which is exactly what `invalidate_block_cache` exists for.
`a_none_source_span_leaves_the_block_as_one_batch` is the negative control.

## What the next slices must not break

**`P5.3` changes `BatchOptions` into `QueryOptions`.** `max_source_span` moves
with the rest of the struct and needs nothing else.

**A projection does not change the span's meaning.** The span is over the raw
row, which `push_row` keeps walking whole whatever the projection is — so the
bound stays sound when only some columns take views. What *shrinks* under a
projection is the number of columns pinning a chunk, never the set of chunks.

## Staleness

This slice edits `pgdump_query/src/batch.rs`, which `nested-end-to-end` and
`cross-file-floor` both declare, so `--stale` reads both stale.

**It is not acknowledgeable.** An acknowledgement says the commit provably
moves no reading, and this one adds a branch and two `u64` writes to
`push_row`, which is the per-row replay path both figures measure. Per
`CLAUDE.md`, a library change has no cheap oracle — `--verify-additive`
settles generator changes only — so both figures stay stale until the next
sweep. That is the designed outcome, not an omission.
