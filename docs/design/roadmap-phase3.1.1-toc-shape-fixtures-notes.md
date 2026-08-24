# Phase 3.1.1 — TOC shape fixtures: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
slice table. That doc states the design; this one records what slice 3.1.1
produced and what later slices inherit from it. Per `CLAUDE.md`, this file is
consolidated into a single phase-level notes doc (and removed) once all of
Phase 3 lands.

## What landed

Three additions to `scripts/fixture_schema_objects.sql`, all real `pg_dump`
output across the routine matrix — no library code changed, matching 3.1's
own "no library code" character:

- **`CREATE TABLESPACE fixture_ts`** plus `objects.tablespaced_table`, a
  table created directly in it. `generate_fixtures.py` gained
  `prepare_tablespace_dir`, called before the schema SQL runs whenever
  `schema == "objects"`: `docker exec ... mkdir` then `chown postgres:postgres`
  the directory `CREATE TABLESPACE`'s `LOCATION` points at — `docker exec`
  defaults to root in the official image, so the mkdir needs no privilege
  drop, but `CREATE TABLESPACE` needs the directory owned by the connecting
  `postgres` OS user. `drop_fixture_db` gained a matching `DROP TABLESPACE IF
  EXISTS fixture_ts`, the same explicit-teardown pattern 3.1 established for
  the role and subscription (cluster-global objects that would otherwise leak
  into the next schema's run in the same container).
- **`objects.no_public_execute()`**, a function whose default PUBLIC
  `EXECUTE` privilege is revoked. This is the one ACL shape that produces a
  *solo* `REVOKE` with no offsetting `GRANT`: the target ACL has strictly
  fewer privileges than the default (PUBLIC's automatic `EXECUTE`), so
  `pg_dump`'s ACL diff has nothing to re-grant. Revoking a privilege from an
  object's *owner* instead (tried first, against `objects.widgets`) always
  pairs the `REVOKE` with a `GRANT` restoring the owner's remaining implicit
  privileges — a real instance of both I19 shapes, but not the standalone one
  this fixture exists to isolate.
- **`objects/stats.sql`** (v18 only): a new version-conditional flag-set form
  in `generate_fixtures.py`'s `SCHEMAS` — a flag-set value can now be
  `(min_version, flags)` instead of a bare list, and `generate_for_version`
  skips it below `min_version`. `objects`'s `"stats"` entry is
  `("18", ["--statistics"])`.

## What later slices inherit

**The real flag is `--statistics`, not `--with-statistics`.** The phase
spec's slice-3.1.1 row and `postgres-invariants.md`'s I18 entry both named
`--with-statistics` before this slice ran; `pg_dump.c`'s long-option table
(`{"statistics", no_argument, NULL, 22}`) has no such string in any version
13-18. Source-only reasoning got the shape right (`TOC_PREFIX_STATS`, the
`STATISTICS DATA` TOC kind, PG18+) and the flag name wrong — exactly the
class of surprise this evidence-gathering slice exists to catch. Corrected in
`postgres-invariants.md` (I18), `pg-dump-compatibility.md`, and this file;
the phase spec's own prose is left as originally written per
`docs/process.md`'s "spec vs. notes" (it is not a decision that changed).

**A `STATISTICS DATA` TOC header has no owner at all**, not even the usual
`Owner: -` placeholder: `-- Statistics for Name: stats_table; Type:
STATISTICS DATA; Schema: public; Owner: -` is what a real run produces, but
that's incidental — `dumpRelationStats` never sets `te->owner`, and
`sanitize_line`'s `NULL`-hyphen substitution is what turns the unset owner
into the literal `-`. `map::parse_toc_header_line` already treats `-` and
empty as equivalent "no owner" (I18), so this needed no code change; recorded
here because it's a fact a `TOC_PREFIX_STATS` implementation would otherwise
have to re-derive.

**`map::parse_toc_header_line` still does not recognize `TOC_PREFIX_STATS`.**
This slice closes the *fixture-evidence* gap (I18/I19's "taken on faith"
scope limits), not a *code* gap — the parser's existing behavior (an
unrecognized prefix leaves `Span::toc: None`, and the span still tiles as
`Unparsed`) was already a documented, deliberate deferral, not something
this slice's "no library code" character was ever going to close. Confirmed
by `every_fixture_tiles_exactly` passing against the new fixture unchanged.
A future slice that wants `STATISTICS DATA` spans attributed can build on
this fixture directly.

**The tablespace and REVOKE fixtures immediately closed one hardcoded test
assumption.** `tests/map.rs::build_index_records_referenced_roles_and_tablespaces`
asserted `index.tablespaces.is_empty()`, true only because no fixture had
ever referenced a non-default tablespace; updated to assert `{"fixture_ts"}`.
No other test hardcoded a byte offset or span count against
`fixtures/*/objects/*.sql` — the rest either search by name/kind
dynamically or tile-check structurally, so they absorbed three new objects
with no changes needed.

**`extract_statement_cross_refs`'s `REVOKE`/`SET default_tablespace` handling
was already correct against real output**, not just against the hand-written
unit-test literals that were its only coverage before this slice — confirmed
by the fixture round-tripping through `build_index` with no parser changes.
`preamble.rs`'s `refs_of` doc comment updated to point at the real lines
rather than describing them as absent.

## Coverage

Register entries closed to fixture evidence: I18 (`Tablespace:` field,
`TOC_PREFIX_STATS`), I19 (`REVOKE`, non-default `SET default_tablespace`).
`pg-dump-compatibility.md` gained a dedicated `--statistics` row and updated
its `objects`-schema-flag-set row to reflect the new `stats` set.
