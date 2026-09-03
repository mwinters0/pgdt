# P12.6 — `int2vector`: the codec, the arm, and the oracle case

The last slice of P12. What the **wrap** inherits, and the calls the code does
not explain itself.

The mechanism is filed by subject: [`architecture.md`](architecture.md), "Type
resolution" (the `List<Int16>` mapping, and why the literal form is decided
outside `builtin_scalar`'s tuple), "The nested literal codec" (the fourth
container form, and the one literal with no leaf rule) and "Ordering operators
compare typed" (the `Int2Vector` node and its constant `dims`/`lower_bounds`).
The external evidence is I47.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/nested.rs` | `decode_int2vector`, `render_int2vector`, `canonical_int2`, `parse_int2vector` |
| `pgdump_query/src/pgtype.rs` | `builtin_scalar`'s `"int2vector"` arm; `NestedPlan::Int2Vector`; `NestedCompare::Int2Vector` and its `walk` arm; `map_builtin`'s plan table and its `debug_assert` |
| `pgdump_query/src/batch.rs` | `ColumnBuilder::Int2Vector`, its `append_typed` arm, and `render_field`'s |
| `pgdump_query/src/predicate.rs` | `nested_key`'s arm, and `nested_accepted_form`'s early return |
| `scripts/comparison_oracle.py` | the `int2vector` case — 8 values, 6 inputs |
| `scripts/floor_mapping.py` | `ARROW_RENDERING` gained `list_of(Int16)`; the `waiting` `Disposition` is gone |
| `docs/design/postgres-invariants.md` | I47 |

## Three answers per declared type, and only two come out of one arm

12.4's notes predicted that the resolution arm and the register row would be
one tuple and could not be split — which held. What they did not predict is the
**third** answer: `builtin_scalar` returns `(DataType, ComparisonPlan)` and has
nowhere to put a `NestedPlan`, because until now no built-in name mapped to a
container. `map_builtin` therefore carries a one-line table from the declared
name to its plan, with a `debug_assert` pairing the two — a built-in yielding a
`List` or a `Struct` with no plan named there would be filled by a scalar
builder and would read every value as text.

*Rejected: widening `builtin_scalar`'s tuple to a triple.* Both Python checks
parse that tuple's first element (`floor_mapping.py`'s `_first_tuple_element`,
`oracle_register.py`'s arm walk) and would have survived it, so the cost was
not the parse — it was that every one of the ~25 scalar arms would then state
`NestedPlan::Scalar` to say nothing. *Rejected: deriving the plan from the
`DataType`, since exactly one arm yields a `List`.* That is the inference
`NestedPlan` exists to deny; the assertion checks the pairing without deriving
it.

## Where the type sits in each enum, and why it is not `Array`

Three enums gained a variant, and none of them is an `Array` wrapping a
`smallint` node:

- `NestedPlan::Int2Vector` and `ColumnBuilder::Int2Vector` — because the Arrow
  type is the one `smallint[]` gets and the *literal* is a different grammar,
  which is the collision `NestedPlan` was introduced for (`int4range[]` against
  `int4multirange` was the first).
- `NestedCompare::Int2Vector` — same reason one layer up. The comparison itself
  is `array_cmp`'s: `nested_key` builds a `NestedKey::Array`, so
  `compare_nested` needed no arm at all.

All three are nullary. The element type is `smallint` in the catalog and
nothing about a column can vary it (I47), so a `Box<…>` child would have
exactly one inhabitant and the `walk` would have a position that can never
refuse an order or announce a divergence. The walk visits the node as the leaf
it effectively is, declaring `int2vector`.

**The one thing to know if a `NestedKey` rule is ever revisited:** an
`int2vector`'s `dims` and `lower_bounds` are `[n]` and `[0]` for *every* value,
the empty vector included, where `array_out`'s `{}` is zero-dimensional.
`int2vectorin` sets `ndim = 1` and `lbound1 = 0` unconditionally. It is
consistent rather than load-bearing — two values of this type always agree on
both, so `array_cmp`'s dimension tie-breaks never decide — but writing `[]` for
the empty vector would have been a quiet inconsistency with the server's own
representation.

## The literal has no leaf, and that is the one rule this type bends

Everywhere else a container's literal is read with the `*_in` superset (I44)
and each *leaf* in its own type's output form, so `--filter 'a={ 1 , 2 }'` is
accepted and `--filter 'p=( 1 , a )'` is refused. `int2vector` has no element
input function for that rule to apply to — `int2vectorin` reads the elements
itself, taking a `+` and leading zeros from `strtol` — so the superset reaches
all the way down and `--filter 'v=+1 01'` matches the value the file writes as
`1 1`.

That is faithful to the server and it makes `nested_accepted_form`'s trailing
clause ("with each element, field or bound spelled as the dump spells it, in
that type's own output form") false for this plan — there is no element type
whose output form it could be — so the function answers `Int2Vector` with a
complete sentence before that clause is reached. A future container with the
same shape has to do the same rather than inherit the clause.

The **decoder** is unaffected and stays strict: `+1`, `01`, `-0` and a doubled
space are refused, because `pg_itoa` writes none of them, which is what keeps
`decode_int2vector` and `render_int2vector` inverses.

## The oracle case, and what it is built to catch

12.5 set the requirement: the case must contain a pair on which element-wise
and lexical order disagree, because a `ComparisonPlan` that fell back to text
would agree with the server on most pairs. `'2'` against `'10'` is that pair
and it is in the committed file at all six majors. `'1 2'` against `'1 2 3'` is
the other half of `array_cmp` — equal on the prefix, then the shorter first —
and `''` is the empty vector, which is a *value* and not a NULL.

The six inputs are `int2vectorin`'s edges, and the two that earn their place
are `1\t2` (refused: the byte after a number must be a space or the end, where
a tab *before* one is skipped) and `{1,2}` (refused: the array spelling is not
this type's). Neither reaches the register through the cell walk — that walk
puts the server's own `output` text to both sides — so they are checked by
`tests/nested.rs`'s `oracle` module, which parses every literal row and
compares acceptance against the server's.

Regeneration was `uv run generate_fixtures.py --skip-dumps --skip-floor`,
**63 s** for six majors, log at `runs/oracle-regen-12.6.log`. Twelve `oracle/`
files moved and no `.sql` file did; `fixtures/oracle-differences.tsv` is
byte-unchanged, since `int2vector` answers identically at 13–18.

## Two stale numbers this slice found and corrected

Both are mechanical facts that had drifted before this slice, and correcting
them was cheaper than leaving a document knowingly wrong:

- **`architecture.md`'s "The register against the oracle's answers" listed
  thirty-four exception cases in a three-row table**, where `EXCEPTIONS` in
  `predicate.rs` has held forty in four populations since the `text[]` element
  row landed. `STATUS.md` was already right. The table now has the fourth row.
- **`STATUS.md` said the register-to-oracle reconciliation covers "38 arms, 54
  cases".** It was 41/54 before this slice and is 42/55 after it; the case count
  was accurate and the arm count was not.

The cell counts the slice actually moved are folded in the same way: 47,746 →
**52,338** asserted cells, 397 → **475** literals in the input-grammar walk,
2020 → **2096** comparisons and 317 → **331** literals per major.

## What the wrap inherits

**P12's checklist is complete.** The wrap is the next piece of work: consolidate
`roadmap-P12.1`–`P12.6` into one notes doc, delete the per-slice files, delete
the checklist from `STATUS.md`, and set the roadmap index row to `Complete` in
the same change. No `(b)` deficiency entry names P12, so nothing has to be
re-homed — `deficiencies.py` confirms it today and will again after the state
moves.

One consequence to check at the wrap rather than rediscover: `floor_mapping.py`
resolves a `waiting` disposition's slice against `STATUS.md`'s checklist, so a
`waiting` row surviving the wrap would fail. None survives — `interval` and
`int2vector` both closed — so the wrap is unblocked, and the mechanism is kept
alive by a synthetic row in `test_floor_mapping.py` rather than by a real one.

## Measurement

`nested.rs` is declared by `nested-decode-micro`, `cross-file-floor` and
`projection-widths`; `batch.rs` by those last two plus `census-attribution`;
`pgtype.rs` and `predicate.rs` by none. Every one of those was already red.
**No acknowledgement is written**: the change adds a match arm on the per-field
decode path and a variant to `ColumnBuilder`, which is neither byte-identical
input nor unreachable code, and "small" is not evidence. `uv run measure.py
--stale` is unchanged at thirteen and `--check` still reconciles thirteen
markers.
