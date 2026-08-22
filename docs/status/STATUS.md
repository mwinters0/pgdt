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
(`docs/design/roadmap-phase2-typed-columns.md`). It lands in five slices,
plus the `<N>.<M>` follow-ups each earned by changing an already-landed
slice's contract — 2.2.1, 2.3.1, 2.3.2, 2.3.3. As each one lands it gets a
line in the checklist below linking to its notes doc, which holds the
detail. Only 2.4 (decoders) and 2.5 (benchmarks) remain.

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
- [x] **2.2.1** Incremental (`table_stream`/`pgdq query`) scans now capture
      the first database's preamble too, not just `build_index`'s full scan.
      Notes: `docs/design/roadmap-phase2.2.1-incremental-preamble-notes.md`
- [x] **2.3** Type resolution (`pgtype.rs`, `resolve.rs`), `ResolvedSchema`,
      diagnostics, `pgdq info` column-type/diagnostic display and
      `--preamble-only`, `IS [NOT] NULL` predicates — `RecordBatch`es stay
      all-`Utf8View` until 2.4 builds decoders. Notes:
      `docs/design/roadmap-phase2.3-type-resolution-notes.md`
- [x] **2.3.1** (off-roadmap) Fixture coverage for concatenated/multi-database
      dumps — a new `--create` fixture plus test-time `\connect` concatenation
      in `tests/{preamble,pgtype,stream}.rs`; fixed a real bug this surfaced
      (a later database's version headers were silently dropped) and pinned
      the still-first-match/silent-union query behavior as a test, not just a
      code-reading inference. Notes:
      `docs/design/roadmap-phase2.3.1-multidb-fixtures-notes.md`
- [x] **2.3.2** Fixtures and evidence, no code: the three unevidenced
      `CREATE TYPE` shapes (base, shell, user-defined range) and PG14+
      multirange DDL added to `fixture_schema_types.sql`; `no-comments` and
      `dumpall` (a real `pg_dumpall` run) added to the `edge_cases` matrix;
      six-version regeneration making the fixture tree uniform. No Rust code
      changed. Notes:
      `docs/design/roadmap-phase2.3.2-fixture-evidence-notes.md`
- [x] **2.3.3** Behaviour: one target per query (per-`CopyBlock` database
      attribution, `Error::AmbiguousTable`, `--database`),
      `Error::MetadataNotScanned` (plus the CLI's missing `--schema-mode`
      flag, added alongside it so the error's own remedy is real),
      multirange recognition (`pgtype.rs`, `TypeKind::Range::multirange_type_name`).
      Notes: `docs/design/roadmap-phase2.3.3-one-target-behaviour-notes.md`
- [ ] **2.4** Decoders, render-back, round-trip tests
- [ ] **2.5** Benchmarks and the synthetic performance dataset

## Not started

- **Phase 2, slices 2.4-2.5** — see the checklist above.
- **Phases 3-7** — not designed. See `docs/design/roadmap.md`.
- **Benchmarks** (`criterion`) — not wired in.

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
  known. See `docs/design/roadmap-phase2.3.3-one-target-behaviour-notes.md`.
