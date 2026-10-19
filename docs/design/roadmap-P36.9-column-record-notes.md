# P36.9 notes — one record per column

`ResolvedSchema` is the Arrow schema and one `pgdt_codec::ResolvedColumn` per
field, both private and read through accessors: `schema()`, `columns()`,
`notes()`, `unmapped_count()`. A record holds its `FieldRef` (the schema is
built from the records' own, so the two cannot disagree), its `ColumnNote`,
its `NestedPlan`, its `ComparisonPlan`, whether it is declared `NOT NULL`, and
the semantics `read_as_text` widened it under. `bounds_kinds` moved from the
schema to the record. `KD121` is struck: `ValueKey::of` and
`ValueKey::literal` take a `&ResolvedColumn`, and a column widened under one
semantics keys in it alone, `None` in the other (D100).

## The shape

- **The note keeps its resolution, and the separate resolutions vector
  goes.** The record holds the `ColumnNote` whole, so the finding a sink
  downcasts is unchanged, and `ResolvedColumn::resolution` reads the note's.
  The spec's "the copy of each column's resolution in its note goes" was
  read as the copy *beside* the note; the call is under STATUS's "Decisions
  worth another look".
- **Rendering takes the record.** `render_field`, `render_field_into` and
  `decode_field` take a `&ResolvedColumn`; the plan-taking recursion is
  `render.rs`'s `render_plan` and `render_plan_into`, crate-private, which
  the crate's own unit tests call. `pgdt query`'s `print_batch` takes the
  plan's columns and zips them with the batch's arrays, where it indexed a
  plans vector with a `Scalar` fallback.
- **Two constructors in `pgdt_codec::internal`**: `column_of`, a record from
  its parts with no widening, and `schema_of`, a schema of records with no
  metadata. The replay's projection is `schema_of` over the requested
  columns' records, and so is each hand-built test schema below the
  embedders. An embedder gets records only from a resolution (D106):
  `datafusion-pgdump`'s dynamic-filter tests, held to the embedder rule by
  `layering.py`, resolve declared types for theirs, so their columns now
  carry the register's plans where they carried `Refused`.
- **Test scaffolding.** `predicate/test_support.rs`'s `Column` is the
  parts a predicate test overrides (`Column { resolution: …, ..Column::of(…) }`),
  in place of mutating a schema's parallel vectors; `tests/common`'s
  `scalar_column` is the record a test renders an arbitrary scalar array
  through, and `render_allocations.rs` builds its records before counting.

## Negative results

- **The predicate's `(resolved, index)` functions keep their signature**,
  binding the record once inside (`announced`, `datafusion_divergences`,
  `resolve_term`). Taking the record would move the index into every caller
  that also names the column by position for a refusal.
- **`ColumnGatherer::new` keeps its separate arguments**, read off the record
  at its one call: it is `internal`'s and per block.

## The count: two rises, outside the slice's code, accepted

`measure.py --count-pair HEAD` on the uncommitted tree, whose parent is
`3f265e2b` (36.13), ran detached (`runs/count-pair-36.9-20261019/log`, its
`HANDOFF.md` beside it) and exited 0 after 25.9 minutes. Written before it
ran: nothing per row moves on `parse` or `pgdt sql`, so no row resolves, or
the two `query` rows resolve a fall, the render's fallback branch gone; a
resolved rise on a `parse` row refuses the slice until it is attributed.
**Both `query` rows resolved falls** (-0.218% strings, -0.064% typed), and
**two rows resolved rises**: the data-level `parse`, +0.001%, and `pgdt
sql`, +0.008%.

**Both were counted exactly**, in `runs/attr-36.9-20261019-callgrind/`:
36.4.1's apparatus unchanged, on the pair's two legs copied to
`/mnt/ssd/fedora/scratch/pgdump_query/attr-36.9/`. `analyze.py`'s rules
were written before the count, and `reading.txt` is what it printed;
`callers.py` (an arc's callers, differenced) and `cls.py` (whether a
function's machine code is identical in both legs) are the reads below.
Neither total is gated, one leg's reps differing by more than the data
row's whole change, so each shape is read by its changed code per rep pair:
**on both, the slice's changed code costs under half the rise**. Where each
sits:

- **The data-level `parse`: identical code's stack offset, under a frame
  the tree grew.** Reproduced (+497,503 and +514,199 `Ir` against the
  native +492,154), the statistics account alike on both legs. Changed
  code falls (`Gatherer::observe_row`, -75,582). The rise is `memcpy`
  called by `ColumnGatherer::close_group` as often in both builds,
  294,912 times, two instructions more each; `close_group` and its caller
  `Gatherer::close_through` are identical machine code. Both of
  `close_group`'s copies (544 and 448 bytes) write into its own frame.
  `observe_row`, which calls `close_through`, reserves 16 bytes more stack
  in the tree (`0x278` to `0x288`), so every frame below it sits 16 bytes
  lower and the copies' destination changes its 32-byte alignment; the one
  call of the same chain from `Gatherer::finish`, under no grown frame,
  costs nothing more. The slice changed no source of `observe_row`; what moved
  its machine code is unattributed.
- **`pgdt sql`: the provider's scan planning, once a query.** Reproduced at
  71% and 82% (+1,311,773 and +1,512,612). It sits in `PgDumpTable::scan`'s
  closure, called once, and what it calls: `compare_keys_stored` out of
  line in the tree (98,272 calls) where the closure's own count fell
  902,779, 3,275 `String` clones the parent's closure never called, and
  about 7,350 more allocations. Nothing per row. Whether the clones and
  allocations are new work or code the parent inlined is unattributed; the
  `.out` files can split it by instruction.

**Both are accepted** (spec, "What a hot-path slice owes"), and 36.9 is
ticked. **The `pgdt sql` rise was accepted by the maintainer** on
2026-10-19, relayed to this round by the session that launched it, without
an exact count; the count above was already running and was finished. **The
data-level `parse` rise is accepted by this round**: no shape of the slice's
code recovers a stack offset in code it did not change. That a rise in
identical code does not reopen how `--count-pair` is read, as 36.4.3's
reading said layout would, is under STATUS's "Decisions worth another look".

## What the next round inherits

- **The next counted slice is counted against its own parent**, which
  carries this slice.
- **36.10** spells the errors; nothing here moved an error.
- **36.11** builds `PgDump` over a `Dump`; the provider already reads the
  schema through the accessors (`filter.rs`, `table.rs`, `statistics.rs`).
