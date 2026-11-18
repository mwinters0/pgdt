# I/O, memory and parallelism

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. What the process may read and hold at once: the source
trait and its positioned reads, the pools a read is served from, the allowance a run states or discovers and how it is
carved into workers and buffers, and which sources recommend parallelism. `pgdt-io` holds the mechanisms; the figures
these entries rest on are `measurements.md`'s `scan-throughput-*`, `reserve` and `chunk-size` families.

### D1 The library never spawns threads by surprise
`Parallelism::default()` is `Serial`; discovery is opt-in, asked for by the CLI and by the DataFusion provider's
`ScanBudget` unless built with an allowance (`ScanBudget::new`). Rejected: `default_workers` read in the library, so
silence means concurrency.

### D2 A plain file recommends one worker; a local compressed one the machine's cores
A plain file recommends one: a serial plain `parse` is device-bound wherever a split reaches, a `COPY` block's interior
(an `INSERT` run, `KD9`, admits none), each piece's tail read adds bytes, and `query`'s typed scan, unbound on the NVMe,
is held flat by `KD57` (D25). A local compressed source recommends its cores capped at its units, a gzip first pass at
its chunks (`RT7`) and its affinity's physical cores (`RT27`); a fetched one, one: more connects to a third party
unasked. Rejected: a plain split once a typed one outran one worker on the NVMe; a fetched first pass's cores, its tasks
fetching no more; a first pass's logical count or one less, neither beating the physical. Reopens: `KD57` closing (D7);
either beating it on a file it or the physical lifts. Evidence: `scan-throughput-*`, `parallel-scan-throughput`,
`gzip-first-pass`.

### D3 The memory constants, and what each one is
`MEMORY_RESERVE` is a subtraction, not a fraction, which under-reserves the small allowances where being wrong kills the
process. `MEMORY_MARGIN_PERCENT` is a judgement about the worst rep, enforced on the count. `MEMORY_UNPOOLED_BOUND` is
read off a grid, never a per-reader term, and a new codec's resident is read against it, never argued to carry over
(`charge_model_problem`). `DEFAULT_MEMORY_BUDGET` stays small enough to decline block decode on an ordinary `.xz`;
raising it to clear that gate makes one number answer two questions. Re-setting the reserve and the bound is P23's
(`KD34`). Evidence: `reserve`, `chunk-size`, `gzip-resident`.

### D83 `--memory` states what the process may hold; one carving serves stated and discovered
A read-buffer budget is a number no operator can size a container from, so `--memory` states what the process may hold
and `Arrangement::resolve` carves it, reserve off the top and `margin_allowance` on the count, exactly as a discovered
limit (D3); `resolve` is the only constructor an embedder has, so no pass holds a parallelism and an allowance not
carved from each other; a test states a cut's budget by `test-support`'s `arrangement_of`. Rejected: a second flag;
keeping the budget flag and giving statistics what it leaves (D85); a gathering pass handed its parallelism and
statistics allowance apart (`GatherOptions`), which two carves could pair; a test's allowance worked back through the
reserve and margin, tying cut tests to them; a `resolve` input stating a budget.
Code: `arrangement.rs`. Evidence: `reserve`.

### D157 A run's held lines draw from one ledger its arrangement carves, waiting in file order
`Arrangement::resolve` carves one `Ledger` under a found allowance: the margin's ceiling less the budget and what others
drew, a read's longest line off the ceiling; else no limit. Statistics draw first, opened at `held`. A line waits behind
earlier waiters; the earliest unadmitted, or statistics past the room, ask later holders to give up, latest first. Why:
the leftmost line progresses, a suspended holder keeps its buffer, statistics never hang on a line. Alone, the earliest
passes the room, never asked, up to the floor (D161), once later readers park (D163). Rejected: dropping a waiter's
buffer; giving up on every wait; a bound per worker; the cap, lending the margin; a fixed split; first come; `held` off
the room, a back-fill billed twice; an uncarved estimate's room unmargined, past halving. Code: `ledger.rs`. Evidence:
`ledger/tests.rs`.

### D160 A read over mapped blocks carves its longest line at its query's factor, then plans under that arrangement
`Table::line_charge` answers the longest line of the blocks the read holds rows of, at its query's factor, zero where
it fits its chunk; `pgdt query` and each provider scan carve it (`ArrangementRequest::line_charge`, D157), and
`Table::plan` takes that arrangement whole; a live scan's charge is each later draw's `held`. A block pruned whole, or
whose plan reads no field, is passed over, its long rows streamed whatever a dynamic filter reads (D138). Why: the
line fits beside its readers, lent by no later scan's ledger; a count and a ledger meet only as carved together (D83).
Rejected: a kept run's own longest, unbounded by a block's record; asking a dynamic filter, arriving after the draw
(D95); a ledger in `QueryOptions`; one per session, for `KD38`'s slack. Code: `line_charge`,
`ScanBudget::draw`. Evidence: `tests/ledger.rs`, `budget.rs`.

