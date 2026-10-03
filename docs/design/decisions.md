# Decisions

The decisions the code cannot explain: shapes kept for a deliverable not yet built, defaults chosen
over an alternative, and obvious changes measured or argued and refused. Nothing here says how the
code works (the named module does) or quotes a number (`measurements.md` does, by figure id, as the
invariant registers do by `I<n>`/`RT<n>`). Cite as `docs/design/decisions.md`, "D12"; the rest of the
rules, the line cap included, are `docs/process.md`, "The decision register".

<!-- decision-watermark: D103 -->

## I/O, memory and parallelism (`io.rs`)
### D1 The library never spawns threads by surprise
`Parallelism::default()` is `Serial`; discovery is opt-in, asked for by the CLI and by the DataFusion
provider's `ScanBudget` where no allowance is stated. Rejected: `default_workers` read in the library, so silence means concurrency.

### D2 A plain file recommends one worker; a local compressed one the machine's cores
On every real device a serial plain scan is device-bound, and each partition reads a chunk-sized
tail past itself, so splitting adds device bytes. `XzSource` answers `available_parallelism()`
capped at its block count (`RT7`); a fetched one answers one, the errors being asymmetric. Reopens: a parallel plain scan measured on a real device (both
parallel figures are warm-tmpfs). Evidence: `scan-throughput-*`, `parallel-scan-throughput`.

### D3 The memory constants, and what each one is
`MEMORY_RESERVE` is a subtraction, not a fraction, which under-reserves where being wrong kills the
process. `MEMORY_MARGIN_PERCENT` binds the resolved *count*, and the budget only by the part of `held`
the ceiling cannot absorb (`within_shared`). `MEMORY_UNPOOLED_BOUND` is a bound off a grid, never a
per-reader term. `DEFAULT_MEMORY_BUDGET` stays small enough to decline block decode on an ordinary
`.xz`; clearing that gate picks one number for two questions. Evidence: `reserve`, `chunk-size`.

### D83 `--memory` states what the process may hold; one carving serves stated and discovered
A read-buffer budget is a number an operator cannot size a container from, so the flag states
resident and `Parallelism::within` carves it — reserve off the top, `margin_allowance` on the count
— with `discover_in` calling the same function, so provenance still never enters `Parallelism`
(D64). Consequences: a source recommending nothing is left on `DEFAULT_MEMORY_BUDGET` capped by the
allowance less `MEMORY_RESERVE` whatever is stated, and the margin now binds a typed number. Rejected: a second flag; keeping the budget and
giving statistics what it leaves. Code: `io.rs`. Evidence: `reserve`.

### D4 A budget is solved against a source's cost, never divided by it
`WorkerMemory` carries a per-worker term and a shared pool term (`affords`, `at`): the block pool's
retention list is flat below `POOL_DEPTH` and a unit per reader above, which no scalar expresses.
`BufferPool` is bounded in bytes, floored at one slot; a unit the budget cannot hold is refused,
never shrunk (`BlockCache::affordable`). Evidence: `reserve`, `parallel-peak-rss`.

### D5 A wait is a permission the read loop grants
`NeverWait` is the default and allocates past the budget. Only `leader::scan_region` grants
`MayWait`, and restores it on exit: each leader worker holds one read, so a blocked one waits on a
sibling that finishes. A loop retaining into a batch holds the buffers it would wait on, and the
block pool never waits whatever the loop said. Rejected: gating on `Parallelism`.

### D6 `ByteRangeSource` mirrors `object_store` and is dyn-compatible
`read_range`/`size`/`modified` copy `get_range`/`head` so a remote source is additive behind a
feature without the dependency. Boxed futures so wrappers compose rather than branching per caller.
Rejected: generics matched per command; `enum AnySource`. `size_is_exact` is for gzip/zstd.

### D7 `partitions()` says where and at what cost, never whether
The same file is worth cutting for extraction and not for discovery, so the caller decides. The scheduler's only
refusal is a memory floor against the file's remainder, erring serial (`KD22`). Rejected: `Anywhere` as "stay serial".

### D8 The cut is one unit wide, sized apart from the charge
`BOUNDARIED_PARTITION_UNITS = 1`; on a boundaried source `window_end` sizes the cut, `partition_bytes` the
charge, and on `Anywhere` the cut is the charge (`KD25`). A wider cut wins at two stated readers and loses at
the flagless default. Rejected: deriving width from the charge. Reopens: an explanation of the flagless collapse.

### D9 Pool sizing constants
`hint_read_size` *becomes* the slot size and larger buffers are dropped on release: a chunk read and
`attach_text`'s one coalesced read are indistinguishable by length. `POOL_DEPTH` is the replay
retention depth, which only a block pool raises to `--jobs`, short of what one batch retains (`KD54`). `PLAIN_PARTITION_CHUNKS` caps tail-read
waste and is not derived from `POOL_MAX_BYTES`. Evidence: `chunk-size`.

### D10 mmap, `fadvise` and double-buffered readahead are refused
Positioned reads via `spawn_blocking`, one chunk at a time: no overlap scheme puts a cold scan below
the device's delivery time, which the scan already sits within a few percent of. mmap also bypasses
the trait, faults uninterruptibly and errors as `SIGBUS`. Buffers are pooled and the `Bytes` sliced
(`vec![0; len]` is `calloc`). `SCAN_CHUNK_DEFAULT_SIZE_BYTES` is shipped, not probed: rotational detection is
masked in containers and undefined over LVM/NFS. Reopens: parse CPU exceeding read time. Evidence:
`scan-throughput-*`, `chunk-size`, `allocator`.

### D11 Limit discovery is a public primitive taking a root
`discover_memory_limit_in` reads the least of `memory.max` and `memory.high` (RT3), walking
ancestors to the mount point (RT5); with no limit, a source recommending a charge is capped at half
of `MemAvailable`, untuned (RT8), and one recommending none is not. The arms worth pinning are ones
no machine is more than one of, so tests need the seam. Rejected: an env var overriding the root.

### D12 Workers are `spawn_blocking`; no runtime flavour is imposed
The library keeps `tokio` at `rt`+`sync`; the CLI runs `current_thread`, so thread count follows
dispatched work, not CPUs (an idle reactor thread seeds a glibc arena). Rejected: a thread pool of
our own; `rayon`. Consequence: cancellation is a cooperative flag (D26).

