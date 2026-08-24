# Status

Snapshot of implementation state. Rewritten in place as state changes — this
doc describes what *is*, not how it got there. For dated notes on what a future
session should pick up, or on discoveries that changed the plan, see `history/`
(one file per day, `YYYY-MM-DD.md`) — not a changelog, only entries worth
keeping.

Phase 1 (MVP) is complete: every functional item in
`docs/design/roadmap-phase1-mvp.md` is implemented; what remains under "Not
started" is groundwork that phase specified but never required. How
it landed — module map, and the implementation facts later phases inherit — is
in `docs/design/roadmap-phase1-mvp-notes.md`.

Phase 2 (typed columns) is complete: every functional item in
`docs/design/roadmap-phase2-typed-columns.md` is implemented, across five
slices plus the `<N>.<M>` follow-ups each earned by changing an
already-landed slice's contract (2.2.1, 2.3.1, 2.3.2, 2.3.3). How it landed —
module map, and the implementation facts later phases inherit — is in
`docs/design/roadmap-phase2-typed-columns-notes.md`.

Phase 3 (full DDL object inventory) is **in progress**,
`docs/design/roadmap-phase3-object-inventory.md`.

Last updated: 2026-08-24.

## Phase 3 progress

- [x] **3.1** The `objects` fixture schema — every TOC `Type:` kind neither
      existing schema produces, plus large objects, under a
      `{default, verbose}` flag set for all 6 routine versions. No library
      code. Notes: `docs/design/roadmap-phase3.1-objects-fixture-notes.md`
- [x] **3.2** `map.rs` as a standalone module: `Span`/`SpanBody`, a
      statement-driven boundary/classification pass, `check_tiling` and its
      test over all 96 fixtures, cache identity checking (format v1→v2), the
      hardened statement accumulator. Notes:
      `docs/design/roadmap-phase3.2-span-model-notes.md`
- [x] **3.2.1** The map becomes `DumpIndex`'s primary structure — `spans`
      primary with `blocks()`/`blocks_for` derived, `Span::Data` holding
      `CopyBlock` inline, spans persisted (cache format bump), and
      `build_index` building spans in its existing pass instead of
      `build_map` being a second one. `stream.rs`'s `Recorder` appends each
      live-discovered block as its own `Span::Data`. `Unscanned` is now
      produced for real by `preamble_only`. Earned by 3.2's mis-sizing, not
      by a wrong contract. Notes:
      `docs/design/roadmap-phase3.2.1-span-wiring-notes.md`.
- [x] **3.2.1.1** `DumpMetadata` as a memoized derived view over `spans`
      (`dump_metadata_from_spans`), replacing the separate `PreambleBuilder`
      pass it used to run alongside the span builder — `SpanBody` grows
      `Connect`, `VersionHeader` and `AlterTypeAddValue` to carry what
      `Framing`/`Unparsed` couldn't. Verified against the old pass's output
      across every fixture before `PreambleBuilder` was deleted. Earned by
      3.2.1's mis-sizing, not by a wrong contract. Notes:
      `docs/design/roadmap-phase3.2.1.1-metadata-derived-view-notes.md`.
- [x] **3.2.1.2** `map::Builder::snapshot` — a non-consuming "spans so far"
      read, sound at any boundary where nothing is mid-classification (e.g.
      right after `on_copy_end`). `push_span` now fixes up the previous
      span's `end` at push time rather than deferring it to a single
      end-of-scan pass. No caller yet. Notes:
      [`roadmap-phase3.2.1.2-builder-snapshot-notes.md`](../design/roadmap-phase3.2.1.2-builder-snapshot-notes.md).
- [x] **3.2.1.2.1** Mapping and streaming are separate passes in `stream.rs`:
      `map_forward` classifies DDL and never yields, stopping once the
      queried table is settled; rows come only from replaying already-mapped
      blocks, resume included. `CopyBlock` gains `partition_root` (I2),
      `BatchOptions` gains `ScanExtent`, cache format v3→v4, and a
      `partitions` fixture schema lands. A query-built `DumpIndex` now tiles
      — and matches `build_index` span for span. Notes:
      [`roadmap-phase3.2.1.2.1-mapping-streaming-split-notes.md`](../design/roadmap-phase3.2.1.2.1-mapping-streaming-split-notes.md).
