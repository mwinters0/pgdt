# P11 inbox — facts filed for its grilling

Evidence found in earlier phases that P11 (typed predicates) will need.
**This is a queue, not a document**: when P11 is grilled, walk every entry,
fold it into its spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## The nested-equality semantics are measured on both sides, and PostgreSQL's are not the obvious ones

**Fact.** Array, composite, range and multirange columns resolve to
`List`/`Struct` Arrow types, but a predicate on one compares the COPY-unescaped
field text — the `array_out`/`record_out`/`range_out` literal — as a plain
string. Making such a predicate mean what PostgreSQL means needs the semantics
below, which were measured against a live server rather than reasoned about:

| Probe | PostgreSQL |
|---|---|
| `array[1,null] = array[1,null]` | **true** — `array_eq` treats NULL elements as equal, not NULL |
| `'{1,2}' = '{{1,2}}'` | false — dimension-sensitive |
| `'[0:1]={1,2}' = '{1,2}'` | **false** — lower bounds are semantically significant |
| `row(1,null) = row(1,null)` | **true** — `record_eq` is NULL-aware |
| `'[1,2]'::int4range = '[1,3)'::int4range` | true, via canonicalization |

The first and fourth are the traps: element-wise and field-wise equality are
**NULL-aware** rather than NULL-propagating, so they do not follow the
three-valued logic the scalar operators do. A phase that builds one 3VL
evaluator and applies it uniformly to nested columns will get both wrong.

DataFusion v55 is structurally the same and therefore not an obstacle: full
`=`/`<`/`<=`/`>`/`>=` on `List` columns, element-wise and lexicographic
(`datafusion/sqllogictest/test_files/array_query.slt`), and struct equality
against a literal coerced **by field name** rather than by position
(`struct.slt:1573` — `s = {y: 2, x: 1}` matches `{x: 1, y: 2}`). PostgreSQL's
`record_eq` is positional instead, which costs nothing here because a
composite's fields are always built in declaration order.

**Why P11 cares.** This phase is exactly the two things P5 deferred — full
boolean structure with real three-valued logic, and type-aware comparison on
nested columns — and the table above is the specification of the second, plus
the reason the first cannot simply be applied to it. The one-time work it
implies is the *input*-side grammar (`array_in` is considerably more permissive
than `array_out`'s inverse — I20's scope limit) and canonicalization for the
three discrete built-in ranges: `int4range`, `int8range`, `daterange`, while
`numrange`, `tsrange` and `tstzrange` do not canonicalize.

**Origin.** P4 grilling, 2026-08-25, filed for P5; re-filed here when P5's
grilling deferred typed nested comparison (2026-08-29). Decision and rationale:
[`architecture.md`](architecture.md), "Predicates"; register entry I20.

---

## The ordering register names the types whose comparison does not match the server

**Fact.** P5 adds `<`, `<=`, `>`, `>=` on any column that resolved `Mapped`
with a `Scalar` plan, and maintains a register of every Arrow type such a
comparison can land on, stating whether it agrees with PostgreSQL and what
would close the gap. Three rows do not agree, and each has a different remedy:

- **`Utf8View` from `text`/`varchar`/`char`/`name`** — PostgreSQL orders by
  collation and a plain dump records none (I32). Bytewise equals the server
  only under `C`/`POSIX`. Nothing in the file can close this.
- **`Utf8View` from bare `numeric`** (and `numeric` past 76 digits) — no Arrow
  decimal representation, so it orders lexicographically. Closing it needs an
  arbitrary-precision decimal comparison, and nothing external is missing.
- **`Dictionary(Int32, Utf8)` from an enum** — PostgreSQL orders by
  *declaration* order; label text orders alphabetically. The dump carries the
  declaration order verbatim in `CREATE TYPE … AS ENUM (…)`, so this is
  additive and needs no new evidence.

**Why P11 cares.** This phase is where predicates are made to mean what
PostgreSQL means, and the register is the worklist: it says which types are
already right (so they must not be disturbed), which are wrong-but-fixable from
the dump alone, and which one is not fixable from the dump at all — the last
being a scope boundary P11 has to state rather than discover. The maintainer's
expectation is that this work proceeds **type by type**, which is what the
register is shaped for.

**Origin.** P5 grilling, 2026-08-29. The register itself lives in
[`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md) until `P5.6` lands, then in
[`architecture.md`](architecture.md), "Predicates". Evidence:
[`../status/history/2026-08-29.md`](../status/history/2026-08-29.md), and
register entries I4 and I32.
