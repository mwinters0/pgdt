# P5.3 — Projection in the library: notes

What the next slices inherit from making a query materialize only the columns
it asked for. The spec is
[`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md); the mechanism now lives in
[`architecture.md`](architecture.md), "Projection" (plus its additions to
"Execution model and API surface" and "Resume"); what has landed is
[`../status/STATUS.md`](../status/STATUS.md).

## What is there now

`BatchOptions` is **`QueryOptions`**, with two new fields ahead of the
batching knobs — `projection: Option<Vec<String>>` and
`filter: Option<Predicate>` — and the `predicate` positional parameter is gone
from both `stream::table_stream` and `batch::read_table`.

One new function does the cutting: `stream::project(&ResolvedSchema,
Option<&[String]>, header_offset)` returns the projected `ResolvedSchema` and a
`field_targets: Vec<Option<usize>>` — one entry per field of the **block**,
naming the projected column that field feeds. `RowBatcher` takes that vector
and keys `push_row` off it; everything else in `batch.rs` is unchanged.

`ResumeToken` gains `query_fingerprint: u64`, and `stream::query_fingerprint`
computes it.

Three new `Error` variants: `UnknownProjectionColumn`,
`DuplicateProjectionColumn`, `ResumeQueryMismatch`.

## Calls made here, and why

**The predicate index is resolved against the *unprojected* schema.** A filter
may name a column the projection dropped, and `Predicate::matches` indexes into
the raw row's fields — which are the block's, not the projection's. So the
order in the two replay call sites is: `resolve_block` → resolve the predicate
index against that full schema → `project`. Getting this backwards would make
`--filter` on an unprojected column an `UnknownPredicateColumn`, which is the
one case the spec explicitly built the figure's floor row on.

**`field_count()` returns the block's width, not the projection's.** It is what
a `ResumeToken` carries so a resumed stream can rebuild a headerless block's
`column1..columnN` placeholder names; a projected width there would silently
name different columns. `RowBatcher` no longer derives it from `schema`, which
is now the projected one — it comes from `field_targets.len()`.

**`ColumnCountMismatch` is also against the block's width.** `push_row` is the
system's only field-count check, so a projection must not weaken it; `expected`
is `field_targets.len()`.

**`max_bytes` counts only projected fields.** It caps what the batch *holds*,
and a batch holds only what it built. A zero-column projection therefore never
trips it, and `max_rows` is what cuts those batches.

**`flush` states the row count for every width**, not only the zero-column one.
`RecordBatch::try_new` cannot express a zero-column batch at all; branching on
width would have given the two cases separate paths, and `try_new_with_options`
still checks the stated count against every array when there are arrays.

**The duplicate-name and fingerprint checks fire on the request**, at the top
of the stream body, before `source.size()`. A repeated name is wrong whatever
the file holds, and a resume token from another query should not be discovered
only once its first block resolves. The test pins this by asking for a table
the dump does not contain — which otherwise yields no error and no rows.

**`project` does not itself reject duplicates.** The up-front check covers
both call sites (replay and resume), and doing it per block would report the
same fault later and once per block.

**The fingerprint hashes field by field through explicit `match`es**, so
adding a `PredicateOp` variant or a `SchemaMode` is a compile error rather than
a stamp that quietly stops covering it. `database` and `scan_extent` are
deliberately outside it, per the spec's four inputs: they change which blocks
are replayed, not the shape of what comes back. `DefaultHasher`'s instability
across Rust releases costs nothing — a token is valid only within its process.

## What the next slices must not break

**`P5.4` adds `--column`/`--no-columns`.** The library side is complete: the
CLI needs only to build `projection` — `None` when neither flag is given,
`Some(vec![])` for `--no-columns`, `Some(names)` for repeated `--column`. The
CLI already passes its `--filter` through `QueryOptions::filter`.

`pgdq query` prints the batch's own field names as its header and re-reads
`stream.resolved_schema().plans` per batch, both of which are already the
projected ones — so the render path needs no change for a projection. One
detail is left for that slice rather than settled here: as the code stands, a
zero-column projection would print an *empty* header line and then one empty
line per row, so `--no-columns | wc -l` — the filtered-row-count idiom the spec
names — would report `rows + 1`. Suppressing the header line when the schema is
empty is the obvious fix and it is a CLI-output decision, which is `P5.4`'s.

**`P5.5` turns `filter` into a list.** The field is `Option<Predicate>` here
because a conjunction is that slice's contract, not this one's; making it a
`Vec` means renaming the field, extending `query_fingerprint`'s filter arm, and
turning `Active`'s `Option<usize>` predicate index into one index per term
(still resolved against the unprojected schema).

**`P5.7` takes `projection-widths`.** Its five widths are executable the moment
`P5.4` lands; nothing about this slice changes the harness's registered command
shapes.

## Staleness

This slice edits `pgdump_query/src/batch.rs`, `pgdump_query/src/stream.rs` and
`pgdump_query-cli/src/main.rs`, which between them are declared by eight
figures: `census-brace-free`, `census-arrays`, `scan-throughput-cold`,
`scan-throughput-warm`, `nested-end-to-end`, `census-attribution`,
`cross-file-floor` and `map-only`. `--stale` reads all eight stale, up from the
two `P5.2` left.

**None is acknowledgeable.** An acknowledgement says the commit provably moves
no reading; this one puts a `field_targets` lookup on the per-field replay path
and changes `flush`'s `RecordBatch` constructor. Per `CLAUDE.md`, a library
change has no cheap oracle, so all eight stay stale until the next full sweep —
which is `P5.7`'s neighbourhood, as the spec already schedules.