### D13 The allocator is the binary's choice
No `#[global_allocator]` in the library; `pgdt` links the platform allocator, `jemalloc`/`mimalloc`
are opt-in features, `--version` names which. Rejected: `mimalloc` on a few percent, making every
table a figure of an unshipped binary; `mallopt(M_ARENA_MAX)`, binding if set at resolution (RT10)
but saving only below the count arenas already follow (D12), on `.xz` alone, at unpriced contention,
overwriting the operator's `MALLOC_ARENA_MAX`. Reopens: a contention figure. Evidence: `allocator`.

## The compressed source and the cache (`io.rs`, `cache.rs`)
### D14 `.xz` is read; recognition sniffs content
An `.xz` dump is third-party post-compression (input reach); gzip/zstd are `pg_dump --compress` output
(compatibility). Recognition compares magic and sits outside the trait, and is *told* those bytes by
`Origin`'s probe — which also answers D20's size, so what is settled before a source exists stops being a
statement about files. Rejected: extension dispatch; a probe cached across failure. `xz-seek` is a vendored
read-only copy (`scripts/vendor_xz_seek.py`); a bug is fixed upstream, and so was the seam a fetched source
needs — nothing can `await` inside `CompressedSource::read_at`, so the walk and the block handle are sans-IO
and driven from here. Rejected: a path dependency, failing `cargo check` without the sibling checkout.

### D15 A read decodes whole blocks and retains them; reading one in pieces is the fallback
Whole-block decode, LRU-retained, afforded at one reader against the caller's number from file-wide inputs
(`BlockCache::affordable`). Rejected: a fixed refusal line; keying on block count. A file whose largest
block the budget cannot hold reads through one live `xz_seek::BlockRead` behind a mutex — one state machine
both arms instantiate over their own `CompressedSource`, never one body branching on the provider — kept
across reads so a forward scan continues, advising one partition (`KD35`); the fetched arm's window is
charged rather than booked unpooled. Two misses decode one block twice (`KD20`). Evidence: `reserve`.

### D17 Two pools per source; retained and free units share a pool's slots
The hinted unit drives both what a pool keeps and how it sizes, so one pool is wrong for either
unit. Eviction runs before acquisition and the reservation drops before the evicted blocks do, or a
released buffer meets a reservation and is discarded. A reuse rule, not progress: see D5.

### D18 The seek table is persisted and identity is three-state
A many-stream file costs a footer read per stream, so `cache::save` records the table, `claim` reads
it before a source exists and `XzSource::with_table` walks nothing. `KnownCompression` is
`Unknown`/`Plain`/`Xz`, checked against the file's magic; a contradiction is `Recognized::Mismatch`
and condemns the span index too, both having come from one save of one file.

### D19 A budget decline is a `PlanNote`; a non-seekable file is warned, not refused
A decline is a property of the file *and this run's budget*, which no persisted `DiagnosticKind` can be.
A one-block file opens and raises `NonSeekableCompressedSource` naming `xz -T0` (`map_file` drops it, `KD58`): only the user
can judge one decode-from-zero. A statistics skip is a note too, stated at zero wherever statistics were consulted,
and a narrowed span (D84) one wherever the plan charged less than stated; the ordinary case is the
invisible one. Rejected: omitting a zero; counting only believed filtered columns; announcing a narrowing
only where the count fell short too, which `ParallelismBudgetLimited` names. Reopens: a puzzling zero,
which naming the columns lacking a statistic would answer.

