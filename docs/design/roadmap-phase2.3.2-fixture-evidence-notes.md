# Phase 2.3.2 notes — fixtures and evidence, no code

Fixtures and generator changes only, as scoped: the three unevidenced
`CREATE TYPE` shapes and multirange DDL added to `fixture_schema_types.sql`,
the `no-comments` and `dumpall` flag sets added to
`fixture_schema_edge_cases.sql`'s matrix, and a six-version regeneration
making the fixture tree uniform. No Rust code changed; `cargo test --workspace`
passes unchanged against the regenerated fixtures (no test asserts an exact
type or table count for `fixture_schema_types.sql`, so adding tables is
additive from every existing test's point of view).

## What landed

- **`public.shellonly`, `public.mybase`, `public.myrange`** in
  `scripts/fixture_schema_types.sql`, following the recipe already recorded
  in `postgres-invariants.md` I11 (base/shell types need no compiled
  extension — `LANGUAGE internal` wrappers over `textin`/`textout` satisfy a
  shell return type). Backed by tables (`t_base_type`, `t_user_range`) so the
  types round-trip through a real `COPY` block, not just a bare `CREATE
  TYPE`.
- **PG14+ multirange DDL**, guarded:
  ```sql
  SELECT current_setting('server_version_num')::int >= 140000 AS has_multirange \gset
  \if :has_multirange
  ...
  \endif
  ```
  `myrange`'s body carries `multirange_type_name = public.myrange_multi` only
  under the guard (the parameter doesn't exist as PG13 grammar at all); a
  guarded `t_multirange` table adds columns of both the built-in
  `int4multirange` and the auto-created `public.myrange_multi` companion.
- **`no-comments`** (`--no-comments --no-security-labels`) and **`dumpall`**
  (real `pg_dumpall --no-role-passwords`, not a `pg_dump` flag set) added to
  `SCHEMAS["edge_cases"]` in `scripts/generate_fixtures.py`. `dumpall` needed
  a sentinel (`None` in the flag-set dict) so `dump_flag_set` can swap the
  binary entirely rather than appending flags — it dumps the whole cluster,
  not `DB_NAME`, and only make sense run while the fixture database is still
  loaded (before `drop_fixture_db`).
- **Six-version regeneration**: every routine version (13-18) now has every
  flag set of every schema — `fixtures/{14,15,17}/edge_cases/create.sql` was
  the backfill 2.3.1 deferred; `no-comments`/`dumpall` are new everywhere.
  `fixtures/<version>/types/*.sql` regenerated for all six too, picking up
  the new type shapes.

## Evidence gathered (the point of this slice)

All three predictions in `postgres-invariants.md` were checked against real
output before trusting them further, using single-container probes ahead of
the full regeneration (kept out of the repo — throwaway containers, per
`CLAUDE.local.md`):

**I11 (base/shell types) confirmed exactly as specified**, on 16.15:
`mybase` emits twice, `SHELL TYPE` then `TYPE`; `shellonly` emits once, under
`TYPE`. `mybase`'s completed body lists `INTERNALLENGTH`/`INPUT`/`OUTPUT`/
`ALIGNMENT`/`STORAGE`, matching the recipe already in I11 (parameter order
differs slightly from the hand-written recipe — `pg_dump` puts
`INTERNALLENGTH` first — which is cosmetic and doesn't affect the grammar).

**I10 (range grammar) confirmed on both ends of the version guard.** On
13.23, `myrange`'s body is multi-line with a multi-word subtype
(`double precision`) and no `multirange_type_name` parameter at all — the
grammar test the design doc called out as never having seen real multi-word
subtype output. On 14.24 through 18.6, the same body gains
`multirange_type_name = public.myrange_multi`. `t_multirange`'s `COPY` data
confirms the companion type's text form (`{[1.5,10.5)}`) is ordinary
multirange syntax, decodable the same way a built-in's would be once Phase 4
writes the decoder.

**I9 (pg_dumpall two-segment shapes) confirmed on 13.23 and 18.6**, closing
the gap the invariant itself flagged ("a genuine `pg_dumpall` fixture is
added in slice 2.3.2"). Both versions show the identical pattern predicted
from source reading alone:
- `template1` and `postgres` (no `--create`, `pg_dumpall` writes their
  `\connect` itself): `\connect <db>` appears, *then* the version-header
  pair.
- `pgdq_fixture` (ordinary database, gets `--create`): the version-header
  pair appears *first*, then `\connect pgdq_fixture` — the `pending_headers`
  branch `PreambleBuilder` already handles, now proven against a real
  `pg_dumpall` file rather than only the hand-concatenated stand-in.

**I3 (`--no-comments` preserves TOC comments) confirmed**: `default.sql` and
`no-comments.sql` for the same version have an identical count of `-- Name:`
TOC header lines (16, for `fixtures/16/edge_cases`). Neither fixture schema
contains a `COMMENT ON` statement, so that half of `--no-comments` isn't
separately exercised here — the preamble segmenter only depends on the TOC
header surviving, which is now checked rather than only source-read.

## `postgres-invariants.md` updates

I9, I10, and I11 each gained the real fixture as a second observation
alongside their existing source citations, rather than resting on source
reading plus a hand-probed container session alone.

## Not done here, on purpose

No new Rust test reads any of the new type shapes or the `dumpall`/
`no-comments` fixtures. 2.3.2 is fixtures-and-evidence only; the range/
multirange *grammar* changes this evidence motivates (recognizing
`int4multirange`-style built-ins, capturing `multirange_type_name` in
`TypeKind::Range`) are slice 2.3.3's job, tested against exactly the fixtures
this slice built.
