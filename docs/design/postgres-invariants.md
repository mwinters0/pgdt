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

**Half of that walk is mechanical, and it goes first.** Add the new major to
`ROUTINE_VERSIONS` in `scripts/generate_fixtures.py`, regenerate
(`cd scripts && uv run generate_fixtures.py --version <N>`), and read what the
cross-major differ says:

```sh
cd scripts && uv run oracle_differences.py
```

Every comparison the new server answers differently from its predecessor is
named there with a verdict, and a **non-additive** one fails the run: it is a
break in the union rule I35 records, and it has to be understood before
anything is re-verified by hand. Re-file the differences with `--write` once it
is. The prose half — re-running each entry's `Re-verify` grep — is unchanged.

*Rejected: a separate "things to check when a new major lands" document.*
Nothing fails when nobody follows a checklist, and this project has twice
chosen a script over one (`measure.py --stale`, `deficiencies.py`). The ritual
keeps the home it already has — this file, whose every entry carries its own
re-verification step — and a second document would compete with it for the same
trigger.

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
**Relied on by:** `decisions.md` ("D30"); the mapping pass restating
`DumpMetadata` at **each** `\connect`ed database's first `COPY` block
(`decisions.md`, "D63"), which the per-database scope limit licenses — each
database's first `COPY` header closes that database's preamble;
`ColumnResolution::MetadataNotScanned` (`decisions.md`, "D43"), which answers
for metadata built by some scan other than the mapping pass.
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

What *is* one-per-table is the archive entry: `makeTableDataInfo()`
returns immediately if `tbinfo->dataObj != NULL`, so at most one `TABLE DATA`
entry exists per table, and it skips views, partitioned parents (`/* Skip
partitioned tables (data in partitions) */`), unselected foreign tables,
excluded unlogged tables, and `tabledata_exclude_oids`.

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

The TOC entry's `Name:` is the partition; the `COPY` header's name is the
root. Plain-format output carries a `-- load via partition root <root>` marker
line between the TOC comment and the header (from the entry's `defn` comment),
so the shape is detectable in the file itself; the pre-data section lists every
`ALTER TABLE ... ATTACH PARTITION`, so the set of leaf partitions is knowable
before any data is reached (I1). The marker survives `--no-comments`,
`--data-only`, both together, and `--inserts` (observed, 16.15): it is the
entry's `defn`, not a `COMMENT ON` statement.

The blocks are not contiguous. `DOTypeNameCompare()` in `pg_dump_sort.c`
orders entries by (type priority, namespace name, **object name**), and a
`TABLE DATA` entry's object name is the *partition's* own name — not the
root's. Any unrelated table whose name sorts between two partition names is
emitted between their blocks. Observed, flagless, 16.15, with partitions
`feel_a`/`feel_z` of root `feel` and an unrelated table `feel_m`:

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
**Relied on by:** `decisions.md` ("D49" — several blocks
under one key are legitimate, not an ambiguity; "D48" — a query cannot stop at the first matching block).
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
**Relied on by:** `decisions.md` ("D31").
**Re-verify:** `grep -rn 'noTocComments' src/bin/pg_dump/` — confirm the only
assignment is still the direct-connection one in `RestoreArchive()`.

---

## I4 — COPY TEXT data is written with `DATESTYLE = ISO`, `INTERVALSTYLE = POSTGRES` and `extra_float_digits = 3`, but no `TimeZone`

**Claim.** Temporal values in COPY TEXT are ISO-formatted, and `timestamptz`
carries an explicit UTC offset — so the *instant* is unambiguous. An
`interval` value is in the **`postgres`** style, which `pg_dump` pins on its
own connection exactly as it pins `DATESTYLE`, so the text a dump holds is
`EncodeInterval`'s `INTSTYLE_POSTGRES` form and nothing else (I40 is the
grammar). The dump-time session's `TimeZone` is **not** recorded anywhere in
the file, so the offset is whatever that session had (koji's is `+00`).

**Proof.** `pg_dump.c` runs `ExecuteSqlStatement(AH, "SET DATESTYLE = ISO")`,
`ExecuteSqlStatement(AH, "SET INTERVALSTYLE = POSTGRES")` — under the comment
*"Likewise, avoid using sql_standard intervalstyle"*, guarded at v13 by
`AH->remoteVersion >= 80400`, which every supported server exceeds — and
`SET extra_float_digits TO 3` (or the `--extra-float-digits` override) on the
*source connection*. None of the three is written into the dump:
`_doSetFixedOutputState()` in `pg_backup_archiver.c` emits `statement_timeout`,
`lock_timeout`, `idle_in_transaction_session_timeout`, `transaction_timeout`,
`client_encoding`, `standard_conforming_strings`, the search_path, ROLE,
`check_function_bodies`, `xmloption`, `client_min_messages` and `row_security`
— and no temporal or float settings at all.

**Observed shape.** koji: `2016-04-13 16:52:35.456696+00`, with fractional
seconds trailing-trimmed to between 0 and 6 digits (`…10.41925+00`,
`…52.27108+00`). Booleans render as `t`/`f`.

**Verified against:** v13.23, v14.24, v15.19, v16.15, v17.11 and v18.6 source
— all six carry the `SET INTERVALSTYLE = POSTGRES` statement; koji
(`pg_dump 16.14`) data. The committed fixtures are the behavioural half:
`fixtures/<13–18>/types/default.sql` writes `1 year 2 mons 3 days 04:05:06`
for a value inserted as `1 year 2 months 3 days 04:05:06`, which is the
`postgres` style at every major.
**Relied on by:** `decisions.md` ("Type resolution and decoders" — temporal mapping;
`interval` left as a `Utf8View`, and "D55", whose
`interval` comparison parses that one style), `docs/manual/type-handling.md`.
**Re-verify:** `grep -n 'DATESTYLE\|INTERVALSTYLE\|extra_float_digits'
src/bin/pg_dump/pg_dump.c` and `awk '/_doSetFixedOutputState\(ArchiveHandle/,/^}$/'
src/bin/pg_dump/pg_backup_archiver.c` — confirm no `SET TimeZone` appears in
the emitted output, and that the three settings above are on the source
connection only.

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

Nor can two blocks of one table (I2) be assumed to match each other: under
load-via-partition-root `dumpTableData()` takes the header's *name* from
`getRootTableInfo(tbinfo)` but its *list* from `fmtCopyColumnList(tbinfo, …)`,
the leaf's own attributes in the leaf's `attnum` order — which differs from
the root's for a partition created standalone and then attached. So each
block of a partitioned table lists the same names in its own order.

**Verified against:** v18.6 (source); the `dropped_column`/`generated_column`
tables in `scripts/fixture_schema_edge_cases.sql` reproduce both shapes in
real `pg_dump` output on 13.23/16.15/18.6 — the dummy column only appears
under `--binary-upgrade`, matching the gate above. The absent list and the
per-leaf list: `scripts/fixture_schema_partitions.sql`'s `hollow` (no
columns), `derived` (only generated ones) and `shuffle` (a leaf attached in
another order) on 13.23, 14.24, 15.19, 16.15, 17.11 and 18.6, under both
flag sets.
**Relied on by:** `decisions.md` ("D43" —
the `COPY` header is authoritative; the DDL is a by-name type lookup);
`stream.rs`'s `TableColumns`, which reorders a table's blocks by name,
unions their census by name, and reads a header listing no columns as a
block copying none.
**Re-verify:** `awk '/^fmtCopyColumnList\(/,/^}$/' src/bin/pg_dump/pg_dump.c`,
and `grep -n 'fmtCopyColumnList(tbinfo' src/bin/pg_dump/pg_dump.c` still
passing the leaf's `tbinfo` where `copyFrom` is the root's.

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
`COPY` block. The same dump carries this OID-preservation noise before
*every* object's real statement (tables included, not just types), so under
`--binary-upgrade` it stands between a TOC comment and the statement it
announces.
**Relied on by:** `decisions.md` ("Type resolution and decoders", enums).
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
**Relied on by:** `decisions.md`, "D51" — a sub-stream that
starts inside a `COPY` block resyncs to a *line start* and never hands the
scanner a mid-row byte, because this claim is about a line start and a row's
own tail can be the two bytes `\.` (a value ending in an escaped backslash, cut
between them). Also `decisions.md`, "D52": a piece of an open block's interior
finds its first row at a line start, and the earliest piece reporting a `\.`
line holds the real terminator, since no line inside the block can be mistaken
for one. The scanner enumerates lines; the needle search this would make safe
is deferred (`decisions.md`, "D29").
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
through the same path. No koji *column* uses a user-defined type; the case is
covered by `fixture_schema_types.sql`.

What makes a built-in bare is visibility, not standardness: `pg_catalog` is
on the search path implicitly whatever `search_path` is set to, so
`format_type` qualifies nothing in it — including a type with no standard SQL
spelling at all, which falls back to its bare `typname`. It is searched
*before* the explicit list unless the path names it (`namespace.c`'s header
comment), so only a path naming `pg_catalog` after another schema lets a
declared object shadow one of its names.
`fixtures/<13–18>/types/default.sql` writes `t_int2vector`'s column as `v_vec
int2vector` at all six majors. A declared name with no `.` in it is a
`pg_catalog` name, whether or not the SQL standard has a word for the type.

**The standard spelling is not always the one written.** `format_type` puts a
typmod where the grammar does — `timestamp(3) with time zone`,
`time(2) without time zone`, `interval day to second(2)` — and writes a
`character` or `bit` column with no typmod as `bpchar` and `"bit"`, the SQL
word alone meaning length 1 (`format_type.c`, the `BPCHAROID` and `BITOID`
cases). `fixtures/<13–18>/types/default.sql`'s `t_type_spelling` carries each
at all six majors; `pgtype.rs`'s `builtin_name` reads them.

**Verified against:** v18.6 source; koji and all three fixture versions emit
the empty-`search_path` line.
**Relied on by:** `decisions.md` ("Type resolution and decoders").
**Re-verify:** `grep -n 'dumpSearchPath' -A45 src/bin/pg_dump/pg_dump.c`, and
confirm fixtures still contain `set_config('search_path', '', false)`;
`grep -n 'BPCHAROID' -A14 src/backend/utils/adt/format_type.c`;
`grep -n 'implicitly-searched namespaces' -A14 src/backend/catalog/namespace.c`; and
`cargo test -p pgdump_query --test pgtype`, which reads `t_type_spelling`.

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
otherwise pure `CREATE DATABASE` noise (see `PreambleBuilder`'s docs) and is
discarded, so the headers are carried forward onto the database the first
`\connect` switches into. Every `\connect` gets that carry-over, not just the
first.

`pg_dumpall` emits two segment shapes, and the header order differs between
them. `dumpDatabases()` passes `--create` to its `pg_dump` child for ordinary
databases — those segments print their version headers *ahead of* their own
`\connect`, which puts the lines in `PreambleBuilder::feed_line` while
`current` is still the previous database's finished (`preamble_complete`)
segment, so they are staged in `PreambleBuilder::pending_headers` and consumed
on that segment's own `\connect`. For `postgres` and `template1` it passes
**no** `--create` and writes `\connect <db>` itself, under the comment "Since
pg_dump won't emit a `\connect` command, we must"; those segments print
`\connect` *first*, so their headers arrive into a segment that is already
`current` and not yet `preamble_complete` — `feed_line`'s ordinary path, not
the staging one. Both databases are always dumped, so every real `pg_dumpall`
file contains both shapes; a hand-built concatenation of `--create` outputs
produces only the first.

Both segment shapes hold against a real `pg_dumpall` run:
`fixtures/{13,18}/edge_cases/dumpall.sql`
(`pg_dumpall --no-role-passwords` against the fixture cluster) show, on both
the oldest and newest routine versions, `template1`/`postgres` printing
`\connect` *then* their version-header pair, and `pgdt_fixture` printing the
pair *then* its own `\connect`.

**Verified against:** v18.6 source (`RestoreArchive()`,
`pg_dumpall.c`'s `dumpDatabases()` per-database invocation and its
`postgres`/`template1` special case); koji
(`pg_dump 16.14`); two concatenated `--create` fixtures
(`pgdump_query/tests/common/mod.rs`'s `multidb_fixture`, versions 13/16/18);
`fixtures/{13,18}/edge_cases/dumpall.sql`, a real `pg_dumpall` run.
**Relied on by:** `decisions.md` ("D36", "D69").
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

Thirdly: a range type's `canonical` function is written into that same body,
as `canonical = <function>`, whenever `pg_range.rngcanonical` is set. The
parameter is emitted after `multirange_type_name`, `subtype_opclass` and
`collation` and before `subtype_diff`, so it is never the first parameter and
never the last. It is the file's only evidence that such a function exists,
and PostgreSQL rewrites every value of that range through it before storing or
comparing one (I46) — arbitrary server-side code no reader of the dump can
apply.

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

The `canonical` parameter is the same function, six statements further on and
unconditional in every major:

```c
	procname = PQgetvalue(res, 0, PQfnumber(res, "rngcanonical"));
	if (strcmp(procname, "-") != 0)
		appendPQExpBuffer(q, ",\n    canonical = %s", procname);
