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
slice's contract — 2.2.1, 2.3.1, and the pending 2.3.2/2.3.3. As each one
lands it gets a line in the checklist below linking to its notes doc, which
holds the detail.

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
- [ ] **2.3.2** Fixtures and evidence, no code — see "Decided, not yet
      built" below
- [ ] **2.3.3** Behaviour: one target per query, `MetadataNotScanned`,
      multirange recognition — same section
- [ ] **2.4** Decoders, render-back, round-trip tests
- [ ] **2.5** Benchmarks and the synthetic performance dataset

## Not started

- **Phase 2, slices 2.3.2-2.5** — see the checklist above.
- **Phases 3-7** — not designed. See `docs/design/roadmap.md`.
- **Benchmarks** (`criterion`) — not wired in.

## Decided, not yet built — slices 2.3.2 and 2.3.3

All of it is specified in `docs/design/roadmap-phase2-typed-columns.md` and
scheduled ahead of 2.4. **2.3.2 is the fixtures and evidence, no code; 2.3.3
is the behaviour** — split because the halves fail and are reviewed
differently, and because 2.3.3's grammar changes are tested against fixtures
2.3.2 generates. The behavioural items come before 2.4 because each is
survivable while every column is `Utf8View` and none is survivable once a
decoder trusts the resolved schema; the fixture work comes before it because
it is the evidence 2.4 would otherwise commit a decoder without.

### 2.3.2 — fixtures and evidence

- **`CREATE TYPE ... AS RANGE` and base/shell types get real fixtures.** They
  have zero fixture or koji coverage today and are unit-tested against
  hand-written statement text only. All three are reachable from pure SQL
  (I11), so `fixture_schema_types.sql` gains them. This matters most for the
  range grammar, which has never seen `pg_dump`'s real output: multi-line
  body, multi-word subtype, trailing `multirange_type_name`. The same
  regeneration makes the fixture tree uniform — all six routine versions get
  every flag set, backfilling the `create.sql` that 2.3.1 produced for 13/16/18
  only — and introduces psql `\if :SERVER_VERSION_NUM` guards for the PG14+
  multirange DDL. See "The fixture tree is uniform across versions". Two flag
  sets join the `edge_cases` matrix in the same run: `no-comments`
  (`--no-comments --no-security-labels`, because I3 — which the preamble
  segmenter rests on — is proven by source reading alone) and `dumpall` (real
  `pg_dumpall --no-role-passwords`, which produces a `\connect`-before-headers
  segment shape the hand-built concatenation cannot, and is the only fixture
  with content ahead of the first `\connect` for Phase 3's file map to tile).

### 2.3.3 — behaviour

- **A query can silently span two targets.** `resolve.rs::database_for` picks
  the first database whose DDL mentions the queried table, and `table_stream`
  matches `COPY` blocks by name alone — so a table defined in two databases
  returns both databases' rows unioned. Independently, `CopyHeader::matches`
  accepts a bare name against any schema, so `widgets` matches
  `public.widgets` and `other.widgets` in a *single* database, with
  `resolve_block` producing a different schema per block. Decided: one target
  per query, error naming the candidates, via per-`CopyBlock` database
  attribution, `Error::AmbiguousTable`, and a `--database` selector as the way
  out of it. See "One target per query".
- **A `Typed` query against a database whose preamble was never scanned
  degrades silently.** `table_stream`'s live scan discards `Event::Line`, so
  only the *first* database's preamble is ever captured incrementally; a
  block in any later database resolves `NotDeclared`/`Utf8View` with no
  complaint. Decided: `Error::MetadataNotScanned { database }`, naming
  `pgdq parse` as the remedy — the check the design doc has specified since
  2.2 and which per-block attribution finally makes precise. The live scan
  deliberately still does *not* accumulate preamble as it goes; see "The
  preamble pass".
- **Multiranges are unmodelled.** The six built-in names resolve `Unknown`,
  and a user range's auto-created companion type is emitted nowhere in the
  dump at all (I10). Decided: recognize both, reusing `DeferredKind::Range`,
  and capture `multirange_type_name` in `TypeKind::Range`.

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
