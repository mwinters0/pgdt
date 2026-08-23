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

Phase 3 (full DDL object inventory) is specified, five slices,
`docs/design/roadmap-phase3-object-inventory.md`. Slice 3.1 (fixture-only, no
library code) is landed: `scripts/fixture_schema_objects.sql` plus
`fixtures/<version>/objects/{default,verbose}.sql` for all 6 routine
versions. Slice 3.2 (the span model) is landed: `pgdump_query/src/map.rs`
(`Span`/`SpanBody`, a statement-driven boundary/classification pass, the
tiling invariant checker), cache identity checking (`crate::cache`), and the
hardened statement accumulator (`crate::preamble::statement_complete`). Two
pieces of 3.2's original scope turned out to be deferrable — a dedicated
`Data`-span fast path for `INSERT` runs/large objects (performance-only; the
generic accumulator already tiles both correctly) and wiring `crate::map`
into `DumpIndex`/`crate::cache`/`crate::stream` — see "Decisions worth a
second look" below and
`docs/design/roadmap-phase3.2-span-model-notes.md`. Slices 3.3-3.5 (TOC
enrichment, cross-reference set, CLI surface) are not started.

Last updated: 2026-08-23.

## Not started

- **Phase 3, slices 3.3-3.5** — specified, no code. See
  `docs/design/roadmap-phase3-object-inventory.md`.
- **Wiring `crate::map` into `DumpIndex`/`crate::cache`/`crate::stream`** —
  the span-building capability landed in slice 3.2 is standalone; making
  spans `DumpIndex`'s primary structure (with `blocks()` derived) per the
  design's "The map is the structure, not a description of it" is a
  deferred follow-up. See "Decisions worth a second look".
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
  Their contents stay out of scope permanently regardless; see
  `docs/design/roadmap.md`, "Large objects: ranges, not contents", and
  `docs/design/roadmap-phase3.2-span-model-notes.md` for the grouping gap.
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

## Decisions worth a second look

Made unattended while landing Phase 3 slices 3.1 and 3.2; flagging rather
than treating as settled.

- **`crate::map` is not wired into `DumpIndex`/`crate::cache`/`crate::stream`
  yet.** The design's "The map is the structure, not a description of it"
  section calls for `spans` to become `DumpIndex`'s primary structure, with
  `blocks()`/`blocks_for` becoming a derived filter over it — a mechanical
  change in principle, but one that touches `stream.rs`'s segment planner
  (the tested core of the streaming query engine) and the cache format. That
  integration was deliberately deferred rather than attempted unattended in
  the same change that introduced the span model, given no one was available
  to review the approach before it landed. `crate::map::build_map` is a
  complete, independently tested, standalone function in the meantime — see
  `docs/design/roadmap-phase3.2-span-model-notes.md`.
- **A dedicated `Data`-span fast path for `INSERT` runs and the large-object
  region was skipped**, after establishing empirically that it's a
  performance optimization, not a correctness requirement, for the tiling
  invariant slice 3.2 delivers — the generic hardened statement accumulator
  already tiles both shapes correctly (`tests/map.rs`'s
  `every_fixture_tiles_exactly` and the embedded-newline `INSERT` case).
  Revisit if/when Phase 8 Track A (row-level `INSERT` reading) or a
  koji-scale `--inserts`/large-object cost measurement makes the missing
  fast path matter in practice.
- **`crate::map`'s span boundaries are TOC-comment-block-anchored** (a
  lexical check for a `-- Name: ...; Type: ...` line, never parsing the
  comment's fields) **rather than pure statement-completion tracking**,
  specifically to route around dollar-quoted function/procedure bodies
  swallowing their own closing `;` invisibly (`crate::scan::CopyScanner`
  never emits an `Event::Line` for one). This reads as a literal
  implementation of the design doc's own boundary-opening rule, but it's an
  interpretation worth a second look: the doc frames TOC comments as
  strictly an "enrichment layer," and this slice leans on their mere
  presence (not their contents) for structural boundary detection before
  Phase 3.3's enrichment officially begins. See
  `docs/design/roadmap-phase3.2-span-model-notes.md`, "Span boundaries:
  TOC-block-anchored, not pure statement completion", for the full
  reasoning and the fixture evidence (`edge_cases.sql`'s two TOC-comment-less
  `CREATE FUNCTION`s) that a pure fallback still has to handle.
- **`objects.sql`'s TOC coverage is broad but not exhaustive.** Six kinds are
  deliberately left uncovered — `SECURITY LABEL` (needs a security-label
  provider extension not present in a stock `postgres:*-alpine` image),
  `ACCESS METHOD`/`OPERATOR`/`OPERATOR CLASS`/`OPERATOR FAMILY`/`TRANSFORM`/
  `TEXT SEARCH PARSER`/`TEXT SEARCH TEMPLATE` (all need a C-level handler
  function, realistically only available via an extension) — with the
  rationale recorded in `scripts/fixture_schema_objects.sql`'s header and
  `roadmap-phase3-object-inventory.md`'s slice-3.1 note. If any of these
  turns out to matter to the sysadmin-inventory use case specifically, that's
  a call for a person, not something the fixture gap forecloses — TOC
  coverage is inherently reported per file (the doc's "graceful degradation"
  design), so an uncovered kind degrades to "unrecognized," not a crash.
- **`objects` schema's flag set is `{default, verbose}` only**, not the full
  matrix `edge_cases` runs (`data-only`, `schema-only`, `binary-upgrade`,
  etc.). The phase doc's ask for slice 3.1 was specifically "a `--verbose`
  flag set," so this matches scope as specified — but if slice 3.2's tiling
  test wants `objects` fixtures under other flag combinations, that's
  additional fixture generation, not a code change.
- **`generate_fixtures.py`'s `drop_fixture_db` now hardcodes cleanup for
  `objects_sub`/`fixture_reader`** (a subscription blocks `dropdb` outright;
  a role is cluster-global and would otherwise leak into whatever schema
  runs next in the same container). Both commands are `IF EXISTS`, so this
  is a no-op for `edge_cases`/`types` — confirmed by regenerating
  `fixtures/18/{edge_cases,types}` and diffing against committed output
  (identical except the fixtures' known non-determinism: the `\restrict`
  token and `now()`-derived timestamp columns, both pre-existing and
  unrelated to this change; that regenerated output was discarded, not
  committed). If a future fixture schema adds its own
  subscriptions/cluster-global objects, this cleanup will need to grow with
  it rather than staying generic.
