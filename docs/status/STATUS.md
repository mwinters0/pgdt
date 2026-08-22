# Status

Snapshot of implementation state. Rewritten in place as state changes — this
doc describes what *is*, not how it got there. For dated notes on what a future
session should pick up, or on discoveries that changed the plan, see `history/`
(one file per day, `YYYY-MM-DD.md`) — not a changelog, only entries worth
keeping.

Phase 1 (MVP) is complete: every functional item in
`docs/design/roadmap-phase1-mvp.md` is implemented, and the two items below
under "Not started" are groundwork that phase specified but never required. How
it landed — module map, and the implementation facts later phases inherit — is
in `docs/design/roadmap-phase1-mvp-notes.md`. Phase 2 has no design yet; each
phase gets its own grilling session when it becomes current
(`docs/design/roadmap.md`).

Last updated: 2026-08-22.

## Not started

- **Phase 2 onward** — nothing beyond Phase 1 is designed or built. See
  `docs/design/roadmap.md`.
- **Benchmarks** (`criterion`) — not wired in.
- **Full 8-version worktree fixture sweep** (`v13.0` … `v18.6`) — worktree
  binaries not yet built; the 3-version container sweep
  (`scripts/generate_fixtures.py`) covers routine needs in the meantime.

## Known gaps

- A line inside a dollar-quoted function body that starts at column 0 *and*
  matches the full `COPY ... FROM stdin;` grammar would be mistaken for a
  real block. Closing this needs dollar-quote tracking in the scanner; see
  `docs/design/pg-dump-compatibility.md`.
- Large objects in plain-format dumps (`lo_create`/loader calls, not `COPY`
  blocks) remain unmodelled.
- Resuming a `ResumeToken` taken from partway through a cache replay treats
  the rest of the file as unscanned live territory rather than continuing
  the replay — see `docs/design/roadmap-phase1-mvp.md`'s "Index / structure
  cache" for why. Correctness is unaffected; it only gives back some of the
  I/O saving for that one combination.
