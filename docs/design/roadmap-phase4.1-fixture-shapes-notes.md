# Phase 4.1 — Fixture value shapes: notes

What 4.2 inherits. The nested-literal codec now has real `pg_dump` output for
every shape it has to handle, across all six routine versions, and an oracle
for round-tripping: the literals below are what `copy::decode_field` hands it,
and what its renderer must reproduce byte-for-byte.

No library code changed. `scripts/fixture_schema_types.sql` grew the shapes,
`fixtures/{13..18}/types/{default,data-only,binary-upgrade}.sql` were
regenerated (`uv run generate_fixtures.py --schema types`), and
`tests/{preamble,pgtype}.rs` gained assertions pinning the new declarations so
a future regeneration cannot quietly drop them.

## Where each shape lives

| Shape | Column | Why it is there |
|---|---|---|
| array-of-enum | `t_array.v_enum_array` | `public.mood`'s labels contain a space, a comma and a quote, so `array_out`'s three force-quote triggers all fire in one value |
| `[lb:ub]=` decoration | `t_array_shape.v_lbound` | I20's `needdims` prefix, in both a 0-based and a negative-based form |
| mixed dimensionality | `t_array_shape.v_mixed_dim` | Two rows of one column disagreeing about `ndim` — the census's reason for existing |
| uniform 2-D | `t_array_shape.v_multidim` | The censused `List<List<T>>` case, distinct from the mixed one |
| array-of-composite | `t_composite.v_points` | `array_out` (backslash) wrapped around `record_out` (doubling) |
| composite-containing-array | `t_composite.v_tagged` | The same two conventions in the other nesting order |
| text-subtype range | `t_text_range.v_textrange` | The only range in the tree whose bounds ever need quoting |
| opaque array element | `t_base_type.v_mybase_array` | `TypeKind::Base` element — the case 4.4 refuses |

## The literals, as `copy::decode_field` produces them

Taken from `pgdq query --schema-mode strings` against
`fixtures/16/types/default.sql`; identical on 13 and 18.

```
t_composite  1  (1,"a,b""c")  {"(1,\"a,b\"\"c\")","(2,plain)"}  ("a,b","{""x\\""y"",""p q"",NULL}")
             3  (,"")         {NULL,"(3,)"}                     ("",{})
t_array      1  {}  {NULL}  \N  {"a,b","c{d}","e\"f","g\\h"}  {sad,"has space","has,comma",has'quote}
t_text_range 1  ["a,b","c""d")
             2  [" lead","trail ")
             3  ["",a)
             4  (,z)
             5  empty
t_array_shape 1 {{1,2},{3,4}}  {1,2}          [0:2]={7,8,9}
              2 \N             {{1,2},{3,4}}  [-1:0]={10,11}
```

Three of these are the ones a single-convention scanner gets wrong while
passing everything else:

- `{"(1,\"a,b\"\"c\")","(2,plain)"}` — the array layer strips `\"` to `"`,
  and only *then* does the record layer see `(1,"a,b""c")` and fold `""` to
  `"`. Applying either rule at the wrong level yields plausible-looking
  garbage rather than an error.
- `("a,b","{""x\\""y"",""p q"",NULL}")` — the same pair inverted. The record
  layer's doubling comes off first, exposing `{"x\"y","p q",NULL}` for the
  array layer.
- `["",a)` versus `(,z)` — an empty-string bound and an absent bound. Both are
  "nothing between the separators" unless the quoting is honoured, and the
  range struct's `lower`/`upper` must come out as `Some("")` and `None`
  respectively.

## Calls made in this slice

**`v_multidim` moved out of `t_array` into the new `t_array_shape`.** Every
array column in `t_array` is now 1-D with lower bound 1, so from 4.4 the whole
table decodes on the optimistic path; `t_array_shape` holds the three columns
that deliberately do not. Without the split there would be no array table that
`read_table` can read end-to-end without a census, because a table's decode
fails on its worst column. The split is also what lets 4.5's verification
target one table for "with a census this degrades, without one it errors".

**No `box[]` column, deliberately.** The spec's delimiter-trap argument covers
`box` and `TypeKind::Base` with one rule — an array whose element type is
either stays `Utf8View` — and `mybase[]` exercises that rule against real
output. A `box[]` column would add the `;` `typdelim` specifically; it is not
in the spec's fixture list and nothing in 4.2–4.4 needs it, since the rule is
"refuse", not "handle". Add it only if the refusal ever becomes conditional.

**`textrange`'s collation is pinned to `pg_catalog."C"`.** `dumpRangeType`
emits the `collation` clause only when the range's collation differs from the
subtype's default (`CASE WHEN rngcollation = st.typcollation THEN 0`), so
leaving it default would have produced no clause at all and made bound
ordering depend on the container's locale. Pinning it fixes the emitted bytes
across images *and* gives the range grammar a qualified, quoted parameter
value to step over. `parse_create_type` ignores unrecognized parameters, so it
already handles it.

## Two things to know before 4.4

**A user range's companion multirange name can look built-in.**
`public.textrange`'s auto-created companion is named `public.textmultirange`
(PG14+), which is shaped exactly like `int4multirange` and friends. The
hardcoded built-in-range/multirange list must match names *exactly* — never by
`multirange` suffix, and never unqualified-after-stripping-`public.`. Nothing
declares a column of it, so this is a trap rather than a current failure.

**`public.mybase[]` resolves as `Deferred(Array)`, not `OpaqueBaseType`.** The
trailing `[]` decides the outcome before the element type is looked at, so
4.4's opaque-element refusal has to happen inside the array arm's recursion,
not at the top-level dispatch. `tests/pgtype.rs` pins both halves.

## Counts, for anyone diffing a regeneration

`fixtures/16/types/default.sql` is 21 `COPY` blocks / 87 rows / 17203 bytes
and 18 of 67 columns unmapped (was 19 / 77 / 15098 and 11 of 58). Every one of
the seven newly-unmapped columns is one of the four deferred families or
`mybase[]`.
