# P11.11.2 — The displaced `COLLATE` clause, observed

What the next slices inherit. How the fixture table, the register's collation
arms and the invariant *work* is [`architecture.md`](architecture.md),
"Fixtures" and "Ordering operators compare typed", and
[`postgres-invariants.md`](postgres-invariants.md), I37; this doc is the part
that is not description — the calls that are not obvious from the diff.

## What landed

- **Four columns on `public.t_collate`**, and a `CREATE COLLATION
  public.c_collation FROM "C"` beside it: `v_text_def` (one displacer),
  `v_src` + `v_gen_nn` (all three), and `v_user` (the user-collation
  reference form). All six majors regenerated; the eleven-line `CREATE TABLE`
  is byte-identical across 13.23–18.6.
- **I37 amended for v18** — `CONSTRAINT <name> NOT NULL`, `NO INHERIT` and a
  *virtual* `GENERATED ALWAYS AS (expr)` are three shapes that can sit between
  the type and the clause which no earlier major can write — and given a
  twenty-second probe recipe inline in `Re-verify`, for a major with no fixture
  yet. Its **Observed** paragraph now covers the placement in committed bytes
  instead of a container that no longer exists.
- **A `pg-dump-compatibility.md` row** marking those two v18-only shapes
  untested and naming the blocker.
- **Two assertion sites**: `tests/ordering.rs` gains `v_text_def` and `v_user`
  in both collation tests; `tests/preamble.rs` gains
  `t_collate_carries_its_collate_clause_wherever_pg_dump_displaced_it`, the
  whole column list with each clause verbatim, at all six majors.
- The user-collation **property** paragraph in `architecture.md`, "Ordering
  operators compare typed", and one user-facing sentence in
  [`../manual/type-handling.md`](../manual/type-handling.md).

## Calls worth knowing about

**Every clause is written in canonical input position, and the dump moves it.**
`COLLATE` is a `ColConstraint` in PostgreSQL's grammar, so it may be written
anywhere in a column's constraint list and both orders are accepted. The
fixture writes it directly after the type — where a reader expects it — and
`pg_dump` displaces the two with a constraint behind them. That is what makes
the committed bytes evidence of the *emitter's* behaviour rather than an echo
of how the schema was typed.

**`COALESCE` in `v_gen_nn`'s expression is load-bearing.** The alphabet's ninth
row is NULL, `v_gen_nn` is `NOT NULL`, and `upper(NULL)` is NULL — so without
it the `INSERT` fails and the "one alphabet across every column" property would
have had to break instead. It also buys the scan a second nesting level and a
quoted literal (`upper(COALESCE(v_src, ''::text))`), which is the shape
`extract_collation` has to step over.

**The `INSERT` needs an explicit column list.** A positional `VALUES` would try
to supply the generated column, which PostgreSQL refuses outright.

**Adding the collation shifts the `types` schema's `--binary-upgrade` OIDs.**
Expected and documented ([`architecture.md`](architecture.md), "Fixtures", the
fourth mover); outside `fixtures/*/types/`, every line the regeneration moved
was a `\restrict` token or a `now()`/`--verbose` timestamp, with no OID drift on
this run.

**The oracle gained nothing, deliberately.** `public.c_collation` is `FROM "C"`,
so a comparison column under it would reproduce the `COLLATE "C"` column cell
for cell, and it would be the first row in that file existing to document a
decision of ours rather than an answer of PostgreSQL's.
`fixtures/*/oracle/` is byte-unchanged across this slice, which is the check
that the regeneration is oracle-neutral.

**No `KD<k>`, and the reason generalises.** `collated_text` answering
`NonBytewiseCollation` for a provably bytewise user collation yields *correct
rows* with a spurious advisory note. There is nothing for a user to remedy, so
it is a property filed beside the mechanism. `KD7` is the list of places the
order genuinely differs; this is the opposite shape.

## What later slices inherit

**11.6 inherits the two assertion sites, not one.** `t_collate` is now split by
reachability: a column absent from `COPY` can only ever be asserted over
`DatabaseMetadata`. When `character(n)` gains its three collation arms, its
reachable columns' assertions go in `tests/ordering.rs` and anything about a
clause's *form* goes in `tests/preamble.rs`.

**A version-conditional schema `.sql` is a fixture-family capability nobody
owns.** `generate_fixtures.py` conditions dump flag sets on version but runs
one schema file against every major, so the two v18-only column shapes cannot
be fixtured. That is filed in `pg-dump-compatibility.md` and nowhere else: it
is not a deficiency (nothing is wrong, no user is affected) and a roadmap
"Future" row would assert intent nobody holds. The matrix row is where a
session wanting a version-specific DDL fixture will hit it.

## What was left out, and why

**No oracle case, no register entry, no library change.** The slice is
apparatus and evidence; `pgdump_query/src/` is untouched, and the register's
answers are identical before and after.

**No figure turned stale.** `uv run measure.py --stale` names the same seven as
before, for the same reasons — nothing here is a declared path of any figure.
