# P5 inbox — facts filed for its grilling

Evidence found in earlier phases that P5 (pushdown) will need.
**This is a queue, not a document**: when P5 is grilled, walk every entry,
fold it into its spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## Nested columns get a typed Arrow representation but keep an untyped, text-shaped predicate

**Fact.** P4 resolves array, composite, range and multirange columns to
`List`/`Struct` Arrow types, and deliberately changes nothing about
`predicate.rs`: a predicate on such a column still compares the
COPY-unescaped field text — the `array_out`/`record_out`/`range_out` literal —
as a plain string, with `PredicateOp` still only `Eq`/`Ne`/`IsNull`/`IsNotNull`.

That is byte-identical to the behaviour before P4, and it agrees with
PostgreSQL more often than it looks like it should, because **every value in a
dump is already in canonical output form**: discrete ranges canonicalize on
input (`'[1,2]'::int4range` stores and dumps as `[1,3)`) and array input
whitespace is dropped (`'{a, b}'::text[]` dumps as `{a,b}`). The divergence is
confined to the user supplying a *non-canonical* literal, where PostgreSQL
matches and the string comparison silently does not.

Measured semantics on both sides, so P5 does not have to re-derive them:

| Probe | PostgreSQL |
|---|---|
| `array[1,null] = array[1,null]` | **true** — `array_eq` treats NULL elements as equal, not NULL |
| `'{1,2}' = '{{1,2}}'` | false — dimension-sensitive |
| `'[0:1]={1,2}' = '{1,2}'` | **false** — lower bounds are semantically significant |
| `row(1,null) = row(1,null)` | **true** — `record_eq` is NULL-aware |
| `'[1,2]'::int4range = '[1,3)'::int4range` | true, via canonicalization |

DataFusion v55 is structurally the same and therefore not an obstacle: full
`=`/`<`/`<=`/`>`/`>=` on `List` columns, element-wise and lexicographic
(`datafusion/sqllogictest/test_files/array_query.slt`), and struct equality
against a literal coerced **by field name** rather than by position
(`struct.slt:1573` — `s = {y: 2, x: 1}` matches `{x: 1, y: 2}`). PostgreSQL's
`record_eq` is positional instead, which costs nothing here because a composite's
fields are always built in declaration order.

**Why P5 cares.** P4 sits ahead of P5 precisely so pushdown is
designed against the full type set, and this is the part of the type set where
the current predicate model runs out. Making a nested predicate mean what
PostgreSQL means needs two things P4 deliberately did not build: the
*input*-side grammar (`array_in` is considerably more permissive than
`array_out`'s inverse — I20's scope limit), and canonicalization for the three
discrete built-in ranges (`int4range`, `int8range`, `daterange`; `numrange`,
`tsrange` and `tstzrange` do not canonicalize). Both are one-time work that
belongs with typed predicates, not smeared across two phases.

**Origin.** P4 grilling, 2026-08-25. Decision and rationale:
[`architecture.md`](architecture.md), "Predicates"; register entry I20.
