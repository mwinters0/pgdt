# P11.2.1 — The register-to-oracle reconciliation

What the next slices inherit. How the check *works* is
[`architecture.md`](architecture.md), "The register-to-oracle reconciliation";
this doc is the part that is not description — the calls that are not obvious
from the code, and what the first run of it found.

## What landed

- `scripts/oracle_register.py` — the arm parse, the case placement, both
  directions and a report.
- `scripts/test_oracle_register.py` — 26 tests; the three `CommittedTree` cases
  are the slice's suite assertion.
- `generate_fixtures.py` runs it at the end of an oracle pass, beside the
  differ.
- [`architecture.md`](architecture.md) gains the section, a Contents row, and
  two pointers: the oracle section now says that a case naming a type
  `fixture_schema_types.sql` does not declare is coverage that tests nothing,
  and the register section says that adding an arm obliges a case.

No library code, per the slice's contract. `pgtype.rs` is **read** and not
edited.

## The first run was green, and that is a fact about the case table

Thirty-three arms, fifty case types, nothing uncovered and nothing unplaced.
That was not the expected outcome — the check was written to find a hole — and
it holds only because 11.1's case table was built from the same list of
declared types the register was later keyed on.

The one place it nearly did not hold is `TypeKind::Shell`, and the resolution
is the reason the user half counts **match arms** rather than `TypeKind`
variants: a shell type cannot be a column's declared type, so no dump can ask
how one compares and no oracle case can exist for it. It shares `Base`'s arm,
which `public.mybase` covers. Per-variant granularity would have demanded
evidence that cannot be taken. The built-in half counts **names** rather than
arms for the opposite reason: the four names sharing `(Utf8View, text)` are
exactly the queue 11.5 closes one at a time, so one case must not excuse three.

## What later slices inherit

**Closing a row of the register does not trip this check; recognising a *new*
type does.** 11.4 and 11.5 change an existing `builtin_scalar` arm's answer —
`AS_TEXT` becomes a `Compared` — and the arm's name is unchanged, so the case
that covered it still does. The check fires the day a base name the table does
not hold today is added to it — `money`, `tsvector`, `pg_lsn` — and the remedy
is one `TypeCases` entry. `xml` and the built-in range names are not examples:
they are already cases, sitting on the unrecognised arm, so recognising one
would move a case rather than leave an arm bare.

**Adding a case is not free after 11.1: it means regenerating the oracle.**
`comparison_cases()` is what the committed files are asserted against row for
row, so a new case makes every major's `comparisons.tsv` stale.
`uv run generate_fixtures.py --skip-dumps` is the pass, and it now ends by
running this check as well as the differ.

**A case for a type the fixture schema does not declare is caught, and it is
the second direction's whole substance.** Such a case answers `E42704` in every
cell of every major, which reads as coverage; the check rejects a
schema-qualified name that is neither declared by
`scripts/fixture_schema_types.sql` nor named as a range's
`multirange_type_name`, and separately rejects any case whose every cell on
every major is `E42704`. The second rule is what covers a misspelled *built-in*
name, which the first cannot see.

**11.11 adds no arm.** The register is keyed on the declared type and a
`COLLATE` clause is a per-column input to an arm that already exists, so
reading it changes which columns are told they diverge and not which arms
exist. The oracle's two collated `text` cases already place on the `text` arm —
the check dedups by type string, so they are one covering case, not three.

**`numeric`'s typmod branches are one arm here.** `map_numeric` picks
`Decimal128`, `Decimal256` or `Utf8View` from the precision, and which of those
a precision reaches is a *mapping* question that `pgtype.rs`'s
`the_register_answers_every_builtin_scalar` owns — it has rows for
`numeric(50,0)` and `numeric(77,0)`, neither of which the oracle asks about.
This check is about comparison arms, and both comparison answers (`Decimal` and
`AS_TEXT`) have cases.

## Calls worth knowing about

**The arms are parsed out of Rust, and the parse's failure mode is silence.** A
renamed function or a restructured `match` yields *fewer* arms, and fewer arms
is a check that passes. So each of the three functions read carries an anchor
string the parse must find — `_ => return None,` in `builtin_scalar`,
`let Some(def) =` in `comparison_user_type`, `array_element(declared).is_some()`
in `comparison_for` — and a missing one is a reported problem. Three tests in
`test_oracle_register.py` remove an anchor apiece. **If you rename one of those
functions, this check is what to update**, and it will say so rather than pass.

**One match guard is understood, and any other is refused rather than
guessed.** `TypeKind::Enum { labels } if labels.is_empty()` is the only guarded
arm, and a guard decides which arm a case lands in — so a guard whose text is
not `labels.is_empty()` is reported and its arm is dropped, rather than
silently treated as unguarded.

**`E42704` earns no invariants entry, because the differ already guards it.**
The evidence rule reads that SQLSTATE as "the server does not have this type",
which is an assumption about PostgreSQL — but a major that changed the code for
an undefined type would move a cell between two *rejections*, which
`oracle_differences.py` classifies non-additive and fails on. The alarm exists;
a second register entry would only restate it.

**The placement re-states `comparison_for`'s walk, and nothing else.** Array,
then schema-qualified, then the built-in table: three lines of Python that say
which arm a case belongs to and never what the arm answers. `is_array` is the
simplification — it takes a trailing `]` or ` ARRAY` where `array_element`
implements the whole `opt_array_bounds` grammar — and it is safe in the
direction that matters: a case using a shape it reads wrong is still placed in
*some* arm, so the risk is a weaker report, never a wrong one.

**The `TypeKind` of a `public.*` case comes from
`scripts/fixture_schema_types.sql`, not from a hand-written table.** That file
is the DDL the oracle's database is loaded from, so the classification reads
the same source the server did, and a type whose kind changes there changes
here. The parse understands the six `CREATE` forms that file uses and reports
anything else; a base type's completion (`CREATE TYPE x (INPUT = …)`) overrides
its own earlier shell.

## What was left out, and why

**The check does not reconcile `the_register_answers_every_builtin_scalar`
against `builtin_scalar`.** The arm parse makes that a third direction for
almost nothing — every name the parse finds must appear in that Rust table —
and it is the direction 11.3's notes named as resting on discipline. It is out
of scope for a slice whose row names two directions, and it is a cheap
follow-up for whichever slice next edits the built-in table.

**Nothing reads the oracle from Rust, still.** The reconciliation is a coverage
check over two artifacts, not an assertion that pgdq's answers match the
server's. Comparing answer to answer is what the per-type slices do, each in
its own terms.
