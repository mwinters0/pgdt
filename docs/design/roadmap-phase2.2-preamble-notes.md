# Phase 2.2 notes — preamble parsing, `DumpMetadata`, `pgdq info`

Recovers dump-level metadata (server/`pg_dump` versions, extensions,
user-defined types, per-column declared types) from the pre-data region of a
`pg_dump` plain-format file, and surfaces it through `pgdq info`. No Arrow
typing yet — every column is still `Utf8View` (Phase 2.3).

## What landed

- **`pgdump_query::preamble`** (new module): `DumpMetadata` / `DatabaseMetadata`
  / `Extension` / `TypeDef` / `TypeKind`, and the crate-private
  `PreambleBuilder` that incrementally constructs a `DumpMetadata` from a
  stream of outside-block lines. Declared types are stored as strings exactly
  as `pg_dump` wrote them (`character varying(16)`, not a parsed pair) — see
  "What the cache stores" in the phase doc; resolving them into Arrow types is
  Phase 2.3.
- **`crate::scan::Event` gained a `Line` variant**: every outside-block line
  that is neither a `COPY` header nor touched by dollar-quote tracking is now
  handed to the caller instead of silently discarded. This is what feeds
  `PreambleBuilder` — cheap, since outside-block lines are the DDL preamble
  (a few thousand lines even for a multi-database dump), never a `COPY`
  block's billions of data rows. `build_index` acts on every `Line` it sees
  (a full scan, so it captures every database's preamble); `table_stream`'s
  own live-scan loop just adds a no-op arm — its metadata capture is a
  separate, bounded prepass added in Phase 2.2.1
  (`docs/design/roadmap-phase2.2.1-incremental-preamble-notes.md`), not
  something the loop's own `Line` events feed. Per-column type *resolution*
  from any of this remains Phase 2.3's job.
- **`build_index` now always populates `DumpIndex::metadata`** — one pass
  serves both structure and metadata, since `PreambleBuilder` only looks at
  lines the scan already walks. `DumpIndex::metadata` is no longer
  reserved-and-unpopulated; `tests/cache.rs`'s round-trip test updated
  accordingly (it now pins that `Some(DumpMetadata { .. })` round-trips, not
  that the field stays `None`).
- **`pgdq info`** gains, always: a short per-database header (server version,
  `pg_dump` version, extension count, user-defined type count), and each
  column's declared type beside its name in the per-block listing. A single
  unnamed database (the common case — no `--create`) gets no `database:
  <name>` line, since one would be pure noise; a real multi-database dump
  would show one per database. Per-column resolution diagnostics and the "N
  of M columns unmapped" summary line are Phase 2.3 (they need the Arrow
  mapping, `crate::pgtype`, which doesn't exist yet).
- **Cache format version not bumped** — per the phase doc's own call: a cache
  written without metadata (Phase 1's shape) still decodes cleanly as
  `metadata: None` under the new, larger `DumpMetadata`, since bincode's
  `Option::None` never touches the inner type's fields. There are no cache
  files in the wild to worry about regardless.
- **`docs/design/postgres-invariants.md` gained I9** (version headers print
  once per `pg_dump` invocation, not once per `\connect` segment) — see
  "What generation corrected" below.

## Grammar approach

The phase doc's own description ("TOC comment as segmenter, then a strict
grammar parses the statement") is followed loosely rather than literally: the
implementation dispatches directly off five fixed line-start keywords
(`CREATE TABLE`, `CREATE TYPE`, `CREATE DOMAIN`, `CREATE EXTENSION`, `ALTER
TYPE`) rather than modeling the `-- Name: ...; Type: ...` TOC-comment grammar
as a separate segmentation pass. The soundness property the TOC-comment
design was after — never guessing, only acting on a fully-recognized shape —
holds either way: an unrecognized line (a TOC comment, a `SELECT
pg_catalog.binary_upgrade_*` call, `CREATE FUNCTION`, `ALTER ... OWNER TO`,
blank lines) is simply never matched to a trigger keyword and is ignored,
identically to how the TOC-comment version would have skipped an object type
it doesn't care about. This is a **deliberate simplification worth a second
look** if a future session hits a real-world DDL shape it doesn't parse
cleanly — the TOC comment is still available as a segmentation fallback if
the keyword-only approach turns out to be too fragile in practice. Nothing in
the fixture matrix or the koji sample exercised a gap in it.

Statement accumulation: once a trigger line is seen, subsequent outside lines
are appended (rejoined with `\n`) until the buffer is a complete statement —
parens balanced (respecting `'...'`/`''`-escaped string literals so a label
like `'has,comma'` can't be misread as a delimiter), not mid-string, ending in
`;`. `pg_dump`'s own DDL never puts a semicolon inside a nested expression for
any of these five statement shapes, so this is sufficient without parsing
expression syntax at all — including through a `--binary-upgrade` dump's
`SELECT pg_catalog.binary_upgrade_set_next_*_oid(...)` noise between every
object's TOC comment and its real statement (confirmed: I6's note in
`docs/status/history/2026-08-22.md`, and this slice's own
`binary_upgrade_oid_noise_between_comment_and_statement_is_skipped` /
`binary_upgrade_dump_yields_the_same_enum_labels_via_alter_type` tests), since
none of those noise lines start with a trigger keyword and so are never
absorbed into a pending statement at all.

A declared type string is recovered without parsing SQL type grammar: `pg_dump`
never puts a space inside a type's own parenthesized modifier
(`numeric(38,10)`, `character varying(16)`), so whitespace-tokenizing the tail
after a column/field name and stopping at the first column-constraint keyword
(`NOT`, `DEFAULT`, `COLLATE`, `GENERATED`, `PRIMARY`, `REFERENCES`, `CHECK`,
`UNIQUE`, `CONSTRAINT`) isolates it. A `--binary-upgrade` dummy column's type
carries a C-style comment (`INTEGER /* dummy */`, I5) that the same
tokenizing would otherwise swallow whole; comments are stripped first.

## Multi-database handling

**A `--create` dump's pre-`\connect` segment is dropped as a database entry,
but its version headers are carried forward onto the database the first
`\connect` switches into.** Before the first `\connect`, `pg_dump --create`
output is just `CREATE DATABASE ...;` for whichever database initiated the
dump — never a real table or type — so keeping it as a `name: None` entry
would be misleading noise, and `name: None` is reserved for the genuine case
(a plain, non-`--create` dump, which never has a `\connect` at all).

This needed a correction mid-slice: the initial implementation dropped the
version headers along with the rest of the pre-connect segment, which is
wrong for exactly the common case it needs to be right for — see I9. Every
`--create` dump (koji included) prints its version headers exactly once, at
the very top, ahead of the `CREATE DATABASE`/`\connect` — never repeated
after connecting into the database being described — so discarding the
segment they were read in would make `pgdq info` show no version at all for
any `--create` dump. Fixed by having `on_connect`'s first transition carry
`server_version`/`pg_dump_version` over into the freshly-started database
rather than letting them go with the rest of the discarded segment. A later
`\connect` (only reachable in a real multi-database `pg_dumpall` file, not
exercised by any fixture) needs no such carry-over: `pg_dumpall` runs a
separate `pg_dump --create` child process per database and concatenates their
full outputs, so each database's segment holds its own version-header pair
from its own `RestoreArchive()` call, captured independently the normal way.

Verified against the real koji sample (not just the fixture matrix, which has
no `--create` dumps at all): truncating the file to its preamble plus one
small table's `COPY` block (`lock_monitor.activity`, ending at the first
line-anchored `\.`) and running `pgdq info` against that fragment reproduces
`database: koji`, `server version: 16.14`, `pg_dump version: 16.14`,
`user-defined types: 1` (`pgstattuple_type`, a bare composite with no owning
extension — matches the census in `docs/status/history/2026-08-22.md`), and
all 12 of `lock_monitor.activity`'s columns typed correctly. A full fresh
`pgdq parse`/`info` scan of the real 784 GB file was **not** run this
session — that's an hour-long process on the HDD and this was an unattended
run (see `CLAUDE.md`'s rule on long-running processes) — so the preamble pass
is unverified past the first COPY block on real koji bytes; nothing in the
design suggests that matters (I1 says nothing of interest follows), but a
future session with time to spend a container-hour could confirm it end to
end.

## Testing

- `pgdump_query/src/preamble.rs` unit tests: each grammar shape in isolation
  (table, enum, domain, domain-over-domain, composite, range, base, shell,
  extension with/without schema, binary-upgrade enum labels via `ALTER TYPE`,
  the OID-noise interleaving, the dummy-column comment/quoted-identifier
  shape, `\connect` database-switching and the pre-connect-segment-drop/
  version-carry-over behavior, a `COPY` block completing the current
  database).
- `pgdump_query/tests/preamble.rs` (new, integration-level): runs `build_index`
  against the real generated fixtures across all 3 PostgreSQL versions —
  `fixtures/*/types/{default,binary-upgrade,data-only}.sql` and
  `fixtures/*/edge_cases/{default,binary-upgrade}.sql` — and asserts on the
  resulting `DumpMetadata` (every mapped column's declared type, the enum
  labels matching between the plain and binary-upgrade emission shapes, I5's
  dropped/generated-column DDL shapes). This is the same self-checking
  philosophy the round-trip decode tests use, applied as far as it can go for
  metadata: real `pg_dump` output is the input, though (unlike a round-trip)
  the expected values are still hand-asserted, since there is no oracle for
  "what type string should this be" other than reading the DDL.

## Not touched / deferred

- **Per-`CopyBlock` database association.** `CopyBlock` still doesn't record
  which database it belongs to. `pgdq info`'s column-type lookup instead
  searches every database's `tables` map and takes the first match — fine for
  diagnostic output, and moot for every fixture and koji (single-database).
  A real query needs this to be exact for a multi-database file ("a query
  never spans databases" in the phase doc); building it is Phase 2.3's
  `resolve.rs`, described there as "the join of a `COPY` header against
  `DumpMetadata`."
- **`CREATE TYPE ... AS RANGE` and base/shell types** have no fixture or koji
  coverage at all (both fixtures' `t_range` column uses the built-in
  `int4range`; a C-level base type can't be created from SQL). Their grammars
  are implemented from `pg_dump` source reading and unit-tested against
  hand-written statement text only — worth a second look if a real range/base
  type ever needs to be typed.
- **`table_stream`'s incremental live-scan segments still don't build
  metadata as they walk** — that matches this slice's own boundary, "proves
  the metadata pass on its own, with no typing involved." Phase 2.2.1
  (`docs/design/roadmap-phase2.2.1-incremental-preamble-notes.md`) closes the
  practical gap this left — a cache file's presence not implying *any*
  metadata, even after a full incremental scan to EOF — without waiting on
  2.3: a bounded prepass now guarantees the *first* database's preamble is
  captured by any cache-enabled call regardless. A **later** `\connect`-ed
  database's preamble in a multi-database dump is still only ever populated
  by `build_index`'s full scan; closing that gap (if it turns out to matter —
  every fixture and koji are single-database) is still open, and still most
  naturally Phase 2.3's, once something downstream needs it.