### D161 A line one reader cannot hold is refused at the floor, whatever the count, a mapped read before its first byte
`Arrangement::resolve` gives a limited ledger a `Floor`: the cap, or the margin's ceiling where higher, less what others
drew and one reader's budget, never under the room, less the statistics and other scans' line charges carved beside it,
an estimate's too. A draw past it is refused wherever its line lies (`LineOutgrowsMemory`, naming those), and
`TablePartitions::new` holds the longest line against it. Why: parked down to one reader, that is all a line can have,
at any `--jobs`; a holding ignored is an OOM kill. Rejected: the room at the resolved count; a pool's grant, which may
pass the allowance; all a parse's statistics may hold; their yielding to a line, or their draw, by timing; the
cap alone, refusing a line the ledger lends; refusing only the earliest. Code: `one_reader_leaves`. Evidence:
`arrangement/tests.rs`, `tests/ledger.rs`.

### D162 A parked reader's pool share is given back as it parks; the budget and the arm stand
A reader parks holding no read (`ParkedReader`, `hint_parked`), and its source sizes its pools for the readers still
reading: a compressed source's block-pool depth is theirs, its retained blocks drained to one below it at the park, and
any pool's free buffers over a lowered limit dropped then. Why: a ledger lending a parked reader's term needs its bytes
gone now, not at the next decode. Rejected: re-announcing a smaller `Parallelism`, which moves the budget `block_path`
and the cut read mid-run; a chunk pool shrinking per reader, its depth flat in the count (`KD24`); a count the pass
sets, which concurrent parkers overwrite. Code: `DecodeBudget::hint_parked`, `BlockCache::give_back`. Evidence:
`blocks/tests.rs`, `pool/tests.rs`.

### D163 Lines and statistics past the room park the latest readers; a line alone is admitted once they have
A reader stands before absorbing a read, a sub-stream aside as it yields. Where lines and statistics hold past the room,
the standing readers after the earliest busy line holding and awaiting no line park, latest first, as few as the carve's
fall covers (`Terms`); a line alone waiting past it is admitted once they have parked or left. They park dropping the
read in hand (D162, `KD172`), a sub-stream's batch emitted first, resuming as the excess falls. Why: the floor is the
room and what others free (D161); a park gives back at once, a give-up at its holder's next draw. Rejected: admitting it
first; parking only behind a line alone; asking a reader whose line waits, which waits on it, or one at a yield,
unpolled under a merge; parking with a batch, its views pinning what no carve states (`KD23`); never asking one holding
rows, cut alike each run but passing the room; crediting an unscaled term (`KD21`). Code: `ledger/park.rs`,
`draw_absorbing`, `replay`. Evidence: `ledger/tests.rs`, `tests/ledger.rs`.

### D4 A budget is solved against a source's cost, never divided by it
`WorkerMemory` carries a per-worker term and a shared pool term (`affords`, `at`): the block pool's retention list is
flat below `POOL_DEPTH` and a unit per reader above, which no scalar expresses. `BufferPool` is bounded in bytes,
floored at one slot; a unit the budget cannot hold is refused, never shrunk (`BlockCache::affordable`). Evidence:
`reserve`, `parallel-peak-rss`.

### D5 A wait is a permission the read loop grants
`NeverWait` is the default and allocates past the budget. Only `interior::scan_region` grants `MayWait`, restoring it on
exit, and each top-level loop announces `NeverWait` (`announce_read_loop`), a dropped region's included: each leader
worker holds one read, so a blocked one waits on a sibling that finishes. A loop retaining into a batch holds the
buffers it would wait on, and the block pool never waits whatever the loop said. Rejected: gating on `Parallelism`.

### D6 `ByteRangeSource` mirrors `object_store` and is dyn-compatible
`read_range`/`extent`/`modified` mirror `object_store`'s `get_range`/`head`, so a remote source is additive behind a
feature without the dependency; futures are boxed, so the trait is dyn-compatible and wrappers compose. `extent()` is
`Exact` or `Open`, a type rather than a size beside a flag, so each site needing the end says what it does without one.
Rejected: generics per command; `enum AnySource`; `size()` beside a flag each site must remember; a counting pre-pass;
an `Unscanned` tail to the bound, which each save re-reads, or open-ended, a case for every span consumer.

### D7 `partitions()` says where and at what cost, never whether
The same file is worth cutting for extraction and not for discovery, so the caller decides. Past a region of open
extent, which it never cuts, the scheduler's only refusal is a memory floor against the file's remainder, erring serial
(`KD22`). Rejected: `Anywhere` as "stay serial".