```

`rngcanonical` is a `regproc`, and a dump runs with `search_path` set to `''`,
so the name it renders to is schema-qualified.

**Observed.** Probed against `postgres:16-alpine` (16.15): `CREATE TYPE
public.myrange AS RANGE (subtype = float8);` dumps as `CREATE TYPE
public.myrange AS RANGE (\n    subtype = double precision,\n
multirange_type_name = public.mymultirange\n);` — note `float8` came back
canonicalised to the two-word `double precision`. A table with a
`public.mymultirange` column dumps that column with no accompanying type
definition anywhere in the file. The `canonical` parameter is observed against
a 16.15 dump of a range type declaring one; the fixture family carries no such
type, since a canonical function must be declared against the shell type and a
SQL function cannot take one (I11's `LANGUAGE internal` recipe is what it
takes).

**Consequence for `crate::pgtype`.** `TypeKind::Range` carries the companion
name so a column declared with it resolves to `DeferredKind::Range` instead
of `Unknown`. The six built-in multirange names are recognized in the
built-in table alongside the six built-in range names, since like them they
appear bare and never reach the user-defined lookup (I8). The range grammar
tolerates a multi-line body and a multi-word subtype value.

**Verified against:** v18.6 source (`selectDumpableType()`,
`dumpRangeType()`), v13.23 source (`dumpRangeType()`, no multirange, and the
same `canonical = %s` append); probed `pg_dump` 16.15;
`fixtures/{13..18}/types/default.sql`'s
`public.myrange`/`public.myrange_multi` — all six routine versions, carrying
the PG13/PG14+ split in the `multirange_type_name` parameter's presence. No
fixture carries a `canonical` parameter, so that third claim rests on the
source at both ends of the supported range plus the 16.15 probe.
**Relied on by:** `decisions.md` ("Type resolution and decoders" — the mapping
table and "Ranges and multiranges", and "D58" for the `canonical` parameter);
`pgtype.rs`'s companion lookup and `TypeKind::Range`'s
`multirange_type_name` and `canonical` fields.
**Re-verify:** `grep -n 'skip auto-generated array and multirange types' -A 4
src/bin/pg_dump/pg_dump.c` — confirm multiranges are still `DO_DUMMY_TYPE`;
`grep -n 'AS RANGE' -A 40 src/bin/pg_dump/pg_dump.c` — confirm the parameter
list is still emitted one-per-line, that `multirange_type_name` is still the
only trace of the companion, and that `rngcanonical` still reaches the body as
`canonical = %s`.

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

The same three statements are in `scripts/fixture_schema_types.sql`, generated
across all six routine versions (`fixtures/<version>/types/default.sql`) and
backed by tables (`t_base_type`, `t_user_range`) so the types round-trip
through a real `COPY` block. `pg_dump` writes the completed `mybase` body's
parameters in its own order — `INTERNALLENGTH` first, then
`INPUT`/`OUTPUT`/`ALIGNMENT`/`STORAGE`.

**Consequence for `crate::preamble` / `crate::pgtype`.** One name can own two
`TypeDef` entries, so `resolve_user_type`'s first-match lookup returns the
`Shell` entry for a completed base type — harmless while `Base` and `Shell`
share the `OpaqueBaseType` outcome, and a first-match lookup over a list that
is not unique by name.

**Verified against:** v18.6 source (`dumpShellType()`, `dumpUndefinedType()`,
`DefineType()`); probed `pg_dump` 16.15; `fixtures/{13..18}/types/default.sql`
— the same recipe across all six routine versions.
**Relied on by:** `decisions.md` ("D69", "Type resolution and decoders").
**Re-verify:** `grep -n '"SHELL TYPE"' -B 12 src/bin/pg_dump/pg_dump.c` —
confirm `dumpShellType()` still emits a bare `CREATE TYPE x;` ahead of the
real definition; re-run the recipe above against the newest major.

---

## I12 — Large-object data is a line-oriented region that cannot contain a `COPY` block

**Claim.** In plain-format output, large-object *data* sits after every
`COPY` block and before post-data DDL, and its contents are ordinary
single-line SQL statements — `SELECT pg_catalog.lo_open('<oid>', 131072);`, a
run of `SELECT pg_catalog.lowrite(0, '\x…');`, `SELECT
pg_catalog.lo_close(0);` — each `BEGIN;`/`COMMIT;`-wrapped. No line inside
it can be mistaken for a `COPY` header or a `\.` terminator, because the
payload is a bytea hex literal and hex cannot contain a line break.

How many archive entries the data is split across differs by major, and the
routine version matrix (13-18) spans both shapes — confirmed by
`scripts/fixture_schema_objects.sql`'s two hand-built large objects, real
`pg_dump` output, all 6 versions (`fixtures/<version>/objects/default.sql`):

| | v13-16 | v17-18 |
|---|---|---|
| Definition entry (ownership/comment/ACL) | one `BLOB` entry per object | one `BLOB METADATA` entry per object |
| Data entry | **one `BLOBS` entry for every large object in the dump**, one shared `BEGIN;`/`COMMIT;` wrapping every object's `lo_open`/`lowrite*`/`lo_close` run back to back | **one `BLOBS` entry per object**, each with its own `BEGIN;`/`COMMIT;` |
| `lo_create` call | inside the `BLOB` definition entry | inside the `BLOB METADATA` definition entry, in the same position |

Large-object *definitions* (ownership, ACL, comments) are separate entries
that sort ahead of the pre-data boundary on both shapes.

**Proof.** `dbObjectTypePriorities` (`pg_dump_sort.c`) places
`PRIO_LARGE_OBJECT_DATA` between `PRIO_TABLE_DATA` and
`PRIO_POST_DATA_BOUNDARY`, and `PRIO_LARGE_OBJECT` (the definition entries)
before `PRIO_PRE_DATA_BOUNDARY` — so the data region sits in a fixed position
either way, never interleaved with table data. On v13-16, `pg_dump.c`
creates exactly one archive entry named `BLOBS` (`.description = "BLOBS"`)
covering every large object in the database; on v17+ (`pg_dump.c`, "Create a
BLOBS data item for the group, too") large objects are split into groups, each
group with its own `BLOBS` entry — a fixture with two large objects exercises
only one group, so this evidence does not pin down the grouping threshold,
only that v17+ is not always exactly one.
`StartRestoreLOs()`/`EndRestoreLOs()` (`pg_backup_archiver.c`) emit the
`BEGIN;`/`COMMIT;` wrapper (one per data entry, whatever it covers) when not
restoring to a live connection; `_StartLO()` emits the `lo_open(...)` line,
and `dump_lo_buf()`'s no-connection branch emits each chunk through
`appendByteaLiteralAHX()` as `SELECT pg_catalog.lowrite(0, %s);`.

**Size.** Chunks are `LOBBUFSIZE`-bounded but the region is not: hex encoding
roughly doubles the on-disk size of the objects, so a dump of a
large-object-heavy database can carry hundreds of gigabytes here. This is the
one inter-`COPY` gap not bounded by schema size.

**Consequence.** `StartRestoreLOs()`/`EndRestoreLOs()` are the *only* emitter
of a bare `BEGIN;`/`COMMIT;` pair anywhere in plain `pg_dump` output
(confirmed across the fixture set used to verify this entry), so
`crate::scan::CopyScanner` recognizes `BEGIN;` as a large-object region opener
(`State::InLargeObjectRegion`, the tier `State::InCopy` sits at) and skips
every line up to `COMMIT;` unread instead of emitting `Event::Line` for each
one (`decisions.md`, "D33").

On v17+, `crate::map::Builder` merges the several `BEGIN;`/`COMMIT;` regions
the scanner reports into one `Data(LargeObjects)` span, on the priority-band
fact above: `BLOB METADATA`/`ACL`/`COMMENT` definition entries sort entirely
*before* the pre-data boundary and every `BLOBS` data entry sorts together in
its own contiguous band, so nothing else can legitimately arrive between two
of a v17+ file's `BLOBS` entries and a second `BEGIN;` arriving before any
span push is the same region continuing. An input violating this ordering
degrades to several smaller spans, still correctly tiled. The per-object OID
is recoverable from the TOC header alone on v17+
(`-- Data for Name: <oid>; Type: BLOBS`) and only from the
`lo_open('<oid>', ...)` line on v13-16, where every object's data shares one
entry; per-object spans are not built either way.

**Verified against:** v13.23 through v18.6 source (`pg_dump_sort.c`
priorities; `dumpLOs`/`BLOBS`/`BLOB METADATA` entries; `StartRestoreLOs`,
`_StartLO`, `dump_lo_buf` in `pg_backup_archiver.c`) and
real fixture output on all 6 routine versions
(`fixtures/<version>/objects/default.sql`) — koji has no large objects, so
this is fixture-only.
**Relied on by:** `decisions.md` ("D33"); `pg-dump-compatibility.md`.
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
`WITH (FORMAT csv)`, `WITH (FORMAT binary)`, a custom `COPY_TEXT_DELIMITER`, or any
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

**Consequence.** The scanner's `parse_copy_header` rejects any `COPY` line
carrying a trailing `WITH (...)` and treats it as ordinary SQL rather than
guessing — safe because `pg_dump` never emits one, so such a line is either
not `pg_dump` output or inside a dollar-quoted body, where it must be ignored
anyway. CSV-format `COPY` blocks are a non-shape, not a compatibility gap:
see `pg-dump-compatibility.md` and P8 Track A in `roadmap.md`. `--inserts`
*is* a real output shape and a real gap, scheduled in P8 Track A.

**Verified against:** v13.23, v16.15 and v18.6 source (`dumpTableData()`,
`dumpTableData_copy()`, `pg_backup_tar.c`'s `copyStmt` check) — the `COPY`
format string is unchanged across all three. Also consistent with every
fixture in `fixtures/` and with koji (`pg_dump 16.14`), none of which
contains a `COPY` line with an options clause.
**Relied on by:** `pgdump_query/src/copy.rs`'s `parse_copy_header` (its doc
comment states the deliberate non-match for `WITH (...)`);
`pg-dump-compatibility.md` ("`COPY` header variants" and "CSV-format `COPY`
blocks"); `roadmap.md` (P8 Track A).
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
(`docs/design/decisions.md`, "D44"), whose fixed/scientific decision uses
`FLT_DIG`/`DBL_DIG` (6/15) as the threshold, and the round-trip test under
`decisions.md`'s "D73".
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

**Relied on by:** `pgdump_query/src/copy.rs`'s `encode_field`, which
implements these seven escapes only — `decode_field`'s inverse for text
`pg_dump` could have produced, not a general COPY-text encoder. Also
`decisions.md`, "D52": `\n` is one of the seven, so a literal LF byte inside a
data region is always a row boundary and never part of a value, which makes an
LF-split of an open `COPY` block's interior a split into whole rows.

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
comments at all, which is why `decisions.md` treats them
as an enrichment layer over a statement-driven pass rather than as the primary
structure.

**Verified against:** v18.6 source; koji (`pg_dump 16.14`); fixtures at 18.6.

**Relied on by:** `decisions.md` ("D30"; "D31").

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
**Relied on by:** `decisions.md` ("D33"), which is why `INSERT` runs get a string-aware scan
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
Type: STATISTICS DATA; Schema: public; Owner: -` — the empty-owner shape,
distinct from `Owner: -`, and not covered by the `Owner:`/`Schema:` grammar
above, since `dumpRelationStats` sets no owner at all). The flag that produces
it is `--statistics`; `--with-statistics` exists in no version.
`map::parse_toc_header_line` recognizes all three prefixes; the boundary
signal `looks_like_toc_name_line` recognizes `TOC_PREFIX_STATS` but not
`TOC_PREFIX_DATA`, since only the former heads a statement rather than a
`COPY` block.

**Verified against:** v18.6 source; fixtures at 13.23 through 18.6 for the
`Schema:`/`Owner:` placeholder shapes and the `Tablespace:` suffix; v18.6
fixture for `TOC_PREFIX_STATS` (PG18+ only, per `--statistics`'s own
availability).
**Relied on by:** `decisions.md` ("D31") — `map::parse_toc_header_line` splits the
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
/ the `want == ""` branch's literal `''`). Byte-identical across
v13.23 through v18.6 (the `ALTER ... OWNER TO` emission point sits elsewhere
in v13 than in v18 — a helper-buffer refactor — while the text it produces is
identical; see I16). Observed in
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
its definition, `SET default_tablespace = '';` after) — the same fixture I18's
`Tablespace:` claim cites.

**Verified against:** v13.23 through v18.6 source; `fixtures/16/objects/default.sql`
for the `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES`/`SET default_tablespace`
(both a real tablespace and the reset) shapes.

**Relied on by:** `decisions.md` ("D30") — `preamble::extract_statement_cross_refs` matches these four
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
what a dump contains, not about what the corresponding `*_in` functions accept.
Those are considerably more permissive, and disagree with each other where
these three merely differ — **I44** is that half. A reader must handle what is
emitted; a *renderer* that claims to reproduce a dump byte-for-byte must
reproduce the `needquote` predicates exactly, including the `NULL` case-fold
and the whitespace test.

**Verified against:** v13.23, v16.15, v18.6 — the `needquote`/`nq` predicates
and both emit loops are character-for-character identical across all three.

**Relied on by:** `decisions.md`, "D45" — the decoder's
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

**Relied on by:** `decisions.md`, "Type resolution and decoders" (the array paragraph)
and "D35" — it is the reason an array column's Arrow type
cannot be settled from the DDL alone.

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
   DOMAIN` has no `COPY_TEXT_DELIMITER` clause — so **the base type's name is the only
   trace of the delimiter in the file.**

**Proof.** `src/include/catalog/pg_type.dat` contains exactly one `typdelim =>
';'` line, on `box`, identical in v13.23 through master.
`src/backend/commands/typecmds.c`, in the domain-definition path: `/* Array
element Delimiter */ delimiter = baseType->typdelim;` — alongside the same
copy-from-base treatment given to alignment, storage, category and the output
function. A user-defined base type may also set one (`CREATE TYPE … COPY_TEXT_DELIMITER =
';'`), and there `pg_dump` *does* emit the clause — but such a type resolves as
`TypeKind::Base` and is refused on its own account, so the clause never has to
be parsed.

**Scope limit.** Composites are unaffected: `record_out`
(`src/backend/utils/adt/rowtypes.c`) writes `appendStringInfoChar(&buf, ',')`
unconditionally, with no reference to any field type's `typdelim`. Ranges and
multiranges likewise separate with a literal `,`; arrays are the entire
exposure.

**Verified against:** v13.23, v18.6 and master (`pg_type.dat`); v18.6
(`typecmds.c`, `rowtypes.c`).

**Relied on by:** `decisions.md`, "Type resolution and decoders" (both array refusals)
and "D45" — it is why the opaque-element refusal tests the
element type *after* domain unwrapping rather than the declared string, and why
the array separator can stay hardcoded to `,` once it does.

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

**Relied on by:** `decisions.md`, "Type resolution and decoders" (the all-or-nothing
field list) and "D39" — "no fields parsed" and "no fields declared" have to stay
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
they are composite". Domains carry a separate guarantee: a domain's base type
must exist before the domain does.

**Scope limit.** This is a property of what PostgreSQL will *create*, so it
holds for every real dump and says nothing about a hand-edited file. It is
also silent about `RECORD`/`ANYARRAY` pseudo-types, which the same function
exempts and which never appear as a column's declared type in `pg_dump`
output.

**Verified against:** v13.23 and v18.6 (`heap.c`, same check and message),
plus both rejections observed live on the local PostgreSQL 16 instance.

**Relied on by:** `decisions.md`, "D41" — it is why `pgtype.rs`'s type walks
can be bounded by the number of definitions the file declares: a path that
visits more has visited one twice, which only a file this does not hold for
reaches, and it answers `Unknown` rather than recursing without end.

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

**Relied on by:** `decisions.md`, "D35" — it is why
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

**Relied on by:** `decisions.md`, "Type resolution and decoders" — the shape
cannot be typed as nested `List`s, since one `NestedPlan::Array` chain would
have to mean two different depths; the column resolves to `Utf8View` with
`ColumnResolution::NestedArrayElement`, which lets
`resolve::retype_from_census` assume every plan it sees is
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

**Relied on by:** `decisions.md` ("Type resolution and decoders", enums) — an enum whose
label set is empty resolves to `ColumnResolution::EmptyEnum` rather than a
`Dictionary`, which is sound only because the label folding I6 describes runs
first.

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

The spelling constrains nothing about the values: a column declared
`integer[3]` may hold a four-element array, and one declared `integer[]` may
hold `{{1,2},{3,4}}`. Only the census over actual values says what a column's
shape is.

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

Two details a normalizer needs. The bracket run is unbounded in the DDL even
though `MAXDIM` is 6: `int[][][][][][][][][]` is accepted and yields
`integer[]` with `attndims` 9, so a declaration's bracket count needs no cap
and carries no meaning. And `ARRAY` is a keyword, so it is case-insensitive —
`INTEGER ARRAY` and `Integer Array[4]` are the same declaration. Both observed
on 16.15 alongside the table above.

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

**Relied on by:** `decisions.md` ("Type resolution and decoders") — it is why resolution
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

All six rows must print `integer[]`, with `attndims` varying — the record of
the declaration, which nothing downstream reads.

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

The quotes are what keep the array-bounds production unambiguous:
`s."x ARRAY"` is a scalar column of a
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
PostgreSQL"), and on what this build does with such a file — deficiency `KD4`
(`decisions.md`, "Type resolution and decoders").

**Verified against:** v16.15, live, `pg_dump 16.14`. Not re-checked on other
majors: `fmtId()` and the quoting rule are not version-varying, and the
`Typename` grammar this interacts with is identical across v13-v18 (I28).

**Relied on by:** `pgtype.rs`'s `array_element` — its bound- and
keyword-stripping helpers bail on a trailing `"`. Deficiency `KD4` rests on
the second half of the claim: because
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
all three; and observed in output on all six routine majors, where
`pgdt_tenant` lands between `pgdt_fixture` and `postgres` in every
`edge_cases/dumpall.sql`.
**Relied on by:** `decisions.md` ("D69"). The `edge_cases/dumpall`
fixture's database sequence is chosen by naming, not observed: a second
data-carrying database named `pgdt_tenant` lands between `pgdt_fixture` and
`postgres`, which is what makes "cancel inside the *second* database's data" a
deterministic file offset for the recurring-metadata-boundary test
(`decisions.md`, "D63"). If the ordering
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
comment block is therefore not adjacent to the data it heads — the one
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
**Relied on by:** `decisions.md` ("D31") — why
`looks_like_toc_name_line` refuses `"Data for "`; and the whole of deficiency
`KD1`.
**Re-verify:**
```sh
grep -n "_printTocEntry(AH, te, true)" -A 25 src/bin/pg_dump/pg_backup_archiver.c
pg_dump -d <db> --data-only --disable-triggers | grep -B 6 'DISABLE TRIGGER ALL'
```

