# P25 — Plan answers from the map's statistics

What this phase will do and why; how it lands is its slices'. Progress is
`STATUS.md`'s checklist, never this file. **Its first slice exists to produce
evidence** — the harness and fixture shapes every later slice is checked
against — so slice numbers after it are allocation order as much as schedule.
Grilled 2026-09-24.

## Premise

**A statistic pays only where something reads it**, and DataFusion 55 reads the
provider's statistics for one whole scan at a time — never a row group's. So
this phase hands DataFusion what the map already knows, or can know from the
parse it already runs, and it ranks the candidates by query reach rather than
by how much they gather. **No new pass over the file**: every change here is
either derived from statistics a cache already holds, or gathered by the one
mapping pass that gathers the rest.

## Scope

**Same-pass gathering this phase makes** — each a change to what `gather.rs`
records, so a `CACHE_FORMAT_VERSION` bump (`decisions.md`, "D78"), and each
priced by what it lets DataFusion answer or stops it answering wrongly:

- **Whether a stored lower bound is the value it came from** (`KD39`), so a
  `MIN` over a text-ordered column answers from statistics as `MAX` already
  does. An enum's extremes are not this phase's (`KD45`, P27).
- **The zero's sign at a float column's extremes** (`KD42`), so an `Exact`
  `MIN`/`MAX` is the value DataFusion's `total_cmp` would pick: a wrong answer
  with no error, closed — and so a float column's recorded order can be
  declared, below.
