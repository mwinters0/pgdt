# P11.1 — The comparison oracle

What the next slices inherit. How the oracle *works* is
[`architecture.md`](architecture.md), "The comparison oracle"; this doc is the
part that is not description — the calls that are not obvious from the code,
and the four facts the generation turned up.

## What landed

- `scripts/comparison_oracle.py` — the case table (50 declared types), the SQL
  that asks the server, and a COPY TEXT reader.
- `scripts/generate_fixtures.py` — a second pass over the same containers,
  with `--skip-dumps` / `--skip-oracle` to run either alone.
- `fixtures/<13…18>/oracle/{meta,literals,comparisons}.tsv` — 1224 comparison
  rows and 279 literal rows per major, ~70 KB each. (11.2.2 took the
  comparisons to 1616 by adding the collation dimension.)
- `scripts/test_comparison_oracle.py` — 21 tests, of which the six structural
  ones walk the committed tree.

No library code, per the slice's contract.

## What 11.2 inherits

**The answer files carry no case identifiers.** A row means what it means only
by sitting at the case table's index, which is why
`test_comparison_oracle.py`'s `CommittedTree` asserts the `(type, left,
right)` triples equal `comparison_cases()` row for row, in order, for every
major. The differ can therefore **zip** two majors' files positionally and
does not need to join on a key — but it must not skip that assertion, because
zipping mis-aligned files is a silent wrong answer rather than an error.

**The differ's classification is a per-cell string comparison, and both
tables want it.** A cell moving from `E<sqlstate>` to a non-`E` value is
additive; any other change is not. Running that over the tree as it stands
today gives:

| Step | Comparison cells additive | Non-additive | Literal rows additive |
|---|---|---|---|
| 13→14 | 318 | 0 | 9 |
| 14→15 | 0 | 0 | 0 |
| 15→16 | 0 | 0 | 0 |
| 16→17 | 180 | 0 | 2 |
| 17→18 | 0 | 0 | 0 |

**Zero non-additive differences exist across the six supported majors**, which
is the evidence the phase's union rule was asserted on and had not been
checked. 13→14 is `numeric`'s infinities plus the two multirange types; 16→17
is `interval`'s infinities. Both announced themselves exactly as the spec
predicted they would.

`literals.tsv` catches something `comparisons.tsv` cannot: a change in a
value's **output spelling**. The spec named that as out of the differ's scope,
on the grounds that a fixture regeneration shows it as a file diff. It is in
scope now, for free, and 11.2's differences file should cover both tables.

**The reconciliation's oracle side is a flat set of type strings.**
`{case.type for case in TYPE_CASES}` is the "every oracle case resolves to a
register arm" direction; the strings are spelled the way `pg_dump` writes a
column's type in `CREATE TABLE`, so the join against the L2 register (11.3) is
string equality with no normalisation. Keep it that way — a normaliser is a
second, untested spelling authority beside `pgtype.rs`.

## Four findings

**`record_in` does not skip whitespace; `array_in` does.** `'( 1 , a )'` into
`public.point2d` canonicalizes to `(1," a ")` — the leading and trailing
blanks around an unquoted field are *preserved* and then force-quoted on
output, while `'{ 1 , 2 }'` into `integer[]` canonicalizes to `{1,2}`. The
phase spec describes the array superset in detail and says "the
`array_in`/`record_in`/`range_in` superset" as if it were one grammar. It is
three, and this is the first place they visibly disagree. 11.9 owns it; both
literals are in `literals.tsv` on all six majors.

**`bpchar::text` is not `bpcharout`.** A cast to `text` strips the blank
padding; the dump's `COPY` block does not. The literal table's `output` column
is produced by `textin(<typoutput>(…))` — the function looked up per type from
`pg_type.typoutput` — precisely so it answers "what does this look like in the
file". 11.6's canonicalize-the-literal-once path reads this column; had it
been a `::text` cast, the `char(5)` case the spec names would have been
recorded backwards.

**The oracle could not see a collation divergence, and 11.2.2 is what that
finding became.** The fixture containers were the Alpine images, so the server
was musl-libc: `datcollate` read `en_US.utf8` while musl's `strcoll` is
`strcmp`, and the text cells were the `C`-collation answers by accident of the
base image, with nothing in the tree saying so. 11.2.2 moved the family to the
Debian images and made the collation a field on the case, so both halves are
now in the file. What stays closed by *statement* is the residue — a
`default`-collation column with no `COLLATE` clause, whose collation a plain
dump does not record (I32).

**A type that does not exist is `E42704`, and that is the mechanism, not a
hole.** Nothing in the case table is version-gated: `int4multirange` and
`public.myrange_multi` answer `E42704` on 13 and answer properly on 14+, which
is exactly the additive transition the differ exists to recognise. Gating them
would have hidden the one transition the check was written for. Resist adding a
version predicate to `TypeCases` for the same reason.

## Calls worth knowing about

**One row per `(type, left, right)`, with six operator columns**, rather than
the spec's `(type, left, right, operator)` long form. Same table, pivoted: six
times fewer committed lines, and the six answers for one pair are what has to
be read together. The differ walks cells, not rows.

**All ordered pairs of a type's `values`, including the self-pair**, rather
than a ladder of consecutive comparisons. A ladder is sound only if both
orders are already known to be total, which is the property under test. The
cost is `n**2` rows per type, which is why the lists are 3–8 long and why
`inputs` — the malformed and non-canonical literals — are compared against
`values[0]` only.

**Every case is its own subtransaction.** Both helpers are PL/pgSQL with an
`EXCEPTION WHEN others` block returning `'E' || SQLSTATE`, so a malformed
literal cannot abort the surrounding `COPY`. That is what makes "record whether
the server accepted the input" possible in one statement instead of 1500.

**The oracle runs in the `types` schema's database**, so a case may name
`public.mood`, `public.point2d`, `public.myrange`, `public.textrange`,
`public.intarr[]`, `public.mybase` and the three domains. Adding a case that
needs a type `fixture_schema_types.sql` does not declare means adding it there
first — and that schema is loaded by `create_fixture_db`, which
`generate_for_version` calls a second time for the oracle pass.

**`--skip-dumps` is the ordinary way to re-take the oracle.** A dump
regeneration is not byte-reproducible (three things move on their own,
[`architecture.md`](architecture.md), "Fixtures"), so regenerating the whole
tree to change an answer table buries the change in noise.
