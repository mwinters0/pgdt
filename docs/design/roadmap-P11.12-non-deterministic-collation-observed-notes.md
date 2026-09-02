# P11.12 — the non-deterministic collation, observed

What the rest of P11 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
fixture table and the register's collation arms *work* is
[`architecture.md`](architecture.md), "Fixtures" and "Equality is typed too",
and the external fact is [`postgres-invariants.md`](postgres-invariants.md),
I42. This doc is the part that is not description.

## What landed

Apparatus and evidence only — `pgdump_query/src/` is untouched, and the
register's answers are identical before and after.

- **`CREATE COLLATION public.nd_collation (provider = icu, locale = 'und',
  deterministic = false)`** in `scripts/fixture_schema_types.sql`, and one
  column of it, `t_collate.v_nd`, carrying the same alphabet every sibling
  carries. All six majors regenerated.
- **I42's "Not yet in committed bytes" paragraph replaced by an *Observed*
  one**, covering all three of its claims, plus two `grep`s in `Re-verify` that
  check the committed lines rather than only `pg_dump.c`.
- **`pg-dump-compatibility.md`'s `CREATE COLLATION` row** — both determinism
  halves now have fixture evidence; the row stays `Partially tested` for the
  `FROM y` copy form alone.
- **Four assertion sites**: `tests/preamble.rs` gains `v_nd` in the column list
  and a second `CollationDef`, and `tests/ordering.rs` gains `v_nd` in the
  verdict table, in the bytewise row-set table, and — the one that is new in
  kind — in the equality-note test, as the only column that warns under `=`.

## Calls worth knowing about

**`und` is the locale, and it is chosen for existence rather than for
behaviour.** It is ICU's root locale, present at every ICU version without a
locale being generated, and nothing here reads it: the register branches on the
`deterministic = false` clause, not on the provider or the locale. A
case-insensitive spelling (`und-u-ks-level2`, which `preamble.rs`'s unit test
uses) would have been the realistic thing a person creates, and it would have
put a *behavioural* claim in a file whose whole argument for admitting ICU is
that no behaviour is being claimed.

**All six majors write the statement byte for byte alike, option order
included.** `provider = icu, deterministic = false, locale = 'und'` — the order
is `dumpCollation`'s append order, not the fixture's, which writes
`provider, locale, deterministic`. That the dump reorders is what makes
`parse_create_collation`'s position-independent option scan a tested property
rather than a defensive one. The `colliculocale`/`collcollate` split at v15
(I42) changes how the locale is *read* and not what is written, which the
committed bytes now show.

**The `--binary-upgrade` flag set does carry a `collversion`, and that is the
one place the spec's rationale was narrower than it read.** The argument for
admitting ICU was that a `CREATE COLLATION`'s dump text holds no version, which
is true of `default.sql` and `data-only.sql` and false of
`binary-upgrade.sql` — the `types` schema's third flag set — where `pg_dump`
appends the version the server computed. The spec's clause now says so, and the
line is **guarded** rather than tolerated: `M42` turned it into
`tests/preamble.rs`'s
`the_icu_collversion_reaches_binary_upgrade_alone_and_agrees_across_majors`,
which requires the six majors to agree on the value without naming it
([`architecture.md`](architecture.md), "Fixtures"). It is also the evidence for
I42's claim about *which* flag set carries the version, so the fixture family
gained a fact rather than only a liability. There is no way to have one without
the other short of a flag-set-conditional schema, which is the capability
[`pg-dump-compatibility.md`](pg-dump-compatibility.md) already records as owned
by nobody.

**The oracle is byte-unchanged, which is the check that this is
oracle-neutral.** `fixtures/*/oracle/` was regenerated in the same run and diffs
empty. That is what `oracle_register.py`'s exemption requires: it *fails* if the
exempt arm acquires a case, so a fixture column is fine and an oracle case would
be a deliberate reversal of the ICU exclusion.

**No `KD<k>` closes and none is added.** `KD7`'s surviving statement is a column
stating a collation this build does not implement — which `v_nd` now *is*, in
committed bytes, and which the register still answers bytewise for. The entry
describes exactly that, so it is unchanged; what moved is the evidence under it,
not the deficiency.

**No figure turned stale.** `uv run measure.py --stale` names the same twelve as
before: no figure declares `fixtures/`, `scripts/fixture_schema_types.sql` or
either integration test.

## What the rest of the phase inherits

**`t_collate` has eight reachable columns now, and one of them answers
differently under `=`.** Any later test that loops the collated columns must
decide which family it means: the seven that are `libc` and silent under
equality, or `v_nd`, which is not. The two are indistinguishable by their
clauses, so the loop cannot be written from the column names.

**The fixture family now holds an ICU release number**, in
`types/binary-upgrade.sql` only. A session bumping the image pins will see it
move; that is expected and nothing reads it. It is not an apparatus key in
`meta.tsv`'s sense — `meta.tsv` guards the *oracle's* apparatus, and the oracle
has no ICU in it.
