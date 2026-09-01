# P11.4 — Enum and bare `numeric`

What the next slices inherit. How the register *works* is
[`architecture.md`](architecture.md), "Ordering operators compare typed"; this
doc is the part that is not description — the calls that are not obvious from
the diff.

## What landed

- **`CompareKind::Enum(Arc<[String]>)`** — the labels in declaration order,
  taken straight off `TypeKind::Enum`. `order_key` answers a label's position,
  so `OrderKey::Int` carries an enum exactly as it carries a date.
- **`CompareKind::Numeric { infinities }`** — arbitrary-precision decimal over
  the text the file holds, with `NumericKey` as the normalized key.
- **`OrderingDivergence::EnumLabels` is retired**, and with it the `AsText`
  message's per-declared-type branch: `numeric` was the only type it named.
- **`ComparisonPlan` and `CompareKind` give up `Copy`**, which 11.3 and 11.11
  both predicted this slice would spend. `stream::project` clones the plan
  rather than copying it; `resolve_term` borrows it out of `ResolvedSchema` and
  clones the kind into the `OrderTerm`.
- The `KD7` source marker moves off `map_numeric` and onto
  `ComparisonPlan::AS_TEXT`, which after this slice is exactly the register's
  remaining text-held-type row. (`scripts/deficiencies.py` reads the marker
  token itself, so this doc must not spell it.)
- I33 amended: `numeric`'s value order is scale-independent, with
  `cmp_var_common` and `numeric_out` as the proof and two more `Re-verify`
  lines.

**No oracle regeneration, and none was needed.** Both rows' cases were already
in `comparison_cases()` with the right values — `public.mood`'s four labels in
declaration order, and bare `numeric`'s `-Infinity`/`-1`/`0`/`1.5`/`1.50`/
`Infinity`/`NaN`. The expected answers in the new tests were read out of
`fixtures/16/oracle/comparisons.tsv` rather than reasoned about. This is
11.2.1's prediction holding: closing a row changes an arm's *answer*, and the
reconciliation joins on the arm's name.

## Calls worth knowing about

**A `numeric` typmod is what decides whether an infinity exists, not the
Arrow type.** `apply_typmod_special` rejects `±Infinity` under any typmod
(I34), so the register has to tell a bare `numeric` from a `numeric(100,2)` —
both of which are `Utf8View` and both of which reach the same comparison.
`infinities` is that flag, and it is only ever visible on a **literal**: no
field of a constrained column can hold one, so nothing about the row set
changes. It is `oid`'s `UnsignedInt` again — a variant that exists to refuse a
literal.

*Rejected: one `Numeric` kind accepting the infinity spellings everywhere.* It
accepts a filter literal the server refuses, on a column where the answer is a
plain error. That is over-acceptance, which is the direction this project
refuses elsewhere for one line of code.

**`NumericKey` is digit strings, not a big integer.** A bare `numeric` is
arbitrary-precision by definition — the file may hold a thousand digits, which
is past `i256` — so there is no fixed-width type to normalize into. What makes
plain byte comparison correct is the normalization: leading zeros off the
integer part, trailing zeros off the fraction. With trailing zeros stripped, a
fraction that is a prefix of another is the *smaller* of the two, so `"5"`
beats `"45"` and loses to `"55"` under a straight `[u8]` compare. Both halves
have to hold or the compare is wrong in one direction only, which is the shape
of bug that passes half a test.

**`-0` is normalized to `0`.** PostgreSQL has one zero and `numeric_out` never
writes `-0`, but a filter literal may, and the sign check runs before the
magnitude check — so without it `--filter 'v>-0'` and `--filter 'v>0'` would
disagree.

**An enum label the type does not declare is a decode fault, not a
comparison.** `order_key` returns `None`, which is `Error::FieldDecode` for a
field and `Error::PredicateValueDecode` for a literal — the same two faults
every other type raises, held apart the same way. A dump whose data holds a
label its own `CREATE TYPE` does not is a file contradicting its own DDL,
which is what that error means everywhere else.

**The label lookup is a linear scan and stays one.** It is over a label list,
which is a handful of entries in every enum a dump has carried; the *field*
side runs it per row, so a map would be the alternative, and it would cost an
allocation per block to save a comparison against three strings.

## What later slices inherit

**Giving up `Copy` is done, and 11.11's deferred sentence is now free.**
`OrderingDivergence::NonBytewiseCollation` says "a collation other than
C/POSIX" rather than quoting the collation it found, because the plan was
`Copy` and a collation name is not. That constraint is gone: a divergence could
carry the name now at the cost of one more clone per block. Nothing here did
it, because the divergence enum is separate from the kind and widening it is a
change to a user-visible sentence, not to this slice's rows.

**11.6 inherits two decode-per-row exemptions and one of them is now real.**
The spec names bare `numeric` and `interval` as the two types whose equality
cannot use the canonicalize-the-literal-once path. `CompareKind::Numeric` is
the mechanism for the first: `NumericKey::parse` on both sides is exactly the
per-row decode that exemption describes, and it already exists. `interval` has
no kind yet — 11.5 adds it.

**`AsText` now means one thing.** Before this slice it covered a bare
`numeric` and the text-held types, and `OrderingNote::message` branched on the
declared type to tell them apart. It no longer does, so a type 11.5 closes just
stops carrying `AsText`; the sentence needs no edit until the variant has no
members left, which is when it goes.

**The register's arm count did not move**, so `oracle_register.py` is green
without an edit. 11.5 is the same shape: `time with time zone`, `interval`,
`jsonb` and the four network types are existing `builtin_scalar` names whose
answers change. The check fires only on a base name the table does not hold
today.

## What was left out, and why

**No CLI test.** `query_ordering.rs` pins how a term is *split* and that a
divergence reaches stderr; what an ordering comparison *means* is pinned at the
library level, which is where both new tests are. Nothing about the CLI surface
moved.

**No figure was re-taken, and none turned newly stale.** `pgtype.rs`,
`predicate.rs` and `resolve.rs` are declared by no figure; the one line of
`stream.rs` is a clone where a copy was, once per projected column per block,
and `stream.rs` has read stale since `a6e713f`. The stale set is the same
eleven `uv run measure.py --stale` named before.
