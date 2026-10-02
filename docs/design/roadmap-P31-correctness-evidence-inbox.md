# P31 inbox — facts filed for its grilling

Evidence P31 (correctness evidence: the emitter register and value oracles)
will need. **This is a queue, not a document**: when P31 is grilled, walk every
entry, fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## Nine defects, one shape: a form `pg_dump` can write that no fixture held

**Fact.** `KD60`–`KD63` (the 2026-10-01 repoint) and `KD64`–`KD68` (the
2026-10-02 audit) were each found by reading `pg_dump`'s source or a decoder
against the forms it can meet, never by a failing test, and each is a shape
the fixture tree does not hold: a negative-scale zero, `CREATE TABLESPACE`
among globals, a database segment after an empty one, `numeric(2,5)`, an
inheritance child, an unlogged table, a `--binary-upgrade` composite with a
dropped attribute, `bytea_output = escape`, a database name taking the
connstring `\connect`. The emitting code is known for each: `shouldPrintColumn`
and the `CREATE %s%s %s` line of `dumpTableSchema`, `dumpCompositeType`'s
dropped-attribute branch, `appendPsqlMetaConnect`'s `complex` branch
(`fe_utils/string_utils.c`), `byteaout`'s `bytea_output` switch,
`numeric_out` under a typmod.

**Why P31 cares.** This is the phase's thesis stated as data: the gap is
enumerable from the producer's source, and a check that walks the producer's
branches and asks for a fixture reaching each would have found every one of
the nine. The grilling decides the unit of that register (a function's branch,
a `pg_dump` flag, a GUC, a catalog shape) and how it is read — by hand from
the checkouts in `/mnt/wd12t/upstream/postgres/` at each supported major, as
the invariants register already cites them, or mechanically.

**Origin.** 2026-10-02 audit; `docs/status/history/2026-10-02.md`. *Contingent
on* nothing: the nine entries stand in the register until closed.

---

## The reconciliations that exist, and the direction none of them reaches

