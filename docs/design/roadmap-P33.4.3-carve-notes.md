# P33.4.3 notes — a read over mapped blocks carves its longest line

**A replay is planned under an arrangement carved around its longest line.**
`Table::line_charge(read, &options)` answers that line's charge, and
`pgdt query` and each provider scan hand it to
`ArrangementRequest::line_charge` before resolving the arrangement that
`Table::plan(read, options, &arrangement)` then takes whole: its parallelism
cuts the replay, and its ledger is the one the replay's held lines draw on
(D160). `QueryOptions::parallelism` and `TablePartitions::drawing_on` are
gone. Under today's line limit the earliest line is still admitted alone past
the room ([`roadmap-P33.4-ledger-notes.md`](roadmap-P33.4-ledger-notes.md)),
so nothing a run answers moved but its count, which a line longer than its
chunk now lowers where the ceiling binds.

## What the next slices inherit

- **The charge is the replay's own formula** (`plan.rs`'s `charged`, which
  the replay's draw reads too): the block-recorded `LongestLine::len`, held
  whole, plus the query's one factor of it, and zero where it does not pass
  the read's chunk, which no reader draws for. A projection reading no long
  column is still charged the line's bytes, the factor being the query's,
  not the column's.
- **The floor (33.5) reads the same number**: a read whose longest line one
  reader cannot hold is refused before it reads a byte, and
  `Table::line_charge` is the charge to hold against what the cap leaves
  `at(1)`. It reads no byte and refuses what `Table::schema` refuses.
- **The provider carves the ledger inside `ScanBudget::draw`**, which now
  keeps the `Arrangement` (`Draw::arrangement`). What a scan's ledger lends
  is the margin's room less the live scans' budgets; another live scan's
  held lines are not billed against it, as no scan's statistics are.
- **`pgdt query` says what it carved**: `resolved the arrangement` ends
  `line_bytes=…` beside `held_bytes=…` where the charge is not zero
  (`Discovered::resolve_replay`); the manual's "`--jobs` and `--memory`"
  says so.
- **Tests state a replay's parallelism through an arrangement**:
  `pgdt_fixtures::arranged_as(Parallelism)` makes one at the budget a test
  states, a ledger without limit, through `pgdt_io::internal::arrangement_of`
  behind pgdt-io's `test-support`; `tests/common`'s `Cold::replay` is the cold
  query's. An embedder, and `tests/embedder.rs`, has only
  `Arrangement::resolve`.

## Negative results

- **The charge is per block, not per row group**: the map records a block's
  longest line, so a read whose pruning skips the group holding it is still
  charged for it, and so is one whose batch and filters read no field, a
  dynamic filter planned after the arrangement being free to read one (D160).
- **The charge plans the table twice**: `Table::line_charge` resolves every
  block for the factor before the arrangement, and `Table::plan` again after
  it. Both read no byte; neither is on a row's path.
- **No count pair was taken**: no row's or byte's path changed, the replay's
  draw computing what it did through `charged`.
- **`long-line-resident`'s query legs** now carve their line, which can lower
  their budget beside their stated reader; the figure is re-taken at 33.7.
