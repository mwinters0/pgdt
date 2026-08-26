# Phase 4.5 — The shape census, recording half: notes

What 4.5.1 inherits. Every full scan now records, per `COPY` block and per
column, the range of array dimensionalities its values carried and whether any
carried an `[lb:ub]=` prefix. **Nothing consumes it** — resolution is
untouched, and the optimistic path still refuses a multi-dimensional value
exactly as it did. The mechanism is in
[`architecture.md`](architecture.md), "The array shape census"; these are the
rest.

## Why this is half a slice

The spec wrote the census as one row. It is two, and the seam is the one the
spec itself names for 4.3/4.4: recording is new, type-blind, L1-only machinery
that nothing reads, while consuming it retypes an already-tested resolution
path and rewrites `tests/decode.rs`'s pinned refusal. The spec's slice table
now carries `4.5.1` as an earned third level, and
`docs/status/history/2026-08-26.md` holds the reasoning.

## The three decisions this slice had to make that the spec did not settle

**Dimensionality is the leading brace run, and that needed a new invariant.**
The spec says the census records a dimension count; it does not say how one is
read out of a literal. The obvious answer — decode the field and walk it with
`nested.rs` — is L2, needs the field unescaped first, and is far more code
than the problem. I25 (added this slice, verified against v13–v18 source and
observed live) says an `array_out` literal opens with exactly `ndim` braces
and force-quotes any element containing a `{`, so **the leading run is exact**
and `{"c{d}"}` is unambiguously one-dimensional. That collapses the whole
question to counting bytes at the front of a still-COPY-escaped field, which
is L1 work with no dependency on `nested.rs` at all.

**A census belongs to a block, not to a file.** *(Per-block storage stands.
The rest of this paragraph — the `ScanExtent::Full` gate and the `Option` it
forced — was reversed in 4.5.1: every mapping pass censuses, `array_shapes` is
a plain `Vec`, and there is no second test for a consumer to make. See "What
4.5.1 inherits" below and
[`roadmap-phase4.5.1-census-consumption-notes.md`](roadmap-phase4.5.1-census-consumption-notes.md).)*
The spec's believability rule is file-level (`is_complete`). That is necessary
but not sufficient: `map_forward` splices new spans onto a prefix it does not
re-read, so a file mapped by a cold query and *then* by a `ScanExtent::Full`
one ends up complete while its early blocks were never censused. So
`array_shapes` is an `Option` per block — "not censused" and "censused, saw no
arrays" are different answers — and **4.5.1 must check both** the block's own
`Option` and `is_complete`. This is not a wrinkle to tidy away later: a
block's bytes are read once, and there is no second visit at which it could
acquire a census.

`tests/query_cache.rs::a_query_built_index_tiles_in_every_cache_state` is
where this surfaced. It asserted a fully-mapped query's spans equal
`build_index`'s span for span; they now legitimately differ in this one field,
so it compares through a `without_census` normalizer that says why.

**Only a scan that will reach EOF censuses** — *reversed on 2026-08-26, and
4.5.1 undoes it.* As landed, `build_index` and `build_map` always census and
`map_forward` only under `ScanExtent::Full`. The premise was that a cold query
would otherwise pay per-row work for a census `is_complete` then discards; in
fact the cold query already receives every row it would need, so the saving is
the pre-filter alone, and the cost is a permanently uncensused prefix in any
dump mapped by a cold query before a full one. `docs/status/history/2026-08-26.md`,
"The census is unconditional", carries the whole argument.

## Non-obvious calls

**`build_map` censuses too, and had to.**
`tests/map.rs::build_index_spans_match_build_map_exactly` pins the two
producers to one implementation. `build_map` always scans to EOF, so
censusing is correct for it on the merits as well.

**The row pre-filter is where the performance argument lives.** A row with
no `{` and no `[` cannot hold an array literal, so it is rejected after one
pass over its bytes with no field splitting. Measured free at the resolution
available on the 3 GiB `COPY` control, which holds no braces at all — the koji
shape. [`measurements.md`](measurements.md), "The array shape census costs
nothing on brace-free data", carries the table and states that scope limit
explicitly: **the cost on array-bearing rows is not measured yet**.

**The census is type-blind and runs over every field**, because `map::Builder`
is L1 and cannot know which columns are arrays. A `json` column's `{…}`
therefore records a depth that nothing will ever read, since `json` does not
resolve to a list. That is correct rather than sloppy — filtering by type
would put an L2 conclusion in L1 — but it means **a recorded `dims` is not
evidence that a column is an array**, only that its text looked like one.

