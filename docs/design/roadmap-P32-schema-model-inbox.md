# P32 inbox — facts filed for its grilling

Evidence P32 (the schema model: every object and property a dump declares)
will need. **This is a queue, not a document**: when P32 is grilled, walk every
entry, fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## P31 discards table constraints; P32 is where they are held

**Fact.** P31 makes `parse_create_table` classify a `CONSTRAINT`, `CHECK`,
`UNIQUE`, `PRIMARY KEY`, `FOREIGN KEY`, `EXCLUDE` or `LIKE` fragment as
not-a-column and drop it (`KD69`), because no reader reads one today. The
maintainer's aim, stated at that grilling, is to capture every object and
property, so the drop is P31's scope and not the design.

**Why P32 cares.** The grammar P31 lands already tells a constraint from a
column, so holding one is additive: the decision is the model's shape, not
the parse. A `NOT NULL` is the nearest reader — it could tell DataFusion a
field is non-nullable — and a `PRIMARY KEY` or `UNIQUE` could answer a
distinct count without statistics.

**Origin.** P31 grilling, 2026-10-02
(`roadmap-P31-correctness-evidence.md`, "Columns declared elsewhere, and what
the preamble holds"). *Contingent on* P31's `KD69` slice landing that grammar.

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

**Fact.** P31 reads two `ALTER` forms beyond today's `ALTER TYPE … ADD
VALUE`: `ALTER TABLE ONLY … INHERIT` and `… OF`, which `--binary-upgrade`
writes after a full column list, and `ALTER TYPE … DROP ATTRIBUTE` (`KD66`).
It resolves inherited and typed-table columns by walking the references
against the preamble's final state, so a later `ALTER TABLE parent ADD
COLUMN` or `ALTER TYPE … ADD ATTRIBUTE … CASCADE` is reached by
construction. Every other `ALTER TABLE` form is ignored: `ADD COLUMN`,
`ALTER COLUMN … SET DEFAULT`, `… SET NOT NULL`, `… ADD GENERATED … AS
IDENTITY`, `… SET COMPRESSION`, `… SET STATISTICS`, `ATTACH PARTITION`
(`dumpTableAttach`), `ADD CONSTRAINT`, `DISABLE TRIGGER ALL` (I31, `KD1`).

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
