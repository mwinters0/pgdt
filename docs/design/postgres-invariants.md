# PostgreSQL invariants we rely on

Every entry is a property of `pg_dump`'s output — or, where a decision turns on
what the *server* accepts rather than on what `pg_dump` emits, of PostgreSQL
itself — that some design decision treats as guaranteed. Each records what the
invariant is, the source that proves it, the versions it was verified against,
and how to re-verify it.

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
**Relied on by:** `architecture.md` ("Bounded preamble-only reads"), and —
through the scope limit above — two mechanisms that turn on where a metadata
computation may legally stand:

- The mapping pass restating `DumpMetadata` at **each** `\connect`ed
  database's first `COPY` block (`architecture.md`, "`parse` resumes, and saves
  as it goes"). The scope limit is what licenses it: the invariant is per
  database, so each database's first `COPY` header closes out that database's
  preamble exactly as the file's first one closes out the first database's.
  Without the scope limit the recurring boundary would not exist and only EOF
  would be legal.
- `ColumnResolution::MetadataNotScanned` (`architecture.md`, "Joining a header
  against the metadata"). Given the above, no mapping scan produces the
  condition any more: a database's DDL is stated before any of its blocks can
  be banked, so a block in the map always has its database covered. The variant
  answers for metadata built by some other scan.
**Re-verify:** `grep -n 'addBoundaryDependencies' -A40 src/bin/pg_dump/pg_dump.c`
and confirm `DO_EXTENSION`/`DO_TYPE`/`DO_SHELL_TYPE` are still in the pre-data
arm, plus the `PRIO_*` ordering in `pg_dump_sort.c`.

---

## I2 — One `COPY` header name can own several blocks in one dump

**Claim.** A schema-qualified name appearing in a `COPY ... FROM stdin;`
header may own **more than one** `COPY` block within a single `pg_dump`
output. This happens whenever load-via-partition-root is in effect for a
partitioned table: each leaf partition gets its own `TABLE DATA` entry, but
every one of them writes a header naming the **root** table. It is not
opt-in — `pg_dump` forces the mode on its own for a table hash-partitioned on
an enum column.

What *is* still one-per-table is the archive entry: `makeTableDataInfo()`
returns immediately if `tbinfo->dataObj != NULL`, so at most one `TABLE DATA`
entry exists per table, and it skips views, partitioned parents (`/* Skip
partitioned tables (data in partitions) */`), unselected foreign tables,
excluded unlogged tables, and `tabledata_exclude_oids`. The name in the entry
and the name in the header are simply not the same name.

**Proof.** `dumpTableData()` in `pg_dump.c` builds the header from `copyFrom`,
which is `fmtQualifiedDumpable(getRootTableInfo(tbinfo))` — not `tbinfo` —
when `tbinfo->ispartition && (dopt->load_via_partition_root ||
forcePartitionRootLoad(tbinfo))`. `getPartitioningInfo()` documents the forced
case: *"the only case for which we force that is hash partitioning on enum
columns, since the hash codes depend on enum value OIDs which won't be
replicated across dump-and-reload."*

Observed directly against a live server (pg_dump 16.15), with **no flags**:

```
-- Data for Name: feelings_0; Type: TABLE DATA; Schema: public; Owner: postgres
--
-- load via partition root public.feelings
COPY public.feelings (id, m) FROM stdin;
...
-- Data for Name: feelings_1; Type: TABLE DATA; Schema: public; Owner: postgres
--
-- load via partition root public.feelings
COPY public.feelings (id, m) FROM stdin;
```

Two consequences beyond the count, both load-bearing:

- **The TOC entry's `Name:` is the partition; the `COPY` header's name is the
  root.** Any code that assumes the two agree is wrong for this shape.
- **Plain-format output carries a `-- load via partition root <root>` marker
  line** between the TOC comment and the header (from the entry's `defn`
  comment), so the shape is detectable in the file itself — and the pre-data
  section lists every `ALTER TABLE ... ATTACH PARTITION`, so the set of leaf
  partitions is knowable before any data is reached (I1). The marker survives
  `--no-comments`, `--data-only`, both together, and `--inserts` (observed,
  16.15): it is the entry's `defn`, not a `COMMENT ON` statement, so
  `--no-comments` does not touch it.

**The blocks are not contiguous, and must not be assumed to be.**
`DOTypeNameCompare()` in `pg_dump_sort.c` orders entries by (type priority,
namespace name, **object name**), and a `TABLE DATA` entry's object name is
the *partition's* own name — not the root's. Any unrelated table whose name
sorts between two partition names is emitted between their blocks. Observed,
flagless, 16.15, with partitions `feel_a`/`feel_z` of root `feel` and an
unrelated table `feel_m`:

```
-- Data for Name: feel_a; ...   COPY public.feel (id, m) FROM stdin;
-- Data for Name: feel_m; ...   COPY public.feel_m (note) FROM stdin;
-- Data for Name: feel_z; ...   COPY public.feel (id, m) FROM stdin;
```

So "read forward until the next block is a different table" does **not**
enumerate a name's blocks; only reaching EOF does.

**Scope limit.** Concatenation (`pg_dumpall`, cat'ed files) remains a
*separate* way for one name to own several blocks, across databases. This
entry is about a single database's dump.

**Verified against:** v16.15 (observed), source read v16.15 and v18.6.
**Relied on by:** `architecture.md` ("One target per query" — several blocks
under one key are legitimate, not an ambiguity; "Query: mapping and streaming
are separate passes" — a query cannot stop at the first matching block).
**Re-verify:** against any live server,

```sh
psql -c "CREATE TYPE mood AS ENUM ('sad','ok','happy')" \
     -c "CREATE TABLE feelings (id int, m mood) PARTITION BY HASH (m)" \
     -c "CREATE TABLE feelings_0 PARTITION OF feelings FOR VALUES WITH (MODULUS 2, REMAINDER 0)" \
     -c "CREATE TABLE feelings_1 PARTITION OF feelings FOR VALUES WITH (MODULUS 2, REMAINDER 1)" \
     -c "INSERT INTO feelings VALUES (1,'sad'),(2,'ok'),(3,'happy')"
pg_dump | grep -E '^COPY |^-- load via partition root'
```

expecting two `COPY public.feelings` headers. Add a table named to sort
between the two partitions (`feelings_m`) and confirm its block still lands
between them. Source side: confirm `dumpTableData()` still builds `copyFrom`
from `getRootTableInfo()`, that `DOTypeNameCompare()` still sorts on
`dobj.name` (the partition), and check `getPartitioningInfo()` for whether the
forced-mode set has grown beyond hash enum_ops.

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
**Relied on by:** `architecture.md` ("TOC enrichment").
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
**Relied on by:** `architecture.md` ("Type resolution" — temporal mapping;
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
**Relied on by:** `architecture.md` ("Joining a header against the metadata" —
the `COPY` header is authoritative; the DDL is a by-name type lookup).
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
**Relied on by:** `architecture.md` ("Type resolution", enums).
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
**Relied on by:** `roadmap-phase7-scan-performance.md` (structure discovery).
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
**Relied on by:** `architecture.md` ("Type resolution").
**Re-verify:** `grep -n 'dumpSearchPath' -A45 src/bin/pg_dump/pg_dump.c`, and
confirm fixtures still contain `set_config('search_path', '', false)`.

---

## I9 — The version header lines print once per `pg_dump` invocation, not once per `\connect` segment

**Claim.** `-- Dumped from database version` / `-- Dumped by pg_dump version`
are written exactly once, at the very top of a plain-format file (before any
`CREATE DATABASE`/`\connect`), never repeated when a `--create` dump switches
into the database it just created. A `pg_dumpall` file is the exception, but
only because it is several *separate* `pg_dump --create` invocations
concatenated — each one gets its own pair, at the top of its own segment,
for the same reason.

**Proof.** Both lines are written in `RestoreArchive()`
(`pg_backup_archiver.c`), unconditionally near the top of the function, ahead
of the `\restrict` token and any TOC-entry processing — there is exactly one
call to `RestoreArchive()` per plain-format output, regardless of how many
databases' worth of `\connect` segments that output goes on to contain.
`pg_dumpall` (`pg_dumpall.c`) does not merge multiple databases into one
`pg_dump` invocation at all: for each database it `find_other_exec()`s a
fresh `pg_dump` child process with `--create` and concatenates that child's
complete output (banner, version headers, `CREATE DATABASE`, `\connect`,
content) after the previous one's.

**Observed.** The koji sample (`pg_dump --create` against a single database)
has exactly one `-- Dumped from`/`-- Dumped by` pair, ahead of its one
`\connect koji` — confirmed by grepping the raw file, not inferred from
source alone.

**Consequence for `crate::preamble`.** The version headers describe the
whole `pg_dump` invocation, not the database whose segment happens to
contain them positionally. A `--create` dump's pre-`\connect` segment is
otherwise pure `CREATE DATABASE` noise (see `PreambleBuilder`'s docs) and
gets discarded — but discarding it naively would silently drop the version
headers for every single-database `--create` dump, including koji. They are
carried forward onto the database the first `\connect` switches into
instead.

**A later `\connect` needs the same carry-over, not none.** This invariant
originally claimed a later `\connect`-ed database's own pair needs no such
handling, "held within that database's own segment" — reasoning about
`pg_dumpall`'s child-process structure without a concatenated fixture to
check it against. The fixture tree has one (two `--create` fixtures
concatenated — `docs/design/architecture.md`, "Fixtures
tree") and found the opposite: a later child's version-header pair prints ahead of
*its own* `\connect`, exactly like the first child's does ahead of its
`\connect` — which puts those lines in `PreambleBuilder::feed_line` while
`current` is still the *previous* database's finished (`preamble_complete`)
segment, not the new one they describe, so they were silently dropped for
every database after the first. Fixed by staging a later segment's headers
separately (`PreambleBuilder::pending_headers`) and consuming them on that
segment's own `\connect`, the same carry-over the first segment already got,
just triggered on every `\connect` instead of only the first.

**`pg_dumpall` emits two segment shapes, and the header order flips between
them.** `dumpDatabases()` passes `--create` to its `pg_dump` child for
ordinary databases — those segments print their version headers *ahead of*
their own `\connect`, the `pending_headers` case above. But for `postgres`
and `template1` it passes **no** `--create` and writes `\connect <db>`
itself, under the comment "Since pg_dump won't emit a `\connect` command, we
must". Those segments print `\connect` *first*, so their headers arrive
afterwards, into a segment that is already `current` and not yet
`preamble_complete` — `feed_line`'s ordinary path, not the staging one.
Both databases are always dumped, so **every** real `pg_dumpall` file
contains both shapes. A hand-built concatenation of `--create` outputs
produces only the first, which is why a genuine `pg_dumpall` fixture is
added later.

**Both segment shapes are confirmed against a real `pg_dumpall`
run**, not just the hand-concatenated stand-in: `fixtures/{13,18}/edge_cases/dumpall.sql`
(`pg_dumpall --no-role-passwords` against the fixture cluster) show, on both
the oldest and newest routine versions, `template1`/`postgres` printing
`\connect` *then* their version-header pair, and `pgdq_fixture` printing the
pair *then* its own `\connect` — exactly the flip this invariant predicted
from source reading alone.

**Verified against:** v18.6 source (`RestoreArchive()`,
`pg_dumpall.c`'s `dumpDatabases()` per-database invocation and its
`postgres`/`template1` special case); koji
(`pg_dump 16.14`); two concatenated `--create` fixtures
(`pgdump_query/tests/preamble.rs`'s `multidb_fixture`, versions 13/16/18);
`fixtures/{13,18}/edge_cases/dumpall.sql`, a real `pg_dumpall` run (2.3.2).
**Relied on by:** `architecture.md` ("The preamble grammar and `DumpMetadata`",
"Multi-database (`\connect`) segmentation", "Fixtures").
**Re-verify:** `grep -n 'Dumped from database version' -B5
src/bin/pg_dump/pg_backup_archiver.c` — confirm it's still inside
`RestoreArchive()` and still unconditional-per-call; `grep -n
'"--create"' src/bin/pg_dump/pg_dumpall.c` — confirm pg_dumpall still shells
out to a fresh `pg_dump --create` per database rather than driving one dump
across all of them, and that the `postgres`/`template1` branch still writes
its own `\connect` instead.

---

## I10 — A range type's companion multirange type is never dumped; its name survives only inside the range's own DDL

**Claim.** `CREATE TYPE x AS RANGE (...)` auto-creates a companion multirange
type (PostgreSQL 14+), and `pg_dump` emits **no `CREATE TYPE` statement for
it at all** — in any flag combination. The only evidence the file carries that
the companion exists, or what it is called, is the
`multirange_type_name = <name>` parameter inside the range type's own
`CREATE TYPE ... AS RANGE` body. A column declared with that companion type is
therefore explained by nothing else in the dump.

Secondarily: the `AS RANGE` parameter list is always emitted **multi-line**,
one parameter per line, `subtype` first, and the subtype may be a multi-word
type name (`subtype = double precision`).

**Proof.** `selectDumpableType()` (`pg_dump.c`) reclassifies any type with
`typtype = TYPTYPE_MULTIRANGE` as `DO_DUMMY_TYPE` under the comment "skip
auto-generated array and multirange types", exactly as it does for
auto-generated array types — so no `dumpType()` ever runs for one.
`dumpRangeType()` writes `"CREATE TYPE %s AS RANGE ("` followed by
`"\n    subtype = %s"` and, when `rngmultitype` is non-null,
`",\n    multirange_type_name = %s"`. The multi-line layout is in the format
strings themselves, not a wrapper, and is identical back to v13 (which has
the `\n    subtype` line but no multirange parameter — multiranges postdate
it).

**Observed.** Probed against `postgres:16-alpine` (16.15): `CREATE TYPE
public.myrange AS RANGE (subtype = float8);` dumps as `CREATE TYPE
public.myrange AS RANGE (\n    subtype = double precision,\n
multirange_type_name = public.mymultirange\n);` — note `float8` came back
canonicalised to the two-word `double precision`. A table with a
`public.mymultirange` column dumps that column with no accompanying type
definition anywhere in the file.

**Consequence for `crate::pgtype`.** `TypeKind::Range` carries the companion
name so a column declared with it resolves to `DeferredKind::Range` instead
of `Unknown`; discarding the parameter would make that unrecoverable without
re-scanning the file. The six built-in multirange names are recognized in the
built-in table alongside the six built-in range names, since like them they
appear bare and never reach the user-defined lookup (I8). The range grammar
must tolerate a multi-line body and a multi-word subtype value.

**Verified against:** v18.6 source (`selectDumpableType()`,
`dumpRangeType()`), v13.23 source (`dumpRangeType()`, no multirange);
probed `pg_dump` 16.15; `fixtures/{13..18}/types/default.sql`'s
`public.myrange`/`public.myrange_multi` (2.3.2) — all six routine versions,
not just one probed container, and confirms the PG13/PG14+ split in the
`multirange_type_name` parameter's presence exactly.
**Relied on by:** `architecture.md` ("Type resolution" — the mapping table and
"Ranges and multiranges")
— `pgtype.rs`'s companion lookup and `TypeKind::Range::multirange_type_name`
are built directly on this invariant, not just tested against it.
**Re-verify:** `grep -n 'skip auto-generated array and multirange types' -A 4
src/bin/pg_dump/pg_dump.c` — confirm multiranges are still `DO_DUMMY_TYPE`;
`grep -n 'AS RANGE' -A 8 src/bin/pg_dump/pg_dump.c` — confirm the parameter
list is still emitted one-per-line and that `multirange_type_name` is still
the only trace of the companion.

---

## I11 — A base type is emitted twice under one name, and both it and a shell type are reachable without compiled C

**Claim.** `pg_dump` emits a C-level base type as **two** TOC entries sharing
one type name: first `CREATE TYPE <name>;` under `Type: SHELL TYPE`, then the
full `CREATE TYPE <name> (INPUT = ..., OUTPUT = ..., ...)` under `Type: TYPE`.
A genuinely never-completed shell type emits only the first, also under
`Type: TYPE` (not `SHELL TYPE`).

Both shapes — and a user-defined range type — are creatable from **pure SQL**
against a stock server, with no compiled extension, so the fixture generator
can produce all three.

**Proof.** Two separate emitters: `dumpShellType()` writes `"CREATE TYPE %s;\n"`
with `.description = "SHELL TYPE"`, and `dumpUndefinedType()` writes the same
statement for a type that never got I/O functions, under the ordinary `TYPE`
description — its comment distinguishes "this case from where we have to emit
a shell type definition to break a circular dependency". `DefineType()`
(`typecmds.c`) requires the shell to exist first ("we must already have a
shell type, since there is no other way that the I/O functions could have been
created"), which is what makes the pair unavoidable rather than incidental.
The I/O functions themselves need no C source: `LANGUAGE internal` bodies
naming the built-in symbols `textin`/`textout` satisfy `CREATE FUNCTION`
against a shell return type (the server emits `NOTICE: return type ... is
only a shell` and proceeds). Creating a base type requires superuser
(`errmsg("must be superuser to create a base type")`), which the fixture
containers run as.

**Observed.** Probed against `postgres:16-alpine` (16.15). This is the whole
recipe:

```sql
CREATE TYPE public.myrange AS RANGE (subtype = float8);
CREATE TYPE public.shellonly;
CREATE TYPE public.mybase;
CREATE FUNCTION public.mybase_in(cstring) RETURNS public.mybase
    AS 'textin' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION public.mybase_out(public.mybase) RETURNS cstring
    AS 'textout' LANGUAGE internal IMMUTABLE STRICT;
CREATE TYPE public.mybase (INPUT = public.mybase_in,
    OUTPUT = public.mybase_out, INTERNALLENGTH = VARIABLE,
    STORAGE = extended);
```

`pg_dump` then emits `mybase` twice (SHELL TYPE, then TYPE), `shellonly` once,
and `myrange` with its multirange parameter (I10).

**This was promoted from a one-off probe to permanent fixture
coverage**: the same three statements are now in
`scripts/fixture_schema_types.sql`, generated across all six routine
versions (`fixtures/<version>/types/default.sql`), backed by tables
(`t_base_type`, `t_user_range`) so the types round-trip through a real
`COPY` block rather than appearing only as bare `CREATE TYPE` statements.
The completed `mybase` body's parameter order differs slightly from the
hand-written recipe above — `pg_dump` emits `INTERNALLENGTH` first, then
`INPUT`/`OUTPUT`/`ALIGNMENT`/`STORAGE` — which is cosmetic, not a grammar
concern.

**Consequence for `crate::preamble` / `crate::pgtype`.** One name can own two
`TypeDef` entries, so `resolve_user_type`'s first-match lookup returns the
`Shell` entry for a completed base type. Harmless as long as `Base` and
`Shell` share the `OpaqueBaseType` outcome — but it is a first-match lookup
over a list that is *not* known to be unique by name, which is worth
remembering if those outcomes ever diverge. Also removes the standing excuse
that these shapes cannot be fixture-generated.

**Verified against:** v18.6 source (`dumpShellType()`, `dumpUndefinedType()`,
`DefineType()`); probed `pg_dump` 16.15; `fixtures/{13..18}/types/default.sql`
(2.3.2) — the same recipe as permanent fixture coverage across all six
routine versions, not a single probed container.
**Relied on by:** `architecture.md` ("Fixtures", "Type resolution").
**Re-verify:** `grep -n '"SHELL TYPE"' -B 12 src/bin/pg_dump/pg_dump.c` —
confirm `dumpShellType()` still emits a bare `CREATE TYPE x;` ahead of the
real definition; re-run the recipe above against the newest major.

---

## I12 — Large-object data is a line-oriented region that cannot contain a `COPY` block

**Claim.** In plain-format output, large-object *data* sits after every
`COPY` block and before post-data DDL, and its contents are ordinary
single-line SQL statements — `SELECT pg_catalog.lo_open('<oid>', 131072);`, a
run of `SELECT pg_catalog.lowrite(0, '\x…');`, `SELECT
pg_catalog.lo_close(0);` — each `BEGIN;`/`COMMIT;`-wrapped. **No line inside
it can be mistaken for a `COPY` header or a `\.` terminator**, because the
payload is a bytea hex literal and hex cannot contain a line break.

**How many archive entries this is split across changed at v17, and the
routine version matrix (13-18) spans both shapes** — confirmed by
`scripts/fixture_schema_objects.sql`'s two hand-built large objects, real
`pg_dump` output, all 6 versions (`fixtures/<version>/objects/default.sql`):

| | v13-16 | v17-18 |
|---|---|---|
| Definition entry (ownership/comment/ACL) | one `BLOB` entry per object | one `BLOB METADATA` entry per object |
| Data entry | **one `BLOBS` entry for every large object in the dump**, one shared `BEGIN;`/`COMMIT;` wrapping every object's `lo_open`/`lowrite*`/`lo_close` run back to back | **one `BLOBS` entry per object**, each with its own `BEGIN;`/`COMMIT;` |
| `lo_create` call | inside the `BLOB` definition entry | inside the `BLOB METADATA` definition entry (unchanged in *position*, only the entry's name) |

Large-object *definitions* (ownership, ACL, comments) are separate entries
that sort ahead of the pre-data boundary on both shapes.

**Proof.** `dbObjectTypePriorities` (`pg_dump_sort.c`) places
`PRIO_LARGE_OBJECT_DATA` between `PRIO_TABLE_DATA` and
`PRIO_POST_DATA_BOUNDARY`, and `PRIO_LARGE_OBJECT` (the definition entries)
before `PRIO_PRE_DATA_BOUNDARY` — so the data region sits in a fixed position
either way, never interleaved with table data. Through v16, `pg_dump.c`
creates exactly one archive entry named `BLOBS` (`.description = "BLOBS"`)
covering every large object in the database; v17 (`pg_dump.c`, "Create a
BLOBS data item for the group, too") splits large objects into groups and
gives each group its own `BLOBS` entry — a fixture with only two large
objects never exercises more than one group, so this evidence doesn't pin
down the grouping threshold, only that v17+ is no longer "always exactly
one." `StartRestoreLOs()`/`EndRestoreLOs()` (`pg_backup_archiver.c`) emit the
`BEGIN;`/`COMMIT;` wrapper (one per data entry, whatever it covers) when not
restoring to a live connection; `_StartLO()` emits the `lo_open(...)` line,
and `dump_lo_buf()`'s no-connection branch emits each chunk through
`appendByteaLiteralAHX()` as `SELECT pg_catalog.lowrite(0, %s);`.

**Size.** Chunks are `LOBBUFSIZE`-bounded but the region is not: hex encoding
roughly doubles the on-disk size of the objects, so a dump of a
large-object-heavy database can carry hundreds of gigabytes here. This is the
one inter-`COPY` gap that is **not** bounded by schema size.

**Consequence.** Two, in opposite directions. The scanner itself needs no
*correctness* defence: a bytea hex literal cannot contain a line break, so no
`COPY` header or `\.` terminator can hide inside the region. But a bare
`BEGIN;`/`COMMIT;` pair is line-anchored recognizable on its own — through
`StartRestoreLOs()`/`EndRestoreLOs()` are the *only* emitter of one
anywhere in plain `pg_dump` output, confirmed across the fixture set used to
verify this entry — so `crate::scan::CopyScanner` recognizes
`BEGIN;` as a large-object region opener at the scanner level (a new
`State::InLargeObjectRegion`, the same tier `State::InCopy` sits at) and skips
every line up to `COMMIT;` unread, rather than emitting `Event::Line` for each
one and paying the DDL keyword-dispatch grammar over what can be hundreds of
gigabytes.

On v17+, `crate::map::Builder` merges what the scanner reports as several
separate `BEGIN;`/`COMMIT;` regions back into **one** `Data(LargeObjects)`
span — not by re-reading each entry's TOC `Type:` field, but by the general
rule "any span push closes the pending region" plus the priority-band fact
above: since `BLOB METADATA`/`ACL`/`COMMENT` definition entries sort entirely
*before* the pre-data boundary and every `BLOBS` data entry sorts together in
its own contiguous band, nothing else can legitimately arrive between two of
a v17+ file's `BLOBS` entries, so a second `BEGIN;` arriving before anything
else has been pushed is unambiguously "the same region, continuing." A
malformed or non-`pg_dump` input that violates this ordering degrades to
several smaller spans instead of one (still correctly tiled) rather than
silently merging across unrelated content. The per-object OID is recoverable
from the TOC header alone on v17+ (`-- Data for Name: <oid>; Type: BLOBS`) but
only from the `lo_open('<oid>', ...)` line on v13-16, where every object's
data shares one entry; per-object spans are deferred either way, since the
phase's design treats the whole region as one span regardless of how cheaply
an OID could be recovered.

**Verified against:** v13.23 through v18.6 source (`pg_dump_sort.c`
priorities; `dumpLOs`/`BLOBS`/`BLOB METADATA` entries; `StartRestoreLOs`,
`_StartLO`, `dump_lo_buf` in `pg_backup_archiver.c`) and
real fixture output on all 6 routine versions
(`fixtures/<version>/objects/default.sql`) — koji still has no large objects,
so this remains fixture-only, no koji coverage.
**Relied on by:** `architecture.md` ("Bulk regions: one span kind, three
payloads"); `pg-dump-compatibility.md`.
**Re-verify:** `grep -n 'PRIO_LARGE_OBJECT_DATA' src/bin/pg_dump/pg_dump_sort.c`
— confirm it still sits between `PRIO_TABLE_DATA` and
`PRIO_POST_DATA_BOUNDARY`; `grep -n 'lowrite' src/bin/pg_dump/pg_backup_archiver.c`
— confirm the no-connection branch still emits one bytea literal per
statement; `grep -n 'description = "BLOB' src/bin/pg_dump/pg_dump.c` — confirm
whether a given major still uses `BLOB`/`BLOBS` per-database or `BLOB
METADATA` per-object.

---

## I13 — `pg_dump` emits table data only as `COPY … FROM stdin;` in TEXT format, or as `INSERT`

**Claim.** There is no `pg_dump` output in which a `COPY` block carries an
options clause. Data is emitted either as `COPY <table> [(<cols>)] FROM
stdin;` with COPY's default TEXT format, or — under `--inserts` /
`--column-inserts` — as `INSERT INTO` statements. No flag produces
`WITH (FORMAT csv)`, `WITH (FORMAT binary)`, a custom `DELIMITER`, or any
other `COPY` option.

**Proof.** `dumpTableData()` (`pg_dump.c`) branches on exactly one condition:

```c
if (dopt->dump_inserts == 0) {
    dumpFn = dumpTableData_copy;
    printfPQExpBuffer(copyBuf, "COPY %s ", copyFrom);
    appendPQExpBuffer(copyBuf, "%s FROM stdin;\n", fmtCopyColumnList(tbinfo, clistBuf));
} else {
    dumpFn = dumpTableData_insert;
}
```

The `COPY` branch is a fixed format string terminated by `FROM stdin;` — the
column list is the only variable part, and there is no code path that appends
anything after it. The server-side read is symmetric: `dumpTableData_copy()`
issues `COPY <table> <cols> TO stdout;` (or `COPY (SELECT …) TO stdout;` for
foreign tables and partition-root loads), again with no options clause.
`grep -rni csv src/bin/pg_dump/` matches nothing whatsoever. `pg_backup_tar.c`
independently assumes the same shape, validating that a stored `copyStmt`
ends in the literal `" FROM stdin;\n"`.

**Consequence.** Two. The scanner's `parse_copy_header` deliberately rejects
any `COPY` line carrying a trailing `WITH (...)` and treats it as ordinary
SQL rather than guessing — safe precisely because `pg_dump` never emits one,
so the only way to encounter it is in text that is not `pg_dump` output (or
inside a dollar-quoted body, where it must be ignored anyway). And
CSV-format `COPY` blocks are not a compatibility gap but a non-shape: see
`pg-dump-compatibility.md` and Phase 8 Track A in `roadmap.md`.

Note the asymmetry with `--inserts`: that one *is* a real output shape and a
real gap, scheduled in Phase 8 Track A. The two were bundled together in
`docs/design/historical/initial.md`'s future-options list; only one of them
exists.

**Verified against:** v13.23, v16.15 and v18.6 source (`dumpTableData()`,
`dumpTableData_copy()`, `pg_backup_tar.c`'s `copyStmt` check) — the `COPY`
format string is unchanged across all three. Also consistent with every
fixture in `fixtures/` and with koji (`pg_dump 16.14`), none of which
contains a `COPY` line with an options clause.
**Relied on by:** `pgdump_query/src/copy.rs`'s `parse_copy_header` (its doc
comment states the deliberate non-match for `WITH (...)`);
`pg-dump-compatibility.md` ("`COPY` header variants" and "CSV-format `COPY`
blocks"); `roadmap.md` (Phase 8 Track A).
**Re-verify:** `grep -n 'FROM stdin' src/bin/pg_dump/pg_dump.c` — confirm the
statement is still built from a fixed format string with no options clause;
`grep -rni csv src/bin/pg_dump/` — confirm it still matches nothing.

---

## I14 — `real`/`double precision` switch to scientific notation at a fixed exponent threshold, not by digit count

**Claim.** `float4out`/`float8out` render the shortest round-trip decimal
(I4 already establishes this part) in fixed-point form when the value's
decimal exponent is in `[-4, 6)` for `real` or `[-4, 15)` for `double
precision`, and in scientific notation (`d.ddde±NN`, sign always shown,
exponent zero-padded to at least 2 digits) otherwise. The threshold is the
literal constant (6 / 15) — not the number of significant digits the
shortest-round-trip representation happens to have, which is what a
textbook `%g` implementation switches on instead.

**Proof.** `float4out`/`float8out_internal` (`src/backend/utils/adt/float.c`)
call `float_to_shortest_decimal_buf`/`double_to_shortest_decimal_buf`, which
live in `src/common/f2s.c`/`d2s.c` (PostgreSQL's port of the public-domain
Ryu algorithm). Each file's `to_chars` picks the format directly on the
computed display exponent:

```c
// src/common/f2s.c
if (exp >= -4 && exp < 6)
    return to_chars_df(v, olength, result) + sign;
// src/common/d2s.c
if (exp >= -4 && exp < 15)
    return to_chars_df(v, olength, result + index) + sign;
```

`6` and `15` are `FLT_DIG`/`DBL_DIG`'s values written as literals in the
Ryu-derived formatter, not references to those macros (which are used
elsewhere in `float.c` only for the *non*-shortest-decimal fallback path,
`extra_float_digits <= 0`) — a coincidence of naming, not of code path, but
the threshold value is identical either way.

**Observed shape.** `real`: `1e5` → `100000`, `1e6` → `1e+06`. `double
precision`: `1e14` → `100000000000000` (`1e14`, 15 digits), `1e15` →
`1e+15`; `123456789012345` (15 digits, exponent 14) prints fixed,
`1234567890123456` (16 digits, exponent 15) prints
`1.234567890123456e+15` — digit count is not what switched it, the exponent
crossing 15 is. Confirmed against a running `postgres:16-alpine`
(`extra_float_digits = 3`) as well as the source above.

**Relied on by:** `pgdump_query/src/decode.rs`'s `render_f32`/`render_f64`
(`docs/design/architecture.md`, "Decoders and
render-back"), whose own fixed/scientific
decision uses `FLT_DIG`/`DBL_DIG` (6/15) as the threshold for exactly this
reason — matching digit-for-digit is necessary but not sufficient for the
round-trip test under `architecture.md`'s "Testing philosophy" to pass.
**Re-verify:** `grep -n 'exp >= -4' src/common/f2s.c src/common/d2s.c` —
confirm the literal thresholds are still `6` and `15`.

---

## I15 — `COPY TO`'s TEXT-format output uses a small, fixed escape set

**Claim.** `COPY ... TO` (what `pg_dump` uses for table data) escapes exactly
seven things in a field's text: the six control-character mnemonics `\b \f
\n \r \t \v` and a literal backslash (doubled: `\\`). Every other byte,
including other ASCII control characters and the high bytes of multibyte
UTF-8, is written unescaped. The octal (`\NNN`) and hex (`\xNN`) forms are a
`COPY FROM` *reader* convenience only — `COPY TO` never emits them.

**Proof.** `CopyAttributeOutText` (`src/backend/commands/copyto.c`) switches
on each byte: bytes `< 0x20` go through a `switch` that maps only `\b \f \n
\r \t \v` to their letter form (`case '\b': c = 'b'; break;`, etc.) and falls
through unescaped for anything else in that range unless it's the delimiter
byte; `c == '\\'` is the only non-control-character case that gets
backslash-prefixed. No code path in `copyto.c` ever writes `\NNN` or `\xNN`.

**Verified against:** `CopyAttributeOutText` in `release-v18.6/src/backend/commands/copyto.c`
(identical logic in `release-v16.15`), and the `public.escapes` fixture (one
row per `chr(n)` codepoint) round-tripping through
`pgdump_query::copy::encode_field`/`decode_field` byte-for-byte —
`copy_text_escaping_round_trips_through_postgres`, `tests/scan.rs`.

**Relied on by:** `pgdump_query/src/copy.rs`'s `encode_field`, which only
implements these seven escapes and is therefore *not* a general COPY-text
encoder — it is exactly `decode_field`'s inverse for text `pg_dump` could
have produced, no more.

**Re-verify:** `grep -n "case '\\\\b'" src/backend/commands/copyto.c` in a new
major's source — confirm the mnemonic set and the "no octal/hex on output"
shape are unchanged.

---

## I16 — The TOC comment's `Type:` vocabulary is closed, and its `Owner:` field is the only owner record for most objects

**Claim.** Two properties of the TOC header comment I3 guarantees is present.

*Closed vocabulary.* The `Type:` field ranges over a finite, source-enumerable
set: 57 string literals assigned to `ArchiveEntry.description` across
`src/bin/pg_dump/*.c`, plus seven computed values — `reltypename` ∈ {`TABLE`,
`VIEW`, `MATERIALIZED VIEW`, `FOREIGN TABLE`} and `keyword` ∈ {`FUNCTION`,
`PROCEDURE`, `CHECK CONSTRAINT`, `CONSTRAINT`} — for a universe of ~63 kinds.
No path constructs a description from catalog text, so the set cannot grow
except by an upstream source change.

*Owner is TOC-only for inherited-ownership objects.* `_printTocEntry()` emits
`ALTER … OWNER TO` only for objects whose ownership is independently settable.
Indexes, constraints, ACL entries, column defaults, `SEQUENCE OWNED BY` and
`SEQUENCE SET` entries inherit ownership from their parent and emit no such
statement, so their owner appears **only** in the comment's `Owner:` field.

`--no-owner` suppresses both halves consistently — the TOC field becomes
`Owner: -` and the statements are dropped — so the two sources can never
disagree.

**Proof.** Enumerable directly from source (see re-verify). Measured on the
koji sample (`pg_dump 16.14`, 603 TOC entries): `TABLE` 75, `SEQUENCE` 30,
`FUNCTION` 8, `SCHEMA` 2, `DATABASE` 1, `TYPE` 1 — all 117 emitting `OWNER TO`;
`FK CONSTRAINT` 191, `CONSTRAINT` 112, `INDEX` 59, `ACL` 34,
`SEQUENCE OWNED BY` 30, `DEFAULT` 30, `SEQUENCE SET` 30 — 486 entries, none
emitting `OWNER TO`. `--no-owner` behaviour observed in
`fixtures/18/edge_cases/no-owner.sql`: 22 `Owner: -` headers, zero `OWNER TO`.

**Scope limit.** Both properties are about `pg_dump`'s own archiver. A
`pg_dump`-compatible dump produced by other ecosystem tooling may carry no TOC
comments at all, which is why `architecture.md` treats them
as an enrichment layer over a statement-driven pass rather than as the primary
structure.

**Verified against:** v18.6 source; koji (`pg_dump 16.14`); fixtures at 18.6.

**Relied on by:** `architecture.md` ("The file map"; "TOC enrichment").

**Re-verify:**

```sh
grep -rhoP '\.description = "\K[^"]+' src/bin/pg_dump/*.c | sort -u   # the 57 literals
grep -n 'reltypename = \|keyword = ' src/bin/pg_dump/pg_dump.c        # the computed 7
```

Confirm no new `.description =` assignment reads a catalog value, and that
`_printTocEntry()`'s ownership branch still gates on the entry's own `owner`
field rather than emitting unconditionally.

---

## I17 — `INSERT`-format table data has no line-anchored statement boundary

**Claim.** In `pg_dump --inserts` / `--column-inserts` output, a statement can
span multiple physical lines: values are single-quoted SQL literals, and a
value containing a newline puts the remainder of its statement on the next
line, which begins with the literal's continuation rather than `INSERT INTO`.
There is therefore **no line-anchored marker** for the end of an `INSERT` run,
unlike a `COPY` block (I7) or the large-object region (I12). Finding a real
statement end requires tracking `'` with `''` doubling; `pg_dump` sets
`standard_conforming_strings = on`, so backslash escapes are not a concern.

A `TABLE DATA` TOC entry is also emitted for tables with **zero** rows, so an
`INSERT` run may legitimately contain no statements at all.

**Proof.** Observed directly in real `pg_dump` output.
`fixtures/*/edge_cases/inserts.sql`'s `public.escapes` table holds one row per
`chr(n)` codepoint; row 10 (`chr(10)`, a newline) emits

```
INSERT INTO public.escapes VALUES (10, '
');
```

across two physical lines, the second beginning `');`. Row 13 (`chr(13)`) does
the same. `public.empty_table` carries a `TABLE DATA` header with no statements
under it.

**Scope limit.** Applies to `INSERT`-format data only. `COPY` blocks and the
large-object region keep their line-anchored guarantees; this invariant exists
precisely because those two do not extend to the third bulk region.

**Verified against:** fixtures at 13.23 through 18.6.
**Relied on by:** `architecture.md` ("Bulk regions: one span kind, three
payloads"), which is why `INSERT` runs get a string-aware scan
rather than the prefix check the other two regions allow.
**Re-verify:** `grep -A1 "VALUES (10, '$" fixtures/*/edge_cases/inserts.sql`
— confirm the statement still breaks across lines on a newline-bearing value.

---

## I18 — The TOC header line's field grammar: fixed order, optional trailing `Tablespace:`, two shapes of "no value"

**Claim.** `_printTocEntry()` writes the header line (I3) as one `ahprintf`
call with fields in a fixed order and literal separators:

```
-- [Data for |Statistics for ]Name: <tag>; Type: <desc>; Schema: <schema>; Owner: <owner>[; Tablespace: <tablespace>]
```

`Tablespace:` is appended only when `te->tablespace` is non-empty and
`ropt->noTablespace` is unset — every other header lacks the suffix entirely,
not an empty one.

"No value" for `Schema:`/`Owner:` is written by `sanitize_line(str,
want_hyphen)`, and which shape appears depends on the **caller's
`want_hyphen`**, not on the field: `Schema:` always passes `want_hyphen =
true` (so an absent schema is always `-`), while `Owner:` passes
`ropt->noOwner ? NULL : te->owner` with `want_hyphen = true` too — **except**
some entry kinds (observed: `COMMENT`) pass `te->owner = ""` (empty string,
not `NULL`) directly, which `sanitize_line` leaves as `""` rather than
substituting `-` (the hyphen substitution only fires on a true `NULL`). So a
reader must treat both `-` and empty as "no value" for `Owner:`, but only `-`
for `Schema:`.

**Proof.** `pg_backup_archiver.c`'s `_printTocEntry()` (see re-verify for the
exact `ahprintf` call) and `dumputils.c`'s `sanitize_line()`. Observed in
fixtures: `fixtures/*/objects/verbose.sql` has `-- Name: EXTENSION
postgres_fdw; Type: COMMENT; Schema: -; Owner: ` (trailing space, empty
owner); `fixtures/*/edge_cases/no-owner.sql` (`--no-owner`) has `Owner: -`
throughout instead.

**Scope limit.** None — closed by the `objects` fixtures. `Tablespace:` is
exercised by `fixtures/<version>/objects/default.sql`'s
`objects.tablespaced_table` (all 6 routine versions:
`-- Name: tablespaced_table; Type: TABLE; Schema: objects; Owner: postgres;
Tablespace: fixture_ts`), created via `generate_fixtures.py`'s
`prepare_tablespace_dir` (`mkdir`/`chown` in the fixture container, then
`CREATE TABLESPACE fixture_ts LOCATION ...` in
`scripts/fixture_schema_objects.sql`). `TOC_PREFIX_STATS` is exercised by
`fixtures/18/objects/stats.sql` (`-- Statistics for Name: stats_table;
Type: STATISTICS DATA; Schema: public; Owner: -` — note the empty-owner
shape, distinct from `Owner: -`; not yet explained by I18's own
`Owner:`/`Schema:` grammar above, since `dumpRelationStats` doesn't set an
owner at all). **The flag that produces it is `--statistics`, not
`--with-statistics`** — the name this entry originally used, before slice
3.1.1's fixture generation went looking for the literal flag in `pg_dump.c`
and found no such option; `--with-statistics` does not exist in any version.
`map::parse_toc_header_line` recognizes all three prefixes; the boundary
signal `looks_like_toc_name_line` recognizes `TOC_PREFIX_STATS` but not
`TOC_PREFIX_DATA`, since only the former heads a statement rather than a
`COPY` block.

**Verified against:** v18.6 source; fixtures at 13.23 through 18.6 for the
`Schema:`/`Owner:` placeholder shapes and the `Tablespace:` suffix; v18.6
fixture for `TOC_PREFIX_STATS` (PG18+ only, per `--statistics`'s own
availability).
**Relied on by:** `architecture.md` ("TOC enrichment") — `map::parse_toc_header_line` splits the
line on these exact literal separators in this exact order.
**Re-verify:**

```sh
grep -n 'ahprintf(AH, "-- %sName: %s; Type: %s; Schema: %s; Owner: %s"' src/bin/pg_dump/pg_backup_archiver.c
grep -n 'Tablespace: %s' src/bin/pg_dump/pg_backup_archiver.c
grep -n "if (\*s == '\\\\n' || \*s == '\\\\r')" src/bin/pg_dump/dumputils.c
```

Confirm the field order and separators are unchanged, and that
`sanitize_line`'s `NULL`-only hyphen substitution still holds.

---

## I19 — The role/tablespace statement shapes `dumputils.c`/`_selectTablespace()` emit are fixed and never appear inside a TOC entry's own header

**Claim.** Four statement shapes, each a single physical line ending in `;`,
carry every role/tablespace reference a TOC header's `Owner:`/`Tablespace:`
fields (I16/I18) don't already cover:

- `GRANT <privs> ON <type> [<name>] TO <grantee>[ WITH GRANT OPTION];` —
  `buildACLCommands()`'s `"%sGRANT %s ON %s "` + literal `"TO "` + either the
  literal, unquoted, uppercase pseudo-role `PUBLIC` or `fmtId(grantee)`.
- `REVOKE <privs> ON <type> [<name>] FROM <grantee>;` — the same function's
  `"%sREVOKE %s ON %s "` + literal `"FROM "` + the same two grantee shapes.
- `ALTER DEFAULT PRIVILEGES FOR ROLE <owner> [IN SCHEMA <nspname>] ` —
  `buildDefaultACLCommands()`'s literal prefix, `fmtId(owner)`, concatenated
  directly onto one of the two shapes above to form a single statement (one
  line, one `;`) rather than two.
- `SET default_tablespace = <value>;` — `_selectTablespace()`'s
  `"SET default_tablespace = %s"`, where `<value>` is `fmtId(tablespace)` for
  a real tablespace or the literal quoted empty string `''` when reverting to
  the database's own default (`want == ""`).

None of these four is itself a TOC entry (`_printTocEntry()` never assigns
one of `_printTocEntry`'s own `ArchiveEntry.description` values to a
`GRANT`/`REVOKE`/`SET` statement — a `GRANT`/`REVOKE` line is instead a `TOC`
entry's own `defn` under kind `ACL`/`DEFAULT ACL`, and `SET default_tablespace`
is framing pg_dump writes ahead of a definition, not a `defn` of its own), so
none is reachable through I16/I18's `Owner:`/`Tablespace:` fields — a reader
needs the statement text itself.

**Proof.** `dumputils.c`'s `buildACLCommands()` (`"%sGRANT %s ON %s "`/
`"%sREVOKE %s ON %s "`/`"TO "`/`"FROM "`/`"PUBLIC;\n"`) and
`buildDefaultACLCommands()` (`"ALTER DEFAULT PRIVILEGES FOR ROLE %s "`);
`pg_backup_archiver.c`'s `_selectTablespace()` (`"SET default_tablespace = %s"`
/ the `want == ""` branch's literal `''`). Confirmed byte-identical across
v13.23 through v18.6 (the `ALTER ... OWNER TO` emission point moved from
`pg_backup_archiver.c` between v13 and v18 — a helper-buffer refactor — but
the text it produces is unchanged; see I16). Observed in
`fixtures/16/objects/default.sql`: `GRANT SELECT ON TABLE objects.events TO
fixture_reader;`, `GRANT SELECT ON TABLE objects.widgets TO PUBLIC;`, `ALTER
DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA objects GRANT SELECT ON TABLES
TO fixture_reader;`, `SET default_tablespace = '';`.

**Scope limit.** None — closed by the `objects` fixtures. `REVOKE` is exercised by
`objects.no_public_execute()`: revoking a function's default PUBLIC `EXECUTE`
privilege is the one ACL shape whose target state has *fewer* privileges than
the default, so `pg_dump`'s ACL diff emits a solo `REVOKE ALL ON FUNCTION
objects.no_public_execute() FROM PUBLIC;` with no offsetting `GRANT` —
confirmed across all 6 routine versions. (Revoking a privilege from an
*owner* instead pairs the `REVOKE` with a `GRANT` restoring the owner's
remaining implicit privileges — still both of I19's shapes, but not a solo
`REVOKE`.) A non-empty `SET default_tablespace` value is exercised by
`objects.tablespaced_table` (`SET default_tablespace = fixture_ts;` ahead of
its definition, `SET default_tablespace = '';` after) — same fixture I18's
`Tablespace:` entry now cites.

**Verified against:** v13.23 through v18.6 source; `fixtures/16/objects/default.sql`
for the `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES`/`SET default_tablespace`
(both a real tablespace and the reset) shapes.

**Relied on by:** `architecture.md` ("Cross-references (roles and
tablespaces)") — `preamble::extract_statement_cross_refs` matches these four
shapes by marker substring rather than a full grammar.

**Re-verify:**

```sh
grep -n '"%sGRANT %s ON %s \|"%sREVOKE %s ON %s \|appendPQExpBufferStr(thissql, "TO \|appendPQExpBufferStr(firstsql, "FROM ' src/bin/pg_dump/dumputils.c
grep -n '"ALTER DEFAULT PRIVILEGES FOR ROLE %s "' src/bin/pg_dump/dumputils.c
grep -n '"SET default_tablespace = %s"\|default_tablespace = ..' src/bin/pg_dump/pg_backup_archiver.c
```

Confirm the four literal shapes and their separators are unchanged.

---

## I20 — The three nested literal forms use two different escape conventions, not one

**Claim.** Inside a `COPY` field, the container literals `pg_dump` can emit for
a non-scalar value quote and escape their parts by three closely related but
**non-identical** rules. Specifically:

| Form | Wrapper | Separator | Quote when a part… | Inside a quoted part |
|---|---|---|---|---|
| array (`array_out`) | `{`…`}` | the element type's `typdelim`, `,` for every type except `box` | is empty, is `NULL` case-insensitively, or contains `"` `\` `{` `}` the delimiter, or whitespace | `"` → `\"`, `\` → `\\` (**backslash**) |
| composite (`record_out`) | `(`…`)` | `,` | is empty, or contains `"` `\` `(` `)` `,` or whitespace | `"` → `""`, `\` → `\\` (**doubling**) |
| range bound (`range_bound_escape`) | `[`/`(` … `]`/`)` | `,` | is empty, or contains `"` `\` `(` `)` `[` `]` `,` or whitespace | `"` → `""`, `\` → `\\` (**doubling**) |

Three further asymmetries follow from the same functions:

- **An array distinguishes a NULL element from the string `NULL` by quoting
  alone** — a SQL NULL element is emitted as bare `NULL`, and a element whose
  text is `NULL` is force-quoted to `"NULL"`. A composite instead spells a NULL
  field as *nothing at all* between its separators, and force-quotes an empty
  string to `""`. A range bound cannot be NULL; an absent bound is likewise
  nothing at all, and `""` is an empty-string bound.
- **`array_out` prefixes the literal with `[lb:ub]`… `=` for every dimension
  whenever any dimension's lower bound is not 1**, and returns the bare `{}`
  for any array with zero elements regardless of its dimensionality.
- **A multirange (`multirange_out`) does not quote or escape its member ranges
  at all** — it concatenates each `range_out` result inside `{`…`}`, separated
  by `,`. The member's own bracket characters are what make it re-parseable.

All of this sits *inside* the COPY TEXT field escaping of I15, which is undone
first: `copy::decode_field` returns the literal exactly as the `*_out` function
produced it.

**Proof.** `array_out()` in `src/backend/utils/adt/arrayfuncs.c` — the
`needquote` computation (empty string, `pg_strcasecmp(values[i], "NULL")`, then
the per-character scan for `"`/`\`/`{`/`}`/`typdelim`/`array_isspace`), the
`needdims` loop over `AARR_LBOUND`, and the emit loop that writes `*p++ = '\\'`
before a `"` or `\`. `record_out()` in `rowtypes.c` — the `nq` scan over
`"`/`\`/`(`/`)`/`,`/`isspace`, and its emit loop, which appends the character
*itself* before re-appending it. `range_bound_escape()` in `rangetypes.c` — the
same doubling emit loop, over a scan that adds `[` and `]`.
`multirange_out()` in `multirangetypes.c` appends `OutputFunctionCall` results
with no escaping step between them.

**Scope limit.** These are the *output* functions, so the invariant is about
what a dump contains, not about what the corresponding `*_in` functions accept
(they are considerably more permissive — `array_in` accepts unquoted whitespace
padding, for instance). A reader must handle what is emitted; a *renderer* that
claims to reproduce a dump byte-for-byte must reproduce the `needquote`
predicates exactly, including the `NULL` case-fold and the whitespace test.

**Verified against:** v13.23, v16.15, v18.6 — the `needquote`/`nq` predicates
and both emit loops are character-for-character identical across all three.

**Relied on by:** `roadmap-phase4-composite-decoding.md` — the nested decoder's
parameterization, and the exactness requirement on render-back.

**Re-verify:**

```sh
grep -n 'force quotes for literal NULL' -B12 -A30 src/backend/utils/adt/arrayfuncs.c
grep -n 'sprintf(ptr, "\[%d:%d\]"' -B12 src/backend/utils/adt/arrayfuncs.c
grep -n 'Detect whether we need double quotes' -A30 src/backend/utils/adt/rowtypes.c
grep -n 'range_bound_escape' -A35 src/backend/utils/adt/rangetypes.c
grep -n 'multirange_out' -A30 src/backend/utils/adt/multirangetypes.c
```

Confirm array still backslash-escapes while composite and range bound still
double, and that the quote-forcing character sets are unchanged.

---

## I21 — An array's dimensionality and lower bounds are per *value*, and appear nowhere in the DDL

**Claim.** `pg_dump` writes an array column's declared type with exactly one
trailing `[]` however it was declared — `integer[][]` and `integer[3]` both
come back as `integer[]` — because PostgreSQL's type system does not record
dimensionality. The number of dimensions and the lower bound of each are
properties of the individual stored value, and **two rows of the same column
may legitimately disagree about both**.

**Proof.** `doc/src/sgml/array.sgml:62` — "The current implementation does not
enforce the declared number of dimensions either." `pg_dump` writes a column's
type with `format_type(atttypid, atttypmod)`, which reconstructs an array as
the element type plus a single `[]`: columns declared `int[][]`, `int[3]` and
`int[]` all print `integer[]`, and the `attndims` that did record the
declaration (2, 1, 1 respectively) is discarded. Observed directly: a single
`int[]` column accepts `{1,2}`, `{{1,2},{3,4}}` and `[0:2]={7,8,9}` in three
consecutive rows, and `COPY … TO STDOUT` emits each one back in the shape it
was stored (see the re-verify script below).

**Scope limit.** A domain over an array (`CREATE DOMAIN d AS int[]`) can carry a
`CHECK` constraint that pins dimensionality, but the constraint text is not
something type resolution interprets — the invariant holds for anything this
project reads out of the declared type.

**Verified against:** v16.15 (behaviour, live server); the `format_type`
reconstruction is unchanged v13.23 through v18.6.

**Relied on by:** `architecture.md` ("Type resolution", the array paragraph) and
`roadmap-phase4-composite-decoding.md` — it is the reason an array column's
Arrow type cannot be settled from the DDL alone.

**Re-verify:**

```sh
grep -n 'does not enforce the declared' -A3 doc/src/sgml/array.sgml
psql -X -q <<'SQL'
create temp table i21 (v int[][]);
insert into i21 values ('{1,2}'), ('{{1,2},{3,4}}'), ('[0:2]={7,8,9}');
select format_type(atttypid, atttypmod), attndims from pg_attribute
  where attrelid = 'i21'::regclass and attname = 'v';
copy i21 to stdout;
SQL
```

The declared type must print as `integer[]` (with `attndims` 2, showing the
declaration was recorded and then discarded) and the three values must come
back in three different shapes.

---

## I22 — A domain inherits its base type's array delimiter, and `box` is the only built-in whose delimiter is not `,`

**Claim.** Two halves, and the decision below needs both:

1. **`box` is the only entry in `pg_type.dat` with a non-default `typdelim`.**
   Every other built-in separates array elements with `,`.
2. **`CREATE DOMAIN` copies `typdelim` from its base type**, so a domain over
   `box` has delimiter `;`, and so does a domain over a domain over `box`. The
   DDL `pg_dump` writes for that domain records nothing about it — `CREATE
   DOMAIN` has no `DELIMITER` clause — so **the base type's name is the only
   trace of the delimiter in the file.**

**Proof.** `src/include/catalog/pg_type.dat` contains exactly one `typdelim =>
';'` line, on `box`, identical in v13.23 through master.
`src/backend/commands/typecmds.c`, in the domain-definition path: `/* Array
element Delimiter */ delimiter = baseType->typdelim;` — alongside the same
copy-from-base treatment given to alignment, storage, category and the output
function. A user-defined base type may also set one (`CREATE TYPE … DELIMITER =
';'`), and there `pg_dump` *does* emit the clause — but such a type resolves as
`TypeKind::Base` and is refused on its own account, so the clause never has to
be parsed.

**Scope limit.** Composites are unaffected: `record_out`
(`src/backend/utils/adt/rowtypes.c`) writes `appendStringInfoChar(&buf, ',')`
unconditionally, with no reference to any field type's `typdelim`. Ranges and
multiranges likewise separate with a literal `,`. **Arrays are the entire
exposure.**

**Verified against:** v13.23, v18.6 and master (`pg_type.dat`); v18.6
(`typecmds.c`, `rowtypes.c`).

**Relied on by:** `roadmap-phase4-composite-decoding.md`, "Render-back must be
exact" — it is why the opaque-element refusal tests the element type *after*
domain unwrapping rather than the declared string, and why the array separator
can stay hardcoded to `,` once it does.

**Re-verify:**

```sh
grep -rn "typdelim => ';'" src/include/catalog/pg_type.dat
grep -n 'Array element Delimiter' -A1 src/backend/commands/typecmds.c
grep -n "appendStringInfoChar(&buf, ',')" src/backend/utils/adt/rowtypes.c
psql -X -q <<'SQL'
create domain dbox as box;
create temp table i22 (v dbox[]);
insert into i22 values (array['(1,2),(3,4)'::dbox, '(5,6),(7,8)'::dbox]);
copy i22 to stdout;
drop domain dbox cascade;
SQL
```

The first grep must return exactly one line (`box`). The `COPY` output must
separate the two elements with `;`, not `,`.

---

## I23 — A zero-field composite is legal, and `()` is what both it and a one-field NULL are written as

**Claim.** `CREATE TYPE x AS ();` is accepted, `pg_dump` re-emits it with an
empty body (`CREATE TYPE public.x AS (\n);`), and `record_out` writes its
value as `()`. **That literal is not unique to it**: a composite with one
field holding SQL NULL is written `()` as well, and so is a composite whose
every field has been dropped. So the literal alone cannot say how many fields
a record has, and **the declared field list is the only thing that can** —
arity has to be joined in from the type definition, never inferred from the
text.

**Proof.** `record_out()` in `src/backend/utils/adt/rowtypes.c` writes `(`,
loops over the tuple descriptor's columns, and writes `)`. A zero-column
descriptor runs the loop zero times. A NULL column takes the `if (nulls[i]) {
/* emit nothing... */ continue; }` arm, and `needComma` is still false for the
first column, so a one-column NULL record emits nothing between the
parentheses either. A dropped column is `continue`d over before `needComma` is
set. `dumpCompositeType()` in `src/bin/pg_dump/pg_dump.c` writes `CREATE TYPE
%s AS (`, appends one `\n\t<name> <type>` per non-dropped attribute, and
closes with `appendPQExpBufferStr(q, "\n);\n")` — no attribute writes nothing
between them.

**Scope limit.** This is about the *output* form only. It says nothing about
whether a zero-field composite is useful, and note that under
`--binary-upgrade` a dropped attribute is *not* skipped: it becomes an
`INTEGER /* dummy */ ` placeholder (I5's shape), so a type whose fields were
all dropped has a non-empty body in that dump and an empty one in an ordinary
dump.

**Verified against:** v13.23, v16.15, v18.6, master (`rowtypes.c` byte-identical
in the relevant loop; `pg_dump.c` identical modulo line numbers), plus
`fixtures/{13..18}/types/default.sql`'s `public.empty_comp` /
`t_composite.v_empty_comp`, whose DDL and `()` values are identical on all six.

**Relied on by:** `roadmap-phase4-composite-decoding.md`, "A zero-field
composite maps", and `architecture.md`, "Type resolution" (the all-or-nothing
field list) — "no fields parsed" and "no fields declared" have to stay
distinguishable in the type definition when the literal cannot tell them apart,
which is why `TypeKind::Composite::fields` is an `Option`.

**Re-verify:**

```sh
grep -n 'emit nothing' -B8 src/backend/utils/adt/rowtypes.c
grep -n 'CREATE TYPE %s AS (' -A50 src/bin/pg_dump/pg_dump.c
psql -X -q <<'SQL'
create type i23 as ();
create temp table i23t (a i23, b i23);
insert into i23t values ('()', null);
copy i23t to stdout;
drop table i23t; drop type i23;
SQL
```

The `COPY` output must be `()` followed by `\N`.

---

## I24 — A dump's type graph is acyclic: no type can contain itself, through any nesting

**Claim.** Following a declared type through composite fields, domain base
types, range subtypes and array element types always terminates. PostgreSQL
refuses to create a composite type that contains itself by any of those
routes, directly (`i24` with a field of type `i24[]`) or mutually (`a` with a
field of `b`, then `b` given a field of `a`), so no dump can hold one.

**Proof.** `CheckAttributeType()` in `src/backend/catalog/heap.c` carries a
`containing_rowtypes` list and errors — `"composite type %s cannot be made a
member of itself"` — when it re-enters a rowtype already on it. Its recursion
covers every route this project's resolution walks: `TYPTYPE_DOMAIN` recurses
on `getBaseType`, `TYPTYPE_COMPOSITE` on each attribute, `TYPTYPE_RANGE` on
`get_range_subtype`, `TYPTYPE_MULTIRANGE` on `get_multirange_range`, and a
final `get_element_type` arm recurses into an array's element type "in case
they are composite". Domains have their own separate guarantee, already relied
on: a domain's base type must exist before the domain does.

**Scope limit.** This is a property of what PostgreSQL will *create*, so it
holds for every real dump and says nothing about a hand-edited file. It is
also silent about `RECORD`/`ANYARRAY` pseudo-types, which the same function
exempts and which never appear as a column's declared type in `pg_dump`
output.

**Verified against:** v13.23 and v18.6 (`heap.c`, same check and message),
plus both rejections observed live on the local PostgreSQL 16 instance.

**Relied on by:** `architecture.md`, "Type resolution" — it is why
`resolve_declared_type` recurses through composites, ranges and array elements
with no cycle guard and no depth limit, exactly as it already did through
domains.

**Re-verify:**

```sh
grep -n 'cannot be made a member of itself' -A6 src/backend/catalog/heap.c
grep -n 'Must recurse into array types' -A6 src/backend/catalog/heap.c
psql -X -q <<'SQL'
begin;
create type i24 as (x integer);
alter type i24 add attribute y i24[];
rollback;
SQL
```

The `ALTER TYPE` must fail with `composite type i24 cannot be made a member of
itself`; a version that accepted it would make unbounded recursion reachable.

---

## I25 — An `array_out` literal's dimensionality is exactly its leading `{` run

**Claim.** For any value `array_out` writes, the number of `{` characters at
the start of the literal — after an optional `[lb:ub]…=` prefix — is exactly
the array's dimension count, and **no element can extend that run**. Three
sub-claims, all from the same function:

1. The output opens with exactly `ndim` braces.
2. Any element whose text contains `{` is force-quoted, so an unquoted element
   can never begin with one.
3. An empty array is written `{}` whatever its dimensionality, so that literal
   carries no dimension count at all — and it is emitted before the
   lower-bound prefix is ever formatted, so `{}` never carries one.

A fourth, from the header: `MAXDIM` is **6**, so a run longer than that did not
come from `array_out`.

**Proof.** `src/backend/utils/adt/arrayfuncs.c`, `array_out()`. The output
loop is `APPENDCHAR('{')` once, then `for (i = j; i < ndim - 1; i++)
APPENDCHAR('{')` with `j == 0` on the first iteration — `1 + (ndim - 1)`
braces. The quoting test is `else if (ch == '{' || ch == '}' || ch == typdelim
|| array_isspace(ch)) needquote = true`, applied to every character of every
element. The empty case is `if (nitems == 0) { retval = pstrdup("{}");
PG_RETURN_CSTRING(retval); }`, which returns before the `needdims` block that
formats `[lb:ub]`. `MAXDIM` is `#define MAXDIM 6` in
`src/include/utils/array.h` on v14+ and in `src/include/c.h` on v13 — the
same value, a moved `#define`, which is why the re-verification below greps
both.

**Scope limit.** This is a property of `array_out`'s *output*, which is what a
dump contains. Array *input* syntax is far more permissive — whitespace
between braces, unquoted elements containing braces are rejected rather than
accepted, and so on — so nothing here licenses reading a hand-written literal.
A file edited by hand can therefore carry a brace run this rule misreads;
`MAXDIM` bounds how far a consumer should trust it.

**Verified against:** v13.23 through v18.6 — the brace loop, the quoting test
and the `{}` early return are byte-identical across all six; only
`array_isspace` was renamed to `scanner_isspace` (v18), which does not change
the character set. All four output shapes observed live on the local
PostgreSQL 16 instance.

**Relied on by:** `architecture.md`, "The array shape census" — it is why
`crate::index::ArrayShape::observe` can read a value's dimensionality off the
raw, still-COPY-escaped field with no array parser at all, and why `{}`
constrains neither bound.

**Re-verify:**

```sh
grep -n "for (i = j; i < ndim - 1; i++)" -A1 src/backend/utils/adt/arrayfuncs.c
grep -n "ch == '{' || ch == '}' || ch == typdelim" -A1 src/backend/utils/adt/arrayfuncs.c
grep -n 'retval = pstrdup("{}")' -B2 src/backend/utils/adt/arrayfuncs.c
grep -rn '#define MAXDIM' src/include/utils/array.h src/include/c.h
psql -X -q <<'SQL'
copy (select '{{1,2},{3,4}}'::int[], '{"c{d}"}'::text[],
             '{}'::int[], '[0:1]={7,8}'::int[]) to stdout;
SQL
```

The four values must come back as `{{1,2},{3,4}}` (run 2), `{"c{d}"}` (run 1 —
the brace inside the element is quoted, not structural), `{}` and
`[0:1]={7,8}` (run 1, after the prefix).

---

## I26 — An array of a domain over an array is legal, and its literal is one brace deep

**Claim.** PostgreSQL accepts `CREATE DOMAIN d AS T[]` followed by a column of
type `d[]`, and `pg_dump` writes that column's declared type as `d[]` with the
domain declared separately as `CREATE DOMAIN d AS T[]`. The **value** of such a
column is a one-dimensional `array_out` literal whose elements are themselves
array literals, force-quoted and backslash-escaped one layer — `{"{1,2}","{3}"}`
— **not** a two-dimensional literal.

So its *leading brace run is 1* while the type it resolves to is two `List`
levels deep. Literal depth and resolved Arrow depth are independent here, which
is what makes this the one DDL shape that reaches a nested
`NestedPlan::Array(Array(…))` in a dump `pg_dump` actually wrote. (`integer[][]`
does not: PostgreSQL collapses it to `integer[]` in the catalog and `pg_dump`
writes `integer[]`, per I21.)

**Proof.** Observed live on PostgreSQL 16 (`pg_dump` 16.x), end to end:

```
CREATE DOMAIN public.intarr AS integer[];
CREATE TABLE public.t_nested_array (id integer, x public.intarr[]);

COPY public.t_nested_array (id, x) FROM stdin;
1	{"{1,2}","{3}"}
\.
```

The one-brace-deep shape is not an accident of this example: it follows from
I25, whose force-quote test quotes any element whose text contains `{`, so an
element that is itself an array literal is always quoted and can never extend
the outer run.

**Scope limit.** The chain may be longer (a domain over a domain over an
array), and the same shape arises for an array whose element type is a
composite *containing* an array — but there the outer literal is a record, not
an array, so it reaches a different code path. This entry is about the
array-of-array-typed-element case specifically.

**Verified against:** PostgreSQL 13.23, 14.24, 15.19, 16.15, 17.11 and 18.6,
with real `pg_dump` output — `fixtures/*/types/default.sql`'s
`public.t_nested_array.v_nested_array`, byte-identical on all six. The
`array_out` half is I25, verified v13.23–v18.6 from source.

**Relied on by:** `architecture.md`, "Type resolution" — this entry is the
whole reason the shape cannot be typed as nested `List`s: the literal's depth
and the column's resolved depth are independent, so one `NestedPlan::Array`
chain would have to mean two different things. The column therefore resolves
to `Utf8View` with `ColumnResolution::NestedArrayElement`, which in turn is
what lets `resolve::retype_from_census` assume every plan it sees is
`Array(non-array)`.

**Re-verify:**

```sh
psql -X -q -d scratch <<'SQL'
CREATE DOMAIN intarr AS int[];
CREATE TABLE t_nested_array (id int, x intarr[]);
INSERT INTO t_nested_array VALUES (1, ARRAY['{1,2}'::intarr,'{3}'::intarr]);
SQL
pg_dump -d scratch | grep -A2 -e 'CREATE DOMAIN' -e '^COPY public.t_nested_array'
```

Equivalently, on all six routine majors at once: `cd scripts && uv run
generate_fixtures.py --schema types`, then diff `public.t_nested_array`'s
`COPY` block, which carries this exact value in every one.

---

## I27 — A label-less enum is written `AS ENUM (\n);`, the same body `--binary-upgrade` writes for *every* enum

**Claim.** `CREATE TYPE x AS ENUM ()` is legal SQL, and a plain `pg_dump`
writes it as `CREATE TYPE x AS ENUM (\n);` — an empty body, byte-identical to
the body I6 says `--binary-upgrade` writes for an enum that *does* have
labels. So an empty body means "this enum has no labels" only in a dump that
is not `--binary-upgrade`; in one that is, the labels follow as `ALTER TYPE
... ADD VALUE` and the empty body means nothing at all.

**Proof.** `dumpEnumType()` in `pg_dump.c` opens the body unconditionally and
gates the label loop on `if (!dopt->binary_upgrade)`; a type with no labels
contributes no loop iterations, which is the same output the gate produces.
There is no marker distinguishing the two cases in the `CREATE TYPE` statement
itself — only the presence or absence of the `ALTER TYPE` lines that follow,
and the `-- For binary upgrade` comments around them.

**Scope limit.** About the enum body only. Everything about where the labels
go under `--binary-upgrade` is I6.

**Verified against:** PostgreSQL 13.23, 14.24, 15.19, 16.15, 17.11 and 18.6,
with real `pg_dump` output — `scripts/fixture_schema_types.sql`'s
`public.empty_enum` in `fixtures/*/types/default.sql` *and*
`fixtures/*/types/binary-upgrade.sql`, beside `public.mood` in the same files,
which is the pair that makes the ambiguity visible.

**Relied on by:** `architecture.md` ("Type resolution", enums) — an enum whose
label set is empty resolves to `ColumnResolution::EmptyEnum` rather than a
`Dictionary`, and that conclusion is only sound because the label folding I6
describes runs first. Reading the body alone would report every
`--binary-upgrade` enum as empty.

**Re-verify:**

```sh
psql -X -q -d scratch -c 'CREATE TYPE empty_enum AS ENUM ()' \
  -c "CREATE TYPE mood AS ENUM ('sad','ok')"
pg_dump -d scratch                    | grep -A3 'AS ENUM'
pg_dump -d scratch --binary-upgrade   | grep -A3 'AS ENUM'
```

---

## I28 — PostgreSQL accepts six spellings for an array-typed column, and all six are the same type

**Claim.** A column, composite field, or domain base type may be declared as an
array in six ways, and every one produces the identical `pg_type` entry —
`integer[]`, `integer[3]`, `integer[][]`, `integer[3][4]`, `integer ARRAY`, and
`integer ARRAY[4]` all yield `_int4`. Neither the dimension count nor the
bounds survive into the type; `attndims` records what was written and is
discarded by `format_type`, so `pg_dump` writes all six back as `integer[]`
(I21).

The consequence a reader must not miss: **the spelling constrains nothing about
the values.** A column declared `integer[3]` may hold a four-element array, and
one declared `integer[]` may hold `{{1,2},{3,4}}`. Only the census over actual
values says what a column's shape is.

**Proof.** `src/backend/parser/gram.y`, the `Typename` production:

```
Typename:	SimpleTypename opt_array_bounds
		|	SETOF SimpleTypename opt_array_bounds
		|	SimpleTypename ARRAY '[' Iconst ']'
		|	SETOF SimpleTypename ARRAY '[' Iconst ']'
		|	SimpleTypename ARRAY
		|	SETOF SimpleTypename ARRAY

opt_array_bounds:
			opt_array_bounds '[' ']'
		|	opt_array_bounds '[' Iconst ']'
		|	/*EMPTY*/
```

`opt_array_bounds` is left-recursive, so any number of `[]`/`[n]` pairs is
accepted; every alternative sets only `arrayBounds`, which `transformTypeName`
turns into "one array type over the element", discarding the count. The
`ARRAY`/`ARRAY[n]` alternatives are the SQL-standard spelling and are
documented for user use (`doc/src/sgml/array.sgml`, "the keyword `ARRAY`, can
be used for one-dimensional arrays").

Observed live on PostgreSQL 16.15:

```
 attname | format_type | attndims
---------+-------------+----------
 a       | integer[]   |        1     -- int[]
 b       | integer[]   |        1     -- int[3]
 c       | integer[]   |        2     -- int[][]
 d       | integer[]   |        2     -- int[3][4]
 e       | integer[]   |        1     -- int ARRAY
 f       | integer[]   |        1     -- int ARRAY[4]
```

Two details a normalizer needs. **The bracket run is unbounded in the DDL even
though `MAXDIM` is 6**: `int[][][][][][][][][]` is accepted and yields
`integer[]` with `attndims` 9, so a declaration's bracket count needs no cap
and carries no meaning. And **`ARRAY` is a keyword, so it is case-insensitive**
— `INTEGER ARRAY` and `Integer Array[4]` are the same declaration. Both
observed on 16.15 alongside the table above.

**Scope limit.** This is about the *input* grammar, so it bears only on SQL
`pg_dump` did not write — a hand-written or hand-edited file, or another
producer's output (`roadmap.md`, "The input contract is valid PostgreSQL").
`pg_dump`'s own output only ever contains the first spelling, which is I21.

**Verified against:** the grammar productions are identical in v13.23, v16.15
and v18.6; the six-column observation is v16.15, live. The collapse is also in
the repo's checked-in bytes on all six routine majors:
`fixtures/*/types/default.sql`'s `public.t_array_spelling` is declared
`integer[3]`, `integer[3][4]`, `integer ARRAY` and `integer ARRAY[4]` and dumps
as four `integer[]` columns. The rejections above — `integer ARRAY[4][5]`,
`integer ARRAY[]`, `integer[] ARRAY`, `integer ARRAY ARRAY`, `integer[abc]`,
`integer[-1]`, `integerARRAY`, a bare `ARRAY` — are v16.15, live, and are what
`a_declaration_postgresql_would_reject_is_not_read_as_an_array` pins.

**Relied on by:** `architecture.md` ("Type resolution") — it is why resolution
normalizes an array declaration to its element type plus one level rather than
reading the spelling literally, and why no spelling is allowed to imply a
nested array. `pgtype.rs`'s
`every_array_declaration_spelling_is_one_array_of_the_element_type` is the
unit-test stand-in for the fixture five of the six spellings can never have
(`roadmap.md`, "Where a fixture is impossible").

**Re-verify:**

```sh
grep -n 'opt_array_bounds:' -A 8 src/backend/parser/gram.y
grep -n 'SimpleTypename ARRAY$' -A 4 src/backend/parser/gram.y
psql -X -q <<'SQL'
create temp table i28 (a int[], b int[3], c int[][], d int[3][4], e int ARRAY, f int ARRAY[4]);
select attname, format_type(atttypid, atttypmod), attndims from pg_attribute
  where attrelid = 'i28'::regclass and attnum > 0 order by attnum;
SQL
```

All six rows must print `integer[]`, with `attndims` varying — that difference
is the record of the declaration, and the point is that nothing downstream
reads it.

The rejections, each of which must be a syntax error (`integerARRAY` an
unknown type):

```sh
for d in 'int ARRAY[4][5]' 'int ARRAY[]' 'int[] ARRAY' 'int ARRAY ARRAY' \
         'int[abc]' 'int[-1]' 'integerARRAY' 'ARRAY'; do
  psql -X -q -c "create temp table chk (a $d);"
done
```

And on all six routine majors at once: `cd scripts && uv run
generate_fixtures.py --schema types`, then check `public.t_array_spelling`'s
`CREATE TABLE` — all four columns must read `integer[]`.

## I29 — A type name needing quotes is legal, and `pg_dump` writes it back quoted

**Claim.** A type or domain name may contain any character, including the
array-declaration metacharacters — `[`, `]`, a space, and the `ARRAY` keyword.
Such a name must be double-quoted wherever it is written, and `pg_dump` quotes
it on the way out (`fmtId()`), in the `CREATE TYPE`/`CREATE DOMAIN` statement
and in every column declaration that uses it. An array *of* such a type is the
quoted name followed by an unquoted `[]`.

The consequence a reader must not miss: **the quotes are what keep the
array-bounds production unambiguous.** `s."x ARRAY"` is a scalar column of a
domain, and `s."x ARRAY"[]` is an array of it; the two are told apart only by
whether the trailing `ARRAY`/`[]` falls inside the quotes. A normalizer that
strips a trailing bound or keyword without checking for a closing quote first
would read the former as an array of `s."x`.

**Proof.** Observed live on PostgreSQL 16.15. All four `CREATE`s are accepted,
and this is `pg_dump 16.14`'s own output for the table that uses them:

```sql
CREATE DOMAIN s."d[3]" AS integer;
CREATE DOMAIN s."my type" AS integer;
CREATE TYPE s."weird[]" AS ENUM ('a', 'b');
CREATE DOMAIN s."x ARRAY" AS integer;

CREATE TABLE s.t (
    a s."weird[]",
    b s."my type",
    c s."x ARRAY",
    d s."d[3]",
    e s."weird[]"[],
    f s."my type"[],
    g s."x ARRAY"[]
);
```

**Scope limit.** Nothing here is reachable from a dump of a database whose type
names are all ordinary identifiers, which is every fixture and the koji sample.
It bears on the input contract (`roadmap.md`, "The input contract is valid
PostgreSQL"), and on what this build does with such a file — see `STATUS.md`,
"Known gaps".

**Verified against:** v16.15, live, `pg_dump 16.14`. Not re-checked on other
majors: `fmtId()` and the quoting rule are not version-varying, and the
`Typename` grammar this interacts with is identical across v13-v18 (I28).

**Relied on by:** `pgtype.rs`'s `array_element` — its bound- and
keyword-stripping helpers bail on a trailing `"`, which is what makes the
quoted spellings safe rather than merely untested. `STATUS.md`'s "Known gaps"
entry for quoted type names rests on the second half of the claim: because
`pg_dump` quotes the name in the *declaration* while `parse_ident` dequotes it
in `TypeDef.name`, the two never compare equal and the column degrades to
`Unknown` instead of being misread.

**Re-verify:**

```sh
psql -X -q -c 'CREATE SCHEMA s' \
  -c 'CREATE DOMAIN s."x ARRAY" AS integer' \
  -c 'CREATE TABLE s.t (c s."x ARRAY", g s."x ARRAY"[])'
pg_dump --schema=s | grep -A 3 'CREATE TABLE s.t'
```

The two columns must print as `c s."x ARRAY",` and `g s."x ARRAY"[]` — the
keyword inside the quotes, the array marker outside.

---

## I30 — `pg_dumpall` writes `template1` first, then every database in `datname` order

**Claim.** The order of the `\connect`-delimited segments in `pg_dumpall`
output is fully determined by database *name*: `template1` first, then the
remaining connectable databases sorted by `datname`. It is not creation order,
OID order, or anything a fixture would have to observe empirically.

**Proof.** `dumpDatabases()` in `pg_dumpall.c` drives the loop from one query:

```sql
SELECT datname FROM pg_database d
WHERE datallowconn AND datconnlimit != -2
ORDER BY (datname <> 'template1'), datname
```

The `ORDER BY` is the whole guarantee — the boolean sorts `false` (i.e.
`template1`) first, then `datname` breaks the rest. The comment above it says
why `template1` leads: the restore script must not be connected to a database
it is about to drop.

**Scope limit.** `datallowconn AND datconnlimit != -2` excludes non-connectable
databases and (v15+) template-marked ones, so a database can be absent
entirely; that is orthogonal to the ordering of those present. Says nothing
about `pg_dump --create`, which emits one database and no ordering question.

**Verified against:** v13.23, v18.6, master — the query is byte-identical in
all three.
**Relied on by:** `architecture.md` ("Fixtures"). The `edge_cases/dumpall`
fixture's database sequence is chosen by naming, not observed: a second
data-carrying database named `pgdq_tenant` lands between `pgdq_fixture` and
`postgres`, which is what makes "cancel inside the *second* database's data" a
deterministic file offset for the recurring-metadata-boundary test
(`architecture.md`, "`parse` resumes, and saves as it goes"). If the ordering
changed, that test would cancel in the wrong segment and still pass.
**Re-verify:**
```sh
grep -n "datname <> 'template1'" -B 6 src/bin/pg_dump/pg_dumpall.c
```

## I31 — `--disable-triggers` writes statements between a data entry's TOC comment and its data

**Claim.** With `--disable-triggers`, `pg_dump` emits `ALTER TABLE <table>
DISABLE TRIGGER ALL;` **after** each `TABLE DATA` entry's `-- Data for Name:`
comment block and **before** the `COPY` header or first `INSERT INTO` line,
plus a `SET SESSION AUTHORIZATION DEFAULT;` ahead of the first such entry. The
comment block is therefore no longer adjacent to the data it heads — the one
adjacency `map::Builder::on_copy_start` and `Mode::Comment`'s `INSERT`-run arm
both depend on.

**Proof.** `restore_toc_entry()` in `pg_backup_archiver.c` calls
`_printTocEntry(AH, te, true)` — which writes the comment block — and only then
`_disableTriggersIfNecessary(AH, te)`, which writes
`_becomeUser(AH, ropt->superuser)`'s `SET SESSION AUTHORIZATION` line followed
by `ahprintf(AH, "ALTER TABLE %s DISABLE TRIGGER ALL;\n\n", …)`. The matching
`ENABLE TRIGGER ALL;` is written after the data by `_enableTriggersIfNecessary`.

**Scope limit.** Opt-in and gated on a data-only restore: the guard is
`if (!ropt->dataOnly || !ropt->disable_triggers) return;` on v13–v17 and
`if (ropt->dumpSchema || !ropt->disable_triggers) return;` on v18/master, so
`--disable-triggers` alone emits nothing — `--data-only` or `--section=data`
must accompany it. Says nothing about any other flag: no other `pg_dump` option
is known to interpose a statement at this position.

**Verified against:** source in all of v13.23, v14.24, v15.19, v16.15, v17.11,
v18.6 and master (identical call order); output observed against 16.15, both
`--data-only --disable-triggers` and `--section=data --disable-triggers`, in
`COPY` and `--inserts` form.
**Relied on by:** `architecture.md` ("TOC enrichment") — it is why
`looks_like_toc_name_line` keeps refusing `"Data for "`, and it is the whole of
the `--disable-triggers` known gap in `../status/STATUS.md`.
**Re-verify:**
```sh
grep -n "_printTocEntry(AH, te, true)" -A 25 src/bin/pg_dump/pg_backup_archiver.c
pg_dump -d <db> --data-only --disable-triggers | grep -B 6 'DISABLE TRIGGER ALL'
```
