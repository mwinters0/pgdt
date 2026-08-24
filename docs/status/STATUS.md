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
- [ ] **3.2.1.2** `stream.rs`'s live segment classifying the DDL between
      discovered blocks so a query-built `DumpIndex` tiles the way
      `build_index`'s does. Needs `map::Builder` to support a non-consuming
      "spans so far" snapshot first (today `finish` consumes it and can only
      run once, which conflicts with `Recorder`'s per-block persistence) — a
      capability gap, not just a wiring gap. Earned by 3.2.1.1's mis-sizing —
      see its notes doc's "What this slice does not do".
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

- **Phase 3, slices 3.2.1.2, 3.2.2-3.5** — specified, no code. See the
  checklist above and `docs/design/roadmap-phase3-object-inventory.md`.
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
- Resuming a `ResumeToken` taken from partway through a cache replay treats
  the rest of the file as unscanned live territory rather than continuing
  the replay — see `docs/design/roadmap-phase1-mvp.md`'s "Index / structure
  cache" for why. Correctness is unaffected; it only gives back some of the
  I/O saving for that one combination.
- A cold (uncached) query against an ambiguous table name can still emit
  some rows before `Error::AmbiguousTable` surfaces, if the first matching
  block sits earlier in the file than the conflicting one — the live scan
  can't know a second candidate exists until it reaches it. The emitted rows
  are genuinely correct (never a union), but a caller must treat any output
  preceding a stream error as incomplete, same as any other mid-stream
  error. A warm cache (or a query run after `pgdq parse`) catches the
  ambiguity before any streaming starts, since every candidate is already
  known. See `docs/design/roadmap-phase2-typed-columns-notes.md`, "One target
  per query".
- `pgdq info <file>` (no `--preamble-only`) trusts whatever cache
  `CacheMode::load` finds without checking it actually covers the whole file:
  running `pgdq info <file> --preamble-only` and then `pgdq info <file>`
  prints the preamble-only cache's metadata but reports zero `COPY` blocks
  instead of running a full scan. Predates Phase 3.2.1 (reproduced against
  `main` before that slice's changes); not fixed there since it's unrelated
  to span wiring — see
  `docs/design/roadmap-phase3.2.1-span-wiring-notes.md`. Likely fix: compare
  `scanned_through` against the source's size before trusting a loaded cache
  as complete.

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed.

- **Slices 3.2.1 and 3.2.1.1 were each split further mid-implementation,
  unattended, both times along the same line: a self-contained mechanical
  change bundled with `stream.rs`'s live-segment DDL classification.**
  3.2.1.1 landed `DumpMetadata` as a derived view over spans standalone (not
  folded into 3.3, resolving the previous round's open question — the
  `SpanBody` additions it needed, `Connect`/`VersionHeader`/`AlterTypeAddValue`,
  turned out narrow and didn't touch anything 3.3's TOC enrichment will add).
  What's left, 3.2.1.2, is `stream.rs`'s DDL classification itself, now
  blocked on a `map::Builder` capability gap (a non-consuming "spans so far"
  snapshot — `finish` currently consumes the builder and can only run once,
  but `Recorder` persists after every completed block) rather than only a
  review-confidence one. Worth a look: whether that capability belongs on
  `Builder` itself or suggests `Recorder`'s per-block persistence should
  change instead — see `docs/design/roadmap-phase3.2.1.1-metadata-derived-view-notes.md`
  and `docs/status/history/2026-08-24.md`.
