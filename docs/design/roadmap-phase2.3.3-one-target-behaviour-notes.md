# Phase 2.3.3 notes — one target per query, `MetadataNotScanned`, multirange recognition

The three behavioural items `docs/status/STATUS.md` deferred behind slice
2.3.2's fixture work, now built and tested against the fixtures that slice
added (real `pg_dumpall` output, and the range/multirange/base/shell type
coverage in `fixture_schema_types.sql`).

## Per-`CopyBlock` database attribution

`CopyBlock` gained `database: Option<String>` (`index.rs`), populated from
`PreambleBuilder::current_database_name()` (new accessor: `self.current.name.clone()`)
at the point a block's `CopyEnd` is recorded in `build_index` — safe to read
there rather than at `CopyStart`, since a `\connect` cannot occur mid-block.

`table_stream`'s live (uncached) scan needed its own tracking, since it
never runs `PreambleBuilder` at all: `preamble::parse_connect` (made
`pub(crate)`) is applied to every `Event::Line` the live loop already
receives, updating a `current_database: Option<String>` local. This is *not*
"reading preamble as it goes" in the sense `docs/design/roadmap-phase2-typed-columns.md`
rules out — `parse_connect` yields a name, never a column type, so the
resolved schema still can't change mid-stream.

**Seeding `current_database` for a fresh (non-resumed) scan** needed two
cases, not one, discovered by a real test failure against the `dumpall`
fixture: when `base_index.blocks` already has entries, the block with the
greatest `end_offset` names the database in scope at the watermark (a
`\connect` between that block and the watermark, if any, gets witnessed live
by this same call before it matters). But under `CacheMode::Disabled` — or
any cold start — no blocks are known yet, and `scanned_through` instead
reflects only the preamble prepass's own watermark, sitting exactly at the
file's first `COPY` block. At that point the truth is `metadata.databases`'
*first* entry, not `None` — the bug's first symptom was `MetadataNotScanned
{ database: None }` on a block that was genuinely in the second database,
because the seed silently defaulted to `None` instead of consulting
metadata. Fixed by falling back to `metadata.databases.first()`'s name when
no block exists to derive it from (I1: nothing precedes any database's
first block, so the very first stopping point in the file is necessarily
still within the first database).

`ResumeToken` (and `InCopyResume`) each gained their own `database: Option<String>`
so a resumed stream carries this state across the pause rather than losing
it — the same reasoning `InCopyResume` already applied to header/field-count.

## `resolve.rs`: a fact, not a guess

`database_for` (first database whose DDL mentions the table) is now
`database_for_name` (exact match on `DatabaseMetadata::name`).
`resolve_columns` gained a `database: Option<&str>` parameter — the caller
now always has one, from the matched block's own attribution, so the lookup
never has to guess. `resolve.rs`'s
`ambiguous_table_across_databases_resolves_against_the_first_match` test —
which 2.3.1's notes flagged as pinning down behavior "scheduled to
change" — is replaced by `database_selects_by_attributed_name_not_by_first_match`,
proving the same two-databases-differing-types setup now resolves whichever
name is passed, not whichever database happened to come first.

## `Error::AmbiguousTable` and the `--database` way out

`table_stream` narrows `blocks_for(&table)`'s name-only matches to at most
one `(database, qualified_name)` candidate:

- **Known (cached) blocks**: filtered by `batch_options.database` first (if
  given), then checked for a second distinct candidate — before any
  streaming starts, exactly as the design intended for the warm-cache case.
- **Live discovery**: the same check runs incrementally as each new
  matching block is found, since a cold scan cannot know the full candidate
  set until it reaches one. `target: Option<(Option<String>, String)>`
  tracks whatever candidate has been committed to so far; a second, differing
  one raises `Error::AmbiguousTable { name, candidates }` immediately.

**A cold query can still emit some rows before erroring.** If the first
candidate's block sits earlier in the file than the second, its rows are
already streamed to the caller by the time the live scan discovers the
conflict — "before any streaming starts" only holds when the candidate set
is already known (a warm cache, or a repeat query after a `pgdq parse`).
This is not silent corruption — the emitted rows are genuinely that
database's own correct rows, not a union — but a caller must still treat any
output preceding a stream error as incomplete, the same as every other
mid-stream error this codebase already raises (`LineTooLong`,
`UnterminatedCopyBlock`). Buffering output until EOF to avoid this would
defeat the point of streaming; not attempted.

`BatchOptions` gained `database: Option<String>`, matched against
`DatabaseMetadata::name`. The CLI's `pgdq query` gained `--database`.

## `Error::MetadataNotScanned`

`stream::resolve_block` checks, only in `SchemaMode::Typed`, that
`metadata.databases` has an entry for the block's attributed database *and*
that entry's `preamble_complete` is `true`; otherwise it returns
`Error::MetadataNotScanned { database }` rather than resolving every column
`NotDeclared`. Reachable only through `table_stream`'s incremental path: a
`build_index`/`pgdq parse` full scan always leaves every database it found
`preamble_complete`, so this is specifically the "queried a later
`\connect`ed database's table without first running `pgdq parse`" case the
design doc named.

**The CLI's own remedy didn't exist yet.** The error message points at
`--schema-mode strings`, but `pgdq query` had never grown that flag in slice
2.3 (only the library's `SchemaMode` did). Added `--schema-mode
<typed|strings>` (`CliSchemaMode`, mapped to `SchemaMode`) so the message's
advice is real rather than a broken promise — caught by manually exercising
the error against a live multi-database file, not by an automated test
(no test asserts CLI `--help` text or flag existence in this codebase).

## Multirange recognition

`TypeKind::Range` gained `multirange_type_name: Option<String>`, captured
by `preamble::parse_create_type`'s `AS RANGE` branch alongside `subtype`
(both are `key = value` parameters in the same top-level-comma-split list).
`pgtype.rs::map_builtin` recognizes the six built-in multirange names
(`int4multirange` … `datemultirange`) as bare, exactly like the six built-in
ranges. `resolve_user_type` falls back to scanning `types` for a `Range`
whose `multirange_type_name` matches the requested name when no direct
`TypeDef` exists — the only way to resolve a range's auto-created companion,
since `pg_dump` never emits a `CREATE TYPE` for it at all (I10).

Verified end to end against `fixtures/16/types/default.sql` via `pgdq info
--verbose`: `t_multirange.v_int4multirange` and `t_multirange.v_myrange_multi`
(the companion, named only inside `myrange`'s own DDL) both report
"deferred (range) — decodable, not yet implemented" rather than "unknown
type".

## `pgdq info` groups blocks by database

Once blocks carry attribution, `print_index` prints a `database: <name>`
header whenever a listing spans more than one — the single most common case
(one database) still prints nothing extra. This is what makes an
`AmbiguousTable` error's candidate names (`database.schema.table`)
actionable: they're names this listing already showed.

## Test changes

- `resolve.rs`: `database_selects_by_attributed_name_not_by_first_match`
  replaces the old first-match test (see above).
- `pgtype.rs`: `builtin_multirange_types_are_deferred_not_unknown`,
  `a_ranges_multirange_companion_resolves_even_with_no_type_def_of_its_own`.
- `preamble.rs`: `parses_a_range_type_with_a_multirange_companion` (real
  multi-line, multi-word-subtype, `multirange_type_name` shape).
- `tests/pgtype.rs`: `resolution_still_works_against_metadata_with_more_than_one_database`
  now selects *each* database by name explicitly and checks it resolves
  against its own declared types, rather than only proving the first
  database's lookup doesn't crash.
- `tests/stream.rs`: `querying_a_table_name_shared_by_two_databases_silently_unions_both`
  (2.3.1's pinned-down gap) is replaced by
  `querying_a_table_name_shared_by_two_databases_errors_without_a_database_selector`,
  `database_selector_resolves_the_ambiguity_to_the_first_databases_rows`, and
  `selecting_a_later_databases_table_needs_strings_mode_or_a_prior_full_scan`
  — one test per outcome (error, first-database success, second-database
  `MetadataNotScanned`-then-`Strings`-success) rather than one test proving
  the old union.

## Cache format

`CopyBlock::database` and the two `ResumeToken`/`InCopyResume` additions are
new fields on already-`serde`-derived types; no `format_version` bump, per
the same reasoning `roadmap-phase2-typed-columns.md`'s "What the cache
stores" already applied to 2.2's metadata addition — pre-1.0 carries no
compatibility obligation, and there are no `.dqcache` files in existence to
worry about deserializing wrong.
