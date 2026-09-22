# P6.5.1 — What the session holds besides its scans: notes

A slice earned after 6.5 of
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), from the review of its
session budget ([`../status/history/2026-09-22.md`](../status/history/2026-09-22.md)).
`ScanBudget` now draws a session's `Finite` memory-pool limit and the
statistics of every dump billed to it before any scan draws, both on
`ScanBudget`'s rustdoc. The library gained
`DumpIndex::statistics_heap_bytes`, the one sum `map_file` bills as
`Term::Loaded` and the provider bills for a resident map. The check is
`the_resident_statistics_are_billed_and_lower_a_budget_the_margin_cannot_hold` in
`datafusion-pgdump/tests/provider.rs`.

## What later slices inherit

- **A dump is billed once per budget, and for its life.** `register_dump`
  bills the session's budget; a table registered by hand bills whatever budget
  its first scan draws on (`ScanBudget::of`, the process's where the session
  carries none). The `Hold`s live on `PgDump`, so they return only when the
  last `Arc<PgDump>` drops — a deregistered catalog whose dump a caller still
  holds stays billed, the map being still resident. 6.8's `STORED AS PGDUMP`
  factory inherits this without doing anything.
- **The billed figure is fixed at open.** A cache is never reloaded under an
  open dump (`PgDump`'s rustdoc), so the statistics it holds do not move. 6.7,
  which hands statistics to DataFusion, reads the same resident map and adds no
  second copy to bill, unless it builds one.
- **The pool limit is read at every draw**, from the session that plans the
  scan (`pool_limit`), not captured at registration: a runtime is the
  session's, and one dump may be registered in several.
- **`ScanBudget::resident()`** reports the billed statistics beside
  `drawn()`'s scans.

## Negative results

- **The holdings never come off the allowance.** Subtracting them from the
  allowance before carving would take the margin as a fraction of less than
  the container's limit, letting the predicted resident pass the margin.
  Which bound they come off is 6.5.2's
  ([`roadmap-P6.5.2-holdings-margin-notes.md`](roadmap-P6.5.2-holdings-margin-notes.md)).
- **With no allowance, nothing is subtracted.** A host where neither a limit
  nor `MemAvailable` answers leaves `ScanBudget` without an allowance; the
  scan takes the library's own discovery, and a pool limit or resident
  statistics have nothing to come off.
- **No `D<k>` entry.** The register is at its cap, and the shape is on
  `ScanBudget`'s rustdoc with its rejected alternative.
- **No manual page and no cache format change.**