- **A per-group sum** for `int2`/`int4`/`int8` (kept as `Int64`) and `oid`
  (as `UInt64`), and for a typmodded `numeric` once DataFusion's `Decimal128`
  sum is confirmed to overflow in a way the gather can reproduce value for
  value — so `SUM` over a whole table answers from statistics (DataFusion
  55's `Sum::value_from_stats`, which reads an `Exact` `sum_value` only). **DataFusion's `SUM` wraps on overflow**
  (`add_wrapping`), and wrapping addition is associative, so group sums kept
  wrapped in the widened type (`Int32` → `Int64`) combine to exactly what a
  read answers; that, not PostgreSQL's `numeric` sum, is the value owed
  ("D89"). Nothing else gets one: a float's sum depends on addition order, a
  bare `numeric` is emitted as text, and a column holding a value its Arrow
  type cannot represent (`KD8`) has none, a sum being unable to leave that
  value out the way a bound can.
- **A per-group text byte size for every tracked column**, whatever the typed
  read emits it as — one running sum of the decoded length per group, beside
  the NULL count, the gatherer already decoding every field. It is a property
  of the text, so a `:strings` read, where every column is `Utf8View`, sizes
  every column it projects from it. In the typed read a fixed-width column
  needs none: rows times width is `Exact` over an unfiltered scan, as
  DataFusion 55's Parquet source hands it, and a boolean is the same rule at a
  bit a row. An enum is its keys' width a row plus its labels' text bytes,
  `Inexact`. A text or `bytea` size is `Inexact` always, a view's bytes living
  in the shared read buffer ("D46"). A nested column stays `Absent`, its text
  being no bound on its leaves' bytes. Under a filter every size is bounded by
  the kept groups as the rows are. What reads it: the join build-side swap,
  which compares bytes before rows when both sides carry them; the
  collect-left threshold, which reads bytes before rows, so a small row count
  no longer collects a wide table; and projection and filter scaling.
  Untracked columns stay `Absent`, and so does `total_byte_size` where any
  projected column is.

**Provider answers this phase makes from what the map already holds**:

- **A pushed filter's rows bounded by what pruning kept.** The rows of every
  group pruning kept, plus the `row_count` of every block it could not
  consult, reach DataFusion as an `Inexact` `num_rows` in place of the whole
  table's; column statistics stay `Inexact` under a filter as now, and no
  selectivity is guessed on top of the bound — a guess is how an estimate goes
  wrong where a bound cannot. This amends "D89", which today copies v55's file
  sources.
- **A declared output ordering from recorded sortedness** (moved here from
  `roadmap.md`'s "Future"). A column declares one only where every block is
  sorted the same way in the set Arrow orders by, **no group holds a NULL** —
  sortedness skips NULLs and a declared ordering binds where they fall — and
  every block boundary a partition crosses is proved in order from the
  adjacent groups' stored bounds at plan time; unproved, nothing is declared.
  Partitions are not cut at block boundaries to avoid the proof, which would
  give up "D51"'s byte balance on every table for the few this serves. Each
  qualifying column is its own single-column ordering, ascending or
  descending as recorded. A float column qualifies only once `KD42`'s sign is
  gathered, below: sorted in PostgreSQL's order is not sorted in
  `total_cmp`'s.

**Refused here, filed as P26** (`roadmap.md`, "P26 — Statistics refused on
their cost, reconsidered"): a per-group bloom filter, and a per-block distinct
count sketch. Both are a new cost class — a hash of every value — and neither
is lowest-hanging; the sketch feeds only estimates.

**Refused here, filed as P27** (`roadmap.md`, "P27 — DataFusion's dynamic
filters"): consuming a hash join's or TopK's dynamic filter. It needs no new
statistic, but it moves pruning into the replay's stream, which is the core
path rather than the provider.

## A distinct count derived from the dictionaries

**Where every group of every block carries a complete dictionary for a column,
the union of their entries is that column's distinct set**, and it reaches
DataFusion as an `Exact` `distinct_count`. Anywhere a group's dictionary is
absent — overflowed past `DICTIONARY_MAX_ENTRIES`, an entry past
`DICTIONARY_ENTRY_MAX_BYTES`, a field that failed to decode, a declined block,
an untracked column — the count is `Absent`, never the union's size as an
estimate: that is only a lower bound, and a low distinct count inflates a join
estimate.

**This does not reopen D79's refusal**, which is of *gathering* a count — a
hash set or sketch being another cost class — not of deriving one from what is
already held. Its reasoning is filed beside D89 when the slice lands.

**A column qualifies only where distinct entry text is distinct emitted
value**, since DataFusion counts Arrow values. Found while grilling:

- **Excluded: `character(n)` and bare `bpchar`.** An entry is stored with its
  trailing blanks trimmed where the column emits padded text, and a bare
  `bpchar` keeps its blanks, so `'a'` and `'a '` may share an entry.
- **Owed a check before it qualifies: `timestamptz`**, which is one-to-one only
  if a dump writes every value in one session time zone — an invariant not yet
  in `postgres-invariants.md`.
- **Counts DataFusion's distinctness, not PostgreSQL's**, which is what is
  wanted: `-0`/`0`, an unconstrained `numeric`'s `1.5`/`1.50`, a `jsonb`'s
  number scales and an `interval`'s `1 mon`/`30 days` are distinct entries and
  distinct Arrow values alike.
- **An enum is emitted `Dictionary`**; whether `COUNT(DISTINCT)` over one reads
  the statistic as `MIN` does not (`KD45`) is checked when the slice lands.

## Evidence

**Plan shape is the contract.** Each slice asserts the optimized plan it
changes — an aggregate replaced by a literal, a sort gone, a join's build side
— and pairs every answer from statistics with the same query read row by row,
over the generated fixtures, as `datafusion-pgdump/tests/statistics.rs` does
(`decisions.md`, "D89"): a wrong `Exact` is a wrong answer with no error. **The
first slice is that harness**, landing no product code, and every later slice
extends it. A timed figure is owed only by a slice that claims a speedup.

**Each answer has its own oracle**:

- **An aggregate answered from statistics** — `SUM`, `COUNT(DISTINCT)`, the
  text `MIN` — against a session whose physical optimizer lacks
  `aggregate_statistics`, typed and as text, over every fixture, as the
  harness already does for `COUNT`, `MIN` and `MAX`.
- **An estimate** — the filtered row bound, a byte size — is checked as the
  bound it claims to be: never below what the scan emits, over the generated
  filters `pgdump_query/tests/pruning.rs` already draws.
- **A declared ordering** has no blind session, since a wrong one changes the
  plan rather than a literal: each partition's rows are read and checked
  sorted under Arrow's own comparator, NULL placement included, and the plan
  is checked to have dropped its sort. Where a fixture's partitions cross no
  block boundary at the harness's partition count, a higher count is run, so
  the boundary proof is exercised rather than skipped.

**The fixtures are real `pg_dump` output**, the generated `statistics` family
extended across every routine version (`roadmap.md`, "Expand the generated
fixtures freely; verify objectively wherever possible"): a sorted column
spanning blocks under load-via-partition-root, both zeros at a float's
extremes, a text minimum longer than `DICTIONARY_ENTRY_MAX_BYTES`, `int8`
values whose sum overflows, a typmodded `numeric`, a `timestamptz`, and
low-cardinality columns spread across blocks.

## Slices

**Provider answers over the caches that exist come first; the two format bumps
come last**, so a user sees every answer that needs no re-parse before being
asked for one, and a dump the size this project is built for is re-parsed
twice rather than once per gathered field. In order:

1. The evidence harness above, and its fixtures. No product code.
2. The filtered row bound, amending "D89".
3. The distinct count from the dictionaries, with the `timestamptz` invariant
   it rests on filed in `postgres-invariants.md`.
4. The declared ordering, for every qualifying column but a float.
5. First format bump: the lower bound's exactness (`KD39`) and the zero's sign
   (`KD42`), which lets a float column declare an ordering.
6. Second format bump: the per-group sum and the per-group byte size — one
   mechanism, a running sum per group beside the NULL count.
7. Third format bump, admitted after 6 landed: the text byte size kept for
   every tracked column, and a size for the typed read's booleans and enums.

**What `pgdt info` shows**: `--json`, which promises every group's
statistics, exports the three stored fields this phase adds — the lower
bound's exactness, the sum, the byte size; `--detail` is unchanged. The
derived answers — the distinct count, the filtered bound, the ordering — are
DataFusion's alone, `pgdt query` planning through none.

## Facts found while grilling

- **Pruning is settled before `scan()` returns** (`TablePartitions::plan` →
  `prune_blocks` → `prune::prune_block`), from the statistics in memory with no
  I/O, and every kept group's row count is in hand there (`RowGroup::rows`).
  Nothing sums them: the plan notes carry groups and bytes, and a pushed filter
  hands DataFusion the whole table's rows as `Inexact`
  (`datafusion-pgdump/src/table.rs`, `scan`). A block without statistics
  contributes its `row_count`. Where a sorted block stops is found only as rows
  are read, so a plan-time bound cannot include it.
- **Recorded sortedness** is per block, per column, per order set; ascending,
  descending or unsorted, non-strict, **skipping NULLs**. DataFusion's declared
  ordering binds NULL placement and every pgdump field is nullable, so only a
  column with no NULL in any group could declare one. A partition may span a
  block boundary and nothing records order across blocks, though adjacent
  groups' bounds can prove it at plan time. `KD42` reaches it too: a float
  block sorted in PostgreSQL's order need not be in `total_cmp`'s.
