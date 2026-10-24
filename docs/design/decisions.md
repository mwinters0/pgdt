# Decisions

The decisions the code cannot explain: shapes kept for a deliverable not yet built, defaults chosen
over an alternative, and obvious changes measured or argued and refused. Nothing here says how the
code works (the named module does) or quotes a number (`measurements.md` does, by figure id, as the
invariant registers do by `I<n>`/`RT<n>`). Cite as `docs/design/decisions.md`, "D12"; the rest of the
rules, the line cap included, are `docs/process.md`, "The decision register".

<!-- decision-watermark: D113 -->

## I/O, memory and parallelism
### D1 The library never spawns threads by surprise
`Parallelism::default()` is `Serial`; discovery is opt-in, asked for by the CLI and by the DataFusion
provider's `ScanBudget` where no allowance is stated. Rejected: `default_workers` read in the library, so silence means concurrency.

### D2 A plain file recommends one worker; a local compressed one the machine's cores
A serial plain `parse` is device-bound on every real device wherever a split reaches, a `COPY` block's interior, an `INSERT` run admitting none (`KD9`); `query`'s typed scan,
which on the NVMe is not, is held flat by `KD57`'s merge; each partition's tail past itself adds device bytes. `CompressedFileSource` answers `available_parallelism()` capped at its unit count
(`RT7`); a fetched one answers one, the errors being asymmetric, and a gzip first pass one until it hands over to the arms (`FirstPass`), its extent open. Rejected: a plain split once a typed one outran one worker on the NVMe (`parallel-scan-throughput`'s cold-NVMe column), the provider delivering it from `target_partitions`, never from this.
Reopens: `KD57` closing, which asks the plain answer per kind of work (`D7`): `query`'s replay is extraction, `parse` and the mapping pass discovery, and one `default_workers()` per source cannot tell them apart. Evidence: `scan-throughput-*`, `parallel-scan-throughput`.

### D3 The memory constants, and what each one is
`MEMORY_RESERVE` is a subtraction, not a fraction, which under-reserves where being wrong kills the
process. `MEMORY_MARGIN_PERCENT` binds the resolved *count* of a source recommending a charge, and the budget only by the part of `held`
the ceiling cannot absorb (`within_shared`). `MEMORY_UNPOOLED_BOUND` is a bound off a grid, never a
per-reader term. `DEFAULT_MEMORY_BUDGET` stays small enough to decline block decode on an ordinary
`.xz`; clearing that gate picks one number for two questions. Rejected: re-setting `MEMORY_RESERVE`, or taking P23's reserve half, before v0.1.0 over `reserve`'s `rule`-band worst reps at the larger block size — one unexplained rep a cell, nothing killed, a shape the manual warns of, fitted against an account `KD34`'s owner has not drawn; a release-text caveat beside the manual's. Evidence: `reserve`, `chunk-size`.

### D83 `--memory` states what the process may hold; one carving serves stated and discovered
A read-buffer budget is a number an operator cannot size a container from, so the flag states
resident and `Arrangement::resolve` carves it — reserve off the top, `margin_allowance` on the count
— a discovered limit reaching the same carve (`within_shared`), so provenance rides the `Arrangement`, never `Parallelism`
(D64). Consequences: a source recommending nothing is left on `DEFAULT_MEMORY_BUDGET` capped by the
allowance less `MEMORY_RESERVE` whatever is stated, and the margin now binds a typed number. Rejected: a second flag; keeping the budget and giving statistics what it leaves;
a gathering pass handed its parallelism and its statistics allowance apart (`GatherOptions`), which two carves can pair, so `map_file` and `backfill_block` take the `Arrangement` and `resolve` is its only constructor. Code: `arrangement.rs`, `parallelism.rs`, `memory.rs`. Evidence: `reserve`.

### D4 A budget is solved against a source's cost, never divided by it
`WorkerMemory` carries a per-worker term and a shared pool term (`affords`, `at`): the block pool's
retention list is flat below `POOL_DEPTH` and a unit per reader above, which no scalar expresses.
`BufferPool` is bounded in bytes, floored at one slot; a unit the budget cannot hold is refused,
never shrunk (`BlockCache::affordable`). Evidence: `reserve`, `parallel-peak-rss`.

### D5 A wait is a permission the read loop grants
`NeverWait` is the default and allocates past the budget. Only `interior::scan_region` grants
`MayWait`, and restores it on exit: each leader worker holds one read, so a blocked one waits on a
sibling that finishes. A loop retaining into a batch holds the buffers it would wait on, and the
block pool never waits whatever the loop said. Rejected: gating on `Parallelism`.

### D6 `ByteRangeSource` mirrors `object_store` and is dyn-compatible
`read_range`/`extent`/`modified` copy `get_range`/`head` so a remote source is additive behind a feature
without the dependency; boxed futures so wrappers compose. `extent()` is `Exact` or `Open`, so each site
needing the end says what it does without one: a read loop reads to an open end by `read_at_most`,
`read_range` staying strict for a driver told what to fetch; a cut needs `Exact`; a map and its cache end
at the watermark, `info`'s completion line stating the rest. Rejected: generics per command; `enum
AnySource`; `size()` beside a flag each site must remember; a counting pre-pass; an `Unscanned` tail to
the bound, which a pass reads once and each save re-reads, or open-ended, a case for every span consumer.

### D7 `partitions()` says where and at what cost, never whether
The same file is worth cutting for extraction and not for discovery, so the caller decides. The scheduler's only
refusal is a memory floor against the file's remainder, erring serial (`KD22`). Rejected: `Anywhere` as "stay serial".

### D8 The cut is one unit wide, sized apart from the charge
`BOUNDARIED_PARTITION_UNITS = 1`; on a boundaried source `window_end` sizes the cut, `partition_bytes` the
charge, and on `Anywhere` the cut is the charge (`KD25`). A wider cut won at two stated readers only by amortising a
tail read's second decode, which a block in flight no longer costs, and lost at the flagless default. Rejected: deriving width from the charge. Reopens: an explanation of the flagless collapse.

### D9 Pool sizing constants
`hint_read_size` *becomes* the slot size and larger buffers are dropped on release: a chunk read and
`attach_text`'s coalesced reads are indistinguishable by length. `POOL_DEPTH` is the replay
retention depth, which only a block pool raises to `--jobs`, short of what one batch retains (`KD54`). `PLAIN_PARTITION_CHUNKS` caps tail-read
waste and is not derived from `POOL_MAX_BYTES`. Evidence: `chunk-size`.

### D10 mmap, `fadvise` and double-buffered readahead are refused
Positioned reads via `spawn_blocking`, one chunk at a time: no overlap scheme puts a cold scan below
the device's delivery time, which the scan already sits within a few percent of. mmap also bypasses
the trait, faults uninterruptibly and errors as `SIGBUS`. Buffers are pooled and the `Bytes` sliced
(`vec![0; len]` is `calloc`). `SCAN_CHUNK_DEFAULT_SIZE_BYTES` is shipped, not probed: rotational detection is
masked in containers and undefined over LVM/NFS. Reopens: parse CPU exceeding read time. Evidence:
`scan-throughput-*`, `chunk-size`, `allocator`.

### D11 Limit discovery takes a root; the seam is `internal`'s
`discover_memory_limit_in` reads the least of `memory.max` and `memory.high` (RT3), walking ancestors to the mount point (RT5); with no limit, a source recommending a charge is capped at half
of `MemAvailable`, untuned (RT8), uncapped where that is unreadable (`KD113`), and one recommending none is not. The arms worth pinning are ones no machine is more than one of, so tests need the root;
an embedder discovers at `/` (`Allowance::discover`) or states its allowance, so the root-taking readers and the reserve's, margin's and unpooled bound's constants are `internal`'s (D106). Rejected: an env var overriding the root; the seam public, only tests reaching it.

### D12 Workers are `spawn_blocking`; no runtime flavour is imposed
The library keeps `tokio` at `rt`+`sync`; `pgdt`'s own commands run `current_thread`, so thread count
follows dispatched work, not CPUs (an idle reactor thread seeds allocator state); `sql` runs DataFusion's.
Rejected: a thread pool of our own; `rayon`. Consequence: cancellation is a cooperative flag (D26).

### D13 The allocator is the binary's choice; the one binary composes DataFusion's CLI and links mimalloc
No `#[global_allocator]` in the library, nor a remedy re-setting its bound for a term one allocator alone holds. `pgdt` is the one binary, composing `datafusion-cli-pgdump` as `sql`
unconditionally, so every figure times what a release ships, and links mimalloc as that CLI does, without `override`: C keeps libc's `malloc`, and the process has two
heaps. `system`/`jemalloc` are legs; an `introspect` build's `--version` marker keeps it untimed; a mimalloc option, if a remedy, is a default the environment overrides.
Rejected: the platform allocator and `override`, off the path DataFusion tests on; `mallopt(M_ARENA_MAX)`, binding if set at resolution (RT10) but on C's arenas alone,
overwriting the operator's `MALLOC_ARENA_MAX`; DataFusion behind a feature (off, no figure times the download; on, a slim build nothing ships or times); two artifacts, or a
second binary the provider figures would time. Reopens: a contention figure; a mimalloc-only gate failure whose confirmed mechanism no library change suiting both allocators (ranking first) and no mimalloc option reaches. Evidence: `allocator`.

## The compressed source and the cache
### D14 `.xz` is read; recognition sniffs content
An `.xz` dump is third-party post-compression (input reach); gzip/zstd are `pg_dump --compress` output (compatibility). Recognition compares magic and sits outside the trait, and is
*told* those bytes by `Origin`'s probe — which also answers D20's size, so what is settled before a source exists stops being a statement about files. Rejected: extension dispatch; a
probe cached across failure. `xz-seek` is a vendored read-only copy (`scripts/vendor_xz_seek.py`); a bug is fixed upstream, and so was the seam a fetched source needs — nothing can
`await` inside `CompressedSource::read_at`, so the walk and the block handle are sans-IO and driven from here. Rejected: a path dependency, failing `cargo check` without the sibling
checkout.

### D15 A read decodes whole blocks and retains them; reading one in pieces is the fallback
Whole-block decode, LRU-retained, afforded at one reader against the caller's number from file-wide inputs
(`BlockCache::affordable`). Rejected: a fixed refusal line; keying on block count. A file whose largest
unit the budget cannot hold reads through one live handle behind a mutex — one state machine both arms,
each written once over the `Codec` trait, run over their own `Positioned` read, never one body branching on the provider — kept
across reads so a forward scan continues, advising one partition (`KD35`); the fetched arm's window is
charged rather than booked unpooled. A miss on a block in flight waits on its decode (`BlockCache::claim`). Evidence: `reserve`.
The `Codec` is `dyn`, an indirect call a unit or read. Rejected: generic arms, holders being `dyn` (D6) and `open` choosing by magic; an enum.

### D17 Two pools per source; retained and free units share a pool's slots
The hinted unit drives both what a pool keeps and how it sizes, so one pool is wrong for either
unit. Eviction runs before acquisition and the reservation drops before the evicted blocks do, or a
released buffer meets a reservation and is discarded. A reuse rule, not progress: see D5.

### D18 The seek table is persisted and identity is three-state
A many-stream file costs a footer read per stream, so `CacheMode::save` records the table, `CacheMode::open` reads
it before a source exists and `CompressedFileSource::with_index` walks nothing. `KnownCompression` is
`Unknown`/`Plain`/`Compressed`, checked against the file's magic; a contradiction is `Recognized::Mismatch`
and condemns the span index too, both having come from one save of one file.

### D19 A budget decline is a `PlanNote`; a non-seekable file is warned, not refused
A decline is a property of the file *and this run's budget*, and a `Diagnostic` is a finding about the file.
A one-block `.xz` file opens and raises `NonSeekableCompressedSource` naming `xz -T0`: only the user
can judge one decode-from-zero. A gzip file raises it where a complete index has a unit wider than twice its spacing, its writer having gone a spacing without ending a block, never where it is one unit for being under the spacing; its `is_seekable` is that rule, so the decline note never fires beside it, and a first pass is a note, the pass making the file seekable. A statistics skip is a note too, stated at zero wherever statistics were consulted,
and a narrowed span (D84) one wherever the plan charged less than stated; the ordinary case is the
invisible one. Rejected: omitting a zero; counting only believed filtered columns; announcing a narrowing
only where the count fell short too, which `ParallelismBudgetLimited` names; for gzip, warning by unit count, which says nothing of the writer, a persisted mark on the checkpoint opening a member's final block, keyed to one encoder's ending, and a gzip-only kind. Reopens: a puzzling zero,
which naming the columns lacking a statistic would answer.

### D112 `gz-seek` inflates through zlib-rs, by `libz-rs-sys`'s zlib C API
Of the engines exporting what an index needs through the zlib API, zlib-rs is the fastest at both of `gz-seek`'s costs, its first pass and a unit resumed from its checkpoint, and the one built with no C toolchain; why
its C API rather than a safe one is UF4's. zlib-ng and stock zlib stay features, the figure's legs, each a build of its own. Rejected: zlib-ng (slower at both; its compatible build CMake-built and exporting zlib's own names, `libz-ng-sys`'s native `zng_` API not the one the module is written over, `libz-sys`'s `cc` build failing to initialise); stock zlib (slowest at both);
the system's zlib (`libz.so.1` beside glibc, a dependency a release does not promise, D104); ISA-L (block stops only in rapidgzip's fork, its resume priming undocumented state fields). Reopens: another engine resolved ahead at either cost in a re-take, at a bump of any engine's pin. Code: `crates/gz-seek/Cargo.toml`'s `default`, `src/zlib.rs`. Evidence: `gzseek-backend`; RT23; `runs/p15.9-backend-legs/`, `runs/p15.9-zlib-ng-cc-probe/`.

### D113 gzip's checkpoint spacing is the block `xz -T0` writes by default
A shipped constant, never derived from the file. Within xz's block sizes a spacing costs gzip only its window section and index entries, the first pass and a resume's fixed cost not moving with it,
so it is set by what a unit shares with xz: the bytes a reader holds, the partition grain, and the default budget's block-decode line (D3), on which it falls where an ordinary `.xz` does.
Its unit is the finer of the two `.xz` block sizes the resident bounds were read at, where a gzip reader, its inflater lighter than xz's decoder, is billed no term above an `.xz` one's, and a koji-scale member's index has the entry count of the seek table `KD26` is argued over.
Rejected: a finer spacing, more window section and index for nothing on gzip's side, the power of two below refused at D3's line only for the chunk and anything finer under it; a coarser one, its retention list past any leg the bounds were read at, toward the block size whose legs overran `MEMORY_RESERVE`;
a power of two for its own sake, a unit running past each multiple to a block boundary and so never one; a row of `gzseek-spacing` at it, gzip's costs flat across the rows either side and the window section arithmetic.
Reopens: a resume's fixed cost resolved and not small at it; a gzip resident reading overrunning `MEMORY_UNPOOLED_BOUND`. Code: `CHECKPOINT_SPACING`, `source/compressed/deflate.rs`. Evidence: `gzseek-spacing`, `parallel-peak-rss`, `reserve`.

### D114 A gzip cache's windows follow its encoded file, and every save writes them all
Each checkpoint's window, in checkpoint order, after the encoded `CacheFile`, placed by the window lengths' running sum, the file whole only where it ends with the last; a load attaches a provider
reading them positionally through a handle of its own, so a load that decodes the envelope alone (`info`) reads none and none is resident. A save writes them through whatever provider the saved
index carries — a loaded cache's reading the previous file, which the rename over its name (D78) leaves open — so the section is rewritten whole, under D62's ratio, never patched. Rejected: windows
inside the envelope, resident at open before any budget exists; a file beside the cache, which D20's and D21's identity checks do not cover, as D78 refused one for statistics; appending in place,
which leaves a killed save no whole previous cache. Reopens: a save's copy of the section, not the index's, setting a save's duration. Code: `pgdt_dump::cache::file::windows`. Evidence: `a_cache_saved_over_itself_keeps_its_windows_and_reads_its_file`.

### D20 The library never replaces cache data automatically
A cache not ours, damaged, another build's or another file's (stored size, or a compression layer the source
contradicts) is almost always a wrong path or a changed file, so `CacheMode::refusal` refuses it before the dump
is read past its magic, at every scan entry point, each opening through `CacheMode::open`. `--overwrite-unusable-cache` (`with_overwrite_unusable`)
starts cold over one recognisably ours, by a header read first, for a file replaced under a stable name; foreign
bytes never. Rejected: a cold start (a silent re-scan after a typo or an upgrade); the guard in `CacheMode::save` (policy
an embedder cannot override); refusing a damaged one of ours (nothing readable lost, an unbumped version its likely
cause); `load` checking size alone (a claimless open gets another file's map). A moved block is `CachedBlockChanged`.

### D21 Identity is `stored_size()` plus weak signals, in an opaque enum
`stored_size()` keeps the check a `stat` where `extent()` needs a decompressing source opened first, and `SourceIdentity`'s variants are read through
signal accessors, so two kinds compare rather than refuse. **Two questions, split by tense.** Between runs a weak signal — modification, and where a
source was fetched from — is advisory, its warning reported and never persisted, and binds under its own `StrictIdentity` term alone, a missing time included.
During one the cadence follows the cost of asking: `SourceWatch` re-reads the open descriptor at the save's cadence (D62), at run end and before a
failure is reported (`attribute`: a short read or bytes that do not parse is often the change itself), aborting without saving or removing, where a
server compares every ranged GET against the probe's validators (`Last-Modified` for a weak tag, `If-Match` comparing strongly); a source nothing can
check (`in_flight_unchecked`) is refused unless `NONE`. Rejected: a check per chunk (rows still reach a caller first); an unpinned run warned of.

### D87 A remote cache is named from the URL, and the origin it records is advisory
Default `./<last URL segment>.dtcache`; no last segment is refused by name. Another origin is a diagnostic,
the stored size still refusing (D20), so two same-named dumps of equal size from different hosts in one
directory read each other's map with a warning the origin makes legible. A local cache records none, its
default sitting beside the dump. Rejected: a canonical path as a local origin, advisory after every move.

### D90 The DataFusion provider reads only a complete cache, never maps, and is cancelled by a drop
`PgDump::open` believes a cache reaching the file's end whose identity checks pass, and refuses anything short, carrying
the mapping pass that builds it; `pgdt sql` never parses either, a parse wanting `pgdt`'s discovery, interrupt
guard, status lines and cache rules. So every block a table owns is seen (`KD6` is the cold query's), a schema is
stated from every block's census, no database's DDL is unread, and the partitioned replay is always available. With
no partial map to keep, a dropped stream is its whole cancellation, as every query's is (D26). Rejected:
falling back to a cold query's early-stopping map, which brings each of those back. Code: `crates/pgdt-query/src/dump.rs`, `crates/datafusion-pgdump/src/dump.rs`.

### D22 `Held` is its own type and `CACHE_FORMAT_VERSION` is bumped freely
An incomplete cache is usable (or `map_forward` restarts from zero), `Disabled` is about the caller and
`Replacing` carries its reason to every caller. Bump on any persisted reshape, a parse fix changing what a span or
the metadata holds included; record it nowhere: an older cache is then refused until replaced by the flag or deleted
(D20), which is what a user hears of first. Rejected: an unbumped parse fix, its cache answering as the old parse did.
Code: `pgdt_model::CACHE_FORMAT_VERSION`. Evidence: `persisted_index_is_pinned_to_the_format_version`.

## The scanner
### D23 The scanner never owns the bytes it scans, and only the whole `COPY` grammar is structural
`CopyScanner` is a synchronous state machine over a caller-owned buffer; the only memory bound is `max_line_bytes`,
and exceeding it errors, never truncates (`KD93`). A chunk is scanned in two passes, carried line then chunk in place
(`ChunkedScanner`, every serial loop's, a chunk's boundary the loop's own to poll at); a growing buffer copied every byte twice. Only a line matching the whole `COPY … FROM stdin;` grammar,
or a bare `BEGIN;` (below), is structural, an off-grammar one being ordinary SQL; inside a block only an exact `\.` line and a row's ending are looked at (I7, I91). Outside
one every line is lexed as psql lexes it (`lex.rs`, I50), a line beginning inside a region is never structure, and a
dollar-quoted one emits no lines, only a closing offset (I1). A bare `BEGIN;`/`COMMIT;` pair is the large-object
region, skipped unread (I12). Rejected: tracking `$` alone, or quotes too (a `$$` in a name or comment hid every block).

### D25 Parallelize what is CPU-bound
Decode is always split, extraction on any source, discovery only behind a decoder; the worker is
fused. One core's rate against the device's offer decides each; readings that disagree reopen it.
Evidence: `xz-decode-scaling`, `parallel-scan-throughput`, `scan-throughput-*`.

### D26 Cancellation is `map_file`'s argument, polled per chunk or leader window by the mapping passes alone
A block can be hundreds of gigabytes, so block-boundary cancellation is a hang; `map_file`'s scan and back-fill keep a partial result. It
takes a `tokio_util` `CancellationToken`, required: options are reused across runs, and a fired token in one stops every later run at byte 0.
A query's plan and streams take none, a dropped stream being their cancellation (D90), nothing they spawn outliving one chunk's `spawn_blocking`; a cold query's pass (`Table::settle`) takes one, answering `ScanCancelled` where it fires. The preamble scan polls none, a stop there reading as the first `COPY`
header and caching as complete; the token handed to the source drops a read in flight, failing it unbanked. **The shape follows the
command**: `parse` interrupts, at byte 0 too, then dies by the signal, and a second one, mid-save too, or one in the listing, ends it in the
handler. As init (RT19) `pgdt` exits 128+n on every signal ending them elsewhere but a fault's, left to the kernel, and what `parse`'s guard
or the REPL's `ctrl_c` catches. Rejected: polling alone; a `ScanOptions` field; a token of our own; keying on what was banked (a race); `exit(128+n)` elsewhere (RT20); needing an init; as init, INT and TERM alone; fault handlers; an `Option<&CancellationToken>` inside, which still asserts and adds a branch per poll; a `Stop` trait whose `Interrupted` is uninhabited where nothing stops the pass, a type parameter through every pass to prove one gate.

### D27 UTF-8 is validated once per chunk, and the seven libraries' one shipped `unsafe` is the view append
`validated_prefix` validates the largest line-terminated prefix and fields slice the `&str` with
`str::get`, delimiters being ASCII. The escaped path still validates (`\xNN` synthesizes bytes); the
bulk pass runs only when a row will decode something. Four `unsafe` attempts lost to the safe shape;
the one that stands is `append_view_unchecked` (`batch/chunks.rs`), whose bounds come from the `contains`
that produced its coordinates and whose validity comes from the decode's borrowed arm (D46).

### D28 One row split, shared unconditionally
`RowSplit` memoizes field ends for every term and the batcher, extends as deep as asked, and is
bound to its row by a debug-only length check (a missed `restart` yields in-range indices into the
wrong row). Rejected: a term-count gate; a second `push_row` for the unfiltered path (a second
`push_field` site defeats inlining); stopping the walk at the last projected column, `push_row`
being the only `ColumnCountMismatch` site. Evidence: `predicate-terms`.

### D29 Refused micro-optimizations on the row path
Pre-sized Arrow builders (a one-row batch fills thousands of slots); a viewing builder for nested
`Utf8View` (the prize is zero below arrow's inlining width); skipping a block with `memmem` (the row
count and the census read every row); a proposal is sized against the per-row profile. Evidence:
`nested-decode-micro`, `map-only`, `cross-file-floor`, `nested-end-to-end`.

## The file map and the preamble
### D30 The statement grammar is primary; the TOC is enrichment
TOC-driven segmentation is unsound on non-`pg_dump` input, and a dollar-quoted body can hold a
TOC-shaped line (I3). A tiling hole is `TilingBroken`, never a refusal. A span's `end` is fixed by
the next push, so `Builder::snapshot` is possible. The preamble prepass (byte 0 to the first `COPY`
header, I1) runs up front in both mapping entry points unless the cache holds it, or an interrupted `parse` banks blocks with
no DDL. `attach_text` slices span text after building and `Data` spans store none, which is why
`query` cannot run cache-only. Evidence: `preamble-prepass`.

### D31 `Span::toc` is "belongs to", vetoed after classification; boundary rules that read as bugs
Follow-ons inherit `governing_toc` (else coverage reads half on healthy input) and `toc_owned`
counts objects; `Framing` vetoes after the transition that seeded the inheritance, and `Connect` is
never seeded. The boundary predicate refuses `"Data for "` and accepts `"Statistics for "`, which matters under `--disable-triggers` (I31).
A blank line does not close a pending comment (`_printTocEntry` writes `--\n\n`); `scan_preamble`
retreats to `pending_comment_start()` rather than guessing the comment's kind.

### D33 Bulk regions are one span kind; large objects skip at the scanner, `INSERT` runs at the map
Only `DataBlock::Copy` has inner offsets, only `COPY` having a row reader; a data span absorbs the
TOC comment pending at its start (`KD1` where none is). `INSERT` runs fold in `feed_line` with no scanner state (their
boundaries rest on no line-anchored invariant) and the run's end is lexed as the scanner lexes. Three cuts
stay untaken until the `INSERT` row reader exists (`KD9`).

### D34 `DumpIndex` stores no fact twice but sortedness, `metadata` and `scanned_through`; whole-file facts need `is_complete`
`blocks()` is filtered, `metadata` computed once, diagnostics never persisted, roles excepted; a streamed schema commits over the blocks it replays, ungated (I2). Statistics sit in
their block behind an `Arc`, so save-gate clones copy a reference (`KD5`), and store sortedness; a group is a byte range, no piece knowing a global row index. A back-fill keeps a
block's columns, and its size unless a stated size or bound it did not record moves it, a maximum it breaks re-reading it at the size its groups predict. Rejected: `SparseRowIndex`;
padded `character` bounds and entries, keyed alike but past the cap; a back-fill narrowed, dropping only re-read blocks' columns.

### D35 The census is type-blind, records both dimension bounds, and runs at the data level
`ArrayShape::observe` reads the leading brace run off still-escaped bytes in the model (I15, I25); min and
max depth, or `{1,2}` beside `{{1,2}}` resolves to a wrong `List`. A table any column of which is at
the data level is censused, and so is every block a query's pass maps: gating on a full scan left
early blocks uncensused. The metadata level records `None` and, but under a strict parse, splits no field; a typed query re-reads
such a table for itself, writing nothing, and a `Dump`'s plan refuses it. Rejected: degrading
to the DDL's `List`; a census-only level a user can reach; the census as a `Table`'s own step, which a `Dump`'s would offer. Evidence: `statistics-gathering`, profiled.

### D96 The unrepresentable count rides the census, typed by the DDL, in two tiers, under a recorded calendar
Every census-taking read counts per block and column what the declared type's typed pair (no census) cannot hold, a leaf
by its own type, lexically, the boundary years and an interval's long hour part alone by arithmetic; a nested value once, in its worst leaf's tier:
`format` past Arrow's spec, `engine` past `calendar_end` (RT21). It persists as counts (D74); a type holding more or
less bumps `CACHE_FORMAT_VERSION`, as D78's order does, and a cache counted under another calendar is refused (D20).
Rejected: a decode per field; a type-blind test (`text` reading `infinity`); counting as a query reads (timing decides);
one tier; a value the server refuses, a decode error, this being our front end's limit. Code: `unrepresentable::Counter`, `FieldCount`. Evidence:
`every_extreme_is_held_by_arrow_or_recorded`, `the_types_fixture_counts_what_its_typed_columns_cannot_hold`.

### D36 The preamble grammar dispatches on fixed keywords and never guesses
Unrecognized lines are ignored, so `--binary-upgrade` noise is free (I5, I6). `record_type` keys on name (I11); a composite's field
list is all-or-nothing, `record_out` being positional (I23); a `--create` dump's pre-`\connect` segment is not a database (I9). The model
stores text, never a type's conclusion: a declared type is its words, block comments and spacing dropped, a collation clause verbatim, `None`
collation is "no clause" (I37), and `CollationDef` keeps only `deterministic` (I42). A type's or collation's name is kept in one spelling,
both sides of a lookup compared in it (I29). A table keeps its `INHERITS` parents and `OF` type, a column found through them as it
is looked up, and a partition its parent and bound, the bound as text (I86). Of what the DDL implies, the model keeps only a column's `NOT NULL` and whether an enum's labels are whole. Rejected: a name's parts dequoted, `"a.b".c` being `a."b.c"`; references flattened in at the fold, which a later `ADD COLUMN` misses.

## Type resolution and decoders
### D37 The bar: the dump alone determines the value
A declared type maps to a real Arrow type only if its text round-trips consulting nothing outside
the file; otherwise `Utf8View` with a note naming the kind of unknown. Misreading is unrecoverable,
not recognizing is not; `money` fails it (`KD13`). Every column is nullable regardless of DDL, a
`NOT NULL` refusing a NULL instead, which `ignore` reads (I76); a range's three flags are not. Rejected: a bare built-in name `Unknown` where a declared type could
shadow it — `pg_dump` writes built-ins bare under an emptied path (I8), so it would demote its own
columns; a collation takes the weaker verdict because `pg_dump` qualifies every one (`KD48`).

### D38 The ADBC driver's shipped release is a floor, swept from the catalog
Where the driver yields a real Arrow type, ours is never wider; the floor is a pinned release, never `main`, one row per declarable `pg_catalog` type. `status`/`extension` columns take
a row out of the rule by themselves, and only what they cannot say gets a `Disposition` with a stance and a resolving citation. `floor_mapping.py` compares value space (`Utf8View` is
`string`) and reports an unknown arm, never guessing. It binds the typed mode alone: the untyped mode, like `--schema-mode strings`, is the user asking for a type wider than the floor,
and widens only a column the map says its front end cannot hold (D100). Rejected: the `typelem` shape test, which deletes `int2vector` (I8, I39).

### D40 The comparison is decided per declared type, in the same arm
`builtin_scalar` answers Arrow type and `CompareKind` both, per declared type and `COLLATE`; types
sharing `Utf8View` compare differently, and neither `predicate/` nor `key/` orders by the Arrow type, a DataFusion-semantics `ValueKey` carrying its column's only for the coercion `compare_value_keys` relates two orders by (D56). DataFusion
semantics, one per query lest rows depend on what a plan pushes, maps each kind (`datafusion_order`); no
term says a divergence (D59); every text-emitted kind maps to `Text`, `macaddr` too though its file
text orders as its octets, since a user's literal is compared bytewise. Rejected: a nested column
ordered as DataFusion does — a `TODO` there; for `macaddr`, refusing a literal not in lowercase, or
keeping the octet key for one that is, a per-literal switch bought for range pruning.

### D41 Array shapes: two refusals off one domain walk, six spellings to one level
An opaque element delimiter (I22) and an array element (I26, `KD3`) both resolve `Utf8View`, decided on
`domain_terminal`'s result by `resolve_array` and `array_comparison`. All `Typename` spellings collapse to element plus one level (I21, I28);
normalizing on parse would edit the user's DDL. A walk spends a visit per definition, the list's
length bounding an acyclic one (I24), so a cycle answers `Unknown`.

### D42 `interval` is the struct; its special values are unrepresentable
`MonthDayNano` is PostgreSQL's three fields, so text would be below the floor; infinities and
a time part past Arrow's nanoseconds are unrepresentable values (D96, D99), as `date`'s and `numeric`'s are. The six built-in
ranges and their six multiranges are fixed (I10); a user-defined range declaring `canonical` is unanswerable (I46).

### D43 `NestedPlan` travels beside the `DataType`; the census speaks after the DDL and moves the pair
An Arrow type does not name its literal (`int4range[]`, `int4multirange`: both `List<Struct>`), so
every builder takes a plan. Rejected: widening `builtin_scalar`'s tuple; `Field` metadata.
`retype_from_census` and the text mode's `read_as_text` (D100) alone change `(DataType, NestedPlan)` after
resolution, the census a parameter so no caller skips it; a run past `MAXDIM` is tested first and kept optimistic (I25). `MetadataNotScanned`
refuses a stream and degrades a listing: same output as `NotDeclared`, opposite advice.

### D44 The control's decoders allocate only where their return type does; its renderers use tables
The measured control's decoders take no intermediate `String`, `interval`'s aside, and its renderers write into one reused buffer
through `DEC_PAIRS`, `uuid`'s and `bytea`'s into one pre-sized `String` each through `HEX_PAIRS` (`tests/render_allocations.rs` pins the count); `interval`'s, a decimal's and a
float's still go through `core::fmt`. Render-back's third outcome, `FieldRender`, is an Arrow value no text form spells (I40), never rounded.

### D45 One quoted-token scanner; strict decoders apart from permissive parsers
Array, record and range literals share one scanner parameterized four ways (I20). `decode_*` reads
what a dump holds and cannot loosen (the render inverse); `parse_*` reads what a user typed and
cannot tighten; each transcribes the newest major and under-accepts (I35, I44). Wrong depth,
`[lb:ub]=` or a field-count mismatch is refused, never reshaped. Evidence: `nested-decode-micro`.

## Batches, streams and the leader
### D46 Zero-copy views are top-level `Utf8View` only, and `max_source_span` alone bounds pinned bytes
Every other arm copies. A retained chunk is released only past its *last* byte (the carried row arrives inside the next chunk) and
`invalidate_block_cache` runs on every flush keeping its batcher; any new flush trigger must honour it. `max_rows` and `max_bytes` count selected
rows, which a filter makes sparse. The span term is dropped only where the advice is non-empty and uniformly by-partition (`KD23`). Rejected:
compacting views past a selectivity threshold. Evidence: `parallel-peak-rss`.

### D84 The batch span is derived from the budget and the count, and spent before the count is cut
`max_source_span` is a ceiling: `plan_partitions` charges `(budget − charge.at(jobs)) / jobs`, floored at the caller's
announced `ReadOptions::chunk_size_bytes` — one read chunk, below which the span costs rows and bounds nothing the
retained unit does not — and `plan_replay` writes it onto the plan the sub-streams share. A plain source stays at most `DEFAULT_MEMORY_BUDGET` whatever
is stated (D83, `KD32`), which the shipped span spent whole, so `--jobs` bought no readers. Rejected: a third flag
(`roadmap.md`, "Two tunables fit pgdt to hardware"); pricing a plain reader (`KD25`); a *larger* floor, a performance
claim with no batch-size figure behind it; the *shipped* chunk as the floor, which at `--chunk-size 64k` declines
readers the announced one seats. Reopens: a floored span measurably slower than an unfloored one. Code: `stream/replay/partition.rs`. Evidence: `parallel-scan-throughput`.

### D48 Mapping and replay are separate passes, and `splice` owns the seam
The map is never behind the rows, so a `ResumeToken` points inside mapped territory. A segment is spliced by extending the
*preceding* span; a start floor on `Builder` made assembly visible in the map. A cold query's pass (`Table::settle`) yields its
table and no row, stopped by its token or a drop (I1), taking the extent and schema mode it reads as arguments. Cached replay and the
cold interior split share `worker_count` and `cut`, differing in what they cut and in the replay charging each sub-stream its batch span
(D84). Rejected: the pass taking the `QueryOptions` it is planned under, its `Table` planned under others anyway (Strings then Typed refuses, D35); a value for those two alone.

### D49 One target per query, the early stop conservative, and `ResumeToken` opaque, fingerprinting the query
A `TableSelector` narrows to one `TableName` before replay or `AmbiguousTable`; `target_settled`
vetoes a stop on a partition root (I2) or any `\connect`. A conflict past the stop is unseen (`KD6`).
The token's filter is hashed by explicit match, as a derived `Hash` silently misses a new operator; the
batching knobs, `postgres_invalid_values` and `use_statistics` stay out, no skipped group holding a resumed row.

### D51 A segment's offsets are search bounds, and a resync is a real read
The first row is past the first LF at or after `start`; the piece runs to the first LF at or after
`limit`, so a cut at a row's first byte leaves it to the earlier piece. The scanner is never started
mid-row, since a value can end in `\.` (I7 covers line starts). The first piece reaches back over
the header, as a pruned block's run of group 0 does. Sub-streams are contiguous byte-balanced runs
in file order, so concatenating them *is* the serial replay. Rejected: cutting at block boundaries,
sparing a declared ordering its proof across them (`summary::partition_orders`) at every table's balance.

### D52 The worker is fused, and the earliest failing piece is the error
A piece reads its range and parses each read in `spawn_blocking`; `scan_piece` does no I/O. Rejected: decode and parse pools over a
channel (the ratio is a property of the command); speculative splitting; cross-block pipelining. Pieces drain in file order so the same
truncated file names the same byte. `close_copy_block` has one body and two callers, or a parallel cache stops matching a serial one's.
A failure only reading finds is the lowest-indexed failed sub-stream's, after the rows before it; a resolution refusal comes from the
plan (D54). Across a DataFusion query's partitions the first refusal wins, each true; rejected: ordering them.

### D93 A dynamic filter's rows are evaluated only under `pgdump.dynamic_filter_rows`, off by default; its state is read at each group entered and each chunk
Off, a state still prunes groups, cuts the sub-streams and stops a sorted block, its stop asked of each kept row where armed; the setting states intent,
which "Two tunables" does not refuse, and is the one way to the unclustered join's win, DataFusion's flag taking pruning with it. On, rows are evaluated
in every block, statistics or not (a column at the metadata level, D85's declined block, `KD33`'s tail), less each bound an `IN` beside it implies, no `Not` above
(D54). The state is read where its generation moved; a read mid-group judges that group again, skipping its rest with each later group it rules out.
Rejected: on by default (`dynamic-filter-join`'s costing row); no switch; evaluating only where statistics delimit groups; reading per row (a shared lock),
per batch (aging) or at group entry alone (older where groups merge); ceasing where nothing is rejected (D77); dropping a static filter's implied bounds,
which raise. Reopens: that row's rows leg within its on leg's spreads (`KD55`); the per-chunk check priced. Code: `stream::RowEvaluation`, `DynamicRead`.

### D95 A dynamic filter enters by `TablePartitions::under` alone, and the byte cut is made once per handle, at the first poll
A replay may drop any row a state it read rejects, so the filter is no `QueryOptions` field: the one entry point that never resumes takes it (D77's
reasoning), with the row evaluation it runs under (D93). Planning stays at `scan()`; the first sub-stream polled cuts them all over the groups the
static filter and the state then keep, a join's filter being complete by then, so D51's balance holds over what is read, at the plan's count; the
planned cut stands where nothing is ruled out or an order would be lost (`KD53`). A group is asked once per state, the cut's verdicts seeding a replay.
Rejected: the cut held by `TablePartitions`, outliving the filter a reset discards; keyed by the filter's `Arc`; the count re-planned (read at planning);
re-pruning a whole block per generation; a partition reading its filter from state shared with the plan node, which its clones share, so a reset node hands out the filter its producer discarded; seeking the scanner in place at a skip.
Code: `stream::DynamicPartitions`, `prune::DynamicPruning`. Evidence: `crates/pgdt-query/tests/dynamic_filter.rs`, `crates/datafusion-pgdump/tests/dynamic_filters.rs`.

### D98 The typed mode nulls what its front end cannot hold, tested before decode where the block's count says it holds one
Under `UnrepresentableMode::Null` a value past the tiers the query's semantics cannot hold — the format spec's under PostgreSQL's, the calendar's too
under DataFusion's, the semantics naming its front end — is NULL to the batch, every filter leaf (static, dynamic, a sorted stop, a dictionary entry)
and statistics (D97), a sum leaving it out; a nested value holding one is the NULL whole. It is tested lexically (D96) before decoding, only
in a column whose block counts one in those tiers or carries no count. Rejected: nulling what fails to decode (an engine-tier value decodes, and
unparsable text in a field holding no such value must still refuse); a tier option beside the semantics, two fields for one fact, the semantics naming
DataFusion, whose display is the calendar's; testing every block. Reopens: another front end, with semantics of its own; `arrow-cast` displaying
past `chrono` (`upstream.md`, "UF2"), retiring the calendar tier. Code: `unrepresentable::UnrepresentableRead`. Evidence: the typed mode's cases.

### D99 The refuse mode refuses at planning, per materialized column, from the map's count over every block
Under `UnrepresentableMode::Refuse` a plan refuses the first column it materializes — projects, or hands DataFusion — whose blocks count a
value in the tiers its semantics cannot hold (D96, D98), before a row is read: over every block, not the groups a filter keeps, which
survive by statistics, and statistics never change an answer. A column only a filter the library answers reads is not materialized, and
compares every value in PostgreSQL's order (D56), so no read refuses one. Rejected: refusing where a read reaches one (timing decides);
over the kept groups; a read-time refusal beside it, the count being exact (D96). Code: `stream::replay::plan::materialized_unrepresentable`.
Evidence: the refuse mode's cases, `a_selected_special_value_still_cannot_be_materialized`.

### D100 The untyped mode widens a column per table from the map's count, and compares it in each semantics
Under `UnrepresentableMode::Text` a column whose blocks count, over every one, a value in the tiers its semantics cannot hold (D96, D98) is
`Utf8View`, settled once per table as the census is (D43), the rest typed; `UnrepresentableValues` keeps the declared plan under PostgreSQL's
semantics, special values ranked (D56), and is `Refused` under DataFusion's, its text bytewise and no bound gathered in the type's order read,
so a pushed filter stays `Exact` (D88); the column records the semantics, and a key asked for in the other is `None`. Rejected: text everywhere, `pgdt --where` refusing its order as an unmapped column's; per block, two
blocks of one table typed apart; the declared plan in both, each reader of a column's comparison then asked which semantics widened it; the key taking its semantics from the column, which one never widened has none to give.
Code: `resolve::read_as_text`, `stream::replay::plan::TableColumns`. Evidence: the untyped mode's cases,
`the_untyped_mode_compares_its_text_column_in_each_semantics_order`.

### D103 The value modes are one option each for every column, fixed where a dump is opened, apart from the schema mode
`QueryOptions::unrepresentable` and `::postgres_invalid_values` are each stated by a `pgdt query` flag, `PgDumpOptions`, a `pgdump.*` table option and
a shell suffix; the text mode changes a table's schema, fixed at `PgDumpTable::build`, so no `SET` reaches it; a library refusal names a mode in its own
words, never a flag. The invalid-values mode is a parse's too (`map_file`'s argument): an ignoring parse keeps no statistic of a refused field's group a read could contradict and its
block records per column the fields it went past, so a `Default` parse tracking such a column fails over the cache unread; `Strict`, a parse's alone,
checks every field, marks each block it checks whole and re-reads each held block unmarked — first, when resuming — never the record, which holds only
what an ignoring parse keyed: a verdict is the dump's, not the gathering run's. Rejected: a per-block record; a `pgdump.*` setting; a third `SchemaMode`;
recording the mode; refusing such a cache (D20); `Strict` failing from the record. Defaults: `Null` (D38); `Default`, a restore refusing them. Code: `PostgresInvalidValues`, `checker`.

## Predicates
### D53 The operator set is closed but for membership and the unrepresentable test
No `LIKE` (collation-dependent folding), `BETWEEN` (`And`), or column-to-column; `IS [NOT] DISTINCT FROM` is what three-valued logic forces. `IN` and
`NOT IN` are `PredicateOp`s holding their list, `NULL` included: one column against literals, as PostgreSQL parses `= ANY` and `<> ALL` over consts
(`transformAExprIn`). `In` answers as the `Or` of `=` but decodes once and looks up once, since per row that `Or` costs per term (`dynamic-filter-join`);
`NotIn` is its truth negated, a variant as every test's negation is, and only `In` drops the bounds it implies (D93).
`IS [NOT] UNREPRESENTABLE` is what the null mode forces, its NULL otherwise the dump's to every operator (D101).
Rejected: `Expr::In`, a second kind of leaf for an embedder to build; recognizing the `Or`, every front end emitting `In`. Evidence: `tests/membership.rs`.

### D54 One tree, no planner yet, short-circuit defined against the root
`filter` is one n-ary `Expr`, by default the empty conjunction. `And` may stop at the first `Unknown` except beneath `Not`, since only the root's `True` matters; a decode failure
surfaces only where evaluation reaches it: never in a group statistics rule out, nor serially past a sorted block's stop, but split a later piece evaluates one row past it, so
outside the contract `--jobs` decides. Resolution refusals come from the plan before any row, for the first refusing block in file order, walking leaves the evaluator would skip,
and in a block names first, a column it lacks before what any term asks (`check_columns`); a block with no column list refuses where reached. Rejected: DNF; exact Kleene
everywhere; not skipping a group holding an unkeyed value (a nested column, `KD2`'s) or under a term naming one; the stop asked first, every valid dump paying for invalid text;
names checked across blocks first (a pass more); a mode first (its remedy fixing no typo). A planner is deferred, not refused, until it buys more than it costs; a resolution-time
rewrite removing redundant work (one decode a row per field, bounds an `IN` implies) is admitted where a reading shows it pays. Evidence: `tests/pruning.rs`, `tests/ordering.rs`.

### D55 A literal is read in the type's `*_out` form at least and its `*_in` grammar at most
`*_out` is the 1.0 floor, `*_in` the ceiling: a literal `*_in` refuses is refused. Between them effort is minimized: an `*_in` spelling is read where free,
simpler or faster, none is refused at runtime cost, and the rest are `PredicateValueDecode`, a shortfall, never a rule. What a kind reads past its floor (one value's
other spellings, field and literal alike; `jsonb`, its canonical form untypeable) is on `accepted_form`. A literal finer than a decimal's scale is refused. `JSONB_MAX_DEPTH`
is fixed: a Rust stack overflow aborts. A field fails a parse only where a `pg-refuses` check refuses it (`decode::Unread`). Rejected: failing on every field read as
no value, which aborts on a shortfall a restore reads. A reader keeps its narrow grammar and a port of `*_in`'s classifies only what it fails, refusals included; rejected: widening a canonicalized kind's reader, so `=`, comparing spellings, misses a field read in another (`KD82`) and `macaddr`'s one bounds set (D79) stops serving both orders. A reader past the ceiling is replaced by its port (`network_in`, compared decoded). A field naming an undeclared enum label is refused only where the preamble read the labels exactly or no label is that long (I70); a literal naming one, always (`KD86`).
A narrowed `*_in` refuses only where every major does (`oidin`, `line_in`). Rejected: the newest or any major's, refusing what an older restore reads; the dump's own major, a dump being read alike whichever wrote it. A server's settings are taken at their most permissive, zone names open (tzdata grows unbidden) and abbreviations closed, PostgreSQL's files by release and tzdata's by its own policy (I83). Rejected: a file of the server's own shadowing a keyword, forgoing `13:00 pm` and I61's era edge for a server nobody runs; the tzdata at hand, refusing a zone a later one names.

### D56 Special values are a rank in the key; equality has three canonicalizations, by injectivity of `*_out`
`infinity`/`NaN` are their position in PostgreSQL's order; `OrderKey` derives no `Ord`. Rejected: excluding the row like a NULL (I33, I34).
`ValueKey` carries the order it was read in, checked first, so two keys compare where the semantics' engine relates their orders without a setting; a nested order names each node's type where the server tells two apart — a leaf's, a range's at every level, a composite's below the top, a domain's as an element's or field's — and relates leaves of one type as scalars (I92). The `OrderKey` a column's own keys share carries none. In DataFusion's semantics two orders relate by DataFusion's own coercion, cast and kernel, in `datafusion-pgdump`, a key handing over its value in its emitted type, none where that type cannot hold it (D96); DataFusion is the oracle (`key_order.rs`). `character varying` is an order apart from `text`, moving no key, because the server casts it to `bpchar` against a `character(n)` and `character(n)` to `text` against a `text`. Rejected: a variant per order (every keyed field pays, for a comparison nothing here makes); a declared-type tag ordering a scalar (`varchar(n)` and `text` order alike); a nested key related by orders alone (relates `text[]` to `varchar[]`, and composites differing only by `text`/`varchar`, which the server refuses); cross-plan answers left unspecified; DataFusion's cross-order answers reimplemented in the codec, or arrow-cast linked there (D74), and the caller passing each key's Arrow type, to the comparison or beside its plan at construction, which nothing checks (`ValueKey::of` reads both off one `ResolvedColumn`).
`v=1.5` hits a `numeric(10,2)` written `1.50`; rendering `timetz`, `inet` once is refused (I33, I38, I41).
A `bytea` literal renders in both `bytea_output` forms, which share no spelling (I56); decoding per row is refused.

### D58 A nested column has one comparison path, and the leaf grammar does not widen
Structural key walk for both operator families; `nested_key`'s literal `Side` stops at the container
but for `int2vector`, whose elements `int2vectorin` reads. Both sides of a range go through
`make_range`, keyed on the range type; a user range declaring `canonical` is refused under every
comparing operator, the NULL tests reading no value (I44–I47).

### D59 Divergence is per position on its own channel: per term, and per column at registration
`ComparisonNote` carries a path and its declared type, per term from `comparison_notes` (operator-
conditional, so query's) and per column from `Table::divergences` in a query's semantics — DataFusion's variants
there alone, as DataFusion's `ORDER BY` reaches a column no term names. See I45, `KD7`, `KD10`. A provider registration
folds its `UnknownCollation` columns into one `UncollatedColumns` finding, every column of a collatable type with no `COLLATE` earning one.
Rejected: the front end folding them, re-reading the provider's sentences to recover a table and a reason.

### D60 `--where` is a second flag and the tokenizer defines the refusal set
`refuse_where_structure` refuses a `--filter` term tokenizing to anything but one leaf, an empty one `parse_filter`'s, so a string
both flags accept means one thing. A keyword needs whitespace, a paren or the string's end on both sides
and `NOT` after `is` stays in the term. A term splits at the earliest operator outside quotes, longest
first; quotes are stripped in a term, under either flag, and in no name flag. Rejected: `&&`/`||`; backslash escaping.

### D88 A pushed filter is `Exact` where the plan resolves it; a literal is the renderer's text
`supports_filters_pushdown` asks `Table::schema` to resolve the translated term in DataFusion semantics,
the scan's own refusals. A typed literal must be the column's Arrow type and is written by
`render_field`, the decoders' inverse; a string one stands only where the library compares text.
Rejected: a formatter per type in the provider, a second grammar to drift; pushing `IN` on a float,
answered from a set with no `-0` made `0`; `Exact` where the library agrees with PostgreSQL, which answers an enum `<`
in declaration order pushed and in label order kept; `Inexact`, promising a superset another semantics can break.
Evidence: `crates/datafusion-pgdump/tests/pushdown.rs`.

### D101 The unrepresentable test reads the text against the declared type; DataFusion's is a scan's alone
`IS [NOT] UNREPRESENTABLE` tests a field's text in the tiers its semantics reads (D98), whatever the mode or the column's resolution,
two-valued; a group answers it off its own count (D97), and `SchemaMode::Strings` refuses it. `pgdump_unrepresentable(<column>)` is pushed
`Exact` and refuses where DataFusion evaluates it; registering a dump registers it and leaves planning alone, and a physical optimizer rule
readying a `SessionStateBuilder` appends refuses at planning any node's expressions holding it (RT22). Rejected: a table function or a companion column
(no row identity; 55.1 has no hidden columns); evaluating it (a NULL keeps no origin); registration installing the guard (a rebuild, dropping prepared
statements); a wrapped planner (one slot, trusting an `Exact` conjunct gone); the function opt-in too (withholds the always-sound pushed use).
Code: `unrepresentable_tests`, `datafusion-pgdump`'s `ready_session`. Evidence: `the_unrepresentable_function_*`, `tests/pruning.rs`.

### D94 A dynamic filter is translated loosened into the library's tree, never read through `PruningPredicate`
Every producer re-checks its rows, so the scan answers `No` and only ever skips. The state is translated as a static filter is (D88), a part with no
term standing as whatever keeps every row beneath its `NOT`s, so no state refuses; a `CASE` is the `Or` of its branches, their `And` under an odd count of `NOT`s. A float compared with a zero
has no term: a TopK and `MIN`/`MAX` order the zeros apart where evaluation equates them, and this holds whichever order a producer keeps. Held filters
are dropped at `reset_state`. Rejected: `PruningPredicate` (bounds decoded to scalars, pruned in DataFusion's semantics, nothing on a `CASE`, a long
`IN` or a sorted block); a narrower zero rule, sound only as far as each producer's shape is followed; keeping filters across a reset as Parquet does,
a recursive query's next iteration needing rows the last one's ruled out. Reopens: a release whose producers hold with the zeros equal, then an `RT<n>`.
Code: `datafusion-pgdump`'s `filter::dynamic::loosened`, `exec.rs`. Evidence: `filter/dynamic/tests.rs`, `tests/dynamic_filters.rs`, `tests/statistics.rs`.

## Statistics
### D75 Pruning takes only what each operator family proves
Bounds answer the ordering operators, and under an equality operator rule out "equal" but never "unequal"; a dictionary answers the
equality operators; a stop needs an ordering term the root conjunction requires, on a column sorted the way its bound closes; terms
combine as if independent. Rejected: bounds proving "unequal", `Canonical`/`Trimmed` comparing spellings, one to a key only in text
`*_out` wrote; and, though sound, a dictionary answering ordering, or a stop under `=` or `NOT` over the opposite operator. Reopens: a
filter shape a reading shows common. Code: `ResolvedTerm::truths`, `prune::SortedStop`. Evidence: `tests/pruning.rs`'s generated check.

### D76 Gathering never keys a bytewise value; a bound past the cap is a prefix and a successor
`text`, `character` and `bytea` order by a clipped head (`gather::clip::Clipped`), not an `OrderKey`: a key copies the whole value, and a row
may run to `--max-line-bytes`. Values agreeing on the head share every byte a stored bound reads, so bounds stay valid and only row
order is lost (`Unsorted`), where a decoded-kind value past the cap loses its group's bounds too. A truncated text `max` replaces its
last character by the next scalar value (`text_upper`), `bytea`'s increments its decoded bytes. Rejected: the byte successor for text,
which can end mid-character. Evidence: `bounds/tests.rs`'s `a_long_texts_stored_bounds_are_on_the_right_side_of_it`.

### D77 The statistics request is the mapping pass's argument, and the metadata level is a selection
`map_file` takes a `&StatisticsRequest` and no query entry point does, as with the cancellation (D26), so no option a query is handed
reads as a request it ignores; a query's pass censuses and gathers nothing, which no selection spells.
A selection is a default level and overrides, the most specific winning, so the type's `Default` is
the data level everywhere. Rejected: a `ScanOptions` field; `Option<&StatisticsRequest>`, whose `None`
contradicts that default or spells it twice; refusing a size beside `METADATA`, or two equally specific
entries, in the library (the CLI refuses both, a person's intent being ambiguous there). Code: `statistics::StatisticsRequest`.

### D78 Statistics share the cache file, its identity and its `CACHE_FORMAT_VERSION`
None is believed from a cache sized unlike the live source (D20); the mtime stays advisory (D21) so a same-size rewrite prunes against its predecessor's, knowingly. Bounds, order and
dictionary hold only under the declared type and `COLLATE` recorded with them, and changing how a kind orders or equates bumps the version. Both ends stream; a save renames its own file
over the cache, so no copy meets the statistics and a kill keeps the last save, orphaning its file. Rejected: a separable statistics file; its own semantics version; the copy charged;
one beside name two saves share. Code: `prune::prune_block`, `CacheMode::save`. Evidence: `golden_order_is_pinned_to_the_format_version`.

### D79 Bounds go in each order a semantics compares by exactly, a dictionary where it equates exactly
A scalar column keeps bounds and row order per exact order, one set where two coincide: the register's where exact, else DataFusion's
(bytewise: text in any collation, `jsonb`, off-`C` `character(n)`, no plan, no DDL), and DataFusion's beside an exact one it differs from
(a D78 bump); `macaddr`'s serves both (I40), and a float's, its tie of `-0` and `0` broken as `total_cmp` breaks it. A term reads
the set keyed by its compared kind as gathering resolved the column, not by its plan. A dictionary needs equality alone, reaching
`KD7`'s text, never an Arrow encoding (D37); nested columns get neither. Rejected: gathering a distinct count (D89 derives one);
bounds off a dictionary (D75); one slot; a second `macaddr` set, or a float's for one pair of values; the zeros told apart in the
key, which every term equates them by; a kind stored per set; a DDL flag; gathering keyed by plan and semantics, not the set's kind (the kind names the set where it is stored, read and reopened, no semantics maps onto a set uniformly — a divergent column's primary set is DataFusion's — and `ValueKey::of` reports no `*_in` refusal); gathering holding `ValueKey`, whose nested variant it never keeps, where a filter holds `internal`'s `OrderKey` (D106). Code: `bounds_kinds`, `bounds_set_keyed_by`, `compare_keys_stored`.

### D81 The statistics account sums allocation sizes as they change; an instrument scope checks it
Each term is the allocator's request — capacity, a table's layout (RT11, RT12) — under one lock, a
vector or map charged ahead of its growth, in flight made when short, unmade when over. Rejected: a
per-group bound, bytes never held; growth charged after it; atomic terms, a moving charge read in
neither; in-flight growth unmade both ways, faulting a table another worker holds; walking every
block; proportional slack. Reopens: a leg past its tolerance (`statistics_account.rs`); an unseen
term growing with the dump; the lock in `statistics-gathering`. Code: `statistics::account::Charge`.

### D82 A block merges pairwise, exactly, between its minimum and its maximum, never mid-scan while a piece lives
Exact, so serial, split and stated agree: a bytewise closed group keeps its extremes' heads till
`finish` clips them, a clipped upper bound not ordering as its value does; a merged dictionary
renumbers first-seen. A piece joins at its own size, so mid-scan a block waits till none lives, reopening
an odd last group; at `finish` one still alive joins nothing more. Rejected: merging stored bounds;
coarsening a piece at its join; an even count, which windows keep odd; the nearest-rank median, not
monotone under the cap; the cap yielding to a stated maximum block by block, a partial distribution the
leader's split would decide; a floor under the coarsening, a constant nothing prices. Code: `Gatherer::fit_cap`.

### D85 Statistics are billed against the margin, and a block that cannot fit declines
`statistics_allowance` is what the arrangement leaves under `margin_allowance`, carved after the workers, a count fixed before a
byte is read; a cache's are known first, the loading pass's `held`; a replay's plan keeps its table's, `pgdt query` carving its replay after the pass, around them. The CLI's is `--memory`,
else the limit, else half of `MemAvailable` (RT8), an embedder stating none declines nothing (D1). A declined piece declines its
block, dense group indices expressing no gap, and the account sees a window's pieces, so a decline differs serial and parallel,
as meant. `finish`'s last close is kept even past it; the next block declines at its first charge. Rejected: a constant bound,
an OOM on a wide table; coarsening to fit, a cache depending on its container; retrying every run; skipping the piece;
MEMORY_RESERVE again, carving the cap, not this. Reopens: the check's cost, or declining's saving. Code: `Gatherer::decline`.

### D89 Statistics reach DataFusion on a leaf plan node, and `Exact` means the bound is the value
`TableProvider` has no statistics method in 55 and `StreamingTableExec` answers unknown, so `PgDumpExec` *holds* one instead of parenting it: a
parent's child can be replaced under it. Rows are `Exact` off block counts; a bound where every group of every block contributed and the stored text
is the value, else `Inexact`; a distinct count the dictionaries' union — derived, not gathered (D79) — where every group kept one of emitted text
(I48), else `Absent`; all `Inexact` under a fetch below the exact rows or a pushed filter, whose rows are the kept groups' plus each unconsulted
block's. It reads each value in the query's view (D98); the refuse mode plans no scan over a column holding one (D99). Rejected: `Absent` for a partial
bound, giving up cardinality; the union as `Inexact`, a floor inflating a join's estimate; NULL counts withheld where a bound fails to decode; under a
filter, v55's rows or a guessed selectivity. Code: `datafusion-pgdump`, `summary.rs`. Evidence: `tests/statistics.rs`.

### D91 A sum is kept wrapped and handed over as `SUM` wraps; a byte size is the text's length
The value owed is DataFusion's `SUM`, which wraps (`add_wrapping`), not PostgreSQL's, and wrapping addition is associative: group sums at 128 bits
narrow exactly to the `Int64` an integer is cast to and an `oid`'s `UInt64`, and are a `Decimal128`'s own, which `SUM` widens unchecked. A `NaN` is
left out as a NULL, the sum read only in a view taking it as one (D98); a value that does not decode drops its column's sums (D54). Text bytes are
kept for every column, a cache being gathered typed whatever reads it: a `Utf8View`'s size, a `Binary`'s bound and an enum's labels beside its keys,
`Inexact` (D46); rows × width, a bit a boolean, `Exact` unfiltered; a nested column none, its text bounding no leaf; a filter bounds each by the
kept groups. Rejected: a float's sum; a bare `numeric`'s, emitted as text; a partial sum; a `bytea`'s decoded length, below `:strings`; `:strings`
sized by bounds and type widths, an invariant a type; a guessed selectivity. Code: `stored::Summand`, `byte_size`. Evidence: `tests/statistics.rs`.

### D97 A group keeps a view per tier only where it holds such a value, the values its type holds the base
A column whose type cannot hold every value counts them per group and bounds and orders the values it holds; beside them, where a
group holds one past the format spec, every value in PostgreSQL's order, and where one past the calendar, the values within it, for
that group alone, read in a `StatisticsView` that adds what it takes as NULL to the NULL count, believed only under D78's DDL. The
base is kept apart from each tier's extremes, allocated at a group's first such value, so a view folds the base with the tiers it
takes, exactly across pieces and merges (D82); every value PostgreSQL admits keys, a timestamp from PostgreSQL's epoch (I49).
Rejected: every value's bounds and the count alone; a set per view per column from its first row; a timestamp key of
`i128` from 1970, a width chosen where PostgreSQL's is inherited. Code: `gather::bounds::KeyedGroup`, `gather::order::ViewOrders`. Evidence: `each_view_bounds_and_orders_the_values_it_takes`.

## The CLI
### D61 `info` never scans
The surprise is that a scan happened at all; `parse` scans ahead, a `query` maps only what the cache
lacks (D48), and `--preamble-only` hangs off `parse`. It reports the cache through `CacheMode::open`, or `OfflineCache` with no source, never a `pgdt_query::Dump`, which holds no partial map, container or envelope. Rejected: a scanning fallback; `--no-scan`; a `Dump` where the cache is complete, a second path for one cache.

### D62 The save throttle is a ratio, and its gate opens splice too
A save is skipped unless `K` times the last save's duration has elapsed, which bounds overhead in
every regime with no constant to tune. The map is rebuilt at the gate's openings, not every
`CopyEnd`; splice's third opener matches the queried table, a superset of save's, `target_settled`. Rejected:
every N seconds or bytes; a disabled-cache floor. `KD5` remains. Evidence: `per-block-quadratic`.

### D63 The interrupt flag is read at both extremes; resume is the default
Per chunk and at every completed block, since neither alone bounds the response. A resumed scan is
span-identical to a straight-through one, which is what makes an interrupted `parse` saved work.
Rejected: `select!` over `ctrl_c` (drops the map); a `--restart` flag.

### D64 An omitted `--jobs` asks the source; provenance is the CLI's fact
Absence recommends, a stated count is never lowered, zero is refused. The arrangement is announced
in two lines, before and after the open, because a fresh `.xz` walks footers first; the limit is
read once, every pass carving from that reading. Whether a number was typed or discovered never enters `Parallelism`.
Logged durations are diagnostics, never figures: one stderr subscriber, no terminal detection.

### D66 Output of what `pg_dump` writes is byte-identical whether typing is on or off, but for a value its type cannot hold or an `escape` `bytea`
Every value renders back to its `*_out` text, which is what `pg_dump` wrote — a hand-written spelling `*_out` never writes rendering as `*_out` would — but one its type cannot hold, NULL in the null mode (D98), and an `escape` `bytea`, `hex` (I56), which `byteain` reads alike.
One contradicting its type is `FieldDecode` naming the strings schema mode, never a null. Rejected: Arrow's display; a `bytea`'s form kept per database or flagged, read by no comparison.

### D67 `--json` is the internal struct; a flag's help is its doc comment
The whole cache file, keyed by block, no version of its own, compact and streamed, group values in:
a script sums its own rollup, a table spanning blocks (I2) with no merge rule but statistics' sums,
`--detail`'s alone. Coverage is one line at the top, nothing below qualified. No `help` attributes but `sql`'s, upstream's style;
pages are snapshotted with width and bare-flag assertions. `sql --dump`'s value name spells its suffixes past the page, its spec line exempt from the width assertion, until one pass over CLI output (`roadmap.md`, "Future — wanted, unscheduled"). Rejected: `long_help` per flag; a rollup; that value name cut to a placeholder piecemeal.

## Layering
### D68 Seven crates, the module assignment Cargo enforces
core, model, io, dump, codec, mapping, query, each using only crates before it, dump and codec siblings, each module's crate and the reason for it stated once in
`concepts.md`; CLI and embedders above query; every member's dependencies, and each library's API namespaces,
held to tables in `scripts/layering.py`. Model is what the cache persists, apart from core so an edit there is the one asking whether the format bumps, and
below the parsers so the codec does not recompile with the grammar. Gathering decodes and builds no array, so is mapping's, with the pass handing it rows and the interior split
that pass leads; the ordering keys it shares with a filter are value semantics, so codec's; worker sizing and the cut are io's, the leader asking them. The tests' fixture vocabulary is `pgdt-fixtures`,
a member only a dev-dependency reaches, its edges, to mapping and what a call to it names, behind a feature. `gz-seek`, a gzip file's positioned read, is a leaf member beneath io, as `namespace-init` is one above query. Rejected: four layers
in one crate with upward deviations; the cache a crate of its own (the loader re-derives the map's diagnostics, so it is dump's); the mapping pass query's (it reads nothing of query); the graph held to the libraries alone (a new member unplaced, `namespace-init` open to a library); any `pub mod` a namespace (one line past `internal`); a test module included across crates by `#[path]` (rust-analyzer resolves nothing outside its package), or a test's own `#[path]` within one (`check.py --affected` reads a test's files off its `mod` lines); statistics a crate apart from query (its edges run both ways); codec and query, or dump and query, one crate (D74's boundaries).

### D74 Below the codec no Arrow, the codec pure, and the core's error naming no dependency
Core, model, io and dump never name Arrow; the codec and mapping only its schema types, and the codec `i256`; the codec is synchronous and does no I/O, nor does
the model; a decoder takes an unescaped field and returns a value, never a builder (query's, whose views marry array building to the read buffers, D46). Anything
persisted is model vocabulary — declared type strings, never a `DataType` or another codec conclusion, a statistic's bounds included. A contract over a block's
rows is model, implemented above (`pgdt_model::internal::BlockObserver`, `pgdt_model::internal::FieldCount`). The core error carries a dependency's error as an
opaque `Cause`, built by `map_err` where a crate meets it and reached by downcast; every refusal of a dump the provider hands DataFusion is `External(datafusion_pgdump::Error)`,
the core error its `source()`. Rejected: a `#[from]` variant per dependency's error, compiling Arrow, xz-seek and bincode into every crate; the core error
crossing bare on some paths; a decimal key (`OrderKey::Decimal`) of digit strings, a heap per field, or of a 256-bit integer other than `Decimal256`'s.

### D106 A crate's API is a designed surface, and what a boundary forces public that is not API is `internal`'s
A library's API is its root and the namespaces `NAMESPACES` lists, each its file's own module made public, declaring the API by `pub use` from `pub(crate) mod` children (E0364
refusing a `pub(crate) use` re-exported `pub`); an API item has one path, never also its crate's root. API is what an embedder of the crate would call given what its `lib.rs` says the crate is
for, whether or not one does today. What a crate above reaches that is not API is `internal`'s, a method so reached on an `internal` `<Type>Ext` trait, and no embedder names one; a crate above
adopts a lower `internal` item into its own API by re-exporting it, where it must be defined low but means something only above. No facade: nothing is published (D104), a crate is reached
directly, and a binding makes its own entry point. Library crates are `pgdt-<name>`; the DataFusion crates keep `datafusion-*`, the prefix that ecosystem finds a provider by.
Rejected: what was `pub` before the carve (forced by one crate's integration tests), or what an embedder names today, as the test of API; a facade re-exporting the crates (every public name
spelled twice); a published `pgtypes` crate out of the codec (its own catalog types, Arrow behind a feature: a product decision); an `introspect` reading in `internal`, or that feature exempt from the embedder rule (a counting allocator, which only a binary installs, calls `pgdt_core::instrument`'s `allocated` and `freed`: the readings are API to whoever turns the feature on, the hooks `internal`'s); the DataFusion adapter exempt so `EmittedValue` stays `internal`'s (a value in its column's emitted units is what any Arrow engine's binding relates two emitted types by). Code: `scripts/layering.py`, each crate's `internal`; `crates/pgdt-query/tests/embedder.rs`, the API walked as an embedder walks it.

### D107 A library refusal names its remedy in the library's terms, and each embedder spells it after the sentence
A library error states the condition and ends with its remedies as the API names them — a mode, a mapping pass, a selector naming a database — never a front end's flag or command, which only
that front end has. What a front end must spell, the variant says, never its `String`: a refusal a mode causes names the mode that answers it, raised where the mode is known (an ordering
operator under the strings schema mode is `TestNeedsDeclaredType`, as an unrepresentable test is), and an accepted form cut short counts what it left unlisted. `pgdt` appends its flag or
command for each, in parentheses and the library's order, by an exhaustive match over the variants, on every refusal leaving a command, through any context; `datafusion-pgdump`'s refusals carry
the mapping pass they need as data, and a front end's `Spelling` handed where a dump is opened or the factory registered is asked of each `Refusal` once, its words the `Error`'s field beside it and closing its message before DataFusion prints it, `pgdt sql` handing one
per grammar (`--dump`'s suffixes, a statement's options). `pgdt info` says a cache's refusal in the library's sentence, spelled as a scan's is. A log line names no flag either.
Rejected: a structured `Remedy` value on the error (a second taxonomy beside the variants); a whole sentence per variant in each front end (the wording twice, which drifted); every enum label listed (nothing bounds how many a type declares); `pgdt sql` spelling in its `run()` alone (the REPL's and `-f`'s refusals are upstream's `exec`'s to print); the spelling a variant wrapping any refusal (a match missing it silently, a refusal spelled twice representable), a field on every variant (unrelated to each one's condition), a wrapper not named `Error` (D74), DataFusion's `Context` (the spelling printed before the refusal), the `Refusal` the `Error`'s source (the sentence again down a walk, and a second error type to `?` unspelled), public fields (a refusal spelled after it left the dump). Code: `pgdt`'s `refusal::spelling`, `datafusion_pgdump::Error`, `datafusion-cli-pgdump`'s `spelling::spelling`. Evidence: `each_refusal_names_the_flag_its_remedy_is`, `each_refusal_is_spelled_in_its_registration_s_grammar`, `a_refusal_is_spelled_once`.

## Fixtures and tests (`scripts/`, `fixtures/`)
### D69 Fixtures are real `pg_dump` output on a pinned glibc image family
`postgres:<major>.<minor>-trixie`, exact minor, never `-alpine`: musl's `strcoll` is `strcmp`, so every text answer would be the `C`
answer. Four things move on regeneration (`\restrict`, `now()`, `--verbose` timestamps, OID drift) and nothing may assert on them.
Every `ColumnResolution` variant but `MetadataNotScanned`, which no scan produces, must come from a real fixture column (I36).

### D70 "Agrees with PostgreSQL" is a generated check
`fixtures/<major>/oracle/` commits the server's own answers, asked through two typed columns (a cast folds constants and derives
collation), every ordered pair, refusals recorded as `E<sqlstate>`, nothing version-gated, text asked under `C` and `"default"` both;
ICU is in one fixture and out of the oracle. Semantics are the newest major's, and `oracle_differences.py` checks the union rule (I35)
by classing every cell moving between adjacent majors as additive or not (I37, I38, I42), a non-additive one failing unless its
literal is exempt beside the invariant narrowing it, each move a row of the manual's "Where PostgreSQL majors differ", every row asked.

### D71 Registers are read out of their producer's source, and the join is existence
A `match` cannot be enumerated at run time, so `oracle_register.py` parses `compare.rs`'s, `compare/walk.rs`'s and `pgtype.rs`'s arms; `emitter_register.py` reads the literals `pg_dump`
appends in each function some reader consumes, and its long options, out of a checkout into `emitters.tsv`, extraction being generation and the join
over committed files the check, the checkouts being machine-local. Anchors turn a rewrite into a report. An arm or a literal is the finest closable unit,
joined by existence — an arm in the oracle's cases, a literal in fixture bytes at its own major — never by branch coverage; an exemption names what the
check resolves (`Evidence(file, needle)`, a consuming source line, an `I<n>`, a `KD<k>`, a compatibility row), a fixture exposing a `KD<k>` held failing (`known_failures.rs`) until its fix deletes
the row. Rejected: a C parser; a "pgdt does not read this" exemption, the map tiling every byte; the function list enumerated once, which drifted as the
readers grew (`READS` holds them together); a cross product of session settings, one variant each; a `DO` block testing the version in place of a sidecar file.

### D73 Round trips, asserted shapes, and a hand-verified signal path
`Typed` against `Strings` over a fixture and `encode(decode(raw))` against on-disk bytes; boundary
values are unit tests; blind at `v_box_domain_array` (I22), pinned directly. A shape under test is
asserted (`is_seekable()`, a stated `--jobs`), never assumed. No test races a signal: `CancelsPast`
trips the flag at an offset, and koji covers the real scale. The round trip holds that render inverts
decode; that decode is right is the value oracle's (`oracle/values.tsv`): each typed node as the server
reads it, by send function or arithmetic, over every `types` flag set with DDL (`value_oracle.rs`).

### D102 The unrepresentable category is read off hand-written extremes through DataFusion's own path, never listed
`types`' `t_extremes` holds PostgreSQL's least, greatest and special values of each arm mapping to a type but `Utf8View`, one column an arm,
and `t_extremes_nested` the nested shapes, by hand, PostgreSQL keeping no catalog of them. Each decoded is formatted by `arrow-cast` and cast
strictly to `Utf8`, an error being a value its type cannot hold, recorded with its tier and held to D96's count tier by tier, so an upgrade
moving the calendar fails the check; `floor_mapping.py` holds columns and typed arms to each other per major, so a new major's typed mapping
forces its extremes (D38). Rejected: the list kept by review; a bound written per type (the list again); a decoder's refusal as the test,
which took `24:00:00`; a binary type's cast, which reads its bytes as UTF-8. Code: `arrow_holds`, `floor_mapping.py`'s `extremes_problems`.
Evidence: `every_extreme_is_held_by_arrow_or_recorded`.

### D108 A hot-path change is priced by `--count-pair`, and a rise it resolves is placed by callgrind, then reworked or accepted
`instructions:u` on the shipped build at the tree's parent and at the tree, `release-profile`'s reps a shape a leg, a row resolved where one leg's fewest clears the other's most:
one build's counts move between runs, so a change is read against its own pair's spread, and a pair of identical sources resolves no row. A resolved rise is placed by callgrind's
`Ir` on the two legs — the change's where it sits in machine code the tree changed or under a frame of it, source changed or not; layout only where callgrind does not reproduce it or
no changed code sits above — then reworked into a shape as clean, or accepted; none is too small to place. A phase adopts the refusal by its spec. Rejected: `release-profile` sat twice
(three legs, its `parse` row short of gathering); one count a leg; determinized shapes (a workload never shipped); a sampled profile placing it (line tables move the code); a figure's
machinery (a count is no figure); a materiality floor; rework resting on codegen — inlining attributes, a cold continuation of `ChunkedScanner`'s pass switch, an infallible `end_pass`, an enum's name and labels behind
one `Arc`. Code: `measure.py`'s `cmd_count_pair`, `count_pair_resolution`. Evidence: `test_measure.py`'s `CountPair`; `runs/attr-36.4.3-20261016-callgrind/`'s `run.sh` and `analyze.py`.

## Releases (`release/`, `scripts/release.py`, `scripts/release_ci.py`, `scripts/debian_window.py`)
### D104 A release is `pgdt` for Linux, linked against one digest-pinned Debian-stable image that also builds what the register times
Two targets, Linux on x86-64 and arm64; macOS waits on a discovery path of its own (P34), Windows on an I/O layer that is `std::os::unix`. glibc, not static musl: the
shipped binary is the timed one (D13) and every figure a glibc build, so musl is a libc swap no sitting priced. A host build needs the newest `GLIBC_` versions, so the floor is a pin
following Debian stable, the `postgres:*` images' base: a new stable gets at least six months' grace on its predecessor, and by twelve the pin has moved (`debian_window.py`). One
`debian:trixie` digest, its packages read from the snapshot it records, builds both targets and every binary the register times, run in the same base, so one pin names the build's
glibc, the floor and every figure's runtime; `rust-toolchain.toml` pins one compiler for development, the register and the release; each pin moves through `upgrade-deps`, an
apparatus change. Rejected: `cargo zigbuild` (zig's clang for every C dependency, a floor suffix synced by hand, a stray `CC` silently compiling against the host's headers); a build
runner per architecture (arm64 rehearsed nowhere locally); cargo-dist (no glibc floor, no text from an annotated tag); the Rust pin in the image alone (development compiling on another compiler than the register times and a release ships).

### D105 The shipped build is fat LTO in one codegen unit, the leg retiring the fewest instructions on the typed query
`[profile.release]` restates the `release-fat` leg, so `profiling`, `bench`, the instrument and a release inherit it. Of the three legs fat retires the fewest `instructions:u` on every query
row, `pgdt sql`'s typed scan, the first-ranked workload, first, and the default more than fat on every row. Wall clock ranks no pair there: which leg leads in time has moved with the tree
between sittings, the layout one tree's clock carries, and a profile outlives every tree; so the count ranks a profile where wall clock does not resolve the legs. The default leg is held to
it in wall clock by the roadmap's downstream-build rule, closed in the code where it fails; every other setting's "small" stays in wall clock, no other figure counting. Fat is DataFusion's own profile (D13).
Rejected: Cargo's default (more retired on every row, a resolved gain traded for nothing); thin LTO (more retired than fat on every query row, and than the default on `pgdt sql`; its quicker build
a cost to us, not a user); user time as the ranker (as layout-exposed as wall clock). Reopens: thin's or the default's count at or below fat's on `pgdt sql`; another leg ahead of fat there in wall
clock, resolved, at successive trees; DataFusion moving its own release profile. Code: `Cargo.toml`'s `[profile.release]`, `test_the_shipped_profile_is_the_fat_leg_restated`. Evidence: `release-profile`.

### D109 arm64's archive is built on the maintainer's machine and attested by its release alone; x86-64's is CI's, with build provenance
CI's runner cannot hold arm64's fat link (D105), so the rehearsal's arm64 archive, built in the pinned image from the draft's commit, goes onto CI's draft by `release_ci.py attach`, with one `SHA256SUMS` over both;
an immutable release has GitHub attest every asset, which `installing.md`'s `verify-asset` and mise's `github:` backend accept alone (RL1, RL2): the asset is the release's, put there with write access, not which commit or machine built it.
The arm64 smoke runs in `release-publish` on real arm64, against the draft's own asset. `attach` reports whether CI's x86-64 archive is the rehearsal's byte for byte and never refuses, until readings decide what the property may carry.
Rejected: swap on the runner (`release.py`'s `--memory-swap` tied to `-m`, a cap sized against a peak nobody has read, a fat link paging); `ubuntu-24.04-arm` (the same memory and aarch64 backend); a larger paid runner; an arm64-only profile (reopens D105, the targets shipping different builds); a self-hosted runner for the arm64 build (a standing setting GitHub advises against on a public repository, up at each dispatch, `sudo nerdctl` on it);
a keyless Sigstore signature under the maintainer's identity (no source repository, so mise's `github:` backend fails it); the local build attested by a workflow's `actions/attest` (provenance that verifies cleanly and is false); the archive uploaded before the dispatch (no input carries a file, and `preflight` refuses a pre-made draft); two checksum files (`installing.md`'s one `sha256sum -c` broken); qemu-user here for the smoke (the last real-arm64 run of the shipped binary dropped); refusing on an x86-64 mismatch now (a release blocked on a property nobody has shown).
Reopens: a GitHub-hosted runner holding the fat arm64 link; RL1 or RL2 failing. Code: `release_ci.py`'s `step_attach`, `plan_attach`, `step_fetch`, `CI_TARGETS`; `release-publish.yml`'s `arm64-smoke`. Evidence: RL1–RL3; `test_release_ci.py`'s `Attach`, `Fetch`, `Workflows`.

### D110 The release suite is the round's, less the never-shipped measurement apparatus
`release.py suite` runs the round's nextest and doctests (`check.every_check`) but for each member whose manifest sets `[package.metadata.pgdt] apparatus`, the harness's instruments;
`mise run check` still tests every member. An attested build tests what ships: performance and the apparatus are verified on the maintainer's machine before a cut, never during one, and
the apparatus needs host facilities a runner need not have, `count-instructions` a hardware counter that GitHub's x86-64 runner did not open.
Rejected: lowering the runner's `perf_event_paranoid` (it answered the EACCES, not the ENOENT that followed); a counting test that skips where the counter does not open (a broken counter
passing on this machine too); the apparatus named in `release.py` (a new instrument tested on the runner until someone edits the list). Reopens: an instrument shipping, or a shipped member
needing a facility a runner lacks. Code: `release.py`'s `suite`, `check.Package.apparatus`. Evidence: `test_release.py`'s `test_the_suite_leaves_out_the_measurement_apparatus`,
`test_the_apparatus_the_suite_leaves_out_is_a_dependency_of_no_member`; `runs/release-v0.2.0/x86-64-suite-39530108777.log`, the EACCES before it in `docs/status/history/2026-10-20.md`, "v0.2.0 is stalled at its build".

### D111 CMake is a `mise.toml` pin for the apparatus, and a build is refused where a crate `pgdt` depends on ran it
CMake is pinned for the measurement apparatus, zlib-ng's `libz-ng-sys` building under nothing else, so the image installs it. CMake is not refused as such: what the shipped build must not do is build `aws-lc-sys` under its CMake
builder, whose compilation options are not its `cc` builder's. While no shipped crate needs CMake that is checked closure-wide: `release.py build` reads every build script its build ran or found fresh off cargo's messages, and refuses where one of a crate in `pgdt`'s normal-dependency closure, at `pgdt`'s features (`cargo tree`), left a `CMakeCache.txt`; scoped so because the apparatus builds in the same target directory.
Rejected: CMake absent, the guard `aws-lc-sys`'s fallback failed on by name (zlib-ng unbuildable where every figure's binary is built); `ENV AWS_LC_SYS_CMAKE_BUILDER=0` in the Dockerfile (one crate guarded by name while nothing shipped needs CMake, another's fallback onto it unseen);
Debian's `cmake` (a version no checkout shares); the closure off `cargo metadata` (one resolve for the workspace, so a feature another member turns on in a crate `pgdt` links lands its dependency in it, `quinn` among them).
Reopens: a shipped dependency built by CMake, the check then narrowing to `aws-lc-sys` by name, its builder forced and its `OUT_DIR` holding no `CMakeCache.txt`. Code: `release.py`'s `step_build`, `cmake_caches`, `shipped_tree`. Evidence: `test_release.py`'s `CMake`; `runs/M244-cmake-probe/`.
