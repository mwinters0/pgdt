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

Last updated: 2026-08-23.

## Not started

- **Phases 3-7** — not designed. See `docs/design/roadmap.md`.

## Known gaps

- Large objects in plain-format dumps (`lo_create`/`lowrite` calls, not
  `COPY` blocks) remain unmodelled — and are **not** a correctness hazard:
  the region is ordinary line-oriented SQL whose bytea hex literals cannot
  contain a line break, so no `COPY` header or `\.` terminator can hide
  inside it (I12). Their contents stay out of scope permanently; their byte
  range becomes one span of Phase 3's full file map, which is that phase's
  exit criterion — see `docs/design/roadmap.md`, "Large objects: ranges, not
  contents".
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
