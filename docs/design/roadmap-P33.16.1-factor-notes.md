# P33.16.1 notes — a query has one factor

**Each column's builder states its decode factor, and a query's plan folds
its columns' with its terms' key factors into one** ([`decisions.md`](decisions.md),
"D154"). No reading was taken and no count pair: nothing on a hot path moved.
What proves it is `batch/builder/factor/tests.rs` — each type's factor, and a
column of each type's densest literal held within its factor's Arrow form by
Arrow's own count of its buffers, within a fifth of it where the part's slot
is the whole of it — and `stream/replay/plan/tests.rs`'s
`a_query_s_factor_is_its_columns_and_terms_largest`.

- **`Factor`** (`pgdt-query`'s `batch/builder/factor.rs`): bytes held per
  text byte, in lowest terms, `From<KeyFactor>`. **`column_factor`** reads it
  off the column's builder: `form` (each repeated part's entry bits over its
  fewest bytes with its delimiter, and a value's own bytes) and `stage` (a
  part's token, unquoted copy and a `bytea`'s decoded bytes).
- **`ResolvedExpr::key_factors`** (`predicate/resolve.rs`): an ordering
  term's, a `jsonb` `=`'s and a nested term's key factor. It returns codec's
  `KeyFactor`s, `predicate` sitting below `batch` (`options.rs` reads it).
- **`ReplayPlan::factor`** (`stream/replay/plan.rs`): the largest over every
  planned block, `None` where nothing grows with a line. Nothing reads it yet;
  `expect(dead_code)` marks it and `Factor::of`.

## What 33.16.2 inherits

- **`bigint[]`'s factor is `65/16`, not the largest.** `text[]`'s is `161/16`,
  `bytea[]`'s `89/16`, a `numeric(76,0)[]`'s `257/16`, and an array of
  records more, each field's slot counted over a record's punctuation
  (STATUS, "Decisions worth another look").
- **The factor is crate-private**: a leg reading it, rather than a constant
  of its own, needs it reached from a binary — the `introspect` build's
  `planned_factor` since 33.16.2.

## What 33.4 inherits

- **The carve reads `ReplayPlan::factor`** before `plan_partitions` solves
  the readers; the provider reaches the plan through `TablePartitions`.
- **A membership's lookup is outside the factor**: `Lookup::Decoded` and
  `Lookup::Nested` key each field whole, which no factor bounds
  ([`roadmap-P33.15-positions-notes.md`](roadmap-P33.15-positions-notes.md)).
- **A builder's doubling is outside it** (D141's precedent), so at a growth a
  `bigint[]` column holds its form twice for a moment, until 33.24 reserves a
  long nested field's builders to its bound.

## Negative results

- **A tier-tested container's factor is its walk's**, its test a walk of
  its own staging nothing
  ([`roadmap-P33.25-tier-notes.md`](roadmap-P33.25-tier-notes.md)).
- **An enum, like every scalar, has none**: its label is a scalar's own text,
  and a label it does not declare reaching the batch is `KD88`'s.
- **An unconstrained `numeric` is text**, so its factor is one.
