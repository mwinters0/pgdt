# PostgreSQL invariants we rely on

Every entry is a property of `pg_dump`'s output that some design decision
treats as guaranteed. Each records what the invariant is, the source that
proves it, the versions it was verified against, and how to re-verify it.

**This exists because a new PostgreSQL major release can quietly invalidate
one of these**, and the resulting bug would surface as wrong data rather than
an error. When a new major lands, walk this file: re-run each re-verification
step against the new source tree, and update "Verified against" — or, if an
invariant broke, fix the design doc named in "Relied on by" before anything
else.

Source checkouts live at `/mnt/wd12t/upstream/postgres/` (worktrees per
release tag). All line numbers below are from `release-v18.6` and are a
starting point, not an anchor — grep for the quoted code instead.

---

## I1 — `CREATE EXTENSION` and `CREATE TYPE` always precede every `COPY` block

**Claim.** Within one database's dump, no extension or type definition can
appear after the first `COPY ... FROM stdin;`. A preamble scan may therefore
stop at the first `COPY` header with no fallback.

**Proof.** `addBoundaryDependencies()` in `pg_dump.c` classifies
`DO_EXTENSION`, `DO_TYPE` and `DO_SHELL_TYPE` as pre-data objects: the
pre-data boundary object depends on each of them, and every `DO_TABLE_DATA`
depends on that boundary, so the topological sort has no freedom to interleave
them. `dbObjectTypePriorities` in `pg_dump_sort.c` agrees (`PRIO_EXTENSION` 5,
`PRIO_TYPE` 6, `PRIO_PRE_DATA_BOUNDARY` 25, `PRIO_TABLE_DATA` 26) but is only
a tiebreak within the sort — the dependency is the real guarantee.

**Scope limit.** The invariant is **per database, not per file**. `pg_dumpall`
concatenates one `pg_dump --create` output per database, so a later database's
preamble legitimately follows an earlier one's data. Hence re-arming the
preamble search at each `\connect`.

**Verified against:** v13.0, v16.0, v18.6 — identical.
**Relied on by:** `roadmap-phase2-typed-columns.md` (preamble pass, early exit).
**Re-verify:** `grep -n 'addBoundaryDependencies' -A40 src/bin/pg_dump/pg_dump.c`
and confirm `DO_EXTENSION`/`DO_TYPE`/`DO_SHELL_TYPE` are still in the pre-data
arm, plus the `PRIO_*` ordering in `pg_dump_sort.c`.

---

## I2 — A table produces at most one `COPY` block per dump

**Claim.** A given schema-qualified table name yields exactly zero or one
`COPY` block in one `pg_dump` output. Several blocks under one name can only
come from concatenation — `pg_dumpall`, or cat'ed dump files — i.e. from
different databases.

**Proof.** `makeTableDataInfo()` in `pg_dump.c` returns immediately if
`tbinfo->dataObj != NULL`, so at most one `TABLE DATA` entry is ever created
per table. It also returns early for views, for partitioned tables
(`/* Skip partitioned tables (data in partitions) */` — the partitions are
separate tables with their own names), for non-selected foreign tables, for
unlogged tables when those are excluded, and for tables in
`tabledata_exclude_oids`.

**Verified against:** v18.6.
**Relied on by:** `roadmap-phase2-typed-columns.md` (one schema per stream;
multi-database detection).
**Re-verify:** `awk '/^makeTableDataInfo\(DumpOptions/,/^}$/'
src/bin/pg_dump/pg_dump.c` — confirm the `dataObj != NULL` early return and
the `RELKIND_PARTITIONED_TABLE` skip.

---

## I3 — TOC header comments are always present in plain-format output

**Claim.** Every object in a plain-format dump is preceded by
`-- Name: <name>; Type: <desc>; Schema: <schema>; Owner: <owner>`, and every
`COPY` block by `-- Data for Name: <table>; Type: TABLE DATA; Schema: <s>;
Owner: <o>`. `--no-comments` does **not** suppress these; it suppresses
`COMMENT ON` statements, which are unrelated.