### D20 The library never replaces cache data automatically
A cache not ours, damaged, another build's or another file's (stored size, or a compression layer the source
contradicts) is almost always a wrong path or a changed file, so `CacheMode::refusal` refuses it before the dump
is read past its magic, at every scan entry point and after `cache::claim`. `--overwrite-unusable-cache` (`with_overwrite_unusable`)
starts cold over one recognisably ours, by a header read first, for a file replaced under a stable name; foreign
bytes never. Rejected: a cold start (a silent re-scan after a typo or an upgrade); the guard in `cache::save` (policy
an embedder cannot override); refusing a damaged one of ours (nothing readable lost, an unbumped version its likely
cause); `load` checking size alone (a claimless open gets another file's map). A moved block is `CachedBlockChanged`.

### D21 Identity is `stored_size()` plus weak signals, in an opaque enum
`stored_size()` keeps the check a `stat` where `size()` needs a decompressing source opened first, and `SourceIdentity`'s variants are read through
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
`PgDump::open` believes a cache reaching the file's end whose identity checks pass, and names the `pgdt parse` that
builds anything short; `datafusion-cli-pgdump` never parses either, a parse wanting `pgdt`'s discovery, interrupt
guard, status lines and cache rules. So every block a table owns is seen (`KD6` is the cold query's), a schema is
stated from every block's census, no database's DDL is unread, and the partitioned replay is always available. With
no partial map to keep, a dropped stream is its whole cancellation, no `ScanOptions::cancel` set (D26). Rejected:
falling back to a cold query's early-stopping map, which brings each of those back. Code: `datafusion-pgdump/src/dump.rs`.

### D22 `CacheLoad` is its own type and `CACHE_FORMAT_VERSION` is bumped freely
`Incomplete` is usable (or `map_forward` restarts from zero), `Disabled` is about the caller and
`Unusable` carries its reason to every caller. Bump on any persisted reshape, a parse fix changing what a span or
the metadata holds included; record it nowhere: an older cache is then refused until replaced by the flag or deleted
(D20), which is what a user hears of first. Rejected: an unbumped parse fix, its cache answering as the old parse did.
Code: `cache::CACHE_FORMAT_VERSION`. Evidence: `persisted_index_is_pinned_to_the_format_version`.

## The scanner (`scan.rs`, `copy.rs`)
### D23 The scanner never owns the bytes it scans, and only the whole `COPY` grammar is structural
`CopyScanner` is a synchronous state machine over a caller-owned buffer; the only memory bound is `max_line_bytes`,
and exceeding it errors, never truncates. A chunk is scanned in two passes, carried line then chunk in place
(`ChunkCarry`); a growing buffer copied every byte twice. Only a line matching the whole `COPY … FROM stdin;` grammar
is structural, an off-grammar one being ordinary SQL; inside a block only an exact `\.` line is looked at (I7). Outside
one every line is lexed as psql lexes it (`lex.rs`, I50), a line beginning inside a region is never structure, and a
dollar-quoted one emits no lines, only a closing offset (I1). A bare `BEGIN;`/`COMMIT;` pair is the large-object
region, skipped unread (I12). Rejected: tracking `$` alone, or quotes too (a `$$` in a name or comment hid every block).

### D25 Parallelize what is CPU-bound
Decode is always split, extraction on any source, discovery only behind a decoder; the worker is
fused. One core's rate against the device's offer decides each; readings that disagree reopen it.
Evidence: `xz-decode-scaling`, `parallel-scan-throughput`, `scan-throughput-*`.

### D26 Cancellation is per chunk or leader window, honoured by the mapping passes alone
A block can be hundreds of gigabytes, so block-boundary cancellation is a hang; `map_file`'s scan and back-fill keep a
partial result. The preamble scan ignores the flag, a stop there reading as the first `COPY` header and caching as
complete. `Cancellation` carries a signal beside the polled bit, so a waiting reader drops its request. **The shape follows
the command**: `query` errors; `parse` interrupts, at byte 0 too, then dies by the signal, and a second one, mid-save too, or
one in the listing, ends it in the handler. As init (RT19) both binaries exit 128+n on every signal ending them elsewhere
but a fault's, left to the kernel, and what `parse`'s guard or the REPL's `ctrl_c` catches. Rejected: polling alone; keying
on what was banked (a race); `exit(128+n)` elsewhere (RT20); needing an init; as init, INT and TERM alone; fault handlers.

### D27 UTF-8 is validated once per chunk, and the library's one `unsafe` is the view append
`validated_prefix` validates the largest line-terminated prefix and fields slice the `&str` with
`str::get`, delimiters being ASCII. The escaped path still validates (`\xNN` synthesizes bytes); the
bulk pass runs only when a row will decode something. Four `unsafe` attempts lost to the safe shape;
the one that stands is `append_view_unchecked` (`batch.rs`), whose bounds come from the `contains`
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

## The file map and the preamble (`map.rs`, `index.rs`, `preamble.rs`)
### D30 The statement grammar is primary; the TOC is enrichment
TOC-driven segmentation is unsound on non-`pg_dump` input, and a dollar-quoted body can hold a
TOC-shaped line (I3). A tiling hole is `TilingBroken`, never a refusal. A span's `end` is fixed by
the next push, so `Builder::snapshot` is possible. The preamble prepass (byte 0 to the first `COPY`
header, I1) runs up front in both mapping entry points, or an interrupted `parse` banks blocks with
no DDL. `attach_text` slices span text after building and `Data` spans store none, which is why
`query` cannot run cache-only. Evidence: `preamble-prepass`.

### D31 `Span::toc` is "belongs to", vetoed after classification
Follow-ons inherit `governing_toc` (else coverage reads half on healthy input) and `toc_owned`
counts objects; `Framing` vetoes after the transition that seeded the inheritance, and `Connect` is
never seeded. The boundary predicate refuses `"Data for "` and accepts `"Statistics for "`, which matters under `--disable-triggers` (I31).

### D32 Boundary rules that read as bugs
A blank line does not close a pending comment (`_printTocEntry` writes `--\n\n`); `scan_preamble`
retreats to `pending_comment_start()` rather than guessing the comment's kind.

### D33 Bulk regions are one span kind; large objects skip at the scanner, `INSERT` runs at the map
Only `DataBlock::Copy` has inner offsets, only `COPY` having a row reader; a data span absorbs the
TOC comment pending at its start (`KD1` where none is). `INSERT` runs fold in `feed_line` with no scanner state (their
boundaries rest on no line-anchored invariant) and the run's end is lexed as the scanner lexes. Three cuts
stay untaken until the `INSERT` row reader exists (`KD9`).

### D34 `DumpIndex` stores no fact twice but sortedness; whole-file facts need `is_complete`
`blocks()` is filtered, `metadata` computed once, diagnostics never persisted, roles excepted; a
streamed schema commits over the blocks it replays, ungated (I2). Statistics sit in their block
behind an `Arc`, so save-gate clones copy a reference (`KD5`), and store sortedness; a group is a
byte range, no piece knowing a global row index. A back-fill keeps a block's columns, and its size
unless a stated bound it did not record moves it, a maximum it breaks re-reading it at the size its
groups predict. Rejected: `SparseRowIndex`; padded `character` bounds and entries, keyed alike but
past the cap; a back-fill narrowed, dropping only re-read blocks' columns.

### D35 The census is type-blind, records both dimension bounds, and runs at the data level
`ArrayShape::observe` reads the leading brace run off still-escaped bytes at L1 (I15, I25); min and
max depth, or `{1,2}` beside `{{1,2}}` resolves to a wrong `List`. A table any column of which is at
the data level is censused, and so is every block a query's pass maps: gating on a full scan left
early blocks uncensused. The metadata level records `None` and splits no field; a typed query re-reads
such a table for itself, writing nothing, and a plan over a held map refuses it. Rejected: degrading
to the DDL's `List`; a census-only level a user can reach. Evidence: `statistics-gathering`, profiled.

### D96 The unrepresentable count rides the census, typed by the DDL, in two tiers, under a recorded calendar
Every census-taking read counts per block and column what the declared type's typed pair (no census) cannot hold, a leaf
by its own type, lexically, the boundary years alone by arithmetic; a nested value once, in its worst leaf's tier:
`format` past Arrow's spec, `engine` past `calendar_end` (RT21). It persists as counts (D74); a type holding more or
less bumps `CACHE_FORMAT_VERSION`, as D78's order does, and a cache counted under another calendar is refused (D20).
Rejected: a decode per field; a type-blind test (`text` reading `infinity`); counting as a query reads (timing decides);
one tier; a value the server refuses, a decode error, this being our front end's limit. Code: `unrepresentable::Counter`, `map::FieldCount`. Evidence:
`every_extreme_is_held_by_arrow_or_recorded`, `the_types_fixture_counts_what_its_typed_columns_cannot_hold`.

### D36 The preamble grammar dispatches on fixed keywords and never guesses
Unrecognized lines are ignored, so `--binary-upgrade` noise is free (I5, I6). `record_type` keys on name (I11); a composite's field
list is all-or-nothing, `record_out` being positional (I23); a `--create` dump's pre-`\connect` segment is not a database (I9). L1
stores text, never a conclusion: a declared type is its words, comments and spacing dropped, a collation clause verbatim, `None`
collation is "no clause" (I37), and `CollationDef` keeps only `deterministic` (I42). A type's or collation's name is kept in one spelling,
both sides of a lookup compared in it (I29). A table keeps its `INHERITS` parents and `OF` type, and a column is found through them as it
is looked up. Rejected: a name's parts dequoted, `"a.b".c` being `a."b.c"`; references flattened in at the fold, which a later `ADD COLUMN` misses.

## Type resolution and decoders (`pgtype.rs`, `resolve.rs`, `decode.rs`, `nested.rs`)
### D37 The bar: the dump alone determines the value
A declared type maps to a real Arrow type only if its text round-trips consulting nothing outside
the file; otherwise `Utf8View` with a note naming the kind of unknown. Misreading is unrecoverable,
not recognizing is not; `money` fails it (`KD13`). Every column is nullable regardless of DDL; a
range's three flags are not. Rejected: a bare built-in name `Unknown` where a declared type could
shadow it — `pg_dump` writes built-ins bare under an emptied path (I8), so it would demote its own
columns; a collation takes the weaker verdict because `pg_dump` qualifies every one (`KD48`).

### D38 The ADBC driver's shipped release is a floor, swept from the catalog
Where the driver yields a real Arrow type, ours is never wider; the floor is a pinned release, never
`main`, one row per declarable `pg_catalog` type. `status`/`extension` columns take a row out of the
rule by themselves, and only what they cannot say gets a `Disposition` with a stance and a resolving
citation. `floor_mapping.py` compares value space (`Utf8View` is `string`) and reports an unknown
arm, never guessing. It binds the typed mode alone: the untyped mode, like `--schema-mode strings`, is the user
asking for a type wider than the floor, and widens only a column the map says its front end cannot hold (D100).
Rejected: the `typelem` shape test, which deletes `int2vector` (I8, I39).

### D39 `NestedPlan` travels beside the `DataType`, built with it
An Arrow type does not name its literal (`int4range[]`, `int4multirange`: both `List<Struct>`);
every builder takes a plan. Rejected: widening `builtin_scalar`'s tuple; `Field` metadata.

### D40 The comparison is decided per declared type, in the same arm
`builtin_scalar` answers Arrow type and `CompareKind` both, per declared type and `COLLATE`; types
sharing `Utf8View` compare differently, and `predicate.rs` never reads the Arrow type. DataFusion
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
out-of-range parts are unrepresentable values (D96, D99), as `date`'s and `numeric`'s are. Twelve built-in range
names are fixed, multiranges apart (I10); a user-defined range declaring `canonical` is unanswerable (I46).

### D43 The census speaks after the DDL and moves the pair
`retype_from_census` and the text mode's `read_as_text` (D100) alone change `(DataType, NestedPlan)` after
resolution, the census a parameter so no caller skips it; a run past `MAXDIM` is tested first and kept optimistic (I25). `MetadataNotScanned`
refuses a stream and degrades a listing: same output as `NotDeclared`, opposite advice.

### D44 The control's decoders allocate only where their return type does; its renderers use tables
The measured control's decoders take no intermediate `String`, and its renderers write into one reused buffer
through `DEC_PAIRS`, `uuid`'s and `bytea`'s into one pre-sized `String` each through `HEX_PAIRS` (`tests/render_allocations.rs` pins the count); `interval`'s, a decimal's and a
float's still go through `core::fmt`. Render-back's third outcome, `FieldRender`, is an Arrow value no text form spells (I40), never rounded.

### D45 One quoted-token scanner; strict decoders apart from permissive parsers
Array, record and range literals share one scanner parameterized four ways (I20). `decode_*` reads
what a dump holds and cannot loosen (the render inverse); `parse_*` reads what a user typed and
cannot tighten; each transcribes the newest major and under-accepts (I35, I44). Wrong depth,
`[lb:ub]=` or a field-count mismatch is refused, never reshaped. Evidence: `nested-decode-micro`.

## Batches, streams and the leader (`batch.rs`, `stream.rs`, `leader.rs`)
### D46 Zero-copy views are top-level `Utf8View` only, and `max_source_span` alone bounds pinned bytes
Every other arm copies. A retained chunk is released only past its *last* byte
(the carried row arrives inside the next chunk) and `invalidate_block_cache`
runs on every flush keeping its batcher; any new flush trigger must honour it.
`max_rows` and `max_bytes` count selected rows, which a filter makes sparse.
The span term is dropped only where the advice is non-empty and uniformly by-partition (`KD23`).
Rejected: compacting views past a selectivity threshold. Evidence: `parallel-peak-rss`.

### D84 The batch span is derived from the budget and the count, and spent before the count is cut
`max_source_span` is a ceiling: `plan_partitions` charges `(budget − charge.at(jobs)) / jobs`, floored at the caller's
announced `ScanOptions::chunk_size_bytes` — one read chunk, below which the span costs rows and bounds nothing the
retained unit does not — and writes it onto the sub-streams. A plain source stays at most `DEFAULT_MEMORY_BUDGET` whatever
is stated (D83, `KD32`), which the shipped span spent whole, so `--jobs` bought no readers. Rejected: a third flag
(`roadmap.md`, "Two tunables fit pgdt to hardware"); pricing a plain reader (`KD25`); a *larger* floor, a performance
claim with no batch-size figure behind it; the *shipped* chunk as the floor, which at `--chunk-size 64k` declines
readers the announced one seats. Reopens: a floored span measurably slower than an unfloored one. Code: `stream.rs`. Evidence: `parallel-scan-throughput`.

### D48 Mapping and replay are separate passes, and `splice` owns the seam
The map is never behind the rows, so a `ResumeToken` points inside mapped territory. A segment is
spliced by extending the *preceding* span; a start floor on `Builder` made assembly visible in the
map. A cancelled mapping pass fails a query rather than shortening it (I1). Cached replay and the
cold interior split share `worker_count` and `cut`, and differ in what they cut and in the replay
charging each sub-stream its batch span (D84).

### D49 One target per query, and the early stop is conservative
Name matches narrow to one `(database, table)` before replay or `AmbiguousTable`; `target_settled`
vetoes a stop on a partition root (I2) or any `\connect`. A conflict past the stop is unseen (`KD6`).

### D50 `ResumeToken` is opaque and fingerprints the query
The filter is hashed by explicit match, as a derived `Hash` silently misses a new operator; `database`,
`scan_extent`, the batching knobs and `use_statistics` stay out, no skipped group holding a resumed row.

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
re-pruning a whole block per generation; the filter handed through the plan node, whose clones share it; seeking the scanner in place at a skip.
Code: `stream::DynamicPartitions`, `prune::DynamicPruning`. Evidence: `pgdump_query/tests/dynamic_filter.rs`, `datafusion-pgdump/tests/dynamic_filters.rs`.

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
over the kept groups; a read-time refusal beside it, the count being exact (D96). Code: `stream::materialized_unrepresentable`.
Evidence: the refuse mode's cases, `a_selected_special_value_still_cannot_be_materialized`.

### D100 The untyped mode widens a column per table from the map's count, and compares it in each semantics
Under `UnrepresentableMode::Text` a column whose blocks count, over every one, a value in the tiers its semantics cannot hold (D96, D98) is
`Utf8View`, settled once per table as the census is (D43), the rest typed; `UnrepresentableValues` keeps the declared plan under PostgreSQL's
semantics, special values ranked (D56), and is `Refused` under DataFusion's, its text bytewise and no bound gathered in the type's order read,
so a pushed filter stays `Exact` (D88). Rejected: text everywhere, `pgdt --where` refusing its order as an unmapped column's; per block, two
blocks of one table typed apart; the declared plan in both, each reader of `comparisons` then asked to know which semantics widened it.
Code: `resolve::read_as_text`, `stream::TableColumns`. Evidence: the untyped mode's cases,
`the_untyped_mode_compares_its_text_column_in_each_semantics_order`.

### D103 The unrepresentable mode is one option for every column, fixed where a dump is opened, apart from the schema mode
`QueryOptions::unrepresentable` is stated by `pgdt query --unrepresentable`, `PgDumpOptions`, the `pgdump.unrepresentable` table option and the
shell's `:unrepresentable=` suffix; the text mode changes a table's schema, which the provider fixes at `PgDumpTable::build`, so no `SET` reaches
it. `--schema-mode` is about ignoring the DDL, and the mode, which reads it, is moot under `strings`. The library's refusal names the modes in its
own words, never a front end's flag, and the provider passes it through. Rejected: a `pgdump.*` session setting, changing a schema a statement
cannot rebuild; a third `SchemaMode`, folding a choice about values into one about the DDL. `Null` is the default, such values being rare in
real dumps and the floor's type kept (D38). Code: `UnrepresentableMode`, `PgDumpTableOptions`.

## Predicates (`predicate.rs`, `where_expr.rs`, `pushdown.rs`)
### D53 The operator set is closed but for membership and the unrepresentable test
No `LIKE` (collation-dependent folding), `BETWEEN` (`And`), or column-to-column; `IS [NOT] DISTINCT FROM` is what three-valued logic forces. `IN` is
`Expr::In`, answering as the `Or` of `=` but decoding once and looking up once, since per row that `Or` costs per term (`dynamic-filter-join`).
`IS [NOT] UNREPRESENTABLE` is what the null mode forces, its NULL otherwise the dump's to every operator (D101).
Rejected: an `IN` `PredicateOp`, every operator carrying a list; recognizing the `Or`, every front end emitting `In`. Evidence: `tests/membership.rs`.

### D54 One tree, no planner, short-circuit defined against the root
`filter` is one n-ary `Expr`, by default the empty conjunction. `And` may stop at the first `Unknown` except beneath `Not`, since only the root's `True`
matters; a decode failure surfaces only where evaluation reaches it: never in a group statistics rule out, nor serially past a sorted block's stop, but split a
later piece evaluates one row past it, so outside the contract `--jobs` decides. Resolution refusals come from the plan before any row, for the first refusing
block in file order, walking leaves the evaluator would skip; a block with no column list refuses where reached. Rejected: DNF; exact Kleene everywhere; not
skipping a group holding an unkeyed value (a nested column, `KD2`'s, is never keyed) or under a term naming one; the stop asked first, every valid dump paying
for invalid text. No planner defers complexity until one buys something and refuses no evident simplification: a resolution-time rewrite removing redundant
work — a field decoded once a row for every leaf reading it, bounds an `IN` implies — is admitted where a reading shows it pays. Evidence: `tests/pruning.rs`.

### D55 A literal is read in the type's `*_out` form at least and its `*_in` grammar at most
`*_out` is the 1.0 floor, `*_in` the ceiling: a literal `*_in` refuses is refused. Between them effort is minimized: an `*_in` spelling is read where free,
simpler or faster, none is refused at runtime cost, and the rest are `PredicateValueDecode`, a shortfall, never a rule. What a kind reads past its floor (one value's
other spellings, field and literal alike; `jsonb`, its canonical form untypeable) is on `accepted_form`. A literal finer than the scale is refused. `JSONB_MAX_DEPTH`
is fixed: a Rust stack overflow aborts. A field fails a parse only where a `pg-refuses` check refuses it (`decode::Unread`). Rejected: failing on every field read as
no value, which aborts on a shortfall a restore reads (`KD83` is the converse, a refusal let through to the query).

### D56 Special values are a rank in the key; equality has three canonicalizations, by injectivity of `*_out`
`infinity`/`NaN` are their position in PostgreSQL's order; `OrderKey` derives no `Ord`. Rejected: excluding the row like a NULL (I33, I34).
`v=1.5` hits a `numeric(10,2)` written `1.50`; rendering `timetz`, `inet` once is refused (I33, I38, I41).
A `bytea` literal renders in both `bytea_output` forms, which share no spelling (I56); decoding per row is refused.

### D58 A nested column has one comparison path, and the leaf grammar does not widen
Structural key walk for both operator families; `nested_key`'s `input` flag stops at the container
but for `int2vector`, whose elements `int2vectorin` reads. Both sides of a range go through
`make_range`, keyed on the range type; a user range declaring `canonical` is refused under every
comparing operator, the NULL tests reading no value (I44–I47).

### D59 Divergence is per position on its own channel: per term, and per column at registration
`ComparisonNote` carries a path and its declared type, per term from `comparison_notes` (operator-
conditional, so L4) and per column from `column_divergences` in a query's semantics — DataFusion's variants
there alone, as DataFusion's `ORDER BY` reaches a column no term names. See I45, `KD7`, `KD10`.

### D60 `--where` is a second flag and the tokenizer defines the refusal set
`refuse_where_structure` refuses a `--filter` term tokenizing to anything but one leaf, an empty one `parse_filter`'s, so a string
both flags accept means one thing. A keyword needs whitespace, a paren or the string's end on both sides
and `NOT` after `is` stays in the term. A term splits at the earliest operator outside quotes, longest
first; quotes are stripped in a term, under either flag, and in no name flag. Rejected: `&&`/`||`; backslash escaping.

### D88 A pushed filter is `Exact` where the plan resolves it; a literal is the renderer's text
`supports_filters_pushdown` asks `table_schema` to resolve the translated term in DataFusion semantics,
the scan's own refusals. A typed literal must be the column's Arrow type and is written by
`render_field`, the decoders' inverse; a string one stands only where the library compares text.
Rejected: a formatter per type in the provider, a second grammar to drift; pushing `IN` on a float,
answered from a set with no `-0` made `0`; `Exact` where the library agrees with PostgreSQL, which answers an enum `<`
in declaration order pushed and in label order kept; `Inexact`, promising a superset another semantics can break.
Evidence: `datafusion-pgdump/tests/pushdown.rs`.

### D101 The unrepresentable test reads the text against the declared type; DataFusion's is a scan's alone
`IS [NOT] UNREPRESENTABLE` tests a field's text in the tiers its semantics reads (D98), whatever the mode or the column's resolution,
two-valued; a group answers it off its own count (D97), and `SchemaMode::Strings` refuses it. `pgdump_unrepresentable(<column>)` is pushed
`Exact` and refuses where DataFusion evaluates it; registering a dump registers it and leaves planning alone, and a physical optimizer rule the
embedder appends to a `SessionStateBuilder` refuses at planning any node's expressions holding it (RT22). Rejected: a table function or a companion column
(no row identity; 55.1 has no hidden columns); evaluating it (a NULL keeps no origin); registration installing the guard (a rebuild, dropping prepared
statements); a wrapped planner (one slot, trusting an `Exact` conjunct gone); the function opt-in too (withholds the always-sound pushed use).
Code: `unrepresentable_tests`, `datafusion-pgdump`'s `unrepresentable::with_guard`. Evidence: `the_unrepresentable_function_*`, `tests/pruning.rs`.

### D94 A dynamic filter is translated loosened into the library's tree, never read through `PruningPredicate`
Every producer re-checks its rows, so the scan answers `No` and only ever skips. The state is translated as a static filter is (D88), a part with no
term standing as whatever keeps every row beneath its `NOT`s, so no state refuses; a `CASE` is the `Or` of its branches. A float compared with a zero
has no term: a TopK and `MIN`/`MAX` order the zeros apart where evaluation equates them, and this holds whichever order a producer keeps. Held filters
are dropped at `reset_state`. Rejected: `PruningPredicate` (bounds decoded to scalars, pruned in DataFusion's semantics, nothing on a `CASE`, a long
`IN` or a sorted block); a narrower zero rule, sound only as far as each producer's shape is followed; keeping filters across a reset as Parquet does,
a recursive query's next iteration needing rows the last one's ruled out. Reopens: a release whose producers hold with the zeros equal, then an `RT<n>`.
Code: `datafusion-pgdump`'s `dynamic_filter::loosened`, `exec.rs`. Evidence: `dynamic_filter/tests.rs`, `tests/dynamic_filters.rs`, `tests/statistics.rs`.

## Statistics (`statistics.rs`, `gather.rs`, `prune.rs`, `summary.rs`)
### D75 Pruning takes only what each operator family proves
Bounds answer the ordering operators, and under an equality operator rule out "equal" but never "unequal"; a dictionary answers the
equality operators; a stop needs an ordering term the root conjunction requires, on a column sorted the way its bound closes; terms
combine as if independent. Rejected: bounds proving "unequal", `Canonical`/`Trimmed` comparing spellings, one to a key only in text
`*_out` wrote; and, though sound, a dictionary answering ordering, or a stop under `=` or `NOT` over the opposite operator. Reopens: a
filter shape a reading shows common. Code: `ResolvedTerm::truths`, `prune::SortedStop`. Evidence: `tests/pruning.rs`'s generated check.

### D76 Gathering never keys a bytewise value; a bound past the cap is a prefix and a successor
`text`, `character` and `bytea` order by a clipped head (`gather::Clipped`), not an `OrderKey`: a key copies the whole value, and a row
may run to `--max-line-bytes`. Values agreeing on the head share every byte a stored bound reads, so bounds stay valid and only row
order is lost (`Unsorted`), where a decoded-kind value past the cap loses its group's bounds too. A truncated text `max` replaces its
last character by the next scalar value (`text_upper`), `bytea`'s increments its decoded bytes. Rejected: the byte successor for text,
which can end mid-character. Evidence: `gather.rs`'s `a_long_texts_stored_bounds_are_on_the_right_side_of_it`.

### D77 The statistics request is the mapping pass's argument, and the metadata level is a selection
`map_file` takes a `&StatisticsRequest` and no query entry point does, so no option a query is handed
reads as a request it ignores; a query's pass censuses and gathers nothing, which no selection spells.
A selection is a default level and overrides, the most specific winning, so the type's `Default` is
the data level everywhere. Rejected: a `ScanOptions` field; `Option<&StatisticsRequest>`, whose `None`
contradicts that default or spells it twice; refusing a size beside `METADATA`, or two equally specific
entries, in the library (the CLI refuses both, a person's intent being ambiguous there). Code: `statistics::StatisticsRequest`.

### D78 Statistics share the cache file, its identity and its `CACHE_FORMAT_VERSION`
None is believed from a cache sized unlike the live source (D20); the mtime stays advisory (D21) so
a same-size rewrite prunes against its predecessor's, knowingly. Bounds, order and dictionary hold
only under the declared type and `COLLATE` recorded with them, and changing how a kind orders or
equates bumps the version. Both ends stream; a save renames its own file over the cache, so no copy
meets the statistics and a kill keeps the last save, orphaning its file. Rejected: a separable
statistics file; its own semantics version; the copy charged; one beside name two saves share.
Code: `prune::prune_block`, `cache::save`. Evidence: `golden_order_is_pinned_to_the_format_version`.

### D79 Bounds go in each order a semantics compares by exactly, a dictionary where it equates exactly
A scalar column keeps bounds and row order per exact order, one set where two coincide: the register's where exact, else DataFusion's
(bytewise: text in any collation, `jsonb`, off-`C` `character(n)`, no plan, no DDL), and DataFusion's beside an exact one it differs from
(a D78 bump); `macaddr`'s serves both (I40), and a float's, its tie of `-0` and `0` broken as `total_cmp` breaks it. A term reads
the set keyed by its compared kind as gathering resolved the column, not by its plan. A dictionary needs equality alone, reaching
`KD7`'s text, never an Arrow encoding (D37); nested columns get neither. Rejected: gathering a distinct count (D89 derives one);
bounds off a dictionary (D75); one slot; a second `macaddr` set, or a float's for one pair of values; the zeros told apart in the
key, which every term equates them by; a kind stored per set; a DDL flag. Code: `bounds_kinds`, `bounds_set_keyed_by`, `stored_order`.

### D80 An early stop is reported after the fact, per block, in bytes
Rejected: counting rows past the stop, never read and so only estimable; a zero for a stop planned
and never reached; exact bytes when split, read past each piece's limit. Reopens: an account needing
exact bytes, which each piece reporting its first row's start would give. Code: `stream::EarlyStop`.

### D81 The statistics account sums allocation sizes as they change; an instrument scope checks it
Each term is the allocator's request — capacity, a table's layout (RT11, RT12) — under one lock, a
vector or map charged ahead of its growth, in flight made when short, unmade when over. Rejected: a
per-group bound, bytes never held; growth charged after it; atomic terms, a moving charge read in
neither; in-flight growth unmade both ways, faulting a table another worker holds; walking every
block; proportional slack. Reopens: a leg past its tolerance (`statistics_account.rs`); an unseen
term growing with the dump; the lock in `statistics-gathering`. Code: `statistics::Charge`.

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
byte is read; a cache's are known first, the loading pass's `held`, a replay's segments dropping them. The CLI's is `--memory`,
else the limit, else half of `MemAvailable` (RT8), an embedder stating none declines nothing (D1). A declined piece declines its
block, dense group indices expressing no gap, and the account sees a window's pieces, so a decline differs serial and parallel,
as meant. `finish`'s last close is kept even past it; the next block declines at its first charge. Rejected: a constant bound,
an OOM on a wide table; coarsening to fit, a cache depending on its container; retrying every run; skipping the piece;
MEMORY_RESERVE again, carving the cap, not this. Reopens: the check's cost, or declining's saving. Code: `Gatherer::decline`.

### D86 Statistics volume follows columns and groups, not row width
Dictionary text is interned once per block and column, not per group, so an input of wide distinct text
cannot fill an allowance — the attribution sitting's wide-text leg was deleted rather than corrected.
Final groups are about `min(block bytes / group size, BLOCK_MAX_ROW_GROUPS, rows / minimum)`, so above one
group size per minimum row count the density merge binds and doubling a row's width halves the volume;
many short columns raise it, as does a bytewise-comparable column earning per-group bounds. The allowance
is itself non-monotone in the container limit, being `margin_allowance(allowance) − budget`. Rejected:
sizing an attribution input by row width. Evidence: `statistics-gathering`.

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
sized by bounds and type widths, an invariant a type; a guessed selectivity. Code: `gather::Summand`, `byte_size`. Evidence: `tests/statistics.rs`.

### D97 A group keeps a view per tier only where it holds such a value, the values its type holds the base
A column whose type cannot hold every value counts them per group and bounds and orders the values it holds; beside them, where a
group holds one past the format spec, every value in PostgreSQL's order, and where one past the calendar, the values within it, for
that group alone, read in a `StatisticsView` that adds what it takes as NULL to the NULL count, believed only under D78's DDL. The
base is kept apart from each tier's extremes, allocated at a group's first such value, so a view folds the base with the tiers it
takes, exactly across pieces and merges (D82); every value PostgreSQL admits keys, a timestamp from PostgreSQL's epoch (I49).
Rejected: every value's bounds and the count alone; a set per view per column from its first row; a timestamp key of
`i128` from 1970, a width chosen where PostgreSQL's is inherited. Code: `gather::KeyedGroup`, `gather::ViewOrders`. Evidence: `each_view_bounds_and_orders_the_values_it_takes`.

## The CLI (`main.rs`, `error.rs`)
### D61 `info` never scans
The surprise is that a scan happened at all; `parse` scans ahead, a `query` maps only what the cache
lacks (D48), and `--preamble-only` hangs off `parse`. Rejected: a scanning fallback; `--no-scan`.

### D62 The save throttle is a ratio, and its gate opens splice too
A save is skipped unless `K` times the last save's duration has elapsed, which bounds overhead in
every regime with no constant to tune. The map is rebuilt at the gate's openings, not every
`CopyEnd`; splice's third opener matches the queried header, a superset of save's, `target_settled`. Rejected:
every N seconds or bytes; a disabled-cache floor. `KD5` remains. Evidence: `per-block-quadratic`.

### D63 The interrupt flag is read at both extremes; resume is the default
Per chunk and at every completed block, since neither alone bounds the response. A resumed scan is
span-identical to a straight-through one, which is what makes an interrupted `parse` saved work.
Rejected: `select!` over `ctrl_c` (drops the map); a `--restart` flag.

### D64 An omitted `--jobs` asks the source; provenance is the CLI's fact
Absence recommends, a stated count is never lowered, zero is refused. The arrangement is announced
in two lines, before and after the open, because a fresh `.xz` walks footers first; the limit is
meant to be read once (`KD29`). Whether a number was typed or discovered never enters `Parallelism`.
Logged durations are diagnostics, never figures: one stderr subscriber, no terminal detection.

### D66 Output is byte-identical whether typing is on or off, but for a value its type cannot hold or an `escape` `bytea`
Every value renders back to the text `pg_dump` wrote, but one its type cannot hold, NULL in the null mode (D98), and an `escape` `bytea`, `hex` (I56), which `byteain` reads alike.
One contradicting its type is `FieldDecode` naming `--schema-mode strings`, never a null. Rejected: Arrow's display; a `bytea`'s form kept per database or flagged, read by no comparison.

### D67 `--json` is the internal struct; a flag's help is its doc comment
The whole cache file, keyed by block, no version of its own, compact and streamed, group values in:
a script sums its own rollup, a table spanning blocks (I2) with no merge rule but statistics' sums,
`--detail`'s alone. Coverage is one line at the top, nothing below qualified. No `help` attributes;
pages are snapshotted with width and bare-flag assertions. Rejected: `long_help` per flag; a rollup.

## Layering
### D68 Four layers, drawn where crate boundaries would go
L1 bytes and structure (`io`, `scan`, `copy`, `lex`, `map`, `index`, `preamble`, `cache`, `diagnostic`,
`statistics`), L2 PostgreSQL semantics (`pgtype`, `resolve`, `decode`, `nested`), L3 Arrow assembly
(`batch`), L4 query (`stream`, `predicate`, `leader`, `gather`, `prune`, `summary`); `error`, `lib` and
`instrument` in none; CLI and embedders above L4. `use` points down or sideways; a module gets a
layer before it is written (`tests/layering.rs`). Deviations, moved with a `batch` rework:
`read_table` (L4 work) and `QueryOptions::filter` naming `predicate::Expr`. Rejected: a split (D74).

### D74 L1 is Arrow-free and L2 is pure, so a metadata-only crate split off would compile no Arrow
L1 never names `arrow`; L2 names `arrow::datatypes` only, is synchronous and does no I/O; a decoder
takes an unescaped field and returns a value, never a builder (L3, whose views marry array building
to the read buffers, D46). Anything persisted is L1's vocabulary — declared type strings, never a
`DataType` or another L2 conclusion, a statistic's bounds included. A cross-layer trait is defined
below and implemented above (`statistics::BlockObserver`), as a scan predicate hook would be.

## Fixtures and tests (`scripts/`, `fixtures/`)
### D69 Fixtures are real `pg_dump` output on a pinned glibc image family
`postgres:<major>.<minor>-trixie`, exact minor, never `-alpine`: musl's `strcoll` is `strcmp`, so every text answer would be the `C`
answer. Four things move on regeneration (`\restrict`, `now()`, `--verbose` timestamps, OID drift) and nothing may assert on them.
Every `ColumnResolution` variant but `MetadataNotScanned`, which no scan produces, must come from a real fixture column (I36).

### D70 "Agrees with PostgreSQL" is a generated check
`fixtures/<major>/oracle/` commits the server's own answers, asked through two typed columns (a cast
folds constants and derives collation), every ordered pair, refusals recorded as `E<sqlstate>`,
nothing version-gated, text asked under `C` and `"default"` both; ICU is in one fixture and out of
the oracle. Semantics are the newest major's, and `oracle_differences.py` checks the union rule
(I35) by classing every cell moving between adjacent majors as additive or not (I37, I38, I42).

### D71 Register arms are parsed out of `pgtype.rs`
A `match` cannot be enumerated at run time; anchors turn a rewrite into a report. An arm is the finest closable unit, the join is
existence rather than branch coverage, and an exemption carries `Evidence(file, needle)` the check resolves.

### D73 Round trips, asserted shapes, and a hand-verified signal path
`Typed` against `Strings` over a fixture and `encode(decode(raw))` against on-disk bytes; boundary
values are unit tests; blind at `v_box_domain_array` (I22), pinned directly. A shape under test is
asserted (`is_seekable()`, a stated `--jobs`), never assumed. No test races a signal: `CancelsPast`
trips the flag at an offset, and koji covers the real scale. The round trip holds that render inverts
decode; that decode is right is the value oracle's (`oracle/values.tsv`): each typed node as the server
reads it, by send function or arithmetic, over every `types` flag set with DDL (`value_oracle.rs`).

### D92 The test build optimizes: the workspace at `opt-level = 1`, its dependencies at `2`
The fixture sweeps are CPU-bound in our own code, so optimizing dependencies alone leaves most of
their cost, and `dev` keeps debug assertions and overflow checks at any level. `release`, `bench`
and `profiling` do not inherit `dev`, so no figure moves. Rejected: tests under `--release`, which
drops both checks; moving a sweep out of the default suite (`pruning.rs`'s own refusal). Reopens: an
edit-to-test rebuild costing more than the sweeps save. Code: `Cargo.toml`. Evidence:
`docs/status/history/2026-09-25.md`, "M152: the test build optimizes".

### D102 The unrepresentable category is read off hand-written extremes through DataFusion's own path, never listed
`types`' `t_extremes` holds PostgreSQL's least, greatest and special values of each arm mapping to a type but `Utf8View`, one column an arm,
and `t_extremes_nested` the nested shapes, by hand, PostgreSQL keeping no catalog of them. Each decoded is formatted by `arrow-cast` and cast
strictly to `Utf8`, an error being a value its type cannot hold, recorded with its tier and held to D96's count tier by tier, so an upgrade
moving the calendar fails the check; `floor_mapping.py` holds columns and typed arms to each other per major, so a new major's typed mapping
forces its extremes (D38). Rejected: the list kept by review; a bound written per type (the list again); a decoder's refusal as the test,
which took `24:00:00`; a binary type's cast, which reads its bytes as UTF-8. Code: `arrow_holds`, `floor_mapping.py`'s `extremes_problems`.
Evidence: `every_extreme_is_held_by_arrow_or_recorded`.
