# P33 notes — every line PostgreSQL writes and reads

What the phase leaves the next one, and what it tried and refused. The spec is
[`roadmap-P33-line-bound.md`](roadmap-P33-line-bound.md); the mechanisms'
decisions are [`decisions.md`](decisions.md), D13's purge-delay clause and
D138–D165; each crate's part is [`concepts.md`](concepts.md). The per-slice
notes went at the wrap, and git holds them at `c33e3886`. Each slice's
decision reached the register in the slice that made it or sits beside its
mechanism in code. The phase filed `KD164`, `KD167` and `KD170`, struck
`KD93` and `KD167`, and its repoints filed `KD165`, `KD166`, `KD168` (since struck),
`KD172`, `KD173`, and the wrap's `KD175` and `KD176`; `KD171` and `KD174` came
of its reviews. `KD170` is `M287`'s; the rest stay `(c) unowned`. No `KD<k>` was
owned by P33.

## What the next phase inherits

### The instruments

- **`long-line-resident` is the phase's figure**, its legs, factors and
  falsifiers `run_long_line_resident`'s, its generator
  `scripts/generate_long_lines.py`. No check confirms a query leg's partition
  count: `long_line_leg_problems` checks a query's or filter's answer only, the
  first `scan started` in its log being the untimed builder's.
- **`line_growth` (`crates/pgdt-dump/examples/`) is D139's `Reopens` probe**: it
  times a line grown by `LineMapping` against one written into a mapping sized
  for it, reporting moves, alignment and `AnonHugePages`;
  `runs/p33.12-line-steps/probe.sh` runs it. A growth leg compacts memory first
  (`vm.compact_memory`) and records `/proc/buddyinfo` beside it: uncompacted,
  the host's free 2 MiB blocks decide both legs' backing, so the reading prices
  the host (`growth.out` against `growth-compacted.out` there).
- **What holds a copy is tested two ways**: pgdt-query's
  `tests/decoded_copies.rs` counts held copies under a tracking allocator,
  blind to a line past its chunk, mapped outside the allocator (D139);
  pgdt-mapping's `tests/streamed_line.rs` reads that line as `VmRSS`.
- **A push machine is held to the reader it replaced** by a `prior_shape.rs`
  with a `differential.rs` beside it (`nested/input/`, `geometric/`), and over
  runs by `check/tests.rs`'s `walked_in_runs`; a walk fed a byte at a time
  cannot see a run's end, so only readers taking runs are tested over cuts.
- **`resumable-reader-micro` times a copy of the reader frozen at `0fd33bd3`**,
  not the crate's walks: the figure declares only `benches/resumable/`, so an
  edit to `value/source.rs` or `nested/input/` leaves it fresh. It times no
  `COPY` unescaping, `[m:n]=`, radix, range or multirange.
- **The `introspect` build's block tracker names what a run keeps past its
  `--memory`, by allocating stack**: built with line tables (`release.py build
  --features introspect --profile profiling`), armed by
  `PGDT_INTROSPECT_BLOCKS_FROM=<bytes>`, each leg read at the shipped delay and
  at `MIMALLOC_PURGE_DELAY=0`, a site's share its shipped sum less its zero
  one. `runs/p33.31-blocks-20261117/` (`run.sh`, `table.py`, `summarize.py`) is
  the pattern.
- **A count pair's method, beyond `--count-pair`'s own rules (D108, D155)**:
  - a commit other than the tree is priced by `--count-pair <parent>..<change>`,
    never a pair from a second checkout, Cargo taking the shared
    `count-pair-tree` build as current by mtime;
  - a span's residual is placed by callgrind on the gap between each pair of
    adjacent legs, each gap's candidates its own commits, a pair on one
    candidate splitting two that change one function only where the split
    changes what is done (`runs/count-pair-33.26-20261112/callgrind/{gen,analyze}.py`,
    adapted in `runs/count-pair-33.28-20261113/placement/`);
  - a job of several pairs keeps each step's legs, which the next pair
    overwrites (`runs/count-pair-33.26-20261112/run.sh`, `legs-<step>/` with
    `legs.sha256`), and a hand-written placement keeps each query row's
    uncounted cache builder, without which 33.26's first query runs mapped the
    dump inside the counted run;
  - callgrind placements run four or five at once, an `Ir` count not moving
    with the machine's load; on `query --schema-mode typed` its gate can miss on
    one rep pair, mimalloc's paths moving, and the strings query's functions
    place the rise;
  - the four long rows cannot resolve a per-row saving, a cluster of 32 MiB
    lines holding few rows: a per-row change is priced on the control rows.
