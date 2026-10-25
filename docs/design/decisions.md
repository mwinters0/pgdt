# Decisions

The decisions the code cannot explain: shapes kept for a deliverable not yet built, defaults chosen
over an alternative, and obvious changes measured or argued and refused. Nothing here says how the
code works (the named module does) or quotes a number (`measurements.md` does, by figure id, as the
invariant registers do by `I<n>`/`RT<n>`). Cite as `docs/design/decisions.md`, "D12"; the rest of the
rules, the line cap included, are `docs/process.md`, "The decision register".

<!-- decision-watermark: D118 -->

## I/O, memory and parallelism
### D1 The library never spawns threads by surprise
`Parallelism::default()` is `Serial`; discovery is opt-in, asked for by the CLI and by the DataFusion provider's
`ScanBudget` where no allowance is stated. Rejected: `default_workers` read in the library, so silence means
concurrency.

### D2 A plain file recommends one worker; a local compressed one the machine's cores
A plain file recommends one: a serial plain `parse` is device-bound wherever a split reaches, a `COPY` block's interior
(an `INSERT` run, `KD9`, admits none), each piece's tail read adds device bytes, and `query`'s typed scan, which the
NVMe does not bind, is held flat by `KD57` (D25). A local compressed source or gzip first pass recommends its cores
capped at its units (`RT7`); a fetched one, one: more opens connections to a third party's server unasked. Rejected: a
plain split once a typed one outran one worker on the NVMe; a fetched first pass recommending cores since its tasks
fetch no extra byte, which answers bytes, not connections. Reopens: `KD57` closing (D7). Evidence: `scan-throughput-*`,
`parallel-scan-throughput`.

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

## The compressed source and the cache
### D14 Recognition sniffs content; `xz-seek` is a vendored read-only copy
Recognition compares a file's first bytes with each format's magic, never its name, outside the source trait, told them
by `Origin`'s one probe, which also answers D20's stored size, so nothing settled before a source exists assumes a local
file. Rejected: extension dispatch. `xz-seek` is a vendored read-only copy (`scripts/vendor_xz_seek.py`): a defect or a
missing seam is fixed upstream and re-synced, never patched or bridged here, as its sans-IO walk and block handle were
for a fetched source. Rejected: a path dependency, failing `cargo check` without the sibling checkout.

### D15 A read decodes whole blocks and retains them; reading one in pieces is the fallback
A read decodes whole units and retains them, LRU, where the caller's budget affords one reader's charge from file-wide
inputs (`BlockCache::affordable`). Rejected: a fixed refusal line; keying on block count. Below that line a read goes
through one live forward handle behind a mutex, kept across reads so a forward scan continues, advising one partition
(`KD35`); the fetched arm's window is charged, not booked unpooled. Both arms are written once over a `dyn Codec` and
their own `Positioned` read, never one body branching on the provider. Rejected: a source per codec and transport;
generic arms or an enum, holders being `dyn` (D6) and the codec chosen at open. Evidence: `reserve`.

### D18 The seek table is persisted and identity is three-state
A many-stream `.xz` file costs a footer read per stream, so a save persists the compression index and an open hands it
to the source, which walks nothing (`CompressedFileSource::with_index`); a local BGZF file's walk is persisted alike, as
its gzip index (D116). `KnownCompression` is three-state, not an `Option`, because a claim of plain is one the file's
magic can contradict; a contradiction is `Recognized::Mismatch` and condemns the span index too, both written by one
save of one file.

### D19 A budget decline is a `PlanNote`; a non-seekable file is warned, not refused
A budget decline is a `PlanNote`, a property of the file *and this run's budget*; a `Diagnostic` is a finding about the
file alone. A file with no seek structure opens and is warned (`NonSeekableCompressedSource`), only its user being able
to judge decoding a unit from its start: an `.xz` file of one block, a gzip file with a unit wider than twice the
spacing, which only a writer coding it in one dynamic block leaves (D118). A gzip first pass is a note, the pass making
the file seekable. A statistics skip is noted at zero wherever statistics were consulted, a narrowed span (D84) wherever
the plan charged less than stated. Rejected: omitting a zero; counting only believed filtered columns; warning gzip by
unit count, which says nothing of the writer. Reopens: a puzzling zero.

### D112 `gz-seek` inflates through zlib-rs, by `libz-rs-sys`'s zlib C API
Of the engines exporting what an index needs through the zlib API, zlib-rs is the fastest at both of `gz-seek`'s costs,
a first pass and a unit resumed from its checkpoint, and the only one built with no C toolchain; why its C API rather
than its safe one is UF4's. zlib-ng and stock zlib stay features, the figure's legs. Rejected: zlib-ng and stock zlib,
slower at both; the system's `libz.so.1`, a dependency a release does not promise (D104); ISA-L, its block stops only in
rapidgzip's fork, its resume priming undocumented state. Reopens: another engine ahead at either cost in a re-take, at a
bump of any engine's pin. Code: `crates/gz-seek/Cargo.toml`'s features, `src/zlib.rs`. Evidence: `gzseek-backend`; RT23.

### D113 gzip's checkpoint spacing is the block `xz -T0` writes by default
A shipped constant, never derived from the file. Across xz's block sizes a spacing moves only gzip's window section and
index size (`gzseek-spacing`), so it is chosen by what a unit shares with xz: a gzip unit is an ordinary `.xz` block to
D3's default-budget line and to the resident bounds read at that block, its inflater billed no term above xz's decoder.
Rejected: finer, more windows and index for nothing; coarser, past the block sizes the bounds were read at, toward the
one whose legs overran `MEMORY_RESERVE`; a power of two for its own sake, a unit always running past its multiple to a
boundary. Reopens: a resume's fixed cost not small at it; a gzip resident reading overrunning `MEMORY_UNPOOLED_BOUND`.
Code: `CHECKPOINT_SPACING`. Evidence: `gzseek-spacing`, `gzip-resident`, `reserve`.

### D114 A gzip cache's windows follow its encoded file, and every save writes them all
Each checkpoint's window follows the encoded `CacheFile` in the one cache file, so a load holds none and D20's and D21's
checks and D78's rename cover them with the map; every save rewrites the whole section through the provider the saved
index carries, its cost under D62's ratio. Rejected: windows inside the envelope, resident at open before any budget
exists; a file beside the cache, which no identity check covers, as D78 refused one for statistics; appending in place,
which leaves a killed save no whole previous cache. Reopens: a save's copy of the section, not the index's, setting a
save's duration. Code: `pgdt_dump::cache::file::windows`. Evidence:
`a_cache_saved_over_itself_keeps_its_windows_and_reads_its_file`.

### D115 A gzip first pass spills its windows to a file with no name, beside the cache or in the temp directory
Each window is written as it is placed to a file with no name (RT24), each save copying the section out of it: none is
resident in any cache mode, nothing outlives the run, and no name exists for D20's and D21's checks to cover. It is made
in the enabled cache's directory, else the OS temp directory, `open`'s caller naming it so `pgdt-io` names no cache.
Rejected: the windows in memory, unbilled, or billed by a `WorkerMemory` term that bounds nothing; re-basing onto each
saved cache, covering no disabled cache; the temp directory always, a tmpfs charged to the run's cgroup; a disabled
cache's would-be directory, beside a dump often on a read-only mount. Reopens: a spill directory named apart from the
cache's. Code: `CacheMode::spill_dir`. Evidence: `a_first_pass_spills_beside_its_cache_or_in_the_temp_directory`.

