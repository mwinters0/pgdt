# Decisions

The decisions the code cannot explain: shapes kept for a deliverable not yet built, defaults chosen
over an alternative, and obvious changes measured or argued and refused. Nothing here says how the
code works (the named module does) or quotes a number (`measurements.md` does, cited by figure id;
so do the invariant registers, cited by `I<n>`/`RT<n>`). Cite an entry as
`docs/design/decisions.md`, "D12"; numbering and striking are `docs/process.md`, "The decision
register". **Capped at 550 lines**: an entry earns its place by being something a later session
would otherwise re-litigate, and adding one may mean striking one.

<!-- decision-watermark: D83 -->

## I/O, memory and parallelism (`io.rs`)
### D1 The library never spawns threads by surprise
`Parallelism::default()` is `Serial`; `discover_in` is opt-in and its one caller is the CLI, run on
purpose. Rejected: `default_workers` read in the library, so silence means concurrency.

### D2 A plain file recommends one worker; a compressed one the machine's cores
On every real device a serial plain scan is device-bound, and each partition reads a chunk-sized
tail past itself, so splitting adds device bytes. `XzSource` answers `available_parallelism()`
capped at its block count (`RT7`). Reopens: a parallel plain scan measured on a real device (both
parallel figures are warm-tmpfs). Evidence: `scan-throughput-*`, `parallel-scan-throughput`.

### D3 The memory constants, and what each one is
`MEMORY_RESERVE` is a subtraction, not a fraction, which under-reserves where being wrong kills the
process. `MEMORY_MARGIN_PERCENT` binds the resolved *count*, not the budget (a budget ceiling never
binds once `BufferPool::slots` clamps). `MEMORY_UNPOOLED_BOUND` is a bound off a grid, never a
per-reader term. `DEFAULT_MEMORY_BUDGET` stays small enough to decline block decode on an ordinary
`.xz`; clearing that gate picks one number for two questions. Evidence: `reserve`, `chunk-size`.

### D83 `--memory` states what the process may hold; one carving serves stated and discovered
A read-buffer budget is a number an operator cannot size a container from, so the flag states
resident and `Parallelism::within` carves it — reserve off the top, `margin_allowance` on the count
— with `discover_in` calling the same function, so provenance still never enters `Parallelism`
(D64). Consequences: a source recommending nothing is left on `DEFAULT_MEMORY_BUDGET` whatever is
stated, and the margin now binds a typed number. Rejected: a second flag; keeping the budget and
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
The same file is worth cutting for extraction and not for discovery, so the
caller decides. The scheduler's only refusal is a memory floor against the
file's remainder, erring serial (`KD22`). Rejected: `Anywhere` as "stay serial".

### D8 The cut is one unit wide, sized apart from the charge
`BOUNDARIED_PARTITION_UNITS = 1`; `window_end` sizes the cut, `partition_bytes` the charge. A wider
cut wins at two stated readers and loses at the flagless default. Rejected: deriving width from the
charge. Reopens: an explanation of the flagless collapse.

### D9 Pool sizing constants
`hint_read_size` *becomes* the slot size and larger buffers are dropped on release: a chunk read and
`attach_text`'s one coalesced read are indistinguishable by length. `POOL_DEPTH` is the replay
retention depth, which only a block pool raises to `--jobs`. `PLAIN_PARTITION_CHUNKS` caps tail-read
waste and is not derived from `POOL_MAX_BYTES`. Evidence: `chunk-size`.

### D10 mmap, `fadvise` and double-buffered readahead are refused
Positioned reads via `spawn_blocking`, one chunk at a time: no overlap scheme puts a cold scan below
the device's delivery time, which the scan already sits within a few percent of. mmap also bypasses
the trait, faults uninterruptibly and errors as `SIGBUS`. Buffers are pooled and the `Bytes` sliced
(`vec![0; len]` is `calloc`). `DEFAULT_CHUNK_SIZE` is shipped, not probed: rotational detection is
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
No `#[global_allocator]` in the library; `pgdq` links the platform allocator, `jemalloc`/`mimalloc`
are opt-in features, `--version` names which. Rejected: `mimalloc` on a few percent, making every
table a figure of an unshipped binary; `mallopt(M_ARENA_MAX)`, binding if set at resolution (RT10)
but saving only below the count arenas already follow (D12), on `.xz` alone, at unpriced contention,
overwriting the operator's `MALLOC_ARENA_MAX`. Reopens: a contention figure. Evidence: `allocator`.