- **Callgrind legs kept on the SSD**, read by nothing else and deletable,
  `--callgrind-recipe` rebuilding a leg:
  `/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.{16,22,24,25,26,27}/`.
- **The ledger's tests compare answers and count no wait or give-up**
  (`tests/ledger.rs` in pgdt-query and pgdt-mapping): at a four-byte chunk the
  `types` fixture both waits and gives up, at sixteen nothing gives up, so a
  changed chunk or fixture can stop exercising give-up unseen. Six test
  wrappers forward `hint_parallelism` but not `hint_parked` — pgdt-query's
  `map_file.rs`, `identity.rs`, `wait_policy.rs`, `partitioned_replay.rs`,
  pgdt's `determinism.rs`, pgdt-mapping's `backfill/tests.rs` — so a parking
  test through one gives nothing back. A test wanting a ledger with no room
  states its floor (`pgdt_fixtures::floored`), never a small allowance, under
  which the floor is zero.
- **Two paths are tested in part**: the real-bound test
  (`crates/pgdt/tests/line_bound.rs`) runs `pgdt parse` and `pgdt query`, never
  the provider or the back-fill; live scans' line charges are one unit test
  (`two_live_scans_ledgers_lend_no_byte_twice`), no join over two dumps.
- **The hand runs** ("The hand runs", below) read
  `datanommer2-2026-10-01.dump.xz` through a symlink in
  `/mnt/ssd/fedora/scratch/pgdump_query/datanommer/` pointing into
  `/mnt/wd12t/fedora/experiments/dn-pgdt/`, so `contained.py` takes `--mount`
  of it; each leg writes its own cache there, and the query reads the copy in
  `p33.7-query/`, under `peak-rss`. A parse's expected counts are
  `runs/p38.7-datanommer-count-20261031-1224/parse.log`'s, a query's its run
  directory's 12g `unlimited.csv`.
- **A `purge-delay` or `purge-delay-resident` re-take reads 100 ms twice and
  mimalloc's 1000 ms in no column**, the shipped leg being `PURGE_DELAY_LEGS`'
  last value, so pricing mimalloc's default (D13's `Reopens`) adds 1000 as a
  stating leg. `measure.py`'s `discovered_budget` and `afforded_readers`
  subtract no `RETAINED_BOUND`, so the bill makes them looser upper bounds.

### What stays unattributed

Each is named with what would attribute it; none blocks anything.