## I32 — A plain dump records no database collation unless `--create`

**Claim.** A `pg_dump` plain-format file carries **no statement of the
database's collation** — no `LC_COLLATE`, no `LOCALE`, no `ICU_LOCALE` — unless
it was taken with `--create` (or `-C`). The preamble states
`client_encoding`, `standard_conforming_strings` and `search_path`, and stops
there. A per-column `COLLATE` clause appears in `CREATE TABLE` only where the
column's collation differs from **its type's** default — not the database's —
so it never supplies the database default either. It does, however, say more
than nothing: a column whose type's default collation is `default` (`text`,
`varchar`, `char`) and that carries no clause is on the database default and
therefore unknown, while a clause that *is* present states the collation
outright, and a `name` column with no clause is on `C`, because `name`'s
`typcollation` is `C` rather than `default`.

**Proof.** `dumpDatabase` is the only emitter of `LC_COLLATE`/`LOCALE`/
`ICU_LOCALE`, and `pg_dump.c`'s `main` calls it under exactly one condition:

```c
/* The database items are always next, unless we don't want them at all */
if (dopt.outputCreateDB)
    dumpDatabase(fout);
```

`outputCreateDB` is set by `--create` alone. Verified in the v13.23, v14.24,
v15.19, v16.15, v17.11, v18.6 and master worktrees — the gating line is
identical in all of them.

The per-column half is `dumpTableSchema`'s own attribute query, whose comment
states the rule verbatim: *"Since we only want to dump COLLATE clauses for
attributes whose collation is different from their type's default, we use a
CASE here to suppress uninteresting attcollations cheaply."* Observed against
`postgres:16` (16.15, glibc 2.41): `pg_type.typcollation` is `default` for
`text` and `C` for `name`, and `'A' < 'a'` answers false for `text`, `varchar`
and `char(n)` while answering true for `name`.

**Scope limit.** A `--create` dump *does* carry it, and so does `pg_dumpall`,
whose per-database `CREATE DATABASE` statements come from the same function. So
this is a statement about the ordinary single-database plain dump, which is the
input pgdt is built around, not about every file `pg_dump` can write.

**Consequence.** A text ordering comparison on a `default`-collation column
with no `COLLATE` clause cannot be made to agree with the server from the dump
alone: PostgreSQL orders `text` by collation, and under any non-`C` collation
the answer differs from a bytewise comparison (`'a' < 'B'` is true in
`en_US.UTF-8`, false bytewise). pgdt therefore compares bytewise and
**registers the divergence** rather than claiming agreement — see
[`decisions.md`](decisions.md)'s "Predicates", which holds the ordering
register. The comparison oracle asks each text pair under `COLLATE "C"` and
under the database's own collation, so `fixtures/<version>/oracle/comparisons.tsv`
shows bytewise agreeing under the first and diverging under the second.

Where the file *does* state the collation, the register says so instead: an
explicit `COLLATE "C"`/`"POSIX"`, and a bare `name` column, agree with the
server exactly and on every server. What is unknown is only the database
default, and only for columns whose type defers to it.

The database default alone would not settle the order either. PostgreSQL
applies whatever the platform's libc provides for a locale name and promises no
more: `en_US.utf8` is `strcmp` on musl and glibc's collation on glibc, so
`'A' < 'a'` answers `t` on the one and `f` on the other. The server tracks that
in `pg_collation.collversion` — `2.41` for a libc-provider collation, the libc
version verbatim — and no dump carries it.

**Relied on by.** The comparison register's `UnknownCollation` verdict — the
one it reaches for a `default`-collation column with no clause. The clause's own
grammar, at every site it appears, is I37.

**Re-verify.**

```sh
grep -n 'outputCreateDB' /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/pg_dump/pg_dump.c
pg_dump --no-owner mydb | grep -ci 'lc_collate\|locale'   # expect 0
pg_dump --create --no-owner mydb | grep -ci 'locale'      # expect > 0
grep -n -B4 'suppress uninteresting attcollations' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/pg_dump/pg_dump.c
psql -c "select typname, typcollation from pg_type where typname in ('text','name')"
```

---

## I33 — The scalar comparison operators the ordering register claims agreement with are byte- or value-order, and NaN is the largest float

**Claim.** For the types the ordering register marks *Agrees*, PostgreSQL's
own `<`/`<=`/`>`/`>=` are exactly the order pgdt computes over the decoded
value:

- **`float4`/`float8`** — `NaN` sorts **above** every other value, infinity
  included, and `NaN = NaN` is true. This is not IEEE and not Rust's
  `partial_cmp`, which answers `None` for either.
- **`uuid`** — `memcmp` over the 16 bytes, with no field-wise or
  version-aware reordering.
- **`bytea`** — `memcmp` over the shorter length, then the shorter value
  first: `[u8]`'s own lexicographic order.
- **`boolean`** — C's `>` on the two values, so `false < true`.
- **`numeric`** — compared by value and never by display scale, so `1.5` and
  `1.50` are one value written two ways. For two values already carried to one
  scale that is the order of their unscaled integers; for a column that keeps
  its own scale per value it is the order of sign, then magnitude, then
  fraction, over the digits `numeric_out` wrote.
- **enum** — ordered by `pg_enum.enumsortorder`, which `AddEnumLabel`
  assigns from **declaration order**, never by the label text.

**Proof.** `src/include/utils/float.h`:

```c
float8_gt(const float8 val1, const float8 val2)
{
	return !isnan(val2) && (isnan(val1) || val1 > val2);
}
```

`float8_cmp_internal` is `float8_gt`/`float8_lt` and nothing else.
`uuid_internal_cmp` (`src/backend/utils/adt/uuid.c`) is a bare
`memcmp(arg1->data, arg2->data, UUID_LEN)`. `byteacmp`
(`src/backend/utils/adt/varlena.c`) is `memcmp` over `Min(len1, len2)` then a
length tiebreak. `boolgt` (`src/backend/utils/adt/bool.c`) is `arg1 > arg2`.
`enum_cmp_internal` (`src/backend/utils/adt/enum.c`) compares OIDs on the fast
path and `enumsortorder` otherwise, and `pg_enum.c` fills `enumsortorder` with
`elemno + 1` in declaration order.

`numeric`'s scale-independence is `cmp_numerics`' non-special branch, which
delegates to `cmp_var_common` — a comparison of sign, then weight, then
digits, with no reference to `dscale` at all. `numeric_out` is the other half:
it returns the three special spellings and otherwise `get_str_from_var`, which
writes an optional `-`, the integer digits and `dscale` fraction digits, in
plain notation with no exponent — so the text in a dump is exactly the digit
string that comparison reads.

**Scope limit.** *Agreement of the operator*, not of the value: a type whose
decoder loses information diverges regardless, which is why `text` (I32,
collation) is registered as divergent rather than covered here. A bare
`numeric` **is** covered — it is held as text and compared as an
arbitrary-precision decimal over that text, which loses nothing. Says nothing
about the nested types, whose comparison stays a string comparison.

**Verified against.** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 and
master — all seven carry the `float8_gt` and `uuid_internal_cmp` bodies quoted
above verbatim, and in all seven `cmp_numerics`' non-special branch is the one
`cmp_var_common` call quoted above and `numeric_out` ends in
`get_str_from_var`. v13's `numeric_out` handles `NaN` alone; v14+ handle the
infinities as well (I34).

**Relied on by.** The comparison register's *Agrees* and enum rows —
[`decisions.md`](decisions.md), "D55";
`pgtype.rs`'s `comparison_for` and `predicate.rs`'s `pg_float_cmp`.

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
grep -n -A3 'float8_gt(const float8' src/include/utils/float.h
grep -n 'memcmp(arg1->data, arg2->data, UUID_LEN)' src/backend/utils/adt/uuid.c
awk '/^byteacmp/,/^}/' src/backend/utils/adt/varlena.c
grep -n 'Anum_pg_enum_enumsortorder' src/backend/catalog/pg_enum.c
grep -n -A8 'cmp_var_common(NUMERIC_DIGITS' src/backend/utils/adt/numeric.c
awk '/^numeric_out\(PG_FUNCTION_ARGS\)/,/^}/' src/backend/utils/adt/numeric.c
```

---

## I34 — `date`/`timestamp`/`interval` infinities and `numeric` `NaN` are ordered values with one fixed spelling, and a typmod'd `numeric` cannot hold an infinity

**Claim.** Four properties, each about a value a healthy database can hold and
`pg_dump` therefore writes:

- **`date`, `timestamp` and `timestamptz` have `infinity` and `-infinity`**,
  written in exactly those two spellings, lower case and unsigned-positive.
  Each type's own `<`/`>` places `-infinity` below and `infinity` above every
  finite value of that type, and each equals itself.
- **`interval` has the same two on v17+**, in the same two spellings and with
  the same order. It is the one of the four whose ordering is not an integer
  comparison over a reserved range: `interval_cmp_value` collapses an
  `Interval` to a 128-bit span (I40), and an infinite one has every field at
  its own extreme, so the extreme span falls out of the arithmetic rather than
  being special-cased.
- **`numeric` has `NaN`**, written `NaN`, which orders **above** every non-NaN
  value — `Infinity` included — and equals itself.
- **A `numeric(p,s)` column can hold `NaN` but never `±Infinity`.** The typmod
  does not apply to a `NaN`; an infinity is rejected by *any* typmod
  restriction. Since a `numeric` without a typmod is the only other kind, no
  column that resolves to a `Decimal128`/`Decimal256` can hold an infinity.
- **`numeric` has `±Infinity` on v14+.** v13 has neither the values nor the
  rejection, so the previous property is vacuous there. `interval`'s two exist
  on v17+; v13–v16 reject the literal outright, which the oracle records as
  `E22007` and the cross-major differ reads as additive (I35).

**Proof.** The spellings are `src/include/utils/datetime.h`:

```c
#define EARLY			"-infinity"
#define LATE			"infinity"
```

which `EncodeSpecialDate`/`EncodeSpecialTimestamp` — and, from v17,
`EncodeSpecialInterval` — are the only writers of, and
`src/backend/utils/adt/numeric.c`'s `numeric_out`, which returns a literal
`"Infinity"` / `"-Infinity"` / `"NaN"` before it formats anything. The two
families capitalize differently, and each type answers only to its own
spelling.

The order is the representation. `src/include/utils/date.h` says *"Infinity and
minus infinity must be the max and min values of DateADT"* and defines
`DATEVAL_NOBEGIN`/`DATEVAL_NOEND` as `PG_INT32_MIN`/`PG_INT32_MAX`;
`src/include/datatype/timestamp.h` defines `DT_NOBEGIN`/`DT_NOEND` as
`PG_INT64_MIN`/`PG_INT64_MAX`. `date_lt` is `dateVal1 < dateVal2` and
`timestamp_cmp_internal` is `(dt1 < dt2) ? -1 : ((dt1 > dt2) ? 1 : 0)` — plain
integer comparisons over a range whose two ends are reserved.

`numeric`'s is explicit rather than representational, in `cmp_numerics`:

```c
	/*
	 * We consider all NANs to be equal and larger than any non-NAN (including
	 * Infinity).  This is somewhat arbitrary; the important thing is to have
	 * a consistent sort order.
	 */
