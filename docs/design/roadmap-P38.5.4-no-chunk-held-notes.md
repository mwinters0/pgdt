# P38.5.4 — A named hypertable with no chunk held reads: notes

What 38.5.4 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "TimescaleDB",
as amended on [2026-10-31](../status/history/2026-10-31.md), "A named
hypertable with no chunk held reads: 38.5.4"; the notes it extends are
[38.5.3's](roadmap-P38.5.3-per-read-notes.md). Checked by `pgdt-query`'s
`tests/timescaledb.rs`, `an_unreadable_catalogs_named_hypertable_with_no_chunk_held_reads`,
over the 18 `exclude-chunks` dump with its catalog broken both ways.

## What 38.6 and 38.7 read

- **An unreadable catalog refuses a read with descendants only where the
  table has one** the DDL declares, and is named by the catalog's rows or no
  name reads (`Verdict::unreadable_refusal`, `dump/hypertables.rs`). A named
  hypertable with none reads its root's own rows, as its restore's `held`
  answer holds them.
- **That read raises `PlanNoteKind::HypertableChunksUnknown`**, a `Warning`
  naming the catalog's fault, after its root's rows note where it holds
  any; read alone it raises none of its own. The fallback's unnamed
  hypertable with no chunk held raises nothing.
- **The map's `TimescaleCatalogUnreadable` says the narrower refusal**: each
  named hypertable the file holds a chunk of is not read with its
  descendants. Its `hypertables` list is unchanged, still every name the rows
  give, so `pgdt info --json` shows what the catalog names, not what refuses.

## Negative results

- **No `CompressionUnchecked` on that read**: it reaches no table that may
  be a chunk, so nothing it reads is unchecked; what is unknown is only what
  it lacks, which is the new kind's alone.
- **"Holds a chunk" is the DDL's walk** (D134), not the catalog's list,
  which does not read: a declared descendant refuses the read, whatever it
  is.
