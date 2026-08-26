# Phase 4.5.1 — The shape census, consuming half: notes

What the rest of Phase 4 inherits. The census is now unconditional, and
resolution reads it: a top-level array column commits to the shape the file
actually holds, before the first batch. The mechanism is in
[`architecture.md`](architecture.md), "The array shape census"; these are the
rest.

## The two decisions the spec left open, and how they went

**A brace run longer than `MAX_ARRAY_DIMS` is not evidence, so the column
keeps its optimistic type.** 4.5's notes said only that 4.5.1 "must treat
`max > MAX_ARRAY_DIMS` as unusable rather than as a depth". Unusable admits
two readings, and they differ in what a user gets. Degrading the column to
text hides a damaged file behind a column that reads fine; keeping `List<T>`
makes the offending row a `FieldDecode` naming the table, column, row offset
and value. The second is what every other value contradicting its declared
type already gets ([`architecture.md`](architecture.md), "Errors"), so that is
what this does — and it is the reason the `MAX_ARRAY_DIMS` test runs *before*
the lower-bound-prefix test in `shape_verdict`, not after. A census of
`(1, 200)` is mixed by the plain reading and would otherwise degrade the
column silently.

**Only a plan of exactly `Array(non-array)` is retyped**, and the case it
protects is **reachable, not hypothetical** — confirmed live on PostgreSQL 16
while reviewing this slice:

```
CREATE DOMAIN intarr AS int[];  CREATE TABLE tt (x intarr[]);
INSERT INTO tt VALUES (ARRAY['{1,2}'::intarr,'{3}'::intarr]);
COPY tt TO STDOUT;   -->   {"{1,2}","{3}"}
```

*(4.4.2 removes the need for this guard: the shape below is refused at
resolution, after which no nested `Array` plan can reach the transform at all.
Until then the guard is what keeps the census from making the column worse.)*

PostgreSQL allows an array *of a domain over an array*, `resolve_declared_type`
gives that column `List<List<Int32>>`, and the value is **one brace deep**
because `array_out` force-quotes any element containing a `{` (I25). So its
census reads `(1, 1)`, and an implementation that stripped every `Array` level
and rebuilt to the census's depth would collapse the column to `List<Int32>`
and then fail on every row of it. The census records
the shape of a whole field, so it cannot distinguish an array of arrays —
written `{"{1,2}","{3}"}`, one brace deep, because `array_out` force-quotes
any element containing a `{` (I25) — from a plain 1-D array. A column already
resolved to `List<List<…>>` therefore reads a census entry that says `(1, 1)`
and must ignore it, or a declared `integer[][]`, or an array of a domain over
an array, would be collapsed to a single list level. The guard is one match
arm in `retype_from_census` and it is load-bearing;
`the_census_only_speaks_for_a_top_level_one_dimensional_array_column` pins it
alongside the composite and multirange cases, which the same guard excludes
for their own reasons.

## Non-obvious calls

**The census is a parameter of `resolve_columns`, not something it looks
up.** `resolve_columns(qualified_table, columns, metadata, database, mode,
census)`. A second entry point that skipped the census would make "did every
caller switch?" a review question at each of `stream::resolve_block`'s three
call sites — including `resume_state`'s, where the failure mode is a resumed
stream rebuilding an optimistic schema where the original had a retyped one,
i.e. the same query returning two different Arrow schemas depending on whether
it was interrupted. One entry point makes it a compile error, which is the
same reasoning that deleted `render_field`'s plan-less form in 4.4. A caller
with no evidence passes `&[]`.

**`&[]` and an all-default census are deliberately the same answer.** Both
mean "nothing constrained anything, keep the optimistic type". Nothing needs
to tell them apart, which is what the `Option`'s removal bought: there is no
longer a state where a consumer must ask whether a block was censused at all.
*Reviewed 2026-08-26 and kept.* The two callers' intents do differ —
`print_index` passes `&[]` to mean "I may not believe this index", a query
passes a census that happens to constrain nothing — but the correct answer does
not, and an `Option<&[ArrayShape]>` that distinguishes states nothing acts on
is the `Option` just deleted, one layer up. Caller intent belongs in
`print_index`'s `complete` flag, where it already is.

