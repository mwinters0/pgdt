# P10.1 — Fixture shapes and `--max-line-bytes`: notes

What the later P10 slices inherit from this one, and what it found. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md);
the fixture's own account of each column is its schema,
`scripts/fixture_schema_statistics.sql`.

## The fixture

- **`fixtures/<major>/statistics/default.sql`, all six majors, one flag set.**
  Three blocks: `public.long_value`, `public.ordered`, `public.specials`, in
  that file order. `pgdump_query/tests/common/mod.rs` names it
  `statistics_fixture`, and `all_fixtures()` already sweeps it, so every
  existing fixture-wide test covers it; the three hand-listed schema loops in
  `tests/map_file.rs` and `tests/query_cache.rs` name it too.
- **Every shape is asserted on the file's bytes** by
  `pgdump_query/tests/statistics_fixture.rs`, read as raw `COPY` text rather
  than through the scanner. A later slice reading a column as "ascending" can
  rely on that test rather than on the schema comment.
- **The two collation claims are checked by the server at generation time**, a
  `DO` block in the schema that fails the load: `default_text` ascends under the
  database default collation and `c_text` does not. Nothing in Rust can order
  under `en_US.utf8`, so the Rust test asserts only the bytewise half.
- **Which column carries what**, for choosing a test's subject:
  `ordered.id` strictly ascending, `reversed` strictly descending, `stepped`
  ascending with equal neighbours, `unsorted` a permutation, `constant` one
  value, `all_null` no value, `gappy` ascending with NULLs between, `single` one
  non-null value; `low_card` 4 distinct texts, `high_card` 200 cycling;
  `c_text`/`default_text` the collation pair. `specials.f8`/`f4` ascending
  under PostgreSQL's float order with `-0` beside `0` and `NaN` last,
  `f8_unsorted` the same values unordered, `n` equal numerics under three
  spellings plus `NaN` and a NULL. `long_value.v` ascending bytewise, one value
  past a mebibyte and one past the stored-value cap but short.
- **`ordered` has 1000 rows** so that a stated group size of a few KiB holds
  more than 64 rows, where `high_card`'s dictionary overflows and `low_card`'s
  does not. At the 1 MiB default each block here is one group, so a test that
  wants several groups states the size.
- **The long value clears one group at the default size but leaves no empty
  group**: the row after it starts inside the next mebibyte. An empty group
  needs a stated size well under the value's length, which the tiny-group
  correctness check has anyway.
- **Not in the fixture:** `numeric` `Infinity`, which PostgreSQL 13 cannot
  hold, and `bytea`, `varchar` and `character` bounds, which the slice row did
  not name; add a column rather than reason about their bytes
  ([`roadmap.md`](roadmap.md), "Expand the generated fixtures freely").

## For the slices that land the constants

- `statistics_fixture.rs` carries its own `STORED_VALUE_CAP`, the spec's value
  cap, because the library has no constant yet. The slice that adds one points
  the test at it, and adds the assertion that the long value exceeds the group
  size's default beside the one against `DEFAULT_CHUNK_SIZE`.

## `--max-line-bytes`

- `pgdq parse` and `pgdq query` take it; the library default is now named,
  `pgdump_query::DEFAULT_MAX_LINE_BYTES`. `pgdump_query-cli/tests/max_line_bytes.rs`
  asserts it reaches the serial scan, a split parse and a query, and that zero
  is refused.
- **The limit bounds the carry, not the line.** Every loop checks it after a
  read completes, so a line runs past the limit by up to one read chunk before
  it is refused: at the shipped chunk, a limit of exactly one mebibyte passes
  the fixture's longer line. The help and the manual say so; the test refuses
  at a limit well under a chunk for that reason.

## Negative results

- **A long line is quadratic to scan serially — `KD27`.** The carry pass
  re-searches the whole carried line for a newline at every chunk it spans. A
  probe over single-line synthetic dumps, debug build, roughly quadrupled its
  time per doubling of the line and fell by about the chunk ratio at a larger
  chunk; the leader's pieces were unaffected. Not a figure; the detail is at the
  marker in `scan.rs`.
- **It is why the suite got slower.** `pgdump_query-cli/tests/determinism.rs`
  runs a serial 64-byte-chunk parse of every fixture, and over this fixture's
  long value on six majors that leg went from seconds to over a minute of the
  debug suite. The sizing that causes it is under STATUS's "Decisions worth
  another look".
- **Size on disk:** each major's file is about 1.1 MB in the working tree and
  tens of kilobytes compressed, the long value being a repeated pattern, so the
  repository carries little of it. Generating the schema alone takes under a
  minute for all six majors.