- [ ] **3.2.2** Span text storage with its 64KB cap, and the file-level
      `Diagnostic` channel the runtime tiling check reports through. Earned
      the same way.
- [ ] **3.2.3** A position-only `scan.rs` event for the end of a
      dollar-quoted region, so a TOC-comment-less dump degrades to one span
      per object instead of one span for the rest of the file. Earned by a
      wrong contract, not by mis-sizing.
- [ ] **3.3** The TOC enrichment layer: owner, kind labels, the
      `Tablespace:` field, TOC-coverage reporting.
- [ ] **3.4** The cross-reference set — referenced roles and tablespaces.
- [ ] **3.6** The `Data`-span fast path for `INSERT` runs and the
      large-object region, plus this phase's two gating measurements. Runs
      before 3.5.
- [ ] **3.5** CLI surface: `pgdq info` role/tablespace/object-kind summaries
      and a `--map` span listing; the `docs/manual/` dump-inspection page.

## Not started

- **Phase 3, slices 3.2.2-3.5** — specified, no code. See the checklist
  above and `docs/design/roadmap-phase3-object-inventory.md`.
- **Phases 4-8** — not designed. See `docs/design/roadmap.md`.

## Known gaps

- Large objects in plain-format dumps (`lo_create`/`lowrite` calls, not
  `COPY` blocks) are tiled but not grouped: `crate::map` (Phase 3.2) covers
  their bytes as a run of `Unparsed` spans (one per statement — `lo_open`,
  each `lowrite`, `lo_close`) rather than the single `Data` span the full
  design calls for, since the region is ordinary line-oriented SQL whose
  bytea hex literals cannot contain a line break, so no `COPY` header or
  `\.` terminator can hide inside it (I12) — the generic statement grammar
  handles it correctly, just not at the cost a dedicated fast path would.
  Grouping them into one `Data` span is slice 3.6. Their contents stay out of
  scope permanently regardless; see `docs/design/roadmap.md`, "Large objects:
  ranges, not contents".
- A query stops mapping once its target is settled, so a conflicting
  candidate **past** the stopping point is never seen and
  `Error::AmbiguousTable` is not raised for it — the query returns the
  candidate it found, with no signal that another existed. The stop rule
  (`stream::target_settled`) rules out the two shapes that announce
  themselves: a matching block carrying a partition-root marker (I2), and a
  file containing any `\connect` at all (`pg_dumpall`, concatenation,
  `--create`). What is left undetectable is a file whose *first* segment is a
  plain dump with something concatenated after it — nothing in the prefix says
  so. `ScanExtent::Full` (or a query after `pgdq parse`) gives exact
  detection. Rows are never a union either way, and ambiguity is now raised
  *before* any row is emitted rather than partway through one candidate's,
  which is what the previous form of this gap cost.
- `pgdq info <file>` (no `--preamble-only`) trusts whatever cache
  `CacheMode::load` finds without checking it actually covers the whole file:
  running `pgdq info <file> --preamble-only` and then `pgdq info <file>`
  prints the preamble-only cache's metadata but reports zero `COPY` blocks
  instead of running a full scan. Predates Phase 3.2.1 (reproduced against
  `main` before that slice's changes); not fixed there since it's unrelated
  to span wiring — see
  `docs/design/roadmap-phase3.2.1-span-wiring-notes.md`. Reachable more often
  since 3.2.1.2.1, because an ordinary query now leaves a *partial* cache by
  design rather than a whole-file one. Likely fix: compare `scanned_through`
  against the source's size before trusting a loaded cache as complete.

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed.

*(Empty. The 3.2.1/3.2.1.1/3.2.1.2 splits that sat here have been reviewed
and settled into the design docs — see
[`roadmap-phase3-object-inventory.md`](../design/roadmap-phase3-object-inventory.md)'s
"Mapping and streaming are separate passes" and
[`history/2026-08-24.md`](history/2026-08-24.md).)*
