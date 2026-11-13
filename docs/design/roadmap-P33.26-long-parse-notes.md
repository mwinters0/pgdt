# P33.26 notes — a long field's parse priced

**33.13, 33.18 and 33.19 each resolve a rise on a long field's parse, and the
span they sit in is every row's account**: the three pairs, the span and
every rise placed by callgrind are under `runs/count-pair-33.26-20261112/`,
its `callgrind/reading.txt` the reading, the legs kept in
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.26/`
([`decisions.md`](decisions.md), "D108", "D155"). Each placement reproduces
on both rep pairs; D108's falls are unplaced.

- **`--count-pair` has four more standing rows** (`COUNT_PAIR_SHAPES`): the
  data-level parse and a strict one (`STRICT_PARSE`, the data level's
  unwrapped `parse` with `--postgres-invalid-values strict`) over
  `long_escaped_32` and `long_bytea_32` (`COUNT_PAIR_LONG_INPUTS`), each
  under `STATISTICS_MEMORY` as the control's data-level row is. A 32 MiB
  line is past a read chunk and under today's 64 MiB line limit, so every
  commit back to `6dd4523c` reads it unflagged. `--callgrind-recipe` has a
  block for each.
- **`--count-span <base>..<tip>` counts a run of landed commits**, both legs
  from checkouts as a named change's are, refused unless `base` is an older
  commit on `tip`'s first-parent chain (`count_span_named`), and its log
  says it is an account over that many commits, never one change's price.
  `--count-pair <parent>..<change>` still refuses anything but the first
  parent. Held by `test_measure.py`'s `CountPair`.

## Where each pair's rise is placed

All three are changed code; none is layout.

- **33.13 (`6d4a1cf4..e3a5ce6d`)**: an escaped field past `LONG_FIELD_HEAD`
  folded a byte at a time off `Unescaped` — `TextFold::push`,
  `Gatherer::observe_row`, `ColumnGatherer::observe_text` and a `bytea`'s
  `bounds::Canonical::of` — costs more than the one `unescape_owned` copy it
  removes. It rises on the control's data-level parse and on every long
  row's but the strict escaped one, which falls.
- **33.18 (`a36e8580..62e8875c`)**: `Unescaped::next` is `Unescape::step`'s
  state machine, nine more instructions an escaped byte wherever it is
  inlined — the gatherer, `unescape_owned`, a query's `bytea` in
  `push_row` — so every row but the metadata-level parse rises. A strict
  check's `bytea` is walked a byte at a time by `FieldWalk` and
  `Check::byte` where `bytea_reads` read a slice, the largest single rise
  (strict `long_bytea_32`, +169%), as 33.18's notes expected.
- **33.19 (`c356be7a..e80138b2`)**: a streamed field is read run by run,
  and `Unescape::run` hands each escape's byte on as a run of one, so
  `LongRead::text` runs its whole fan-out — the head, `TextFold::push` and
  its `from_utf8`, a `bytea`'s forms — twice an escape: 33.55M calls over
  `long_escaped_32`'s 16.8M escapes. A strict parse feeds `FieldWalk` the
  same runs through a second `Unescape`. The control's held rows take the
  same fold (`Gatherer::field`), and the metadata-level parse's small rise is
  `RowFold`'s adapter in `map_forward`. 2026-11-11's unplaced candidate is
  this.

## The account

**On every row the three pairs miss the span by more than its spread**, and
the residual is placed, not fitted: per row the span's median change is the
three pairs' plus four gaps between adjacent legs — `6dd4523c..6d4a1cf4`,
`e3a5ce6d..a36e8580`, `62e8875c..c356be7a`, `e80138b2..93428ef7` — each
gap's own two legs callgrinded, so its candidates are its commits alone
rather than the span's 58. Where two candidates change one function, a pair
on a candidate separates them (33.3.1, 33.3.2, 33.3.3, 33.21; each in the
run directory). Every residual has a name (`reading.txt`, "The account"):

- **33.9** is the largest: `unescape_owned` read through `Unescaped`, and a
  query's and a strict check's `bytea` decoded once off it, falls on every
  row with an escaped or `bytea` column — up to 2.2e9 on a strict
  `long_bytea_32`.
- **33.3.1** (both parses, every query row), **33.11** (`CopyScanner`
  keeping each block's longest line), **M276** (the datetime readers),
  **33.3.2** (the query rows, falls) and **33.3.3**, by pair or by the one
  diff touching the function.
- **M278** (`append_typed`, the query rows' falls); **M277 with M278**, the
  data-level parse's 13.9M in `Gatherer::observe_row` around their
  refusal quote, unseparated.
- **33.13.3**: `Check::byte`, two instructions a byte of a strict
  `long_bytea_32`. 33.21's pair resolves no row.
- **33.16**, its own pair within 1M of the gap on the strings, typed and
  `arrays` rows, accepted there; and the `pgdt sql` row's other 54.6M,
  `ResolvedTerm::eval` testing per row which reader an escaped field takes,
  a test each of **33.14, 33.14.1 and 33.15** adds, unseparated.

**The two joint names stand unsplit**: a pair on a candidate earns its run
only where the split changes what is done, and each of `ResolvedTerm::eval`
and `Gatherer::observe_row` is reworked or accepted whole by one slice.

## What 33.27 inherits

- **The three rises, each placed and neither reworked nor accepted here**;
  the slice reworks or accepts each against its parent's pair (D108).
- **The residual's rises too** (D155), each placed and named under "The
  account" and none judged here: 33.13.3's and M277's with M278's, in
  functions 33.27 already reworks; the scanner's, 33.3.1's, 33.3.3's and
  33.11's, to 33.28, ahead of the ledger's carry built there; M276's and
  `ResolvedTerm::eval`'s, off the ledger's path, to 33.29. 33.16's was
  accepted by its own pair; falls owe nothing.
- **33.19's is the largest on an escaped field and is not inherent to a
  fold**: one `from_utf8` and one fan-out per one-byte run is per escape,
  not per byte read. 33.13's and 33.18's are per byte.
- **The four long rows are standing**, so the slice's own pair reads them.

## Negative results

- **The query rows' callgrind runs first ran cold**: a query row's recipe
  block builds the cache with an untimed `parse` before each run, and a job
  written from the run's three lines alone mapped the dump inside the
  counted run. The gates still read near 100% on most segments, which hid
  it; the arrays row's gap read 66%. Every query run was re-taken warm.
