# P6 — DataFusion integration: notes

The consolidated notes of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md),
written at the wrap. What landed is `../status/STATUS.md`'s two DataFusion
capability rows; how it works is the code's rustdoc. This keeps what a later
phase, the next DataFusion major and the keystone inherit, and the negative
results the code cannot record.

## For the keystone

- **The phase's decisions are its spec's, not yet the register's.** The
  slices added D88 and D89 and amended D40, D59 and D79 in place, and no
  other `D<k>`, the register sitting at its cap throughout. What the spec settled
  and the register does not yet hold: the complete-cache-only provider and
  its rejected mapping fallback; a catalog per database, named by the user
  only where the file names none; one schema per table and a list-less block
  copying nothing, with both rejected alternatives; cancellation by drop
  alone; one session budget, its holdings off the margin's ceiling and at the
  floors off the budget; the `pgdump.*` settings and `SET pgdump.memory = 0`;
  the binary as a marked copy of `datafusion-cli`'s `main.rs`; the one sink,
  plan notes at planning and scan counts as metrics, and a budget-quoting
  note's account and levers. Each shape's local reasoning is on its rustdoc
  (`ScanBudget`, `Parallelism::within_shared`, `PgDumpSettings`,
  `BudgetedPlanNote`, `PlanNote::levers`, `factory.rs`, `StderrSink`,
  `DumpArg`, `PgDumpTable`).
- **Every judgement call the slices made past the spec was reviewed** with
  the maintainer and filed; none stands under "Decisions worth another look".

## At the next DataFusion major

- **`datafusion-cli-pgdump/src/main.rs` is upstream's 55.1.0 `main.rs`**
  without its tests, reformatted by this workspace's `rustfmt.toml`, each
  addition marked `pgdump:`. Re-copying is diffing upstream's new `main.rs`
  against 55.1.0's and re-applying the marked lines; the binary keeps
  upstream's `mimalloc`.
- **Tests that fail on purpose when upstream moves**: `tests/settings.rs`
  asserts `RESET pgdump.memory` is refused (`ConfigOptions::reset` reaches only
  `datafusion.` keys in 55), and `tests/pushdown.rs`'s
  `a_nested_column_s_arrow_notes_are_datafusion_s_comparison` pins
  `compare_op_for_nested`'s NULL-first order and unnormalized `-0`, which
  upstream marks `TODO: make SortOptions configurable`.
- **The ADBC floor page is tied to the pin**: `scripts/floor_mapping.py` fails
  if `../manual/datafusion-cli-pgdump.md` does not name the
  `adbc-driver-postgresql` release `scripts/pyproject.toml` pins.
- **What 55.1.0's optimizer hands a provider** (`tests/pushdown.rs`'s
  `sql_pushes_down_what_the_library_answers_and_answers_alike`): a literal's
  cast onto the column's type is unwrapped; an `IN` list of three or fewer
  arrives as `=` terms, a longer one as a set, which on a float makes no `-0`
  into `0` and so is not pushed; a literal that cannot be cast keeps a cast
  around the column and is not pushed.
- **`datafusion-cli` parses `LOCATION` as a `ListingTableUrl`** and registers
  an object store for its scheme before the factory runs; harmless, the
  provider reading through `Origin::resolve`.

## Comparison in Arrow semantics

- **Eleven kinds order and bound as Arrow does by construction or by their
  output format**: `Bool`, `Int`, `UnsignedInt`, `Decimal`, `Date`, `Time`,
  `Timestamp`, `Uuid`, `Bytea`, `Text` (bytewise under any collation) and
  `MacAddr` (`macaddr_out` writes fixed-width lowercase). That is what makes
  `ARROW_AGREEMENT` (`predicate.rs`) hold in general; the oracle walk only
  holds it over its population, whose `inet`/`cidr` values agree by
  coincidence without `SUPPLEMENT`'s `9.0.0.1` and `9.0.0.0/8`.
- **The library's walks emulate `apply_cmp` and do not call it**; the check
  against DataFusion itself is `datafusion-pgdump/tests/pushdown.rs`.
- **A literal not spelled as the server writes misses in Arrow semantics**
  (`ValueAsText`, `PaddedText`): the oracle's literals are the server's own
  spellings, so only a unit case pins it.
- **A value the emitted type cannot hold still answers** (`KD8`): it keeps its
  rank, so a pushed filter can drop a row whose unpushed read raises. Any
  "pushdown on and off answer alike" check projects only columns that decode.
