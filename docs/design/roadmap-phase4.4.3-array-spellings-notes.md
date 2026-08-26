# Phase 4.4.3 — The array-declaration spellings: notes

The defect 4.4.2 shipped, closed by reading the declaration the way the server
does. `pgtype::array_element` normalizes the whole `Typename` array-bounds
production to the element type plus one array level, so all six spellings of an
array-typed column (I28) resolve exactly as `integer[]` does. The mechanism and
its rejected alternative are in
[`architecture.md`](architecture.md), "Type resolution"; these are the rest.

## What the next slice inherits

**`integer[][]` is an ordinary array column again**, and the census owns its
depth. Before this slice it was `ColumnResolution::NestedArrayElement` — a
refusal that stated something false about the column rather than declining to
answer it. Two pinned expectations changed with it, and both are the same
change:

- `pgtype.rs`'s `an_array_over_an_array_typed_element_is_refused_through_any_chain_of_domains`
  no longer lists `integer[][]`;
- `resolve.rs`'s `the_census_only_speaks_for_a_column_the_ddl_resolved_to_an_array`
  needed a real I26 shape for its third column and now spells it
  `public.intarr[]` over `CREATE DOMAIN public.intarr AS integer[]`.

**The I26 refusal is untouched, and is now the only way to reach it.** A domain
whose base is an array, arrayed, is still refused — and the refusal is tested
on the domain walk's terminal, which is why the spelling matters there too:
`CREATE DOMAIN d AS integer ARRAY` is as legal as `AS integer[]`, and the walk
stops on whatever the DDL wrote.
`a_domain_over_an_array_is_recognized_in_every_spelling` runs both faces (the
column `d`, and the column `d[]`) over all five array spellings of the base.

**Nothing about `pg_dump` output changed.** Every spelling collapses in the
catalog, so this is entirely the hand-written and other-producer input path —
which `pg-dump-compatibility.md`'s "Producers other than `pg_dump`" row already
names as in-contract and untested, and which is why five of the six spellings
can never have a fixture.

## Where the evidence lives, and why it is in two places

The slice's claim splits cleanly, and neither half can carry the other:

| Claim | Pinned by |
|---|---|
| `pg_dump` writes all six spellings back as `integer[]` | `fixtures/*/types/default.sql`'s `public.t_array_spelling`, and `tests/preamble.rs`'s assertion on its four declared types across every major |
| pgdq reads all six as one array of the element | `pgtype.rs`'s `every_array_declaration_spelling_is_one_array_of_the_element_type`, citing I28 |
| pgdq reads a declaration PostgreSQL rejects as *no* array | `pgtype.rs`'s `a_declaration_postgresql_would_reject_is_not_read_as_an_array` |
| the seam resolution → census → `retype_from_census` survives a spelling no fixture can hold | `tests/decode.rs`'s `an_array_declaration_pg_dump_never_writes_resolves_and_takes_its_census`, over a hand-built dump |

The unit tables are `roadmap.md`'s "Where a fixture is impossible" carve-out,
and the I28 citation at the test site is the carve-out's check: it puts the
behaviour inside the register's re-verify ritual instead of outside every
mechanism the repo has.

**`t_array_spelling` is its own table, not columns of `t_array_shape`.** Every
value in it is an ordinary 1-D array and its census entries say nothing;
`t_array_shape` is read as "the shapes the census reports". Same reason 4.4.2
kept `v_nested_array` out of it.

**Its columns did not go into `tests/nested.rs`'s `NESTED_COLUMNS`**, unlike
4.4.2's. That list is the codec's round-trip set, and these literals are
`integer[]`'s — already covered by `t_array` several times over. The novelty is
entirely in the DDL, which is where the assertions are. The table *is* in
`tests/decode.rs`'s typed-vs-`Strings` list, which costs nothing and says the
collapse leaves an ordinary column behind.

## Calls made in this slice

**The normalization is one helper at two call sites, and the second is
`element_is_array` rather than `domain_terminal` itself.** The grilling put it
"inside `domain_terminal`"; the walk returns a borrowed `&str` and normalizing
`integer[3][4]` to `integer[]` there would have to allocate, while
`element_is_array` is the only reader of the terminal whose question is "is
this an array". `element_is_opaque` reads the same terminal and is unaffected:
a spelling never hides an opaque terminal, it only ever adds an array level,
and the array refusal answers first for anything it would have caught.

**The grammar is followed rather than approximated, in both directions.** The
bracket run is unbounded (`integer[][][][][][][][][]` is one array level, since
a declaration's bracket count is not `MAXDIM`-limited and means nothing), but
`ARRAY` takes at most one bound and only in the `[n]` form, over a bare type
name. So `integer ARRAY[4][5]`, `integer ARRAY[]`, `integer[] ARRAY` and
`integer ARRAY ARRAY` stay `Unknown`. All four are syntax errors on 16.15,
checked live, and the reasoning is the slice's own: inventing an array type for
input PostgreSQL rejects is the same mistake as reading a spelling more
literally than PostgreSQL does, pointed the other way. The observations and
their re-verify command are in I28.

**Normalizing at parse time was rejected**, and the reason is durable enough to
sit in `architecture.md`: `ColumnNote::declared` carries the raw declared string
so `pgdq info --verbose` can print what the file says beside what we made of
it, and a parse-time rewrite would have pgdq quietly editing the user's DDL in
the one place the raw text is the entire point.

**Not quote-aware**, which is pre-existing and not widened: `CREATE DOMAIN
"weird[]" AS integer` reads as an array of `"weird`. Named at the helper;
fixing it means a real type-name tokenizer.

## Counts, for anyone diffing a regeneration

`fixtures/16/types/default.sql` is 24 `COPY` blocks / 95 rows / 21270 bytes,
8 of 85 columns unmapped (4.4.2 left it at 23 / 92 / 20494, 8 of 80). The
unmapped count is unchanged, which is the point: four new columns, all
`Mapped`. `t_array_spelling`'s `CREATE TABLE` is byte-identical across 13–18.

## Verification

- `cargo test --workspace`: 327 pass, 0 fail. `cargo clippy --workspace
  --all-targets` and `cargo fmt --check` clean.
- New: three `pgtype.rs` unit tests (the six spellings, the rejections, the
  domain-over-an-array spellings); `tests/decode.rs`'s hand-built
  spelling dump; per-table assertions for `t_array_spelling` in
  `tests/preamble.rs` (the declared types) and `tests/pgtype.rs` (the
  outcomes).
- Changed: the two tests that pinned `integer[][]` as a refusal, above.
- **Not run: the koji scan.** It runs once at phase wrap, after 4.6, and this
  slice cannot move it — koji's preamble carries six array declarations, every
  one spelled `[]`.
