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

Phase 2 is **in progress**
(`docs/design/roadmap-phase2-typed-columns.md`). It lands in five slices; as
each one does, it gets a line in the checklist below linking to its notes doc,
which holds the detail.

Last updated: 2026-08-22.

## Phase 2 progress

- [x] **2.0** Generate `fixtures/*/types/` and validate the mapping table
      against what `pg_dump` actually emits — no changes needed to the
      mapping table itself; two clarifications added to the phase doc (`NaN`
      reachable via any `numeric` column, `infinity`/`-infinity` reachable via
      `date` too). Notes:
      `docs/design/roadmap-phase2.0-fixture-validation-notes.md`
- [x] **2.1** Dollar-quote tracking in the scanner — closes the Phase 1
      known gap. Notes:
      `docs/design/roadmap-phase2.1-dollar-quote-tracking-notes.md`
- [ ] **2.2** Preamble parsing, `DumpMetadata`, cache persistence, `pgdq info`
- [ ] **2.3** Type resolution, `ResolvedSchema`, diagnostics (still all-`Utf8View`)
- [ ] **2.4** Decoders, render-back, round-trip tests
- [ ] **2.5** Benchmarks and the synthetic performance dataset

## Not started

- **Phase 2, slices 2.2-2.5** — see the checklist above.
- **Phases 3-7** — not designed. See `docs/design/roadmap.md`.
- **Benchmarks** (`criterion`) — not wired in.
- **Full 8-version worktree fixture sweep** (`v13.0` … `v18.6`) — worktree
  binaries not yet built; the 3-version container sweep
  (`scripts/generate_fixtures.py`) covers routine needs in the meantime.

## Known gaps

- Large objects in plain-format dumps (`lo_create`/loader calls, not `COPY`
  blocks) remain unmodelled.
- Resuming a `ResumeToken` taken from partway through a cache replay treats
  the rest of the file as unscanned live territory rather than continuing
  the replay — see `docs/design/roadmap-phase1-mvp.md`'s "Index / structure
  cache" for why. Correctness is unaffected; it only gives back some of the
  I/O saving for that one combination.