```

and the typmod rule is `apply_typmod_special`, whose comment says *"NaN is
allowed regardless of the typmod … Inf is rejected if we have any typmod
restriction"*, ending in `errdetail("A field with precision %d, scale %d cannot
hold an infinite value.")`.

**Scope limit.** Says nothing about `real`/`double precision`, whose three
special values are IEEE's and are covered by I33. Says nothing about
*materializing* one: this is the order, and no Arrow type gains a
representation from it — `interval` included, which is held as a `Utf8View`
and whose infinities therefore reach a comparison and never a batch.

**Verified against.** The spellings, `DATEVAL_*`, `DT_*` and the `cmp_numerics`
comment are byte-identical in v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 and
master. `apply_typmod_special` exists in v14 onward only, and
`INTERVAL_NOBEGIN`/`INTERVAL_NOEND` in v17 onward only.
`fixtures/<13–18>/oracle/comparisons.tsv` is the behavioural record for
`interval`: every cell naming an infinity is `E22007` at 13–16 and an ordinary
answer at 17–18. Live against the koji replica (PostgreSQL 16.15): every one of
`'infinity'::date > '2020-01-01'`, `'2020-01-01'::date > '-infinity'`,
`'infinity'::timestamp > '2020-01-01'`, `'NaN'::numeric > 5`,
`'NaN'::numeric = 'NaN'`, `'NaN'::numeric > 'Infinity'::numeric` and
`'Infinity'::numeric > 5` answers `t`; `'NaN'::numeric(5,2)` yields `NaN`;
`'Infinity'::numeric(5,2)` raises *numeric field overflow*.

**Relied on by.** `predicate.rs`'s `special_order_key` and `OrderKey`'s three
non-finite variants — [`decisions.md`](decisions.md), "D55", whose `Date32`, `Timestamp`, `Decimal` and
`interval` register rows are *Agrees* only because of this.

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
grep -n 'define EARLY\|define LATE' src/include/utils/datetime.h
grep -n 'define DATEVAL_NOBEGIN\|define DATEVAL_NOEND' src/include/utils/date.h
grep -n 'define DT_NOBEGIN\|define DT_NOEND' src/include/datatype/timestamp.h
grep -n 'define INTERVAL_NOBEGIN\|define INTERVAL_NOEND' src/include/datatype/timestamp.h
awk '/^EncodeSpecialInterval/,/^}/' src/backend/utils/adt/timestamp.c
awk '/^timestamp_cmp_internal/,/^}/' src/backend/utils/adt/timestamp.c
grep -n -A8 '^cmp_numerics' src/backend/utils/adt/numeric.c
grep -n -A15 '^apply_typmod_special' src/backend/utils/adt/numeric.c
```

---

## I35 — No two supported majors disagree about a typed comparison both accept

**Claim.** Across PostgreSQL 13–18, for every case in the comparison oracle's
table, two adjacent majors that both *accept* an input agree about it: the same
six operators answer the same way, and an accepted literal canonicalizes to the
same `*_out` text. Every difference between two adjacent majors is **additive**
— the older one rejected an input the newer one accepts.

So version-varying semantics are implemented as the *newest* semantics
unconditionally, with no branch on the version the dump header records: an
older server cannot have produced a value it does not accept.

**Scope limit.** It is a property of the **cases the oracle asks**, not of
PostgreSQL in general: a type or operator no case constructs is not covered,
and adding a case is the only answer available (`comparison_oracle.py`'s
`TYPE_CASES`). It says nothing about a *minor* release, which the tree pins to
one per major. And the text answers are **glibc's** — the Debian (`-trixie`)
fixture containers, `datcollate` `en_US.utf8`, `collversion` 2.41 — so it does
not speak for a musl deployment, which orders the same locale bytewise
([`decisions.md`](decisions.md), "D70").

**Proof.** Observed: `fixtures/<13…18>/oracle/` holds 2020 comparisons and 317
literals per major as the server itself answered them, and
`fixtures/oracle-differences.tsv` holds every cell that differs between
adjacent majors — 533 of them, all additive. The three transitions are
`numeric`'s infinities and the two multirange types at v14, and `interval`'s
infinities at v17.

**Verified against.** 13.23, 14.24, 15.19, 16.15, 17.11, 18.6 — the versions
`meta.tsv` records per major.

**Relied on by.** [`decisions.md`](decisions.md), "D70", and every comparison there that implements one semantics for all
majors — "D55" for `interval`'s and `numeric`'s
infinities, "D45" for the four `*_in` transcriptions, and
"D58" for the multirange types.

**Re-verify.**

```sh
cd scripts && uv run oracle_differences.py
```

A non-additive difference is the failure, and it is reported by case with both
answers. Re-taking the oracles themselves is
`uv run generate_fixtures.py --skip-dumps`.

---

## I36 — The version header's version string is the build's full version string, which a packager may extend

**Claim.** `-- Dumped from database version <s>` and `-- Dumped by pg_dump
version <s>` do not carry a bare `<major>.<minor>`. `<s>` is whatever the
build's version string is, and a distribution that configures
`--with-extra-version` appends a parenthetical to it — Debian's PGDG packages
write `16.15 (Debian 16.15-1.pgdg13+2)`. A reader must therefore take the rest
of the line verbatim and must not parse a number out of it.

**Proof.** `RestoreArchive()` in `pg_backup_archiver.c` prints
`AH->archiveRemoteVersion` and `AH->archiveDumpVersion`. The first is
`AH->public.remoteVersionStr` (`pg_backup_db.c`), which is the connected
server's `server_version` GUC; the second is `PG_VERSION`, assigned once in
`_allocAH()`. `configure` defines that as `PG_VERSION="$PACKAGE_VERSION$withval"`
when `--with-extra-version` is given and as `$PACKAGE_VERSION` when it is not
— so both strings carry the packager's suffix on a build that sets it, and
neither does on a vanilla build.

**Observed.** Every fixture in `fixtures/` is a Debian (`-trixie`) build and
carries the parenthetical on both lines; the koji sample, dumped by a build
that sets no extra version, carries `16.14` on both. Both shapes are in the
tree.

**Scope limit.** It says nothing about the *content* of the suffix, which is a
packager's to choose, nor about the two lines agreeing with each other — a
dump taken by a client of a different build than the server writes two
different strings.

**Verified against.** 13.23, 14.24, 15.19, 16.15, 17.11, 18.6 (Debian PGDG),
and `pg_dump` 16.14 (koji, no extra version).

**Relied on by.** `crate::map`'s `version_header_field`, which keeps the rest
of the line whole, and `DatabaseMetadata::{server_version, pg_dump_version}`,
which are reported and never compared or ordered.

**Re-verify.**

```sh
grep -n -A3 'archiveRemoteVersion' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/pg_dump/pg_backup_archiver.c
head -9 fixtures/16/types/default.sql | grep 'Dumped'
head -9 /mnt/wd12t/fedora/koji/koji-2026-07-23.dump | grep 'Dumped'
```

---

## I37 — A `COLLATE` clause is written at three sites, in one form, and only where the collation differs from the type's own default

**Claim.** `pg_dump` writes `COLLATE <collation>` in exactly three places — a
`CREATE TABLE` column, a `CREATE DOMAIN` base type, and a composite `CREATE
TYPE`'s attribute — and in each it writes one only when that object's collation
differs from **its own type's** default (`pg_type.typcollation`). The reference
is always `fmtQualifiedDumpable`'s output: schema-qualified and quoted where
quoting is needed, so `pg_catalog."C"`, `pg_catalog."en_US.utf8"`,
`public.mycoll` — never a bare `C`.

Four consequences:

- **The clause is not adjacent to the type.** In a `CREATE TABLE` column it is
  appended *after* `DEFAULT`/`GENERATED` and after `NOT NULL`, so a parser that
  looks at the token following the type words finds nothing. In a `CREATE
  DOMAIN` and a composite attribute it does directly follow the type.
- **What can sit between the type and the clause is version-dependent.** On
  v13–v17 the column emission is
  `[GENERATED ALWAYS AS (expr) STORED | DEFAULT expr]` then `[NOT NULL]`. v18
  emits three further shapes in that window: a *named* not-null constraint,
  `CONSTRAINT <name> NOT NULL`; a following `NO INHERIT`; and a **virtual**
  generated column, `GENERATED ALWAYS AS (expr)` with no `STORED`.
  Two of the three have no fixture — see the scope limit below.
- **Its absence is a fact about the type, not about the column.** The four
  collatable built-ins split two ways: `text`, `varchar` and `bpchar` have
  `typcollation = default`, so a bare column of one is on the database's
  collation, which a plain dump never states (I32); `name` has `typcollation =
  C`, so a bare `name` column is bytewise on every server.
- **A domain's own clause is a type default in turn.** A column declared with a
  domain that itself says `COLLATE "C"` carries no clause of its own, and a
  column-level clause is written exactly to override the domain's.

**Proof.** Three emitters in `pg_dump.c`, each guarded by
`OidIsValid(...collation)` over a value the fetching query has already reduced
to zero where it matches the type's default:

- `dumpTableSchema`, whose attribute query carries the comment *"Since we only
  want to dump COLLATE clauses for attributes whose collation is different from
  their type's default, we use a CASE here to suppress uninteresting
  attcollations cheaply"*, and whose emission — `appendPQExpBuffer(q, " COLLATE
  %s", fmtQualifiedDumpable(coll))` — sits under the comment `/* Add collation
  if not default for the type */`, after the `DEFAULT`/`GENERATED` and
  `NOT NULL` appends. Those two appends are where v18 differs: its
  `print_default` branch has a third arm for `ATTRIBUTE_GENERATED_VIRTUAL`
  (`" GENERATED ALWAYS AS (%s)"`, no `STORED`), and its `print_notnull` branch
  writes `" CONSTRAINT %s NOT NULL"` when `notnull_constrs[j]` is non-empty
  and appends `" NO INHERIT"` when `notnull_noinh[j]`. v17's is a bare
  `appendPQExpBufferStr(q, " NOT NULL")` with no branch at all.
- `dumpDomain`: `/* Print collation only if different from base type's
  collation */`, emitted directly after `CREATE DOMAIN %s AS %s`.
- `dumpCompositeType`: `/* Add collation if not default for the column type */`,
  emitted directly after each attribute's `%s %s`.

`typcollation` is `pg_type.dat`: `name` carries `typcollation => 'C'`, while
`text`, `bpchar` and `varchar` carry `typcollation => 'default'`.

**Observed, in committed bytes.** `scripts/fixture_schema_types.sql`'s
`public.t_collate` reaches all three emission sites, and
`fixtures/<13–18>/types/default.sql` is what each of the six servers wrote:

```
CREATE COLLATION public.c_collation (provider = libc, locale = 'C');
CREATE TYPE public.collated_pair AS (
	plain text,
	c text COLLATE pg_catalog."C"
);
CREATE DOMAIN public.text_c AS text COLLATE pg_catalog."C";
CREATE TABLE public.t_collate (
    id integer NOT NULL,
    v_text_c text COLLATE pg_catalog."C",
    v_text_locale text COLLATE pg_catalog."en_US.utf8",
    v_text_ucs text COLLATE pg_catalog.ucs_basic,
    v_name name,
    v_domain_c public.text_c,
    v_pair public.collated_pair,
    v_text_def text DEFAULT 'x'::text COLLATE pg_catalog."C",
    v_user text COLLATE public.c_collation,
    v_src text,
    v_gen_nn text GENERATED ALWAYS AS (upper(COALESCE(v_src, ''::text))) STORED NOT NULL COLLATE pg_catalog."C"
);
```

Four things are visible there and nowhere else in the tree. The reference is
schema-qualified and quoted **only where quoting is needed** — `pg_catalog."C"`
against `pg_catalog.ucs_basic` — and `v_user` shows the same rule for a
collation outside `pg_catalog`, unquoted because `c_collation` needs no
quoting. The two bare columns are exactly the two whose
collation is their type's own default: `name`'s is `C`, and `v_domain_c`'s is
the domain's own clause, which the `CREATE DOMAIN` carries instead. And the
composite's attribute clause sits directly after its type, as the domain's
does.

The displacement is in those bytes too, at all six majors. The fixture SQL
writes each clause directly after the type; `pg_dump` writes the two with a
constraint behind them at the end of the fragment. `v_text_def` shows one
displacer and `v_gen_nn` the whole v13-v17 stack — `GENERATED ALWAYS AS (expr)
STORED`, then `NOT NULL`, then the clause, with a nested call and a quoted
literal inside the expression. Both lines are byte-identical across 13.23
through 18.6.

**Scope limit.** There is a **fourth** `COLLATE %s` emission,
`createDummyViewAsClause`'s `NULL::<type> COLLATE <coll> AS <name>`, written
only for a view whose real definition is postponed by a circular dependency. It
is inside a `CREATE VIEW`, which is not one of the five statement shapes the
preamble grammar triggers on, so it never reaches a column definition — but a
count of `COLLATE %s` in `pg_dump.c` finds four, not three.

Two of the three v18-only shapes have no fixture: `CONSTRAINT <name> NOT
NULL`/`NO INHERIT` and a virtual `GENERATED` rest on the source reading above
alone. Neither can be fixtured until the generator can run version-conditional
schema SQL — it conditions dump *flag sets* on version, but one schema `.sql`
runs against every major, so an 18-only DDL shape fails on 13-17. The row is in
[`pg-dump-compatibility.md`](pg-dump-compatibility.md).

Beyond that, the entry says nothing about which collations *exist* on a server,
nor about what a named collation orders like: `pg_collation.collversion` is the
server's own notion of that, and no dump carries it (I32). `CREATE COLLATION`
itself `pg_dump` emits as an ordinary object — `fixtures/<13-18>/types/*.sql`
carry `public.c_collation`, and this entry says nothing about that statement's
own grammar.

**Verified against.** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 — all six
carry the same three comments and four `COLLATE %s` emissions, and in all six
the column emission follows the `GENERATED`/`DEFAULT`/`NOT NULL` appends. The
placement and every form above are observed in the committed fixtures at all
six; the two v18-only window shapes are read from v18.6's source only.

**Relied on by.** `crate::preamble`'s `extract_collation` (the placement and the
form) and `ColumnDef::collation`/`TypeKind::Domain::collation`; the comparison
register's collatable arms in `crate::pgtype`, which read a clause's absence as
the type's default.

**Re-verify.**

```sh
grep -n -B2 'COLLATE %s' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/pg_dump/pg_dump.c
grep -n -B32 'Add collation if not default for the type' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/pg_dump/pg_dump.c
grep -n -A6 "typname => 'name'" \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/include/catalog/pg_type.dat
psql -c "select typname, typcollation::regtype from pg_type
         where typname in ('text','varchar','bpchar','name')"
grep -n -A13 'CREATE TABLE public.t_collate' fixtures/*/types/default.sql
```

The last of those re-verifies the placement from committed bytes at the six
majors we generate. For a newly released major, before any fixture for it
exists, the probe below answers the same question and needs nothing but the
image:

```sh
sudo docker run -d --name i37 --memory 512m -e POSTGRES_HOST_AUTH_METHOD=trust \
  postgres:<N>-trixie
# pg_isready can answer from the image's transient init instance, so retry
# the first DDL-capable command rather than trusting it.
until sudo docker exec i37 createdb -U postgres probe; do sleep 1; done
sudo docker exec -i i37 psql -U postgres -d probe -v ON_ERROR_STOP=1 <<'SQL'
CREATE TABLE t (
    v_def  text COLLATE "C" DEFAULT 'x',
    v_nn   text COLLATE "C" NOT NULL,
    v_both character(10) COLLATE "C" DEFAULT 'x' NOT NULL,
    v_gen  text COLLATE "C" GENERATED ALWAYS AS (upper(v_nn)) STORED NOT NULL
);
SQL
sudo docker exec i37 pg_dump -U postgres probe | grep -A6 'CREATE TABLE public.t'
sudo docker rm -f i37
```

Every clause goes in directly after its type; each one that comes back at the
end of its fragment is the placement claim holding for that major, and whatever
sits between is the window this entry has to describe.

---

## I38 — `character(n)` values are written blank-padded, and PostgreSQL compares them with trailing blanks stripped

**Claim.** A `COPY` block writes every non-NULL value of a `character(n)`
column padded with spaces to exactly `n` characters, while PostgreSQL's
`bpchar` comparison operators first strip **all** trailing blanks from both
sides. So a bytewise comparison of the stored text against an unpadded literal
is not the server's answer, whatever the column's collation: a field whose
significant text equals the literal compares *greater* bytewise and *equal* on
the server.

**Proof.** `bpcharcmp` (and `bpchareq`, `bpcharlt`, …) in
`src/backend/utils/adt/varchar.c` call `bcTruelen` on both arguments before
`varstr_cmp`:

```c
len1 = bcTruelen(arg1);
len2 = bcTruelen(arg2);
cmp = varstr_cmp(VARDATA_ANY(arg1), len1, VARDATA_ANY(arg2), len2,
                 PG_GET_COLLATION());
```

`bcTruelen` is `bpchartruelen`, which walks back from the end while `s[i] ==
' '`. The padding itself is `bpchar_input`/`bpchar`'s `maxlen` branch, which
fills to the declared length. Trimming happens before the collation is consulted
at all, which is why a `COLLATE "C"` clause does not make the two agree.

**Observed.** `fixtures/16/types/default.sql`'s `public.t_text` writes
`v_char char(10)` as ten spaces for `''` and as `hi` followed by eight spaces
for `'hi'`.

**Scope limit.** `character varying(n)` and `text` are not padded and not
trimmed, so neither half applies to them. It is about the comparison operators;
`length()` and the output function have their own rules.

**Corollary: padding the literal is sound for `=` and unsound for `<`.** Padding
both sides to `n` and comparing bytewise agrees with trim-then-compare for
equality — padding to a fixed width is a bijection on the trailing-blank
equivalence classes — but not for ordering, because a byte below `0x20` sorts
under the pad space while the server, having stripped the pad, ranks the longer
string above the shorter. Trimming both sides is the canonicalization that holds
for both operators. Probed on 16.15:

```
select ('ab'||chr(9))::char(3) > 'ab'::char(3);                     -- t
select rpad('ab'||chr(9),3,' ') > rpad('ab',3,' ') COLLATE "C";     -- f
```

**Verified against.** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 — `bcTruelen`
is called by every `bpchar` comparison in all six.

**Relied on by.** The comparison register's `character` arm in `crate::pgtype`,
which is `CompareKind::PaddedText` — trailing `0x20` off both sides, then the
same collation question `text` asks, in that order because the corollary above
says the other order is unsound — see [`decisions.md`](decisions.md),
"D55". The corollary is checked across all six majors:
`fixtures/<13-18>/oracle/comparisons.tsv` asks `character(10)` against a
tab-bearing value under both collations.

**Re-verify.**

```sh
grep -n -A12 '^bpcharcmp' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/backend/utils/adt/varchar.c
grep -n -A6 'bpchartruelen(char' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/backend/utils/adt/varchar.c
grep -A4 'COPY public.t_text ' fixtures/16/types/default.sql | cat -A
```

---

## I39 — `oid` is unsigned in the file and in the order, and the server reads a signed literal by wrapping it

**Claim.** Three properties of PostgreSQL's `oid`, which is `unsigned int`
(`typedef unsigned int Oid`, `src/include/postgres_ext.h`):

- **In the file** — `oidout` is `snprintf(result, 12, "%u", o)`, so a `COPY`
  block writes an OID as unsigned decimal digits with no sign, whatever its
  value. `4294967295` is what the file holds, never `-1`.
- **In the order** — `oidlt`/`oidgt` are C's `<`/`>` on two `Oid`s, hence
  unsigned comparison, and `oid_cmp` is the same three-way. So the order over
  the whole range is the numeric order of the digits the file holds.
- **On input** — `oidin` accepts a leading minus and **wraps** it: `strtoul`
  is allowed to return a value that does not fit, and the branch commented
  *"For backwards compatibility, we want to accept inputs that are given with
  a minus sign"* keeps it when it matches after signed extension. `'-1'::oid`
  is 4294967295.

**Proof.** `src/backend/utils/adt/oid.c`:

```c
Oid			o = PG_GETARG_OID(0);
char	   *result = (char *) palloc(12);
snprintf(result, 12, "%u", o);
```

```c
oidgt(PG_FUNCTION_ARGS)
{
	Oid			arg1 = PG_GETARG_OID(0);
	Oid			arg2 = PG_GETARG_OID(1);
	PG_RETURN_BOOL(arg1 > arg2);
}
```

The input wrap is `oidin_subr`'s

```c
	if (cvt != (unsigned long) result &&
		cvt != (unsigned long) ((int) result))
```

which is in `oid.c` at v13–v15 and moved verbatim into `numutils.c`'s
`uint32in_subr` at v16, where `oidin` delegates. It is also in master.
The oracle records the behaviour rather than the source:
`fixtures/<13…18>/oracle/literals.tsv` carries `oid  -1  ok  4294967295` at all
six majors.

**Scope limit.** The v16 move changed the `strtoul` base from 10 to **0**, so
`0x2a` is accepted as an OID from v16 and rejected below it. That affects
which literals the *server* takes, not the file's spelling and not the order;
no oracle case asks for one.

**Verified against.** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 — all six
carry the `oidout` and `oidgt` bodies quoted above verbatim, and all six carry
the minus-sign branch (in `oid.c` up to v15, `numutils.c` from v16).

**Relied on by.** `oid`'s `UInt32` mapping and its `CompareKind::UnsignedInt`
comparison in `pgtype.rs` — [`decisions.md`](decisions.md), "Type resolution
and decoders" and "D55". The third property is why a signed filter literal is
refused there rather than compared (`decisions.md`, "D55"): this build does
not implement the wrap.

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
grep -n 'snprintf(result, 12, "%u", o)' src/backend/utils/adt/oid.c
awk '/^oidgt\(PG_FUNCTION_ARGS\)/,/^}/' src/backend/utils/adt/oid.c
grep -rn 'cvt != (unsigned long) ((int) result)' src/backend/utils/adt/
```

and, from the repo root, the behaviour the source predicts:

```sh
grep -P '^oid\t-1\t' fixtures/*/oracle/literals.tsv
```

---

## I40 — `interval`, `timetz` and the network types each have one output form and a comparison that is not byte order

**Claim.** Four properties, one per family, and each is two halves: the text a
dump can hold, and the order PostgreSQL puts two such values in.

- **`interval`.** Written by `EncodeInterval` under `INTSTYLE_POSTGRES`, which
  `pg_dump` pins (I4): an optional `<n> year[s]`, then `<n> mon[s]`, then
  `<n> day[s]`, then an optional `[+|-]HH:MM:SS[.ffffff]` tail, single-space
  separated, with a wholly-zero interval written `00:00:00`. A part is
  suppressed when its value is zero; the unit takes an `s` whenever the value
  is not exactly `1`, so `-1 days` is what a minus-one-day interval is written
  as; and a part that is positive and *follows* a negative one carries a `+`.
  The hour field is at least two digits and unbounded above; minutes and
  seconds are exactly two. Unbounded above is literal: nothing normalizes
  hours into days, so the tail of `interval '100000000 hours'` is written
  `100000000:00:00`, and the field's true ceiling is the `Interval` struct's
  own — `time` is `int64` *microseconds*, giving `2562047:47:16.854775807`. The order is `interval_cmp_value`: months collapse
  to 30 days, days to 86400 seconds, and the result is a **128-bit**
  microsecond span. So `1 mon`, `30 days` and `720:00:00` are one value, and
  the collapse is what a text comparison cannot approximate.
- **`time with time zone`.** Written as `HH:MM:SS[.ffffff]` followed by
  `EncodeTimezone`'s `±HH[:MM[:SS]]` — minutes appended only when nonzero,
  seconds only when nonzero. The stored `zone` is seconds **west** of GMT,
  which is the negation of the sign displayed. `timetz_cmp_internal` sorts by
  `time + zone * 1e6` — the UTC-equivalent instant — and breaks a tie on
  `zone`, so two spellings of one instant are ordered rather than equal.
- **`inet` and `cidr`.** Written by `pg_inet_net_ntop`, with `cidr_out`
  appending `/bits` when the address form omitted it and `inet_out` not; so an
  `inet` at its family's full width has no `/bits` and a `cidr` always does.
  `network_cmp_internal` is family first (`PGSQL_AF_INET6` is
  `PGSQL_AF_INET + 1`, so every IPv4 address sorts below every IPv6 one), then
  `bitncmp` over the **shorter** of the two netmasks, then the netmask lengths,
  then `bitncmp` over the family's full width. The middle step is what makes
  this not a byte order: `10.1.0.0/8` sorts *below* `10.0.0.0/16`, because
  their first eight bits agree and `8 < 16`.
- **`macaddr` and `macaddr8`.** Written as six (resp. eight) lowercase hex
  pairs joined by `:`. `macaddr_cmp_internal` and `macaddr8_cmp_internal`
  compare the high half then the low half of the octets, which is the plain
  byte order of the address.

**Proof.** `src/backend/utils/adt/datetime.c`'s `EncodeInterval`
(`case INTSTYLE_POSTGRES:`) and its `AddPostgresIntPart` helper, whose
`sprintf` format is `"%s%s%lld %s%s"` — the leading space, the `+` under
`(*is_before && value > 0)`, the value, the unit, and the `s` under
`(value != 1)`. `EncodeTimezone` in the same file is the offset form.
`src/backend/utils/adt/timestamp.c`:

```c
	days = interval->month * INT64CONST(30);
	days += interval->day;
	span = int64_to_int128(interval->time);
	int128_add_int64_mul_int64(&span, days, USECS_PER_DAY);
```

v13's `interval_cmp_value` splits the time field into whole days and a
remainder before doing the same sum, which is the identical value. `date.c`'s
`timetz_cmp_internal` is the `t1 = time1->time + (time1->zone * USECS_PER_SEC)`
comparison with the zone tiebreak, under the comment *"we only want to say
that two timetz's are equal if both the time and zone parts are equal"*.
`network.c` carries `network_cmp_internal`, `bitncmp` and `network_out`;
`mac.c`/`mac8.c` carry the two `*_cmp_internal`.

The behavioural half is committed:
`fixtures/<13–18>/oracle/comparisons.tsv` holds the server's own answer for
every pair of each family's case list — 492 ordered pairs across the six
majors — and `fixtures/<13–18>/oracle/literals.tsv` holds each value's output
spelling beside the input that produced it (`interval  1.5 hours  ok
01:30:00`, `inet  192.168.1.1  ok  192.168.1.1`, `macaddr  08-00-2b-01-02-04
ok  08:00:2b:01:02:04`).

**Scope limit.** The *output* forms only. Each type's `*_in` accepts a far
wider grammar — `1.5 hours` and `P1Y2M` for an interval, an abbreviated
`10` for an IPv4 address, four separator conventions for a MAC — and this
entry says nothing about those beyond that they exist; `pgtype.rs`'s register
reads the output form alone, and refuses the rest. Says nothing about
`interval`'s infinities, which are I34's.

**Verified against.** v13.23, v14.24, v15.19, v16.15, v17.11 and v18.6 —
`EncodeInterval`'s `INTSTYLE_POSTGRES` arm, `EncodeTimezone`,
`timetz_cmp_internal`, `network_cmp_internal` and `bitncmp` are byte-identical
across all six but for v13's `interval_cmp_value`, noted above, and v17's
addition of the `INTERVAL_NOT_FINITE` branch ahead of `EncodeInterval`.
`AddPostgresIntPart` differs only in the width of its `value` argument and the
matching `printf` conversion — `int`/`%d` at v13–v14, `int64`/`%lld` at
v15–v17, `int64`/`PRId64` at v18 — which changes no output for any value the
type can hold.

**Relied on by.** `pgtype.rs`'s `CompareKind::Interval`, `TimeTz`, `Network`
and `MacAddr` arms and `predicate.rs`'s parsers for them —
[`decisions.md`](decisions.md), "D55",
where these are four *Agrees* rows. The `macaddr` output form is also what
lets its one set of stored bounds serve Arrow's semantics, which compares the
column as text — `ComparisonPlan::bounds_kinds` and
[`decisions.md`](decisions.md), "D79". The hour field's ceiling is also what keeps
`interval` a `Utf8View` — Arrow's `Interval(MonthDayNano)` holds nanoseconds in
the same `int64`, a thousandth of the span — [`decisions.md`](decisions.md),
"D37".

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
awk '/^AddPostgresIntPart/,/^}/' src/backend/utils/adt/datetime.c
awk '/^EncodeTimezone/,/^}/' src/backend/utils/adt/datetime.c
awk '/^interval_cmp_value/,/^}/' src/backend/utils/adt/timestamp.c
awk '/^timetz_cmp_internal/,/^}/' src/backend/utils/adt/date.c
awk '/^network_cmp_internal/,/^}/' src/backend/utils/adt/network.c
awk '/^bitncmp/,/^}/' src/backend/utils/adt/network.c
awk '/^macaddr_cmp_internal/,/^}/' src/backend/utils/adt/mac.c
```

and, from the repo root, the answers the source predicts:

```sh
grep -P '^interval\t(1 mon|30 days|720:00:00)\t' fixtures/17/oracle/comparisons.tsv
grep -P '^(inet|cidr|macaddr|macaddr8|time with time zone)\t' fixtures/16/oracle/literals.tsv
```

---

## I41 — `jsonb` has one output form and a structural order, and a top-level scalar is stored inside a one-element array

**Claim.** Four properties: the text a dump can hold, the text the server will
read back, the order two documents are put in, and the one part of that order
that is not a fact about the documents.

- **Output.** `jsonb_out` walks the *stored* value, so it is canonical and not
  the text that was inserted. `,` and `:` are each followed by exactly one
  space and nothing else is; a number goes through `numeric_out`, which never
  writes an exponent; a string goes through `escape_json`, which writes `\b`,
  `\f`, `\n`, `\r`, `\t`, `\"` and `\\` and renders every other byte below
  `0x20` as `\u00xx`, leaving everything else — non-ASCII included — as itself;
  and an object's pairs are written in **storage order**, which is
  `lengthCompareJsonbString`: key length first, then `memcmp`. So
  `{"z":1,"aa":2}` comes back `{"z": 1, "aa": 2}`, not alphabetized.

- **Input.** `jsonb_in` is RFC 8259 with two refusals of its own. A JSON number
  is `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` and is stored as a
  `numeric`, so `1`, `1.0` and `1e2` are accepted while `01`, `+1`, `.5`, `1.`
  and `NaN` are not. A string may not carry an unescaped byte below `0x20`;
  `\u0000` is refused outright, because a `jsonb` string is `text` and `text`
  cannot hold a NUL; and a `\uD800`-range escape must be followed by its low
  half. Duplicate object keys are accepted and resolved to the **last** one
  written.

- **Order.** `compareJsonbContainers` walks two documents in lockstep and
  decides at the first position they differ at. A position where the two kinds
  differ is decided by the kind alone, in `JsonbValue.type`'s own numbering —
  `jbvNull` `0x0`, `jbvString` `0x1`, `jbvNumeric` `0x2`, `jbvBool` `0x3`,
  `jbvArray` `0x10`, `jbvObject` `0x11`. A position where both are containers
  of one kind is decided by the element or pair **count** before any member is
  looked at. Otherwise: two numbers by `numeric_cmp`, two booleans by
  `false < true`, two nulls equal, and two strings by `varstr_cmp`. An object's
  pairs are visited in storage order while its keys are compared as strings,
  and those are two different orders.

- **The raw-scalar wrapper.** A top-level scalar is stored as a one-element
  array with `rawScalar` set, and `compareJsonbContainers` tests that flag
  *and then lets the element count overwrite what it concluded*. So a scalar
  sorts below a one-element or longer array and **above an empty one**:
  `'1'::jsonb > '[]'::jsonb` is true, and so is `'null'::jsonb > '[]'::jsonb`.
  v18's source carries a comment on it — *"There should be an "else" here, to
  prevent us from overriding the above, but we can't change the sort order now,
  so there is a mild anomaly that an empty top level array sorts less than
  null."* — so upstream holds the behaviour frozen.

**Proof.** `src/backend/utils/adt/jsonb_util.c` carries
`compareJsonbContainers`, `compareJsonbScalarValue`,
`lengthCompareJsonbString`, `lengthCompareJsonbPair` and
`uniqueifyJsonbObject`; `src/include/utils/jsonb.h` carries the `JsonbValue`
type numbering quoted above. The kind-order fallback is one line, reached from
both branches of the walk:

```c
			/* Type-defined order */
			res = (va.type > vb.type) ? 1 : -1;
```

The last-duplicate-wins rule is two facts together: `lengthCompareJsonbPair`
breaks a key tie on *descending* insertion order (`res = (pa->order >
pb->order) ? -1 : 1`, under the comment *"Unique algorithm will prefer first
element as value"*), and `uniqueifyJsonbObject` then keeps the first of each
run. `src/backend/utils/adt/jsonb.c`'s `JsonbToCStringWorker` is the output
walk — `numeric_out` for a number, `escape_json` for a string —
`src/backend/utils/adt/json.c` carries `escape_json`, and
`src/common/jsonapi.c` carries `json_lex_number` and `json_lex_string`.

**Observed**, on `postgres:13.23-trixie`, `postgres:16.15-trixie` and
`postgres:18.6-trixie`, all three answering identically: `'1' > '[]'`,
`'null' > '[]'`, `'1' < '[1]'`, `'1' < '[1,2]'`, `'1' < '{}'`, `'[]' < '{}'`,
`'"a"' < '1'`, `'true' > '1'`, `'null' < '"a"'`, `'[1,2]' > '[3]'`,
`'{"b":1}' < '{"a":1,"c":2}'`, `'{"z":1}' > '{"a":2}'` and
`'{"z":1,"aa":2}' > '{"y":3,"zz":4}'` all true — the last being the pair that
separates storage order from alphabetical, since sorted the other way it
answers false. Output and input on the same containers: `'{"z":1,"aa":2}'`
prints `{"z": 1, "aa": 2}`, `'{"a":1,"a":2}'` prints `{"a": 2}`, `'1e2'` prints
`100`, `'-0.0'` prints `0.0`, and `01`, `+1`, `.5`, `1.`, `[1,]`, `{a:1}`,
`NaN`, `1 2`, `"\x41"`, `"\ud83d"`, `"\u0000"` and a string holding a raw
newline are each refused.

The behavioural half is committed and reaches every branch of the order.
`fixtures/<13–18>/oracle/comparisons.tsv` holds the server's own answer for all
225 ordered pairs of the `jsonb` case list — 8,316 cells across the six majors
— over both booleans, a string, a number, JSON `null`, both empty containers,
an array, five one-pair objects and two two-pair ones. That is where the kind
numbering, `false < true`, a container's size deciding before any member,
storage order differing from alphabetical, and the raw-scalar anomaly
(`'1' > '[]'`, `'null' > '[]'`, `'1' < '[1, 2]'`) are each pinned.
`literals.tsv` carries the output and input half over the same values:
`{"z": 1, "aa": 2}` is its own `output`, unsorted; `{"a":1,"a":2}` prints
`{"a": 2}`; `1e2` prints `100` and `-0.0` prints `0.0`; `{` and `01` are
refused. The four ordering operators of every one of those cells are asserted
against this build's comparison ([`decisions.md`](decisions.md), "D71").

**Scope limit.** The *output* form and the input grammar, not the wider
question of what a `json` value looks like: `json` stores its input verbatim
and normalizes nothing, so none of the output half applies to it, and
PostgreSQL defines no comparison for `json` at all. Says nothing about the
containment and existence operators (`@>`, `?`), which are not an order.
`varstr_cmp` is called with `DEFAULT_COLLATION_OID`, so every string leaf and
object key is ordered by the **database's** collation — which a plain dump does
not record (I32), and which is why this entry cannot support a claim of
agreement for a document with a string anywhere in it.

**Verified against.** v13.23, v14.24, v15.19, v16.15, v17.11 and v18.6.
`compareJsonbContainers` is byte-identical across all six but for v18's added
comment, quoted above, which changes no code. `compareJsonbScalarValue` differs
only in its two parameter names (`aScalar`/`bScalar` → `a`/`b`, at v16).
`uniqueifyJsonbObject` gained `unique_keys` and `skip_nulls` parameters at v16
for the SQL/JSON object constructors; `jsonb_in` passes neither, so the sort
and the duplicate rule are unchanged. `escape_json`'s body moved into an
`escape_json_char` helper at v18 with every arm identical.
`lengthCompareJsonbString` and `lengthCompareJsonbPair` are byte-identical
across all six. `json_lex_number` gained the incremental-parser branch at v17,
which `jsonb_in` does not enter.

**Relied on by.** `pgtype.rs`'s `CompareKind::Jsonb` arm and `predicate.rs`'s
`Jsonb`, `JsonCursor` and `storage_order` —
[`decisions.md`](decisions.md), "D55", where
this is the row that agrees about structure and diverges at a string.

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
awk '/^compareJsonbContainers/,/^}/' src/backend/utils/adt/jsonb_util.c
awk '/^compareJsonbScalarValue/,/^}/' src/backend/utils/adt/jsonb_util.c
awk '/^lengthCompareJsonbPair/,/^}/' src/backend/utils/adt/jsonb_util.c
awk '/^uniqueifyJsonbObject/,/^}/' src/backend/utils/adt/jsonb_util.c
sed -n '/typedef enum jbvType/,/} jbvType;/p' src/include/utils/jsonb.h
```

and the behaviour, against a throwaway container of the major in question —
the only way to reach the raw-scalar anomaly, which no `fixtures/` case
constructs. The SQL arrives on stdin rather than inside the `sh -c` string:

```sh
sudo nerdctl run --rm -i -m 512m -e POSTGRES_HOST_AUTH_METHOD=trust \
  postgres:<N>-trixie sh -c 'docker-entrypoint.sh postgres >/tmp/pg.log 2>&1 &
    for i in $(seq 60); do pg_isready -q && break; sleep 1; done
    cat > /tmp/q.sql; psql -U postgres -X -q -f /tmp/q.sql' <<'SQL'
SELECT '1'::jsonb > '[]'::jsonb    AS scalar_gt_empty_array,
       '1'::jsonb < '[1]'::jsonb   AS scalar_lt_one_elem_array,
       'true'::jsonb > '1'::jsonb  AS bool_gt_number,
       '"a"'::jsonb < '1'::jsonb   AS string_lt_number,
       '{"z":1,"aa":2}'::jsonb     AS storage_order_out;
SQL
```

and, from the repo root, the committed cells the source predicts:

```sh
grep -P '^jsonb\t' fixtures/16/oracle/comparisons.tsv
grep -P '^jsonb\t' fixtures/16/oracle/literals.tsv
```

---

## I42 — A plain dump carries a user-defined collation's provider, locale and determinism, and never its version

**Claim.** `pg_dump` writes a user-defined collation as `CREATE COLLATION
<name> (provider = …[, deterministic = false][, locale = …| , lc_collate = …,
lc_ctype = …][, rules = …]);`. Three parts of that are load-bearing:

- **`deterministic = false` is emitted unconditionally** wherever the collation
  is non-deterministic — it is not gated on any dump option — so an ordinary
  plain dump *does* state that `texteq`/`bpchareq` on a column of that
  collation is not a byte comparison.
- **`version =` is emitted only under `--binary-upgrade`.** An ordinary plain
  dump carries a collation's *name and definition* and never the
  `pg_collation.collversion` the server computed for it, which is the libc or
  ICU version verbatim.
- **A non-deterministic collation is always ICU.** The server refuses
  `deterministic = false` for any other provider, so this shape never appears
  with `provider = libc`.

Together those say what a dump can and cannot settle about a stated collation:
it settles determinism, and therefore whether `=` is bytewise; it does not
settle the *order*, because that is what the provider version supplies.
Collations `initdb` created in `pg_catalog` — `en_US.utf8`, `ucs_basic`,
`und-x-icu` — are not dumped at all, so for those the file carries only the
name a `COLLATE` clause spells (I37).

**Proof.** `dumpCollation` in `pg_dump.c`. The determinism append sits before
every provider branch and is guarded only on the catalog value:

```c
if (strcmp(PQgetvalue(res, 0, i_collisdeterministic), "f") == 0)
    appendPQExpBufferStr(q, ", deterministic = false");
```

while the version append is inside `if (dopt->binary_upgrade)`, under the
comment *"For binary upgrade, carry over the collation version. For normal
dump/restore, omit the version, so that it is computed upon restore."* The
provider restriction is the backend's, in `collationcmds.c`'s
`DefineCollation`:

```c
if (!collisdeterministic && collprovider != COLLPROVIDER_ICU)
    ereport(ERROR,
            (errcode(ERRCODE_FEATURE_NOT_SUPPORTED),
             errmsg("nondeterministic collations not supported with this provider")));
```

Verified in the v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 and master
worktrees. What *does* differ across them is only how the ICU locale is read —
v15+ takes `colliculocale`, earlier majors take `collcollate` — and both emit
the same `, locale = '…'` text.

**Scope limit.** This is about a **user-defined** collation, which is the only
kind `pg_dump` emits a definition for. It claims nothing *for* the
`--binary-upgrade` output beyond the version being there — that flag set is
where the version append is observed, below — and nothing about the database's
own collation, which needs `--create` (I32).

**Observed, at all three claims.** `fixtures/<13-18>/types/` carries two
user-defined collations, and between them they pin every part of the shape:

```
CREATE COLLATION public.c_collation (provider = libc, locale = 'C');
CREATE COLLATION public.nd_collation (provider = icu, deterministic = false, locale = 'und');
```

Both lines are byte-identical at every major, option order included — provider,
determinism, locale, which is `dumpCollation`'s own append order. The first has
no determinism clause, which is the unconditional emission's other side: the
absence is the catalog's `true`. The second is the emission itself, and it is
`provider = icu` because the server refuses `deterministic = false` under any
other provider. `tests/preamble.rs`'s
`the_types_schema_declares_one_deterministic_and_one_non_deterministic_collation`
reads both back.

**`version =` is observed by its absence and by its one presence.** Neither
line above carries one, in `default.sql` or in `data-only.sql`; the same
`nd_collation` in `fixtures/<13-18>/types/binary-upgrade.sql` reads `…, locale
= 'und', version = '153.128');`. That is the only ICU release number in the
tree and it moves when the base images move, which is the drift that keeps ICU
out of the comparison columns and out of the oracle. Both halves of this
paragraph are asserted by `tests/preamble.rs`'s
`the_icu_collversion_reaches_binary_upgrade_alone_and_agrees_across_majors`:
the flag-set claim exactly as written, and the version itself by six-way
agreement without naming the value, so an image bump is a regeneration and a
*split* between majors is a fault ([`decisions.md`](decisions.md),
"D69" and "D70").

**Consequence.** Equality's divergence is knowable per column wherever the
collation is user-defined and the dump says `deterministic = false`, and
unknowable only where the column carries no clause and the database default is
absent (I32). For ordering, the clause names the collation but not the order,
so a comparison per named collation would agree with *a* server rather than
with the server — which keeps `KD7`'s fix conditional on a provider version
rather than absolute.

**Relied on by.** The comparison register's `NonDeterministicCollation` verdict
and `DatabaseMetadata::collations`, the `CREATE COLLATION` parse that feeds it
([`decisions.md`](decisions.md), "D36" and "D55") — the determinism half is
the verdict itself, the version half why it is a divergence rather than a
comparison. Also `KD7`'s claim about what its fix can close, and
`oracle_register.py`'s exemption for the arm, which rests on a
non-deterministic collation being ICU-only.

**Re-verify.**

```sh
grep -n -A3 'collisdeterministic' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/pg_dump/pg_dump.c
grep -n -B4 -A6 'carry over the collation version' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/pg_dump/pg_dump.c
grep -n -B2 -A4 'nondeterministic collations not supported' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/backend/commands/collationcmds.c

# And against the committed bytes: two lines per major with no version, one
# with it, and the same option order everywhere.
grep -h 'CREATE COLLATION' fixtures/*/types/default.sql | sort | uniq -c
grep -h 'CREATE COLLATION public.nd_collation' fixtures/*/types/binary-upgrade.sql \
  | sort | uniq -c
cargo test -p pgdump_query --test preamble the_icu_collversion
```

---

## I43 — Three collations order by `memcmp` on every server, whatever the provider version

**Claim.** `varstr_cmp` — the one function all text ordering funnels through —
short-circuits to `memcmp` (then shorter-first) whenever the collation's
`collate_is_c` flag is set, without consulting libc or ICU at all. Three
collation shapes set it, and their ordering is therefore **independent of the
server's libc version, ICU version, platform and major**:

- **The libc provider with `collcollate` exactly `C` or `POSIX`.** Not
  `C.UTF-8`, which is a locale name like any other and goes through
  `strcoll_l`.
- **The builtin provider, unconditionally** (PG 17+). `C`, `C.UTF-8` and
  `PG_UNICODE_FAST` (PG 18) differ from each other only in *ctype*; all three
  order by code point.
- **`pg_catalog."ucs_basic"`**, in every supported major — by two different
  routes across the range, per the table below.

This is the **forward** direction only: these order bytewise. It makes no claim
that nothing else does — glibc's `C.UTF-8` happens to sort by code point too,
and that is a fact about one libc, not a guarantee (see the scope limit).

**Proof.** The short-circuit is `varstr_cmp` in
`src/backend/utils/adt/varlena.c`:

```c
if (mylocale->collate_is_c)
{
    result = memcmp(arg1, arg2, Min(len1, len2));
    if ((result == 0) && (len1 != len2))
        result = (len1 < len2) ? -1 : 1;
}
```

The flag is set in one place per provider. In v18 it is
`src/backend/utils/adt/pg_locale_libc.c`'s `create_pg_locale_libc`:

```c
result->collate_is_c = (strcmp(collate, "C") == 0) ||
    (strcmp(collate, "POSIX") == 0);
```

and `pg_locale_builtin.c`'s `create_pg_locale_builtin`, which does not test the
locale at all:

```c
result->provider = COLLPROVIDER_BUILTIN;
result->deterministic = true;
result->collate_is_c = true;
result->ctype_is_c = (strcmp(locstr, "C") == 0);
```

Through v17 both live in one `pg_locale.c`, in `lookup_collation_cache`, with
the same tests: a `COLLPROVIDER_BUILTIN` branch setting `collate_is_c = true`
(v17 only — the provider does not exist before it), a `COLLPROVIDER_LIBC`
branch testing `C`/`POSIX`, and an `else` setting it false. v13 and v14 do not
branch on provider at all and apply the `C`/`POSIX` test to `collcollate`
directly.

`ucs_basic` is created two ways across the range, and both are `C`:

| majors | where | how |
|---|---|---|
| 13, 14, 15 | `src/bin/initdb/initdb.c` | an `INSERT` into `pg_collation` with `'C', 'C'` as `collcollate`/`collctype` |
| 16 | `src/include/catalog/pg_collation.dat` | `collprovider => 'c', collcollate => 'C', collctype => 'C'` |
| 17, 18 | `src/include/catalog/pg_collation.dat` | `collprovider => 'b', colllocale => 'C'` |

Verified in the v13.23, v14.24, v15.19, v16.15, v17.11 and v18.6 worktrees.

**Scope limit.** Ordering only. `ctype_is_c` is a separate flag and the builtin
provider's three locales differ precisely there, so nothing here licenses a
claim about `upper`/`lower`/`initcap` or pattern matching. It says nothing
about *equality* either, which for a deterministic collation never consults the
collation at all and for a non-deterministic one does (I42) — and the builtin
and libc providers are always deterministic, so no shape named here is
affected. And it is not a claim that a collation absent from this list is
non-bytewise: glibc 2.41's `C.UTF-8` orders by code point in fact, which pgdt
must not act on, because it is a property of that libc and the file names only
the collation.

**Consequence.** These are the collations pgdt can order **without knowing
anything about the server that wrote the dump** — no libc version, no ICU
version, no platform. That is what the comparison register's *Agrees, on every
server* verdict means and the only thing that earns it; every other collation's
order is a function of the source server's provider version, which no plain
dump carries (I32, I42). The register reaches the verdict from the collation's
**name** rather than from this invariant, and so under-claims it
([`decisions.md`](decisions.md), "D55").

**Relied on by.** The comparison register's *Agrees* verdict for `COLLATE "C"`,
`COLLATE "POSIX"` and a bare `name` column
([`decisions.md`](decisions.md), "D55"), and
I37's "a bare `name` column is bytewise on every server", which asserts this
without proving it. The roadmap's Future item "Collation-aware comparison"
names this set as the half that needs no environment.

**Re-verify.**

```sh
grep -n -A6 'collate_is_c' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/backend/utils/adt/varlena.c
# v18+: two files; v17 and earlier: pg_locale.c's lookup_collation_cache
grep -rn 'collate_is_c = ' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/backend/utils/adt/
grep -rn -A2 'ucs_basic' \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/include/catalog/pg_collation.dat \
  /mnt/wd12t/upstream/postgres/release-v<N>/src/bin/initdb/initdb.c
```

---

## I44 — The four container `*_in` grammars are supersets of their `*_out` forms, and they disagree with each other

**Claim.** For each container form, the input function accepts strictly more
than the matching output function writes (I20's scope limit), and the four
supersets are **not one grammar wearing four hats**. They disagree on
whitespace, on where quoting and escapes may appear, on how SQL NULL is
spelled, and on whether arity is checked at all:

| Form | Whitespace around a part | Escapes / quotes outside where `*_out` puts them | SQL NULL | Arity |
|---|---|---|---|---|
| `array_in` | **dropped** — leading and trailing, per element | `\` anywhere, `"` around all of an element or none of it | bare `NULL`, matched case-insensitively, only where the element carries no quote and no escape | not checked; dimensionality is deduced from the braces or declared |
| `record_in` | **kept, byte for byte** | `\` anywhere, `"` may open and close mid-field | nothing at all between separators; `""` is the empty string | **checked** — exactly the composite's declared field count |
| `range_in` | **kept, byte for byte** inside a bound; skipped outside the brackets | the same as `record_in` | not applicable — a bound is never NULL, and nothing at all is *unbounded* | fixed at two bounds |
| `multirange_in` | dropped between members; a member is a substring, so its own blanks survive | delegated — the member text goes to `range_in` unchanged | not applicable | any number of members |

Four consequences:

- **`( 1 , a )` and `{ 1 , a }` do not mean the same thing.** The composite
  keeps both fields' blanks and force-quotes them back on output as
  `(1," a ")` — the `1` loses its blanks to `int4in`, not to `record_in` — while
  the array canonicalizes to `{1,2}`. A parser written once against the array
  rules and reused for the other two eats a composite field's blanks and
  matches nothing.
- **Whitespace is the same six characters in all four**: space, `\t`, `\n`,
  `\r`, `\v`, `\f`. `array_isspace` (through v16), `scanner_isspace` (v17+) and
  the C locale's `isspace` (record, range, multirange) agree exactly, so one
  predicate serves the family.
- **An array's dimension decoration takes forms `array_out` never writes**:
  `[n]` as well as `[m:n]`, a sign on either bound, whitespace between items
  and after the `=`, and an all-1 decoration that the output function drops
  (`[1:2]={1,2}` is `{1,2}`). A dimension with `ub < lb` is `2202E`
  (`array_subscript_error`), not `22P02`.
- **`empty` is case-insensitive**, may carry whitespace on either side, and is
  accepted as a `multirange_in` member — where it is silently dropped, though
  `multirange_out` never writes one.

The one place two supported majors disagree is `array_in`, additively: 13–16
parse with `ArrayCount` + `ReadArrayStr` and reject a right brace terminating
an **empty sub-array**, while 17–18's single-pass `ReadArrayStr` +
`ReadArrayToken` accepts it — `{{},{}}` is `22P02` on 13–16 and the empty array
`{}` on 17–18. Every other difference is presentational (which
`errdetail` is produced). `record_in`, `range_parse`/`range_parse_bound` and
`multirange_in` are character-for-character unchanged across the range, apart
from the soft-error (`escontext`) plumbing.

**What the grammar cannot see.** Two server refusals are not grammar refusals
and no parser can reproduce them without the subtype's own order:
`int4range '[10,1)'` is `22000` from `range_serialize`'s bound comparison, and
`multirange_in` sorts, coalesces and empty-drops its members
(`{[5,6),[1,2)}` → `{[1,2),[5,6)}`, `{[1,3),[2,5)}` → `{[1,5)}`, `{[1,1)}` →
`{}`). A discrete range also canonicalizes its bounds — `int4range '[1,10]'` is
`[1,11)` — through the subtype's successor function. All three are I46, and
`predicate.rs` reproduces them over the decoded bound *keys* after this
grammar has done what it can.

**Proof.** `array_in`/`ArrayCount`/`ReadArrayStr` in
`src/backend/utils/adt/arrayfuncs.c` through v16, and
`array_in`/`ReadArrayDimensions`/`ReadDimensionInt`/`ReadArrayStr`/`ReadArrayToken`
in the same file from v17; `record_in` in `rowtypes.c`; `range_parse` and
`range_parse_bound` in `rangetypes.c`; `multirange_in` in
`multirangetypes.c`. The whitespace claim is `array_isspace` in `arrayfuncs.c`,
`scanner_isspace` in `src/backend/parser/scansup.c`, and the C-locale `isspace`
the other three call.

**Observed.** Every row above answers as stated on `postgres:16.15-trixie` and
`postgres:18.6-trixie`, through the probe below. The committed evidence is the
acceptance walk in
`pgdump_query/tests/nested.rs` (`oracle::the_input_grammars_accept_exactly_what_the_server_accepted`),
which asserts every nested row of `fixtures/<13-18>/oracle/literals.tsv`
against the parsers — 397 literals over six majors.

**Scope limit.** The *container* grammar only. What each element, field or
bound means is the element type's own `*_in`, which is wider than its `*_out`
in its own ways and is not covered here — so a literal this invariant says is
well-formed can still be refused by the type underneath it, and an element's
spelling can still differ from what `*_out` would write for the same value.

**Verified against:** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 (source);
observed on 16.15 and 18.6.

**Relied on by:** [`decisions.md`](decisions.md), "D45" — `parse_array`/`parse_record`/`parse_range`/`parse_multirange`, which
implement the **newest** grammar unconditionally, per the union rule.

**Re-verify.** Read the functions:

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
grep -n -A40 '^ReadArrayToken' src/backend/utils/adt/arrayfuncs.c   # v17+
grep -n -A60 '^ArrayCount' src/backend/utils/adt/arrayfuncs.c       # through v16
grep -n -A30 'Check for null: completely empty input' src/backend/utils/adt/rowtypes.c
grep -n -A40 '^range_parse_bound' src/backend/utils/adt/rangetypes.c
grep -n -A40 'MULTIRANGE_BEFORE_RANGE' src/backend/utils/adt/multirangetypes.c
```

And ask a server, which is a minute per major:

```sh
docker run -d --rm --name pgdt-i44 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:<N>-trixie
docker exec -i pgdt-i44 psql -qtA -U postgres <<'SQL'
CREATE TYPE point2d AS (x integer, y text);
CREATE FUNCTION probe(typ text, lit text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE out text;
BEGIN
  EXECUTE format('SELECT textin(%s($1::%s))',
                 (SELECT typoutput::text FROM pg_type WHERE oid = typ::regtype), typ)
    INTO out USING lit;
  RETURN coalesce(out, '<null>');
EXCEPTION WHEN others THEN RETURN 'E' || SQLSTATE;
END $$;
SELECT t.typ, t.lit, probe(t.typ, t.lit) FROM (VALUES
  ('integer[]', '{{},{}}'), ('integer[]', '[2]={1,2}'), ('integer[]', '[1:2] = {1,2}'),
  ('text[]', '{ a , b }'), ('text[]', '{"a"b}'), ('text[]', '{N\ULL}'),
  ('point2d', '( 1 , a )'), ('point2d', '()'), ('point2d', '(1,a,b)'),
  ('int4range', '[ 1 , 10 )'), ('int4range', 'EMPTY'), ('int4range', '[10,1)'),
  ('int4multirange', '{empty}'), ('int4multirange', '{[5,6),[1,2)}')
) AS t(typ, lit);
SQL
docker rm -f pgdt-i44
```

Confirm `{{},{}}` is the only cell that moves between 16 and 18, and that
`( 1 , a )` still keeps its blanks where `{ a , b }` drops them.

---

## I45 — A container's order is its parts' order, with one NULL rule; an array reaches its shape only after its elements

**Claim.** Three statements, all of them about `array_cmp` and `record_cmp`:

- **The NULL rule is one rule, at every position and in both functions.** Two
  NULLs are equal; a NULL is **greater** than a not-NULL. So a container
  comparison is two-valued throughout — it never yields SQL `UNKNOWN`, and only
  the whole field being NULL makes a term unknown.
- **An array compares its elements before anything about its shape**, up to the
  *shorter* array's element count, and falls back — only when those agree — to
  element count, then dimension count, then the dimensions, then the lower
  bounds, in that order. `array_eq` goes the other way round, `memcmp`ing
  dimensions and lower bounds before it looks at an element; the two agree on
  *equality* and only `array_cmp` defines an order, so the ordering rule is
  `array_cmp`'s.
- **A composite compares field-wise in declaration order**, which is also the
  order `record_out` writes them in (I23), so a positional walk over the
  literal is the server's comparison and no field name is consulted.

A container inherits comparability from its parts, and a missing part is an
error rather than a fallback: both functions look up the element or field
type's `cmp_proc_finfo` and `ereport(ERROR, ERRCODE_UNDEFINED_FUNCTION)` when
there is none — so a `json[]` column, and a composite with a `json` field, have
no `<` and no `=` on the server either.

**Proof.** `array_cmp` in `src/backend/utils/adt/arrayfuncs.c` (v18.6, and
unchanged in shape since v13):

```c
    /* We consider two NULLs equal; NULL > not-NULL. */
    if (isnull1 && isnull2)
        continue;
    if (isnull1) { result = 1; break; }
    if (isnull2) { result = -1; break; }
```

and, after the element loop over `min_nitems`:

```c
    if (result == 0)
    {
        if (nitems1 != nitems2)
            result = (nitems1 < nitems2) ? -1 : 1;
        else if (ndims1 != ndims2)
            result = (ndims1 < ndims2) ? -1 : 1;
        else
        {
            for (i = 0; i < ndims1; i++) { ... dims1[i] vs dims2[i] ... }
            if (result == 0)
            {
                for (i = 0; i < ndims1; i++) { ... lbound1[i] vs lbound2[i] ... }
            }
        }
    }
```

The comment above it is the server's own account of why the order of those
four is what it is: *"The relative significance of the different bits of
information is historical; mainly we just care that we don't say 'equal' for
arrays of different dimensionality."*

`array_eq`, in the same file, is the contrast:

```c
    /* fast path if the arrays do not have the same dimensionality */
    if (ndims1 != ndims2 ||
        memcmp(dims1, dims2, ndims1 * sizeof(int)) != 0 ||
        memcmp(lbs1, lbs2, ndims1 * sizeof(int)) != 0)
        result = false;
```

`record_cmp` in `src/backend/utils/adt/rowtypes.c` carries the NULL sentence
verbatim — `/* We consider two NULLs equal; NULL > not-NULL. */` — and walks
the two tuples' columns in logical order, raising
`could not identify a comparison function for type %s` where a column type has
no `cmp` proc.

**Observed.** `fixtures/<13-18>/oracle/comparisons.tsv` carries the shape
tie-break as committed answers: `integer[]`'s `{1,2}` is **above**
`[0:1]={1,2}` (equal elements, equal counts, lower bound 1 above 0) and
**below** `{{1,2},{3,4}}` (the two elements it has agree, and it has fewer). A
shape-first order would answer the first the other way round.

**Scope limit.** Ordering and equality of *array* and *composite* values.
Ranges and multiranges have their own comparisons and their own canonical
storage form, which are I46. Nor is the *collation* an element comparison runs under: that is
the element's own question, one level down, and I32/I37/I43 are where it lives.

**Verified against:** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 (source);
observed in the committed oracle at all six, and the probe below run against
16.15 and 18.6.

**Relied on by:** [`decisions.md`](decisions.md), "D58" — `predicate.rs`'s `compare_nested`, and `pgtype.rs`'s
`NestedCompare`, whose `Uncomparable` position is the inheritance rule.

**Re-verify.** Read the functions:

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
grep -n -A150 '^array_cmp(FunctionCallInfo' src/backend/utils/adt/arrayfuncs.c
grep -n -A40  '^array_eq' src/backend/utils/adt/arrayfuncs.c
grep -n -B5 -A40 'We consider two NULLs equal' src/backend/utils/adt/rowtypes.c
```

And ask a server, which is a minute per major:

```sh
docker run -d --rm --name pgdt-i45 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:<N>-trixie
docker exec -i pgdt-i45 psql -qtA -U postgres <<'SQL'
CREATE TYPE point2d AS (x integer, y text);
SELECT '{1,2}'::int[]        > '[0:1]={1,2}'::int[]   AS lower_bound_last,
       '{1,2}'::int[]        < '{{1,2},{3,4}}'::int[] AS fewer_elements_below,
       '{{1,2,3,4}}'::int[]  < '{{1,2},{3,4}}'::int[] AS dims_before_lbs,
       '{NULL}'::int[]       > '{2147483647}'::int[]  AS null_element_above,
       ROW(NULL,'a')::point2d > ROW(1,'a')::point2d   AS null_field_above;
SELECT '{}'::json[] < '{}'::json[];   -- 42883: no operator, no cmp proc
SQL
docker rm -f pgdt-i45
```

Every column of the first query is `t` at every supported major, and the second
statement is an error at every one.

---

## I46 — A range is stored in a canonical form, and its order is bound-wise with `empty` below everything

**Claim.** Four statements, all about `rangetypes.c` and `multirangetypes.c`:

- **`range_in` never stores the literal it was given.** It hands the parsed
  bounds to `make_range`, which is `range_serialize` — three
  type-independent rules — followed, for a range type that has one, by the
  *canonical function*, after which `range_serialize` runs again. The three
  rules: a lower bound whose value is above its upper is
  `ERRCODE_DATA_EXCEPTION` (`22000`); bounds whose values are equal without
  *both* ends including the point make the range `empty`; and an absent
  ("infinite") bound is never inclusive.
- **Three built-in range types canonicalize and three do not.**
  `int4range`, `int8range` and `daterange` carry `int4range_canonical` /
  `int8range_canonical` / `daterange_canonical`; `numrange`, `tsrange` and
  `tstzrange` carry none, and neither does a user-defined range unless its
  `CREATE TYPE … AS RANGE` declares `canonical = …`. All three built-in
  functions are the same rewriting — an exclusive lower bound becomes
  inclusive at the successor, an inclusive upper becomes exclusive at the
  successor — differing only in the width they raise "out of range" at.
  **`daterange_canonical` additionally skips any bound that is
  `DATE_NOT_FINITE`**, so `[2020-01-01,infinity]` keeps its inclusive upper
  while `[-infinity,2020-01-01]` still becomes `[-infinity,2020-01-02)`.
- **`range_cmp` sorts `empty` below every other value**, then compares lower
  bound against lower and upper against upper. `range_cmp_bounds` settles an
  infinite bound before it looks at a value — an infinite *lower* is the
  minimum, an infinite *upper* the maximum — and settles inclusivity only
  when the two values are equal, where an exclusive **lower** ranks above an
  inclusive one and an exclusive **upper** below.
- **`multirange_in` normalizes before it stores.**
  `multirange_canonicalize` sorts the members by `range_cmp`, drops every
  empty one, and merges any adjacent or overlapping pair, so no stored
  multirange has an empty member or two members that meet. `multirange_cmp`
  is then member-wise, with a shorter multirange below a longer one whose
  members agree. Whether two members are *adjacent* is decided by
  `bounds_adjacent`, which asks whether the range **between** them — the two
  bounds relabelled and their inclusivity flipped — comes out empty, and
  answers `false` outright for a range type with no canonical function.

**Proof.** `src/backend/utils/adt/rangetypes.c` (v18.6, and unchanged in shape
since v13). `make_range`:

```c
	range = range_serialize(typcache, lower, upper, empty, escontext);
	...
	/* no need to call canonical on empty ranges ... */
	if (OidIsValid(typcache->rng_canonical_finfo.fn_oid) &&
		!RangeIsEmpty(range))
```

`range_serialize`'s three rules:

```c
		/* error check: if lower bound value is above upper, it's wrong */
		if (cmp > 0)
			ereturn(escontext, NULL,
					(errcode(ERRCODE_DATA_EXCEPTION), ...
		/* if bounds are equal, and not both inclusive, range is empty */
		if (cmp == 0 && !(lower->inclusive && upper->inclusive))
			flags |= RANGE_EMPTY;
		else
		{
			/* infinite boundaries are never inclusive */
```

`daterange_canonical`'s guard is the one place the three differ:

```c
	if (!lower.infinite && !DATE_NOT_FINITE(DatumGetDateADT(lower.val)) &&
		!lower.inclusive)
```

`range_cmp` puts `empty` first (`/* For b-tree use, empty ranges sort before
all else */`) and then calls `range_cmp_bounds` twice;
`range_cmp_bounds` handles `b1->infinite`/`b2->infinite` before invoking the
subtype's comparison proc, and falls through to the inclusivity block only
when that proc returns zero. `multirange_canonicalize` and `multirange_cmp`
are in `src/backend/utils/adt/multirangetypes.c`; `bounds_adjacent`,
`range_before_internal` and `range_union_internal` are back in
`rangetypes.c`.

**Observed.** `fixtures/<13-18>/oracle/literals.tsv` carries the rewriting as
committed answers — `int4range '[1,10]'` is written `[1,11)`, `'(0,10)'` is
`[1,10)`, `daterange '[2020-01-01,2020-01-01]'` is `[2020-01-01,2020-01-02)`
while `'[2020-01-01,infinity]'` is unchanged, `numrange '[1,10]'` is
unchanged, and `'[10,1)'` is `E22000` — and `comparisons.tsv` carries the
order, `empty` below every other `int4range` value at all six majors. The
probe below answers identically on `postgres:16.15-trixie` and
`postgres:18.6-trixie`.

**Scope limit.** Ranges and multiranges. Arrays and composites are I45. The
*collation* a `text`-bounded range's comparison runs under is not covered here:
a range type carries its own `collation` parameter, which this build does not
read (see `decisions.md`, "D58").

**Verified against:** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 (source);
observed in the committed oracle at all six, and the probe below run against
16.15 and 18.6.

**Relied on by:** [`decisions.md`](decisions.md), "D58" — `predicate.rs`'s `make_range`, `compare_range`,
`compare_bounds` and `canonical_multirange`, and `pgtype.rs`'s
`NestedCompare::Range`/`Multirange` and the `discrete` flag
`builtin_range_subtype` sets. The first claim is also what makes a
user-declared `canonical` function (I10) a refusal rather than a divergence:
the rewriting happens before the value is stored *or* compared, so it is not
an operator this build could route around.

**Re-verify.** Read the functions:

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
grep -n -A40 '^range_serialize(TypeCacheEntry' src/backend/utils/adt/rangetypes.c
grep -n -A20 '^make_range(TypeCacheEntry'      src/backend/utils/adt/rangetypes.c
grep -n -A45 '^daterange_canonical'            src/backend/utils/adt/rangetypes.c
grep -n -A60 '^range_cmp_bounds(TypeCacheEntry' src/backend/utils/adt/rangetypes.c
grep -n -A50 '^multirange_canonicalize'        src/backend/utils/adt/multirangetypes.c
```

And ask a server, which is a minute per major:

```sh
docker run -d --name pgdt-i46 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:<N>-trixie
docker exec -i pgdt-i46 psql -qtA -U postgres <<'SQL'
SELECT 'empty'::int4range < '(,)'::int4range   AS empty_below_unbounded,
       '(,5)'::int4range < '[1,10)'::int4range AS unbounded_lower_first,
       '[1,10)'::int4range < '[1,)'::int4range AS unbounded_upper_last,
       '(1,10)'::numrange > '[1,10)'::numrange AS excl_lower_above_incl,
       '[1,10)'::numrange < '[1,10]'::numrange AS excl_upper_below_incl,
       '{}'::int4multirange < '{[1,10)}'::int4multirange AS shorter_below;
SELECT '[1,10]'::int4range::text, '(0,10)'::int4range::text, '(1,2)'::int4range::text,
       '[1,1)'::numrange::text,   '[1,10]'::numrange::text,
       '[2020-01-01,infinity]'::daterange::text,
       '[-infinity,2020-01-01]'::daterange::text;
SELECT '{[1,5),[5,10)}'::int4multirange::text, '{[5,10),[1,5)}'::int4multirange::text,
       '{[1,1),[1,10)}'::int4multirange::text, '{[1,5),[6,10)}'::int4multirange::text,
       '{[1,5],[6,10)}'::int4multirange::text, '{[1,5),[6,10)}'::nummultirange::text,
       '{[1,3),[2,5)}'::int4multirange::text,  '{[1,5),(5,10)}'::nummultirange::text;
SELECT '[1,9223372036854775807]'::int8range;   -- bigint out of range
DO $$ BEGIN PERFORM '[10,1)'::int4range;
      EXCEPTION WHEN OTHERS THEN RAISE NOTICE 'out of order: %', SQLSTATE; END $$;
SQL
docker rm -f pgdt-i46
```

Every column of the first query is `t`. The second answers `[1,11)`,
`[1,10)`, `empty`, `empty`, `[1,10]`, `[2020-01-01,infinity]`,
`[-infinity,2020-01-02)`; the third `{[1,10)}`, `{[1,10)}`, `{[1,10)}`,
`{[1,5),[6,10)}`, `{[1,10)}`, `{[1,5),[6,10)}`, `{[1,5)}`,
`{[1,5),(5,10)}`. The last two statements are an error and a `22000` notice.
The multirange lines need v14 or later (I10).

---

## I47 — `int2vector` writes space-separated `int16`s, takes a wider input, and compares as an array of `smallint`

**Claim.** Four statements about the one `pg_catalog` type whose text form is a
container and whose name is not spelled like one:

- **`int2vectorout` writes each element's decimal spelling separated by a
  single space, and nothing else.** No wrapper, no quoting, no escaping, and
  the **empty string** for a zero-element vector. An element is `pg_itoa`'s
  output, so it never carries a `+`, a leading zero or a `-0`.
- **There is no NULL element.** The struct sets `dataoffset = 0` — the comment
  in `int2vectorin` says *never any nulls* — so a value of this type cannot
  hold one and the output form needs no spelling for one.
- **`int2vectorin` accepts a strict superset of that.** Before each element it
  skips any run of `isspace`; the element itself goes to `strtol` base 10, so a
  leading `+` and leading zeros are taken; the result is refused with `22003`
  outside `SHRT_MIN`…`SHRT_MAX`; and the byte *after* a number must be a space
  or the terminator, so `1\t2` is `22P02` while `\t1 2` is accepted.
- **The type names no operator or cast of its own, and its comparisons resolve
  through `anyarray`.** `pg_operator` and `pg_cast` have zero rows mentioning
  it; `typlen = -1` with `typelem = int2` is what makes `get_element_type`
  answer, so `<` resolves to `array_lt` and `=` to `array_eq` — I45's
  comparison, over `smallint` elements. `int2vectorin` sets `ndim = 1`,
  `dim1 = n` and `lbound1 = 0` unconditionally, the empty vector included, so
  `array_cmp`'s dimension and lower-bound tie-breaks are constant between two
  values of this type and only the elements and the element *count* can decide.

**Proof.** `src/backend/utils/adt/int.c` (v18.6, unchanged in shape since
v13). `int2vectorout`:

```c
	/* assumes sign, 5 digits, ' ' */
	rp = result = (char *) palloc(nnums * 7 + 1);
	for (num = 0; num < nnums; num++)
	{
		if (num != 0)
			*rp++ = ' ';
		rp += pg_itoa(int2Array->values[num], rp);
	}
```

`int2vectorin`'s loop is the input half, and its three refusals are the three
`ereturn`s:

```c
		while (*intString && isspace((unsigned char) *intString))
			intString++;
		if (*intString == '\0')
			break;
		...
		l = strtol(intString, &endp, 10);
		if (intString == endp)
			ereturn(... ERRCODE_INVALID_TEXT_REPRESENTATION ...
		if (errno == ERANGE || l < SHRT_MIN || l > SHRT_MAX)
			ereturn(... ERRCODE_NUMERIC_VALUE_OUT_OF_RANGE ...
		if (*endp && *endp != ' ')
			ereturn(... ERRCODE_INVALID_TEXT_REPRESENTATION ...
```

and its tail is the shape claim:

```c
	result->ndim = 1;
	result->dataoffset = 0;		/* never any nulls */
	result->elemtype = INT2OID;
	result->dim1 = n;
	result->lbound1 = 0;
```

`src/include/catalog/pg_type.dat` carries the polymorphism's premise —
`typname => 'int2vector', typlen => '-1', ..., typelem => 'int2'` — and
`array_lt`/`array_eq` are declared over `anyarray` in `pg_operator.dat`.

**Observed.** `fixtures/<13-18>/oracle/` carries all four claims as committed
answers: `literals.tsv` has the empty vector's `output` as the empty string,
`  1   2  ` canonicalizing to `1 2` and `+1 01` to `1 1`, and `32768`,
`1,2`, `1\t2` and `{1,2}` as `E22003`/`E22P02`; `comparisons.tsv` has
`'2' < '10'` true — element-wise, where a byte comparison of the same two
strings is false — and `'1 2' < '1 2 3'` true, which is the element *count*
deciding after an equal prefix. `fixtures/<13-18>/types/default.sql`'s
`public.t_int2vector` carries the output form in real `pg_dump` bytes,
including the empty field beside a `\N`.

**Scope limit.** `int2vector` only. **`oidvector` shares every one of these
properties** — same shape, same `anyarray` resolution, `oidvectorout` writing
space-separated `Oid`s — and is not covered here because nothing maps it: the
ADBC floor answers `arrow.opaque` for it, so it sits in the opaque tail
(`decisions.md`, "D38"), and no committed fixture or oracle case exercises
it.

**Verified against:** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 (source);
observed in the committed oracle and the `types` fixture at all six, and the
probe below run against 16.15.

**Relied on by:** [`decisions.md`](decisions.md), "Type resolution and decoders" (the
`List<Int16>` mapping and `NestedPlan::Int2Vector`), "D44"
(`nested.rs`'s `decode_int2vector`/`render_int2vector`/`parse_int2vector`) and
"D55" (`NestedCompare::Int2Vector`, and the
constant `dims`/`lower_bounds` `nested_key` builds).

**Re-verify.** Read the two functions:

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
sed -n '/^int2vectorin/,/^}/p'  src/backend/utils/adt/int.c
sed -n '/^int2vectorout/,/^}/p' src/backend/utils/adt/int.c
grep -n -A6 "typname => 'int2vector'" src/include/catalog/pg_type.dat
```

And ask a server:

```sh
docker run -d --name pgdt-i47 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:<N>-trixie
docker exec -i pgdt-i47 psql -qtA -U postgres <<'SQL'
SELECT 'ops', count(*) FROM pg_operator WHERE 'int2vector'::regtype IN (oprleft, oprright);
SELECT 'casts', count(*) FROM pg_cast WHERE 'int2vector'::regtype IN (castsource, casttarget);
SELECT 'type', typcategory, typelem::regtype::text, typlen FROM pg_type WHERE typname='int2vector';
SELECT 'out', quote_literal(v::text)
  FROM (VALUES ('1 2 3'::int2vector), (''), ('0'), ('-32768 32767')) t(v);
SELECT 'in', quote_literal('  +1   02  '::int2vector::text);
SELECT 'dims', array_dims('1 2 3'::int2vector::int2[]);
CREATE TEMP TABLE v (a int2vector, b int2vector);
INSERT INTO v VALUES ('2','10'), ('1 2','1 2 3'), ('','0');
SELECT 'cmp', a::text, b::text, a<b, a=b FROM v;
DO $$ BEGIN PERFORM E'1\t2'::int2vector;
      EXCEPTION WHEN OTHERS THEN RAISE NOTICE 'tab between: %', SQLSTATE; END $$;
DO $$ BEGIN PERFORM '32768'::int2vector;
      EXCEPTION WHEN OTHERS THEN RAISE NOTICE 'range: %', SQLSTATE; END $$;
DO $$ BEGIN PERFORM '{1,2}'::int2vector;
      EXCEPTION WHEN OTHERS THEN RAISE NOTICE 'braces: %', SQLSTATE; END $$;
SQL
docker rm -f pgdt-i47
```

`ops` and `casts` are both `0`; `type` is `A|smallint|-1`; the four `out` rows
are `'1 2 3'`, `''`, `'0'` and `'-32768 32767'`; `in` is `'1 2'`; `dims` is
`[0:2]`; all three `cmp` rows answer `t|f`; and the three notices are `22P02`,
`22003` and `22P02`.

---

## I48 — A `timestamptz` is written in its session's one zone, so within a table its text is a function of its instant

**Claim.** Every `timestamptz` value one `pg_dump` connection writes is
rendered in that connection's `TimeZone`, which nothing `pg_dump` runs
changes. The ISO text carries the wall-clock time in that zone and the zone's
offset at that instant, so the text fixes the instant and the instant, in one
zone, fixes the text: **two values of one table are one text exactly when they
are one instant**, however each was written in. Only the offsets vary with the
zone's own rules (DST), never with how a value was entered.

**Proof.** `timestamptz_out` in `src/backend/utils/adt/timestamp.c` calls
`timestamp2tm(dt, &tz, tm, &fsec, &tzn, NULL)`, and `timestamp2tm` takes a
`NULL` zone to mean `session_timezone`; `EncodeDateTime` then writes the
fields and `EncodeTimezone` the offset — hours, then minutes and seconds
wherever they are not zero. `pg_dump`'s `setup_connection` pins `DATESTYLE`,
`INTERVALSTYLE` and `extra_float_digits` and never `TimeZone` (I4), and no
source file of `src/bin/pg_dump/` sets one: the word appears only in
`pg_backup_archiver.c`'s comment on reading an archive's own creation date.
The infinities have one spelling each (I34).

**Observed.** `fixtures/<13–18>/statistics/*.sql`'s `public.spans.stamp` is
inserted from `2026-01-01 00:00:00+00` and `2026-01-01 05:00:00+05`, one
instant, and `2026-07-01 12:00:00-07`: every major writes two texts, the first
two as `2026-01-01 00:00:00+00` (`pgdump_query/tests/statistics_fixture.rs`
asserts the count).

**Scope limit.** **Per connection, so per database.** `pg_dumpall` runs one
`pg_dump` a database, and an `ALTER DATABASE … SET timezone` makes two
databases' zones differ — which no table spans. A directory-format dump's
workers each open a connection, set up alike from one environment; a
`timezone` set on the role or database between two of them opening is outside
this claim. A zone whose offset has seconds (`LMT`) is still one text an
instant.

**Verified against:** v13.23, v14.24, v15.19, v16.15, v17.11, v18.6 — all six
carry `timestamptz_out`'s `timestamp2tm(..., NULL)` call and
`timestamp2tm`'s `session_timezone` fallback, and none sets a time zone in
`src/bin/pg_dump/`; observed in the `statistics` fixture at all six.

**Relied on by:** [`decisions.md`](decisions.md), "D89" — a distinct count
read off a `timestamptz` column's dictionary is its distinct instants, and so
its distinct Arrow values (`summary.rs`, `ColumnSummary::distinct`).

**Re-verify.**

```sh
cd /mnt/wd12t/upstream/postgres/release-v<N>
awk '/^timestamptz_out\(PG_FUNCTION_ARGS\)/,/^}/' src/backend/utils/adt/timestamp.c | grep -n timestamp2tm
awk '/^timestamp2tm\(/,/^}/' src/backend/utils/adt/timestamp.c | grep -n session_timezone
grep -rniE 'SET +(TIME +ZONE|timezone)|PGTZ' src/bin/pg_dump/*.c
```

The first two print one line each; the third prints nothing.