### D8 The cut is one unit wide, sized apart from the charge
`BOUNDARIED_PARTITION_UNITS = 1`, and the cut (`window_end`) is sized apart from the charge (`partition_bytes`), so a
change to one cannot move the other (`KD25`). A wider cut amortised only a tail read's second decode of its successor's
block, which a miss on a block in flight no longer costs (`BlockCache::claim`), and it costs granularity. Rejected: a
wider cut; deriving the width from the charge.

### D10 mmap, `fadvise` and double-buffered readahead are refused
Positioned reads on `spawn_blocking`, one chunk at a time, into pooled buffers. Refused: mmap, `fadvise` and
double-buffered readahead, since a cold `COPY` scan already sits at the device's delivery on every device measured; mmap
also bypasses the trait, faults uninterruptibly and errors as `SIGBUS`. `SCAN_CHUNK_DEFAULT_SIZE_BYTES` is shipped, not
probed: rotational detection is masked in containers and undefined over LVM/NFS. Reopens: parse CPU exceeding read time,
which an `INSERT` run on the NVMe meets (`KD9`) and a default `parse` there may (P23). Evidence: `scan-throughput-*`,
`chunk-size`, `allocator`.

### D11 Limit discovery takes a root; the seam is crate-private
`discover_memory_limit_in` reads the cgroup limit (RT3, RT5) under a filesystem root, because the arms worth pinning are
ones no machine is more than one of; an embedder discovers at `/` (`Allowance::discover`) or states an allowance, so the
root-taking readers are crate-private (D106). With no limit found, `pgdt` caps a source recommending a charge at half of
`MemAvailable` (RT8), untuned, and not one recommending none; a request carving that estimate (`carve_estimate`, the
provider's) carves half as an allowance. With `MemAvailable` unreadable `DEFAULT_MEMORY_BUDGET` is the estimate,
unstated not being unlimited: a cap, which a carving request draws down by what others drew. Rejected: an env var
overriding the root; the seam public for tests alone; refusing such a recommendation; the default as a cap per scan,
which multiplies.

### D12 Workers are `spawn_blocking`; no runtime flavour is imposed
The library keeps `tokio` at `rt`+`sync` and imposes no runtime flavour, its work being `spawn_blocking`; `pgdt`'s own
commands run `current_thread`, so thread count follows dispatched work, not CPUs, and `sql` runs DataFusion's. Rejected:
a thread pool of our own; `rayon`. Consequence: cancellation is cooperative (D26).

### D13 The allocator is the binary's choice; the one binary composes DataFusion's CLI and links mimalloc
No `#[global_allocator]` in a library. `pgdt` is the one binary, composing `datafusion-cli-pgdump` as `sql`
unconditionally so every figure times what ships, and links mimalloc as that CLI does, without `override`: C keeps
libc's `malloc` (`KD109`). It sets mimalloc's purge delay (`PURGE_DELAY_MS`) as a default `MIMALLOC_PURGE_DELAY`
overrides, an embedder keeping its own, chosen on throughput: leaving zero buys back most of the speed. Rejected: the
platform allocator, or `override`, off DataFusion's path; `mallopt(M_ARENA_MAX)`, binding C's arenas alone (RT10) and
overwriting `MALLOC_ARENA_MAX`; DataFusion behind a feature, or a second binary; a library constant re-set for a term
one allocator alone holds; a delay fitted under one run's allowance. Reopens: a contention figure; a figure where
another delay's speed outweighs what it keeps; a mimalloc-only gate failure no library change or option reaches.
Evidence: `allocator`, `purge-delay`, `purge-delay-resident`.

### D165 What the allocator keeps of what it frees is one bound the binary states, held beside every arrangement
`pgdt` states what mimalloc keeps under its purge delay (`RETAINED_BOUND`) as `ArrangementRequest::retained` on every
pass, and `pgdt sql` as `ScanBudget::retaining`; an embedder states its allocator's, or none. It comes off the margin's
ceiling, off the ledger's room, never a statistics draw an account replaces, and off the floor as beside a line. It is
read off a grid of reader counts as `MEMORY_UNPOOLED_BOUND` is (D3), that bound's smallest step covering the largest
median the delay keeps over a zero delay's, the worst rep the margin's. Why: the allocator is the binary's (D13), and
what it keeps moves with the cut, not the count. Rejected: a term per reader; the count one run's allowance carves; a
library constant; the margin, which the run overruns; off the count alone, as a grant, a holding ignored at the floor
being an OOM kill (D161). Reopens: a workload keeping more; P23 re-fitting `MEMORY_UNPOOLED_BOUND` net of it.
Code: `alloc.rs`. Evidence: `runs/p33.34-delay-readers-20261118/`, `arrangement/tests.rs`.
