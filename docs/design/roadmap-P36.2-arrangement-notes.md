# P36.2 notes — the arrangement

The resolution is `pgdt-io`'s `arrangement.rs`: `Arrangement::resolve` over an
`Allowance` and an `ArrangementRequest`, whose rustdoc says what each term does.
`pgdt`'s `Discovered::resolve` and the provider's `ScanBudget::draw` both call
it, and neither carves anything of its own. The seek table, the other half of
the spec's row, is 36.2.1 (the 2026-10-14 history entry).

## What landed beside the spec's words

- **`AllowanceOrigin` moved down folded into `Allowance`**, one enum carrying
  the bytes and where they came from (`Stated`, `Setting`, `Limit`,
  `HalfAvailable`, `NoneFound`). The provider's `BudgetAccount` holds one in
  place of its `allowance: Option<u64>` and `origin` pair, which could state an
  origin for no allowance, and its report no longer has an arm for that.
- **The three constructors** are `Parallelism::workers`, `Default` and the
  resolution. `within_shared` and `fit` are the crate's own; `within` is a
  test helper.
- **One divergence is kept, by a request field.** Under half of
  `MemAvailable`, `pgdt` caps its pools at that number with no reserve or
  margin, and the provider carves it as a stated allowance, for the reason
  `ScanBudget::discover` gives. `ArrangementRequest::carve_estimate` keeps
  that difference. The spec ruled out changing behaviour, so the two were not
  merged.
- **`KD29` is closed by reading once.** `ParallelArgs::discover` takes
  `Allowance::discover()` before the open, and both of `query`'s passes resolve
  from that one reading. `ScanBudget` no longer runs discovery again for each
  scan when it found nothing. Before, its `discover_for` call read the limit
  again.
- **`Parallelism::budget_bytes`** replaces the copied
  `memory_bytes().unwrap_or(DEFAULT_MEMORY_BUDGET)`.

## What the embedder rule cost

Moving the root-taking readers and the constants into `internal` means no
embedder may name them, and that includes its tests (`layering.py`). So:

- **The runtime roots moved to `crates/pgdt-io/tests/data/runtime/`**, and the
  resolution is pinned against each one in `arrangement/tests.rs`. `pgdt`'s
  tests build the `Allowance` that each root states and test only `pgdt`'s
  wiring and wording.
- **No embedder test restates the reserve.** `pgdt`'s tests read it from the
  resolution (`parallel/tests.rs`, `reserve()`), so P23 re-fitting the
  constant does not require editing them. The provider's tests use an
  allowance of 1 MiB, which is below any reserve. Its holdings test gets its
  expected budget from the resolution.

## What the next slices inherit

- **`GatherOptions::statistics_allowance_bytes` and
  `StatisticsRequest::backfill` still take the number by hand**, now from
  `Arrangement::statistics_allowance`. 36.7's back-fill takes the arrangement
  itself.
- **The statistics allowance subtracts what others drew.** The only caller
  that draws is the provider, and it never maps (D90), so no behaviour
  changed.
- **`ScanOptions` and `QueryOptions` still carry a bare `Parallelism`**,
  which is what the passes read. Provenance stays on the `Arrangement` (D64).

## Negative results

- **Keeping a public root-taking `ScanBudget::discover_in`** was refused: only
  its own test reached it, and that test is now `pgdt-io`'s
  `each_root_states_its_allowance_and_where_it_came_from`.
- **Comparing a discovered budget against a second `Allowance::discover()`**
  was dropped from the provider's tests: `MemAvailable` moves between two
  reads, so the comparison is flaky.
