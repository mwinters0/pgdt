# P11.6.2 — the non-deterministic collation is read

What the rest of P11 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "The preamble
grammar and `DumpMetadata`" for the parse and "Ordering operators compare
typed" for the verdict.

## What landed

`CREATE COLLATION` stopped being an `Unparsed` span.

- **`SpanBody::Collation { collation: CollationDef }`** in `map.rs`, from a
  fourth `StatementShape` arm — `classify_statement` now dispatches off six
  keywords rather than five.
- **`CollationDef { name, deterministic }`** in `preamble.rs`, collected per
  database in DDL order on `DatabaseMetadata::collations`. `cache.rs`'s
  `FORMAT_VERSION` went 13 → 14 for it.
- **`comparison_for` takes a fourth argument**, `&[CollationDef]`, threaded
  through `builtin_scalar` and `comparison_user_type` to `collated_text` — the
  same shape `types` already had, and read from `db.collations` at the one real
  call site in `resolve.rs`.
- **`ComparisonDivergence::NonDeterministicCollation`**, the fourth collation
  branch and the first collation verdict whose `affects_equality` is `true`.
- **`oracle_register.py` grew an exemption**, because the new arm can have no
  oracle case; see below.

## The review property: no committed fixture's answer moves

Every `CREATE COLLATION` in the tree is `provider = libc`, which the server
refuses to make non-deterministic (I42), so `collated_text`'s new branch is
never taken on any fixture and the 45,394-cell oracle assertion is unchanged.
That is what made the parse and the verdict landable in one slice despite the
spec's "size by review" rule: the second half cannot move an answer this tree
can observe, and the half that *can* — the parse — is asserted against
committed bytes at six majors.

**The consequence is that the new branch's only evidence is unit tests**, and
11.12 is what puts a non-deterministic column in a dump. Until then
`a_collation_the_dump_declares_non_deterministic_diverges_under_equality_too`
(in `pgtype.rs`) and its two companions in `predicate.rs` and `resolve.rs` are
the whole of it.

## The join is parsed on both sides, and it is deliberately loose one way

A `COLLATE` clause and a `CREATE COLLATION` name come from different `pg_dump`
code paths, so they are matched through `collation_parts` rather than as text:
either may quote an identifier the other leaves bare. **An unqualified
reference matches on the name alone**, which is a false-positive risk and the
one this register is willing to take — it announces where it is unsure, and a
spurious note over correct rows is the direction every other ambiguity here
errs in. `pg_dump` writes both sides schema-qualified for a user-defined
collation, so the loose case is reachable only from a hand-written file.

**The `CREATE COLLATION x FROM y` copy form reads as deterministic and is
wrong about a copy of a non-deterministic collation.** The server copies
`collisdeterministic` along with everything else; this parse sees no option
list and says `true`. `pg_dump` never writes the copy form — it emits the full
option list for every collation it dumps (I42) — so the shape is hand-written
only, where under-claiming costs a note that is not printed rather than a wrong
row set. It is written down in `parse_create_collation`'s doc comment rather
than fixed, because fixing it means resolving one collation against another and
the input it would serve does not exist.

## The reconciliation had to learn that an arm can have no evidence

Adding a fourth branch to `collated_text` obliges a fourth entry in
`oracle_register.py`'s `COLLATION_ARMS`, and that entry can never have an
oracle case: the spec excludes ICU from the oracle because a `collversion`
drifts with the base image, and a non-deterministic collation is ICU-only
(I42). Two options, and neither was free:

- **Leave `COLLATION_ARMS` at three.** The check passes and the arm list is
  silently short — the exact decay the reconciliation exists to catch, in the
  half of it that is a hand-maintained list.
- **Add the arm with an exemption.** The check keeps naming every arm and gains
  a category it did not have.

The second, and `Arm.unoracled` is it: a reason string, reported under its own
heading, excluded from `uncovered`. The precedent is `TypeKind::Shell`, whose
arm no dump can ask about — the difference being that `Shell` is handled by
*granularity* (it shares `Base`'s arm) where this one cannot be, the other
three collation branches all having cases.

**The exemption is checked in the other direction**, which is what stops it
becoming a place to hide an arm: an exempt arm that acquires a case is a
reported problem, not a quiet pass. And each collation branch is now anchored
on a string the parse must find — `states_non_deterministic(` for this one — so
deleting the branch from `pgtype.rs` is reported rather than leaving an arm
nothing can reach. `ANCHORS` values became `(signature, (anchor, ...))` for
that.

## What 11.12 inherits

**The parse is in place and the fixture is the other half of its guard.**
`tests/preamble.rs`'s
`the_types_schema_declares_one_deterministic_collation_at_every_major` asserts
`public.c_collation` comes back `deterministic: true` at all six majors; adding
an ICU `deterministic = false` collation to
`scripts/fixture_schema_types.sql` means extending that assertion rather than
writing a new one, and the column of it belongs in `tests/ordering.rs` beside
the other `t_collate` columns.

**The oracle must stay out of it.** `oracle_register.py`'s exemption argues
from ICU carrying no case, and it now *fails* if one appears — so a fixture
column is fine and an oracle case is a deliberate reversal of the spec's ICU
exclusion, not an oversight to be waved through.

## What the rest of the phase inherits

**`comparison_for` has four arguments now**, and both list arguments come from
the same `DatabaseMetadata`. If a fifth fact about the database is ever needed
the signature is the place that will say so; nothing here bundled them, because
`types` and `collations` are read at one call site and every test passes `&[]`
for both.

**`ComparisonDivergence` has six variants and three answer `affects_equality`.**
`only_the_non_deterministic_collation_reaches_equality` in `pgtype.rs` states
the whole partition in one test, so a variant added without deciding the
operator dimension fails there as well as failing to compile.

**One spec line did not hold literally.** The checklist said "nothing in
`predicate.rs`"; `ComparisonNote::message` is an exhaustive `match` over
`ComparisonDivergence`, so a variant cannot be added without an arm there. It
is a sentence, not a mechanism — `resolve_term` and the comparison path are
untouched.