**The union is computed once per stream, not per block.** `table_stream`
collects `matches`, folds `index::union_census` over it, and passes one
`&[ArrayShape]` to all three `resolve_block` sites. One schema per stream is
the existing rule ([`architecture.md`](architecture.md), "One schema per
stream, resolved up front"); a per-block census would let two blocks of one
table commit to two Arrow types.

**`print_index` gained a `complete` flag that is `true` at every call site
today.** The reported path is where `DumpIndex::is_complete` still matters,
and no CLI surface currently reaches a partial index — `info --source`
rescans whenever the cache falls short, cache-only mode refuses an incomplete
cache, and `parse` always reaches EOF. The parameter is the rule written down
where Phase 9 will make it live (`roadmap-phase9-partial-reporting.md`, 9.2),
not dead machinery.

**A reported schema and a streamed one can legitimately disagree** for a table
spanning several blocks (I2). `pgdq info` lists per block and resolves from
that block's own census; a query commits one schema from the union. So a table
whose first block is 1-D and whose second is 2-D shows `List(Int32)` and
`List(List(Int32))` on two `info` lines and comes back as text from a query.
Each is true of what it describes — the listing is about a block, the query's
schema is about every row it will hand back — and no fixture produces the
shape.

## What this changes for a reader of the old behaviour

- `tests/decode.rs::a_multidimensional_or_decorated_array_is_refused_on_the_optimistic_path`
  is gone. `t_array_shape` now round-trips against `Strings` mode like every
  other table, and `the_census_retypes_an_array_column_before_the_schema_commits`
  asserts the schema it commits to.
- The refusal's one remaining cause — an array inside a composite — has no
  fixture, because no fixture schema inserts a 2-D array into a composite
  field. `an_array_inside_a_composite_keeps_the_optimistic_path` writes a
  small dump by hand and pairs the failing table with a control table holding
  a 1-D array in the same composite field: the control round-trips through the
  typed path, so a failure on the other table is the nested array's and not a
  misquoted composite's. That pairing is what keeps a hand-written literal
  from encoding a misreading (`architecture.md`, "Testing philosophy").
- `Error::FieldDecode`'s message now ends with "use --schema-mode strings to
  read this column verbatim". It is true of every cause the error has, not
  just the array one — reading more of the file helps none of them.

## What a later slice inherits

- **The manual states one path, where the spec's "What the manual must say"
  still describes two.** The spec's manual bullets were written before the
  2026-08-26 reversal that made the census unconditional and freed a streamed
  schema from `is_complete`; after it, a query is on the same path whether the
  file has been fully parsed or not, so "how to tell which path a given run is
  on" has no answer to give a user. The spec's census section was rewritten
  that day and its manual section was not. Flagged in `STATUS.md`, "Decisions
  worth another look", rather than amended unattended.
- **4.6's array stress data is now measuring a wider path.** Every mapping
  pass censuses, so the unmeasured cost — array-bearing rows, where the
  pre-filter passes and every field is split — now falls on a cold query too,
  not only on `pgdq parse`. `measurements.md`'s census section says so and
  names 4.6 as where it gets measured.
- **The cache format bumped** (`array_shapes` lost its `Option`), so any
  `.dqcache` from before this slice is ignored, koji's included.

## Verification

- `cargo test --workspace`: 297 pass, 0 fail. `cargo clippy --workspace
  --all-targets` and `cargo fmt --check` clean.
- New: five `resolve.rs` unit tests over the retype rules (depth 1–3, the two
  varying shapes, the dimension-limit case, what the guard excludes, a census
  shorter than the column list); `ArrayShape::merge`'s unit test in
  `index.rs`; `tests/census.rs`'s cold-query retype and partitioned-table
  union; `tests/decode.rs`'s two above.
- Inverted: `tests/census.rs::every_mapping_pass_censuses_whatever_its_extent`,
  and `tests/query_cache.rs::a_query_built_index_tiles_in_every_cache_state`
  compares spans for span again, census included — 4.5's `without_census`
  normalizer is deleted.
- **Not run: the koji scan.** It runs once at phase wrap, after 4.6
  (`docs/status/history/2026-08-26.md`). Outstanding, not passed.
