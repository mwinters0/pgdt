# P10 — Per-row-group column statistics

What this phase will do and why; how it lands is its slices'. Progress is
[`../status/STATUS.md`](../status/STATUS.md), "P10 progress", never this file.
**Its first slice exists to produce evidence** — the fixture shapes every later
slice is tested against — so slice numbers after it are allocation order as
much as schedule. Grilled 2026-09-14
([`../status/history/2026-09-14.md`](../status/history/2026-09-14.md), "P10 is
grilled and sliced").

## Premises

**This phase is not shaped by any one dump.** Its design argues from what
PostgreSQL and `pg_dump` do in general — heap-order output, MVCC updates
relocating tuples, collation-ordered text — and from the shapes the generated
fixtures can hold. koji-derived figures in the sketch (its row count, the
statistics volume computed from it, its expected sorted columns, its scan rate)
are illustrations and bind nothing here; a decision needing a number takes it
from a fixture-backed figure or from arithmetic over a stated shape.

**A parse is paid to accelerate queries, so it accelerates them maximally.**
Gathering statistics converts the mapping pass into a parse of the tracked
columns; having paid that, anything derivable from what was gathered is derived
once and persisted rather than recomputed by every query that could use it.

## The consumer ships in this phase

Statistics are consumed by the query replay, in the library and in `pgdq
query`: a row group whose statistics prove no row can satisfy the filter is
never read. Statistics without a consumer are unverifiable, and this phase's
failure mode is a silently wrong answer, so **the consumer is the correctness
check: a pruned query returns exactly what the same query returns unpruned**,
over every fixture.

The replay already has the shape this needs — an interior segment starts at
the first row after an arbitrary byte and stops after the row straddling its
limit, and nothing downstream (batch flushing, `ResumeToken`, the column-count
check, the census) counts rows it did not read — so a pruned query is a segment
list with gaps. Two things it lacks: filter resolution happens per block at
activation rather than at plan time, and no function compares two texts outside
a row (`predicate.rs`'s `order_key`/`compare_keys` are private and row-free
only by accident).

## A row group is a byte range

**Group `k` of a block is the rows whose first byte lies in `[k·N, (k+1)·N)`
of the block's data**, `N` tunable and independent of the read chunk size. Not
a fixed row count: the parallel mapping pass cuts a block's interior into
concurrent pieces at LF boundaries, a piece numbers its rows from zero and
learns its count only after the fact, so no piece can know where global row
`8192·k` falls in it — while every row's absolute offset is in hand, and a row
straddling a cut already belongs to the earlier piece, so byte placement is
consistent under any cut. It is also the unit pruning saves: device bytes, not
rows.

**`SparseRowIndex` is struck**, with its reserved `CopyBlock::sparse_index`
slot: a list of byte-aligned groups carrying their row counts and byte lengths
is the index, no consumer needs seeking to a row number, and `ResumeToken` is
opaque. The reserved `column_stats` slot is what fills, renamed to fit, and
`decisions.md`, "D34" is amended where it lands.

## What is gathered, and what is derived

Per group: **its row count and its byte length** — the extent from its first
row's start to its last row's end, which a straddling row carries past `N` —
both recorded during the parse. Per group and per tracked column:

- **`null_count`**, answering `IS NULL` / `IS NOT NULL`.
- **`min` / `max`**, for the columns "Which columns get which statistics"
  names.
- **a dictionary**, for the columns "Dictionaries" names.

Distinct count stays out: a hash set or sketch is a different cost class.

**Sortedness is derived, and once derivable it is stored at the block level.**
The sketch ranked a gathered block-wide sortedness flag ahead of min/max; that
is reversed. Strict physical order is fragile in general — `pg_dump` writes
heap order and any `UPDATE` writes a new tuple version elsewhere — and on nearly
sorted data per-group bounds already hand a range predicate its byte ranges from
memory.

What it promises is **row order**, not only group order, as a tri-state per
block and column: **`Ascending`, `Descending` or `Unsorted`**. It is gathered
directly rather than assembled from per-group facts, because a block's
statistics are all or nothing — the cache never holds a half-scanned block and
a back-fill re-reads a whole one — so two running flags per column (never
decreased, never increased), one comparison of each non-null value against the
previous, settle it by the block's end. A leader piece join compares the
carried last and first values. Equal neighbours are allowed, and NULLs are
outside the order, located by `null_count`, since physical order puts them
nowhere in particular. A column whose non-null values are all equal, or number
at most one, holds both flags and is stored `Ascending`, the early stop being
correct either way; a join whose carried values are truncated past deciding is
`Unsorted`. Row order is what lets the replay stop inside a group at the first
row past a bound rather than reading the group out.

The tri-state is persisted with the block rather than re-derived by every query
— a deliberate derived-and-stored fact against `decisions.md`, "D34" ("stores
no fact twice"), which this phase amends. It exists only for columns that get
bounds.

**A value that does not key leaves its group without bounds on that column, and
its block `Unsorted` there**, as a value past the cap does ("Very large
values"): a bound or an order that omitted it would not cover its row, which the
server may keep — a wrong answer, not a lost error. Its null count and
dictionary stand. A hand-written test pins it; the generated check cannot hold
such a value, the unpruned query raising on it
([`../status/history/2026-09-14.md`](../status/history/2026-09-14.md), "A skipped
group raises nothing").

## When a statistic is believed

**The identity check is mandatory, and identity is the stored size.** No
statistic is believed from a cache whose recorded stored size is not the live
source's — the same refusal the structural cache already makes
(`decisions.md`, "D20"). The mtime is not part of that identity: it cannot be
controlled in every environment a dump file passes through, so it stays the
advisory diagnostic it is for structure (`decisions.md`, "D21"), and the
sketch's size-and-mtime check is narrowed to size. The consequence is accepted
knowingly: a same-size file rewritten in place is pruned against statistics
that describe its predecessor.

**What a statistic means is answered by what the cache already records.** Each
column's statistics carry the declared type text and the `COLLATE` clause they
were computed under (L1 vocabulary, `decisions.md`, "D74"), and any change to
how this build compares a type's values is a persisted reshape that bumps
`FORMAT_VERSION` (`decisions.md`, "D22"). No second semantics version exists.
A query uses a column's statistics only where that column's current
`ComparisonPlan` still orders (for bounds) or equates (for a dictionary)
exactly under the recorded declared type and collation.

**That rule carries a check.** A golden-order test sorts every value in the
committed oracle fixtures under each comparison kind and pins the ordering's
digest beside `FORMAT_VERSION`'s current value, so a change to how any kind
orders fails until the version is bumped and the digest re-pinned. It lands in
the slice that first persists a bound.

## Which columns get which statistics

**`min` / `max` and sortedness go to exactly the columns the comparison
register orders exactly** — a `ComparisonPlan::Compared` whose divergence, if
any, does not affect ordering (`decisions.md`, "D40", "D55"). That is the
integers, `numeric`, the date and time types, `interval`, `uuid`, `bytea`,
`inet`/`cidr`, `macaddr`, enums, `name`, and `text`/`varchar`/`character` under
`C` or `POSIX`. **Floats qualify under PostgreSQL's own order**, which
`OrderKey` already implements (`NaN` above everything, `-0.0` equal to `0.0`):
the sketch's open float policy is answered by the server's, and a bound is taken
and compared under the same key a filter uses.

Text under any other collation (`KD7`), `json`/`jsonb`, nested columns and
unmodelled types get no bounds. **Every column gets `null_count` and the row
count**, so `IS NULL` / `IS NOT NULL` prune on all of them.

## How gathering is asked for

**`pgdq parse` gathers every statistic by default**; `--statistics` takes an
optional selection (tables or `schema.table.column`s) to narrow it, and
`--statistics none` disables it. This reverses the sketch's opt-in rule, under
this phase's premise: the parse is the price, and the default pays it.

**The library's mapping pass defaults the same way**, gathering every statistic
unless its caller states none; how many workers and how much memory it takes
keep their conservative library defaults ([`roadmap.md`](roadmap.md), "A parse
does all the work a later query could use"). **The request is an argument of
the mapping pass alone**: no query entry point accepts one, so no option a
query is handed reads as a request it ignores.

**Asking for statistics a mapped block lacks re-reads that block.** `parse`
otherwise resumes from `scanned_through`, so a complete cache would never
acquire statistics; a block whose structure is known but whose requested
statistics are missing is re-read on its own, saving as it goes and resumable.
In the library this is the mapping pass's request plus a per-block back-fill
entry point.

**A query gathers nothing in this phase**; it consumes what `parse` stored.
Gathering what a query already reads is P21's ([`roadmap.md`](roadmap.md), "P21
— Statistics gathered by a query"), since a query reading part of a block needs
a statistic to tell "not yet gathered" from "none available" per group, where a
parse gathering whole blocks needs it only per block and column.

## One cache file

Statistics are persisted in the same cache file as the structural map; where
inside it they sit is an implementation choice. The sketch's separate file,
discardable independently, is not taken.

## Resident memory grows, and is optimized later

**Resident memory is expected to grow in this phase and to scale with the size
of the dump**, statistics being held per group per tracked column. The
flat-memory goal holds block-level structures flat and does not reach them:
they are drawn from row values, whose widths vary ([`roadmap.md`](roadmap.md),
"Project goals"). **This phase raises no library constant for it**:
`MEMORY_RESERVE` is a flat subtraction that bills no statistic, and until
`statistics-gathering` no harness run gathers under a limit. **That figure's
legs state a generous container limit of their own**, so no OOM interrupts the
phase. Bounding what statistics hold, and the defaults and expectations that
follow, are P20's ([`roadmap.md`](roadmap.md), "P20 — Statistics memory and its
measured expectations"); the evidence is
[`../status/history/2026-09-14.md`](../status/history/2026-09-14.md), "P10 is
grilled and sliced".

## Gathering across the layers

**An observer trait defined in L1, implemented at L4** — the shape
`decisions.md`, "D74" already assigns this phase's parse step. The mapping
builder and each leader piece stay type-blind and hand every row's offset and
raw bytes to a per-block observer; `map_forward`, which holds the database's
preamble at its first `CopyStart` before any of its rows, builds that observer
from `resolve_columns` and the comparison key the filter uses. Leader pieces
each get their own and merge in file order, the fold the census already takes;
a group can straddle a cut, so a piece carries each column's first and last
non-null value for the join.

**Bounds persist as unescaped field text**, the spelling `pg_dump` wrote but
for a `character` value's trailing blanks, which its comparison ignores and its
declared length restores — L1 vocabulary. A query re-keys a candidate group's bounds at plan time, which is
two parses per group per filtered column against reading the group. *Rejected:*
persisting binary comparison keys, an L4 conclusion `decisions.md`, "D74"
forbids persisting.

## Group size

**`N` defaults to 1 MiB**, chosen coarsely and refined in P20, and is stated
with `pgdq parse --statistics-group-size`. The arithmetic that bounds it, for a
dump of `D` bytes with `C` tracked columns: `D / N × C ×` (two bound texts plus
the counts). Smaller `N` prunes more finely and costs memory in
proportion. The size is recorded per block; a block gathered at a size other
than an explicitly stated one counts as lacking statistics and is re-gathered,
while an unstated size re-gathers nothing. A row longer than `N` leaves groups
in which no row starts, which are empty rather than skipped.

## Pruning

**A group is skipped only when `True` is impossible at the root.** Each term is
evaluated over a group's statistics to the set of truth values some row of the
group could produce — for `col < v`, `True` needs a non-null value with
`min < v`, `False` a non-null value with `max ≥ v`, `Unknown` a positive
`null_count`; a column without bounds can produce whatever its non-null rows
might — and the sets combine through `And`/`Or`/`Not` as three-valued logic
does, so `NOT` stays sound where a "may match" flag would not. Equality on a
kind compared by decoded value uses the same key the filter does.

**A skipped group raises nothing.** A decode failure surfaces only where
evaluation reaches it (`decisions.md`, "D54", amended where the consumer
lands), so a value this build cannot read in a skipped group goes unreported
where the unpruned query raises it; the rows returned are the same. *Rejected:*
never skipping a group marked as holding a value that did not key — a nested
column is never keyed while gathering, so the mark misses `KD2`'s values — and
never skipping where the filter names an unkeyed column, which ends pruning for
every filter naming one.

**Early stop.** Where the root is a conjunction holding an ordering term on a
column whose block-level sortedness is known, the replay stops at the first row
past the bound, inside a group if need be.

**Visible and switchable.** Pruning is settled before any byte is read, so it is
a new `PlanNote` kind stating the groups and bytes skipped out of the total, and
`plan_partitions` balances readers over the bytes that remain. `pgdq query
--statistics none`, and its library option, runs unpruned — which is also how
the pruned-equals-unpruned check runs from the CLI.

**An early stop is found while rows are read**, after the plan's notes are
settled, so it is **reported after the fact**, per block: a stream lists each
block it replayed with a stop planned and the bytes its stopped pieces left
unread, readable once drained, and `pgdq query` merges the lists of its
sub-streams by block into a `note:` printed only where a stop fired. Per block,
since each piece of a split block past the stopping row stops at its own first
row and a per-stream count would count the block once per sub-stream. Bytes, so
they add to the pruning note's; exact serially, and short by at most one row's
tail per piece boundary when split, the row straddling a piece's limit being
its own and never read to find. A library caller can tell a block with no stop
planned from one whose bound was never reached. Pruned, a stop saves at most the
rest of the group its bound falls in, one per block, a group wholly past it
being skipped by its bounds
([`../status/history/2026-09-14.md`](../status/history/2026-09-14.md), "The
early stop is reported after the fact"). *Rejected:* counting rows past the
stop, which are never read and so only estimated; a zero where a stop was
planned and not reached, the pruning note already saying statistics were
consulted and a stop never reached having saved nothing; exact split bytes by
reading each piece past its limit. *Reopens:* an account needing exact bytes,
which each piece reporting its first row's start alone would give a caller
merging every sub-stream.

## Shapes the phase must hold

Stated by the maintainer as datasets this phase serves, not as one dump's
properties:

- **Sortedness is a tri-state per block and column** — above.
- **Rows with extremely large `varchar` values, on the order of 256 MB** —
  below.
- **Dictionary statistics, for pruning**, at most **64** entries per row group
  and no entry larger than **256 bytes** — below.

## Very large values

**`ScanOptions::max_line_bytes` keeps its 64 MiB default and becomes
`--max-line-bytes` on `parse` and `query`.** A longer line is
`Error::LineTooLong` today with no flag to raise it, so a dump holding a
256 MB value is refused before statistics enter into it; the flag is the
remedy, and a caller who has such rows states it.

**No stored value exceeds 256 bytes** — one constant shared by bounds and
dictionary entries. For a kind compared bytewise (`text`/`varchar`/`character`
under `C`/`POSIX`, `name`, `bytea`) a longer `min` is stored as its prefix, a
valid lower bound, and a longer `max` as its prefix with the last byte
incremented after dropping trailing `0xFF`s, a valid upper bound marked inexact
so it is never read as a value. For a kind compared by decoded value, a value
past the cap leaves that group with no bound on that column; its other
statistics stand.

**The fixture is a value larger than both `N` and the read chunk**, which
exercises the empty groups, truncation, and a long row carried across a piece
join; the 256 MB case belongs to the performance generator, not the repository.

## Dictionaries

**A dictionary is a statistic, never an Arrow encoding.** Making a column's
Arrow type follow its statistics would change a query's schema between a cold
and a warm cache, a type-mapping decision `decisions.md`, "D37" does not make
from statistics.

**Any column whose equality is exact gets one**: a `ComparisonPlan::Compared`
whose divergence, if any, does not affect equality, nested columns excluded.
Dictionary pruning needs equality alone, so it reaches text under every
deterministic collation — exactly the `KD7` columns bounds must skip. It answers
`=`, `!=` and `IS [NOT] DISTINCT FROM` through the truth-value sets under
"Pruning"; a kind compared by decoded value tests the literal against each
entry's key, so `1.5` finds `1.50`.

**More than 64 distinct texts in a group, or any value past 256 bytes, leaves
that group and column without a dictionary**; its other statistics stand. A
`character` entry is stored and measured without its trailing blanks, as its
bounds are, so a column wider than the cap keeps a dictionary where its values
are short ([`../status/history/2026-09-14.md`](../status/history/2026-09-14.md), "`character` statistics drop their padding").

**Entries are interned per block and column**, each group holding up to 64 small
indices into that table, so a value repeated across groups is stored once. The
worst case — every group contributing 64 fresh 256-byte entries — is up to
16 KiB per group per column, and is P20's to bound.

## Statistics are never copied per save-gate opening

Statistics live in the one cache file, so they are part of `DumpIndex`, whose
span list is cloned twice at every opening of the save gate (`snapshot`, then
`splice`) and once more by `cache::save` — the clones `KD5` names, which the
save throttle does not count (`decisions.md`, "D62"). **A block's statistics
are held by shared ownership**, so those clones copy a reference and `save`
encodes through it; a long `parse` is then not quadratic in statistics volume.
`KD5` is not absorbed and stays `(c) unowned` at its statistics-free size.

## The correctness check

**Gathered at a tiny group size.** At the 1 MiB default every fixture block is
one group and pruning proves nothing, so the check gathers at a group size of
tens of bytes — `--statistics-group-size` accepts it; only the default is
coarse.

**Filters, generated.** For every column carrying statistics, terms under every
operator the filter supports, with literals drawn from the values stored as
bounds and dictionary entries and their neighbours, from the committed
comparison oracle's literals for that type (`decisions.md`, "D70"), and `NULL`
tests; then seeded random `And`/`Or`/`Not` trees over those terms, a fixed
number per fixture. **The assertion is identical row sets in file order**,
pruned against `--statistics none`, at one worker and at a parallel count.

**Pruning actually happening** is pinned apart from that, by hand-written tests
whose expected skip is known by construction — a sorted column under a range
filter, a dictionary under an absent literal.

## Reporting

`pgdq info --detail`, from the cache alone (`decisions.md`, "D61"), states per
table and column whether statistics exist and over how many of its blocks, the
**group size in both bytes and rows** — the configured `N` beside the observed
bytes and rows per group — the sortedness tri-state, and the share of groups
carrying bounds and carrying a dictionary. **`--json` exports what the cache
holds instead, every group's statistics included**, compact and with no rollup:
its reader is `jq` or a script, which sums a rollup itself, and it promises
neither legibility nor a stable shape
([`../status/history/2026-09-14.md`](../status/history/2026-09-14.md),
"`--json` exports the cache, group values included"). A `parse` that back-fills
states on stderr how many blocks lacked the requested statistics and were
re-read.

## Measurements

**Every existing figure that times `parse` passes `--statistics none`**, and
`scripts/test_measure.py` asserts it, so the default this phase changes does not
silently re-time what those figures measure.

Two new figures, each with its instrument:

- **`statistics-gathering`** — `pgdq parse` with statistics against
  `--statistics none`, whole-file through the CLI, warm, on the existing
  generated scan-throughput inputs, in a container limit of its own chosen
  generously. It prices the parse; resident is recorded beside it and not
  refined.
- **`statistics-pruning`** — `pgdq query` under a selective range filter on a
  sorted column, under an equality filter against a dictionary, and under a
  filter returning few rows over a column its statistics cannot narrow, each
  with and without `--statistics none`, warm, on a generated file carrying a
  sorted id column and a low-cardinality column. The first two price what
  pruning buys, the third what carrying and consulting statistics costs where
  it buys nothing, which every filtered query pays by default — so the third
  filter also runs against a cache written by `parse --statistics none`, the
  cache being decoded whole whatever the query states. The low-cardinality
  column's values arrive in runs, so the dictionary leg is a best case, and the
  table prints the groups each query skipped beside its timing
  ([`../status/history/2026-09-15.md`](../status/history/2026-09-15.md), "The
  pruning figure prices its cost as well as its best case"). *Rejected:* values
  scattered among the runs, a mix no distribution in the spec gives a basis for.

Re-taking `reserve` and deriving `MEMORY_RESERVE` from readings is P20's; this
phase leaves the constant as `reserve` chose it.

## Inbox, drained

- *The census avoided the L1/L2 injection* — folded into "Gathering across the
  layers": the census showed a type-blind recorder in L1 works, and bounds are
  the case that genuinely needs the injected step.
- *A column's order and its Arrow type have come apart* — folded into "Which
  columns get which statistics", which keys on `ComparisonPlan`, never on the
  Arrow type.
- *This phase is the second half of `xz-seek`'s publication gate* — stale for
  P10, which adds no call into the crate; refiled to
  [`roadmap-P14-remote-input-inbox.md`](roadmap-P14-remote-input-inbox.md).
- *The sparse row index is this phase's outright* — folded into "A row group is
  a byte range", which strikes it.
- *`KD5`* — folded into "Statistics are never copied per save-gate opening";
  not absorbed.
