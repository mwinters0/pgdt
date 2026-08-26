# Phase 4.4.2 — The array-of-array-typed-element refusal: notes

The defect 4.4 shipped, closed by declining the shape rather than
representing it. `resolve_declared_type` now answers
`TypeOutcome::NestedArrayElement` for an array whose element type resolves —
through any chain of domains — to an array, and the column comes back
`Utf8View` with `ColumnResolution::NestedArrayElement` beside it. The
mechanism and its rejected alternatives are in
[`architecture.md`](architecture.md), "Type resolution"; these are the rest.

## What the next slice inherits

**No nested `NestedPlan::Array` can come from the DDL any more**, so a nested
`Array` plan is always one `resolve::retype_from_census` built from the shape
census. 4.5.1's guard — "only a plan of exactly `Array(non-array)` is
retyped" — is deleted, and `retype_from_census` no longer has to ask how the
depth it is looking at was arrived at. Anything that later teaches the
builders to recurse per element has to reintroduce the distinction *before*
producing such a plan, not after.

**`integer[][]` is refused too, and that is the same rule, not a second
one.** Its element is `integer[]`, which is an array. `pg_dump` never writes
it (I21 — PostgreSQL collapses it in the catalog), so no fixture reaches it
and nothing real changes; but it is what makes "no nested `Array` plan from
the DDL" a statement about the code rather than about what dumps happen to
contain, and it is why the guard above could go. Before this slice the
declared spelling resolved to `List<List<Int32>>` and *would* have decoded a
2-D literal — the hand-built case in
`the_census_only_speaks_for_a_column_the_ddl_resolved_to_an_array` is the one
pinned expectation that changed.

**The refusal composes into a composite with no code.** A composite field of
the refused type is `Utf8View` in that position, because `resolve_nested` maps
every non-`Mapped` outcome that way; `t_nested_array.v_arr_holder` is the
fixture face of it and
`a_composite_field_of_the_refused_array_type_is_a_string_field_only` the unit
test. Producing the outcome in one place was the whole change.

**The fixture rule now has a check behind it**, and 4.6 and Phase 9 both meet
it. `tests/pgtype.rs`'s
`every_resolution_outcome_is_produced_by_a_real_fixture_column` resolves every
column of every `COPY` block of every fixture and requires each
`ColumnResolution` variant to have a real column behind it. Two consequences
for later work:

- `outcome_name`'s match is exhaustive with no wildcard, so **adding a
  `ColumnResolution` variant breaks this file's compile** until the variant is
  named and listed. That is deliberate.
- **Phase 9.4's `MetadataNotScanned` is the one variant that must be exempted
  rather than fixtured** — it is a property of how much of the file was read,
  and every fixture here is scanned to EOF. Leave it out of `EVERY_OUTCOME`
  and say why; the test's doc comment already names it, so 9.4 does not have
  to rediscover that the assert message ("add a fixture column") is wrong for
  its case.

## Where each shape lives

`scripts/fixture_schema_types.sql` gained one table and one column. The table
is deliberately **not** part of `t_array_shape`: that one is read as "these
are the shapes the census reports", and `v_nested_array`'s census entry is
`(1, 1)` — correct, and the one place where reading it as an ordinary 1-D
array is exactly the mistake.

| Column | Declared | What it is for |
|---|---|---|
| `t_nested_array.v_nested_array` | `public.intarr[]` | The refusal. I26's literal, `{"{1,2}","{3}"}`, one brace deep |
| `t_nested_array.v_arr_holder` | `public.arr_holder` | The same refusal reached through a composite field — `(L,"{""{1,2}""}")` |
| `t_nested_array.v_pointdom` | domain over a composite | Worked, pinned by nothing |
| `t_nested_array.v_pointdom_array` | array of that domain | Worked, pinned by nothing |
| `t_nested_array.v_boxed_point` | composite whose field is a composite | Worked, pinned by nothing |
| `t_nested_array.v_myrange_array` | array of a user range | Worked, pinned by nothing |
| `t_nested_array.v_rangedom` | domain over a range | Worked, pinned by nothing |
| `t_enum_domain.v_empty_enum` | `public.empty_enum` | The one outcome (`EmptyEnum`) with no real `pg_dump` value behind it |

