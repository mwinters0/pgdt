# P38.5.3 — The hypertables' findings, said per read too: notes

What 38.5.3 leaves the slices after it. The decision is D135; the facts
behind each note are TS8 and TS10, and the findings it mirrors are
[38.5.2's](roadmap-P38.5.2-findings-notes.md). Checked by `pgdt-query`'s
`tests/timescaledb.rs` (every pin and flag set, each read's notes against the
map's findings and the root's rows against its `held` answer, a cold pass and
a resumed stream alike, and an unreadable catalog's) and `pgdt`'s
`tests/descendants.rs` (`pgdt query` printing them).

## What 38.6 and 38.7 read

- **Three `PlanNoteKind`s, each `Warning`**: `HypertableRootRows`,
  `HypertableChunksAbsent`, `CompressionUnchecked`. `Verdict::notes`
  (`dump/hypertables.rs`) settles them per mode beside the refusals, carried
  on `Table` as `ReadNotes`, and `Table::plan` raises them ahead of the plan's
  own notes, a resumed stream included. They reach `pgdt query`'s stderr,
  `pgdt sql`'s at planning and an embedder's sink as every plan note does.
- **Which read says which**: root rows, a hypertable read with its
  descendants whose root holds any, and read alone where the release is known
  to be 2.28 or later — stated, or an inferred era starting there — whose
  `ONLY` reads a hypertable empty (TS8); absent chunks, a hypertable read with
  its descendants; under an unreadable catalog, a read reaching a table below
  a hypertable the catalog's rows name, or below any table where none is
  named, under either mode. Under an unreadable catalog a named hypertable
  read alone says its root's rows as above.
- **One count for both channels**: `pgdt_model::timescaledb::root_rows` and
  `absent_chunks` are public, and the findings and the notes both call them.
- **A cold pass's table says what a complete map's does**: a hypertable or a
  table with an ancestor in a TimescaleDB database maps to the end (D131,
  D134), so its catalog is whole before its notes are settled.

## Negative results

- **No unreadable catalog's note on a named hypertable read alone**: it
  reaches no chunk. Read with its descendants where the file holds none of
  its chunks, it says its own since
  [38.5.4](roadmap-P38.5.4-no-chunk-held-notes.md).
- **A note's table is its qualified name alone**, as `ReadAsNull`'s is; a
  front end locating its findings (`Located`) names the database.
