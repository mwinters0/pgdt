# Phase 2.3.1 notes — concatenated/multi-database dump fixtures

Not on the original roadmap. Landed to close a gap several phases had
flagged and deferred: every fixture and the koji sample are single-database,
so the `\connect`-delimited multi-database shape (`docs/design/roadmap-phase2-typed-columns.md`,
"Multi-database dumps") was only ever exercised by hand-written statement
text in `pgdump_query/src/preamble.rs`'s unit tests, never against real
`pg_dump` output end to end.

## What landed

- **`fixtures/{13,16,18}/edge_cases/create.sql`** (new flag set, `--create`):
  the only flag combination in the matrix that makes `pg_dump` emit a
  `\connect` at all — plain `pg_dump` never does, and `--create` (single
  database) emits exactly one, ahead of the database it just created.
  Generated for the three versions the Rust test suite already samples
  elsewhere, not all six routine versions — see "Why only three versions"
  below. `scripts/generate_fixtures.py`'s `SCHEMAS["edge_cases"]` gained a
  `"create": ["--create"]` entry so a future full regeneration keeps
  producing it for every version.
- **No second fixture tree for "multi-database."** Each of
  `pgdump_query/tests/{preamble,pgtype,stream}.rs` builds the multi-database
  shape itself, at test time, in a tempdir: read `create.sql`, write a
  second copy with its `pgdq_fixture` database name replaced by
  `pgdq_fixture_2` (the only place that fixed string appears in a `--create`
  dump — `CREATE DATABASE`/`ALTER DATABASE`/`\connect`, nothing in the
  schema itself), concatenate the two, done. No second Postgres instance,
  no new container run, no random per-run `\restrict` token to keep in sync
  in a checked-in file. The three test files each carry their own small
  `multidb_fixture(version)` helper (duplicated, not shared — no test
  file in this crate shares code with another; see any of the existing
  `fixture()`/`types_fixture()` helpers for the same pattern) rather than
  literally concatenating `pg_dumpall`-style output, since the parser only
  cares about the `\connect`-delimited shape, not which process produced it.
- **A real bug this surfaced, fixed:** a database after the *first*
  `\connect` was silently losing its own version-header pair
  (`-- Dumped from database version` / `-- Dumped by pg_dump version`). I9
  (`docs/design/postgres-invariants.md`) documents that every concatenated
  `pg_dump --create` child prints its own pair ahead of its own `\connect`
  — including children after the first — but `PreambleBuilder` only ever
  carried that pair forward across the *first* `\connect` (`on_connect`'s
  `!seen_connect` branch). A later database's header lines arrive while
  `current` is still the *previous* database's already-`preamble_complete`
  segment, so `feed_line`'s early-return gate (`if self.current.preamble_complete
  { return; }`) skipped them entirely — never captured anywhere, never
  carried anywhere. Fixed with a `pending_headers: (Option<String>,
  Option<String>)` staging field on `PreambleBuilder`: the
  `preamble_complete`-gated branch of `feed_line` now stages a matching
  header line there instead of dropping it, and `on_connect` consumes it for
  *every* `\connect`, not just the first. See I9's updated "Consequence"
  section for the full before/after. Regression-tested at the unit level
  (`preamble::tests::a_later_connect_segment_keeps_its_own_version_headers_too`)
  and the fixture level (`tests/preamble.rs`'s
  `concatenated_create_dumps_yield_two_named_databases_each_fully_parsed`
  asserts both databases' version fields are `Some`).
- **A real gap this surfaced, left as-is (see "Decision for next session"
  below):** the design doc's stated intent — a table query never spans
  databases, erroring by naming the candidates rather than silently unioning
  — isn't implemented. `resolve.rs::database_for` (type resolution) and
  `stream::table_stream` (row data) both match by qualified table name alone
  with no notion of which `\connect` segment a `CopyBlock` came from. Two
  new tests pin this down as *current, observed* behavior rather than
  leaving it as an inference from reading the code:
  - `resolve::tests::ambiguous_table_across_databases_resolves_against_the_first_match`
    (hand-built, `pgdump_query/src/resolve.rs`) — two databases declaring
    the same table with genuinely different column types, proving
    first-match by making the outcome differ depending on which database
    would win.
  - `tests::querying_a_table_name_shared_by_two_databases_silently_unions_both`
    (`pgdump_query/tests/stream.rs`) — the real multi-database fixture,
    confirming `table_stream` returns both databases' `public.widgets` rows
    concatenated, in file order.

## Why only three versions — superseded

`fixtures/{14,15,17}/edge_cases/` have no `create.sql`, because this slice
generated only the trio the Rust test suite iterates (`[13, 16, 18]` —
oldest, middle, newest), on the reasoning that generating fixtures no test
reads buys nothing.

Slice 2.3.2 reverses that: the fixture tree is uniform across all six routine
versions, so that "absent" always means "deliberately absent" rather than
"nobody got round to it". Which versions the *tests* iterate stays a separate
question. See `roadmap-phase2-typed-columns.md`, "The fixture tree is uniform
across versions". The backfill is additive and needs no change here — the
`SCHEMAS` entry this slice added already produces `create` for every version.

## Why test-time concatenation instead of a real `pg_dumpall` fixture — partly superseded

Slice 2.3.2 adds a real `pg_dumpall` fixture. The reasoning below was sound
on its own terms and the concatenation stand-in stays, but it rested on a
premise that turned out to be false: that a real `pg_dumpall` file is just
several `pg_dump --create` outputs concatenated. It isn't. `pg_dumpall`
passes `--create` for ordinary databases but *not* for `postgres` and
`template1`, writing those `\connect` lines itself — which flips the order
of a segment's version headers relative to its `\connect` and exercises a
`PreambleBuilder` branch this stand-in cannot produce (I9). The global
objects dismissed below as "extra container surface" are also the only
fixture content that sits ahead of the first `\connect`, which Phase 3's
file map has to tile. See `roadmap-phase2-typed-columns.md`, "Fixtures".

The reasoning as it stood:

`pg_dumpall` itself was not run. What it produces — several complete
`pg_dump --create` child outputs concatenated — is exactly what the test
helpers build by hand from one already-generated fixture, per I9's proof
that each child's structure is independent (its own `RestoreArchive()`
call). Running real `pg_dumpall` would additionally require a second
throwaway database (to make the concatenation meaningful) and would dump
global objects (roles, tablespaces) ahead of the first `\connect` that nothing
in this codebase parses yet — extra container surface for no additional
parser coverage, since the parser's `\connect`-segmentation logic doesn't
distinguish `pg_dumpall`'s wrapper from a hand-built one. If global-object
parsing (`CREATE ROLE`, etc.) ever becomes in scope, that would be the
reason to add a real `pg_dumpall`-driven fixture; it isn't needed for the
gap this phase closes.

## Where the gap this surfaced went

Settled as **slice 2.3.3**: the error the design doc promised gets built, and
generalized. The tests this slice added become the tests 2.3.3 inverts — they
pin behavior that is now scheduled to change, which is what they were for.

The generalization came from a second collision this slice's evidence made
visible: `CopyHeader::matches` accepts a bare name against any schema, so the
same silent union happens *within* one database, no `\connect` involved. The
rule is therefore one target per query — `(database, schema, table)` — not a
multi-database special case. Full specification in
`roadmap-phase2-typed-columns.md`, "One target per query"; the corruption
argument for scheduling it ahead of 2.4 is there too.