## The compressed source and the cache (`io.rs`, `cache.rs`)
### D14 `.xz` is read; recognition sniffs content
An `.xz` dump is third-party post-compression (input reach); gzip/zstd are `pg_dump --compress`
output (compatibility). `open_local` reads magic bytes and sits outside the trait. Rejected:
extension dispatch. `xz-seek` is a vendored read-only copy (`scripts/vendor_xz_seek.py`); a bug is
fixed upstream. Rejected: a path dependency, which fails `cargo check` without the sibling checkout.

### D15 A read decodes whole blocks and retains them; streaming is the fallback
Whole-block decode, LRU-retained. The mutexed streaming reader stays for a file whose largest block
the budget cannot hold (plain `xz bigfile` is single-block), and there `partitions()` answers one
partition, two readers forcing each other's restarts. Decodes read through a `File`, not a `Window`
(larger than the chunk it spares; reopens with ranged GETs). Two misses on one block decode it twice
(`KD20`): an in-flight map would lock the common case to spare a boundary collision.

### D16 Block decode is afforded out of the stated budget, keyed on largest block
`BlockCache::affordable` compares the charge at one reader (unit, chunk, decoder
footprint, and the retention list's `POOL_DEPTH − 1` units) against the caller's number,
from file-wide inputs. Rejected: a fixed refusal line; keying on block count. Evidence: `reserve`.

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
A decline is a property of the file *and this run's budget*, which no persisted `DiagnosticKind`
can be. A one-block file opens and raises `NonSeekableCompressedSource` naming `xz -T0`; only the
user can judge whether one decode-from-zero is worth waiting for. A statistics skip is a note too,
stated at zero wherever a block's statistics were consulted, the one answer to why a filter read
everything. Rejected: omitting a zero; counting only believed filtered columns. Reopens: a user
puzzled by a zero, which naming the filter's columns lacking a usable statistic would answer.

### D20 The library never replaces cache data automatically
A cache recording another file's stored size is refused before a byte is read: at every scan entry
point as `Error::CacheSourceMismatch`, in `cache::claim` as `CacheClaim::SourceChanged`; the other
unusable statuses start cold. Rejected: a `--force` override (set once in a script, never
reconsidered); the guard inside `cache::save` (policy an embedder cannot override). The CLI words
both refusals with one tail. A back-fill meeting a block that no longer ends where the map says is
`CachedBlockChanged`: stored, its statistics contradict the offsets beside them; skipped, the
back-fill goes on for a file it knows was rewritten; re-mapped, cache data is replaced unasked.

### D21 Identity is `stored_size()` plus a weak mtime, in an opaque enum
`stored_size()` keeps the check a `stat` where `size()` needs a decompressing source opened first.
An mtime mismatch is `CacheMtimeChanged`, never persisted. `SourceIdentity` is matched through its
variant because the next source has an ETag. `CompressionIndex` is a sibling of `ContainerKind`,
whose `Plain` is honest for a compressed source; `total_size` is its own field.

### D22 `CacheLoad` is its own type and `FORMAT_VERSION` is bumped freely
`Incomplete` is usable (or `map_forward` restarts from zero) and `Disabled` is about the caller;
every entry point spells the outcomes out. Bump on any persisted reshape, record it nowhere.

## The scanner (`scan.rs`, `copy.rs`)
### D23 The scanner never owns the bytes it scans
`CopyScanner` is a synchronous state machine over a caller-owned buffer; the only memory bound is
`max_line_bytes`, and exceeding it errors, never truncates. A chunk is scanned in two passes,
carried line then chunk in place (`ChunkCarry`); a growing buffer copied every byte twice.

### D24 Parser robustness requirements (hardcoded)
Only a line matching the whole `COPY … FROM stdin;` grammar is structural; inside a block only an
exact `\.` line is looked at (I7). An off-grammar `COPY` line is ordinary SQL. A dollar-quoted
region emits no lines, only a closing offset, since a body can match the grammar by coincidence
(I1). A bare `BEGIN;`/`COMMIT;` pair is the large-object region, skipped unread (I12).

### D25 Parallelize what is CPU-bound
Decode is always split, extraction on any source, discovery only behind a decoder; the worker is
fused. One core's rate against the device's offer decides each; readings that disagree reopen it.
Evidence: `xz-decode-scaling`, `parallel-scan-throughput`, `scan-throughput-*`.

### D26 Cancellation is per chunk or leader window, honoured by the mapping passes alone
A block can be hundreds of gigabytes, so block-boundary cancellation is a hang; `map_file`'s scan
and back-fill are the drivers with a partial result to keep. The preamble scan ignores the flag: a
stop there is indistinguishable from reaching the first `COPY` header and would cache as complete.

### D27 UTF-8 is validated once per chunk, and no hot path uses `unsafe`
`validated_prefix` validates the largest line-terminated prefix and fields slice the `&str` with
`str::get`, delimiters being ASCII. The escaped path still validates (`\xNN` synthesizes bytes); the
bulk pass runs only when a row will decode something. Four `unsafe` attempts lost to the safe shape.

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
`nested-decode-micro`, `map-only`, `census-attribution`, `cross-file-floor`, `nested-end-to-end`.

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
never seeded. The boundary predicate refuses `"Data for "` and accepts `"Statistics for "`, which
matters under `--disable-triggers` (I31).

### D32 Boundary rules that read as bugs
A blank line does not close a pending comment (`_printTocEntry` writes `--\n\n`); `scan_preamble`
retreats to `pending_comment_start()` rather than guessing the comment's kind.

### D33 Bulk regions are one span kind; large objects skip at the scanner, `INSERT` runs at the map
Only `DataBlock::Copy` has inner offsets, only `COPY` having a row reader; a data span absorbs its
TOC comment unconditionally. `INSERT` runs fold in `feed_line` with no scanner state (their
boundaries rest on no line-anchored invariant) and the run's end is string-aware. Two cuts stay
untaken until the `INSERT` row reader exists (`KD9`).

### D34 `DumpIndex` stores no fact twice but sortedness; whole-file facts need `is_complete`
`blocks()` is filtered, `metadata` computed once, diagnostics never persisted, roles excepted; a
streamed schema commits over the blocks it replays, ungated (I2). Statistics sit in their block
behind an `Arc`, so save-gate clones copy a reference (`KD5`), and store sortedness; a group is a
byte range, no piece knowing a global row index. A back-fill keeps a block's columns, and its size
unless a stated bound it did not record moves it, a maximum it breaks re-reading it once. Rejected:
`SparseRowIndex`; padded `character` bounds and entries, keyed alike but past the cap; a back-fill
narrowed, dropping only re-read blocks' columns.

### D35 The census is type-blind, records both dimension bounds, and always runs
`ArrayShape::observe` reads the leading brace run off still-escaped bytes at L1 (I15, I25); min and
max depth, or `{1,2}` beside `{{1,2}}` resolves to a wrong `List`. Every mapping pass censuses:
gating on a full scan left early blocks uncensused. Evidence: `census-brace-free`, `census-arrays`.

### D36 The preamble grammar dispatches on fixed keywords and never guesses
Unrecognized lines are ignored, so `--binary-upgrade` noise is free (I5, I6). `record_type` keys on
name (I11); a composite's field list is all-or-nothing, `record_out` being positional (I23); a
`--create` dump's pre-`\connect` segment is not a database (I9). L1 stores text, never a conclusion:
declared types and collation clauses are verbatim, `None` collation is "no clause" (I37), and
`CollationDef` keeps only `deterministic` (I42).

## Type resolution and decoders (`pgtype.rs`, `resolve.rs`, `decode.rs`, `nested.rs`)
### D37 The bar: the dump alone determines the value
A declared type maps to a real Arrow type only if its text round-trips consulting nothing outside
the file; otherwise `Utf8View` with a note naming the kind of unknown. Misreading is unrecoverable,
not recognizing is not; `money` fails it (`KD13`). Every field is nullable regardless of DDL.

### D38 The ADBC driver's shipped release is a floor, swept from the catalog
Where the driver yields a real Arrow type, ours is never wider; the floor is a pinned release, never
`main`, one row per declarable `pg_catalog` type. `status`/`extension` columns take a row out of the
rule by themselves, and only what they cannot say gets a `Disposition` with a stance and a resolving
citation. `floor_mapping.py` compares value space (`Utf8View` is `string`) and reports an unknown
arm, never guessing. Rejected: the `typelem` shape test, which deletes `int2vector` (I8, I39).

### D39 `NestedPlan` travels beside the `DataType`, with one producer
An Arrow type does not name its literal (`int4range[]`, `int4multirange`: both `List<Struct>`);
every builder takes a plan. Rejected: widening `builtin_scalar`'s tuple; `Field` metadata.

### D40 The comparison is decided per declared type, in the same arm
`builtin_scalar` answers Arrow type and `CompareKind` both, keyed on declared type and `COLLATE`;
six declared types share `Utf8View` under six comparisons. `predicate.rs` never reads the Arrow type.

### D41 Array shapes: two refusals off one domain walk, six spellings to one level
An opaque element delimiter (I22) and an array element (I26) both resolve `Utf8View`, decided in
`domain_terminal`. All `Typename` spellings collapse to element plus one level (I21, I28);
normalizing on parse would edit the user's DDL. Containers recurse unguarded (I24): `KD3`, `KD4`.

### D42 `interval` is the struct; special values are decode failures
`MonthDayNano` is PostgreSQL's three fields, so text would be below the floor; infinities and
out-of-range parts are `FieldDecode`, as for `date` and `numeric` (`KD8`). Twelve built-in range
names are fixed, multiranges apart (I10); a `canonical` function makes a range unanswerable (I46).

### D43 The census speaks after the DDL and moves the pair
`retype_from_census` alone changes `(DataType, NestedPlan)` after resolution, a parameter so no
caller skips it; a run past `MAXDIM` is tested first and kept optimistic (I25). `MetadataNotScanned`
refuses a stream and degrades a listing: same output as `NotDeclared`, opposite advice.

### D44 A decoder allocates only where its return type does; renderers use tables
No scalar decoder takes an intermediate `String`; renderers write into one reused buffer through
`DEC_PAIRS`/`HEX_PAIRS`, never `core::fmt` (`tests/render_allocations.rs` pins it). Render-back's
third outcome, `FieldRender`, is an Arrow value no text form spells (I40), never rounded.

### D45 One quoted-token scanner; strict decoders apart from permissive parsers
Array, record and range literals share one scanner parameterized four ways (I20). `decode_*` reads
what a dump holds and cannot loosen (the render inverse); `parse_*` reads what a user typed and
cannot tighten; each transcribes the newest major and under-accepts (I35, I44). Wrong depth,
`[lb:ub]=` or a field-count mismatch is refused, never reshaped. Evidence: `nested-decode-micro`.

## Batches, streams and the leader (`batch.rs`, `stream.rs`, `leader.rs`)
### D46 Zero-copy views are top-level `Utf8View` only
Every other arm copies. A retained chunk is released only past its *last* byte
(the carried row arrives inside the next chunk) and `invalidate_block_cache`
runs on every flush keeping its batcher; any new flush trigger must honour it.

### D47 `max_source_span` is the only trigger that bounds pinned bytes
`max_rows` and `max_bytes` count selected rows, which a filter makes sparse.
The span term is charged only where the source retains by chunk (`KD23`).
Rejected: compacting views past a selectivity threshold. Evidence: `parallel-peak-rss`.

### D48 Mapping and replay are separate passes, and `splice` owns the seam
The map is never behind the rows, so a `ResumeToken` points inside mapped territory. A segment is
spliced by extending the *preceding* span; a start floor on `Builder` made assembly visible in the
map. A cancelled mapping pass fails a query rather than shortening it (I1). Cached replay and the
cold interior split share `worker_count` and `cut` and differ only in what they cut.

### D49 One target per query, and the early stop is conservative
Name matches narrow to one `(database, table)` before replay or `AmbiguousTable`; `target_settled`
vetoes a stop on a partition root (I2) or any `\connect`. A conflict past the stop is unseen (`KD6`).

### D50 `ResumeToken` is opaque and fingerprints the query
Table, projection, filter tree, schema mode and partition, hashed by explicit match (a derived
`Hash` silently stops covering a new operator). `database`, `scan_extent`, the batching knobs and
`use_statistics` are outside it, a skipped group holding no row a token resumes past.

### D51 A segment's offsets are search bounds, and a resync is a real read
The first row is past the first LF at or after `start`; the piece runs to the first LF at or after
`limit`, so a cut at a row's first byte leaves it to the earlier piece. The scanner is never started
mid-row, since a value can end in `\.` (I7 covers line starts). The first piece reaches back over
the header, as a pruned block's run of group 0 does. Sub-streams are contiguous byte-balanced runs
in file order, so concatenating them *is* the serial replay.

### D52 The worker is fused, and the earliest failing piece is the error
A piece reads its range and parses each read in `spawn_blocking`; `scan_piece` does no I/O.
Rejected: decode and parse pools over a channel (the ratio is a property of the command);
speculative splitting; cross-block pipelining. Pieces drain in file order so the same truncated file
names the same byte. `close_copy_block` has one body and two callers, or a parallel cache stops
matching a serial one's. A failure only reading finds is the lowest-indexed failed sub-stream's,
after the rows before it; a resolution refusal comes from the plan (D54).

## Predicates (`predicate.rs`, `where_expr.rs`)
### D53 The operator set is closed
No `LIKE` (collation-dependent folding), `IN` (`Or`), `BETWEEN` (`And`), or
column-to-column. `IS [NOT] DISTINCT FROM` is what three-valued logic forces.

### D54 One tree, no planner, short-circuit defined against the root
`filter` is one n-ary `Expr`, by default the empty conjunction. `And` may stop at the first
`Unknown` except beneath `Not`, since only the root's `True` matters; a decode failure surfaces only
where evaluation reaches it, never in a row group statistics rule out or past a sorted block's
stopping row, neither read (`prune.rs`). Resolution refusals come from the plan before any row, for
the first refusing block in file order, walking leaves the evaluator would skip; a block with no
column list refuses where reached. Rejected: DNF; exact Kleene everywhere; not skipping a group
holding an unkeyed value (a nested column, `KD2`'s, is never keyed) or under a term naming one.

### D55 A literal is read in the type's `*_out` form and no wider
`*_in` spellings `*_out` never writes are `PredicateValueDecode`; the remedy is the user's. `jsonb`
(its canonical form is untypeable), hex digits of either case, and integers (`str::parse`, field and
literal alike) are wider; `boolean` deliberately not. A literal finer than the column's scale is
refused, not rounded. `JSONB_MAX_DEPTH` is fixed because a Rust stack overflow aborts.

### D56 Special values are a rank in the key
`infinity`/`NaN` are their position in PostgreSQL's order; `OrderKey` derives
no `Ord`. Rejected: excluding the row like a NULL, which has no order (I33, I34).

### D57 Equality has three canonicalizations, chosen by injectivity of `*_out`
Render the literal once, trim the field per row (`bpchar`), or decode both per row; `v=1.5` must hit
a `numeric(10,2)` written `1.50`. Rejected: rendering `timetz` and `inet` once too (I33, I38, I41).

### D58 A nested column has one comparison path, and the leaf grammar does not widen
Structural key walk for both operator families; `nested_key`'s `input` flag stops at the container
but for `int2vector`, whose elements `int2vectorin` reads. Both sides of a range go through
`make_range`, keyed on the range type; a user range declaring `canonical` is refused under every
comparing operator, the NULL tests reading no value (I44–I47).

### D59 Divergence is per term and per position, on its own channel
`ComparisonNote` carries a path and the declared type there, reported by
`TableStream::comparison_notes`: operator-conditional, so L4. Only the `json`
arm announces its bytewise `=` under a container (I45). See `KD7`, `KD10`.

### D60 `--where` is a second flag and the tokenizer defines the refusal set
`refuse_where_structure` refuses any `--filter` term that does not tokenize to one leaf, so a string
both flags accept means one thing. A keyword needs whitespace or a paren on both sides and `NOT`
after `is` stays in the term. A term splits at the earliest operator outside quotes, longest first;
quotes are stripped in `--filter` and nowhere else. Rejected: `&&`/`||`; backslash escaping.

## Statistics (`statistics.rs`, `gather.rs`, `prune.rs`)
### D75 Pruning takes only what each operator family proves
Bounds answer the ordering operators, and under an equality operator rule out "equal" but never
"unequal"; a dictionary answers the equality operators; a stop needs an ordering term the root
conjunction requires, on a column sorted the way its bound closes; terms combine as if independent.
Rejected: bounds proving "unequal", `Canonical`/`Trimmed` comparing spellings, one to a key only in
text `*_out` wrote; and, though sound, a dictionary answering ordering, or a stop under `=` or `NOT`
over the opposite operator. Reopens: a filter shape a reading shows common. Code:
`ResolvedTerm::truths`, `prune::SortedStop`. Evidence: `tests/pruning.rs`'s generated check.

### D76 Gathering never keys a bytewise value; a bound past the cap is a prefix and a successor
`text`, `character` and `bytea` order by a clipped head (`gather::Clipped`), not an `OrderKey`: a
key copies the whole value, and a row may run to `--max-line-bytes`. Values agreeing on the head
share every byte a stored bound reads, so bounds stay valid and only row order is lost (`Unsorted`),
as for a decoded-kind value past the cap. A truncated text `max` replaces its last character by the
next scalar value (`text_upper`), `bytea`'s increments its decoded bytes. Rejected: the byte
successor for text, which can end mid-character. Evidence: `gather.rs`'s
`a_long_texts_stored_bounds_are_on_the_right_side_of_it`.

### D77 The statistics request is the mapping pass's argument, and gathering nothing is a selection
`map_file` takes a `&StatisticsRequest` and no query entry point does, so no option a query is
handed reads as a request it ignores. `StatisticsSelection::None` is a variant, so the type's
`Default` gathers everything. Rejected: a `ScanOptions` field; `Option<&StatisticsRequest>`, whose
`None` either contradicts that default or spells it twice; refusing a size beside `NONE` in the
library, which closes one instance of a size sizing nothing (the CLI refuses it, a person's intent
being ambiguous there). Code: `statistics::StatisticsRequest`.

### D78 Statistics share the cache file, its identity and its `FORMAT_VERSION`
None is believed from a cache sized unlike the live source (D20); the mtime stays advisory (D21), so
a same-size rewrite prunes against its predecessor's, knowingly. Bounds, order and dictionary hold
only under the declared type and `COLLATE` recorded with them, and changing how a kind orders or
equates bumps the version. Both ends stream; a save renames its own file over the cache, so no copy
meets the statistics and a kill keeps the last save, orphaning its file. Rejected: a separable
statistics file; its own semantics version; the copy charged; one beside name two saves share.
Code: `prune::prune_block`, `cache::save`. Evidence: `golden_order_is_pinned_to_the_format_version`.

### D79 Bounds go where a comparison orders exactly, a dictionary where it equates exactly
Bounds and row order need the key a filter compares with, so a divergent comparison gets neither; a
dictionary needs equality alone, reaching `KD7`'s text. Nested columns get neither. A value that
does not key leaves its group without bounds and its block `Unsorted`, or a bound would not cover
its row. A dictionary is never an Arrow encoding, which would change a query's schema between a cold
and a warm cache (D37). Rejected: a distinct count. Code: `gather::ColumnGatherer::new`.

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

### D82 A block merges pairwise, exactly, between its minimum and its maximum, never while a piece lives
Exact, so serial, split and stated agree: a bytewise closed group keeps its extremes' heads till
`finish` clips them, a clipped upper bound not ordering as its value does; a merged dictionary
renumbers first-seen. A stated maximum stops the merge first and lifts the cap. A piece joins at
its own size, so a block waits till none lives, reopening an odd last group. Rejected: merging
stored bounds; coarsening a piece at its join; an even count, which windows keep odd; the
nearest-rank median, not monotone under the cap. Code: `Gatherer::fit_cap`.

## The CLI (`main.rs`, `error.rs`)
### D61 `info` never scans
The surprise is that a scan happened at all; `parse` scans ahead, a `query` maps only what the cache
lacks (D48), and `--preamble-only` hangs off `parse`. Rejected: a scanning fallback; `--no-scan`.

### D62 The save throttle is a ratio, and one gate opens save and splice
A save is skipped unless `K` times the last save's duration has elapsed, which bounds overhead in
every regime with no constant to tune. The map is rebuilt at the gate's openings, not every
`CopyEnd`; the third opener matches the queried header, a superset of `target_settled`. Rejected:
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

### D66 Output is byte-identical whether typing is on or off
Every value renders back to the text `pg_dump` wrote. One contradicting its type is `FieldDecode`
with an offset naming `--schema-mode strings`, never a null. Rejected: Arrow's display formatting.

### D67 `--json` is the internal struct; a flag's help is its doc comment
The whole cache file, keyed by block, no version of its own, compact and streamed, group values in:
a script sums its own rollup, a table spanning blocks (I2) with no merge rule but statistics' sums,
`--detail`'s alone. Coverage is one line at the top, nothing below qualified. No `help` attributes;
pages are snapshotted with width and bare-flag assertions. Rejected: `long_help` per flag; a rollup.

## Layering
### D68 Four layers, drawn where crate boundaries would go
L1 bytes and structure (`io`, `scan`, `copy`, `map`, `index`, `preamble`, `cache`, `diagnostic`,
`statistics`), L2 PostgreSQL semantics (`pgtype`, `resolve`, `decode`, `nested`), L3 Arrow assembly
(`batch`), L4 query (`stream`, `predicate`, `leader`, `gather`, `prune`); `error`, `lib` in none;
CLI and embedders above L4. `use` points down or sideways; a module gets a layer before it is
written (`tests/layering.rs`). Deviations, moved only with a `batch` rework: `read_table` (L4 work)
and `QueryOptions::filter` naming `predicate::Expr`. Rejected: a crate split (D74).

### D74 L1 is Arrow-free and L2 is pure, so a metadata-only crate split off would compile no Arrow
L1 never names `arrow`; L2 names `arrow::datatypes` only, is synchronous and does no I/O; a decoder
takes an unescaped field and returns a value, never a builder (L3, whose views marry array building
to the read buffers, D46). Anything persisted is L1's vocabulary — declared type strings, never a
`DataType` or another L2 conclusion, a statistic's bounds included. A cross-layer trait is defined
below and implemented above (`statistics::BlockObserver`), as a scan predicate hook would be.

## Fixtures and tests (`scripts/`, `fixtures/`)
### D69 Fixtures are real `pg_dump` output on a pinned glibc image family
`postgres:<major>.<minor>-trixie`, exact minor, never `-alpine`: musl's `strcoll` is `strcmp`, so
every text answer would be the `C` answer. Four things move on regeneration (`\restrict`, `now()`,
`--verbose` timestamps, OID drift) and nothing may assert on them. Every `ColumnResolution` variant
must come from a real fixture column (I36).

### D70 "Agrees with PostgreSQL" is a generated check
`fixtures/<major>/oracle/` commits the server's own answers, asked through two typed columns (a cast
folds constants and derives collation), every ordered pair, refusals recorded as `E<sqlstate>`,
nothing version-gated, text asked under `C` and `"default"` both; ICU is in one fixture and out of
the oracle. Semantics are the newest major's, and `oracle_differences.py` checks the union rule
(I35) by classing every cell moving between adjacent majors as additive or not (I37, I38, I42).

### D71 Register arms are parsed out of `pgtype.rs`
A `match` cannot be enumerated at run time; anchors turn a rewrite into a report. An arm is the
finest closable unit, the join is existence rather than branch coverage, and an exemption carries
`Evidence(file, needle)` the check resolves.

### D73 Round trips, asserted shapes, and a hand-verified signal path
`Typed` against `Strings` over a fixture and `encode(decode(raw))` against on-disk bytes; boundary
values are unit tests; blind at `v_box_domain_array` (I22), pinned directly. A shape under test is
asserted (`is_seekable()`, a stated `--jobs`), never assumed. No test races a signal: `CancelsPast`
trips the flag at an offset, and koji covers the real scale.