**`observe` saturates at `u8::MAX`, and `MAX_ARRAY_DIMS` is the real bound.**
PostgreSQL's `MAXDIM` is 6, so a run longer than that did not come from
`array_out`. 4.5.1 must treat `max > MAX_ARRAY_DIMS` as unusable rather than
as a depth — building a `List` nested 200 deep out of a hand-edited file is
the failure this constant exists to prevent.

## What 4.5.1 inherits

- **Make the census unconditional first.** Delete `Builder::censusing` and the
  `ScanExtent::Full` gate in `stream::map_forward`; `CopyBlock::array_shapes`
  becomes a plain `Vec<ArrayShape>`, since a block only ever reaches the map
  fully walked (`scanned_through` advances at `CopyEnd` watermarks and at EOF,
  nowhere else). `tests/census.rs::only_a_scan_that_reaches_eof_censuses`
  inverts into "every mapping pass censuses", and
  `tests/query_cache.rs::a_query_built_index_tiles_in_every_cache_state` drops
  its `without_census` normalizer for span-for-span equality again.
- **A streamed schema needs no completeness test at all.** `table_stream`
  finishes `map_forward`, then collects `matches` — every block in the map
  bearing the target's `(database, qualified name)` — and commits one schema
  from that set before emitting a row. Union those blocks' censuses
  (min-of-mins, max-of-maxes) and the evidence covers exactly the rows the
  stream will hand back, on a cold query as much as on a full scan.
  `index.is_complete(size)` survives only for a *reported* schema — `pgdq
  info`, which answers from an index with no replay to bound the claim.
- **All three `resolve_block` call sites must retype, `resume_state`'s
  included.** It currently takes only `metadata`, so a stream resumed
  mid-block would rebuild an optimistic schema where the original had a
  retyped one — the same query returning two different Arrow schemas depending
  on whether it was interrupted. The index is in scope at every call site
  (`src/stream.rs`), so the block's census can be passed down; putting the
  shape in the `ResumeToken` is the alternative and costs a persisted-shape
  change to an opaque type for no gain.
- **The census vector is pre-sized from the header, and only a header-less
  block's can end up short.** `on_copy_start` allocates
  `header.columns.len()` entries, so a headered block's census is index-aligned
  with its columns whatever the rows held. A header-less block starts empty and
  grows by field index, but only on rows that pass the `{`/`[` pre-filter, so
  its vector ends at the highest brace-bearing field — shorter than the true
  field count whenever the trailing columns never held an array. A missing
  entry reads as the default `ArrayShape`, i.e. `dims: None`, which is exactly
  the "nothing constrained this column, keep the optimistic type" answer, so a
  consumer indexing defensively needs no special case.
- **`dims: None` means "no value constrained this column"** — every row was
  NULL or `{}` — not "no arrays here". A column with `None` has nothing to
  retype from and keeps the optimistic `List<T>`, which is right: `{}` and
  NULL both fit it.
- **`lower_bound_prefix` disqualifies on its own**, however uniform `dims` is.
  `v_lbound` in the fixture is uniformly 1-D *and* prefixed, and it must come
  back `Utf8View`.
- **The fixture columns and what they record**, all six majors, asserted in
  `tests/census.rs`: `v_multidim` `(2, 2)`, `v_mixed_dim` `(1, 2)`, `v_lbound`
  `(1, 1)` + prefix, `v_empty` `(1, 1)` (the `{}` row contributes nothing),
  `v_text_special` `(1, 1)` (the quoted `c{d}` element does not deepen it).
- **A table's census is the union of its blocks'** — min-of-mins,
  max-of-maxes, per I2. `blocks_for` is the enumeration; nothing in this slice
  performs the union, because nothing consumes it yet.
- **4.5.1 reshapes a persisted field** (`array_shapes` loses its `Option`), so
  it bumps the cache format like any other change to a stored shape.

## Verification

- `tests/census.rs` — four tests. The fixture shapes on all six majors, the
  quoting cases out of real `pg_dump` output, a composite column contributing
  nothing, and `only_a_scan_that_reaches_eof_censuses` pinning the two paths.
- `src/index.rs`'s six unit tests over `ArrayShape::observe` — the brace run,
  the quoted-element case a naive count gets wrong, mixed versus uniformly
  deep, NULL/`{}` contributing nothing, the prefix, and the non-array fields
  that must contribute nothing.
- `cargo test --workspace`: 288 pass, 0 fail. `cargo clippy --workspace
  --all-targets` and `cargo fmt --check` clean.
- **Not run: the koji scan.** The spec puts it at phase wrap, not per slice.
  This is the first slice of the phase to touch the scan hot path, so it is
  the one where an identity failure would be meaningful — but it is an hour on
  the HDD and needs a detached run (`CLAUDE.md`). Outstanding, not passed.
