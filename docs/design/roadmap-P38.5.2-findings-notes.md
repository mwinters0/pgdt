# P38.5.2 — The hypertables' findings: notes

What 38.5.2 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "TimescaleDB";
the facts behind each finding are TS6, TS8 and TS10. The pins are checked by
`pgdt-mapping`'s `tests/timescaledb.rs` (each root's rows against its `held`
answer, each absent chunk against the catalog, a parallel pass and a cache
load alike) and `pgdt-query`'s `tests/timescaledb.rs` (an unreadable
catalog's finding).

## What 38.6 and 38.7 read

- **The findings are the map's diagnostics**: three `DiagnosticKind`s,
  `TimescaleChunksAbsent` per hypertable, `TimescaleRootRows` per root
  holding any row of its own, `TimescaleCatalogUnreadable` per database,
  each `Warning` and naming its database where the dump does.
  `pgdt_model::timescaledb::findings` derives them, and
  `derived_diagnostics` (`pgdt-dump`'s `map/tiling.rs`), now over the whole
  `DumpIndex`, calls it wherever a whole map is finished or loaded. A cold
  query's partial map carries none.
- **Where they reach**: `pgdt info` text and `--json`, `pgdt parse`'s
  listing, `pgdt sql` at registration (its warnings on stderr), and an
  embedder's sink through the provider's registration. **`pgdt query`
  prints no file-level diagnostic** (`announce_comparisons`' doc in
  `crates/pgdt/src/query.rs`), so a hypertable read there says nothing of
  its root's rows or absent chunks.
- **Absent chunks** are every non-dropped catalog chunk the database's DDL
  does not declare, internal hypertables' included: a dump excluding
  `_timescaledb_internal` names the materialization and compressed
  hypertables beside the user ones, though their own roots are absent too.
  None is concluded where the database's DDL is not held.
- **An unreadable catalog's finding comes first** in its database's, then
  the root rows of each hypertable its rows still name.

## Negative results

- **A root holding no row raises nothing**: every root before 2.23 and most
  after hold none, where pgdt's answer and the server's agree, so a finding
  would say only that nothing differs.
- **No cache format bump**: `DumpIndex::diagnostics` is `#[serde(skip)]`, so
  the new kinds are derived on load from what the cache already holds.
