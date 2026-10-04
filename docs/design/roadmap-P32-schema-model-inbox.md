# P32 inbox — facts filed for its grilling

Evidence P32 (the schema model: every object and property a dump declares)
will need. **This is a queue, not a document**: when P32 is grilled, walk every
entry, fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## P31 discards table constraints; P32 is where they are held

**Fact.** `preamble.rs`'s `parse_table_element` classifies a `CONSTRAINT`,
`CHECK`, `UNIQUE`, `PRIMARY KEY`, `FOREIGN KEY`, `EXCLUDE`, 18's table-level
`NOT NULL <column>` or `LIKE` fragment as not-a-column (I53), keeping of them
only the columns a `NOT NULL` or a `PRIMARY KEY` makes `NOT NULL`
(`TableDef::not_null`, I76) and dropping the rest, because no reader reads one
today; a `LIKE`'s columns are not followed either, so a hand-written `CREATE
TABLE t (LIKE s)` declares none. The
maintainer's aim, stated at P31's grilling, is to capture every object and
property, so the drop is P31's scope and not the design.

**Why P32 cares.** The grammar already tells a constraint from a
column, so holding one is additive: the decision is the model's shape, not
the parse. A `NOT NULL` is held already (`ResolvedSchema::not_null`, refusing
a NULL), the Arrow field staying nullable because `ignore` reads one (D37);
a `PRIMARY KEY` or `UNIQUE` could answer a distinct count without statistics.

**Origin.** P31 grilling, 2026-10-02
(`roadmap-P31-correctness-evidence.md`, "Columns declared elsewhere, and what
the preamble holds"); landed at 31.5.

---

## A primary key, unique, exclusion or foreign-key constraint follows the data

**Fact.** `dumpConstraint` in `pg_dump.c` (18.6) writes `p`, `u` and `x`
constraints, and `f`, as `ALTER TABLE ONLY … ADD CONSTRAINT` in
`SECTION_POST_DATA`, after every `COPY` block; a `CHECK` or `NOT NULL` is
inline in `CREATE TABLE` or, written separately, in pre-data. Indexes
(`dumpIndex`) and a postponed definition (`postponed_def`) are post-data too.

**Why P32 cares.** The model cannot be complete when the preamble is: a
property declared after the data is known only once the scan has passed the
data, or by a read of the file's tail. Whether a metadata-level scan reaches
post-data DDL, and whether `preamble_complete` still means "the model is
complete", is this phase's first decision.

**Origin.** P31 grilling, 2026-10-02, reading 18.6's `dumpConstraint`.
*Contingent on* the section assignment, re-read at each major.

---

## `ALTER TABLE` and `ALTER TYPE` forms that move an object's state

**Fact.** P31 reads these `ALTER` forms beyond today's `ALTER TYPE … ADD
VALUE`: `ALTER TABLE ONLY … INHERIT` and `… OF`, which `--binary-upgrade`
writes after a full column list, `ALTER TYPE … DROP ATTRIBUTE`, and `ALTER
TABLE ONLY <parent> ATTACH PARTITION` (`dumpTableAttach`), its bound kept as
text — a hand-written `CREATE TABLE … PARTITION OF` records the same, but no
lookup walks to its parent's columns (`KD102`).
It resolves inherited and typed-table columns by walking the references
against the preamble's final state, so a later `ALTER TABLE parent ADD
COLUMN` or `ALTER TYPE … ADD ATTRIBUTE … CASCADE` is reached by
construction. Every other `ALTER TABLE` form is ignored: `ADD COLUMN`,
`ALTER COLUMN … SET DEFAULT`, `… SET NOT NULL`, `… ADD GENERATED … AS
IDENTITY`, `… SET COMPRESSION`, `… SET STATISTICS`, `ADD CONSTRAINT`,
`DISABLE TRIGGER ALL` (I31, `KD1`).

**Why P32 cares.** A model of each object at the end of the file is a fold
of every one of these, and P31's walk is the precedent for reading final
state rather than first declaration. Whether P31's walk becomes this model's
general mechanism or is replaced by it is the grilling's.

**Origin.** P31 grilling, 2026-10-02. *Contingent on* P31 landing the walk
as specified.

---

## P31's emitter register enumerates what there is to capture

**Fact.** P31's register extracts every literal the `pg_dump` functions pgdt
reads append, and every long option, at each major, and requires each to
reach a fixture. Its function list is limited to what pgdt reads today:
`dumpTableSchema`, `dumpCompositeType`, `dumpEnumType`, `dumpRangeType`,
`dumpDomain`, `dumpBaseType`, `appendPsqlMetaConnect`, `_printTocEntry`,
`setup_connection`, and `pg_dumpall`'s database and tablespace emitters.

**Why P32 cares.** "Every object and property" needs a completeness
criterion, and the register is a mechanical one: widening its function list
to every `dump*` function and asking that each literal map to a captured
property states the goal as a check rather than an aspiration.

**Origin.** P31 grilling, 2026-10-02. *Contingent on* P31 landing the
register in the shape its spec gives.

---

## Widening the emitter register's function list needs a finer query rule

**Fact.** The extraction drops a buffer whose `data` an execute call reads,
as a catalog query, and stops on one whose `data` is read anywhere else too
(`scripts/emitter_register.py`, `query_buffers`). At 18.6, `dumpDatabase`,
`dumpTableData_copy`, `dumpTableData_insert`, `_selectOutputSchema`,
`_selectTablespace`, `_selectTableAccessMethod` and
`_printTableAccessMethodNoStorage` each trip it: the archiver's helpers
execute a statement when connected and print it otherwise, and the others
reuse a buffer across both.

**Why P32 cares.** Its completeness criterion is the register widened to
every `dump*` function; these are among the first it would add, and each
needs a rule telling a buffer's query uses from its output ones before its
literals can be rows.

**Origin.** P31.1, 2026-10-02, a trial extraction over the widened list.
*Contingent on* the extraction keeping its buffer-level rule.

---

## The `emitters` fixtures hold properties no model captures

**Fact.** `fixtures/<major>/emitters/` holds, at every major, a form of each
property P31's listed emitters write that pgdt reads into nothing: a base
type's `CREATE TYPE` properties, a range's `canonical`, `subtype_diff` and
`subtype_opclass`, a domain's named `CHECK`, reloptions and toast
reloptions, a view's check option, forced row security, replica identity, a
column's statistics target and compression, and a tablespace's options and
comment under `pg_dumpall` (`scripts/fixture_schema_emitters.sql`,
`fixture_schema_emitters_cluster.sql`).

**Why P32 cares.** Capturing each of them needs a real dump holding it, and
these are already generated and committed; a property captured can be
asserted against them without a new schema.

**Origin.** P31.4, 2026-10-02. *Contingent on* the `emitters` schema keeping
those objects.
