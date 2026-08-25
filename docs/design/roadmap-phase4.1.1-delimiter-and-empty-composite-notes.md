# Phase 4.1.1 — Domain-over-`box` array and zero-field composite: notes

The two fixture values 4.1 did not know it needed. Both are inputs to 4.4:
one is the value its opaque-element refusal has to catch, the other is a value
its field-count check has to *not* reject. No library code changed —
`scripts/fixture_schema_types.sql` grew both shapes,
`fixtures/{13..18}/types/{default,data-only,binary-upgrade}.sql` were
regenerated (`uv run generate_fixtures.py --schema types`), and
`tests/{preamble,pgtype,nested}.rs` pin what they produce.

## Where each shape lives

| Shape | Column | Why it is there |
|---|---|---|
| domain-over-`box` array | `t_delimiter.v_box_domain_array` | I22's delimiter trap in the disguise that slips a refusal written against the declared spelling |
| the same type, undisguised | `t_delimiter.v_box_domain` | The scalar, where nothing is at stake — it is the contrast that shows the outcome the array's element walk must *not* stop at |
| zero-field composite | `t_composite.v_empty_comp` | I23: `CREATE TYPE … AS ()` and its `()` value, in the table that already holds every other composite shape |

`public.box_domain` and `public.empty_comp` are the two new type
declarations. Both are identical across 13-18, and the composite's DDL and
value are identical in `binary-upgrade` too.

## The literals

```
t_delimiter  1  (1,1),(0,0)  {(1,1),(0,0);(3,3),(2,2)}
             2  \N           \N
t_composite  1  …  ()
             2  …  \N
             3  …  ()
```

## What 4.4 inherits

**The delimiter trap cannot be caught downstream of resolution — a round-trip
test passes on it.** `decode_array("{(1,1),(0,0);(3,3),(2,2)}")` splits the
two boxes into **seven** elements on the hardcoded `,`; none of the seven
contains a quote-forcing character, so `render_array` puts them back
byte-for-byte identical. Wrong element boundaries, an exact round trip, no
error anywhere. `tests/nested.rs`'s
`the_delimiter_trap_round_trips_while_splitting_on_the_wrong_character` pins
exactly that, so the fixture is not mistaken for a codec test later. The
column is deliberately **not** in `NESTED_COLUMNS`: it would pass, and passing
would mean nothing.

**`public.box_domain` resolves `UnknownType` today, not `OpaqueBaseType`.**
That is the whole leak (`tests/pgtype.rs` pins it): resolution unwraps the
domain to `box`, `map_builtin` has no entry for `box`, and the outcome
`resolve_declared_type` returns keeps no trace of the terminal name it walked
to. A refusal that matches on `box` or `TypeKind::Base` therefore has nothing
to match against by the time it sees this column. The spec's fix — have the
domain walk report its terminal, not just its outcome — is the one that
changes `pgtype.rs`'s shape, and this is the column that proves it is needed.

**`()` decodes as one NULL field, and that is correct.** `decode_record("()")`
returns `RecordLiteral { fields: [None] }`, because the literal is genuinely
ambiguous: a zero-field composite and a one-field composite holding SQL NULL
are written identically (I23, now in the register with its proof and a
re-verification command). So 4.4's field-count refusal **cannot be a plain
equality against the declared field count** — a zero-field composite would
fail it on every row of valid data, which is the exact failure the refusal
exists to prevent. The declared arity has to drive the decode: zero declared
fields means the literal must be exactly `()` and yields no fields.
`render_record(RecordLiteral { fields: [] })` already writes `()`, so the
render direction needs nothing.

**The parser cannot currently tell "zero fields declared" from "no field
parsed".** `parse_create_type`'s composite arm collects with
`filter_map(parse_column_fragment)`, so a body whose fragments are all
unparseable produces `TypeKind::Composite { fields: [] }` — the same value
`CREATE TYPE public.empty_comp AS ();` produces. The spec requires these to
diverge in 4.4 (an unparseable body must yield *no* field list and refuse the
column; an empty body must map to a zero-field `Struct`), so **4.4 has to
change how a composite's field list is represented**, not merely add a check
on top of it. `tests/preamble.rs` pins the empty-body side of that as
`Composite { fields: vec![] }` today; whatever representation 4.4 picks, that
assertion is what says which side of the split the fixture is on.

## Calls made in this slice

**No plain `box[]` column.** 4.1 declined one on the grounds that the rule is
"refuse", not "handle", and the spec's fixture list still names only the
domain-over-`box` array. The disguised spelling is the one a correct
implementation can fail on; the direct one is covered by `pgtype.rs`'s
hand-written unit tests, which need no `pg_dump` output.

**The two shapes went into a new table and an existing one respectively.**
`t_delimiter` is new because the delimiter trap is its own mechanism, distinct
from `t_base_type`'s opaque-element case that it is easily confused with —
having them in separate tables is what lets 4.4's tests say "this one, not
that one". The zero-field composite went into `t_composite` because it belongs
to the family that table already collects, and because it keeps that table
readable end-to-end once 4.4 lands: a zero-field `StructArray` is well-formed
Arrow, so nothing in the table degrades.

## Counts, for anyone diffing a regeneration

`fixtures/16/types/default.sql` is 22 `COPY` blocks / 89 rows / 18262 bytes
and 21 of 71 columns unmapped (was 21 / 87 / 17203 and 18 of 67). The three
newly-unmapped columns are the two new `t_delimiter` value columns and
`v_empty_comp`.