- **A four-worker long-line parse is slower than a one-worker one at every
  length and kind**, at `36888923` and still at `543e5877`
  (`runs/long-line-resident-20261118-2206/log`'s wall clocks, no figure). A
  `--profile-recipe` profile of a four-worker parse over `long_text_512`.
- **A parse's slow growth with its line**, which `long-line-resident` states:
  the `introspect` build's live peak at 512 MiB and at the bound would say
  whether it is Rust's heap.
- **A four-partition query over escaped text or bytea holds less than four lines
  and four copies**, increasingly with the line: whether each partition's copy
  is live at the peak, for the `introspect` build.
- **Wall clocks the figure does not publish moved**: a query over escaped text
  under the resident wrapper is slower than at `93428ef7`, and a strict parse
  over bytea slower at `543e5877` than at `4cb97fd4`. A count pair over each.
- **About 555 MiB of 33.7's query run's peak at zero delay lies outside the block
  tracker**: the held line's mapping (D139), C's heap with liblzma's decoders,
  the binary. The `introspect` report's `mallinfo_*` lines beside the mapping's
  size at the peak.
- **What 100 ms keeps over zero at one, two, six and seven readers is confounded
  by UF5's repartition**, the two peaks falling at different moments: both
  delays at `target_partitions` the carved count, never taken.
- **Two gaps in `resumable-reader-micro`**, as its section says: the push machine
  costs more than the pull on a one-run scalar, and the pull's per-element slope
  is several times `nested-decode-micro`'s. Callgrind on the bench's `push` and
  `pull` legs; re-taking `nested-decode-micro` for the second.
- **How much of 33.26's rise in `ResolvedTerm::eval` 33.29's fall recovers**, and
  whether `eval` is still out of line, its pair being against its parent alone:
  callgrind on 33.29's tree against 33.26's `gap3` legs. **M277's and M278's
  joint residual** was judged by the data-level row's fall at 33.27, not by
  reading `Gatherer::field`: callgrind on 33.27's kept legs.
- **The walk's fall on the `arrays` count-pair row** (`dbdaead4..067b8f69`) is
  unplaced, D108 placing rises only: `--callgrind-recipe`'s `arrays` block on
  legs at those two commits.

### Behaviour a later phase meets

- **What statistics, the reserve and the bill leave the floor**: under `pgdt`'s
  bill the cap never binds on a run with an allowance and statistics gather
  nothing at or below 720 MiB, `RETAINED_BOUND` exceeding `MEMORY_RESERVE −
  MEMORY_UNPOOLED_BOUND`; `memory.rs`'s account of the bands is the library's
  and leaves the bill out. A parse's floor subtracts the statistics its cache
  handed it, so a back-fill growing the cache makes the next parse refuse more
  lines. P23 meets both, and P26 moves the second.
- **A provider scan's floor depends on the session**: its billed maps and every
  live scan's carved line, so a join's second-planned scan can be refused a line
  it would read alone; `KD38` names only the speed.
- **A replay can hold a line's bytes with no draw in two places**: a drawn row a
  filter drops keeps its bytes in the carry and the retained chunks until its
  chunk's events end, across a yield, at most one line per sub-stream; and a row
  cut before waits for its re-draw with the line in its carry, which it cannot
  give up. P23's resident account meets both.
- **A source stating `WorkerMemory` implements `hint_parked`**, or `Terms` credits
  bytes its pools never free; the trait's default does nothing. P18's zstd and
  lz4 sources and P41's compressed entries meet it. After a park, a compressed
  source's retained blocks can briefly stand past the drain: a decode in flight
  lands after `BlockCache::give_back`, until the next decode drains it.
- **`line_charge` (`stream/replay/plan.rs`) and `reads_no_field`
  (`stream/replay.rs`) state one rule twice**, the first off the planned block,
  the second off what `activate` builds from the header; a header resolving
  otherwise than its plan would split them, which no test dump does.
- **What P40 meets beyond its inbox**: a TimescaleDB catalog block the scanner
  records holds every row whole (`record_rows`, D133), never streaming; a
  strict check of a walked container collects each scalar part's text and a
  range's bounds through the global allocator, no ledger drawing for them;
  `Strtod` (`value/libc.rs`) is the byte-fed `strtod` a streamed `float8in`
  can take, `float.rs` and `datetime/grammar.rs` reading by slices of their
  own; a `numeric` radix spelling (v16 on) cannot be bounded by its decimal
  length while read; each datetime reader inlines `within_workbuf`'s refusal
  path, a reader added later inheriting its per-call cost; and `KD176`.
- **D152's positions' time is unpriced**: a `jsonb` term reads its field twice
  and a sort keys two pairs again per comparison, a multirange each member once
  per comparison, and no `--count-pair` row or figure reads either term.
- **Growing a mapped line by a move reserves the new length and a step of
  address space beside the old mapping**, inaccessible and uncommitted, so only
  `RLIMIT_AS` sees it: an embedder under one (P24, P39) meets about twice a long
  line, transiently.
- **mimalloc 3.3.2 arms an arena's purge expiry at its first pending free and
  never extends it**, purging on a later page allocation or free, one thread at
  a time under a global guard, and at 0 ms inside the free; so a delay keeps at
  most the free rate times the delay (`libmimalloc-sys` 0.1.49,
  `v3/src/arena.c`, `mi_arena_schedule_purge`, `mi_arenas_try_purge`). P23's
  inbox has the floor grid this leaves.

## The readings the record cites

### The block tracker's readings

Taken from `05ac83ae` over 33.7's query run, armed at 1 MiB, twice at
mimalloc's shipped delay and twice at zero (`runs/p33.31-blocks-20261117/`,
`readings.txt`); every run answered 33.7's rows and peaked where 33.7's did.
The account that the excess is the freed decoded copy is refuted: at the
shipped delay the decoded copy (`unescape_owned`) names a third of the largest
freed-resident sum, the view builder's own blocks (`push_utf8view_field`'s
`append_value` under `OWN_BLOCK_PAST`) about as much, and the compressed
source's decoded-block buffers (`BufferPool::pick`) most of the rest. **At the
resident peak the decoded copy holds almost none of the excess**: shipped less
zero, the view builder's blocks and the decoded-block buffers are most of it,
DataFusion's coalescer, xz's decoder, the decoded copy and the line's growth a
few MiB each — the bytes of rows streaming through, freed at the run's rate and
kept to the purge, which no draw charges. **A decoded copy outlives its draw**:
at the peak two decoded copies of 440 MiB lines are live while the ledger holds
a few MiB, the plan's `RepartitionExec` pushing the yielded batch into an output
partition's coalescer, which keeps a dense view array's buffers by reference
until its output batch fills (`../status/upstream.md`, "UF5").