### D116 A forward pass coalesces BGZF members as the walk does, so a fetched BGZF file is read by a first pass
A fetched source recommends one reader (D2), so a member walk would download the object for parallelism the run does not
take, before the scan downloads it again; the first pass reads it once and coalesces members as the walk does, so a file
has one index whichever arm reads it, a broken chain included. Each member is decided by its own header alone, never by
a stated length, which a resumed pass cannot know. Rejected: the walk on both arms, two downloads; the coalesced run
ending for good at the first plain header; a BGZF mode the caller states or recognition sets, a broken file then indexed
per arm. Reopens: a fetched source recommending more than one reader (D2). Code: `gz-seek`'s `placement.rs`;
`open_remote`. Evidence: `a_remote_bgzf_file_is_read_once_by_a_first_pass_that_builds_the_walk_s_index`.

### D117 A parallel first pass speculates with a decoder of `gz-seek`'s own, and hands out only the engine's plaintext
No zlib-API engine decodes from unknown history, so a speculation decodes a chunk natively only to guess its bounds and
its ending history; the engine decodes the chunk from there, and the frontier takes a decode only standing at its start
with that history, so the index is the serial pass's. The crate's decoder decides who decodes a byte, never what it is:
in one member no chunk but the last meets a trailer, so native plaintext would go unchecked. Rejected: native plaintext
handed out, saving the second decode; the frontier waiting on its own chunk's speculation, slower than serial; per-task
fetches over a transport; the charge lowered before a resident reading attributes a worker's remainder (`KD141`).
Reopens: a reading where the second decode keeps the pass from lifting the inflate. Code: `gz_seek::Parallel`.

### D118 A checkpoint inside a static block is placed by walking its codes beside the engine
A writer ending no block across a spacing is checkpointed as any other, with no inflater of our own: the engine walks a
static block's codes on the native decoder's tables, writing nothing, and caps its own output at the symbol (D117).
Rejected: `Z_TREES`'s header stop for a block's type (RT23); a taken chunk's decode walking its static blocks, which
needs marks only the frontier holds; holding a unit ending at a symbol to its compressed end, a second walk on every
read to refuse a unit sooner than the next unit's CRC-32 does; a bound on the bytes consumed, catching only a
misplacement past the lookahead. Reopens: a consumer acting on `IndexMismatch` apart from `UnitCheck`; the walk visible
in a first pass's profile. Code: `native/walk.rs`. Evidence: `a_checkpoint_moved_inside_a_static_block_is_refused`.

