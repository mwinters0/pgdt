# P33.25 notes — a tier test stages nothing

**A container's tier is read by the output grammars' walk, each leaf tested as
it is handed over** ([`decisions.md`](decisions.md), "D153"), for a query's
batch, the census and the gatherer alike. What proves it is
`crates/pgdt-query/tests/decoded_copies.rs`'s `a_tier_test_stages_nothing` —
a `date[]` of eight MiB in a block a short row's `infinity` counts, read by a
cold query and by a default and a strict parse — which fails on the parent at
its query and at its parse, each holding the staged elements; and
`unrepresentable.rs`'s `the_walk_tests_a_container_as_its_elements_did`,
holding the walk to a test-only copy of the staged test over every container
form's seeds damaged a byte at a time, in place and escaped.

- **`Counter::tier_walked`** (`pgdt-codec`'s `unrepresentable.rs`): a
  container walked by `walk_array`, `walk_record`, `walk_range` and
  `walk_multirange` with `walk_bracketed`, the worst tier kept and every leaf
  past a format-tier one read through untested. `tier_of` hands it a
  container's text in place; `FieldCount::tier` hands it the census's field
  as `COPY` wrote it, unescaped as read, after `escaped_utf8_error` has found
  it UTF-8, so a field held whole is never copied.
- **`UnrepresentableRead::cannot_hold_walked`**: the batch's test, over a
  clone of the `FieldText` its decode then walks. `push_field` walks every
  nested column (`RowBatcher::walks_column` is `is_walked` alone), testing
  first where the column has a test, so a tested column is cut before and
  reserved for a long value as an untested one is (D154).
- **`column_factor`** takes no `tested` flag and `staged` is gone: a tested
  column's factor is its walk's, the test's walk holding the parts the
  decode's does, one at a time, before it. `date[]`'s is `33/16` and
  `datemultirange`'s `80/32` (`factor/tests.rs`).
- **`column_read`** (`pgdt-codec`, test-support): a nested column's
  `UnrepresentableRead` for a test holding no resolved schema.

## Its count pair, placed and accepted

**`80abeb99` against the tree resolves a rise on the data-level parse,
`query --schema-mode strings` and the `arrays` row, and a fall on the typed
query and `pgdt sql -c`**; the metadata-level parse does not move
(`runs/count-pair-33.25-20261111/log`). callgrind reproduces each rise on
both rep pairs and places it (`…/callgrind/reading.txt`, the legs in
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.25/legs/`), D108's falls
unplaced.

- **The parse's is `FieldCount::tier`'s dispatch**, on every field the
  census splits, a held column's included: its `match` decodes `Leaf`'s
  niche discriminant whole and tests the container arms before `Held`,
  where the parent asked `== Held` alone. Changed code, the dispatch a
  container's arm needs.
- **The `arrays` row's is the walked arm's shape**: the `FieldText` built
  before the tier test is held across it and the reservation, then copied
  into `append_walked`'s argument, where the parent built it there.
- **The strings row's is layout**: no column walks, so the restructured arm
  is never entered, and the every-field path is `push_row`'s register
  allocation of code it did not change.
- **Accepted** (D108): spelling either change's code so it lowers shorter, a
  `Held` test apart from the `match` or a `FieldText` built at each use, is
  the rework resting on codegen D108 refuses. Most of the parse's is on held
  columns' fields, which `observe_row` asks `tier` of though its counter
  tests none, as the parent did and `CensusFold` does not: `M280`'s.

## What 33.4 inherits

- **A tested container holds what its walk does**: no factor term beyond the
  untested column's, the second read costing time only in a block counting
  such a value (`unrepresentable_reads`).
- **The census still holds a counted container's field whole**
  (`Kept::Whole`), now with no copy beside it, and the gatherer still
  unescapes a long escaped field whole where its column counts a tier; both
  are P40's ([`roadmap-P40-every-reader-streams-inbox.md`](roadmap-P40-every-reader-streams-inbox.md)).

## Negative results

- **A container malformed past a format-tier leaf is counted nowhere**, as
  `decode_*` refusing it left it: the walk is read to its end whatever it
  has found, so a value is tested whole or not at all
  (`unrepresentable.rs`'s `a_container_is_tested_as_the_walk_reads_it`).