**Proof.** `_printTocEntry()` in `pg_backup_archiver.c` emits the header under
`if (!AH->noTocComments)`. `noTocComments` is assigned in exactly one place —
`AH->noTocComments = 1` in `RestoreArchive()`, inside the branch taken when
restoring directly to a database connection ("If we're talking to the DB
directly, don't send comments since they obscure SQL when displaying errors").
No command-line option sets it, and plain-format dump output never takes that
branch.

**Caveat, not a scope limit.** `sanitize_line()` collapses newlines in the tag,
so the header is always a single line — but a dollar-quoted function body could
still *contain* a line that looks like one. TOC comments are therefore a
segmentation hint, never a correctness guarantee: the statement grammar
validates what the comment announced.

**Verified against:** v18.6.
**Relied on by:** `roadmap-phase2-typed-columns.md` (TOC comment as segmenter).
**Re-verify:** `grep -rn 'noTocComments' src/bin/pg_dump/` — confirm the only
assignment is still the direct-connection one in `RestoreArchive()`.

---

## I4 — COPY TEXT data is written with `DATESTYLE = ISO` and `extra_float_digits = 3`, but no `TimeZone` or `IntervalStyle`

**Claim.** Temporal values in COPY TEXT are ISO-formatted, and `timestamptz`
carries an explicit UTC offset — so the *instant* is unambiguous. The
dump-time session's `TimeZone` is **not** recorded anywhere in the file, so the
offset is whatever that session had (koji's is `+00`). `IntervalStyle` is
never set, so an `interval` value's text is in the server's default style and
is **not** determined by the file.

**Proof.** `pg_dump.c` runs `ExecuteSqlStatement(AH, "SET DATESTYLE = ISO")`
and `SET extra_float_digits TO 3` (or the `--extra-float-digits` override) on
the *source connection*. Neither statement is written into the dump:
`_doSetFixedOutputState()` in `pg_backup_archiver.c` emits `statement_timeout`,
`lock_timeout`, `idle_in_transaction_session_timeout`, `transaction_timeout`,
`client_encoding`, `standard_conforming_strings`, the search_path, ROLE,
`check_function_bodies`, `xmloption`, `client_min_messages` and `row_security`
— and no temporal or float settings at all.

**Observed shape.** koji: `2016-04-13 16:52:35.456696+00`, with fractional
seconds trailing-trimmed to between 0 and 6 digits (`…10.41925+00`,
`…52.27108+00`). Booleans render as `t`/`f`.

**Verified against:** v18.6 source; koji (`pg_dump 16.14`) data.
**Relied on by:** `roadmap-phase2-typed-columns.md` (temporal mapping;
`interval` left as a string), `docs/manual/type-handling.md`.
**Re-verify:** `grep -n 'DATESTYLE\|extra_float_digits' src/bin/pg_dump/pg_dump.c`
and `awk '/_doSetFixedOutputState\(ArchiveHandle/,/^}$/'
src/bin/pg_dump/pg_backup_archiver.c` — confirm no `SET TimeZone` /
`SET IntervalStyle` appears in the emitted output.

---

## I5 — The `COPY` column list excludes dropped and generated columns

**Claim.** A `COPY` header's column list can legitimately be a strict subset of
the columns in the table's `CREATE TABLE`, and the two can never be assumed to
match position-for-position. It can also be **absent entirely**.

**Proof.** `fmtCopyColumnList()` in `pg_dump.c` skips every attribute with
`attisdropped[i]` or `attgenerated[i]`, and returns `""` — no parentheses at
all — when that leaves no columns. Separately, under `--binary-upgrade`,
`dumpTableSchema()` re-creates dropped columns in the `CREATE TABLE` body as
`INTEGER /* dummy */`, so the DDL there contains columns the data never will.

**Verified against:** v18.6 (source); the `dropped_column`/`generated_column`
tables in `scripts/fixture_schema_edge_cases.sql` reproduce both shapes in
real `pg_dump` output on 13.23/16.15/18.6 — the dummy column only appears
under `--binary-upgrade`, matching the gate above.
**Relied on by:** `roadmap-phase2-typed-columns.md` ("the `COPY` header is
authoritative; the DDL is a by-name type lookup").
**Re-verify:** `awk '/^fmtCopyColumnList\(/,/^}$/' src/bin/pg_dump/pg_dump.c`.

---

## I6 — Under `--binary-upgrade`, enum labels follow the type as `ALTER TYPE ... ADD VALUE`

**Claim.** A `--binary-upgrade` dump emits `CREATE TYPE x AS ENUM (\n);` with an
**empty body**, then one `ALTER TYPE x ADD VALUE '<label>';` per label. The
labels are still in the preamble, in the same TOC entry — so they are
recoverable without scanning further into the file. A label extractor that only
reads the `AS ENUM ()` body silently yields an empty enum.

**Proof.** `dumpEnumType()` in `pg_dump.c` gates the label loop on
`if (!dopt->binary_upgrade)`, then under `if (dopt->binary_upgrade)` emits
`-- For binary upgrade, must preserve pg_enum oids`, a
`binary_upgrade_set_next_pg_enum_oid('<oid>'::pg_catalog.oid)` call, and
`ALTER TYPE %s ADD VALUE ` per label.

**Verified against:** v18.6 (source); `scripts/fixture_schema_types.sql`'s
`public.mood` enum reproduces this exactly in real `pg_dump --binary-upgrade`
output on 13.23/16.15/18.6 (`fixtures/<version>/types/binary-upgrade.sql`) —
empty `AS ENUM ()` body, one `binary_upgrade_set_next_pg_enum_oid` +
`ALTER TYPE ... ADD VALUE` pair per label, all still ahead of the first
`COPY` block. The same dump also shows this OID-preservation noise
interspersed before *every* object's real statement (tables included, not
just types) — worth knowing for the 2.2 preamble parser, since the "TOC
comment segments, then a strict grammar parses the statement" design needs
to tolerate that noise between the two under `--binary-upgrade`.
**Relied on by:** `roadmap-phase2-typed-columns.md` (enum resolution).
**Re-verify:** `awk '/^dumpEnumType\(Archive/,/^}$/' src/bin/pg_dump/pg_dump.c`.

---

## I7 — `LF 5C 2E` cannot occur at the start of a data line

**Claim.** Within a `COPY` block, a single `memmem` search for the 3-byte
needle `LF \ .` finds the block terminator directly and safely; no newline
enumeration is needed.

**Proof.** COPY TEXT doubles backslashes in field values, so a data row whose
content is `\.` appears in the file as `\\.` — in `LF 5C 5C 2E` the dot is not
directly preceded by newline-backslash. A match is therefore always the real
terminator.

**Verified against:** koji (19.58B rows, no false terminator); the
`public.escapes` round-trip on `pg_dump` 13.23 / 16.15 / 18.6.
**Relied on by:** `roadmap-phase6-scan-performance.md` (structure discovery).
**Re-verify:** the `public.escapes` fixture test already asserts the escaping
rule this rests on; a new major that changed it would fail that test.

---

## I8 — Built-in types are unqualified in DDL; user-defined types are schema-qualified

**Claim.** In a `CREATE TABLE` column list, a built-in type appears bare
(`integer`, `text`, `timestamp with time zone`, `character varying(16)`) and a
user-defined type appears schema-qualified (`public.mood`,
`public.pgstattuple_type`). So a declared type containing a `.` is a cheap,
reliable first discriminator, and a user type's lookup key is its qualified
name — the same form its `CREATE TYPE` statement uses.

**Proof.** `pg_dump` runs with `search_path` emptied — `ALWAYS_SECURE_SEARCH_PATH_SQL`
on the connection, and `SELECT pg_catalog.set_config('search_path', '', false);`
written into the dump by `dumpSearchPath()`. With nothing on the path,
`format_type()` qualifies every type not in `pg_catalog`, while built-ins keep
their standard SQL spellings.

**Observed.** koji declares `RETURNS public.pgstattuple_type` (qualified,
user-defined) alongside `RETURNS integer` and
`RETURNS timestamp with time zone` (bare, built-in) — all `format_type` output
through the same path. No koji *column* uses a user-defined type, so
`fixture_schema_types.sql` needs to cover that case.

**Verified against:** v18.6 source; koji and all three fixture versions emit
the empty-`search_path` line.
**Relied on by:** `roadmap-phase2-typed-columns.md` (type resolution).
**Re-verify:** `grep -n 'dumpSearchPath' -A45 src/bin/pg_dump/pg_dump.c`, and
confirm fixtures still contain `set_config('search_path', '', false)`.

---
