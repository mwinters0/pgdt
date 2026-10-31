# P38.6.1 — What refuses a read, found: notes

What 38.6.1 leaves the slices after it. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "What is shown,
claimed and checked", as amended on
[2026-10-31](../status/history/2026-10-31.md), "What refuses a read is a
finding `pgdt info` prints"; the decision is D136. Checked by `pgdt-query`'s
`tests/descendants.rs` (every partition refused alone named by a finding,
every one refused with its descendants below a root one names, each
refusal's blocks among its finding's, at every major and flag set of both
schemas), its `tests/timescaledb.rs`
(`what_a_compressed_chunk_refuses_is_the_map_s_finding`, every pin and flag
set) and `pgdt`'s `tests/descendants.rs` (`info_says_what_refuses_a_read`).

## What 38.7 reads

- **Two `DiagnosticKind`s, each `Warning`**: `PartitionsThroughRoot` per
  root, naming its blocks' offsets and the partitions they hold, or `None`
  where a block's TOC entry names none (`KD1`); and
  `TimescaleChunksCompressed` per hypertable, naming the compressed and
  partially compressed chunks the file holds, the DDL declaring them, or
  every one where the DDL is not held, as `absent_chunks` concludes none
  absent without it. Each sentence says what refuses under which mode.
- **One derivation each, read by both channels**: `DumpIndex::through_roots`
  (`span.rs`), which `table_read` (`dump/descendants.rs`) now walks for its
  through-root blocks and `ThroughRoot::finding` turns into the finding; and
  `pgdt_model::timescaledb::compressed_chunks`, which `Verdict::of` and the
  findings both call. `derived_diagnostics` (`pgdt-dump`'s
  `map/tiling.rs`) pushes the roots' ahead of TimescaleDB's; within a
  hypertable's findings the compressed one comes first. TS4 and TS6's
  markers moved from `Verdict::of` to `compressed_chunks`.
- **Where they reach** is where 38.5.2's do: `pgdt info` text and `--json`,
  `pgdt parse`'s listing, `pgdt sql` at registration, an embedder's sink.
  `pgdt query` prints neither; the read it makes refuses, saying why.
- **No cache format bump**: diagnostics are not persisted.

## Negative results

- **A finding's partitions are those the blocks hold**, from their TOC
  entries, not every table below the root: the partitioned tables between
  refuse read with their descendants alone, which the sentence says without
  naming them, and none of them has a block.
- **A compressed chunk the file lacks is in no finding**: it refuses nothing
  (D134), and the absent-chunks finding already names it.