### The hand runs

On the reference dump, `datanommer2-2026-10-01.dump.xz` (its longest line
441 MiB, `runs/datanommer-survey/`), as the spec's "How it is shown" states:

- **The strict parse under its longest line held no line before the bill**:
  `--memory` 432 MiB at `7f593832`, one reader, exit 0, every block and row
  counted, its peak under half the line
  (`runs/p33.7-strict-hand-run-20261116-1842/`). **Under 33.35's bill the same
  run is refused** (`runs/p33.7-strict-hand-run-20261118-2344/`): the floor is
  zero below about 750 MiB, and a row a strict parse streams is drawn for once
  before its carry knows it streams (`KD170`).
- **So the strict run is read at 813 MiB, the smallest whole MiB whose floor
  holds two read chunks**: above 640 MiB the floor is the allowance less
  `MEMORY_RESERVE`, `WorkerMemory::at(1)`, `RETAINED_BOUND` and the statistics
  held, `at(1)` on this `.xz` read off a refusal by resuming a copy of a
  refused cache lower (`runs/p33.7-strict-floor-20261119-0050/HANDOFF.md`).
  Taken from `543e5877` at 100 ms and at zero: each exit 0, one reader, every
  block and row counted, each peak under the stated `--memory` and under half
  the line. Re-derive the allowance when any of those terms moves.
- **The query run** is `pgdt sql`'s `count(*)` and `sum(octet_length(msg))`
  over `_hyper_1_509_chunk`, the block holding the longest line, under `SET
  pgdump.memory`. At 2 GiB before the bill it passed its `--memory` at the
  shipped delay and held at zero (`runs/p33.7-query-hand-run-20261117-0320/`),
  its live Rust heap under the allowance; at 33.33's 100 ms one rep of three
  passed (`runs/p33.7-query-default-20261118/`); under the bill 2 GiB carves
  two readers and holds, but the cut falls before the cluster of long lines
  (`runs/p33.7-query-billed-20261118/`).
- **So the query run is read at eight readers**, the smallest whole MiB whose
  `EXPLAIN` carves eight (`runs/p33.7-query-eight-20261118/carve.sh <bin>
  <MiB>...`): eight puts the two 440 MiB lines in different pieces, as four and
  seven do, placed from `runs/datanommer-survey/long-lines.tsv` and `pgdt info
  --detail`, never from a reading. There, from `543e5877`, six reps held, its
  rows an unlimited run's, every peak under the stated `--memory`, the cluster's
  charges at once past the ledger's room. The margin carved above the ledger
  absorbs the copy a repartition holds past its draw at eight readers (UF5);
  not at every cut ("The zero delay where the repartition holds copies").
  Re-carve when the bill, the reserve, a factor or `cut_blocks` moves.

### The zero delay where the repartition holds copies

`runs/p33.34-delay-readers-20261118/` is the grid `RETAINED_BOUND` is read
from (`run.sh`, `reduce.py`, `readings.txt`). At a zero delay its worst rep
passes its allowance at one reader by about a gigabyte, at six by about
215 MiB, and at two and seven by less. `probe.sh` re-ran those four at zero with
`datafusion.execution.target_partitions` the carved count, which leaves no
`RepartitionExec` above the scan, and every rep held, by about 260 to 545 MiB:
UF5's account, a coalescer holding a decoded copy past the ledger for each
output partition, so "the margin absorbs it" is false where the cut leaves
several long lines' copies held at once. Its zero-delay legs are re-read when
UF5 lands.

### The purge delay's leg grid

The grid stops at 100 ms, and `purge-delay`'s four parallel rows stay, where a
positive delay's cost shows first. Where 33.7's query run stops holding is
between 100 and 125 ms read by hand, and at most a quarter to a third of a
second by mimalloc's purge (above): taking the free rate from what the shipped
delay keeps over the zero-delay runs, the headroom runs out at 230–370 ms, an
upper bound. The hand-run delay grids are
`runs/p33.32-query-delays-20261117-0959/`,
`runs/p33.32-query-above-100-20261117-1334/` and
`runs/p33.32.1-query-grid-20261118-0341/`.

### A grown line's gap to a sized one

