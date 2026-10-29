# I/O, memory and parallelism

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. What the process may read and hold at once: the source
trait and its positioned reads, the pools a read is served from, the allowance a run states or discovers and how it is
carved into workers and buffers, and which sources recommend parallelism. `pgdt-io` holds the mechanisms; the figures
these entries rest on are `measurements.md`'s `scan-throughput-*`, `reserve` and `chunk-size` families.

### D1 The library never spawns threads by surprise
`Parallelism::default()` is `Serial`; discovery is opt-in, asked for by the CLI and by the DataFusion provider's
`ScanBudget` where no allowance is stated. Rejected: `default_workers` read in the library, so silence means
concurrency.

### D2 A plain file recommends one worker; a local compressed one the machine's cores
A plain file recommends one: a serial plain `parse` is device-bound wherever a split reaches, a `COPY` block's interior
(an `INSERT` run, `KD9`, admits none), each piece's tail read adds bytes, and `query`'s typed scan, unbound on the NVMe,
is held flat by `KD57` (D25). A local compressed source or gzip first pass recommends its cores capped at its units
(`RT7`), a first pass's also at its affinity's physical cores (`RT27`); a fetched one, one: more connects to a third
party unasked. Rejected: a plain split once a typed one outran one worker on the NVMe; a fetched first pass's cores, its
tasks fetching no more; a first pass's logical count or one less, neither beating the physical. Reopens: `KD57` closing
(D7); either beating it on a file it or the physical lifts. Evidence: `scan-throughput-*`, `parallel-scan-throughput`,
`gzip-first-pass`.

### D3 The memory constants, and what each one is
`MEMORY_RESERVE` is a subtraction, not a fraction, which under-reserves the small allowances where being wrong kills the
process. `MEMORY_MARGIN_PERCENT` is a judgement about the worst rep, enforced on the count. `MEMORY_UNPOOLED_BOUND` is
read off a grid, never a per-reader term. `DEFAULT_MEMORY_BUDGET` stays small enough to decline block decode on an
ordinary `.xz`; raising it to clear that gate makes one number answer two questions. Re-setting the reserve and the
bound is P23's (`KD34`). Evidence: `reserve`, `chunk-size`.

### D83 `--memory` states what the process may hold; one carving serves stated and discovered
A read-buffer budget is a number no operator can size a container from, so `--memory` states what the process may hold
and `Arrangement::resolve` carves it, reserve off the top and `margin_allowance` on the count, exactly as a discovered
limit (D3); `resolve` is the `Arrangement`'s only constructor, so no pass holds a parallelism and an allowance not
carved from each other. Rejected: a second flag; keeping the budget flag and giving statistics what it leaves (D85); a
gathering pass handed its parallelism and statistics allowance apart (`GatherOptions`), which two carves could pair.
Code: `arrangement.rs`. Evidence: `reserve`.

### D4 A budget is solved against a source's cost, never divided by it
`WorkerMemory` carries a per-worker term and a shared pool term (`affords`, `at`): the block pool's retention list is
flat below `POOL_DEPTH` and a unit per reader above, which no scalar expresses. `BufferPool` is bounded in bytes,
floored at one slot; a unit the budget cannot hold is refused, never shrunk (`BlockCache::affordable`). Evidence:
`reserve`, `parallel-peak-rss`.

### D5 A wait is a permission the read loop grants
`NeverWait` is the default and allocates past the budget. Only `interior::scan_region` grants `MayWait`, and restores it
on exit: each leader worker holds one read, so a blocked one waits on a sibling that finishes. A loop retaining into a
batch holds the buffers it would wait on, and the block pool never waits whatever the loop said. Rejected: gating on
`Parallelism`.

### D6 `ByteRangeSource` mirrors `object_store` and is dyn-compatible
`read_range`/`extent`/`modified` mirror `object_store`'s `get_range`/`head`, so a remote source is additive behind a
feature without the dependency; futures are boxed, so the trait is dyn-compatible and wrappers compose. `extent()` is
`Exact` or `Open`, a type rather than a size beside a flag, so each site needing the end says what it does without one.
Rejected: generics per command; `enum AnySource`; `size()` beside a flag each site must remember; a counting pre-pass;
an `Unscanned` tail to the bound, which each save re-reads, or open-ended, a case for every span consumer.

### D7 `partitions()` says where and at what cost, never whether
The same file is worth cutting for extraction and not for discovery, so the caller decides. The scheduler's only refusal
is a memory floor against the file's remainder, erring serial (`KD22`). Rejected: `Anywhere` as "stay serial".

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

### D11 Limit discovery takes a root; the seam is `internal`'s
`discover_memory_limit_in` reads the cgroup limit (RT3, RT5) under a filesystem root, because the arms worth pinning are
ones no machine is more than one of; an embedder discovers at `/` (`Allowance::discover`) or states an allowance, so the
root-taking readers are `internal`'s (D106). With no limit found, a source recommending a charge is capped at half of
`MemAvailable` (RT8), untuned, and one recommending none is not. Rejected: an env var overriding the root; the seam
public for tests alone.

### D12 Workers are `spawn_blocking`; no runtime flavour is imposed
The library keeps `tokio` at `rt`+`sync` and imposes no runtime flavour, its work being `spawn_blocking`; `pgdt`'s own
commands run `current_thread`, so thread count follows dispatched work, not CPUs, and `sql` runs DataFusion's. Rejected:
a thread pool of our own; `rayon`. Consequence: cancellation is cooperative (D26).

### D13 The allocator is the binary's choice; the one binary composes DataFusion's CLI and links mimalloc
No `#[global_allocator]` in a library. `pgdt` is the one binary, composing `datafusion-cli-pgdump` as `sql`
unconditionally so every figure times what a release ships, and links mimalloc as that CLI does, without `override`: C
keeps libc's `malloc` (`KD109`). Rejected: the platform allocator, or `override`, off the path DataFusion tests on;
`mallopt(M_ARENA_MAX)`, binding on C's arenas alone (RT10) and overwriting the operator's `MALLOC_ARENA_MAX`; DataFusion
behind a feature, or a second artifact or binary; a library constant re-set for a term one allocator alone holds.
Reopens: a contention figure; a mimalloc-only gate failure that neither a library change suiting both allocators nor a
mimalloc option the environment can override reaches. Evidence: `allocator`.
