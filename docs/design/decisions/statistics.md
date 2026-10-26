# Statistics

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. Per-row-group statistics: what pruning may conclude from
them, how bounds and dictionaries are gathered and stored, their memory account and allowance, how blocks merge, and how
they reach DataFusion. `pgdt-mapping`'s `statistics`, `pgdt-query`'s `prune` and `summary`, `datafusion-pgdump`'s
`statistics`.

### D75 Pruning takes only what each operator family proves
Bounds answer the ordering operators, and under an equality operator rule out "equal" but never "unequal" (D56); a
dictionary answers the equality operators; a stop needs an ordering term the root conjunction requires, on a column
sorted the way its bound closes; terms combine as if independent (D54). Rejected, though sound: a dictionary answering
ordering; a stop under `=`, or under `NOT` over the opposite operator. Reopens: a filter shape a reading shows common.
Code: `ResolvedTerm::truths`, `prune::SortedStop`. Evidence: `tests/pruning.rs`'s generated check.

### D76 Gathering never keys a bytewise value; a bound past the cap is a prefix and a successor
`text`, `character` and `bytea` bound and order by a clipped head (`gather::clip::Clipped`), never an `OrderKey`: a key
copies the whole value, and a row may run to `--max-line-bytes`. Values agreeing on the head share every byte a stored
bound reads, so bounds stay valid and only row order is lost (`Unsorted`); a truncated `max` is stored as a successor.
Rejected: the byte successor for text, which can end mid-character. Reopens: P26. Evidence: `bounds/tests.rs`'s
`a_long_texts_stored_bounds_are_on_the_right_side_of_it`.

