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
- [x] **2.2** Preamble parsing, `DumpMetadata`, cache persistence, `pgdq info`
      display. Notes: `docs/design/roadmap-phase2.2-preamble-notes.md`
- [ ] **2.3** Type resolution, `ResolvedSchema`, diagnostics (still all-`Utf8View`)
- [ ] **2.4** Decoders, render-back, round-trip tests
- [ ] **2.5** Benchmarks and the synthetic performance dataset

## Not started

- **Phase 2, slices 2.3-2.5** — see the checklist above.
- **Phases 3-7** — not designed. See `docs/design/roadmap.md`.
- **Benchmarks** (`criterion`) — not wired in.

## Decisions worth a second look

- **Phase 2.2's preamble grammar dispatches off five fixed line-start
  keywords rather than modeling the phase doc's `-- Name: ...; Type: ...` TOC
  comment as a formal segmenter.** Same soundness property either way (never
  guesses; an unrecognized line is just ignored), and nothing in the fixture
  matrix or koji exercised a gap in it — but it's a deliberate departure from
  the letter of the spec, worth revisiting if a real-world dump ever trips it
  up. `docs/design/roadmap-phase2.2-preamble-notes.md`, "Grammar approach".
- **`CREATE TYPE ... AS RANGE` and C-level base/shell types have zero fixture
  or koji coverage** — neither can be produced by the fixture generator
  (koji's own range column uses a built-in type; base types need C
  functions). Implemented from `pg_dump` source reading and unit-tested
  against hand-written statement text only. Same notes doc, "Not touched /
  deferred".
- **A full fresh `pgdq parse`/`info` scan of the real 784 GB koji dump was
  not run this session** (an hour-long process on the HDD; this was an
  unattended run — see `CLAUDE.md`'s long-running-process rule). The
  preamble pass is verified against real koji bytes only up through the
  first `COPY` block (`lock_monitor.activity`), via a truncated-file
  technique described in the notes doc — not end to end. Nothing in the
  design suggests the untested remainder matters (I1 says nothing of
  interest follows a database's first `COPY` block), but a future session
  with a container-hour to spare could confirm it.

## Known gaps

- Large objects in plain-format dumps (`lo_create`/loader calls, not `COPY`
  blocks) remain unmodelled.
- Resuming a `ResumeToken` taken from partway through a cache replay treats
  the rest of the file as unscanned live territory rather than continuing
  the replay — see `docs/design/roadmap-phase1-mvp.md`'s "Index / structure
  cache" for why. Correctness is unaffected; it only gives back some of the
  I/O saving for that one combination.