### D20 The library never replaces cache data automatically
A cache not ours, damaged, another build's or another file's (stored size, or a compression layer the source
contradicts) is almost always a wrong path or a changed file, so every open refuses it (`CacheMode::refusal`) before the
dump is read past its magic. `--overwrite-unusable-cache` (`with_overwrite_unusable`) starts cold over one recognisably
ours, for a file replaced under a stable name; foreign bytes never. Rejected: a cold start (a silent re-scan after a
typo or an upgrade); the guard in `CacheMode::save` (policy an embedder cannot override); keeping a damaged one of ours
from the flag (nothing readable lost, an unbumped version its likely cause); `load` checking size alone (a claimless
open gets another file's map).

### D21 Identity is `stored_size()` plus weak signals, in an opaque enum
`stored_size()` keeps the staleness check a `stat` where `extent()` needs a decompressing source opened first.
`SourceIdentity` is opaque, read through signal accessors, so two kinds compare rather than refuse. Between runs a weak
signal — modification, origin (D87) — is advisory, warned and never persisted, binding only under its own
`StrictIdentity` term, absence included. During a run a change is an error, checked where asking is cheap: locally at
the save's cadence (D62), at run end and before a failure is reported (the failure is often the change); remotely by a
precondition on every ranged GET. A source nothing can pin is refused unless `NONE`. Rejected: a check per chunk (rows
still reach a caller first); an unpinned run merely warned of.

### D87 A remote cache is named from the URL, and the origin it records is advisory
Default `./<last URL segment>.dtcache`; no last segment is refused by name. Another origin is a diagnostic, the stored
size still refusing (D20), so two same-named dumps of equal size from different hosts in one directory read each other's
map with a warning the origin makes legible. A local cache records none, its default sitting beside the dump. Rejected:
a canonical path as a local origin, advisory after every move.

### D90 The DataFusion provider reads only a complete cache, never maps
`PgDump::open` believes only a cache reaching the file's end whose identity checks pass, and refuses anything short
naming the mapping pass that builds it; `pgdt sql` never parses, a parse wanting `pgdt`'s discovery, interrupt guard,
status lines and cache rules. So every block a table owns is seen (`KD6` is the cold query's), a schema is stated from
every block's census, no database's DDL is unread, the partitioned replay is always available, and there is no partial
map for a dropped stream to keep (D26). Rejected: falling back to a cold query's early-stopping map, which brings each
of those back. Code: `datafusion_pgdump::PgDump::open`.

### D22 `Held` is its own type and `CACHE_FORMAT_VERSION` is bumped freely
`Held` is its own type: an incomplete cache is usable, `Disabled` is about the caller and `Replacing` carries its reason
to every caller. `CACHE_FORMAT_VERSION` bumps on any persisted reshape, a parse fix changing what a span or the metadata
holds included, and the bump is recorded nowhere: an older cache is refused until replaced by the flag or deleted (D20),
which is what a user hears of first. Rejected: an unbumped parse fix, its cache answering as the old parse did. Code:
`pgdt_model::CACHE_FORMAT_VERSION`.

## The scanner
### D23 The scanner never owns the bytes it scans, and only the whole `COPY` grammar is structural
`CopyScanner` is a synchronous state machine over a caller-owned buffer, so a chunk is scanned where it lies, only the
line straddling its edge copied (rejected: a growing buffer, copying every byte twice). `max_line_bytes` is its one
memory bound, and past it a line errors, never truncates (`KD93`). Only a line matching the whole `COPY … FROM stdin;`
grammar, or a bare `BEGIN;` (D33), is structural; inside a block only an exact `\.` line and a row's ending are read
(I7, I91). Outside one every line is lexed as psql lexes it (I50), and a line beginning inside a string, identifier,
comment or dollar-quoted body is never structure (I1). Rejected: tracking `$` alone, or quotes too (a `$$` in a name or
comment hid every block).

### D25 Parallelize what is CPU-bound
One core's rate against the device's offer decides each kind of work (decode, extraction, discovery) per source; D2
holds the answers, D7 who asks, D52 the worker. Readings that disagree reopen it. Evidence: `xz-decode-scaling`,
`parallel-scan-throughput`, `scan-throughput-*`.

### D26 Cancellation is `map_file`'s argument, polled per chunk or leader window by the mapping passes alone
A block can be hundreds of gigabytes, so block-boundary cancellation is a hang. A mapping pass (`map_file`,
`Table::settle`) takes a `tokio_util` `CancellationToken` as a required argument, polls it (D63) and keeps what it
banked; a query's streams take none, a dropped stream being their cancellation (D90). The preamble scan polls none: a
stop there would read as the first `COPY` header and cache as complete. `parse` dies by the signal it caught, so a shell
script stops (RT20), and a second signal ends it in the handler; as its namespace's init `pgdt` exits 128+n instead
(RT19). Rejected: a `ScanOptions` field (options are reused; a fired one stops every later run); a token type of our
own; `Option<&CancellationToken>` or a `Stop` trait inside the passes; `exit(128+n)` outside an init.

### D27 UTF-8 is validated once per chunk; `unsafe` is denied everywhere and allowed at the site that needs it
One SIMD validation of the largest line-terminated prefix per chunk, not one `from_utf8` per field, skipped where
nothing is decoded; the escaped path still validates (`\xNN` synthesizes bytes). In D68's seven crates the one site is
`append_view_unchecked` (`batch/chunks.rs`), its bounds from the `contains` that produced its coordinates and its
validity from the decode's borrowed arm (D46); `gz-seek`'s zlib module is the other shipped site (UF4). The workspace
denies `unsafe` in every target and each root forbids it unless a site of its production tree allows it
(`scripts/layering.py`). Four `unsafe` attempts lost to the safe shape.

### D28 One row split, shared unconditionally
One `RowSplit` per row, memoized and shared unconditionally by the filter's terms and the batcher, extending only as
deep as asked. Rejected: a term-count gate; a second `push_row` for the unfiltered path (a second `push_field` site
defeats inlining); stopping the walk at the last projected column, `push_row` being the only `ColumnCountMismatch` site.
Evidence: `predicate-terms`.

### D29 Refused micro-optimizations on the row path
Pre-sized Arrow builders (a one-row batch fills thousands of slots); a viewing builder for nested `Utf8View` (the prize
is zero below arrow's inlining width); skipping a block with `memmem` (the row count and the census read every row); a
proposal is sized against the per-row profile. Evidence: `nested-decode-micro`, `map-only`, `cross-file-floor`,
`nested-end-to-end`.

## The file map and the preamble
### D30 A span is classified by the grammar; the TOC anchors and enriches, never required
A span is classified by the statement grammar and the TOC comment is enrichment: TOC-driven segmentation is unsound on
non-`pg_dump` input and a dollar-quoted body can hold a TOC-shaped line (I3), so a TOC block anchors a span where one
exists and the grammar closes every statement without one. A tiling hole is `TilingBroken`, never a refusal. The
preamble prepass (byte 0 to the first `COPY` header, I1) runs ahead of either mapping entry point's pass unless the
cache holds it, so a scan stopped anywhere, its first chunk included, still states the first database's DDL. Span text
is sliced from the file after building, never accumulated, and `Data` spans store none. Evidence: `preamble-prepass`.

### D31 `Span::toc` is "belongs to", vetoed after classification; boundary rules that read as bugs
Follow-ons inherit `governing_toc` (else coverage reads half on healthy input) and `toc_owned` counts objects; `Framing`
vetoes after the transition that seeded the inheritance, and `Connect` is never seeded. The boundary predicate refuses
`"Data for "` and accepts `"Statistics for "`, which matters under `--disable-triggers` (I31). A blank line does not
close a pending comment (`_printTocEntry` writes `--\n\n`); `scan_preamble` retreats to `pending_comment_start()` rather
than guessing the comment's kind.

### D33 Bulk regions are one span kind; large objects skip at the scanner, `INSERT` runs at the map
Only `DataBlock::Copy` has inner offsets, only `COPY` having a row reader; a data span absorbs the TOC comment pending
at its start (`KD1` where none is). A large-object region, a bare `BEGIN;`/`COMMIT;` pair, is skipped unread at the
scanner (I12). `INSERT` runs fold in `feed_line` with no scanner state, their boundaries resting on no line-anchored
invariant, the run's end lexed as the scanner lexes; the cuts `KD9` names wait for P8's row reader.

### D34 `DumpIndex` stores no fact twice but sortedness, `metadata` and `scanned_through`; whole-file facts need `is_complete`
Every fact has one owner: `blocks()` and `metadata` are views over `spans` (`metadata` computed once) and diagnostics
are never persisted. Whole-file facts (roles, tablespaces, a reported schema) are believed only once `is_complete`; a
streamed schema commits over the blocks it replays, ungated (I2). Statistics sit in their block behind an `Arc`, so a
map clone copies a reference (`KD5`), and a row group is a byte range, so no piece needs a global row index. A back-fill
keeps a block's columns, and its size unless a stated size or bound it did not record moves it, a maximum it breaks
re-reading it at the size its groups predict. Rejected: `SparseRowIndex`; padded `character` bounds and entries (keyed
alike, but past the cap); a back-fill dropping only re-read blocks' columns.

### D35 The census is type-blind, records both dimension bounds, and runs at the data level
`ArrayShape::observe` reads the leading brace run off still-escaped bytes in the model (I15, I25); min and max depth, or
`{1,2}` beside `{{1,2}}` resolves to a wrong `List`. A table any column of which is at the data level is censused, and
so is every block a query's pass maps: gating on a full scan left early blocks uncensused. The metadata level records
`None` and, but under a strict parse, splits no field; a typed query re-reads such a table for itself, writing nothing,
and a `Dump`'s plan refuses it. Rejected: degrading to the DDL's `List`; a census-only level a user can reach; the
census as a `Table`'s own step, which a `Dump`'s would offer. Evidence: `statistics-gathering`, profiled.

### D96 The unrepresentable count rides the census, typed by the DDL, in two tiers, under a recorded calendar
Every census-taking read counts, per block and column, the values the declared type's typed pair (no census) cannot
hold: each leaf by its own type, lexically, only boundary years and an interval's long hour part by arithmetic; a nested
value once, in its worst leaf's tier — `format` past Arrow's spec, `engine` past `calendar_end` (RT21). Counts persist
(D74); a cache counted under another calendar is refused (D20). Rejected: a decode per field; a type-blind test (`text`
reading `infinity`); counting as a query reads (timing decides); one tier; counting a value the server refuses.
Evidence: `every_extreme_is_held_by_arrow_or_recorded`.

### D36 The preamble grammar dispatches on fixed keywords and never guesses
Unrecognized lines are ignored, so `--binary-upgrade` noise is free (I5, I6). `record_type` keys on name (I11); a
composite's field list is all-or-nothing, `record_out` being positional (I23); a `--create` dump's pre-`\connect`
segment is not a database (I9). The model keeps the dump's text (D74): a declared type as its words; a collation clause
verbatim, `None` meaning no clause (I37); a name in one spelling, both sides of a lookup compared in it (I29);
`INHERITS` parents, `OF` type and a partition's bound as text (I86). Of what the DDL implies it keeps only a column's
`NOT NULL`, an enum's label completeness and a collation's `deterministic` (I42). Rejected: a name's parts dequoted
(`"a.b".c` is `a."b.c"`); references flattened in at the fold, which a later `ADD COLUMN` misses.

## Type resolution and decoders
### D37 The bar: the dump alone determines the value
A declared type maps to a real Arrow type only if its text round-trips consulting nothing outside the file; otherwise
`Utf8View` with a note naming the kind of unknown. Misreading is unrecoverable, not recognizing is not; `money` fails it
(`KD13`). Every column is nullable regardless of DDL, a `NOT NULL` refusing a NULL instead, which `ignore` reads (I76);
a range's three flags are not. Rejected: a bare built-in name `Unknown` where a declared type could shadow it —
`pg_dump` writes built-ins bare under an emptied path (I8), so it would demote its own columns (`KD48`).

### D38 The ADBC driver's shipped release is a floor, swept from the catalog
Where the ADBC driver yields a real Arrow type, ours is never wider. The floor is one pinned release, never `main`,
swept per declarable `pg_catalog` type at every major, and `floor_mapping.py` fails drift in either direction; the
driver's refusals and `arrow.opaque` answers fall outside it by the file's own columns, and a row it reaches that we do
not meet carries a `Disposition`. It binds the typed mode alone: the untyped mode, like `--schema-mode strings`, is the
user asking for a wider type, and widens only a column the map says its front end cannot hold (D100). Rejected: the
`typelem` shape test, which deletes `int2vector` (I47).

### D40 The comparison is decided per declared type, in the same arm
`builtin_scalar` answers Arrow type and `CompareKind` together, per declared type and `COLLATE`: types sharing
`Utf8View` compare differently, so nothing orders by the Arrow type. DataFusion semantics is one per query, lest rows
depend on what a plan pushes; it maps each kind (`datafusion_order`), every text-emitted kind to `Text` — `macaddr` too,
though its file text orders as its octets, since a user's literal is compared bytewise — and a divergence is the
column's, never a term's (D59). Rejected: a nested column ordered as DataFusion does (`make_comparator` under default
options, marked `TODO` upstream in `compare_op_for_nested`); for `macaddr`, a per-literal switch bought for range
pruning (refusing a literal not in lowercase, or keeping the octet key for one that is).

### D43 `NestedPlan` travels beside the `DataType`; the census speaks after the DDL and moves the pair
An Arrow type does not name its literal (`int4range[]`, `int4multirange`: both `List<Struct>`), so a `NestedPlan`
travels beside every `DataType`, every builder and renderer takes it, and the pair changes only whole. The census speaks
after the DDL, never instead of it, and is a parameter of resolution so no caller skips it. `MetadataNotScanned` is held
apart from `NotDeclared`: same output, opposite advice. Rejected: widening `builtin_scalar`'s tuple; `Field` metadata.

### D45 Strict decoders apart from permissive parsers; decode and render allocate only what they return
`decode_*` reads what a dump holds and cannot loosen, each the inverse of its `render_*`; `parse_*` reads what a user
typed and cannot tighten: two scanners, not one with a flag, each transcribing the newest major and under-accepting
(I35, I44). Wrong depth, `[lb:ub]=` or a field-count mismatch is refused, never reshaped; a value no text form spells is
`FieldRender`, never rounded (I40). A renderer writes into its caller's buffer and a decoder allocates only what it
returns, a nested element borrowing unless it carried an escape. Rejected: a `String` per rendered field
(`tests/render_allocations.rs` pins the count). Evidence: `nested-decode-micro`.

## Batches, streams and the leader
### D46 Zero-copy views are top-level `Utf8View` only, and `max_source_span` alone bounds pinned bytes
Every arm but a top-level `Utf8View` copies, and a batch's views pin the chunks they point into until it flushes, so
`max_source_span` is the one bound on pinned bytes: `max_rows` and `max_bytes` count selected rows, which a filter makes
sparse. A source retaining by partition is charged no span (`RetainedUnit`), unsound where a query's cut spans several
units (`KD23`). Rejected: compacting views past a selectivity threshold. Evidence: `parallel-peak-rss`.

### D84 The batch span is derived from the budget and the count, and spent before the count is cut
`max_source_span` is a ceiling: a partitioned replay derives the span it charges from the budget and the reader count,
narrowing the span before it cuts the count, never below the announced read chunk (`ReadOptions::chunk_size_bytes`, the
unit a batch pins), and its sub-streams read under the span it was solved for. Rejected: a third flag (`roadmap.md`,
"Two tunables fit pgdt to hardware"); pricing a plain reader (`KD25`); a floor above the read chunk, a performance claim
with no batch-size figure behind it; the shipped chunk as the floor, which under a smaller `--chunk-size` declines
readers the announced one seats. Reopens: a floored span measurably slower than an unfloored one. Evidence:
`parallel-scan-throughput`.

### D48 Mapping and replay are separate passes, and `splice` owns the seam
A mapping pass settles a table's blocks and yields no row; replay reads them again, so the map is never behind the rows
and a `ResumeToken` points inside mapped territory. A segment is spliced by extending the *preceding* span, so a blank
line stays with the span before it across a stop. A cold query's pass (`Table::settle`) takes the extent and schema mode
it reads under as arguments. Rejected: a start floor on `Builder`, which made assembly visible in the map; the pass
taking the `QueryOptions` it is planned under, its `Table` planned under others anyway (Strings then Typed refuses,
D35); a value for those two alone.

### D49 One target per query, the early stop conservative, and `ResumeToken` opaque, fingerprinting the query
A `TableSelector` narrows to one `TableName` before replay or `AmbiguousTable`; `target_settled` vetoes a stop on a
partition root (I2) or any `\connect`. A conflict past the stop is unseen (`KD6`). The token's filter is hashed by
explicit match, as a derived `Hash` silently misses a new operator; the batching knobs, `postgres_invalid_values` and
`use_statistics` stay out, no skipped group holding a resumed row.

### D51 A segment's offsets are search bounds, and a resync is a real read
Sub-streams are contiguous byte-balanced runs cut through blocks in file order, so concatenating them *is* the serial
replay. A cut's offsets are search bounds, a row straddling one belonging to the earlier piece, and a piece resyncs by a
real read to a line start, never handing the scanner a mid-row byte, since a value can end in `\.` (I7 covers line
starts). Rejected: cutting at block boundaries, sparing a declared ordering its proof across them
(`summary::partition_orders`) at every table's balance.

### D52 The worker is fused, and the earliest failing piece is the error
A piece reads its range and parses each read in `spawn_blocking`; `scan_piece` does no I/O. Rejected: decode and parse
pools over a channel (the ratio is a property of the command); speculative splitting ahead of the leader; cross-block
pipelining. Pieces drain in file order so the same truncated file names the same byte. A failure only reading finds is
the lowest-indexed failed sub-stream's, after the rows before it; a resolution refusal comes from the plan (D54). Across
a DataFusion query's partitions the first refusal wins, each true; rejected: ordering them.

### D93 A dynamic filter's rows are evaluated only under `pgdump.dynamic_filter_rows`, off by default; its state is read at each group entered and each chunk
Off still prunes groups, cuts the sub-streams and stops a sorted block; the setting states intent, which "Two tunables"
does not refuse, and is the one way to the unclustered join's win. On, rows are evaluated in every block, less the
bounds an `IN` beside them implies outside any `Not`. The state is read at each group entered and each chunk. Rejected:
on by default (`dynamic-filter-join`'s costing row); DataFusion's flag as the switch (pruning goes with it); evaluating
only where statistics delimit groups (`KD33`'s tail); reading per row, per batch or at group entry alone; ceasing where
nothing is rejected (an explicit request, D77); dropping a static filter's implied bounds, which raise. Reopens: that
row's rows leg within its on leg's spreads (`KD55`); the per-chunk check priced.

### D95 A dynamic filter enters by `TablePartitions::under` alone, and the byte cut is made once per handle, at the first poll
A replay may drop any row a state it read rejects, so the filter is no `QueryOptions` field: the one entry point that
never resumes takes it (D77's reasoning). Planning stays at `scan()`; the first sub-stream polled cuts them all over the
groups the static filter and the state then keep, so D51's balance holds over what is read, at the plan's count; the
planned cut stands where nothing is ruled out or an order would be lost (`KD53`). Rejected: the cut held by
`TablePartitions`, outliving the filter a reset discards; the count re-planned (read at planning); a partition reading
its filter through the plan node, whose clones a reset node shares; seeking the scanner in place at a skip. Code:
`stream::DynamicPartitions`.

### D98 The typed mode nulls what its front end cannot hold, tested before decode where the block's count says it holds one
Under `UnrepresentableMode::Null` a value past the tiers the query's semantics cannot hold — the format spec's under
PostgreSQL's, the calendar's too under DataFusion's — is NULL to the batch, every filter leaf and statistics (D97); a
nested value holding one is NULL whole. It is tested lexically (D96) before decoding, only in a column whose block
counts one or carries no count. Rejected: nulling what fails to decode (an engine-tier value decodes; unparsable text
must still refuse); a tier option beside the semantics, two fields for one fact; testing every block. Reopens: another
front end, with semantics of its own; `arrow-cast` displaying past `chrono` (`upstream.md`, "UF2"). Code:
`UnrepresentableRead`.

### D99 The refuse mode refuses at planning, per materialized column, from the map's count over every block
Under `UnrepresentableMode::Refuse` a plan refuses the first column it materializes — projects, or hands DataFusion —
whose blocks count a value in the tiers its semantics cannot hold (D96, D98), before a row is read: over every block,
not the groups a filter keeps, which survive by statistics, and statistics never change an answer. A column only a
filter the library answers reads is not materialized, and compares every value in PostgreSQL's order (D56), so no read
refuses one. Rejected: refusing where a read reaches one (timing decides); over the kept groups; a read-time refusal
beside it, the count being exact (D96). Code: `stream::replay::plan::materialized_unrepresentable`. Evidence:
`a_selected_special_value_still_cannot_be_materialized`.

### D100 The untyped mode widens a column per table from the map's count, and compares it in each semantics
Under `UnrepresentableMode::Text` a column whose blocks count, over every one, a value in the tiers its semantics cannot
hold (D98) is `Utf8View`, settled once per table as the census is (D43). It compares in its widening's semantics alone:
in PostgreSQL's by its declared plan (D56), so `pgdt --where` keeps its order; in DataFusion's bytewise, so a pushed
filter stays `Exact` (D88); a key asked in the other is `None`. Rejected: text everywhere (an unmapped column's
refusals); per block (two blocks of one table typed apart); the declared plan in both semantics. Code:
`resolve::read_as_text`, `TableColumns`. Evidence: `the_untyped_mode_compares_its_text_column_in_each_semantics_order`.

### D103 The value modes are one option each for every column, fixed where a dump is opened, apart from the schema mode
`QueryOptions::unrepresentable` and `::postgres_invalid_values` are each one option for every column, on every surface
(a `pgdt query` flag, `PgDumpOptions`, a `pgdump.*` table option, a shell suffix), fixed where a dump is opened: the
text mode changes a schema at `PgDumpTable::build`. The invalid-values mode is a parse's too. An ignoring parse records
per column what it went past and keeps no statistic a read could contradict, so a `Default` parse tracking such a column
fails over the cache; `Strict`, a parse's alone, re-reads each block not marked checked: a verdict is the dump's, not
the gathering run's. Rejected: a record per block; a `SET`-able setting; a third `SchemaMode`; recording the mode;
refusing such a cache (D20); `Strict` trusting the record. Defaults: `Null` (D38); `Default`, as a restore.

## Predicates
### D53 The operator set is closed but for membership and the unrepresentable test
No `LIKE` (collation-dependent folding), `BETWEEN` (`And`), or column-to-column; `IS [NOT] DISTINCT FROM` is what
three-valued logic forces, `IS [NOT] UNREPRESENTABLE` what the null mode forces (D101). `IN` and `NOT IN` are
`PredicateOp`s holding their list, `NULL` included: one column against literals, as PostgreSQL parses `= ANY` and `<>
ALL` over consts (`transformAExprIn`), answering as the `Or` of `=` (or its negation) but decoding and looking up once a
row, where that `Or` costs per term (`dynamic-filter-join`). Rejected: `Expr::In`, a second kind of leaf for an embedder
to build; recognizing the `Or`, every front end emitting `In`. Evidence: `tests/membership.rs`.

### D54 One tree, no planner yet, short-circuit defined against the root
`filter` is one n-ary `Expr`, by default the empty conjunction. `And` may stop at the first `Unknown` except beneath
`Not`, only the root's `True` mattering. A decode failure surfaces only where evaluation reaches it — never in a group
statistics rule out, nor serially past a sorted block's stop — so which rows raise is outside the contract, `--jobs`
deciding. A resolution refusal is the plan's, before any row: the first refusing block's in file order, every leaf
walked, a misnamed column before anything a term asks. Rejected: DNF; exact Kleene everywhere; not skipping a group
holding an unkeyed value (`KD2`); the stop asked first (every valid dump paying for invalid text); a mode refused before
a typo. Reopens: a planner or rewrite, where a reading shows it pays. Evidence: `tests/pruning.rs`.

### D55 A literal is read in the type's `*_out` form at least and its `*_in` grammar at most
Floor and ceiling are `roadmap.md`'s, "A literal is guaranteed in `*_out`'s form and never read past `*_in`'s". A reader
keeps its narrow grammar, a nested literal's leaves too, and a port of `*_in`'s classifies only what it fails
(`decode::Unread`): a field fails a parse only at a `pg-refuses` check. A reader past the ceiling is replaced by its
port (`network_in`). `JSONB_MAX_DEPTH` is fixed (a stack overflow aborts). Zone names are open (tzdata grows unbidden),
abbreviations those PostgreSQL ships (I83). Rejected: failing every field read as no value; widening a canonicalized
kind's reader (`=` misses another spelling, `KD82`; `macaddr`'s one bounds set, D79, splits); for a narrowed `*_in`, the
dump's own major's; a server's own abbreviation file (forgoes I61's era edge); the tzdata at hand.

### D56 Special values are a rank in the key; equality has three canonicalizations, by injectivity of `*_out`
`infinity`/`NaN` are a rank in PostgreSQL's order, compared before the value; `OrderKey` derives no `Ord`. Rejected:
excluding the row like a NULL (I33, I34). `=` canonicalizes by whether `*_out` is injective over one file (`Equality`):
`v=1.5` hits `1.50` in a `numeric(10,2)`; `timetz`, `inet` are not rendered (I33, I38, I41); a `bytea` literal renders
in both `bytea_output` forms (I56), not decoded per row. A `ValueKey` carries the order it was read in, two relating
only as the semantics' engine relates them: the server's by type name (I92), DataFusion's by its coercion, in
`datafusion-pgdump`, its oracle (`key_order.rs`). Rejected: a variant per order; a declared-type tag on a scalar order;
cross-plan answers unspecified; arrow-cast in the codec (D74); the caller passing each key's Arrow type, unchecked.

### D59 Divergence is per position on its own channel: per term, and per column at registration
`ComparisonNote` carries a path and its declared type, per term from `comparison_notes` (operator-conditional, so
query's) and per column from `Table::divergences` in a query's semantics — DataFusion's variants there alone, as
DataFusion's `ORDER BY` reaches a column no term names. See I45, `KD7`, `KD10`. A provider registration folds its
`UnknownCollation` columns into one `UncollatedColumns` finding, every column of a collatable type with no `COLLATE`
earning one. Rejected: the front end folding them, re-reading the provider's sentences to recover a table and a reason.

### D60 `--where` is a second flag and the tokenizer defines the refusal set
`refuse_where_structure` refuses a `--filter` term tokenizing to anything but one leaf, an empty one `parse_filter`'s,
so a string both flags accept means one thing. A keyword needs whitespace, a paren or the string's end on both sides and
`NOT` after `is` stays in the term. A term splits at the earliest operator outside quotes, longest first; quotes are
stripped in a term, under either flag, and in no name flag. Rejected: `&&`/`||`; backslash escaping.

### D88 A pushed filter is `Exact` where the plan resolves it; a literal is the renderer's text
`supports_filters_pushdown` asks `Table::schema` to resolve the translated term in DataFusion semantics, the scan's own
refusals. A typed literal must be the column's Arrow type and is written by `render_field`, the decoders' inverse; a
string one stands only where the library compares text. Rejected: a formatter per type in the provider, a second grammar
to drift; pushing `IN` on a float, answered from a set with no `-0` made `0`; `Exact` where the library agrees with
PostgreSQL, which answers an enum `<` in declaration order pushed and in label order kept; `Inexact`, promising a
superset another semantics can break. Evidence: `crates/datafusion-pgdump/tests/pushdown.rs`.

### D101 The unrepresentable test reads the text against the declared type; DataFusion's is a scan's alone
`IS [NOT] UNREPRESENTABLE` tests a field's text against the declared type in the tiers its semantics reads (D98), under
every mode; `SchemaMode::Strings` refuses it. `pgdump_unrepresentable(<column>)` is pushed `Exact` and refuses wherever
DataFusion would evaluate it; `ready_session`'s appended optimizer rule refuses that at planning (RT22). Rejected: a
table function or companion column (no row identity; 55.1 has no hidden columns); evaluating it (a NULL keeps no
origin); registration installing the guard (a rebuild, dropping prepared statements); a wrapped planner (one slot,
trusting an `Exact` conjunct gone); the function opt-in (withholds the always-sound pushed use). Code:
`unrepresentable_tests`, `ready_session`.

### D94 A dynamic filter is translated loosened into the library's tree, never read through `PruningPredicate`
Every producer re-checks its rows, so the scan answers `No` and only ever skips; a state is translated as a static
filter is (D88), loosened, so none refuses. A float compared with a zero has no term: a TopK and `MIN`/`MAX` order the
zeros apart where evaluation equates them. Held filters drop at `reset_state`. Rejected: `PruningPredicate` (bounds
decoded to scalars, pruned in DataFusion's semantics, nothing on a `CASE`, a long `IN` or a sorted block); a narrower
zero rule, sound only as far as each producer's shape is followed; keeping filters across a reset as Parquet does (a
recursive query's next iteration needs rows the last ruled out). Reopens: a release whose producers hold with the zeros
equal, then an `RT<n>`. Code: `filter::dynamic::loosened`.

## Statistics
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
DataFusion's (bytewise), and DataFusion's beside an exact one it differs from; `macaddr`'s serves both (I40), a float's
ties its zeros as `total_cmp` does. A dictionary needs exact equality alone, reaching `KD7`'s text; nested columns get
neither. A set is named by the kind it was gathered under, recomputed from the recorded DDL. Rejected: gathering a
distinct count (D89 derives one); one slot; a second `macaddr` set, or a float's for its zeros; the zeros told apart in
the key, which every term equates; a kind or DDL flag stored per set; gathering keyed by plan and semantics, which map
onto no set uniformly. Code: `bounds_kinds`, `bounds_set_keyed_by`, `compare_keys_stored`.

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
piece at its join; the nearest-rank median; the cap yielding to a stated maximum block by block, which the leader's
split would decide; a floor under the coarsening, a constant nothing prices. Code: `Gatherer::fit_cap`, `density.rs`.

### D85 Statistics are billed against the margin, and a block that cannot fit declines
`statistics_allowance` is what the arrangement leaves under `margin_allowance`, carved after the workers, whose count is
fixed before a byte is read; statistics a cache hands a pass bill the carve as `held`, `pgdt query` carving its replay
after the pass around the table's. The CLI's is `--memory`, else the limit, else half of `MemAvailable` (RT8); none
stated declines nothing (D1). A piece's decline takes its block, dense group indices expressing no gap, and the account
sees a window's pieces, so serial and parallel differ, as meant. Rejected: a constant bound, an OOM on a wide table;
coarsening to fit, a cache depending on its container; retrying every run. Reopens: the check's cost, or declining's
saving. Code: `Gatherer::decline`.

### D89 Statistics reach DataFusion on a leaf plan node, and `Exact` means the bound is the value
`TableProvider::statistics` is read by nothing in DataFusion 55 and `StreamingTableExec` answers unknown, so
`PgDumpExec` holds one rather than parenting it, a parent's child being replaceable under it. `Exact` only where the
value is the table's: a bound every group of every block contributed, stored whole; a distinct count where every group
kept a dictionary of emitted text (I48). Rejected: `Absent` for a partial bound, giving up cardinality; the union as
`Inexact`, a floor inflating a join's estimate; under a filter, a guessed selectivity. Code: `datafusion-pgdump`'s
`statistics.rs`, `exec.rs`; `pgdt-query`'s `summary.rs`. Evidence: `tests/statistics.rs`.

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

## The CLI
### D61 `info` never scans
The surprise is that a scan happened at all; `parse` scans ahead, a `query` maps only what the cache lacks (D48), and
`--preamble-only` hangs off `parse`. It reports the cache through `CacheMode::open`, or `OfflineCache` with no source,
never a `pgdt_query::Dump`, which holds no partial map, container or envelope. Rejected: a scanning fallback;
`--no-scan`; a `Dump` where the cache is complete, a second path for one cache.

### D62 The save throttle is a ratio, and its gate opens splice too
A save is skipped unless `K` times the last save's duration has elapsed since it: a ratio bounds overhead in every
regime, with no interval to tune. The map is rebuilt only where the gate opens, each rebuild being `KD5`'s cost, the
gate opening too on a block that could settle a queried table. Rejected: every N seconds or bytes; a disabled-cache
floor. Evidence: `per-block-quadratic`.

### D63 The interrupt flag is read at both extremes; resume is the default
Per chunk and at every completed block, since neither alone bounds the response. A resumed scan is span-identical to a
straight-through one, which is what makes an interrupted `parse` saved work. Rejected: `select!` over `ctrl_c` (drops
the map); a `--restart` flag.

### D64 An omitted `--jobs` asks the source; provenance is the CLI's fact
Absence recommends (the source's `default_workers`), a stated count is never lowered, zero is refused; whether a number
was typed or discovered rides the `Arrangement`'s `Allowance`, never `Parallelism`, which every pool reads. The flags
and the limit are announced before the open, the arrangement after it, since a fresh `.xz` walks footers first; the
limit is read once, so the limit announced is the one carved. Status goes to one stderr subscriber, no terminal
detection; its durations are no figures. An index build is announced by its source: at `info` where its cost is the
scan's own (a local walk; a gzip first pass, saying why the scan is one reader), at `warn` before a cost beyond it (a
fetched `.xz` walk). Rejected: the first pass's reason in the arrangement's lines (codec knowledge in the arrangement).

### D66 Output of what `pg_dump` writes is byte-identical whether typing is on or off, but for a value its type cannot hold or an `escape` `bytea`
Every value renders back to its `*_out` text, which is what `pg_dump` wrote — a hand-written spelling `*_out` never
writes rendering as `*_out` would — but one its type cannot hold, NULL in the null mode (D98), and an `escape` `bytea`,
`hex` (I56), which `byteain` reads alike. One contradicting its type is `FieldDecode` naming the strings schema mode,
never a null. Rejected: Arrow's display; a `bytea`'s form kept per database or flagged, read by no comparison.

### D67 `--json` is the internal struct; a flag's help is its doc comment
The whole cache file, keyed by block, no version of its own, compact and streamed, group values in: a script sums its
own rollup, a table spanning blocks (I2) with no merge rule but statistics' sums, `--detail`'s alone. Coverage is one
line at the top, nothing below qualified. No `help` attributes but `sql`'s, upstream's style; pages are snapshotted with
width and bare-flag assertions. `sql --dump`'s value name spells its suffixes past the page, its spec line exempt from
the width assertion, until one pass over CLI output (`roadmap.md`, "Future — wanted, unscheduled"). Rejected:
`long_help` per flag; a rollup; that value name cut to a placeholder piecemeal.

## Layering
### D68 Seven crates, the module assignment Cargo enforces
Seven library crates — core, model, io, dump, codec, mapping, query — each using only those before it, dump and codec
siblings; front ends and embedders above query. Each module's crate and its reason are `concepts.md`'s;
`scripts/layering.py` holds every member to the graph. Model is apart from core so an edit there is the one asking
whether the cache format bumps, and below the parsers so the codec does not recompile with the grammar. Rejected: four
layers in one crate with upward deviations; the cache a crate of its own (its loader re-derives the map's diagnostics,
so it is dump's); the mapping pass query's (it reads nothing of query); the graph held to the libraries alone (a new
member unplaced); codec and query, or dump and query, one crate (D74's boundaries).

### D74 Below the codec no Arrow, the codec pure, and the core's error naming no dependency
Core, model, io and dump name no Arrow; codec and mapping only its schema types, the codec also `i256`; model and codec
do no I/O, the codec synchronous. A decoder takes an unescaped field and returns a value, never a builder: arrays are
query's, whose views tie building to the read buffers (D46). Anything persisted is model vocabulary — declared type
strings, never a `DataType` or another codec conclusion; a contract over a block's rows is model's. The core error
carries a dependency's as an opaque `Cause`, and the provider's refusals reach DataFusion as
`External(datafusion_pgdump::Error)` over it. Rejected: a `#[from]` variant per dependency (each compiled everywhere);
the core error crossing bare; `OrderKey::Decimal` as digit strings or any integer but `Decimal256`'s.

### D106 A crate's API is a designed surface, and what a boundary forces public that is not API is `internal`'s
A library's API is its root and the namespaces `layering.py`'s `NAMESPACES` lists: what an embedder would call given
what the crate's `lib.rs` says it is for. What a crate above needs that is not API is `internal`'s (a method on an
`internal` `<Type>Ext` trait), named by no embedder; an item defined low that means something only above enters that
crate's API by re-export. No facade: nothing is published (D104) and a crate is reached directly. The seven are
`pgdt-<name>`; the DataFusion crates keep `datafusion-*`, the prefix that ecosystem finds a provider by. Rejected: what
is `pub` or named by an embedder today as the test; a facade (every name spelled twice); a published `pgtypes` crate out
of the codec; `introspect` or the DataFusion adapter exempt from the embedder rule. Evidence: `tests/embedder.rs`.

### D107 A library refusal names its remedy in the library's terms, and each embedder spells it after the sentence
A library error states the condition and ends with its remedies as the API names them — a mode, a mapping pass, a
selector naming a database — never a front end's flag or command, nor does a log line. What a front end must spell is in
the variant, never its `String`, a refusal a mode causes naming that mode where it is raised. `pgdt` appends its flags
in the library's order on every refusal leaving a command; `datafusion-pgdump`'s `Error` carries the words of the
`Spelling` a front end handed at open or registration, `pgdt sql` handing one per grammar. Rejected: a structured
`Remedy` (a second taxonomy); a sentence per variant per front end (wording that drifted); spelling in `pgdt sql`'s
`run()` alone; DataFusion's `Context`; a variant wrapping any refusal.

## Fixtures and tests (`scripts/`, `fixtures/`)
### D69 Fixtures are real `pg_dump` output on a pinned glibc image family
`postgres:<major>.<minor>-trixie`, exact minor, never `-alpine`: musl's `strcoll` is `strcmp`, so every text answer
would be the `C` answer. Four things move on regeneration (`\restrict`, `now()`, `--verbose` timestamps, OID drift) and
nothing may assert on them.

### D71 Agreement with PostgreSQL and `pg_dump`'s forms are generated checks, read out of their producers and joined by existence
"Agrees with PostgreSQL" is the fixture servers' own answers, committed per major (`comparison_oracle.py`,
`value_oracle.py`), never written by hand; one semantics, the newest major's, serves all (I35). A register is read out
of its producer's source, a `match` being no data and the upstream checkouts machine-local (`emitter_register.py
--extract` commits what the join reads), and joined by existence, an arm in an oracle case, a literal in a fixture of
its major, never by branch coverage. An exemption names what the check resolves, never "not read", the map tiling every
byte; a fixture exposing a `KD<k>` is held failing until its fix (`known_failures.rs`). Rejected: a C parser; a cross
product of session settings; a `DO` block testing the version in place of a sidecar file.

### D73 Round trips, asserted shapes, and a hand-verified signal path
`Typed` against `Strings` over a fixture and `encode(decode(raw))` against on-disk bytes hold that render inverts
decode; that decode is right is the value oracle's (`value_oracle.rs`). Boundary values are unit tests;
`v_box_domain_array` (I22), where the round trip is blind, is pinned directly. A shape under test is asserted
(`is_seekable()`, a stated `--jobs`), never assumed. No test races a signal or a scheduler: it trips or sequences the
event at a chosen point (`CancelsPast`, `a_lifted_frontier_takes_the_chunks_its_tasks_decoded`), and koji covers the
real scale.

### D102 The unrepresentable category is read off hand-written extremes through DataFusion's own path, never listed
Which values an Arrow type cannot hold is read, never listed: `types`' `t_extremes` and `t_extremes_nested` hold
PostgreSQL's least, greatest and special values of each arm mapping to a type other than `Utf8View`, by hand, PostgreSQL
keeping no catalog of them; each is held to what DataFusion's own `arrow-cast` formats and strictly casts, the rest
recorded by tier and held to D96's count, so an upgrade moving the calendar fails. `floor_mapping.py` forces a newly
typed arm's extremes (D38). Rejected: the list kept by review; a bound per type (the list again); a decoder's refusal as
the test (it took `24:00:00`). Code: `arrow_holds`, `extremes_problems`. Evidence:
`every_extreme_is_held_by_arrow_or_recorded`.

### D108 A hot-path change is priced by `--count-pair`, and a rise it resolves is placed by callgrind, then reworked or accepted
A hot-path change is priced in `instructions:u` on the shipped build at the tree's parent and at the tree, against its
own pair's spread, never as a re-taken figure. A resolved rise is placed by callgrind's `Ir` on the two legs, layout
only where callgrind does not reproduce it or no changed code sits above, then reworked into a shape as clean or
accepted; none is too small to place. A phase adopts the refusal by its spec. Rejected: one count a leg; determinized
shapes (a workload never shipped); a sampled profile placing it (line tables move the code); a materiality floor; rework
resting on codegen (`ChunkedScanner::end_pass`). Code: `cmd_count_pair`. Evidence: `test_measure.py`'s `CountPair`.

## Releases (`release/`, `scripts/release.py`, `scripts/release_ci.py`, `scripts/debian_window.py`)
### D104 A release is `pgdt` for Linux, linked against one digest-pinned Debian-stable image that also builds what the register times
Two targets, Linux on x86-64 and arm64; macOS waits on its own discovery path (P34), Windows on an I/O layer that is
`std::os::unix`. glibc, not static musl: the shipped binary is the timed one (D13). The floor follows Debian stable: at
least six months' grace for a new stable, moved by twelve (`debian_window.py`). One digest-pinned `debian:trixie` image
builds both targets and every binary the register times, so one pin names the build's glibc, the floor and every
figure's runtime, as `rust-toolchain.toml` names one compiler. Rejected: `cargo zigbuild` (zig's clang for every C
dependency); a build runner per architecture (arm64 rehearsed nowhere locally); cargo-dist (no glibc floor); the Rust
pin in the image alone.

### D105 The shipped build is fat LTO in one codegen unit, the leg retiring the fewest instructions on the typed query
`[profile.release]` restates `release-fat`, the leg retiring the fewest `instructions:u` on every query row, `pgdt sql`
first (`release-profile`). The count ranks profiles because wall clock cannot: which leg leads in time moves with the
tree, and a profile outlives every tree. Nothing else is ranked by count, and the default leg is held to fat by the
roadmap's downstream-build rule. Fat is DataFusion's own profile (D13). Rejected: Cargo's default (more retired on every
row); thin LTO (more than fat on every query row; its quicker build our cost, not a user's); user time (as
layout-exposed). Reopens: thin's or the default's count at or below fat's on `pgdt sql`, or ahead in wall clock at
successive trees; DataFusion moving its profile. Code: `test_the_shipped_profile_is_the_fat_leg_restated`.

### D109 arm64's archive is built on the maintainer's machine and attested by its release alone; x86-64's is CI's, with build provenance
CI's runner cannot hold arm64's fat link (D105), so the arm64 archive is built in the pinned image on the maintainer's
machine and attached to CI's draft. Its one attestation is the immutable release's, which `verify-asset` and mise's
`github:` backend accept alone (RL1, RL2). `attach` never refuses on CI's x86-64 archive differing from the rehearsal's:
nothing relies on that. Rejected: swap on the runner (a fat link paging); `ubuntu-24.04-arm` (the same memory); a paid
or self-hosted runner; an arm64-only profile (reopens D105); a keyless signature as the maintainer (mise's backend fails
it); `actions/attest` over the local build (provenance that verifies and is false); qemu-user for the smoke. Reopens: a
hosted runner holding the link; RL1 or RL2 failing. Code: `step_attach`, `arm64-smoke`.

### D110 The release suite is the round's, less the never-shipped measurement apparatus
`release.py suite` leaves out each member whose manifest sets `[package.metadata.pgdt] apparatus`; `mise run check`
still tests it. An attested build tests what ships: performance and its apparatus are verified on the maintainer's
machine before a cut, and the apparatus needs facilities a runner need not have (GitHub's x86-64 runner opened no
counter). Rejected: lowering the runner's `perf_event_paranoid` (no counter behind it); a counting test that skips where
the counter does not open (a broken counter passing here too); the apparatus listed in `release.py` (a new instrument
tested on the runner until someone edits it). Reopens: an instrument shipping, or a shipped member needing a facility a
runner lacks. Evidence: `test_the_apparatus_the_suite_leaves_out_is_a_dependency_of_no_member`.

### D111 CMake is a `mise.toml` pin for the apparatus, and a build is refused where a crate `pgdt` depends on ran it
CMake is a `mise.toml` pin, so the image has it, for the apparatus: zlib-ng's `libz-ng-sys` builds under nothing else.
The shipped build must not build `aws-lc-sys` under its CMake builder, whose compilation options are not its `cc`
builder's; while no shipped crate needs CMake, `release.py build` refuses any crate of `pgdt`'s normal-dependency
closure that left a `CMakeCache.txt`. Rejected: CMake absent (zlib-ng unbuildable where every figure's binary is built);
`AWS_LC_SYS_CMAKE_BUILDER=0` in the image (one crate guarded by name, another's fallback onto CMake unseen); Debian's
`cmake` (a version no checkout shares). Reopens: a shipped dependency built by CMake, the check then narrowed to
`aws-lc-sys`. Code: `cmake_caches`. Evidence: `test_release.py`'s `CMake`.