At I71's bound a line grown by remapping in base pages wrote in about twice the
time of one written into a mapping sized for it, and dropped in about
twenty-five times it, some 180 ms a line against a parse's 2 s: the page size,
not the remaps, the sized mapping backed whole by transparent huge pages and
the grown one by none (`growth-thp.out`). Growing in huge-page steps (D139)
closed it.

### The scalar push machines priced

33.13.4's and 33.13.5's pairs (`runs/count-pair-33.22-20261109/`,
`runs/count-pair-33.22-rework-20261109/`) placed each rise in the readers that
replaced a slice reader: `KeyRead::byte` matching its reader afresh each byte
and calling `IntIn::byte` out of line, `UuidIn::byte` fed a byte at a time
where the slice reader was inlined into `append_typed`, and `NumericIn`'s
per-value digit `Vec` grown a push at a time. A cheaper rework — `KeyRead::run`
picking its machine once per run, `IntIn` reading by table into a saturated
`u64`, `UuidIn` without per-byte flags — recovered the data-level parse but
rose on `query --schema-mode typed` and `pgdt sql -c`; never committed, its one
build `/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.22/legs-49f3cf05/pgdt-count-pair-tree`.
The machines are at `89cf0a30` and `f799618c`, their differential suites with
them; 33.23 reverted both.

## Negative results

- **Reserving a builder that already holds rows**, rather than cutting the batch
  before a long nested value: arrow 59's builders have no `reserve`, capacity
  being `with_capacity`'s alone, and growing one copies the batch's earlier rows
  beside the reservation. Reading a column's plan off `RowBatcher::plans` on
  every field raised all four query rows (`runs/count-pair-33.24-20261111/`);
  the per-field test is the builder's variant (`walks_column`).
- **The scanner folding an `Option<LongestLine>` per row**: it tests and rebuilds
  the option every row, 33.11's rise in `CopyScanner::next_event`
  (`runs/count-pair-33.26-20261112/callgrind/reading.txt`), where measuring a
  row against the block's longest by its length fell on every control row.
- **A non-allocating split test beside the `ParseDateTime` port**, for M276's
  rise: a second statement of I83's buffer the readers could drift from; a cold
  or inline attribute is D108's codegen-dependent rework.
- **Zeroing `Unescape`'s stage is about 7% of 33.27's rise in
  `unescape_owned`**; the rest is per escape and short stretch, a `memchr`, an
  out-of-line `escape_at` and the copy into the stage
  (`runs/count-pair-33.27-20261113/callgrind/reading.txt`).
- **Waking only the next waiter in file order**: a pass has tens of readers, and
  the admission test enforces the order, not who wakes, so `wake_waiters` wakes
  every one. Weighed, not measured.
- **Solving an uncarved estimate's count against the margin's ceiling only when
  a line is charged**: a one-byte line would cost a fifth of the estimate and
  `MEMORY_UNPOOLED_BOUND` in readers.
- **A parked reader gives up its carry**: a line under its chunk term sits in
  its own buffer, which the per-worker term already bills.
- **Planning a read once**: `Table::line_charge` and `Table::plan` each resolve
  and prune every block, neither reading a byte nor on a row's path.
- **A count pair for the ledger, parking, the floor or the bound**: each adds
  work per chunk or per plan, and D108's trigger is per row or byte.
- **`pgdt sql -c` over `arrays` as a count-pair row**: its query counts `perf`'s
  sixteen scalar columns, so no nested builder runs; the typed `query` over
  `arrays` is the only row pricing nested decoding.
- **A purge delay between 10 and 100 ms**: 10 ms keeps most of what 100 ms does
  over zero and resolves four `purge-delay` rows slower against 100 ms's two
  (`runs/p33.32.1-query-grid-20261118-0341/readings.txt`). **One above 100 ms**
  keeps more on 33.7's query run, at 125 and 150 ms one rep of three holding its
  2 GiB and at 200 and 250 ms none; a longer default re-reads `RETAINED_BOUND`'s
  grid first.
- **The purge delay set at compile time or without `unsafe`**: v3's `options.c`
  hard-codes 1000 ms with no define, `libmimalloc-sys` passes none, the
  `mimalloc` crate exposes no option, and setting the variable in `main` comes
  after mimalloc has read it (`alloc::configure`, D13).
- **An instruction count pricing a purge delay**: a purge is an `madvise` and the
  faults after it, system time `instructions:u` never sees; `cpu_seconds`
  carries it.
- **The query run's shape**, one block through `pgdt sql` rather than a
  whole-table read or `pgdt query`, is the spec's ("How it is shown").