The five middle rows are the sweep's passing shapes
([`../status/history/2026-08-26.md`](../status/history/2026-08-26.md)). They
ride along because the generator was being run anyway, and because "the
recursion handles it" is exactly the reasoning that let `intarr[]` through.

**The domain-over-domain chain earns no fixture column.** Its literal is
byte-identical to the single-hop case, so there is no `pg_dump` output shape
left to predict.
`an_array_over_an_array_typed_element_is_refused_through_any_chain_of_domains`
pins the transitive walk directly, on `intarr`/`intarr2`/`intarr3`.

## Calls made in this slice

**The empty-enum fixture column was added, though the spec's slice row does
not name it.** The coverage test cannot pass without it — `EmptyEnum` was the
one outcome no generated fixture reached — and the alternative was exempting
the outcome, which would have made the check's first act a hole in itself.
`roadmap.md`'s "Expand the generated fixtures freely" covers the addition. It
also turned up a fact worth recording: a plain dump writes a label-less enum
as `AS ENUM (\n);`, byte-identical to the body `--binary-upgrade` writes for
an enum that *does* have labels (I6). That pair is now **I27**, and the
fixture holds both halves of it in both flag sets.

**`element_is_opaque` and `element_is_array` share one `domain_terminal`
walk** rather than each walking the chain. Both refusals test the terminal
rather than the declared spelling, for the same reason — a domain's own DDL
records neither the delimiter it inherited (I22) nor the array-ness of its
base (I26) — so the walk is the shared thing and the predicate is not.
`element_is_opaque`'s behaviour is unchanged: it checked `box` at every step
before, and the terminal is the only step where that can match.

**The opaque check runs first.** An element that is a domain over `box[]` is
both, and `NestedArrayElement` is the answer either ordering would want to
give; the ordering matters only for an element whose terminal is literally
`box`, where the opaque label is the accurate one.

**The new columns went into `NESTED_COLUMNS`**, unlike 4.1.1's
`v_box_domain_array`, which was deliberately excluded because it round-trips
while splitting on the wrong character. Nothing here has that property: the
outer literal of every one of these is a well-formed array/record/range
literal, so a codec round trip means what it says.

## Counts, for anyone diffing a regeneration

`fixtures/16/types/default.sql` is 23 `COPY` blocks / 92 rows / 20494 bytes,
8 of 80 columns unmapped (was 22 / 89 / 18262, and 21 of 71 before 4.4 mapped
the container families). The `default` output is byte-identical across 13–18
for `t_nested_array`'s whole `COPY` block, which is what upgrades I26's
"Verified against" from one live PostgreSQL 16 to all six routine majors.

## Verification

- `cargo test --workspace`: 300 pass, 0 fail. `cargo clippy --workspace
  --all-targets` and `cargo fmt --check` clean.
- New: two `pgtype.rs` unit tests (the domain chain, the composite field);
  `tests/pgtype.rs`'s per-table assertions for `t_nested_array` and the
  now-four-column `t_enum_domain`; the resolution-outcome coverage test.
- Widened: `tests/decode.rs`'s
  `nested_columns_round_trip_against_strings_mode` takes `t_nested_array` —
  the table that was a `FieldDecode` on every row of `v_nested_array` before
  this slice; `tests/nested.rs`'s `NESTED_COLUMNS` takes its seven nested
  columns.
- Changed: `the_census_only_speaks_for_a_top_level_one_dimensional_array_column`
  is renamed `…_for_a_column_the_ddl_resolved_to_an_array` and now asserts the
  `integer[][]` column is refused rather than ignored.
- **Not run: the koji scan.** It runs once at phase wrap, after 4.6.