**Fact.** Three checks already reconcile an enumerable source against the
evidence: `tests/pgtype.rs::every_resolution_outcome_is_produced_by_a_real_fixture_column`
(resolution outcomes ↔ fixture columns), `scripts/oracle_register.py`
(comparison arms ↔ oracle cases, D71) and `scripts/floor_mapping.py` (ADBC
floor rows ↔ `builtin_scalar` arms, D38). Each enumerates *our* side or the
*driver's*; none enumerates **`pg_dump`'s emitters**, and the roadmap's
fixture rule names that as the uncovered half ("a shape that resolves to an
existing outcome and merely works … stays a judgement call"). Every one of the
nine defects above is in that half.

**Why P31 cares.** The emitter register is the fourth reconciliation, aimed at
the uncovered direction, and the three existing scripts are its model: what
they parse, how they fail, how their tests assert their own rules
(`scripts/test_oracle_register.py`, `test_floor_mapping.py`).

**Origin.** 2026-10-02 audit, reading `roadmap.md`, "Expand the generated
fixtures freely".

---

## The round-trip oracle is self-consistency, not a second reading

**Fact.** `pgdump_query/tests/decode.rs`'s round trip compares the typed read,
rendered back, against `SchemaMode::Strings` over the same fixture (D73). That
proves `render ∘ decode` is the identity on the fixture's values. It cannot see
a decoder that is wrong and invertible, a value the fixture does not hold, or
a mapping that reads a value as NULL where the server holds one — `KD60`'s
zero would have failed it had a negative-scale fixture column existed, and
`KD63`'s panic needed a `numeric(2,5)` column no fixture has. The comparison
oracle (D70) is a genuine second reading, but of *order*, not of value.

**Why P31 cares.** A value oracle the server writes beside the comparison
oracle — per typed column, a reading not in the output text's spelling: a
timestamp's `EXTRACT(EPOCH …)` in microseconds, a `numeric(p,s)`'s
`(v * 10^s)::bigint`, a `bytea`'s `encode(v, 'hex')`, a `date`'s Julian day —
lets a test assert the Arrow value itself, not its rendering. Whether it lives
in `comparison_oracle.py`'s pass or its own, and which types take one, is the
grilling's.

**Origin.** 2026-10-02 audit. *Contingent on* the oracle pass staying in
`generate_fixtures.py`'s container, which is where the typed DDL is.

---

## The output GUCs `pg_dump` leaves unpinned are a fixture axis

**Fact.** `setup_connection` in `pg_dump.c` pins `DateStyle = ISO`,
`IntervalStyle = POSTGRES` and `extra_float_digits = 3` (I4), and nothing
else that moves an output spelling. Three GUCs reach a `COPY` block's text
unpinned: `bytea_output` (`KD67`, confirmed by a 16.15 probe under `ALTER
DATABASE … SET`), `TimeZone` (I48: a `timestamptz`'s text is one function of
its instant *within a table*, the offset being the session's), and
`lc_monetary` (`KD13`). `generate_fixtures.py` runs every container at the
image's defaults, so every fixture is `hex`, `UTC`, `C`.

**Why P31 cares.** Each unpinned GUC is a value of a fixture axis, not a
one-off fixture: a flag set, or a database-level `SET` applied before the
dump, that the generator runs the `types` schema under. The grilling decides
whether the axis is a column of the fixture tree (`fixtures/<major>/types/
<flag-set>-<guc>.sql`) and which GUCs it takes.

**Origin.** 2026-10-02 audit; I4, I48, `KD13`, `KD67`.

---

## `parse_create_table` reads non-column fragments as columns

**Fact.** `split_top_level_commas` tracks parens and quotes, not `[`/`]`, and
`parse_column_fragment` accepts any fragment whose first token is an
identifier. So a table-level `CONSTRAINT c CHECK (…)` becomes a `ColumnDef`
named `constraint` with the constraint's name as its type, and a `DEFAULT
ARRAY[a, b]` splits into two fragments, the second a `ColumnDef` named `b`
whose type is `]`. Both are harmless today because resolution joins by the
`COPY` header's names and a table's real columns precede its constraints
(16.15 probe: `public.checked`, whose real `"constraint"` column resolves
`Int32`), but a later real column named after a word inside an earlier
default's brackets would find the bogus definition first.

**Why P31 cares.** An emitter register that enumerates `dumpTableSchema`'s
column-list branches is the place this grammar gets a shape per branch and a
fixture per shape; a `CREATE TABLE` grammar that returns constraints and
columns as different things is the fix, and it is a `D<k>` question whether the
preamble should hold constraints at all.

**Origin.** 2026-10-02 audit; `runs/parse-audit-20261002/16-plain.sql`.

---

## A typed table's columns are in the dump, as its composite's fields

**Fact.** `CREATE TABLE x OF t;` has no column list (I5), so its columns
resolve `NotDeclared`, but `t`'s `CREATE TYPE … AS (…)` precedes it with the
same names and types in the same order. `pg_dump` writes the clause
`OF <type>` after the name (16.15 probe: `public.typed OF public.pt`).

**Why P31 cares.** A gap in the same family as `KD64`'s inheritance walk — a
table whose columns are declared elsewhere in the file — and closed the same
way, by recording the reference and resolving a missing column through it;
whether the two share a mechanism is for the grilling.

**Origin.** 2026-10-02 audit.

---

## What the audit did not read

**Fact.** The 2026-10-02 audit read `copy.rs`, `decode.rs`, `nested.rs`,
`pgtype.rs`'s mapping half, `preamble.rs`, `lex.rs`, `scan.rs`, `map.rs`'s
builder, `batch.rs`, `resolve.rs`, `unrepresentable.rs` and `index.rs`'s
census against `pg_dump` 16.15's source and probe output. It did not read:
`statistics.rs`/`gather.rs` (a bound's text re-decoded, `KD46`/`KD47`'s
neighbourhood — a wrong bound prunes rows silently), `predicate.rs`'s literal
parsers (`parse_*` in `nested.rs` were read, their callers were not),
`pgtype.rs`'s comparison register, `pgt`'s `--where` grammar, or the `INSERT`
run (P8).

**Why P31 cares.** The emitter register's scope decision: whether it covers
only what a dump *holds* (DDL and `COPY` text) or also what a query *types*
(a filter literal, a stored bound), where the oracle already stands in for
the latter.

**Origin.** 2026-10-02 audit.