### D77 The statistics request is the mapping pass's argument, and the metadata level is a selection
`map_file` takes a `&StatisticsRequest` and no query entry point does, as with the cancellation (D26), so no option a
query is handed reads as a request it ignores; a query's pass censuses every block and gathers nothing, which no
selection spells. A selection is a default level and overrides, the most specific winning, so the type's `Default` is
the data level everywhere. Rejected: a `ScanOptions` field; `Option<&StatisticsRequest>`, whose `None` contradicts that
default or spells it twice; refusing a size beside `METADATA`, or two equally specific entries, in the library (the CLI
refuses both, a person's intent being ambiguous there). Code: `statistics::StatisticsRequest`.

### D78 Statistics share the cache file, its identity and its `CACHE_FORMAT_VERSION`
None is believed from a cache sized unlike the live source (D20); the mtime stays advisory (D21), so a same-size rewrite
prunes against its predecessor's, knowingly. Bounds, order and dictionary are believed only under the declared type and
`COLLATE` recorded with them, and changing how a kind orders or equates bumps the version. A save streams into its own
file renamed over the cache, so no encoded copy is held. Rejected: a separable statistics file; its own semantics
version; the copy charged. Code: `stored::believed_under`, `CacheMode::save`. Evidence:
`golden_order_is_pinned_to_the_format_version`.

### D79 Bounds go in each order a semantics compares by exactly, a dictionary where it equates exactly
A scalar column keeps bounds and row order per exact order, one set where two coincide: the register's where exact, else
DataFusion's (bytewise), and DataFusion's beside an exact one it differs from; `macaddr`'s serves both (I40), and a
float's ties its zeros as `total_cmp` does, the order Arrow sorts by and DataFusion's `MIN`/`MAX` take, refining the
filter's so one set serves both. A dictionary needs exact equality alone, reaching `KD7`'s text; nested columns get
neither. Rejected: gathering a distinct count (D89 derives one); one slot; a second `macaddr` set, or a float's for its
zeros; the zeros told apart in the key, which every term equates; a kind or DDL flag stored per set; gathering keyed by
plan and semantics, which map onto no set uniformly. Code: `bounds_kinds`, `bounds_set_keyed_by`, `compare_keys_stored`.

### D81 The statistics account sums allocation sizes as they change; an instrument scope checks it
Each term is the allocator's request — capacity, a table's layout (RT11, RT12) — under one lock, a vector or map charged
ahead of its growth, in flight made when short, unmade when over. Rejected: a per-group bound, bytes never held; growth
charged after it; atomic terms, a moving charge read in neither; in-flight growth unmade both ways, faulting a table
another worker holds; walking every block; proportional slack. Reopens: a leg past its tolerance
(`statistics_account.rs`); an unseen term growing with the dump; the lock in `statistics-gathering`. Code:
`statistics::account::Charge`.

### D82 A block merges pairwise, exactly, between its minimum and its maximum, never mid-scan while a piece lives
Pairwise and exact, so serial, split and stated gathering agree: a bytewise group keeps its extremes' heads till
`finish` clips them. Mid-scan a block merges only while no piece it made lives; at `finish` one still alive joins
nothing more. The minimum reads the upper middle group, monotone in size. Rejected: merging stored bounds; coarsening a
piece at its join; waiting, as closing does, for an even count, which a window's join can leave odd and the block past
its cap; the nearest-rank median; the cap yielding to a stated maximum block by block, which the leader's split would
decide; a floor under the coarsening, a constant nothing prices. Code: `Gatherer::fit_cap`, `density.rs`.

### D85 Statistics are billed against the margin, and a block that cannot fit declines
`statistics_allowance` is what the arrangement leaves under `margin_allowance`, carved after the workers, whose count is
fixed before a byte is read while statistics accrue only as they are gathered; statistics a cache hands a pass bill the
carve as `held`, `pgdt query` carving its replay after the pass around the table's. The CLI's is `--memory`, else the
limit, else half of `MemAvailable` (RT8); none stated declines nothing (D1). A piece's decline takes its block, dense
group indices expressing no gap, and the account sees a window's pieces, so serial and parallel differ, as meant.
Rejected: a constant bound, an OOM on a wide table; coarsening to fit, a cache depending on its container; retrying
every run. Reopens: the check's cost, or declining's saving. Code: `Gatherer::decline`.

### D89 Statistics reach DataFusion on a leaf plan node, and `Exact` means the bound is the value
`TableProvider::statistics` is read by nothing in DataFusion 55 and `StreamingTableExec` answers unknown, so
`PgDumpExec` holds one rather than parenting it, a parent's child being replaceable under it. `Exact` only where the
value is the table's, `AggregateStatistics` answering an aggregate with it unchecked: a bound every group of every block
contributed, a distinct count where every group kept a dictionary of emitted text (I48). Rejected: `Absent` for a
partial bound, giving up cardinality; the union as `Inexact`, a floor inflating a join's estimate; under a filter, the
table's rows made `Inexact` as v55's file sources do, above the kept groups'; a guessed selectivity. Code:
`datafusion-pgdump`'s `statistics.rs`, `exec.rs`; `pgdt-query`'s `summary.rs`. Evidence: `tests/statistics.rs`.

### D91 A sum is kept wrapped and handed over as `SUM` wraps; a byte size is the text's length
The sum owed is DataFusion's `SUM`, which wraps, not PostgreSQL's; wrapping addition is associative, so 128-bit group
sums narrow exactly to what `SUM` returns. Text bytes are kept for every column, a cache being gathered typed whatever
later reads it. Rejected: a float's sum; a bare `numeric`'s, emitted as text; a partial sum; a `bytea`'s decoded length,
short of what `:strings` emits; sizing `:strings` by bounds and type widths, an invariant per type. Code:
`stored::Summand`, `datafusion-pgdump`'s `byte_size`. Evidence: `tests/statistics.rs`.

### D97 A group keeps a view per tier only where it holds such a value, the values its type holds the base
A column whose type cannot hold every value counts them per group and bounds and orders the values it holds; a group
holding one past a tier also keeps that tier's extremes and order apart, allocated at its first such value, so a
`StatisticsView` folds in the tiers it takes, exactly across pieces and merges (D82), believed only under D78's DDL.
Every value PostgreSQL admits keys, a timestamp from PostgreSQL's epoch (I49). Rejected: every value's bounds and the
count alone; a set per view per column from its first row; an `i128` timestamp key from 1970. Code:
`gather::bounds::KeyedGroup`, `gather::order::ViewOrders`. Evidence: `each_view_bounds_and_orders_the_values_it_takes`.