- **`Timestamp` with no zone is not in the pushdown walk**: its one fixture
  column holds `infinity` on every major. `timestamptz` shares its decoder and
  renderer.

## One schema per table

- **`pgdump_query/tests/data/edge_cases.sql` keeps a list-less block with
  non-empty rows**, a shape `pg_dump` never writes, as the scanner's fixture
  for a `\.` row; `public.no_column_list` is refused as a query target.
- **The mapping pass still grows a block's census for a row wider than its
  header**; nothing reads the extra entries, and stopping it would change the
  mapping path for malformed input alone.
- **The reordered, zero-column and generated-only fixture tables are in the
  `partitions` schema**, not `edge_cases`, which about thirty test files read.

## The provider

- **A table whose `COPY` header names no schema is listed under `public`**,
  hiding a real `public.<same name>` beside it; only a hand-written file
  reaches it.
- **One table whose plan refuses fails `SHOW TABLES`**, which calls `table()`
  on every table; `pg_dump` cannot write that shape (I5).
- **A compressed scan at one reader loses block decode** once the session's
  holdings push its budget below a block (D4); unmeasured, and no `.xz`
  fixture reaches it through the provider.
- **Where the budget already passes the margin's ceiling with nothing held**
  — one reader at `at(1)` on a narrow allowance — the holding comes off it
  whole and the margin is still passed by that excess, as `pgdt`'s one-reader
  floor is.
- **A dump's sources are shared by concurrent scans**, each announcing its own
  draw, so a source's pools follow the last announcement (`KD38`, `KD41`).
- **`cargo build -p pgdt` is unaffected** by the DataFusion tree: Cargo
  unifies features only across the packages it builds.

## Statistics handed to DataFusion

- **A float's extremes are gathered in PostgreSQL's order**, where `-0` and
  `0` tie, and DataFusion's `MIN`/`MAX` orders by `total_cmp`
  (`datafusion/functions-aggregate-common/src/min_max.rs`, 55.1.0); pruning
  compares under `apply_cmp` and is unaffected, a statistic handed over is
  not (`KD42`).
- **Whether a column has Arrow-ordered bounds is each block's answer**, not
  the query schema's; one table's blocks share their DDL in practice, and
  nothing enforces it.
- **A cache at format 24 holding an undeclared column without bounds reads
  them absent** until a gathering `parse` back-fills it; the provider never
  parses, so it prunes nothing there until someone does.
- **`prune_block` resolves a block's columns once more** per block with
  statistics, as gathering resolved them; unpriced.
- **A `KD8` value takes its column's bounds with it, not its counts**:
  `t_date.v_date`, `t_timestamp.v_ts` and `t_numeric.v_small` hand over no
  extremes and an `Exact` NULL count.

## The sink, the settings and the metrics

- **Nothing a query raises reaches the sink after the statement.** In Arrow
  semantics the comparison channel is empty, and `EarlyStop` implements no
  `Finding`; an after-statement hook would mean owning the REPL loop.
- **Registration on a real dump is loud**: every `text` column with no
  `COLLATE` warns (`KD40`); the binary folds them into one line per dump.
- **`bytes_unread_early_stop` reads `0` wherever no stop was planned**, and
  the pruning metric appears only where statistics were consulted; a plan
  executed twice counts its early stops twice and its pruning once.
- **A line limit binds only a line that crosses a read**, as `pgdt`'s
  `--max-line-bytes` does.
- **A floored draw's `drawn` is `0`** under a stated allowance at the reserve,
  the floor drawing a zero budget.
- **`datafusion-cli-pgdump`'s planning test leans on the host reporting a
  memory figure**: with neither a limit nor `MemAvailable`, a scan plans with
  no warning.
- **`pgdt` adopted none of it**: it prints each channel where it did, keeps
  its own `plan_note_origin` clause, reads no levers, and its status lines
  quote a budget without the held statistics the provider's account names.

## For P23's figures

- **No P6 slice took a figure**, and the spec promises none.
- **`statistics-gathering` is expected to move**: its control's `v_text`,
  `v_long_text` and `v_escaped` are bare `text`, now bounded, and every
  retained column carries one more `Option<ColumnBounds>`, which the account
  charges.
- **`statistics-pruning`'s input moved and its semantics did not**: its
  `v_category` is bare `text` and now carries bounds, so the fidelity guard
  stopped asserting `no bounds`; `pgdt query` still reads only its dictionary.
  What the heavier cache costs that figure's legs is unpriced.
