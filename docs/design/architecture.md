# Architecture

How `pgdump_query` works, filed by subject. This is the reference for *how the
built system behaves and why it is built that way* — read the section for the
mechanism you are touching, not the whole file.

What this doc is not: it does not track what is built versus not
(`../status/STATUS.md`), it does not hold the rules for where new code goes
([`layering.md`](layering.md)), it does not carry the user-facing story
(`../manual/`), and it does not prove the `pg_dump` behaviours the design leans
on ([`postgres-invariants.md`](postgres-invariants.md), whose `I<n>` numbers
this doc cites throughout). Work still ahead is in
[`roadmap.md`](roadmap.md).

Sections carry their **rejected alternatives** inline, marked *Rejected:*.
Those are the paragraphs that cannot be recovered from the code, because the
code records only what was built.

They also carry that mechanism's **known deficiencies**, each opening with a
`<!-- deficiency: KD<k> -->` marker and indexed one line apiece in
`../status/STATUS.md` ("Known deficiencies"). The detail lives here rather than
there so that a session reading this section before changing the mechanism
cannot miss it; `cd scripts && uv run deficiencies.py` is what keeps the two
halves resolving to each other.

## Contents

Jump to the mechanism you are changing; there is no need to read the file
through.

| If you are touching… | Read |
|---|---|
| the async/IO trait, batch sizing, push vs. pull | [Execution model and API surface](#execution-model-and-api-surface) |
| `.xz` input, source recognition, `XzSource`, the seek table | [The compressed source](#the-compressed-source) |
| `scan.rs`, `copy.rs`, a new `Event` variant, a read loop's buffer, how a field's bytes become a `str` | [Bytes and structure](#bytes-and-structure) |
| `map.rs`, spans, tiling, TOC headers, `INSERT`/large-object regions | [The file map](#the-file-map) |
| `index.rs`, what the index owns, diagnostics | [`DumpIndex`: one owner per fact](#dumpindex-one-owner-per-fact) |
| array dimensionality, `ArrayShape`, what a scan records per row and what retypes a column from it | [The array shape census](#the-array-shape-census) |
| `preamble.rs`, the DDL grammar, `\connect` handling | [The preamble grammar and `DumpMetadata`](#the-preamble-grammar-and-dumpmetadata) |
| `pgtype.rs`, `resolve.rs`, the type mapping table | [Type resolution](#type-resolution) |
| whether a mapping is at or above the ADBC driver's, a floor stance | [The floor: the ADBC driver's answer bounds ours](#the-floor-the-adbc-drivers-answer-bounds-ours) |
| `decode.rs`, a new type's decode/render pair | [Decoders and render-back](#decoders-and-render-back) |
| `batch.rs`, the zero-copy `Utf8View` path, a batch's flush triggers | [Arrow assembly and the zero-copy path](#arrow-assembly-and-the-zero-copy-path) |
| `stream.rs`, `map_forward`/`map_file`, replay, resume, projection, predicates, the `--filter` term grammar, the `--where` expression grammar | [Query: mapping and streaming are separate passes](#query-mapping-and-streaming-are-separate-passes) |
| where a scan's time actually goes, before proposing to make one faster | [Where a scan's time goes](#where-a-scans-time-goes) |
| `alloc.rs`, a `#[global_allocator]`, what a figure's apparatus line names | [The allocator is the binary's choice](#the-allocator-is-the-binarys-choice) |
| `cache.rs`, the format version, cache modes | [The cache](#the-cache) |
| the CLI's flags or output, the save throttle, the interrupt guard | [CLI surface](#cli-surface) |
| `scripts/`, a new fixture schema | [Fixtures](#fixtures) |
| what PostgreSQL answers for a comparison, a new comparison case | [The comparison oracle](#the-comparison-oracle) |
| whether two majors disagree, the committed differences file | [The cross-major differ](#the-cross-major-differ) |
| a comparison-register arm, whether the oracle still covers one | [The register-to-oracle reconciliation](#the-register-to-oracle-reconciliation) |
| a comparison's *answer*, or the set of cases it is knowingly wrong on | [The register against the oracle's answers](#the-register-against-the-oracles-answers) |
| what the ADBC driver returns for a type, the committed floor sweep | [The ADBC floor oracle](#the-adbc-floor-oracle) |
| adding or changing a test | [Testing philosophy](#testing-philosophy) |

## Module map

| Concern | Where | Layer |
|---|---|---|
| Byte-range I/O trait, the local-file and `.xz` implementations, and `open_local`'s content sniffing | `pgdump_query/src/io.rs` | L1 |
| `COPY` block and large-object structure discovery, and the read loops' chunk carry (`ChunkCarry`) | `pgdump_query/src/scan.rs` | L1 |
| COPY TEXT field splitting / escaping / unescaping, the split a row's consumers share (`RowSplit`), and the bulk UTF-8 pass a row is decoded off (`RawRow`, `field_ranges`, `validated_prefix`) | `pgdump_query/src/copy.rs` | L1 |
| `Span`/`SpanBody`/`DataBlock`/`TocHeader`, the boundary+classification state machine (`Builder`), `check_tiling`, `attach_text` | `pgdump_query/src/map.rs` | L1 |
| DDL statement grammar: `classify_statement`, `statement_complete`, `in_open_quote`, `extract_statement_cross_refs`, `dump_metadata_from_spans`; `DumpMetadata` and friends | `pgdump_query/src/preamble.rs` | L1 |
| `DumpIndex`, `CopyBlock`, `build_index`/`scan_preamble`/`preamble_only` | `pgdump_query/src/index.rs` | L1 |
| Structure cache (envelope, `CacheMode`, `CacheStatus`, source identity) | `pgdump_query/src/cache.rs` | L1 |
| `Diagnostic`/`DiagnosticKind`/`Severity` — the file-level channel | `pgdump_query/src/diagnostic.rs` | L1 |
| Declared-type string → Arrow `DataType`; domain/enum/range/multirange resolution; `NestedPlan`; the comparison register (`comparison_for`, `ComparisonPlan`) | `pgdump_query/src/pgtype.rs` | L2 |
| `ResolvedSchema`/`ColumnResolution`/`ColumnNote` — joins a `COPY` header against `DumpMetadata`, and carries each column's comparison plan | `pgdump_query/src/resolve.rs` | L2 |
| Per-type field decode + render-back | `pgdump_query/src/decode.rs` | L2 |
| Array / record / range / multirange literal decode + render-back, and the `*_in` supersets a filter literal is read with | `pgdump_query/src/nested.rs` | L2 |
| Arrow batch assembly (`ColumnBuilder`, `RowBatcher`), push-mode `read_table` | `pgdump_query/src/batch.rs` | L3 |
| Pull-mode `table_stream`, `map_forward`, `map_file` (`pgdq parse`'s scan), replay, `ResumeToken`, `ScanExtent` | `pgdump_query/src/stream.rs` | L4 |
| Post-parse predicate | `pgdump_query/src/predicate.rs` | L4 |
| CLI (`pgdq parse` / `info` / `query`), the `--filter` term grammar | `pgdump_query-cli/src/main.rs` | above L4 |
| The `--where` expression grammar | `pgdump_query-cli/src/where_expr.rs` | above L4 |
| Which allocator the binary links, and `--version`'s report of it | `pgdump_query-cli/src/alloc.rs` | above L4 |

`error.rs` and `lib.rs` are cross-cutting and belong to no layer. Which layer a
module is in constrains what it may depend on and what it may know:
[`layering.md`](layering.md), which also records the two known deviations and
the greps that enforce the rest.

Integration tests mirror the split: `tests/{scan,map,preamble,pgtype,decode,nested,batch,stream,cache,query_cache}.rs`,
plus an `insta` snapshot of the whole event stream over `tests/data/edge_cases.sql`.

## Execution model and API surface

**Async core on `tokio`, with only the I/O layer async.** The parsing and
scanning logic — finding `COPY` boundaries, splitting rows, unescaping — is
synchronous CPU-bound code operating on bytes already read.

The library defines its own minimal internal trait for byte-range reads:

```rust
trait ByteRangeSource: Send + Sync {
    fn read_range(&self, offset: u64, len: usize)
        -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>>;
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>>;
    fn modified(&self)
        -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>>;

    // defaulted, and each one exists for a source the local file is not
    fn stored_size(&self) -> …          { self.size() }
    fn size_is_exact(&self) -> bool     { true }
    fn hint_read_size(&self, _len: usize) {}
    fn seek_table(&self) -> Option<xz_seek::SeekTable> { None }
}
```

`read_range`/`size`/`modified` are shaped to match `object_store`'s
`get_range`/`head` semantics **on purpose**, so an `object_store`-backed
implementation is a drop-in addition rather than a redesign. Two
implementations exist: a local-file backend (`std::fs::File::read_at` wrapped
in `spawn_blocking`, since `tokio` has no native async positioned-read) and
`XzSource`, which decodes an `.xz` file beneath it ("The compressed source"
below).

*Rejected:* depending on the `object_store` crate now. It pulls in a cloud-SDK
dependency tree that buys nothing for "read one local file". The trait shape is
what keeps the addition additive; it arrives behind a default-off Cargo feature.

**The trait is dyn-compatible, and that is what keeps a second source from
squaring every caller.** Each method returns `Pin<Box<dyn Future + Send + '_>>`
rather than `impl Future`, so every consumer — `scan`, `build_index`,
`scan_preamble`, `preamble_only`, `map_forward`, `map_file`, `table_stream`,
`attach_text`, `cache::load`/`save` — takes `&dyn ByteRangeSource` instead of a
type parameter, and the CLI holds an `Arc<dyn ByteRangeSource>`. The cost is one
allocation per `read_range`, which is one per chunk and noise beside the read
itself; what it buys is that a *wrapping* source composes with whatever it wraps
instead of multiplying the branch at every call site. `object_store`'s own trait
is dyn-safe for the same reason, so this increases the mirroring rather than
departing from it.

*Rejected:* staying generic, with a per-command `match` over the source kind.
Zero runtime cost and no API change, but it pushes the same combinatorial branch
onto every embedder and every test, and it grows with each source added rather
than being paid once. *Also rejected:* an `enum AnySource` — it keeps static
dispatch and one code path, at the price of L1 enumerating every source this
project will ever have, the recursive wrapping case included.

**The borrow form covers the library; the owned form is what a factory
forces.** Converting every generic consumer needed `&dyn ByteRangeSource` and
nothing more — each one already held a plain reference, none an owned handle
crossing a spawn boundary — so `Arc<dyn ByteRangeSource>` earned its keep only
where recognition has to *return* a source whose type is decided at run time,
which is what the CLI holds. Worth knowing when a similar rework is costed
elsewhere: the two forms are not a matter of taste, and which one is needed is
decided by whether anything constructs a source it cannot name.

**Three of the four defaulted methods exist for a source the local file is
not.** `stored_size()` is the bytes as stored on the device where `size()` is
the addressable length — equal for a plain file, divergent for a decompressing
one, and it is what the cache's staleness check reads, so that check stays a
`stat` rather than a stream-index walk ("The cache"). `size_is_exact()` says
whether `size()` is exact rather than a bound; everything implemented so far
answers `true` honestly and nothing reads it yet — it is carried for a codec
that cannot answer exactly (gzip's `ISIZE` is useless above 4 GiB, zstd's frame
content size is optional), added in the pass that reshaped these signatures
because a later pass would have had to touch them all again for one bool.
`seek_table()` hands a compressed source's block index to the cache without the
cache knowing what kind of source it holds. `hint_read_size` is the fourth and
is about the caller rather than the source; it is described below.

**The local backend pools its read buffers, and the trait shape is why.**
`read_range` returns owned `Bytes` because `get_range` does, so the obvious
implementation allocates one buffer per chunk — and `vec![0u8; len]` is
`calloc`, which zeroes a megabyte that `read_exact_at` overwrites a microsecond
later. That memset was **23.2% of a warm `parse`'s user time** on the 3.00 GiB
control ("parse-profile"), and freeing the region per chunk is separately what
put `jemalloc` at 3,161 `madvise` calls against glibc's 50 over the same file. So
`LocalFileSource` keeps a small free list: a read takes the smallest buffer
that fits, reads into it, and hands back `Bytes::from_owner(…)` sliced to the
length read, whose owner returns the buffer when the last reference dies. A
pooled buffer is fully initialized once and thereafter only ever read into, so
no reuse zeroes anything.

Three properties are load-bearing rather than incidental. **The `Bytes` is
sliced, not the buffer**, so a short final chunk does not shrink a pooled
buffer and force the next full one to grow it back with a memset. **The owner
is what returns it**, which is what makes this safe for the query replay path:
that path retains a chunk in `batch::SourceChunk` for as long as a zero-copy
`Utf8View` points into it ("Arrow assembly and the zero-copy path"), so the
buffer must not be recycled on the read loop's schedule. And **the pool is
bounded at both ends** — four slots, and nothing above 8 MiB kept unless a
caller announced it as its read size — because `map::attach_text`'s coalesced
span read can be far larger than a chunk and happens once per map, and holding
one of those for the life of the process would trade a scan's ~5.9 MiB resident
set ([`measurements.md`](measurements.md), "What a scan holds resident") for an
allocation nothing asks for twice. The bound that follows is
`4 × max(8 MiB, announced)`, not four chunks: **below the ceiling a one-off is
pooled like anything else**, since the ceiling is what has to keep working for a
source no caller announced to, and the hint only separates the two above it. An
RSS claim is read against that bound rather than against the steady state.

**One-off-ness is a property of the caller, so the caller says it.**
`ByteRangeSource::hint_read_size` is a third, advisory method — defaulted to
nothing, deliberately outside the `object_store` surface the other two mirror —
through which each of the three read loops announces its `chunk_size` once
before it starts. `LocalFileSource` keeps a buffer of exactly that length
however large it is, and applies the 8 MiB ceiling to every other length,
including a span read *smaller* than a large chunk. Nothing else in a
`read_range` call distinguishes a chunk read, which repeats for the whole scan,
from a span read that happens once per map; length alone stood in for that
distinction and caught a deliberately large chunk as collateral.

*Rejected:* a `LocalFileSource`-only setter the CLI calls when `--chunk-size` is
given, which leaves the trait untouched. `ScanOptions::chunk_size` is where the
size is configured and a library embedder never touches the CLI, so that shape
configures one thing in two places and silently loses pooling for the embedder
who sets only the one the library reads. *Also rejected:* inferring the chunk
size inside the pool from the lengths it is asked for — a rule keyed on a length
repeating turns a second map of the same file into a retained span buffer,
which is the failure the ceiling exists to prevent, reintroduced as a heuristic.

*Rejected:* **keeping the trait to the `object_store` mirror and refusing an
advisory method outright.** The mirror is a compatibility argument about the
three *required* methods — the ones a ranged backend must implement for the
addition to be additive — and a defaulted method is outside it by construction:
an `object_store`-backed impl need not know it exists, and inherits the
do-nothing body that leaves it exactly where it would be without the method.
What the refusal would buy is a trait whose surface is describable in one
sentence; what it costs is that the only remaining home for the announcement is
one implementation's inherent API, which is the setter rejected above. It is
also the reversible direction: pre-1.0 there is nothing to migrate, so deleting
the method later is a mechanical change, where discovering that a remote backend
wanted the caller's read length and having nowhere to put it is not.

*Rejected:* changing `read_range` to read into a caller-owned buffer. It
departs from `get_range` exactly where the trait exists to mirror it, and
`spawn_blocking` needs `'static` ownership of whatever it writes into, so a
borrowed-buffer signature cannot be implemented here anyway — the buffer would
have to be moved in and back out, which is the pooling protocol above with the
pool moved into every caller. It was tempting because it looked like the only
way to also delete the scanner's own chunk copy; that copy is gone without it
("The scanner never owns the bytes it scans": the carry), which is what makes
the refusal free.

*Rejected:* **mmap**, which is the intuitive choice for reading a very large
file and is the wrong default here, on three grounds that need no measurement.
It bypasses `ByteRangeSource`, so it could never be the path a ranged backend
takes and adopting it means maintaining two readers. Page faults on a
hundreds-of-gigabytes file on slow media are synchronous and uninterruptible,
with no way to bound prefetch depth or time out — against a design whose
interrupt guard is a promise. And I/O errors arrive as `SIGBUS` rather than
`Result`, which for a tool whose whole premise is reading a file bigger than
memory is a bad trade. It stays available as a possible local-only fast path,
gated on a measurement showing it beats positioned reads by enough to justify a
second code path; nobody has taken that measurement, and every win the scan
work found was inside the shape above.

**The read chunk is one measured constant, and the two schemes above it are
refused.** `ScanOptions::chunk_size` defaults to `scan::DEFAULT_CHUNK_SIZE`,
1 MiB, and `pgdq parse`/`pgdq query` expose it as `--chunk-size` — a tuning
escape hatch for a device unlike the three measured, not a knob with a known
win behind it. **1 MiB is the fastest of the six sizes swept**: on the only
device class where a chunk size shows anything, its neighbours tie with it and
everything further out is clearly slower, and on the other two it is flat
([`measurements.md`](measurements.md), "What the read chunk size is worth"). What decides all three of the I/O defaults is one subtraction:
**no scheme that overlaps I/O with parsing can put a cold scan below the time
the device takes to deliver the bytes**, and on the fastest disk this project
owns a cold `COPY` scan exceeds that floor by 5.8%
([`measurements.md`](measurements.md), "Scan throughput by input shape"). On
the SATA SSD the same subtraction is ~1%; on the HDD the scan is device-bound
by a factor of several. **That ceiling is a fact about the fastest disk we
own**, not a general one: a device on which parse CPU exceeded read time would
reopen both schemes below at once. It has no owner and is not a deficiency —
what would promote it is hardware.

*Rejected:* choosing the chunk size at run time from
`/sys/block/<dev>/queue/rotational`. It is Linux-only, and it degrades exactly
where this tool runs — `/sys` may be masked inside a container, and on LVM,
dm-crypt, MD, NFS or an overlay, resolving a path to its backing device is a
walk with several ways to be wrong. The sweep then made the question moot: the
measured spread is a tie across the middle of the range and flat on two of the
three device classes, so there is nothing for adaptivity to chase.

*Rejected:* removing `--chunk-size` once the sweep came back flat, on the
grounds that a knob justified by a null result is surface nobody needs. Two
things say otherwise. The flag **is** the figure's regeneration command, and a
figure whose command is gone is deleted rather than kept — so removing it
deletes the published evidence that 1 MiB is right, which `DEFAULT_CHUNK_SIZE`'s
doc comment and the manual both cite and which cost a six-size, three-regime,
nine-rep sitting. And the null result is scoped to the three device classes
this project owns; a lever worth nothing on all of them is exactly what someone
on unlike hardware needs in order to find out it is worth something there. It
stays on both `parse` and `query`, because they are the same read path under
the same `ScanOptions` and an asymmetry there would read as a defect rather
than as restraint. *Also rejected:* keeping it but refusing values above the
8 MiB pool ceiling, the way zero is refused — that would delete the figure's
16 MiB row, which is the one that brackets the ceiling, and the ceiling no
longer decides anything for a chunk in any case.

*Rejected:* `posix_fadvise(POSIX_FADV_SEQUENTIAL | WILLNEED)` on the local
backend. The chunk sweep is the experiment that answers it: a 16 MiB chunk is a
deeper prefetch than a doubled readahead window, issued while the parser is
idle and stated rather than inferred — and it is the **slowest** row cold on the
NVMe, 1.37× the 1 MiB default, with 8 MiB slower too
([`measurements.md`](measurements.md), "What the read chunk size is worth").
Cold time does not fall with request depth on any device measured, so the
kernel's own readahead has already taken what there was and a hint asking for
more has nothing to win. It would also cost this crate its first direct
`libc`/`rustix` dependency, which is not what the spec's six-line bullet
looked like.

*Rejected:* double-buffered readahead — issuing chunk *N+1*'s read while chunk
*N* is parsed. Its prize is `min(device time, parse time)` and it is bounded by
the same 5.8%, on the one device class where that number is not ~0; against
that it is a rework of three read loops, a second in-flight buffer against the
~5.9 MiB a scan holds resident ([`measurements.md`](measurements.md), "What a
scan holds resident"), and one more thing the interrupt guard
and the query path's chunk retention have to be correct about. Most of the
parse CPU is already hidden behind the read on that device, and overlapping
harder cannot recover what is already overlapped.

**What a raised chunk size costs is memory, and the bound is the caller's own
number.** `POOL_SLOTS` is 4, so a 16 MiB chunk can hold 64 MiB against the
~5.9 MiB a one-block scan otherwise sits at
([`measurements.md`](measurements.md), "What a scan holds resident") — which a
caller who asked for 16 MiB buffers has largely accepted already. That is the whole cost, and it is stated in the
flag's help, in the manual and in `DEFAULT_CHUNK_SIZE`'s own doc comment.

**What it used to cost is the measurement of what the pool is worth.** Before
the announced read size, a chunk above the ceiling was never returned to the
pool, so every chunk became the fresh `calloc` the pool exists to remove:
0.472 s → 0.866 s warm at 16 MiB against 8 MiB, **1.83×**, not a few percent
([`measurements.md`](measurements.md), "What the read chunk size is worth" —
whose 16 MiB row was taken under that behaviour and is stale until the figure
is re-taken). The number is kept because it prices the pool from the outside:
it is what a per-chunk allocation costs on a real file, measured rather than
argued, and nothing else in the register states it.

**Two entry points, deliberately different in kind:**

- **Pull mode** — an async `Stream<Item = Result<RecordBatch>>`, natural for
  async engine embedding, with a blocking `Iterator` wrapper over the same
  stream for sync contexts (the CLI is one).
- **Push mode** — a **sync** callback, `FnMut(RecordBatch) -> ControlFlow<()>`,
  driven internally as the library drains the async stream. *Rejected:* an
  async callback, to avoid `async fn`-in-traits ergonomics and a per-call boxed
  future; an async caller who needs non-blocking callback behaviour uses pull
  mode directly.

`pgdq query` is a pull-mode caller by necessity: rendering a nested column
needs the stream's `NestedPlan`s *while* iterating, and push mode hands the
`ResolvedSchema` back only once the stream is drained. It reads
`stream.resolved_schema().plans` per batch rather than once, since a block's
schema is per-block (see "Nested columns"). That leaves `read_table` with no
non-test caller, filed in
[`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md)
for the phase that decides whether push mode keeps its place.

**One struct carries the whole query.** `QueryOptions` holds what the query
*asks for* — the projection and the filter terms — beside how its batches are
cut,
and both entry points take it. The two halves are the same kind of thing, so
splitting them across a struct and a positional argument would make every
caller learn which is which; the name also follows from the struct already
carrying `database` and `scan_extent`, neither of which is about batching.

**Batch size** is caller-settable by row count, in-memory byte size and the
source byte span a batch covers, whichever is hit first. Default 8192 rows
(matching the common Arrow/DataFusion convention), no byte cap, and a 64 MiB
source span. The third of those is a memory bound rather than a sizing knob —
see "Three flush triggers, and only one of them bounds memory".

**Eager versus incremental indexing is a caller choice.** Incremental (the
default) discovers structure only as far as needed to answer the current query,
persisting whatever it found along the way — so a query for table X that scans
past A, B and C leaves the cache useful for those too. Eager (`pgdq parse`)
scans the whole file up front before answering anything.

### The compressed source

`.xz` input is read directly — `pgdq parse|info|query --source foo.dump.xz` —
by a second `ByteRangeSource` that decodes beneath everything else. Nothing
above L1 knows: the span offsets a compressed source produces are
**uncompressed** offsets, byte for byte the offsets a plain scan of the
decompressed file produces, so the scanner, the map, the schema and the query
passes are unchanged. gzip and zstd are not read; those are the codecs
`pg_dump`'s own plain-format `--compress` writes, and closing that
*compatibility* gap is separate work
([`pg-dump-compatibility.md`](pg-dump-compatibility.md)). Every `.xz` file this
reads was compressed by a third party after the fact, which is how the dumps
that actually get shipped around arrive — koji's included.

**Recognition is a library convenience, deliberately outside the trait.**
`pgdump_query::open_local(path, known: KnownCompression) -> Result<Recognized>`
reads the first six bytes, compares them against `.xz`'s magic
(`\xfd7zXZ\x00`), and returns an `XzSource` or a `LocalFileSource`
accordingly; the CLI's three commands call it in place of naming a source
type. `known` is what a cache already said about this file, and the walk it
spares is below; `KnownCompression::Unknown` is the no-knowledge case and
always yields a source. Both implementations stay
source-agnostic and are constructible directly, so an embedder that already
knows what it holds need not go through recognition at all, and a further codec
extends this one function rather than the CLI growing a second dispatch.
Content sniffing rather than a path-extension check: a real `.xz` file is
recognised whatever it is named, a file merely *named* `.xz` whose bytes are
plain opens plain, and a file shorter than the magic is answered "not xz"
rather than an error. *Rejected:* extension-based dispatch — cheaper, since it
needs no read before the source exists, but wrong on a renamed or extensionless
file, and every embedder wanting the convenience would reimplement it. It is
exported at the crate root rather than through a public `io` module: `mod io` is
private with its types re-exported, so recognition joins that list and the
module's privacy boundary is unchanged.

**One decoder, restarted on seek, retaining nothing.** `XzSource` holds a
single live `xz_seek::Reader` and its current uncompressed position behind a
`std::sync::Mutex`, because the reader's own positioned read takes `&mut self`
where the trait's methods take `&self`. A read at the current position keeps
pulling, which is every read on the forward path; a read anywhere else restarts
the decoder at the block covering the offset and discards to it. *Rejected:*
retaining the last decoded block, so repeated reads inside one block are free —
it costs 24 MiB resident on a file of ~24 MiB blocks and 128 MiB on a
`--block-size=128MiB` one, to accelerate a pattern this library barely has,
since every read but the two backward ones is sequential and those are served
with no waste by the streaming form. *Also rejected:* decoding from the covering
block on every call, which re-decodes 24–128 MiB per 1 MiB read.

**Exactly two callers read backwards**, which is what bounds that decision.
`stream.rs`'s replay loop re-reads a block the mapping pass has already walked
past (the deliberate double read under "Query: mapping and streaming are
separate passes"), and `map::attach_text` re-reads the gaps between `Data` spans
once the scan has finished. Neither is on the `pgdq parse` path, which is purely
forward.

**Two file handles, deliberately.** `XzSource::open` walks the file's stream
footers once to build the seek table and gives that handle to the reader, which
owns it for decoding; a second handle answers `stored_size()`/`modified()` with
a plain `stat` and never disturbs the decoder's live position. So `size()` is
the exact **uncompressed** length, read out of the stream index with no further
I/O, and `stored_size()` is the compressed file's own on-disk length — the two
axes the cache needs kept apart. `read_range` wraps the reader's positioned read
in `spawn_blocking`, through the same `BufferPool` `LocalFileSource` uses, and
turns a short fill into `UnexpectedEof` to match `read_exact_at`'s contract:
every read loop already clamps its length against `size()`, so a short read here
is a caller/source disagreement rather than a normal outcome. Verification is
the decoder's stronger default — a partly-decoded block's check is completed
before the reader leaves it — because a scan persists the structure it discovers
as it goes, so bytes whose check failed two calls later would already have been
recorded as fact.

**`size()` keeps its promise, and the walk is what pays for it.**
`ByteRangeSource::size()` still means *the exact number of bytes `read_range`
can address*, which for this source is the exact uncompressed length. *Rejected:*
relaxing the contract, letting `size()` return a bound and making a zero-length
read the termination condition. It is mechanically cheaper than it looks — every
read loop is already `want = chunk_size.min(size - read_pos)` … `read_pos +=
bytes.len()`, so all of them already tolerate a short read, and only the loop's
exit test and the coverage denominator depend on the number being exact — but
the exact number is what `total_size`, `scanned_through` and `pgdq info`'s
coverage arithmetic *mean*, and xz can answer it honestly. A codec that cannot
is where that relaxation has to be faced. *Also rejected:* deriving the size from
a forward decode instead. The seek table does fall out of a forward scan for
free, but every caller takes `size()` **before** the first read, so a
forward-only derivation has no answer at the moment the answer is required.

**Which shape a file has decides what a backward read costs**, and the ranking
is not the intuitive one. An `.xz` file is a sequence of *streams*, each holding
one or more *blocks* and ending in an index listing every block's compressed and
uncompressed size:

| Shape | Produced by | Seek table costs | Random access |
|---|---|---|---|
| One stream, many blocks | `xz` 5.6+ default (`-T0`), any `-T>1`, `--block-size` | one footer read | per block |
| Many streams | concatenation — `cat a.xz b.xz`, chunked pipelines | one footer read **per stream** | per stream |
| One stream, one block | `xz -T1`, xz older than 5.6, library writers | one footer read | **none — every backward read decodes from zero** |

All three are read and none is refused at open. The third is **announced** as a
`NonSeekableCompressedSource` warning the moment the seek table becomes
available — freshly walked or loaded from a cache — rather than met by the user
as a stall, with the cause and the remedy (`xz -T0`, or `--block-size=<size>`)
in the message ("Diagnostics: one severity scale, two types"). `pgdq parse` is
unaffected in fact as well as in principle, since it never reads backwards.
*Rejected:* refusing `query` on a non-seekable file. It denies the user a command
that would work, merely slowly, and only the user can judge whether one
decode-from-zero is worth waiting for.

The many-streams shape is the one that costs most in practice and the one the
motivating file has: the koji upstream download is 31,150 concatenated streams
of one block each, and walking 31,150 footers is **85 s** on an HDD at 10% CPU,
because each is a separate seek. One footer read is free. That difference is the
whole reason the seek table is persisted rather than re-derived per run.

**The persisted table is read back, so a file's walk is paid once rather than
once per command.** `cache::save` records it through
`ByteRangeSource::seek_table()`; `cache::claim` reads it back
*before any source exists*, which is the moment recognition needs it, since
recognition is what decides which source to build; and `open_local` hands it to
`XzSource::with_table`, which opens the reader over it and walks nothing. What
crosses that boundary is a bare `xz_seek::SeekTable` rather than a cache, so
`io.rs` names nothing in `cache` — the caller does the loading, through an entry
point that checks the envelope's identity against a plain `stat` on the dump
path, since there is no source yet to hand `load`. `load` itself is unchanged
and runs a moment later for the pass that follows. *Rejected:* stopping that
decode short of the index — `compression` and `identity` do sit ahead of it in
the envelope, but bincode is positional, and decoding the envelope twice is
bounded by index size.

**What recognition is told is a claim it checks, not one it believes.**
`KnownCompression` is three-state — `Unknown`, `Plain`, `Xz(table)` — because
"the cache says this file is plain" is something the file's own magic can
contradict, and an `Option` would fold that case into "nothing is known". A
table that passes the envelope's identity check and then fails `xz_seek`'s own
`validate` invalidates the **whole** cache rather than only the table: both were
written by one `save` from one file, so a table that does not describe this file
is evidence the span index does not either — and the index is the half that
mis-addresses rows silently, where a bad table is caught loudly by a block
header's CRC32. The same holds when the envelope carries a compression index for
a file that no longer sniffs as `.xz`, or the reverse. Each is one outcome,
`Recognized::Mismatch`, and it is **not** a `CacheStatus`: reaching this point
means the two stored sizes `SourceChanged` quotes as its evidence are equal, so
`load` never sees it and none of the four statuses describes it. It gets a
sentence of its own, carrying the same two ways out the size mismatch carries —
see "The CLI's two refusals are worded as one" for why it stopped borrowing
`Unreadable`'s.

**Rejecting a table reads no bytes, so nothing is spent reaching the refusal.**
All three commands report the contradicted claim having read nothing,
recognition answering "the cache does not describe this file" as an outcome of
its own rather than recovering transparently.

**The sibling refusal is free as well, and it is the same read that pays for
it.** Where the cache's recorded stored size is simply wrong for this file,
`cache::claim` answers `SourceChanged` rather than collapsing it into
"nothing is known", so no source is opened and no stream footer is walked to
reach a refusal that was already settled by the envelope and a `stat` — 85 s on
the koji download, one read on every other shape. It is one *outcome* added to
the early read rather than a second read: the two facts a caller needs before
opening a file both come out of the envelope it has just decoded, and the
comparison stays in `cache.rs` beside `load`'s, so the early refusal is the
late one pre-empted rather than a second reading of the same rule. The library
still refuses on its own, which is the guarantee an embedder that never goes
through the CLI holds ("The cache"). *Rejected:* a sibling function reporting
the stored size beside this one — both decode the envelope, so the *usable*
path, which is the one a warm koji run takes, would decode a 31,150-entry seek
table twice to spare a walk on the path that is about to fail. *Rejected:*
widening `KnownCompression`: `io.rs` names nothing in `crate::cache`, and a
cache's identity verdict is not something recognition has any use for.

*Rejected:* `parse` deleting that cache and rescanning, which is what it did
when the table was first read back. It was defensible on its own terms — `parse`
overwrites whatever sits at its `--dqcache` path within the first throttled save
("`parse` resumes, and saves as it goes"), so deleting at startup only moved a
loss that was coming anyway — and both halves of that are now the prohibited act:
the library never replaces cache data automatically ("The cache"). `parse`
refuses with the other two, and the way out is the caller's.

The saving is
tested behaviourally rather than by instrumentation: the constructor is handed a
table naming a check algorithm this file's streams do not use — which `validate`
does not police and no walk would ever produce — so a silent walk fails the test
by succeeding.
*Rejected:* making `XzSource` generic over its file handle so footer reads can
be counted — the walk happens below `ByteRangeSource`, so counting it means
reworking a tested type's signature to buy an assertion the behavioural test
already makes.

**The walk is still paid by the command that first parses a file**, there being
no cache to read a table out of yet, and by nothing after it. Two things bound
that meanwhile: a single-stream file's walk is one read whatever its size, and
`pgdq info --dqcache <path>` with no `--source` opens no source at all and so
never pays it.

**A property falls out of the offsets being uncompressed ones**: a cache built
from the `.xz` describes the decompressed file equally well, differing only in
the identity that guards it.

**Concurrent `read_range` calls serialize on the mutex**, and nothing calls
concurrently today — every read loop in this crate is sequential. Parallel,
stream-aligned decode is the parallel-scan work's, and that is where the scaling
is: on this corpus one core decodes ~446 MB/s of plaintext where four concurrent
per-stream decodes reach ~1.48 GB/s, and `xz`'s own threaded decoder gains
nothing on a many-streams file because it parallelises blocks *within* a stream.
Those are probes, not registered figures — no harness, no `drop_caches`
discipline — and no document may quote them as measurements. Nothing here
commits to a figure: the number a caller actually wants is concurrent decode
throughput against the plain path's device-bound figures, and that is unreachable
until parallel decode exists.

**The decoder is a vendored crate, not a published dependency.** No published
crate answers a positioned read over an `.xz` file — `liblzma`'s safe Rust
surface exposes no stream-index parser or block decoder, and a survey of
`lzma-rust2`, `xz4rust`, `gibblox-xz` (GPL), `ixz-core` and `iluvatar` found none
providing `read_range(offset, len)` over the uncompressed stream. That gap was
carved out into `xz-seek`, its own crate and repository, and this project is its
first real-world consumer; it is not published until that consumption has vetted
the interface, and the gate is two consumers rather than one — this source, and
the row-group-statistics work, which reads the same addressing layer for a
different purpose. So what this repo depends on is a **frozen read-only copy** at
`vendor/xz-seek/`, excluded from the workspace, re-synced on demand by
`scripts/vendor_xz_seek.py` and stamped with the source commit in
`vendor/xz-seek/VENDORED_FROM`. A bug found here is fixed upstream and returns
at the next sync, never patched in place. *Rejected:* a live path dependency on
the sibling working tree — verified that even behind an off-by-default feature
the dependency graph is resolved before features are considered, so a checkout
lacking the sibling repo fails `cargo check` outright. *Rejected:* publishing
first and depending on a version — publishing an interface nobody has consumed
is what produces the 0.x churn the arrangement avoids. The build takes the
crate's default `liblzma` backend over its pure-Rust one (unsafe-free, ~2.2×
slower), pulling vendored C into an otherwise pure-Rust workspace, on the ground
that the integration being vetted should be the one that ships.

**The snapshot is deliberately behind upstream, and the first consumer that
needs newer work is what re-syncs it.** The copy was taken at the moment that
crate's parallel-block-decode work was still unstarted, precisely so it would
sit still while that work moved the source underneath it. One method asked for
on this project's behalf — a block count scoped to a byte range, which would let
the non-seekable warning be bounded to what a query actually touches — landed
upstream after the snapshot, and re-syncing for it was **refused**: the warning
this design asks for is file-wide, which the frozen copy answers from
`SeekTable::block_count()` (always 0 or 1 once a table is not seekable). So the
parallel-decode work is both the first consumer that needs a newer snapshot and
the thing that re-syncing would drag in early; it is where the copy is re-taken,
and where whether a vendored copy is still the right arrangement gets asked
again. `vendor/xz-seek/VENDORED_FROM` records which commit is in, and
`scripts/vendor_xz_seek.py` is what re-takes it.

**None of the container facts above is an entry in
[`postgres-invariants.md`](postgres-invariants.md)**, and that is the register's
scope rather than an omission: its entries are properties of `pg_dump`'s output,
and its whole payoff is one ritual — walk the file when a new PostgreSQL major
lands. Nothing about the xz container or the decoder can be invalidated by a
PostgreSQL release, so an entry there would be re-verified at a trigger with no
bearing on it. The facts live here, beside the mechanism that leans on them, and
the decoder's own guarantees are that crate's register to keep.

## Bytes and structure

### The scanner never owns the bytes it scans

`CopyScanner` is a synchronous state machine: the caller holds the buffer,
passes a slice, and the scanner reports how much it consumed. That is what
keeps events zero-copy and lets one state machine back both the async driver
(`scan()`) and the pull-mode `Stream` without a second parser.

The consequence is that the caller's buffer is the only thing bounding memory,
which is why `ScanOptions::max_line_bytes` exists and why exceeding it is a
hard error rather than a truncation.

**What a read loop carries is one line, not one chunk.** The scanner consumes
whole lines, so what a chunk leaves unconsumed is always a *single*
unterminated line — `next_event` stops only where it finds no newline. So a
chunk is scanned in two passes, which `scan::ChunkCarry` holds the state for:
the carried line with the chunk's first line-terminated prefix appended to it,
and then the rest of the chunk **where the reader left it**. Handing the
scanner two buffers within one chunk costs it nothing, because `CopyScanner`'s
`base` is an absolute file offset and each pass is bracketed by
`take_consumed` exactly as a single buffer's refill would be. All three read
loops — `scan::scan`, `stream::map_forward` and the replay loop in
`stream::table_stream` — drive it identically, through
`ChunkCarry::PASSES`.

*Rejected:* one growing buffer per read loop, appended to per chunk and
`drain`ed of its consumed prefix, which is what the loops did until the
carry. It copies every byte of the file twice — once in, once when the
remainder shifts down — and was **38.0% of a warm `parse`'s user time**, the
largest single term in it ("parse-profile"). Its one virtue was that a row
always arrived contiguous in a buffer the loop owned, and the carry keeps that:
the pass that produces a
straddling row is precisely the pass that has just made it contiguous.

Two properties are what make the carry safe rather than merely smaller. **The
`Bytes` a chunk arrives in outlives the pass that scans it**, because the
buffer pool returns a buffer only when the last reference to it dies
("Execution model and API surface"), so an in-place scan is not also a
lifetime problem — and the query replay path was already retaining that same
`Bytes` in `batch::SourceChunk`. And **the zero-copy `Utf8View` path never
depended on which buffer a row was read out of**: `push_utf8view_field`
locates a field's chunk by *absolute file offset* and falls back to a copy
when it finds none, so a row scanned in place and a row reassembled on the
carry resolve identically ("Arrow assembly and the zero-copy path").

The degenerate case is a chunk with no newline in it at all: the whole of it
joins the carry and the in-place pass is empty, which is what the one-buffer
shape did on *every* chunk and is the growth `max_line_bytes` bounds.

### Parser robustness requirements (hardcoded)

Derived from sanity-checking the design against the real koji sample (784GB,
`pg_dump 16.14`, plain format). These are correctness requirements with one
right answer, not configurable behaviour. **Read this before changing scanner
behaviour.**

- The scanner must tolerate arbitrary leading-backslash **psql meta-commands**
  (`\restrict`, `\unrestrict`, `\connect`, and generically any line starting
  with `\`) as skippable non-SQL lines. `\restrict`/`\unrestrict` are emitted
  by `pg_dump` versions carrying the 2025 search_path-safety patch (confirmed
  present in `pg_dump 16.14`) and were not part of the original format
  description.
- `COPY` block detection must be **line-anchored** (`^COPY `). Row *data* can
  legitimately contain the literal substring `COPY … TO stdout;` mid-line —
  observed in koji's `lock_monitor.activity` logging table — and a
  non-anchored scan misfires on it.
- Row/column splitting must honour standard COPY TEXT escaping (`\N` for NULL,
  `\n`/`\t`/`\\`) rather than naive byte-level line splitting; confirmed
  load-bearing in koji, whose long text fields contain embedded literal `\n`
  sequences.
- Input is **plain SQL text by the time it reaches this state machine**, and
  that is what the scanner assumes rather than a scope decision. Where the file
  on disk is `.xz`, a decompressing `ByteRangeSource` sits beneath the scanner
  and hands it the same bytes at the same uncompressed offsets ("The compressed
  source"), which changes nothing here. `.gz`/`.zst` are not read and stay
  caller-side preprocessing (`roadmap.md`, P15). Archive formats are different
  again — they compress per entry, internally, and will need streaming
  decompression inside their container layer.

### `Event` is the scanner's contract

`Event` is `CopyStart` / `Row` / `CopyEnd` / `Line` / `DollarQuoteEnd` /
`LargeObjectStart` / `LargeObjectEnd`. **Every site matching it exhaustively
(no `_` arm) needs updating when a variant is added**, and that set is:
`index::build_index`/`scan_preamble`, `stream::map_forward` and its replay
loop, `map::build_map`, and three spots in `tests/scan.rs`. The exhaustive
match is the intended mechanism, not an oversight — `stream.rs`'s replay
ignores both `DollarQuoteEnd` and `Line` explicitly, because a replay covers
exactly one `COPY` block and nothing outside a block can fall inside one.

`Event::Row` carries `{}`. A pass that is only mapping the file never parses a
row — the scanner only needs to find `\.` to know the block's extent — so it
allocates no `SourceChunk`s and takes no zero-copy views. That is why the
target block's double read (see "Mapping and streaming") is cheaper than it
sounds.

### COPY TEXT escaping is L1 and bidirectional

`copy::decode_field` unescapes; `copy::encode_field` is its exact inverse,
implementing only the seven escapes `pg_dump`'s `COPY TO` ever emits — a
doubled backslash and the six control-character mnemonics `\b \f \n \r \t \v`
(I15). The octal and hex forms `decode_field` accepts are a `COPY FROM` reader
convenience that `COPY TO` never produces.

`encode_field` is deliberately **not** a general COPY-text encoder. Nothing
here needs one: every correctness fixture is real `pg_dump` output, never
hand-encoded, and the single use is proving the two functions round-trip
against real on-disk bytes.

### A row's bytes are validated once, in bulk

A `Utf8View` column takes its bytes straight out of the read chunk, so every
field that reaches it has to be known-UTF-8 first. That check used to be
`std::str::from_utf8` **per field** — sixteen calls a row on the 16-column
control, each one a scalar validation of a few bytes, and together **7.81% of
a `strings` query profile** and 2.60% of a typed one. It is now one SIMD pass
per chunk.

**The soundness is one property of UTF-8 and one of COPY TEXT.** A COPY TEXT
row is delimited by `0x09` and terminated by `0x0A`, both ASCII, and an ASCII
byte never occurs inside a multi-byte UTF-8 sequence — so every tab-delimited
piece of a validated row is itself valid UTF-8, and no field needs its own
check. `copy::validated_prefix` is that pass: the largest prefix of a span
ending on a newline, through `simdutf8::basic::from_utf8`. Cutting at the last
newline is what makes it safe to run over a *chunk* — the bytes past it are a
partial line whose continuation is in the next chunk, and a multi-byte sequence
split across that boundary would fail for no reason.

**What a row carries is `copy::RawRow`**, the row's bytes plus, when the loop
validated them, the same bytes as `str`; `RawRow::decode` takes a field's
*range* and slices whichever of the two it has. The range comes from
`copy::field_ranges`, or — on the replay path, where a row has more than one
consumer — from the `copy::RowSplit` they share; either way it is a position
rather than a slice, because a caller holding the row twice needs to take the
same field out of either. `split_fields` is `field_ranges`, resolved.

**The escaped path still validates, and must.** `\xNN` and the octal forms
synthesize bytes the input did not carry, so `unescape_field` ends in
`String::from_utf8` exactly as it always did. Only the borrow path — a field
with no backslash, which is the common case and the only one a zero-copy view
is taken of — skips a check.

**Nothing about which inputs are accepted changes.** A prefix that does not
validate answers the empty string, which puts every row of that span back on
the per-field check, raising `Error::InvalidUtf8` at exactly the fields it
always did — so a dump carrying non-UTF-8 bytes behaves as before and pays one
extra pass over the spans that hold them. `tests/stream.rs` drives both halves:
a query answers identically at every chunk size from 1 byte up, and a non-UTF-8
field is refused only where a projection actually reads it.

**The bulk pass is taken on the first row that will decode something**, not per
chunk unconditionally. A query that decodes nothing — `COUNT(*)`, `pgdq query
--no-columns`, no filter — would otherwise pay a validation of bytes it never
reads; `RowBatcher::decodes_fields` and `ResolvedExpr::reads_fields` are the
gate, and the `--no-columns` row of "What a column costs" is what it protects.

**There is no `unsafe` in it**, which was a design choice rather than an
accident. The obvious shape — `str::from_utf8_unchecked` under a boolean the
caller promises — makes a safe function able to cause undefined behaviour if a
caller's offset arithmetic is wrong. Slicing the validated `&str` with
`str::get` instead costs a bounds-and-boundary check per field, which is O(1),
and a wrong range costs that row its fast path rather than the process.

*Rejected:* validating per row rather than per chunk. It is one call instead of
sixteen and needs no plumbing at all, but it leaves the per-call overhead that
is most of what the sixteen cost on short fields, and `simdutf8`'s dispatch is
amortized over a megabyte in the shape above and over 200 bytes in this one.

*Rejected:* shortening the prefix to `compat::from_utf8`'s `valid_up_to` when a
span fails, so that the rows before a bad byte keep the fast path. It buys
nothing on a dump that is UTF-8 (the common case, and the only one the figures
are taken on) and complicates the one path a reader has to trust.

## The file map

An ordered `Vec<Span>` that **tiles** the file: every byte belongs to exactly
one span, spans sum to the file size, none overlap.

`Span { start, end, database, toc: Option<TocHeader>, toc_owned: bool, text:
Option<SpanText>, body: SpanBody }`.

- `SpanBody`: `Table`, `TypeDef`, `Extension`, `Data(DataBlock)`, `Connect`,
  `VersionHeader`, `AlterTypeAddValue`, `Framing`, `Unparsed`, `Unscanned`.
- `DataBlock`: `Copy(CopyBlock)`, `InsertRun(InsertRun)`,
  `LargeObjects(LargeObjectRegion)`.

`SpanBody`'s vocabulary is deliberately not the TOC's ~63 kinds; those ride on
`TocHeader::kind` and attach to `Unparsed` spans, which is exactly what
distinguishes known-but-unhandled from unrecognized.

*Rejected:* TOC-driven segmentation as the **primary** structure. Its appeal is
a closed, upstream-verifiable vocabulary (~63 `Type:` values) and free tiling
boundaries. It fails on input that is not `pg_dump`'s own archiver, and I3's own
caveat — a dollar-quoted function body can contain a line that looks like a TOC
header — means the statement grammar has to corroborate regardless. So the
statement pass is primary and the TOC is an enrichment layer read off the same
lines. **This is not a performance decision**: both approaches are line-oriented
passes over the same bytes, and on koji that is 154KB of DDL against 784GB of
data, unmeasurable either way.

**Coverage increases monotonically** — a later change may subdivide a span or
attach detail to it, never reduce coverage. That is a standing constraint on
all future work and lives in [`roadmap.md`](roadmap.md) with the rest of them;
the fixture tiling test is what keeps it from decaying into an aspiration.

### A span's `end` is fixed up at push time

The tiling invariant makes a span's true end exactly the next span's start, so
`Builder::push_span` closes the previous span the moment a new one is pushed.
That is what makes `Builder::snapshot(&self, end)` — a non-consuming "spans so
far" read — possible at all, and leaves `finish` with only the one span still
open to close.

*Rejected:* an end-of-scan fix-up pass. It cannot coexist with `snapshot`,
which the incremental mapping path depends on.

**A finished scan's spans are greedy; `snapshot`'s stop at the watermark.** In
a whole-file map a `COPY` block's span runs on to wherever the next span
starts, past the block's own `end_offset`, while `snapshot(end)` closes the
open span *at* `end`. So an index truncated by hand — which is how the CLI's
partial-cache tests build one — keeps spans by `start < frontier` and then
clamps the last one's `end` to the frontier; filtering on `end <= frontier`
silently drops the very block the scan had just banked.

**`snapshot` is sound at two kinds of point, not one.** A `CopyEnd` watermark
is the obvious one. The other is immediately after `Builder::on_copy_start`,
which leaves the builder `Idle` in every arm with the block's `Data` span still
pending, so `spans.last()` is the DDL span ahead of the block — the boundary
the recurring metadata recompute stands on (see "`parse` resumes, and saves as
it goes"). The offset to close at there is
`pending_comment_start().unwrap_or(header_offset)`, read **before**
`on_copy_start` consumes the comment, or the block's `-- Data for Name:` header
is swallowed into the span before it. `scan_preamble` uses the same idiom.

### Three things close a statement

Anything adding a fourth should check `on_dollar_quote_end`'s
`Mode::Statement`-only guard.

1. `preamble::statement_complete` — the accumulator's rule, which tracks
   double-quoted identifiers (`""` doubling) and `--` line comments. Without
   that, an apostrophe inside either opens a string that never closes. It is a
   wrapper over `preamble::StatementScan`, which an `INSERT` run drives
   incrementally instead (see "Bulk regions" below), so there is one
   implementation of the rule and two ways of feeding it.
2. A `--`-prefixed line reasserting a boundary, *unless* `preamble::in_open_quote`
   says the buffer is mid-string (a value spanning physical lines legitimately
   starts a line with `--`).
3. `scan::Event::DollarQuoteEnd` — position-only, no text.

The third exists because `scan.rs` emits no `Event::Line` for a dollar-quoted
line, *including* the one carrying a `CREATE FUNCTION`'s own closing `;`: a
detector watching only for statement completion can never observe such a
statement ending. Its detection is `touched && tag.is_none()` from
`copy::scan_dollar_quotes`, **not** a tag transition — testing "had a tag, now
doesn't" misses `AS $$ SELECT 1 $$;`, a shape `pg_dump` really emits.

Real `pg_dump` output is unaffected by any of this: every entry carries a
`-- Name:` header, which already reasserts a boundary, and every fixture's map
is byte-identical with and without the dollar-quote handling. What it buys is
the guarantee a header-less file degrades to **one span per object**, not to
one span for the rest of the file.

### Two boundary rules that are easy to break

Both were pre-existing bugs caught only by later, more precise tests, and each
was invisible to `check_tiling` by construction — the tiling check does not
care *which* span owns which bytes.

**A blank line must not force a pending comment block to close.**
`looks_like_toc_name_line` only matches `-- Name: `, never `-- Data for Name: `,
and `_printTocEntry()` always writes a blank line after the closing `--`. With
a blank line closing the block, *every* real `COPY` block's TOC comment became
its own `Framing`/`Unparsed` span before `on_copy_start` ever saw it, and
`on_copy_start`'s comment-absorption arm was dead code on real input.

**`scan_preamble` retreats its stop point rather than guessing.** It never
feeds the first `COPY` block's `CopyStart` to its `Builder` (that would open a
`Data` span it cannot close). Once a pending comment can survive to that stop
point, `finish`'s `flush_pending` would otherwise guess it closed as
`Framing`/`Unparsed` — the wrong answer, since a later full scan classifies it
as part of the `Data` span. `Builder::pending_comment_start()` lets the caller
retreat `end` to the comment's own start, and `flush_pending` discards a
pending comment whose `start >= end` instead of pushing a zero-length span.

### Bulk regions: one span kind, three payloads

Only `CopyBlock` carries inner offsets
(`header_offset`/`data_offset`/`terminator_offset`/`end_offset`), because it is
the only one anything reads rows out of. The
`span.start ≤ header_offset < data_offset ≤ terminator_offset < end_offset ≤ span.end`
invariant is a one-off for `COPY`, not a pattern to copy onto the other two.

A TOC-commented `COPY` block is **one span**, comment absorbed. Anything
reasoning about `Data` span starts should use `span.start`, not
`header_offset`.

**The large-object region gets a scanner-level fast path; `INSERT` runs get a
map-level one.** That asymmetry is the map's one real cost decision, and I12 is
why it is safe: on v17+ the region can run to hundreds of gigabytes, and a bare
`BEGIN;`/`COMMIT;` pair never appears anywhere else in plain `pg_dump` output
(`StartRestoreLOs`/`EndRestoreLOs`, `pg_backup_archiver.c`), so
`scan::CopyScanner` recognizes it with no TOC context at all and skips every
line between **unread** — no `Event::Line`. An unterminated region is
`Error::UnterminatedLargeObjectRegion`, mirroring `UnterminatedCopyBlock`.

An `INSERT` run instead reuses the statement *scan* — `preamble::StatementScan`,
the same rule the accumulator applies, without the accumulator — and simply
stops pushing a span per statement: `Mode::InsertRun`, entered from
`Mode::Statement`'s first line via `parse_insert_target`, folding completed
statements into one
span until a different table's `INSERT INTO` arrives or the next TOC comment
reasserts a boundary. `Mode::Comment`'s close arm opens it too, at the
comment's own offset, when the line that closes the block is an `INSERT INTO` —
so a TOC-commented `INSERT` run is **one span**, comment absorbed, exactly as a
TOC-commented `COPY` block is, and it owns its `TABLE DATA` entry. That arm is
the only route to attribution here: `looks_like_toc_name_line` must keep
refusing `"Data for "` (see "TOC enrichment"), so the comment would otherwise
close as its own `Framing` span, and `Framing` is one of the three kinds
`governing_toc` inheritance never crosses.

**That absorption is unconditional** — a comment block carrying no parseable
TOC header is folded in the same way, as `on_copy_start`'s `Mode::Comment` arm
folds one for a `COPY` block. `pg_dump` cannot produce the shape that
distinguishes the two (its only header-less block is the file header, and `SET`
statements always separate that from any data), so what decides it is the
producer class that *can*: a hand-written or `pg_dump`-compatible dump, where
`-- a note` above an `INSERT INTO` is ordinary. Absorbing gives that file one
`Data` span where gating gives it a `Framing` span plus a `Data` span — the
coarser-and-cheaper trade the `COPY` path already makes. The symmetry between
the two paths is a consequence of that call, not the argument for it.

<!-- deficiency: KD9 -->
**An `INSERT` run costs a few times a `COPY` scan per byte.** Warm it is
**4.9×** — 2.25 s against 0.457 s over 3.00 GiB — and **7.05× the `dd` floor**
where a `COPY` scan is 1.8×; cold on the SSD the difference is gone, 1.02× the
floor against 1.00×, and **cold on the NVMe it is back, 2.62× against 1.06×**
([`measurements.md`](measurements.md), "Scan throughput by input shape"). Which
of those three a user meets is decided by their storage, not by their dump.
Carry it as a magnitude rather than a value: the legs are warm
sub-second and sub-three-second readings that move several percent between
sittings, and the ratio is the durable half. Correctness, tiling and row counts
are unaffected.

**It cannot be 1×, and that is a property of the two algorithms.** A `COPY`
block's data is *skipped* — the terminator is a line-anchored needle — while an
`INSERT` run's boundaries rest on no line-anchored invariant at all, so every
byte has to be crossed quote-aware to find where a statement ends. No
implementation of this path reaches a `COPY` scan's cost.

**What that argument does not establish is that 4.9× *is* that floor, which is
why `KD9` is a live entry rather than a property.** Two cuts are known,
specific, and untaken. `insert_run_line` feeds the whole line to the scan
including the `INSERT INTO <table>` prefix it has just matched, whose
scan-state delta is provably nothing — `parse_ident` guarantees a balanced
quoted identifier — so those bytes are crossed twice. And `StatementScan::feed`
spends a second `memchr2` pass per plain run counting parens, which the shared
statement rule needs and no `INSERT` statement does. Nearly 80% of the flat
profile is `memchr` across four needle widths ("insert-profile"), so both cuts
land on the dominant term.

**The remainder costs a user on fast storage most of the scan, and the device
figure that says so has been taken.** "Cold, the difference is gone" was always
a claim about the SATA SSD, whose ~557 MB/s hides a path running well above it.
It does not carry to the NVMe: cold on a 970 EVO Plus an `INSERT` scan is
**2.62× the device's own time** where the `COPY` path is 1.06×, so roughly 2.0 s
of a 3.40 s scan is spent where the disk is idle
([`measurements.md`](measurements.md), "Scan throughput by input shape", the
cold-NVMe table). That is what [`roadmap.md`](roadmap.md)'s goal of device-bound
"on hardware from HDD through NVMe" asks of the bulk-row path, and an `INSERT`
run does not meet it at the top of that range. **This entry is not retired to a
property**: the two named cuts above are real and untaken.

**Both cuts belong to P8, not to the phase that measured them.** Track A's row
reader has to find where each `INSERT INTO … VALUES (…);` statement ends, which
is this scan — extended rather than rewritten, so that
`standard_conforming_strings` is assumed in one place
([`roadmap-P8-format-coverage-inbox.md`](roadmap-P8-format-coverage-inbox.md),
"The statement-end scan Track A needs already exists"). That is the session
that opens `StatementScan` with a second caller's requirements in hand, and the
paren cut in particular cannot be shaped without them: a reader splitting a
`VALUES` tuple has its own view of whether the depth count is dead weight,
where a scan-only micro-benchmark does not. *Rejected: taking the two cuts inside the
scan-performance work, which its own rules allowed* — it admitted levers from
its own profiles as it ran, and did so three times. It is
refused because it would price both cuts against a workload that only *maps* an
`INSERT` run, months before the reader that gives the second cut its shape, and
would then have P8 re-open the same function anyway.

**Neither cut is measured, and the instrument is a profile rather than a
figure.** "insert-profile" puts nearly 80% of the flat profile in `memchr`
across four needle widths but does not split the prefix's double crossing from
the paren pass, so which of the two is worth taking is open. A profile answers
it in seconds and publishes no number, so the reading is not a slice of its
own.

**What made it mid-teens was the accumulation, and removing that is what
moved it.** Under the `ba2fc12` stamp the same scan read 9.19 s warm, 16.5×, and
1.89× the floor cold, because `feed_line` turned every line into a `String`
(`String::from_utf8_lossy(raw).into_owned()`) and `push_stmt_line` copied it
into a buffer that `statement_complete` then re-walked with
`chars().peekable()` — 58.2% and 17.3% of a warm `parse`'s user time against
**0.6%** in `CopyScanner::next_event` (see "Where a scan's time goes"). What
replaced it is `preamble::StatementScan`: one incremental, byte-level,
quote-aware scan carried across a run's lines, with no `String` and no buffer
behind it, crossing string values and comments with `memchr` and the run
between them with one `memchr3` for the next mode-changing byte plus a
`memchr2` count for the parens. `Mode::InsertRun` holds that scan in place of
the `String` it used to hold, which it can because a run's span carries a table
name and a row count and nothing reads its text.

**`Mode::Statement` still accumulates a `String` and still re-walks it per
line, and that asymmetry is the reason the fast path was worth building.** A
statement span carries its text, which `classify_statement` and
`extract_statement_cross_refs` read, so the buffer cannot simply go the way
`Mode::InsertRun`'s did; the per-line re-walk could be made incremental with a
second `StatementScan` beside the buffer. It is left because the two modes see
different volumes of the same file: DDL is a few megabytes even in a
784 GB dump ([`roadmap.md`](roadmap.md), "Project goals"), where an `INSERT`
run is the file. Nobody owns it, and nothing would promote it short of a
producer that writes gigabytes of statements outside a data run.

**The fast path declines every line it cannot decide, and `Builder::step`
decides those exactly as it did before.** `Builder::insert_run_line` hands back
a line whose first non-ASCII-whitespace byte is not ASCII, one starting `-`
(every boundary signal begins `--`), a blank one, and — at a statement
boundary — any line that does not restate the run's own `INSERT INTO <table>`
opening byte for byte. That last test is deliberately a *conservative* stand-in
for `parse_insert_target`: matching proves the line parses to the same table,
and not matching costs only the ordinary path. Both paths feed the same
`StatementScan`, and each line reaches exactly one of them, so which path saw a
line never changes how the run folds.

*Rejected:* a second scanner-level fast path in `scan.rs`, the mechanism
`State::InLargeObjectRegion` already is — which is what `KD9` was originally
filed against. The profile refused it before it was written: the scanner was
0.6% of the scan, so a third region state would have bought under a percent.
It was also the larger claim, since the scanner cannot know a run has begun
without `parse_insert_target`, which is statement classification and therefore
L2's, and unlike `BEGIN;`/`COMMIT;` (I12) an `INSERT` run's boundaries rest on
no line-anchored invariant. The supporting reading for the layer that was
chosen instead is the large-object region, which skips lines *unread* at
0.449 s per 3.00 GiB against `COPY`'s 0.532 — an `INSERT` run that touched no
bytes could not beat that, and it is now within a factor of five of it.

**Two builds of one source read ~10% apart on this input, and it was code
layout rather than code** — the finding that closed an earlier 9.85 → 10.87 s
cold and 8.37 → 9.19 warm move against unchanged bytes. `release` builds at
`b70589f` and at `ba2fc12` retired **114.62 G and 114.66 G instructions** for
the same scan, 0.03% apart, and spent **35.8 G and 40.0 G cycles** doing it,
with branch misses, cache misses, icache misses and frontend stalls all flat or
lower on the slower one. The whole 4.2 G-cycle difference sat inside the
`scan_buf` of the day, whose 293 instructions were byte-identical between the
two binaries and differed only in address. Building both with
`-C llvm-args=-align-all-functions=6` collapsed the gap to −1.5% and took both
below the faster one, while the same flag moved the control's `parse`,
`strings` and `typed` shapes not at all — so it was one tight character loop's
placement, not a general win. There was nothing to bisect to and nothing to
fix: what it changed is how a figure is read
([`measurements.md`](measurements.md), "Two builds of one source can differ by
layout").

**An `INSERT` run's end needs a string-aware scan, not a line-anchored check.**
A `pg_dump --inserts` value is a single-quoted SQL literal, and a value carrying
a newline puts the rest of its statement on the next physical line, which begins
`');` rather than `INSERT INTO`. So the end is found by tracking `'` (with `''`
doubling; `standard_conforming_strings = on` means there are no backslash
escapes) to locate real statement ends, then closing at the next TOC header.

*Rejected:* closing an `INSERT` run at the next line-anchored `--`. A value
containing a newline followed by `--` breaks it, and the difference never shows
up at fixture scale — so the unsoundness would have shipped untested.

An `INSERT`-format dump also emits a `TABLE DATA` TOC header for a table with
**zero** rows, so a data span containing no data at all is a shape the map
handles, not an anomaly.

`extract_statement_cross_refs` is deliberately **not** run over `INSERT`
bodies (`push_insert_run` calls `push_span` directly) — a text value containing
`" TO "` or `"GRANT "` would otherwise register as a role reference.

**The v17+ multi-entry large-object merge needs no per-entry re-inspection.**
`on_large_object_start`/`on_large_object_end` push no span; they extend a
`pending_large_objects: Option<(start, end, toc)>` field, and `push_span`
unconditionally flushes it first — so "no span was pushed since the last
`COMMIT;`" *is* "the next `BEGIN;` continues the same region". I12's
priority-band proof (`pg_dump_sort.c`'s `dbObjectTypePriorities`) is what makes
that sound: nothing but another `BLOBS` entry can legitimately arrive between
two of a file's large-object data entries. The merged span's `Span::toc` is the
*first* entry's header; each merged-in entry's own `Owner:`/`Tablespace:` still
feeds the cross-reference set independently.

### TOC enrichment

`Span::toc: Option<TocHeader>` is filled whenever the preceding comment block
contains a line matching `_printTocEntry()`'s grammar (I3/I18) — `-- Name:
<name>; Type: <kind>; Schema: <schema>; Owner: <owner>[; Tablespace: <ts>]`, or
its `-- Data for Name: ` sibling.

`parse_toc_header_line` is a fixed sequence of `split_once` calls on the literal
separators, **not** a general grammar, and a field that fails to parse just
leaves `toc: None` — the same graceful degradation header-less input already
produces. `--verbose`'s `-- TOC entry N (class C OID O)` / `-- Dependencies: …`
lines are ordinary comment lines to it. **The comment block is not a fixed
height** — three lines at minimum, more under `--verbose` — so a reader runs to
the closing `--` rather than assuming an offset.

**`Span::toc` means "the TOC entry this span belongs to", not "the TOC comment
this span starts with".** A follow-on statement — `ALTER … OWNER TO`, `ADD
MAPPING FOR`, `ALTER EVENT TRIGGER … DISABLE` — inherits the governing entry's
header from `Builder::governing_toc`, and `Span::toc_owned` records whether the
span carried the header text itself. **Any code reading `toc` to mean the
latter must check `toc_owned`**, or it gets a false positive for every
inherited follow-on.

*Rejected:* merging follow-ons into the object's span. It reduces specificity,
which the monotonic-coverage rule forbids, and it deletes the byte-exact
statement boundaries a future writer or a `--filter` would need.
*Also rejected:* leaving them unattributed. That puts object identity in
adjacency, where only a reader's eye can recover it, and it is what made the
TOC-coverage figure read ~50% on healthy input. Attribution by inheritance is
enrichment, which is the direction the standing rule permits.

`governing_toc` is updated by every `push_span`, keyed on the pushed span's
body: a plain statement or `Data` span becomes the new governing entry;
`Framing`, `Connect` and `VersionHeader` clear it. **The one place inheritance
must be vetoed after the fact** is `push_statement_span`: a mid-file `SET
default_tablespace = …;` only classifies as `Framing` once `classify(buf)`
runs, which is after the `Idle`→`Statement` transition that seeded the
inheritance, so `(toc, toc_owned)` is overridden to `(None, false)` there.

`classify` recognizes any complete statement starting `SET ` or `SELECT
pg_catalog.set_config(` (case-insensitive) as `Framing` wherever it appears,
including the `SET default_tablespace = …;` that `_selectTablespace()` emits
ahead of an individual object's definition. Treating both producers uniformly
was simpler than distinguishing them by position, and the cross-reference
extractor reads that statement's tablespace either way.

Two figures come out of this layer and they count **different things**:

- **`TocCoverage { attributed, spans }`** (an `Info` diagnostic) counts
  *attributed* spans — `toc.is_some()`, inheritance included.
  *Rejected:* counting only header-bearing spans. It reported ~50% on a
  healthy, fully-TOC'd dump, which cannot distinguish the degraded case the
  figure exists to name.
- **`pgdq info`'s `object kinds:`** is an object *census* and counts
  `toc_owned` spans — one per archive entry, so it reads "eight tables", not
  "eight tables plus their owner statements".

**Grouping is unimplemented and unassigned.** An object's trailing
`ALTER … OWNER TO` is still its own adjacent span (attributed, not orphaned).
If something later wants real grouping, the TOC's `Dependencies:` field —
captured only under `--verbose`, parsed by nothing — is the documented
mechanism.

**All three of `_printTocEntry()`'s prefixes are recognized**, and the two
functions that read them draw the line differently on purpose.
`parse_toc_header_line` treats `TOC_PREFIX_DATA` (`"Data for "`) and
`TOC_PREFIX_STATS` (`"Statistics for "`, a v18+ `--statistics` component)
alike — each is an optional prefix before `Name: `. `looks_like_toc_name_line`,
the boundary signal, accepts the stats prefix but not the data one. A
statistics entry heads an ordinary `pg_restore_relation_stats()` statement and
must leave the builder in `Mode::Statement`; a data entry must not.

**What the data-prefix refusal buys is narrow.** `saw_name` decides one thing
only: whether `Mode::Comment`'s close arm absorbs the block into the statement
that follows, or pushes it as its own span. On a default dump it is a **no-op
for `COPY`** — the close arm never runs, because the blank line after `--` is
absorbed in place and `scan::CopyScanner` intercepts the header line as
`Event::CopyStart` before `feed_line` sees it, so `on_copy_start` reads the
pending `TocHeader` out of `Mode::Comment` without consulting `saw_name` at
all. It earns its keep on a `--disable-triggers` dump (I31), where a statement
*does* intervene: absorbing there routes the entry into `push_statement_span`'s
`Framing` veto, and a leading `SET SESSION AUTHORIZATION DEFAULT;` classifies
`Framing` — so the entry is not merely misplaced but **destroyed**, taking TOC
coverage from 2/24 to 1/22. Refusing keeps it on a `Framing` span of its own,
which is worse than attributed and better than gone.

That dump is the **only** input on which the refusal changes an outcome, so it
is the only thing that can pin it:
`a_data_entry_keeps_its_own_span_when_disable_triggers_intervenes` in `map.rs`
is that test, and patching the predicate to accept the prefix fails it and
nothing else in the suite. A predicate whose only test restates it is a
predicate nothing checks.

Under `--inserts` a data entry heads an `INSERT` run, and `Builder::step`'s
`Mode::Comment` close arm opens `Mode::InsertRun` for it directly — the same
absorption from the same mode, for the data format the scanner does not
intercept, so this predicate never has to say yes. Free related fact: a
`STATISTICS DATA` header has no owner at all, not even a placeholder —
`dumpRelationStats` never sets `te->owner`, and `sanitize_line`'s NULL-hyphen
substitution turns that into the literal `-`, which `parse_toc_header_line`
already treats as "no owner".

Prefix recognition moves the *diagnostic*, never the tiling: an unrecognized
prefix costs two unattributed spans per data entry, and both prefixes are
recognized, so `fixtures/18/objects/stats.sql` reads 140/161 (87%) and
`fixtures/16/edge_cases/inserts.sql` 30/47 (64%). Object censuses are unchanged
either way — the entry was always counted, just against the wrong span.

**`--disable-triggers` defeats attribution for every data span in the file,
`COPY` and `INSERT` alike, and that is accepted.** I31 puts `SET SESSION
AUTHORIZATION DEFAULT;` and `ALTER TABLE … DISABLE TRIGGER ALL;` between the
`-- Data for Name:` block and the data, so neither `on_copy_start`'s
`Mode::Comment` arm nor the `INSERT`-run arm ever sees the entry: the comment
closes as its own `Framing` span, and `Framing` clears `governing_toc`.
Measured against 16.15, two tables: TOC coverage 2/24, the two being the
comment blocks themselves. What is lost is the coverage diagnostic and
`Span::toc` on data spans; nothing else — the table name comes from the `COPY`
header or the `INSERT INTO` line, the object census still reads `TABLE DATA:
2` off the comment spans, and roles still come off the entry's own `Owner:`.
It is also the only known shape that reaches `on_copy_start`'s `Mode::Idle`
arm, which passes `None` rather than inheriting `governing_toc` — unlike
`step`'s own `Mode::Idle` arm, which seeds `toc: self.governing_toc.clone()`.
Across the whole fixture tree there are **904 `COPY` spans and none
unattributed**, so no default-flag `pg_dump` output exercises it.

*Rejected:* fixing that arm on its own. It is two lines and no fixture can
observe the difference, which is what disqualifies it — a change no test can
see, to a rule this section states as a decision, is what the out-of-band
admission rule exists to refuse. It would also fix nothing by itself: the
`Data for` comment closes as `Framing` and so leaves no `governing_toc` behind
for the arm to inherit. All three changes land together or none do.

<!-- deficiency: KD1 -->
**The attribution loss above is deficiency `KD1`** (`../status/STATUS.md`,
"Known deficiencies"), and it is unowned.
The shape is opt-in — I31 emits nothing at all without
`--data-only`/`--section=data` — which is why it is not scheduled; the fix is
three coordinated changes, spelled out in `roadmap.md`'s "Future — wanted,
unscheduled".

### Cross-references (roles and tablespaces)

`Builder` accumulates both sets as it classifies. `push_span` reads
`toc.owner`/`toc.tablespace` off every span's `TocHeader` regardless of kind;
`push_statement_span` (the entry point every `classify` call site goes through)
first runs `preamble::extract_statement_cross_refs` over the statement text,
reaching the three shapes no TOC field covers — `ALTER … OWNER TO`,
`GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, and `SET
default_tablespace` (I19). Matching is by marker substring plus
`Cursor::parse_ident`: the same tolerance `parse_toc_header_line` documents,
sound because this only ever feeds enrichment, never a span boundary.

**The `PUBLIC` filter must be case-insensitive.** `Cursor::parse_ident`
lowercases every *unquoted* identifier, including the literal `PUBLIC` a
`GRANT`/`REVOKE`'s empty grantee list writes, so it arrives at the filter as
`public`. This is also an irreducible ambiguity in the dump text: `GRANT … TO
public;` unquoted means the pseudo-role to PostgreSQL's own grammar too — a
role genuinely named `public` can only be granted to when quoted, which `fmtId`
does and `parse_ident`'s quoted branch preserves case for.
`insert_role`/`insert_tablespace` are the single shared filters (`PUBLIC` and
`pg_default` are never recorded).

Per-database filtering is not implemented and is not a missing feature: a
caller filters `spans` by `Span::database` rather than `DumpIndex` growing a
third structure.

### Span text

`Span::text: Option<SpanText> { text, truncated }`, filled by
`map::attach_text(source, &mut spans)` and persisted. `Data` and `Unscanned`
spans **never** store text — which, not a scoping choice, is why `query` cannot
run cache-only.

**It is a pass over finished spans, not a slice taken as each span closes.**
*Rejected:* close-time slicing. Its rationale — the bytes are already in the
scan buffer — is true, but only the *driver* owns the buffer while only the
`Builder` knows where a span started, and a span can be the whole file
(`--inserts`). Close-time slicing therefore means feeding a retention watermark
from the builder back into `scan.rs`'s buffer management, in two drivers. The
pass gets an identical result, because text is a pure function of the span's
offsets with no coupling to buffer lifetime.

**It is not one read per span.** Spans tile, and text-bearing spans are exactly
the ones *between* `Data` blocks, so `attach_text` coalesces each contiguous
run into a single `read_range` — one read per gap, over schema-sized regions
the scan just walked — capped at `spans_in_run * TEXT_CAP` so a multi-gigabyte
`Unparsed` region is never pulled in whole. The mapping pass re-slices
wholesale at every checkpoint rather than incrementally, deliberately: the last
span's `end` grows as the scan advances, so its text has to be re-taken anyway.
On koji that is roughly 200 × 154KB against an hour-long scan.

## `DumpIndex`: one owner per fact

The through-line, and the thing to preserve: **no fact is stored twice.**

- **`spans` is primary.** `blocks()` is a filtered iterator
  (`SpanBody::Data(DataBlock::Copy(b)) => Some(b)`); `blocks_for`/`total_rows`
  build on it. There is no `blocks` field, so a block's byte offsets have
  exactly one owner.
- **`metadata: DumpMetadata` is a memoized derived view** —
  `preamble::dump_metadata_from_spans(&[Span])`, a pure function keyed on span
  kinds rather than line prefixes. Making that possible is why `SpanBody` grew
  `Connect`, `VersionHeader` and `AlterTypeAddValue`: the derived view needs
  the payload, not just "this was framing".
- **`roles`/`tablespaces: BTreeSet<String>` are the one exception**, and
  deliberately so: they are accumulated during the scan and persisted, not
  derived. Recovering a role reference from `Span::text` means re-parsing raw
  statement text on every load, unlike filtering an already-typed `SpanBody`.
  Both sets are stored flat and per-file.
- **`diagnostics: Vec<Diagnostic>` is `#[serde(skip)]`** and recomputed per
  load. Not persisting is load-bearing, not an optimization: a stored
  `CacheMtimeChanged` would replay a warning about a check *this* run performed
  successfully. `diagnostics_do_not_round_trip_through_the_cache` pins it.

*Rejected:* `spans` alongside a `blocks` field, with `Span::CopyData { block:
usize }` indexing into it. It gives the same byte offsets two owners, so "spans
sum to the file size" could be true while `blocks` disagreed — precisely the
failure the tiling invariant exists to catch.
*Also rejected:* storing a parsed `DumpMetadata` *and* raw text in spans (one
fact in two places), and storing only raw text and re-parsing per query
(throwing away work the scan already did). The derived view is built once when a
`DumpIndex` is produced or loaded and memoized as `#[serde(skip)]`, so it costs
nothing per query and cannot diverge from the spans.

`roles`/`tablespaces` are **complete only once `scanned_through` reaches the
file's size** — `DumpIndex::is_complete(size)`, the test they share with a
block's array-shape census — the same partiality `metadata`'s
`preamble_complete` carries,
for the same reason: a query that stops at its target
(`ScanExtent::UntilTargetSettled`, the default) never reaches a reference past
the stopping point. koji's `backup` role, the motivating case, is granted only
in a post-data `GRANT`. `ScanExtent::Full`, or a query after `pgdq parse`,
gives the complete set.

### Diagnostics: one severity scale, two types

*Rejected:* one enum spanning L1 file diagnostics and L2's per-column outcomes.
It cannot be built — `DumpIndex` is L1 and `resolve::ColumnResolution` is an L2
conclusion about PostgreSQL type semantics, so a `DiagnosticKind` variant
carrying one would have L1 name an L2 type.

What is shared is the `Severity` scale (`Ord`: `Info < Warning < Error`) and
the `{severity, kind}` shape. `Diagnostic` is the file-level type in L1;
`resolve::ColumnNote` is the per-column *record* in L2 — one per column, always
present, the ordinary case being a clean resolution. Its severity is derived
from `resolution`, not stored, for the same one-owner reason as everything
above. Every `DiagnosticKind` that exists is a property of the *file* —
tiling, cache, TOC coverage — which is the other half of why a type-semantics
conclusion is not one. The visibility argument for making it one does not hold
either: `--json` exports `DumpIndex`, which carries no `ResolvedSchema`, so a
column-level fact is absent from that export whichever type holds it.

*Rejected:* returning diagnostics alongside every result. It changes every
public signature for something most callers ignore. A caller-supplied sink (the
`tracing-subscriber` shape) is the right long-term embedder story and is
P6's; a sink can drain this list, so nothing here forecloses it.

Producers today: `TilingBroken` (`check_tiling`), `CacheMtimeChanged`
(`CacheMode::load`), `TocCoverage` (always `Info`), `CacheOffline`
(`Severity::Warning`, pushed by `load_offline` on every successful load),
`NonSeekableCompressedSource` (`Severity::Warning` —
`index::non_seekable_compression_diagnostic` shared by `build_index`,
`preamble_only` and `cache::status_from_file` so a `.xz` source with no more
than one block earns the same warning whether its seek table was just walked
or read back from a persisted cache).

### Reserved slots

Two `CopyBlock` fields are serialized but never constructed:
`sparse_index` (`SparseRowIndex`) and `column_stats` (`RowGroupStats`).
Populating either is additive, not a format-version bump — that is why they
were reserved. The types are placeholders; their real shape is the design work
of whoever fills them. See [`roadmap.md`](roadmap.md) for what each is for.

### The array shape census

An array column's Arrow type cannot be settled from the DDL: dimensionality
and lower bounds belong to the *value* (I21), and one column may hold `{1,2}`,
`{{1,2},{3,4}}` and `[0:2]={7,8,9}` in three consecutive rows. A full scan
therefore records what the file actually holds, so the schema can be decided
against evidence rather than optimism.

`CopyBlock::array_shapes` is `Vec<ArrayShape>`, one entry per column in
`header.columns` order, and each `ArrayShape` is the **minimum and maximum**
dimension count seen plus whether any value carried an `[lb:ub]=` prefix.
Combining blocks is `ArrayShape::merge` — min-of-mins, max-of-maxes, and a
prefix anywhere counts everywhere — which `index::union_census` folds over a
set of blocks.

**The vector is pre-sized from the header, and only a header-less block's can
end up short.** `on_copy_start` allocates one entry per declared column, so a
headered block's census is index-aligned with its columns whatever the rows
held. A header-less block — placeholder `column1…N` names, sized to the first
row — starts empty and grows by field index, and only on rows that pass the
pre-filter below, so its vector ends at the highest brace-bearing field. A
missing entry reads as the default `ArrayShape`, which is the same answer a
present-but-unconstrained one gives ("nothing constrained this column"), so a
consumer indexing defensively needs no special case.

**Both bounds, never just the maximum.** A column holding `{1,2}` and
`{{1,2},{3,4}}` records `(1, 2)`; a column holding only 2-D values records
`(2, 2)`. With a maximum alone the two are indistinguishable, so the mixed
column would resolve to `List<List<T>>` and then fail on every 1-D value in
it — a confidently wrong schema, which is strictly worse than the optimistic
path that reaches the same type while still treating a disagreeing value as an
error.

**Dimensionality is read off the raw field, with no array parser.** I25: an
`array_out` literal opens with exactly `ndim` braces, and any element whose
text contains a `{` is force-quoted, so no element can extend the run —
`{"c{d}"}` is one-dimensional. Neither `{` nor anything in the `[lb:ub]=`
prefix is in COPY TEXT's escape set (I15), so `ArrayShape::observe` reads a
still-escaped field directly. `{}` is what `array_out` writes for an empty
array of any dimensionality, so it constrains neither bound; a SQL NULL
likewise. A run longer than `MAX_ARRAY_DIMS` (PostgreSQL's `MAXDIM`, 6) did
not come from `array_out` at all.

*Rejected:* a quote-aware walk of the literal, or reusing the `nested.rs`
codec. Both are L2, both need the field decoded first, and I25 makes the
leading run exact — there is nothing for a parser to add.

**The census is type-blind, so it runs over every field.** `map::Builder` is
L1 and cannot know which columns are arrays; it records what each literal looks
like and leaves interpretation to the consumer. A composite's `(…)` and a
`json` column's `{…}` therefore reach `observe` too — the first contributes
nothing, the second records a depth nothing reads, since a `json` column does
not resolve to a list.

**A row is rejected wholesale before it is split.** An array literal always
contains a `{`, and only an `[lb:ub]=` prefix can precede it, so a row holding
neither byte costs one `memchr2` pass and no field splitting — not a
hand-rolled loop, because on the shape a real dump mostly has this is the
*only* census work there is, and the scalar loop it replaced cost roughly 20×
as much. `on_row`'s doc comment names the two measurements a reader regenerates
by patching that function.

**The cost is one tier in practice: the rows that pass the pre-filter.** On
brace-free data — the koji shape — a row pays the pre-filter alone, tens of
nanoseconds per 16-column row (60 ns at the current reading, 36 ns at the
`ba2fc12` stamp), a few percent of a scan reading from memory; a row that passes
pays field splitting and `observe` on top, 287 ns over 19 columns, +54% warm. Both
collapse to +0% and +1% cold on this SSD, where the device floor hides them
([`measurements.md`](measurements.md), "The census on brace-free rows" and
"…on array-bearing rows"). It runs unconditionally anyway: the alternative is a
query that cannot retype its array columns without a second pass over the same
bytes.

**Every mapping pass censuses, so a mapped block always carries one** —
`build_index`, `build_map` and `stream::map_forward`, under either
`ScanExtent`. `scanned_through` advances only at a `CopyEnd` watermark or EOF,
so a `CopyBlock` that reached the map was walked end to end: there is no
half-censused block and no state a later pass could repair. An empty vector
means "censused, saw no array-shaped literal", the same answer a vector of
unconstrained `ArrayShape`s gives, so it needs no separate representation.

*Rejected:* censusing only under `ScanExtent::Full`, so a cold query declines
the per-row work. The saving is the pre-filter alone — a cold query already
receives every row of every block it maps — which is tens of nanoseconds a row
with the bytes in memory and vanishes behind the device a cold query reads
from. What it cost
was a state no user could observe or repair: a dump mapped by a cold query and
*then* by a full one came out `is_complete` with its early blocks permanently
uncensused, because `map_forward` splices onto a prefix it does not re-read.
Reasoning:
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).

*Rejected:* gating instead — erroring up front with a `ShapeNotScanned` unless
a census exists. koji's array columns are certainly 1-D (they are entirely
`NULL`), so gating would fail a cold query and demand an hour of `pgdq parse`
to learn nothing.

*Rejected:* a transparent query-time prepass over the target blocks. That is a
*second* read added for the census's sake, silently doubling the cost of every
cold array query — the cost this project is least willing to hide. What the
census does instead is not that: the mapping pass exists regardless ([Query:
mapping and streaming are separate passes](#query-mapping-and-streaming-are-separate-passes),
so a queried block's bytes are read twice whatever the census does), and the
census is per-row work folded into a read already being performed.

### What the census decides, and who may believe it

`resolve_columns` takes the census beside the DDL and retypes each column's
`(DataType, NestedPlan)` pair from it — **the pair, never a half**, which is
the only place after `resolve_declared_type` where either changes. A caller
with no evidence passes `&[]`, which reads as "nothing constrained anything"
and leaves every column optimistically typed; that is the same answer an
all-default census gives, so the two never need telling apart.

Per column, and only for a column the DDL resolved to an array:

- uniform depth *d* → *d* nested `List` levels (`List<List<T>>` for `d = 2`);
- **mixed dimensionality, or any `[lb:ub]=` prefix → `Utf8View`**, with
  `ColumnResolution::VaryingArrayShape` saying so. Arrow lists are 0-based and
  have no lower bound, and no fixed list type is honest about a column holding
  both `{1,2}` and `{{1,2},{3,4}}`. The degradation is decided before the
  schema is fixed rather than discovered at row 40 million, and the column
  round-trips exactly as the string it already is;
- a leading brace run past `MAX_ARRAY_DIMS` → **not evidence at all**, so the
  column keeps its optimistic type and the offending row is an ordinary
  `FieldDecode`. Such a literal did not come from `array_out` (I25), which
  means the file is damaged or hand-edited; degrading the column would hide
  that behind a text column, and believing it would build a `List` nested as
  deep as the file asked for.

**A composite and a multirange are untouched**, though the type-blind census
records entries for them: a multirange's `List` is filled by `multirange_out`,
not `array_out`.

**The plan reaching the transform is always `Array(non-array)`**, and that is
a consequence of the refusal rather than a check here. The one declared shape
that would resolve to a nested `Array` — an array whose element type is itself
an array, whose literal is one brace deep and whose census therefore reads
`(1, 1)`, correctly (I26) — never gets that far: [type
resolution](#type-resolution) refuses it as `NestedArrayElement`. So the
transform never has to ask how the depth it is looking at was arrived at, and
a nested `Array` plan can only ever be one this transform built.

**A streamed schema needs no completeness test.** `table_stream` finishes
`map_forward`, then collects `matches` — every block bearing the target's
`(database, qualified name)` — and commits one schema from the union of their
censuses before emitting a row. So the evidence covers exactly the rows the
stream will hand back, on a cold query as much as on a full scan. That holds
even where the stop rule is fooled: the shape `stream::target_settled` cannot
detect (deficiency `KD6`, [One target per query](#one-target-per-query)) is
also one whose extra blocks the stream never replays.

*Rejected:* gating the streamed schema on `DumpIndex::is_complete` too. A cold
query can never satisfy it, so every query against a large dump would keep the
optimistic path and the `FieldDecode` refusal the census exists to remove —
while the evidence it needed sat in the blocks it had just walked.

**`is_complete` qualifies a *reported* schema instead.** `pgdq info` answers
"what is this table's schema" from an index alone, with no replay to bound the
claim; a mapped block's census is total, but one table's data can occupy
several blocks (I2), so a map that stopped short cannot speak for a block past
its frontier. `print_index` therefore passes `&[]` unless the index covers the
file — a rule rather than a second code path, since no CLI surface reaches the
case (`info --source` rescans when the cache falls short, and cache-only mode
refuses an incomplete cache).

**So a reported schema and a streamed one may legitimately disagree about one
table**, each true of what it describes: `info` lists per `COPY` block and
resolves each from that block's own census, where a query commits one schema
from the union over the blocks it will replay. A table whose first block holds
1-D values and whose second holds 2-D ones reads `List(Int32)` and
`List(List(Int32))` on two `info` lines and comes back as text from a query. No
fixture produces the shape.

<!-- deficiency: KD2 -->
**What survives all this is the array nested inside a composite** —
deficiency `KD2` —
or inside another array's element type. The census is keyed by column and has
nowhere to record a shape at that depth, so those stay on the optimistic path
however much of the file has been scanned: a multi-dimensional or
`[lb:ub]=`-decorated value there is a hard `FieldDecode` naming the column, and
scanning more of the file cannot help.

**Two escapes leave the rest of the table usable**, and neither reaches the
value typed. `--schema-mode strings`, which the message names, returns the
literal verbatim for the whole table and is the only route to that column's
data; and *not projecting the column* leaves every other column typed, since an
unprojected column is never decoded ([Projection](#projection)). Neither
survives *filtering* on the column with an ordering operator, which decodes the
field itself and raises the same `FieldDecode`. The message names only the
first, so the second is the manual's to state. A *top-level* array column does
not reach this at all, and neither does an array whose element type is an
array — that one is refused outright as `NestedArrayElement` (I26). Keying the
census by path is a roadmap "Future" item, deferred on frequency, and is purely
additive whenever it lands.

The cache format bumps whenever this shape changes, like any other persisted
field ([The cache](#the-cache)).

## The preamble grammar and `DumpMetadata`

`DumpMetadata` / `DatabaseMetadata` / `Extension` / `TypeDef` / `TypeKind` /
`CollationDef` / `ColumnDef` recover server and `pg_dump` versions, extensions,
user-defined types and collations, and each column's declared type **as the
literal string `pg_dump` wrote** — never a parsed pair — per the layering rule
that L1 stores what the dump said, not what a later layer concludes.

**Grammar approach: dispatch directly off six fixed line-start keywords**
(`CREATE TABLE`, `CREATE TYPE`, `CREATE DOMAIN`, `CREATE EXTENSION`, `CREATE
COLLATION`, `ALTER TYPE`).

*Rejected:* modelling `pg_dump`'s `-- Name: …; Type: …` TOC-comment grammar as
a separate segmentation pass. Keyword dispatch gives the same soundness
property the TOC design was after — never guess, only act on a fully-recognized
shape — because an unrecognized line (a TOC comment, a `SELECT
pg_catalog.binary_upgrade_*` call under `--binary-upgrade`, `CREATE FUNCTION`,
`ALTER … OWNER TO`, a blank line) is simply never matched to a trigger keyword
and ignored. The TOC comment later came back as an *enrichment* layer read off
the same lines, which is a different job.

Once a trigger line is seen, subsequent lines are appended (rejoined with `\n`)
until parens balance (respecting `'…'`/`''`-escaped literals) and the statement
ends in `;` — sufficient because `pg_dump` never puts a semicolon inside a
nested expression for any of these five shapes. A declared type string is
recovered by whitespace-tokenizing the tail after a column/field name and
stopping at the first column-constraint keyword (`NOT`, `DEFAULT`, `COLLATE`,
`GENERATED`, `PRIMARY`, `REFERENCES`, `CHECK`, `UNIQUE`, `CONSTRAINT`), since
`pg_dump` never puts a space inside a type's own parenthesized modifier
(`numeric(38,10)`); a `--binary-upgrade` dummy column's type comment
(`INTEGER /* dummy */`, I5) is stripped first.

**A column is a `ColumnDef`, and its third field is the `COLLATE` clause.**
`pg_dump` writes one only where the column's collation differs from its *type's*
default, so the field is `None` far more often than not — and `None` is a fact
about the type, not about the column (I37). The clause is kept **verbatim**,
`pg_catalog."C"` and all, for the same reason the declared type is: what it
means is L2's conclusion, and `crate::pgtype::comparison_for` is where it is
reached. A `CREATE DOMAIN`'s own clause is kept the same way, on
`TypeKind::Domain`, because it is the *type default* a column-level clause
exists to override.

**Finding it is a scan, not a lookahead.** `COLLATE` is one of `STOP_WORDS`, so
the type words still end there — but the clause is appended *after*
`DEFAULT`/`GENERATED` and after `NOT NULL` (I37), so the token following the
type is usually something else entirely. `extract_collation` walks the whole
fragment at paren depth zero, outside `'…'` and `"…"`, which is what keeps a
`DEFAULT 'collate me'` literal and a `CHECK ((v COLLATE "C") > 'a')` expression
from being read as the column's own.

*Rejected:* folding the collation into the declared type string. It is not part
of the type — two columns of one type can carry different clauses — and every
reader of that string would have to strip it back off.

**`CREATE COLLATION` is read for one option, and it is the only one that
changes an answer.** `CollationDef` is a name plus `deterministic`, collected
per database in DDL order on `DatabaseMetadata::collations`. `pg_dump` writes
`, deterministic = false` unconditionally wherever the catalog says so — it is
not gated on any dump option (I42) — so the clause's *absence* is the server's
own default and `deterministic: true` states what the file said rather than
guessing. That is the one equality divergence a plain dump carries outright:
under a non-deterministic collation `texteq`/`bpchareq` are not byte
comparisons, and ["Equality is typed too"](#equality-is-typed-too) is where the
register acts on it.

Only the *option list's top level* is read, through the same
`split_top_level_commas` the type grammar uses, because an ICU locale is an
arbitrary string the server does not re-quote — `locale = 'und, deterministic =
false'` is a locale and not a clause. The `CREATE COLLATION x FROM y` copy form
carries no option list and reads as deterministic; `pg_dump` never writes it
(it emits the full list for every collation it dumps, I42), so the shape is
reachable only from a hand-written file, where under-claiming costs a note that
is not printed rather than a wrong row set.

*Rejected: keeping the provider and the locale beside them.* Both are in the
file and nothing reads either: a collation's *order* is a function of the
provider version, which a plain dump never carries (I42), so storing the
provider would be storing a fact with no reader. The determinism is kept
precisely because it is the half the file settles.

*Rejected: recording only the non-deterministic ones.* The list is what the
dump declared, not what the register found interesting — the same rule that
keeps `ColumnDef::collation` verbatim — and a dump declares a handful of
collations at most.

**`types` is keyed on the type, not on the statement.** `pg_dump` writes a
completed C-level base type twice under one name — the shell, then the
definition (I11) — so `record_type` replaces an entry whose name is already
present, and a `TypeKind::Shell` never replaces anything. One entry per type
follows: `TypeKind::Base` is what a lookup of `public.mybase` finds, and
`pgdq info`'s `user-defined types` count says how many types the dump declares
rather than how many `CREATE TYPE` statements it wrote. A type that never got a
completion (`public.shellonly`) keeps its `Shell` entry, which no column can
name.

*Rejected:* deduplicating at the lookups instead — a helper every reader of
`types` must remember to use, where the count is precisely the reader that
forgot. The list is built in one place and read from several, so the rule
belongs where it is built.

**`--binary-upgrade` interleaves OID-preservation noise** — one to three
`SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid(…)`-style calls between
a TOC comment and the statement it announces, for *every* object type, not just
the enum-label case I6 documents. The parser tolerates this for free: none of
the noise lines start with a trigger keyword, so they are never absorbed into a
pending statement.

### Multi-database (`\connect`) segmentation

A `--create` dump's pre-`\connect` segment is never a real database entry
(`name: None` is reserved for a genuine non-`--create`, no-`\connect` dump) —
but its version headers, which print once ahead of the first `\connect`, are
carried forward onto the database that first `\connect` switches into (I9).
Every subsequent database segment carries its own independent version-header
pair, staged via a `pending_headers` field and consumed by `on_connect` on
*every* transition, not just the first.

### Bounded preamble-only reads

`index::scan_preamble` scans from byte 0 to the first `COPY` block header (or
EOF), which — per I1 — always closes out the *first* database's preamble
regardless of how many `\connect`-ed databases precede it, so its cost is
independent of dump size. **Both mapping entry points run it once, up front**,
whenever the first database's `preamble_complete` is not already known,
persisting immediately (not deferred to a later segment's save):

- `table_stream`, because a `Typed` query needs the DDL whether or not its own
  target table is ever reached. Under `CacheMode::Disabled` the scan still runs
  — `--dqcache none` disables persistence, not typing — and the save is a no-op.
- `map_file` (`pgdq parse`), only for a scan starting at byte 0, because an
  interrupted parse would otherwise bank blocks with no DDL behind them and
  report `not declared` — the final answer — for every column of them. The
  prepass's spans *are* the prefix a resumed scan splices onto, not something
  to splice onto one, so a resumed run skips it and takes the metadata its own
  earlier run stated.

This is also what makes `pgdq parse --preamble-only` cheap.

**Every later `\connect`ed database is stated at its own first `COPY` block**
by the mapping pass (see "`parse` resumes, and saves as it goes") — the same kind of
boundary the prepass stops at, per I1, and the only other point
`dump_metadata_from_spans` may be called at. So the prepass is not a special
first case; it is that rule's first firing.

### Per-`CopyBlock` database attribution

`CopyBlock::database: Option<String>` is populated from the mapping pass's
current database at `CopyEnd`, safe there since a `\connect` cannot occur
mid-block. `ResumeToken`/`InCopyResume` both carry their own
`database: Option<String>` so a resumed stream does not lose the state.

## Type resolution

`resolve_declared_type(declared, types) -> TypeOutcome` maps the built-in half
of the table below keyed on the base type name (typmod split off first — only
`numeric` inspects it, every other mapping being `Microsecond`-precision or
otherwise typmod-independent), and the user-defined half (`.`-qualified per I8)
against the querying database's own `CREATE TYPE`/`CREATE DOMAIN` list,
recursing through domains with **no cycle guard needed** (PostgreSQL cannot
create a domain over a type that does not exist yet).

`TypeOutcome` is `Mapped(DataType, NestedPlan)`, `Unknown`,
`OpaqueElementType`, `NestedArrayElement`, `OpaqueBaseType`, or `EmptyEnum`. The `Mapped` pair has
**one producer** — nothing outside `pgtype.rs` builds either half of a nested
column's pairing, which is what keeps the two trees in agreement without a
third structure enforcing it (see "Nested columns" below for what the plan is
for).

**The four container families resolve through this same function,
recursively**, so nesting composes with no special case: `public.comp[]` is
`List<Struct<…>>`, a composite with a `text[]` field is
`Struct<…, List<Utf8View>>`. A leaf whose own type does not map — `inet`,
an opaque base type, an unknown — is `Utf8View` **in that position**, exactly
as it would be at top level, so the recursion introduces no failure mode of
its own. The walk needs no cycle guard and no depth limit for the same reason
the domain walk never did: PostgreSQL refuses to create a type that contains
itself through composites, ranges, domains or array elements (I24).

*Rejected:* one-level-only containers, with a scalar-element requirement and a
whole-column `Utf8View` fallback otherwise. The depth check is more code than
the recursion it forbids, and it strands `text[]`-inside-a-composite, which
the recursion handles for free.

### The bar: "the dump alone determines the value"

A type is mapped only if its COPY TEXT rendering round-trips without consulting
anything outside the file. Everything else stays `Utf8View` with a diagnostic
naming the *kind* of unknown. Same asymmetry the `COPY` grammar chose: not
recognizing something is recoverable and inspectable; misreading it is not.

The koji census says the bar is not restrictive in practice — `integer` (246
columns), `text` (108), `boolean` (56), `timestamp with time zone` (38),
`character varying(n)` (17), `bigint` (4), `double precision` (3) account for
almost every column in a real 75-table schema.

| Declared type | Arrow type | Notes |
|---|---|---|
| `smallint`, `integer`, `bigint` | `Int16`, `Int32`, `Int64` | |
| `oid` | `UInt32` | PostgreSQL's one unsigned integer type, and `oidout` writes `%u` (I39) — the ADBC driver's `Int32` turns every OID at or above 2^31 negative |
| `boolean` | `Boolean` | Rendered `t` / `f` |
| `real`, `double precision` | `Float32`, `Float64` | `extra_float_digits = 3` guarantees exact round-trip (I4). `NaN`/`Infinity`/`-Infinity` parsed explicitly — Rust accepts `inf`/`NaN`, not PostgreSQL's spellings |
| `numeric(p,s)` | `Decimal128` (p ≤ 38), `Decimal256` (p ≤ 76) | `NaN` bypasses PostgreSQL's precision/scale check and is reachable through *any* numeric column, typed or not — a `FieldDecode`, same as the untyped column |
| `numeric` (no typmod) | `Utf8View` | Arbitrary precision, plus `NaN`/`Infinity`, have no Arrow decimal representation |
| `text`, `character varying(n)`, `character(n)`, `name` | `Utf8View` | The zero-copy path |
| `date` | `Date32` | |
| `timestamp without time zone` | `Timestamp(Microsecond, None)` | Wall-clock reading, no offset in the data |
| `timestamp with time zone` | `Timestamp(Microsecond, Some("UTC"))` | Offset explicit in the data and normalized to UTC (I4) |
| `time without time zone` | `Time64(Microsecond)` | |
| `time with time zone` | `Utf8View` | Offset semantics map to no Arrow type |
| `interval` | `Interval(MonthDayNano)` | Months, days and a time part, independent, which is PostgreSQL's own `Interval` struct. Two value classes have no encoding and are a `FieldDecode`: v17's `infinity`/`-infinity` (I34), and any interval whose time part exceeds `2562047:47:16.854775807` — PostgreSQL's field is `int64` *microseconds* against Arrow's `int64` nanoseconds, and nothing normalizes hours into days (I40). See below |
| `uuid` | `FixedSizeBinary(16)` | Canonical 36-char form |
| `bytea` | `Binary` | `\x48656c6c6f` after COPY unescaping |
| `json`, `jsonb` | `Utf8View` | Arrow has no JSON type |
| `inet`, `cidr`, `macaddr`, `macaddr8` | `Utf8View` | |
| enum (`CREATE TYPE … AS ENUM`) | `Dictionary(Int32, Utf8)` | Only when the label set is non-empty |
| domain (`CREATE DOMAIN`) | base type's mapping | Resolved transitively |
| `T[]`, in any of the six spellings (I28) | `List<resolve(T)>`, retyped from the census | Dimensionality belongs to the *value* (I21), so the DDL's answer is optimistically 1-D and [the census](#the-array-shape-census) settles it; a value that disagrees with what commits is a `FieldDecode`, not a reshape |
| composite (`CREATE TYPE … AS (…)`) | `Struct<` one field per declared field, in declaration order `>` | Zero fields included (I23) — `()` is a real value that round-trips |
| range (built-in or `AS RANGE`) | `Struct{lower: S, upper: S, lower_inclusive, upper_inclusive, empty}` | The fifth field is not redundant: `empty` and `(,)` both have absent bounds |
| multirange | `List<` the range struct `>` | Same Arrow type as `S[]`-of-range, different literal — see `NestedPlan` |
| `int2vector` | `List<Int16>` | The one built-in whose Arrow type is a container and whose name is not spelled like one. Same Arrow type as `smallint[]`, different literal — space-separated `int16`s with no braces, no quoting and no NULL element (I47) — so it too travels with a `NestedPlan`. See below |
| an array whose element is opaque | `Utf8View` | `box`, `TypeKind::Base`, `TypeKind::Shell`, through any chain of domains — `ColumnResolution::OpaqueElementType`, below |
| an array whose element is itself an array | `Utf8View` | `CREATE DOMAIN d AS T[]` and a column of `d[]`, through any chain of domains — the literal is one brace deep (I26), so its depth and the column's would disagree; `ColumnResolution::NestedArrayElement`, below. **Not** `integer[][]`, which is a spelling of `integer[]` (I28) |
| an array column whose values disagree on shape | `Utf8View` | Mixed dimensionality or an `[lb:ub]=` prefix, read off [the census](#the-array-shape-census) — `ColumnResolution::VaryingArrayShape` |

Microsecond precision throughout, because that is PostgreSQL's storage
resolution. **Every Arrow field is nullable**, regardless of a `NOT NULL` in
the DDL.

**Two columns carry a canonical Arrow extension name**, which is the part of
the mapping the `DataType` column above cannot hold: `uuid`'s field is stamped
`arrow.uuid` and `json`/`jsonb`'s `arrow.json`, through any chain of domains.
Neither changes a byte — both extensions' storage types are exactly what we
already emit — and both let a consumer tell a UUID from sixteen arbitrary
bytes, and JSON from any other string, without asking what the declared
PostgreSQL type was. `pgtype.rs`'s `extension_for` is the walk (the same one
`comparison_for` makes) and arrow-rs's own `Field::try_with_extension_type`
does the writing, so the metadata's spelling is the crate's rather than ours —
which matters more than it looks, since `arrow.json` requires its metadata key
to be *present and empty* and a reader calling arrow-rs's
`try_canonical_extension_type` rejects a field where it is absent.

**Only a top-level column's field is stamped, and only while it is `Mapped`.**
A `uuid` inside a composite or an array element keeps its
`FixedSizeBinary(16)` unnamed: the nested `Field`s are built by this module's
shared type constructors, and a `uuid[]` column's own field is a `List`, which
is not a UUID. A column the census took back to `Utf8View` holds an
`array_out` literal rather than the value its declared type names, so it
claims nothing either. None of this is load-bearing for decoding — the name is
a claim *about* the bytes, never an input to producing them.

***`interval` maps to the struct, and the two values that do not fit are a
`FieldDecode`.*** Arrow's `Interval(MonthDayNano)` carries months, days and a
time part as three independent fields, which is exactly PostgreSQL's
`Interval`, so the mapping is the struct rather than a reading of it. Two value
classes have no encoding in it and decode as failures, the way `Date32`'s
missing infinity and `Decimal128`'s missing `NaN` already do: an infinite
interval (v17, I34), which sets every field of PostgreSQL's struct extremal;
and a time part past `2562047:47:16.854775807`, where PostgreSQL's `int64`
*microseconds* run out of Arrow's `int64` nanoseconds — a thousandth of the
range, and reachable by an ordinary unnormalized value like `interval
'100000000 hours'`, since nothing normalizes hours into days (I40).
`--schema-mode strings` is the recourse, as it is for the other two.

Nothing about the *text* is in question: `pg_dump` pins
`INTERVALSTYLE = POSTGRES` on its own connection (I4), so `interval_out`'s
grammar is determined, and `decode.rs`'s `interval_parts` is one reading of it
shared by the decoder and by the ordering's fused span — which is what keeps a
field the decoder refuses and a literal the filter refuses the same set.
`render_interval` is `EncodeInterval`'s inverse, sign rules included.

*Rejected: keeping it `Utf8View`, on the grounds that the two value classes are
unbounded where `numeric(p,s)`'s `NaN` is rare by construction.* A typmod is a
promise the DDL makes about the range and `interval` has no typmod, so nothing
bounds how often the fallback would fire — which is a real asymmetry and is not
what decides it. What decides it is that the floor is a claim about what a user
could otherwise get: the ADBC driver returns `month_day_nano_interval` for this
type, so text is below the floor with no stance available for it, and the two
lost classes are the shape the register already carries for `date`,
`timestamp` and `numeric(p,s)` rather than a new kind of cost. The driver pays
for the same mapping more dearly — its reader guards the multiply against
`kMaxSafeMicrosToNanos` and fails the whole batch with `EINVAL`, an infinite
interval included, by a message about nanosecond overflow rather than about
infinity — where one field is lost here.

*Rejected: a per-column census deciding the Arrow type, so that a column with
no such value is typed and one with them stays text.* It is answerable from the
values and a statistics pass would compute it for nothing, which is why it was
filed at the statistics phase while the type was text. Mapping unconditionally
makes the question moot: a data-dependent Arrow type would buy back the two
classes at the cost of a column whose type cannot be known before the scan,
which is a price the array-shape census pays because a wrong `List` depth is a
wrong *answer*, and here the alternative is a named failure on one field.

***`int2vector` is a container the mapping table reaches through
`builtin_scalar`.*** Its value space is exactly `List<Int16>`'s: I47 states
that `int2vectorout` writes space-separated `int16`s with no quoting and no
escaping, and that the type has no NULL element at all, so nothing about a
value is lost and nothing about the text needs a wrapper. It is a floor row —
the ADBC driver answers `list<item: int16>` — and closing it is what leaves the
floor holding with no exception beyond the three stances.

**Three things are decided per declared type and only two of them come out of
that one arm.** `builtin_scalar` answers the Arrow type and the comparison in
one tuple, which is what makes a mapping added without a comparison a compile
error; the **literal form** is not in the tuple, because it is the one thing
neither of the other two determines — a `smallint[]` column and an
`int2vector` column are the same `List<Int16>` and are written in different
grammars. So `map_builtin` carries a one-line table from the declared name to
its `NestedPlan`, and a `debug_assert` pairs the two: a built-in mapped to a
`List` or a `Struct` with no plan named there would be filled by a scalar
builder and would read every value as text.

**So closing a built-in type is one indivisible change, whatever a plan says.**
The arm carries the Arrow type and the comparison, the `NestedPlan` table
carries the literal where the type is a container, and [the register-to-oracle
reconciliation](#the-register-to-oracle-reconciliation) demands an oracle case
for the new declared name — so landing the arm and deferring any of the other
three fails a check rather than shipping a half-mapped type. Two things do
separate cleanly, and they are the seams to cut at: the **fixture column** the
type needs, which comes first so the mapping is checked against bytes it did not
produce, and any **render-back** rework a new value class forces, which is a
change to an already-tested path rather than a new one.

*Rejected: widening `builtin_scalar`'s tuple to a triple, so the plan comes out
of the arm with the other two.* Both Python checks parse that tuple's first
element and would have survived it, so the cost is not the parse: it is that
every one of the two dozen scalar arms would then have to write
`NestedPlan::Scalar` to say nothing, for the one arm that has something to say.
*Rejected: deriving the plan from the `DataType` the arm yields, since exactly
one built-in arm returns a `List` today.* It is true today and it is the
inference `NestedPlan` exists to deny — the enum's whole premise is that one
Arrow type can have several literal forms, so a second container arm would
inherit the first's grammar silently. The assertion above checks the pairing
without deriving it.

*Rejected: carving `int2vector` out as a catalog type no real schema declares.*
Neither koji nor any fixture had one before this work, which is true and is not
a stance the floor rule admits: the three it does admit are "the driver
answered opaque", "the two encodings denote different values" and "below by
decision", and none of them fits. The codec is the cheapest in the type system
— `split(' ')` and `i16::from_str` — so the honest reason to carve it out would
have been effort, which is not one of the three.

*Rejected: hand-writing the two `ARROW:extension:*` keys, to avoid naming
`arrow-schema` as a dependency of its own.* `arrow::datatypes` re-exports the
type list and not the `extension` module, so the canonical types are reachable
only through the crate underneath — but four lines of hand-written metadata
would also have to restate two storage-type rules and `arrow.json`'s empty
metadata value, and getting that last one wrong produces a field arrow-rs
itself refuses to read back. The check being upstream's `supports_data_type` is
the point of the dependency, not a side effect of it.

**Looking up a declared type is a two-way split on the qualified name** (I8).
`pg_dump` empties `search_path`, so built-ins are written bare and
user-defined types schema-qualified (`public.mood`) — matching the form their
own `CREATE TYPE` uses. A `.` in the declared type is therefore a cheap first
discriminator.

**Enums and the `--binary-upgrade` trap.** Labels are recoverable in every
mode: a `--binary-upgrade` dump emits an empty `AS ENUM ()` body and follows it
with one `ALTER TYPE x ADD VALUE '<label>';` per label, still in the preamble,
still in the same TOC entry (I6). The parser reads both forms. This is what
makes the metadata pass a **hard prerequisite** for typing, not just an `info`
nicety.

**Ranges and multiranges.** PostgreSQL's six built-in range types and six
built-in multirange types are special-cased as bare names in `map_builtin`,
since — unlike composites — they appear unqualified and never reach the
`.`-qualified user-type path at all. A range's auto-created multirange
companion is **never itself the subject of a `CREATE TYPE`** (I10); its only
trace in the file is the `multirange_type_name = <name>` parameter inside the
range's own DDL, so `TypeKind::Range` carries the companion name and the lookup
honours it. Discarding that parameter would make the information unrecoverable
without re-scanning.

`TypeKind::Range` keeps a third parameter for the same reason: `canonical`,
which `pg_dump` writes for a range type whose `rngcanonical` is set (I10) and
which the arm's `key = value` split otherwise discards. Nothing reads its
*value* — a user's canonical function is arbitrary server-side code — but its
presence is what makes a column of that type refuse every comparison, in
"Nested columns compare structurally" below. The three other parameters
`dumpRangeType` can write (`subtype_opclass`, `collation`, `subtype_diff`) are
still stepped over.

**Twelve built-in names carry hardcoded subtypes, not six.**
`TypeDef::Range::subtype` is populated only for a user-defined range;
PostgreSQL keeps a built-in's subtype in the catalog rather than in DDL text,
so `int4range`→`integer` … `daterange`→`date` live in `builtin_range_subtype`
— **and the same six subtypes again for the PG14+ multirange companions**,
which produce a different Arrow type and a different plan from the same
subtype. Letting the multirange half inherit the range half's answer yields a
`Struct` where the file holds `{[1,10),[2,3)}`, and fails at the first row.
A user range's auto-created companion has no `CREATE TYPE` of its own at all,
so its bound type comes from the range that names it in
`multirange_type_name`. A range whose parameter list the grammar could not
read keeps the struct with `Utf8View` bounds rather than losing the shape.

**A composite's declared field list is all-or-nothing.** `parse_create_type`
yields `Composite { fields: None }` for a body holding any unparseable
fragment, and the column then resolves `Unknown`/`Utf8View` like anything else
the grammar does not recognize. `record_out` is positional and carries no
field names (I23), so — unlike a `CREATE TABLE` column list, which
`resolve_columns` joins against the `COPY` header *by name*, making a dropped
column merely `NotDeclared` — there is no join to recover a dropped field: a
three-field type parsed as two would refuse every row of entirely valid data,
and relaxing the field-count check would decode field 3's text as field 2's
type. The `CREATE TABLE` path keeps its `filter_map`; the asymmetry is
deliberate.

The field is `Option<Vec<…>>` because two states must stay apart and only one
of them is reachable: `Some(vec![])` is a zero-field composite, a real type
whose `()` values round-trip (I23), while `None` is a body the grammar could
not read — which `pg_dump` does not emit, since `parse_column_fragment`
returns `None` only for an empty fragment, a non-identifier name, or an empty
type-word list. So the `None` arm is a representable state with no input
behind it, and it deliberately earns **no `ColumnResolution` variant of its
own**: the column reports the ordinary "no mapping for this build" reason,
which is true but not specific, rather than a label naming a shape nobody has
seen. *Rejected:* dropping the whole `TypeDef` when the body will not parse,
which needs no unreachable state at all — but then the type vanishes from
`pgdq info`'s census and the column's reason becomes indistinguishable from a
type the dump never declared, which is strictly less information for a case
that cannot occur.

**Two array shapes are refused, both landing on `Utf8View`, and both decided in
`resolve_array` off the terminal of one `domain_terminal` walk** — because a
domain's own DDL records neither the delimiter it inherited nor the array-ness
of its base, so what decides either refusal is visible only at the walk's end.

- **An opaque element type**, tested after domain unwrapping rather than
  against the declared string (I22): a domain inherits its base's `typdelim`
  and records nothing about it, so `CREATE DOMAIN d AS box` makes `d[]`
  semicolon-separated while being named neither `box` nor `TypeKind::Base`.
  The separator stays hardcoded to `,`, and such an element resolves to
  `Utf8View` anyway, so `List<Utf8View>` would recover nothing a plain string
  does not — only a way to split on the wrong character, which round-trips
  byte-for-byte while being wrong.
- **An element type that is itself an array**, which is what keeps
  `NestedPlan::Array` meaning one thing. `CREATE DOMAIN d AS integer[]` with a
  column of `d[]` is the only DDL shape reaching a nested `Array` plan at all
  (I26; `integer[][]` does not — it is a spelling of `integer[]`, normalized
  before the refusal is tested, I28). Its value is written **one brace deep**,
  `{"{1,2}","{3}"}`, since `array_out` force-quotes any element containing `{`
  (I25), so the literal's brace run and the column's `List` depth are
  independent for this shape alone — and `Array(Array(…))` would have to mean
  both *one literal, two dimensions* (what the census produces) and *one
  literal whose elements are literals*. That is the collision `NestedPlan`
  exists to prevent, one level down.

**Their order decides a label, not a type**, and it is opaque-first because
`OpaqueElementType` is the stronger statement. No input reaches both: the
opaque test matches a bare type name where the array test matches that name
with bounds appended, so an array over a domain whose base is `box[]` answers
`NestedArrayElement` — true, but silent about the delimiter. Answering I22
instead means testing opaqueness recursively through the element's own array
levels: a behaviour change, for a better diagnostic on a shape `pg_dump` cannot
write (I21).

**Both compose into a composite for free.** A composite field of a refused type
is a `Utf8View` field inside an otherwise typed `Struct`, the rule every
unmapped leaf already follows — and with no nested `Array` plan reachable from
the DDL, [the census's](#the-array-shape-census) transform has no such shape to
guard against.

*Rejected:* refusing any array whose element does not resolve to a mapped Arrow
type. It closes the same hole and pays by degrading `inet[]`, `money[]` and
every unknown-element array to a whole-column string, where `List<Utf8View>`
recovers the element boundaries and splits on the right character.
*Rejected:* parsing `DELIMITER` out of `CREATE TYPE` into `TypeKind::Base` — it
handles the trap instead of removing it, and buys a `List<Utf8View>` over
values that are opaque by construction. *Rejected:* reusing
`OpaqueElementType` for the second refusal. The mechanism matches, but the
label would lie: `integer[]` is not opaque, it is understood and declined.
Opaque means *never improves*; this means *yes, if anyone needs it*.
*Rejected:* splitting the plan variant into a dimensional `Array` beside an
element-is-a-literal one, teaching `append_typed` to recurse into
`decode_array` per element with `render_field` inverting it. It is the complete
answer and it is contained, but it buys `List<List<T>>` over a shape almost
nobody declares by putting a second meaning into the one type whose purpose is
keeping meanings apart, and it lands in `batch.rs`. It stays available and is
strictly additive: it only ever touches columns this refusal leaves `Utf8View`.

**Array dimensionality is not in the catalog** (I21): `integer[][]` and
`integer[3]` both come back from `pg_dump` as plain `integer[]`, and one column
may hold values of differing dimensionality and lower bound. So a decoder infers
nesting from the literal's own brace structure, never from the declared type —
and **every array declaration is normalized to its element type plus one array
level** before anything reads the string. `integer[]`, `integer[3]`,
`integer[][]`, `integer[3][4]`, `integer ARRAY` and `integer ARRAY[4]` are one
type, with the bracket count and bounds discarded rather than recorded
somewhere a dump could carry them (I28). `pg_dump` writes only the first
spelling, so this serves the hand-written and other-producer input path
(`roadmap.md`, "The input contract is valid PostgreSQL"): reading a spelling
more literally than PostgreSQL does says something false about the column
rather than declining to answer it.

The normalization follows the `Typename` grammar rather than approximating it,
in both directions: the bracket run is unbounded (a declaration's bracket count
is not `MAXDIM`-limited and carries no meaning), while the `ARRAY` keyword
takes at most one bound, in the `[n]` form only, over a bare type name. Text a
server would reject — `integer ARRAY[4][5]`, `integer[abc]` — resolves
`Unknown`, because inventing an array type for input PostgreSQL refuses is the
same mistake pointed the other way.

It lives in `pgtype.rs` at two call sites: `resolve_declared_type`'s entry,
which a composite field, a range bound and a domain's base type all reach
through, and `resolve_array`, which reads the domain walk's terminal —
`CREATE DOMAIN d AS integer ARRAY` is as legal as any other spelling and the
walk stops on whatever the DDL wrote. *Rejected:* normalizing in `preamble.rs`
at parse time. `ColumnNote::declared` carries the raw declared string so `pgdq
info --verbose` can print what the file says beside what we made of it, and a
parse-time rewrite would have pgdq quietly editing the user's DDL in the one
place the raw text is the entire point.

A quoted type name is never misread as an array (I29). A name may legally
contain the array metacharacters, and `pg_dump` writes it quoted wherever it
appears, so the closing quote is what the suffix strippers bail on: `s."x
ARRAY"` is a scalar and `s."x ARRAY"[]` sheds only the bound outside the
quotes. Such a name costs a weaker type rather than a wrong one — the lookup
misses, because `TypeDef.name` is dequoted while the declaration is not, and
the column resolves `Unknown`.

<!-- deficiency: KD4 -->
**That is deficiency `KD4`**, and it is unowned. No spelling is *misread* and every
value still decodes as the text the file holds, so what is lost is strength,
not correctness; it is unreachable from any dump whose type names are ordinary
identifiers, which is every fixture and the koji sample. Fixing it means a real
type-name tokenizer, a roadmap "Future" item and strictly additive.

### The floor: the ADBC driver's answer bounds ours

**The Arrow type the Arrow ADBC PostgreSQL driver returns for a declared type
is a *floor*: wherever it yields a real Arrow type, ours is never a widening of
it.** The table above is better than the floor almost everywhere, and doing
better is expected; what the floor buys is that doing *worse* becomes a defect
with a name, rather than an unbounded "every built-in type, eventually" that
nothing can ever report progress against. `scripts/floor_mapping.py` is the
join, and [the floor oracle](#the-adbc-floor-oracle) is the evidence it joins
against.

**The floor is a shipped release, not upstream main** — `apache-arrow-adbc-24`,
Python `adbc_driver_postgresql` 1.12.0. A floor is a promise about what a user
could otherwise get, and users get releases; measuring against code nobody can
install would declare us below a floor that does not exist in the world.
`scripts/pyproject.toml` pins the version, every row of
`fixtures/<major>/adbc/floor.tsv` records the version it was taken with, and
the reconciliation asserts the two are equal — so bumping the pin is the
deliberate act that obliges re-taking the sweep, and the two cannot drift
silently. *Rejected: checking the rows against whatever `uv` resolves as
latest.* It turns somebody else's release into a spurious local failure, and
makes the check fire on an event nobody in this project chose. Upstream main is a watch item: two unreleased commits there give
`uuid` a `FixedSizeBinary(16)` and stamp `POSTGRESQL:type` on every non-root
field, which is what release 25 will likely ask for and is not what this floor
says.

*Rejected: measuring against main, so the mapping is ahead of the curve.* It
inverts what a floor is for — the point is that no user can do better
elsewhere, not that we match the newest unreleased commit — and it makes the
evidence unreproducible, since main has no artifact to pin.

**The rule holds where the driver yields a *non-opaque* Arrow type and the two
encodings denote the same value. Everywhere else the floor is *undefined*, not
violated — and every row outside it declares why.** Two of those classes are
placed by a column of the oracle file itself, with no per-type line, which is
what keeps this coverage unbounded while the work stays bounded:

- **`arrow.opaque`.** The driver's bottom is raw *binary* wire bytes plus a
  type name; ours is the file's own text. The two are incomparable and ours is
  the more useful — nobody can read ADBC's `inet`, and anybody can read
  `192.168.1.0/24`. It is a long tail: `bit`, `varbit`, `inet`, `cidr`,
  `macaddr`, `macaddr8`, `uuid`, `xml`, `tsvector`, `pg_lsn`, `time with time
  zone`, the geometric family, every range and multirange, `oidvector`, and
  **`numeric`**, whose `string` answer is tagged opaque and so is not a floor
  row at all.
- **A refusal.** `aclitem` and `gtsvector` answer `E42883` at every major — the
  server has no binary output function and binary is the only encoding the
  driver reads — so the floor for them is *nothing*.

The remainder is 24 rows a major, and 21 of them our mapping simply meets. The
three that do not each carry a stance, in the reconciliation's own table,
because they are not one kind of thing and a flat exception list could not say
that `regproc` is unanswerable while `money` is refused:

| Type | Theirs | Ours | Stance |
|---|---|---|---|
| `money` | `int64` | `string` | **below by decision** — `KD13`, below |
| `regproc` | `int32` | `string` | **different encodings**: `regprocout` writes the function's *name*, schema-qualified where the bare name would not resolve and `-` for `InvalidOid`, where the binary encoding ADBC reads is the OID. So "narrower than `Int32`" is not a question the text can be asked — stated over the `reg*` family, of which this is the one member the driver gives a real Arrow type to |
| `oid` | `int32` | `uint32` | **narrower**, which the rule permits: `oidout` is `snprintf("%u")` (I39), so the driver's `Int32` turns every OID at or above 2^31 negative. Only a *wider* answer is a violation |

**A fourth stance exists and no row carries one.** `waiting` is what a row a
slice of the open phase is about to close declares, and the two that had it —
`interval` and `int2vector` — both closed, so the table is three rows and the
mechanism is exercised by `test_floor_mapping.py` against a synthetic row. It
is kept rather than deleted with its last user because it is what the next
such row will be held to.

**A waiting row is a disposition, not a register entry.** `KD<k>` means known
and *not being fixed now*; a row a slice of the open phase closes is being fixed
now, and allocating a number only to strike it inside the same phase would turn
the register into a progress tracker. The disposition names the slice instead,
the reconciliation resolves that name against `STATUS.md`'s checklist so a
re-slice is reported rather than owed on discipline, and the row closing is what
deletes the disposition — the check reports a row that has started meeting the
floor while a stance still stands over it. `money` is the contrast that draws
the line: it earns an entry precisely because no slice will ever close it.

**The rule is over declared *types*, and it is scoped to fidelity.** Two places
where we are strictly wider than ADBC are outside it for that reason, and
neither acquires a stance:

- **The array census.** ADBC names `List<T>` from the type alone and *flattens*
  a multi-dimensional array into a one-dimensional list; we resolve `List<T>`
  optimistically and let [the census](#the-array-shape-census) demote a column
  of mixed dimensionality or `[lb:ub]` decoration to `Utf8View`. Strictly read
  we are wider; their narrower type is a wrong answer, so scoped to fidelity we
  are not. `KD3` keeps its own stance and this rule adds nothing to it.

  *Rejected: making the shape-general `Struct{dims, lbounds, elements}` the
  automatic demotion target, so the floor holds with no carve-out and `KD3`
  closes.* It is the tempting answer and it over-reaches. That representation is
  scoped as a **caller-selected knob** (`roadmap.md`, "Future — wanted,
  unscheduled"), so making it what a demotion silently produces pre-empts the
  choice it exists to leave with the caller — and it takes `List`, the signal
  every generic Arrow consumer reads as "this is an array", off columns that
  have it today. The knob stays wanted; a floor rule is not the thing that
  should decide it.
- **Name resolution.** A type name that needs quoting (`KD4`), or a composite
  whose body the grammar could not read, resolves `Utf8View` where ADBC —
  resolving by OID against a live catalog — always gets a real type. Those are
  per-*dump* facts rather than per-type mapping choices, and a floor stated over
  columns would report them on every run, which is a signal that is always on.

**The reconciliation fails in both directions**, which is the shape [the
register-to-oracle reconciliation](#the-register-to-oracle-reconciliation)
already runs on and for the same reason: a type mapped with no evidence and
evidence for a type nothing maps decay in opposite directions, and a
one-directional check leaves whichever half it does not read free to rot. So
every floor row the rule reaches is either met or dispositioned; every
`builtin_scalar` arm resolves to a floor row, an arm naming a type no supported
major declares being a mapping decision with nothing behind it; and every
disposition is about a row that still needs one. The arms are parsed out of
`pgtype.rs` — a `match` is not data — and each arm's `DataType` is rendered into
the file's own pyarrow vocabulary by a table that reports an expression it does
not carry rather than guessing, so a new arm arrives loudly. `Utf8View` renders
as `string`: a floor is a claim about which values a column can hold, and the
three Arrow string layouts hold the same ones.

*Rejected: a Rust test emitting the mapping into a committed file the check
reads instead.* It removes the parse and adds a third artifact to keep in step,
and the parse's own failure modes are already answered — every anchor it keys
on has a test that removes it, and a `DataType` expression the rendering table
does not carry is reported rather than guessed.

**The stances are kept apart mechanically, not by wording.** A
`below-by-decision`, `different-encodings` or `waiting` row must be one this
build models *no* Arrow type for, and a `narrower` row must be one where both
sides are real types — so a mapping that gave `money` an `Int64` while its
stance still stood fails on the pairing rather than passing under a line that
has quietly stopped being true.

**Two things the join deliberately does not read**, each because reading them
would check something that cannot fail:

- **The extension metadata.** Release 24 stamps `arrow.json` on `json`/`jsonb`
  and nothing else, where we stamp those two plus `arrow.uuid`, so the metadata
  floor is met everywhere by construction. `extension` is read here only for
  `arrow.opaque`, which is a statement about the type rather than about
  metadata. Release 25's unreleased `POSTGRESQL:type` commit is what would turn
  this into a real obligation.
- **The built-in ranges.** `builtin_range_subtype`'s twelve names are mapped
  outside `builtin_scalar` and every one of their floor rows is `arrow.opaque`,
  so joining them would add twelve "floor undefined" lines and answer nothing.

**What the check computes is equality, not a subtype lattice**, and the
difference is deliberately conservative. A pair that is not equal is either
*below* — ours is the text fallback, which is the widest answer there is — or
*differs*, and both demand a written stance. So no widening can pass silently;
what it costs is that a difference which is obviously fine, as `oid`'s is, has
to be said out loud once. Teaching it which Arrow types contain which would let
a genuine widening through the first time somebody got an ordering wrong, over a
lattice with one member in it.

**Nothing about this is in the manual.** *"The schema you get is at least as
good as ADBC's"* is an embedder-facing promise, and the surface an embedder
would read it against is the embeddable-engine work, which does not exist —
publishing it now would state a guarantee about an API that has not been
presented. What exists here is the mechanism that makes the promise checkable
whenever that work chooses to make it.

<!-- deficiency: KD13 -->
**That is deficiency `KD13`: `money` is below the floor by decision, and it will
not be worked.** `cash_out` renders a value through the monetary locale —
`frac_digits`, `mon_decimal_point`, `mon_thousands_sep`, `currency_symbol` and
`mon_grouping` — and `pg_dump` sets `lc_monetary` nowhere in its preamble, so
`1.234,56` and `1,234.56` are the same stored value written under two locales
and the file cannot say which. That is exactly the bar this section opens with,
failed: the dump alone does not determine the value. ADBC escapes it only
because the binary encoding hands it the raw `int64` and it never renders
anything, and its own documentation calls the answer lossy. The stance is
**(a)**, a consequence of a deliberate tradeoff: closing it means either
guessing a locale or asking the user for one, and both are things this project
declines to do for every other type. The column still resolves `Utf8View` and
still filters as text, so what is lost is the typed integer, not the data.

### Joining a header against the metadata

`resolve_columns(qualified_table, columns, metadata, mode, database) ->
ResolvedSchema` joins a `COPY` header's column list against `DumpMetadata` by
name, requiring the caller's `database` — resolved from the matched block's own
attribution, **never guessed** — to pick the right `DatabaseMetadata`.

`ResolvedSchema { schema, columns, notes, plans, comparisons }`: `columns`,
`plans` and `comparisons` are positional (parallel to `schema.fields()`);
`notes` carries one `ColumnNote` per column, mapped and unmapped alike, with
the raw declared-type string attached, since `pgdq info`'s display needs both.
Every column has a `plans` entry, `NestedPlan::Scalar` included, and a
`comparisons` entry, `ComparisonPlan::Refused` included, so no consumer has to
ask whether either vector applies to it.

`comparisons` is the comparison register's answer for each column's declared
type ("Ordering operators compare typed"), decided here because this is where
the declared type string and the database's own `CREATE TYPE` list meet. A
column that did not resolve `Mapped` is `Refused` whatever its declared type
said — the census speaks *after* the DDL and can take a column out of
`Mapped`, so the two are kept in step here rather than at the one call site
that reads them.

*Rejected:* hanging the plan off `ColumnNote`, which is the human-facing
per-column record and would become two things — the one display that reads a
plan (`--verbose`'s `Range<T>` substitution, above) reads it positionally
beside the `DataType` it renders, which is where the pairing already puts it;
and making it a payload on `ColumnResolution::Mapped`, which expresses
"a plan exists exactly when a column mapped" but breaks the sites that compare
that enum by equality, to buy a coupling one producer already gives.

`ColumnResolution` is `Mapped`, `UnknownType`, `NotDeclared`,
`MetadataNotScanned`, `OpaqueElementType`, `NestedArrayElement`,
`VaryingArrayShape`, `OpaqueBaseType` or `EmptyEnum` — **the ways a column of a
container type can still be a string are told apart**, because "why is this
column text" has several answers a reader wants: its element type is opaque
(never improves), its element type is itself an array (a shape we decline to
represent and could), its arrays do not share one shape (what the file holds),
or strings were asked for.

<!-- deficiency: KD3 -->
**Two of those outcomes are deficiency `KD3`.** `NestedArrayElement` and
`VaryingArrayShape` leave a column as `Utf8View` with no way for a caller to
ask for more, and neither is opaque: both shapes are fully understood, and one
lossless representation — the shape-general `Struct{dims, lbounds, elements}`
under `roadmap.md`'s "Future — wanted, unscheduled" — would cover both. Adding
it only ever touches columns these two refusals leave as text, which is what
makes it additive and what leaves it unowned.

`MetadataNotScanned` is the odd one: a property of **how much of the file was
read**, not of a declared type, and the only outcome no fixture can produce
(`tests/pgtype.rs` names it as the sole exemption from the fixture rule, since
every fixture is scanned to EOF). It needs a `pg_dumpall`/`--create` dump whose
scan stopped inside a *later* database — `scan_preamble` always captures the
first (I1).

*Rejected:* reporting such a column as `NotDeclared`. It means "the dump never
explained this column" and is final, where this one means "finish the parse and
ask again" — identical-looking output, opposite advice.

`resolve_columns` also takes the block's array-shape census, positional like
the column list — see [The array shape census](#the-array-shape-census). It is
a parameter rather than a lookup this module performs so that a caller cannot
silently skip it: the three `stream::resolve_block` call sites, the resumed one
included, would otherwise be free to disagree about one stream's schema.

`SchemaMode::Strings` never looks anything up — every column is
`NotDeclared`/`Utf8View`, at zero lookup cost. In `SchemaMode::Typed` only,
the block's attributed database has to be both present in `metadata.databases`
and `preamble_complete`, and **the two paths answer that differently on
purpose**:

- **Streaming** refuses: `stream::resolve_block` raises
  `Error::MetadataNotScanned { database }` before a batch exists, because a
  stream hands back rows and a wrongly-typed one is a wrong answer with no
  signal. Its remedy is real: `--schema-mode strings` bypasses the check
  entirely.
- **Reporting** degrades and says so: `resolve_columns` marks the block's
  unexplained columns `ColumnResolution::MetadataNotScanned`. A listing covers
  every block in the index, so one unresolvable block must not sink the
  document.

A caller with `metadata: None` is left on `NotDeclared`: it has no DDL for any
database, so there is no scan to finish.

**No mapping scan produces the condition**, since the pass states a database's
DDL at that database's first `COPY` block — necessarily before any of that
database's blocks can be banked. So `Error::MetadataNotScanned` is unreachable
through every public entry point (`stream::resolve_block` is private, and all
three call sites read `metadata` after their own mapping pass, the resumed one
included), while `ColumnResolution::MetadataNotScanned` stays reachable:
`resolve_columns` is public and takes its `metadata` from the caller, so an
embedder resolving against an index it assembled itself can still present it.

The check is kept anyway and pinned by a unit test rather than left as untested
defence: it stands between a future reordering — moving that metadata read back
above the mapping pass — and a silently wrongly-typed row.

### One schema per stream, resolved up front

A stream's Arrow schema is fixed before its first batch. If the metadata needed
to type a column has not been scanned, the query says so
(`Error::MetadataNotScanned`) rather than silently returning untyped columns.

*Rejected:* lazy resolution — typing a column once its DDL happens to have been
seen. It makes the Arrow schema depend on scan progress.

*Rejected for the same reason, in a different hat:* having the **row replay**
accumulate preamble as it goes. A stream passing database 2's DDL on its way to
database 2's data could read it, but that DDL sits *before* its `COPY` block,
so the stream would learn a column's type partway through its own run and the
schema would again depend on how far the replay had got.

**The mapping pass reading it is not the same thing**, and is what happens:
mapping and streaming are separate passes, the map is finished before the first
batch, and the metadata is read off the index *after* the map returns. A file
containing any `\connect` is never early-stopped (`stream::target_settled`), so
the map that answers is always the whole file's. The schema therefore depends
on the finished map — a deterministic function of the file — never on replay
progress.

*Rejected:* a `--full` flag on `query`. `parse` already means "scan the whole
file eagerly and persist what you find", and a cold query and one run after
`parse` type a multi-database dump alike, so there is little occasion to want
it. Reasoning:
[`../status/history/2026-08-27.md`](../status/history/2026-08-27.md).

## Decoders and render-back

One `decode_*`/`render_*` pair per Arrow `DataType` `resolve_declared_type` can
produce. Every `decode_*` takes the already-COPY-unescaped `&str` that
`copy::decode_field` returns and yields a plain Rust value; every `render_*` is
its exact inverse, producing that same decoded-text shape — **never** an Arrow
builder (that is L3) and **never** raw on-disk COPY-escaped bytes (that is L1).

`numeric`/`decimal` go through `decimal_unscaled_digits`/`render_decimal`,
producing and consuming an unscaled integer digit string at a fixed scale,
which `i128::from_str`/`i256::from_str` accept directly. `Int16/32/64` use
`str::parse` directly in `batch.rs`, with no dedicated function.

**A `decode_*` allocates only where its return type is an allocation.**
`decode_bytea` owns a `Vec<u8>` and `decimal_unscaled_digits` a digit string
because that is what they hand back; nothing else on the scalar path takes a
`String` on the way to a fixed-size value. Three of them used to — `decode_uuid`
collected the hyphen-stripped text, `parse_time_of_day` padded its fraction to
six digits and parsed the result, and the digit string was built three times
over — and each was a `malloc`/`free` pair per *field*, which is per row per
column. What replaced them is arithmetic on the field's own bytes: a nibble
table for the two hex decoders (`HEX_NIBBLE`, with validity accumulated across
the value and tested once rather than branched on per byte), a multiply by a
power of ten for the fractional seconds, and indices into `int_part`/`frac_part`
for the unscaled digits. **Keep that property when adding a decoder**: the cost
here is paid on every scalar column of every typed row, so an intermediate
`String` in a decoder is a decision, not a detail.

*Rejected: giving `batch.rs` an `i128` straight from the `numeric` text*, which
would remove the last allocation on that arm. It moves the fixed-scale
arithmetic into the builder, where `Decimal256` cannot follow it — `i256` has no
`from_str_radix`-shaped constructor this could feed — so the two widths would
stop sharing one reading of the text, which is what makes them agree on what a
`numeric` field means.

**And a `render_*` writes one value into one pre-sized `String`.** The two hex
renderers are the only per-*byte* loops going the other way, and both knew their
whole output length before they started while allocating a `String` per byte
anyway: `format!("{b:02x}")` for each byte of a `bytea` and each of a `uuid`'s
sixteen, which on the control is 64 and 21 allocations a row against roughly 16
for the whole rest of `render_field` — a count taken as one `String` per field
and, per the paragraph below, low. What replaced them is `HEX_PAIRS` — the 256
lowercase pairs end to end as one `&'static str`, `HEX_NIBBLE`'s counterpart in
the render direction — so a byte is an indexed
two-byte slice and a `push_str`, and the UTF-8 conversion happens once at
compile time over a table that is ASCII by construction. A typed `pgdq query`
over the control falls **89.725 G → 49.074 G user instructions**, −45.3%, with a
`strings` query unmoved at 24.7935 G on both sides. **Keep that property when
adding a renderer**: `core::fmt` is an expensive way to write a fixed-width
integer, and the cost is paid once per value of every rendered column.

**And a row is rendered into one buffer, not into a `String` per field.**
`render_field_into(column, row, plan, &mut String)` is the form that does the
work — it appends the value and answers whether there was one, `false` being
SQL NULL — and `render_field`, which returns `Result<Option<String>>`, is a
wrapper over it, so the two cannot say different things. `pgdq query` appends
every column of a row into a line it clears and reuses, where it used to
collect a `String` per field into a `Vec` and `join` them, which copied every
field a second time. **An error may leave a partial value in the buffer**; the
only discipline that asks for is not to print a buffer an error came out of,
and a caller that reuses one clears it per row anyway.

**The date and time renderers write their digits rather than formatting
them.** `DEC_PAIRS` is `HEX_PAIRS`'s decimal counterpart — the 100 two-digit
pairs end to end as one `&'static str` — so every zero-padded field of a
`date`, a `time` or a `timestamp` is an indexed two-byte slice, and
`push_padded` reproduces `{:0width$}` for the values outside that range, which
only a caller-assembled Arrow array can hold. Together with the sink this takes
a typed `pgdq query` over the control from **49.074 G to 34.610 G user
instructions** (−29.5%) and its wall from 6.07 to 4.75 s; `date`, `time`,
`timestamp` and `timestamptz` render **80–88% faster** in isolation
(`benches/decoders.rs`). Unlike the hex table this one reaches `--schema-mode
strings` too — **24.793 G → 19.610 G, −20.9%** — because the sink is what a
`strings` query pays for its sixteen text columns; a `parse`, which renders
nothing, is flat to 0.0004%.

**An array column is written where it will be read, not assembled first.**
`render_array_into` walks the Arrow `List` and appends each element into the
caller's buffer through `render_field_into`, in place of building a
`nested::ArrayLiteral` and copying its rendered text in — which cost a `String`
per element, the `Vec` collecting them, an un-presized whole-array `String` and
that final copy. An element is rendered at a mark in the buffer and then offered
to `nested::quote_array_element`, which **leaves it alone unless the grammar
wants it quoted**; only a quotable element is moved aside, into a scratch
`String` cleared per element and reused for the whole value. The scratch is
unavoidable rather than incidental: the quoting decision is made from the
finished element text, escaping expands it, and `String` has no safe in-place
shift. A typed `pgdq query` over the `--arrays --composite` file falls
**104.094 G → 83.925 G user instructions** (−19.4%) and the 50-element array
column projected alone **67.535 G → 49.440 G** (−26.8%), while the same query
over a file with no array column is +0.14% with the legs' ranges overlapping and
a `parse` is flat to 0.00003%. The four other nested arms deliberately keep the shape this
replaced: each would need its own container grammar pushed into a sink form,
and each renders one value a row against an array's fifty.

**The count of what is left is a test rather than a paragraph**, since it is
the property most likely to rot: `tests/render_allocations.rs` installs a
counting allocator and asserts, per column of the measured control shape, that
rendering one row into a warm buffer allocates **20 times, none of them in a
column this work touched**, and that an array column costs the same at fifty
elements as at five — one allocation for `ListArray::value`'s slice, plus one
for the scratch where the elements are quotable at all. Eighteen of the twenty are `render_f32` (5),
`render_f64` (7) and `render_decimal` (6), which build their digit strings
through `format!`; the other two are `render_uuid` and `render_bytea`, one
pre-sized `String` each. The count this replaces was 16 `String`s a row, taken
as one per field and low: the four date/time renderers alone built nine
temporaries underneath those, and `print_batch` added the `Vec` and the `join`.

*Rejected: `write!(out, "{value}")` for an integer field.* It reaches the same
`Display` impl and is the obvious spelling of "append the digits", and it is
several times slower than `to_string`, because `to_string` for an integer is
specialised away from `core::fmt` entirely and `write!` is not. It cost 287
instructions per element on a 50-element `integer[]` column — enough to cancel
this whole change on the array-bearing file — and `push_integer` exists because
of it. Two neighbouring details are the same measurement: every piece
`push_padded` appends is a slice of a `&'static str`, because converting a
stack byte buffer with `str::from_utf8` put a per-field validation back; and it
reserves its whole length once, because `push_str` into a `String` that starts
empty grows it twice for a ten-digit value.

The equivalence to the shapes these replaced is asserted rather than argued:
`decode.rs`'s `differential` tests keep the previous implementations verbatim
and check the new ones against them over a generated corpus — a free-form one
that exercises the rejecting branches, and a per-decoder one built to that
decoder's own grammar and then damaged, because a random string is almost never
a well-formed time of day. The renderers take numbers rather than text, so
theirs is enumerated and swept instead of fuzzed: every entry of the hex pair
table, and for the `uuid` every entry at each of the sixteen positions the
hyphens are interleaved into; for the date and time renderers every day of a
six-century window and a stride across the whole of `i32`, every fractional
width, and the values only a caller-assembled array can hold — a negative
`Time64`, an hour past 99, a year past four digits — which are what the
two-digit and four-digit fast paths must fall out of rather than panic on.
`push_integer` is checked against `i64::to_string` the same way. Above them
all, the three whole-file `pgdq query` outputs the render path is measured on
are **byte-identical** across the change, which is the statement the corpora
exist to make about values no committed file happens to hold.

Non-obvious calendar and formatting facts, pinned as tests rather than left for
a future reader to re-derive:

- Date/timestamp decode and render use Howard Hinnant's public-domain
  `days_from_civil`/`civil_from_days` (proleptic Gregorian, correct for
  negative/BCE years by construction) rather than a calendar crate. BCE values
  convert through PostgreSQL's astronomical-year convention (`1 - year` for a
  `" BC"`-suffixed value) before the day-count math, so `0044-01-01 BC` and
  `9999-12-31`/`0001-01-01` share one code path with **no BCE special case in
  the arithmetic itself**.
- `timestamp with time zone` decode subtracts whatever UTC offset the text
  carries (general, not hardcoded `+00`), even though I4 guarantees every real
  `pg_dump` output normalizes to `+00` — costs nothing extra and covers a
  future PostgreSQL version or an unusual session `TimeZone`.
- PostgreSQL's float formatting switches to scientific notation on a fixed
  **exponent** threshold (`FLT_DIG`/`DBL_DIG`, 6/15), not by significant-digit
  count — I14, source-confirmed against `src/common/f2s.c`/`d2s.c`.
- PostgreSQL's documented maximum timestamp (`294276-12-31 23:59:59.999999`,
  relative to its 2000-01-01 epoch) genuinely overflows the
  `Timestamp(Microsecond)` mapping (`i64` micros since the 1970-01-01 Unix
  epoch, 30 years earlier). A real, expected `Error::FieldDecode`, not a bug.
- `jsonb` reformats whitespace on storage (`{"a":1}` in, `{"a": 1}` out) while
  `json` preserves source text verbatim. Affects neither the mapping (both stay
  `Utf8View`) nor round-trip testing, which compares against what the dump
  emits rather than the original `INSERT`.
- **`interval`'s three formatting rules are all sign-conditional**, so a
  fixture whose values are all positive exercises none of them: a part is
  suppressed when its count is zero, its unit takes an `s` whenever the count
  is not exactly `1` (`-1 mons`, and `1 mon` without), and a part that is
  positive and *follows a negative one* carries a `+` — the time tail
  included, which is what makes `-1 days +01:00:00`. The tail otherwise takes
  one sign for the whole of it, its hour field at least two digits and
  unbounded above (I40). `render_interval`'s test table is a live server's own
  answers for each rule rather than a reading of `EncodeInterval`, committed as
  a literal table in `decode.rs` so that no test depends on a server existing.
  *Rejected: putting the sign cases in `t_interval` and regenerating six
  majors*, which is how a codec's evidence usually lands here. These are pure
  function inputs, so the fixture would buy a slower unit test — and it could
  not reach the `i32` and nanosecond ceilings at all, since no `pg_dump` can
  write a value past them.
- **The `interval` grammar is read once, by `interval_parts`, for two
  consumers.** The decoder narrows the three parts to Arrow's widths; the
  ordering fuses them into `interval_cmp_value`'s 128-bit span. Sharing the
  walk is what keeps a field the decoder refuses and a literal the filter
  refuses the same set; the fusing is deliberately *not* shared, since it is
  what makes `1 mon` and `30 days` one value and a decoder that did it would
  lose the fields Arrow carries apart.

**Render-back has a third outcome, and it is not a NULL.**
`render_field` returns `Result<Option<String>, Error>` (`render_field_into`,
`Result<bool, Error>`): `Ok(None)` is SQL NULL
and `Error::FieldRender` is an Arrow value no PostgreSQL text form spells. It
is `Error::FieldDecode`'s mirror and names the Arrow value rather than a table,
a column and a row offset, because **nothing this crate scans can reach it** —
every typed column it fills comes from a `decode_*`, whose range is by
construction what the matching `render_*` writes back — so the only array
carrying one is an array a caller assembled, which has no dump position to
name. `interval` is its one case: Arrow's `Interval(MonthDayNano)` counts
nanoseconds where PostgreSQL's field counts microseconds, so a nanosecond count
with a remainder is not an `interval` at all and `EncodeInterval` has six
fractional digits with nowhere to put a seventh.

*Rejected: truncating to the nearest microsecond, which is what the naïve
inverse does.* It puts a value in the output that is not the one the array
holds, and render-back's whole contract is that `pgdq query`'s output is
byte-identical to what the dump held — a renderer allowed to round is a
renderer that can no longer be an oracle for the decoder. *Rejected: a panic,
which is what keeping the `Option<String>` signature would have forced.* The
cost of the `Result` is a `?` at each call site — the four nested arms of the
walk included, since it recurses — plus a fallible `print_batch` in the CLI,
against a library that aborts a caller's process for a value it merely cannot
spell.

<!-- deficiency: KD8 -->
**A typed column cannot hold every value its declared type admits** —
deficiency `KD8`, and unowned. Three classes, and the file is at fault for none
of them: `Date32` has no infinity and `Decimal128` no `NaN`, both of which
`pg_dump` emits from any healthy database (I34); and
`Interval(MonthDayNano)` holds neither v17's infinite interval nor a time part
past `2562047:47:16.854775807`, where PostgreSQL's `int64` microseconds run out
of Arrow's `int64` nanoseconds. A field holding one is an
`Error::FieldDecode` and there is no typed way to read the value.
`--schema-mode strings` returns the literal verbatim in every case. *Comparing*
one is a separate question and is already answered — see "Ordering operators
compare typed" below, which is why a filter may legitimately select a row the
output column then cannot represent. What stays open is **materialization**,
where the choices are a null, a sentinel indistinguishable from a real value,
or the error; it belongs to whichever phase owns typed materialization.

### The nested literal codec

`nested.rs` is the same contract one level in: `decode_array`/`decode_record`/
`decode_range`/`decode_multirange` take the COPY-unescaped field text and
return `ArrayLiteral`/`RecordLiteral`/`RangeLiteral`, and each `render_*` puts
it back byte-for-byte. It is **one quoted-token scanner parameterized four
ways** — separator, force-quote set, escape convention, and whether SQL NULL is
spelled as a bare `NULL` or as nothing at all — because I20 records that the
three container forms genuinely disagree on all four. A composite inside an
array carries both escape conventions at once, one per nesting level, and that
is the case a single-convention decoder passes every other value on and still
gets wrong. A multirange needs no parameter set of its own: `multirange_out`
concatenates its members' `range_out` results unescaped, so the split walks
brackets and steps over quoted bounds.

Five properties are load-bearing and easy to lose:

- **`ArrayLiteral` carries the shape, not just the elements.** `dims` and
  `lower_bounds` belong to the value (I21); the elements are flattened
  row-major. `{}` is `dims: []` whatever the array's dimensionality was, which
  is what `array_out` emits.
- **Decode rejects anything the matching `*_out` would not have written**,
  including an unquoted token that would have been force-quoted, whitespace
  padding, a ragged nesting, and an inner `{}`. That strictness is what makes
  decode and render inverses rather than merely compatible — the alternative
  is a value that decodes and renders back differently.
- **A range's `empty` flag is not redundant with its bounds.** `empty` and
  `(,)` both have absent bounds; only the flag separates them. A bound is never
  SQL NULL, so `None` unambiguously means unbounded and `Some("")` is the
  empty-string bound.
- **An array element borrows from the literal unless it carried an escape.**
  `ArrayLiteral::elements` is `Vec<Option<Cow<'_, str>>>`, and `scan_quoted`
  looks for the closing quote before it copies anything: a token holding a `\`
  or a doubled `""` falls into a second pass that rebuilds it, and every other
  token — quoted or bare — is a slice of the field. `array_out` quotes far
  more elements than it escapes, since a space or a separator is enough to
  force quoting and neither needs undoing, so the borrowed arm is the common
  one. It is asserted on the arm rather than on the text, because `Cow`'s
  equality cannot tell the two apart
  ([`measurements.md`](measurements.md), "Nested decode costs what it
  copies").

  **The borrow serves decode only, and the render direction has no use for
  it.** Rendering an Arrow value produces every element `Owned`,
  unconditionally, because it is rendering rather than slicing a literal — so
  `ArrayLiteral`'s lifetime is not a lever on the render path, and a session
  trying to make rendering allocate less by reshaping `elements` would be
  reworking the structure that carries the reading above while buying nothing.
  What the render path needed was not a different `ArrayLiteral` but not to
  build one, and it no longer does: `batch::render_array_into` walks the Arrow
  list and writes each element through `render_field_into` ("Decoders and
  render-back"). `ArrayLiteral` is what the *decode* direction returns and what
  renders back through `render_array`, which is where its shape fields and this
  borrow earn their keep.
- **The force-quote set is 256 bits, not a byte slice.** `needs_quote` asks
  "does this byte force quoting" once per byte of every token, in **both**
  directions — `scan_token` to reject an unquoted token `*_out` would have
  quoted, `push_token` to decide whether to quote at all — so it is the
  innermost operation of the whole codec. Held as the `&'static [u8]` it was
  written as, the test was a linear `[u8]::contains`, which specializes to
  `memchr` over four to six bytes and cost **10.4% of a typed
  `--arrays --composite` query** by itself; `ByteSet` is a `[u64; 4]` built by
  a `const fn`, so every `Syntax` is still a compile-time constant and the test
  is a shift and a mask. The truth table is unchanged by construction, and
  `the_force_quote_set_holds_exactly_the_bytes_each_syntax_names` checks each
  set against the byte string its `Syntax` was written as over the whole byte
  domain, rather than over the cases the round-trip tests happen to reach.

*Rejected: fusing `scan_token`'s terminator walk with `needs_quote`'s second
walk, into one byte-classification table that answers both.* The two walks are
real — a token is scanned once for the byte that ends it and once to check it
against the quoting rule — but the cost of a walk is what each byte costs
inside it, and that is now one bit test. What is left of the terminator walk's
own search is **1.4%** of a typed `--arrays --composite` query: `stops` calls
`terminators.contains(&c)`, which is the same linear `memchr` the force-quote
set stopped being, over a one- to two-byte slice. Fusing would buy some part of
that 1.4% at the cost of a scanner rework, and the strictness rule that makes
decode and render inverses lives in the second walk, so the two are not merely
adjacent loops. The cheaper half of it — giving `terminators` the same
`ByteSet` representation — is a smaller change with a measured ceiling and no
strictness consequence, and it is the one to try first if anyone reopens this.

*Rejected: pushing that borrow into `decode_record` and `decode_range` too, so
that a composite's or a range's fields are slices as well.* Both call
`Cow::into_owned` at the call site, on the same `scan_quoted` the array uses, so
the change is their literals' field types plus the `predicate.rs` and `batch.rs`
sites that read them — `range_key` takes `&Option<String>` and `batch.rs` builds
both literals to render them back. What says it is not worth that reach is the
figure: `record_2/decode` fell **190 ns → 114 ns** when the array's elements were
borrowed and the record's were *not*, because the win there was `scan_quoted`
sizing a quoted token before it allocates rather than the final `String`. The
whole composite column is **+0.75 µs a row** at the end-to-end level against the
two array columns' +6.03 ([`measurements.md`](measurements.md), "What a column
costs"), so one `String` per field of it is a sliver of a sliver — and its prize
is whatever the landed levers leave, which nobody has measured.

**`int2vector` is the fourth form and it shares none of that machinery** —
which is the point rather than an omission. `int2vectorout` writes `int16`s
separated by one space (I47), and an `int16`'s spelling can contain neither the
separator, nor a quote, nor a NULL, so there is nothing for a quoting rule to
decide: `decode_int2vector` is `split(' ')` and a canonical-spelling check per
element, and `render_int2vector` is a join. The strictness rule is the same one
every `decode_*` here obeys — `+1`, `01`, `-0` and a doubled space are refused,
because `pg_itoa` writes none of them — and it is what makes the pair inverses.
`parse_int2vector` is the matching superset and it is `int2vectorin`, whose one
rule worth reproducing is asymmetric: whitespace *before* a number is skipped,
and the byte *after* one must be a space or the end, so `\t1 2` is accepted and
`1\t2` is not.

**It is also the one literal with no leaf rule**, and the layer above says so.
Everywhere else a container's `*_in` superset stops at the element, which is
handed to `order_key`; `int2vector` has no element input function for that to
apply to — `int2vectorin` reads the elements itself — so the superset reaches
all the way down, and `nested_accepted_form` answers this plan before the leaf
clause it cannot carry.

**That buys less separation than it looks like, and the asymmetry runs the
other way.** `--filter 'v=+1 01'` matching the value the file writes as `1 1`
is not something only `int2vector` does: `order_key`'s integer arms are
`str::parse`, which takes the same `+` and the same leading zeros, so
`--filter 'a={+1,02}'` matches an `integer[]` holding `{1,2}` and
`--filter 'v_smallint=+0'` matches a scalar column holding `0`. Where the two
genuinely differ, `int2vector` is the **stricter** one: `parse_int2vector`
range-checks each element to `int16` and refuses `32768`, while an Int leaf is
parsed as `i64` whatever the column's width and takes `{99999999999}` without
complaint.

*Rejected: parsing the element as `i64` too, for consistency with that leaf.*
The cases are not alike. A `smallint` column's width is erased before
`order_key` sees it, and a literal outside the width still orders correctly
against every value the column can hold — so refusing buys nothing, which is
what `predicate.rs` says where it makes that call. An `int2vector`'s width is
intrinsic to the container grammar instead: `int2vectorin` itself raises
`22003`, the element type is fixed at `int2` by the type rather than by a
column, and no value of the type can hold `32768` for an out-of-range literal
to order against. Same reasoning, opposite outcome.

`tests/nested.rs` is the conformance test: every nested column of every
`types` fixture, on all six majors, read in `SchemaMode::Strings` and required
to round-trip. A force-quote predicate transcribed even slightly wrong from
`arrayfuncs.c`/`rowtypes.c`/`rangetypes.c` fails there against values
PostgreSQL itself wrote.

**The literal side is a second, separate scanner.** `decode_*` reads what a dump *holds*; `parse_array`/`parse_record`/
`parse_range`/`parse_multirange`/`parse_int2vector` read what a user *typed* — a filter's
right-hand side, which the `*_in` functions accept a good deal more of than
`*_out` ever writes (I44). The two directions may not be one scanner with a
leniency flag: `decode_*`'s strictness is exactly what makes it and `render_*`
inverses, and loosening it turns a value that decodes and re-renders
differently into a silent wrong answer, while tightening `parse_*` makes
`--filter 'tags={a, b}'` fail on a space and a typed nested filter worse to use
than the text comparison it replaces.

**They are four grammars, not one, and whitespace is where they first
disagree.** `array_in` drops unquoted whitespace around an element; `record_in`
and `range_in` keep every byte of it, so `( 1 , a )` is a two-field value whose
second field is `" a "`. One predicate still serves all four, because
`array_isspace`, `scanner_isspace` and the C locale's `isspace` are the same six
characters. Three more asymmetries carry their weight in the code: `record_in`
**checks arity**, which is why `parse_record` takes the declared field count
and `decode_record` does not; `array_in`'s bare `NULL` is disqualified by any
quote or escape in the element; and a `multirange_in` member spelled `empty` is
accepted and dropped.

*Rejected: reading a literal with the strict `decode_*` instead, so no second
grammar exists.* A space after a comma is what a person types, and refusing
`{a, b}` would make a typed nested comparison worse to use than the text
comparison it replaced. *Rejected: normalizing the literal with a cheap
pre-pass and feeding the strict decoder.* Stripping whitespace is wrong inside
a quoted element, so the pre-pass has to parse the literal to know where it may
strip — at which point it is the parser, written informally and with no oracle
behind it.

Each parser is a transcription of the **newest** major's function, with no
branch on the dump's version — the union rule (I35), and the one place two
supported majors disagree is v17's `array_in` rewrite accepting `{{},{}}` where
13–16 refuse it (I44). Under-accepting relative to `strtol` in a dimension item
is deliberate: refusing what the server takes costs the user an error message,
where taking what the server refuses is the divergence direction nothing else
here permits.

**What comes back is the parts as the user spelled them**, not as the element
type's `*_out` would write them. `parse_record("( 1 , a )", 2)` yields
`" 1 "` and `" a "`, where the server stores `1` and `" a "` — `int4in` threw
the first field's blanks away, not `record_in`. What the layer above does with
that is "Nested columns compare structurally": each leaf goes through
`order_key`, which for a number is `str::parse` — so `" 1 "` is refused rather
than trimmed. A discrete range's `[1,10]` → `[1,11)` and a
multirange's sort-coalesce-drop are the piece still missing there, and they are
why a range column has no comparison yet.

`tests/nested.rs`'s `oracle` module is where the supersets are checked: every
nested row of `fixtures/<13–18>/oracle/literals.tsv` — 475 literals over six
majors — parsed and compared against whether the server itself accepted it,
with the four canonicalization exceptions and the one semantic refusal
(`int4range '[10,1)'`, which is well-formed and out of order) asserted as exact
sets. That file is the only evidence of where the line sits that was not
written by the same hand as the parser.

`docs/manual/type-handling.md` is the user-facing statement of what recovers
exactly and what stays a string; this section covers only what an implementer
needs.

## Arrow assembly and the zero-copy path

`ColumnBuilder` covers every producible `DataType`, including
`Dictionary(Int32, Utf8)` for enum (an unparsed dictionary-key append — enum
text is never itself decoded or rendered) and `Utf8View`.

**Every non-`Utf8View` builder arm copies unconditionally**, since a decoded
value (an `i32`, a `[u8; 16]`, an unscaled `i128`/`i256`) has its own
representation rather than a byte range of the original field. The zero-copy
machinery stays scoped to exactly `Utf8View`.

The `Utf8View` path has **three sharp edges**. A field with no escapes is
appended as a view into the Arrow `Buffer` backing the read chunk it came from
(`append_block`/`append_view_unchecked`), not copied; only escaped fields, and
the rare field straddling two read chunks, take the copying `append_value`
path. Around that:

- Chunk buffers are retained in a small deque and evicted only once the scanner
  has moved past them for good — **a finished batch pins its source chunks.**
- A chunk's cached `StringViewBuilder` block index is invalidated on **every
  flush**, because `StringViewBuilder::finish()` resets the builder's internal
  block list. Anything adding a new flush trigger must honour this.
- Non-UTF8 field bytes are a hard `Error::InvalidUtf8`, never a lossy
  conversion.

### Three flush triggers, and only one of them bounds memory

`RowBatcher::should_flush` fires on `max_rows`, on `max_bytes`, or on
`max_source_span` — the distance from the start of a batch's first selected
row to the end of its latest. All three are evaluated after a row has been
appended, so each is overshot by at most one row. The span trigger alone
cannot fire on an empty batch whatever its cap, since a batch has no span
until a row lands.

`max_bytes` counts only the fields a projection actually built, because it caps
what the batch *holds* and a batch holds only what it built — so a zero-column
projection never trips it, and `max_rows` is what cuts those batches.

**The span cap is the only one that bounds what a batch pins.** A pinned chunk
is one the builder holds a `Buffer` clone of, and it is held until the batch
flushes; the `chunks` deque's own eviction at the scanner position cannot
release it. `max_rows` counts *selected* rows and `max_bytes` counts *selected*
field bytes, and a filter makes both arbitrarily sparse in the file — so with
the default 1 MiB `ScanOptions::chunk_size`, a 1%-selective filter would pin on
the order of 80 MiB and a 0.01%-selective one on the order of 8 GiB, against
this project's flat-memory goal. The span is a conservative bound on that:
pinned bytes never exceed the span rounded out to chunk boundaries.

A row a predicate rejected never reaches `push_row`, so it neither opens a span
nor extends one past the last selected row. That is what stops a long stretch
matching nothing from flushing a batch that pins nothing.

**64 MiB is chosen not to trip on ordinary work** — 64 default chunks, well
inside the 512 MB cgroup the measurements run in. The perf inputs are ~3.86 and
~4.49 KB/row, so a full-selectivity 8192-row batch spans ~32–37 MiB and still
hits `max_rows` first. `None` restores an unbounded span.

*Rejected: compacting the batch's views once selectivity drops below a
threshold.* It admits an unbounded peak before the threshold trips, and it
copies exactly the data the zero-copy path exists to avoid copying. A span cap
bounds memory against a number a caller can set, and — unlike a threshold — it
is testable at fixture scale with a small chunk size.

*Rejected: measuring the span against the scanner position rather than the last
selected row.* It would flush an in-flight batch while the scan crossed a
region matching nothing, which pins nothing, turning a hard filter into one
tiny batch per cap's worth of file.

### Nested columns: `NestedPlan` travels beside the `DataType`

`List` and `Struct` builders recurse: a `ColumnBuilder::Array`/`Multirange`
holds a child builder and its own offsets and validity, a
`ColumnBuilder::Record`/`Range` holds one child per struct field. `finish`
assembles a `ListArray`/`StructArray` from the schema's own `FieldRef`/`Fields`,
so the built array's type matches what the schema promised —
`RecordBatch::try_new` checks exactly that.

**The Arrow type does not say which literal fills it**, and that is not a
detail: `int4range[]` and `int4multirange` both resolve to
`List<Struct{lower, upper, …}>` and are written `{"[1,10)","[2,3)"}` and
`{[1,10),[2,3)}` respectively. So `pgtype::NestedPlan` — a small tree of
`Scalar`/`Array`/`Record`/`Range`/`Multirange` — is passed alongside the
`DataType` into `new_column_builder` and `render_field`, and is what picks the
`nested.rs` codec at each level. It comes from `ResolvedSchema::plans`,
positionally.

`render_field(column, row, plan)` and its sink form
`render_field_into(column, row, plan, &mut String)` are the entry points: there
is deliberately **no plan-less sibling**, because one that panicked on a nested
column would make "did every caller switch?" a review question rather than a
compile error. A scalar caller passes `&NestedPlan::Scalar`, its `Default`.

**Two trees that must agree are kept agreeing by a rule, not a structure.**
`resolve_declared_type` is the one producer of the `(DataType, NestedPlan)`
pair, and `resolve_columns`' census transform the one place either half changes
afterwards; neither is ever rewritten alone. That is affordable exactly while
those two sites are the only writers.

*Rejected — but costed, as the fallback if a third writer ever appears:* a
single tree owning both (`Scalar(DataType)` / `Array(Box<…>)` / …, with a
`data_type()` accessor), which makes disagreement unrepresentable. It costs a
rewrite of `pgtype.rs`'s public surface plus a `.data_type()` at every existing
`DataType`-shaped call site, against a drift risk that two writers do not yet
have.

*Rejected:* inferring the literal form from the Arrow type. It is not merely
fragile, it is impossible for the array-of-range/multirange pair. *Rejected:*
smuggling the marker into Arrow `Field` metadata. Metadata is part of `Field`
equality and so of the enclosing `DataType`, which makes every schema
comparison carry it; and the marker for "this struct is a range" would have to
live on a *child* field, since a `DataType::Struct` describes its children and
not itself.

Three rules the arms enforce, all of them the same asymmetry the `COPY`
grammar already chose — not recognizing something is recoverable, misreading it
is not:

- **A value whose dimensionality does not match the column's `List` depth is
  refused**, not flattened or wrapped. `{}` is the exception: `array_out`
  emits it for a zero-element array of any dimensionality, so it fits any
  depth.
- **An `[lb:ub]=` prefix is refused.** Arrow lists are 0-based with no lower
  bound, so the index origin has nowhere to go, and dropping it would make
  render-back inexact.
- **A composite literal whose field count disagrees with the declared type is
  refused.** A zero-field composite is the one place the count is not read off
  the literal: `()` is what `record_out` writes both for it and for a
  one-field composite holding NULL (I23), so the declared list decides and the
  builder accepts exactly that literal. Finishing one needs
  `StructArray::try_new_with_length` — with no children there is nothing to
  read the row count off, and Arrow refuses to guess.

A failure anywhere inside a nested literal is reported against the **whole
field**, not the fragment that tripped it, because that is what
`Error::FieldDecode`'s `value` means everywhere else.

**Nested values always copy, `Utf8View` elements included.** Widening the
zero-copy view path into a recursive builder means honouring its three sharp
edges at every level of nesting, and **that swap has been measured and
refused**: it is worth at most 4.7 ns an element, 0.24 µs of an 11.08 µs row,
on a file built to flatter it. The reading and what would reopen it are under
"The library's own per-row budget".

A `COPY` header with no explicit column list gets placeholder names
(`column1`, `column2`, …) sized to the **first row's** field count, so **a
block's schema is per-block, not per-table**: two blocks of the same table can
carry different schemas, which is why predicate column resolution happens once
per block rather than once per query.

`read_table` returns `(ResolvedSchema, Option<ResumeToken>)`. The schema is
known before the first batch, so returning it at the end is late but never
wrong, and every batch already carries `RecordBatch::schema()` for a caller who
needs types *during* the callback. *Rejected:* an out-parameter or a second
diagnostics callback — both burden the common path to serve the rare one. A
caller who needs diagnostics before consuming should use pull mode, which is
what it is for.

`RowBatcher` carries the qualified table name and each column's declared-type
string (from `ResolvedSchema::notes`), which is everything `Error::FieldDecode`
needs.

## Query: mapping and streaming are separate passes

`stream.rs`'s `table_stream` is two phases with a hard boundary, and **nothing
yields until the first is done**:

1. **`map_forward`** — a free `async fn`, not part of the generator. Walks from
   `index.scanned_through`, drives a `map::Builder`, and at each opening of the
   save throttle's gate splices `snapshot` onto the base spans, advances
   `scanned_through`, merges the builder's `roles()`/`tablespaces()`, and
   persists — see "`parse` resumes, and saves as it goes" for what opens that
   gate and what banking at it rather than at every `CopyEnd` costs. At a `CopyStart`
   opening a database the metadata does not yet cover, it restates
   `index.metadata` — I1's recurring boundary; see "`parse` resumes, and saves
   as it goes". Returns when the target is settled or at EOF. Its stop rule is
   `target: Option<(&str, Option<&str>)>` — `None` means "run to EOF", which is
   what `ScanExtent::Full` asks for and what `map_file` always wants. An
   `Option` rather than a `ScanExtent` beside an unused table name, since a
   sentinel would be dead data every later reader has to prove is unused.
2. **Replay** — a scanner over exactly `[block.header_offset, block.end_offset)`
   per matching block, in file order, producing batches.

The cost is a second read of the queried block: once to find its extent, once
to emit its rows. Recovering the interleaved form without reintroducing the
unmapped hole that motivated the split is a known, deliberately deferred
optimization — see [`roadmap.md`](roadmap.md).

**`splice` owns the seam, not `Builder`.** A `Builder` beginning partway
through a file opens its first span at its own first recognized content, which
is *after* its start byte whenever blank lines separate the two. `splice`
closes that by extending the *preceding* span to where the next one starts —
the same rule `push_span` applies everywhere else.

*Rejected:* a start floor on `Builder`. It tiles, but it hands the blank line
after a block to the *following* span where a single-pass scan hands it to the
block, so a map assembled from several scans stopped agreeing with one built in
a single pass. `full.spans == eager.spans` is now a test, and the property to
preserve is: **how a map was assembled must not be visible in it.**

**`map_forward` has a second caller: `map_file`**, which is `pgdq parse`'s
scan — see "`parse` resumes, and saves as it goes" under [CLI
surface](#cli-surface). It is what keeps the two whole-file producers from
drifting: `tests/map.rs`'s `build_index_spans_match_build_map_exactly` and
`tests/map_file.rs` exist because that drift is a failure this codebase has
already had to defend against once.

**`target_settled` is deliberately conservative.** Two things veto an early
stop: a matching block carrying `partition_root` (I2 — those blocks are not
adjacent, so only EOF enumerates them), and **any `Connect` span anywhere in
the map** (a `pg_dumpall`, a concatenation or a `--create` dump can define the
same qualified name in a later database, and a `batch_options.database`
selector does not lift it — two `\connect` segments can name the same
database). So early stopping applies to exactly the common case: a
single-database, non-partition-root dump. koji is one.
`QueryOptions::scan_extent`'s `ScanExtent::Full` is the explicit opt-out.

### One target per query

A `COPY` header match by bare or qualified name alone is not enough once a
resolved schema and real row data both depend on *which* database and table a
match came from — the same collision can happen across `\connect`ed databases
or, via `CopyHeader::matches`'s schema-agnostic bare-name rule, within one
database across schemas.

`table_stream` narrows every match to at most one `(database, qualified_name)`
candidate. **Ambiguity is detected before any row goes out**, once, over every
candidate the map holds, between the two phases; a second differing candidate
raises `Error::AmbiguousTable { name, candidates }`. `--database` (CLI) /
`QueryOptions::database` (library) resolves it by name.

`pgdq info` groups its block listing by `database: <name>` whenever more than
one is present (silent for the common single-database case) — that is what
makes an `AmbiguousTable`'s candidate names actionable, since they are names
the listing already showed.

<!-- deficiency: KD6 -->
**The residual deficiency is `KD6`**: a conflicting table *past* a query's
stopping point is never seen, so `Error::AmbiguousTable` is not raised for it
and the query returns the candidate it found. The stop rule rules out the two shapes
that announce themselves — a partition-root marker (I2) and any `\connect` at
all — leaving one undetectable case, a file whose *first* segment is a plain
dump with something concatenated after it. `ScanExtent::Full`, or a query after
`pgdq parse`, gives exact detection; rows are never a union either way, and
ambiguity is raised before any row is emitted. Closing it by default means
abandoning early stopping, which is what makes a cold query on a large dump
affordable — so what an embedded API promises here is P6's to decide, and the
three shapes open to it are filed in
[`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md).

*Rejected: evicting `KD6` from the register as a property rather than a
deficiency.* The register's eviction test is whether the remedy is already the
user's today, and `KD6` carries the same remedy sentence that turned
`DumpIndex::roles`/`tablespaces` into properties with no identifier —
`ScanExtent::Full`, or a query after `pgdq parse`. Two things separate it. An
evicted property tells the user what it did not cover, so they know to ask
again; this one is silent, and a user cannot invoke a remedy against a
possibly-wrong answer they have no signal for. And the index line is what
carries the question *past* P6: an inbox is drained and deleted as part of
grilling the phase it belongs to, so if P6 picks the "leave the default as-is
and document it" shape, the entry survives only as a `KD<k>` line whose stance
moves from **(b) owned by P6** to (a) or (c). Evicting it now would mean the
outcome most likely to keep the tradeoff is the one that erases the record of
it.

### Resume

A resume point is inside a mapped block by construction, so `resume` is "skip
blocks ending at or before the token's offset, start the first survivor at the
token's offset instead of its header". There is **no fallback path**.

`ResumeToken` carries a file offset, a cumulative row count, and — for a token
taken mid-block — the block's header and in-block row count, which is what lets
a fresh stream reconstruct the same schema and continue with no gap or repeat.
The header's field count is the **block's** width, never the projection's: a
headerless block names its columns `column1..columnN` from that number, so a
resumed stream rebuilding them from a projected width would name different
columns than the original did.
A reserved `generation` field is always 0.

**It also carries a fingerprint of the query that produced it** — the table,
the projection, the filter expression and the schema mode, hashed — and resuming a stream
whose options hash differently is `Error::ResumeQueryMismatch`. That defends
"one schema per stream, resolved up front", which nothing else defends: the
token never named even the table, so resuming against a different one was
silently accepted, and a projection makes changing the schema without changing
the table an ordinary thing to do rather than an exotic one. The hash is over
an explicit `match` per field rather than a derived `Hash`, so a new operator
or option is a compile error instead of a stamp that quietly stops covering it.
The filter tree is hashed node-kind first, then arity, then each child in
order, so `And`/`Or`/`Not` cannot collide by carrying the same terms and two
conjunctions differing only in term order fingerprint differently — a
`ResumeQueryMismatch` on a resume nobody would write, against a
canonicalization rule the stamp would otherwise have to own. `database` and `scan_extent` are deliberately
outside it: they change which
blocks are replayed, not the shape of what comes back.

**`ResumeToken` exposes no public fields, and must stay that way.** A raw file
offset is meaningless inside a compressed archive entry; opaque now means the
representation can change without an API break. It is also valid only within
the producing process.

### Projection

**A projection names columns**, resolved per block against that block's own
`COPY` header exactly as a predicate's column is. A name the block does not
carry is `Error::UnknownProjectionColumn`; a repeated name is
`Error::DuplicateProjectionColumn`. Order is as requested, so a projection may
reorder. `None` is every column; the empty projection is the `COUNT(*)` shape,
and it is reachable rather than a degenerate case nobody can express.

**The two refusals fire at different moments, and that is the ordering, not an
accident.** A repeated name is wrong whatever the file holds, so it is refused
on the request — before the source is even sized, alongside the resume
fingerprint check, so a token from another query is not discovered only once
its first block resolves. An unknown name is a fact about a *block*, so it
waits for one to resolve. `stream::project` therefore does not re-check
duplicates itself: the up-front check covers replay and resume both, where a
per-block check would report the same fault later and once per block.

**The projected schema is what the stream reports.** All five of
`ResolvedSchema`'s vectors — `schema`, `columns`, `notes`, `plans`,
`comparisons` — are cut together, in the requested order. They are positional
and parallel by construction, and `RecordBatch::try_new` checks the built
arrays against `schema` exactly, so a stream advertising the full table while
emitting narrow batches would put those two out of agreement. An unprojected
column's resolution note therefore is not reported by that stream, which costs
nothing that matters: `pgdq query` does not print notes and `pgdq info` never projects.

*Rejected: reporting the full table schema and projecting only the batches.* It
reads as the friendlier API and it desynchronizes the one invariant
`RecordBatch::try_new` is checking.

*Rejected: addressing columns by index.* DataFusion's `TableProvider::scan`
hands a provider `Option<Vec<usize>>`, and those indices are into the schema
pgdq itself advertised — so the embedding layer translates them to names
against that schema. Taking indices here instead would make the meaning of `2`
depend on which block matched, which is what every other lookup avoids by going
through the `COPY` header.

**A filter term may name a column the projection does not.** The projection
decides what is *built*, never what may be *tested*: `ResolvedTerm::eval` reads
the row's own field split, so every term's column index is resolved against the
block's **unprojected** schema. That is also what makes a filtered row count —
zero columns plus a filter — expressible.

**A projection narrows what is decoded, never what is walked.**
`RowBatcher::push_row` skips `decode_field` and the builder append for a column
nobody asked for; it does not stop at the last needed field. The walk is the
row's `copy::RowSplit`, finished from wherever the filter's terms left it, and
it is the cheap half — and it is the system's **only** field-count
check — `push_row` is the sole site raising `Error::ColumnCountMismatch`, and
the mapping pass, which walks every field for the array-shape census, never
errors on a count. A row is checked against the block's width, not the
projection's. Under the standing rule that the input contract is valid
PostgreSQL rather than `pg_dump`'s output, a hand-edited dump with a stray tab
is exactly what that check exists for, and an early stop turns it into a wrong
answer with no error.

*Rejected: stopping at the last needed column and counting the remaining
delimiters with `memchr::count` to keep the check.* It is sound, and it
optimizes the half that was never shown to cost anything — the figure below
still does not show it costing anything.

**What a projection saves is measured, and it is the columns' whole build
cost.** One 3.00 GiB file read at five widths, warm and typed
([`measurements.md`](measurements.md), "What a column costs: five projection
widths over one file"): `--no-columns` costs **1.61 µs a row** where all 19
columns cost **12.97**, so the replay a projection cannot avoid — the block
read, every row walked and field-counted, the predicate evaluated — is an
eighth of a complete typed read. Between those, one `smallint` is +0.12 µs, the
other fifteen scalars +4.46 between them, the composite +0.75, and the two
array columns **+6.03** — 89% of what all three nested columns cost, and more
than every scalar column in the table. So the saving is real, it is
concentrated in the nested columns, and it is what makes projecting one array
column away worth more than projecting every scalar away.

That table is also the instrument this doc's per-column costs are now read off.
It replaced a cross-file subtraction — three generated files differing by the
columns under test — whose own noise floor was ±0.5 µs/row, wider than the
composite column it was being used to price.

**Whether a query succeeds depends on its projection**, and that is intended.
A column that is not projected is never decoded, so a value that would raise
`Error::FieldDecode` no longer does — which is the per-column escape from a
hard decode failure that `SchemaMode::Strings`, untyping the whole table, was
previously the only form of. An array nested inside a composite is the
motivating case (deficiency `KD2`, [What the census decides, and who may believe
it](#what-the-census-decides-and-who-may-believe-it)).

*Rejected: decoding unprojected columns anyway to preserve error parity.* It
discards the entire saving to raise an error about a column nobody selected.
*Rejected: a strict mode that restores the checking.* The strict answer is
already reachable by projecting the column back in.

**A zero-column batch needs `RecordBatch::try_new_with_options`.** `try_new`
fails with "must either specify a row count or at least one column", so
`RowBatcher::flush` states the row count explicitly — for every width, so the
two do not become separate paths; with arrays present the row count is still
checked against every one of them.

**On the command line a projection repeats rather than splits.** `pgdq query
--column <name>`, once per column, in the order wanted; `--no-columns` for the
empty projection; neither flag for every column. The two flags are mutually
exclusive and clap refuses the pair, so the CLI's whole job is turning them
into the three states `QueryOptions::projection` already has — a repeated name
and an unknown one are the library's refusals, raised identically for an
embedder.

*Rejected: `--columns a,b,c`.* A PostgreSQL column name may legally contain a
comma (`"a,b"` is a valid quoted identifier), so a comma-separated list either
invents a CLI quoting grammar or has a case it cannot express — against the
standing rule that the input contract is valid PostgreSQL rather than
`pg_dump`'s usual output. Repeating needs no grammar and matches `--filter`.
*Rejected: `--columns ''` for the zero-column case.* `--no-columns` says it.

**A zero-column query prints no header line.** The header is the batch's own
field names, so at width zero it would be an empty line — and
`pgdq query --no-columns | wc -l` is the filtered row count, which that line
would make `rows + 1`. Each row still prints as an empty line, which is what
makes the count come out. The "no rows found" message is therefore keyed off
whether any batch arrived, not off whether a header was printed.

### Predicates

Post-parse row filtering: a row is fully parsed, then dropped if it fails.
`IsNull`/`IsNotNull` are the two operators that exist because before typed
columns there was no way to ask for a NULL at all, and they compare nothing.
Every other operator is **typed**, through the column's own comparison plan —
the four ordering ones, `=`/`!=`, and the two `IS DISTINCT FROM` forms alike —
and they have their own subsection below. Evaluating predicates *during* the
scan is future work; it inverts control, not dependency (see `layering.md`).

**A filter is an expression tree.** `QueryOptions::filter` is one `Expr`:
`Term(Predicate)`, `And(Vec<Expr>)`, `Or(Vec<Expr>)` or `Not(Box<Expr>)`, with
`And` and `Or` n-ary because the shape a repeated `--filter` builds is n-ary
by construction and binary nesting would make the ordinary case a
right-leaning chain every reader has to flatten. The default is the empty
conjunction, `Expr::all([])`, which every row satisfies — so "no filter" is a
degenerate tree rather than a case of its own, and nothing on the row path
branches on whether a filter exists. The tree is resolved to a parallel
`ResolvedExpr` once per block, and that travels in the block's `Active` state;
a leaf naming a column the block does not carry is
`Error::UnknownPredicateColumn`, raised for the first such leaf in a
left-to-right walk.

**Evaluation is three-valued, and a row survives only where the root is
`True`.** A NULL field is `Truth::Unknown` under every comparing operator, and
`And`/`Or`/`Not` are Kleene's. That is a *restatement* of what a conjunction
always did rather than a change to it: unknown was collapsed to "excluded" at
each term before, which gives the same row set for every filter a conjunction
can express, because a conjunction carrying an unknown is not `True` either
way. What the restatement buys is that `Not` is now expressible at all —
`NOT UNKNOWN` is `UNKNOWN`, not `TRUE`, so the old collapse would have turned
a NULL row into a *kept* row under a negation.

**`IS DISTINCT FROM` / `IS NOT DISTINCT FROM` are the addition three-valued
logic makes necessary** rather than redundant, and they are the only operators
added with it: `IN` is a disjunction of `=` and `BETWEEN` is two ordering
terms ANDed, so `Or` retires both before they are proposed. Without
`IS DISTINCT FROM` there is no spelling at all for "different, counting NULL
as a value" — `Not(Term(a = 1))` drops the NULL row. They route through the
same equality comparison `=`/`!=` do (see "Equality is typed too"); all that
differs is the NULL rule, which is two-valued: `True` and `False` respectively
on a NULL field.

**Two more exclusions are closed decisions rather than gaps.** `LIKE` is a
matching engine of its own — pattern syntax, escapes, and case folding that is
collation-dependent, which is precisely the boundary the register spends its
length declaring — so adding it would reopen every collation question against
an operator that cannot answer them. And **column-to-column comparison** is
not a `Predicate`: every one of them is one column against one literal, and
changing that is a different feature, not a wider operator set. *Rejected:
adding `IN` for the ergonomics of a long list.* `Or` already expresses it, and
if repeated `OR` proves painful in use it is an out-of-band ergonomics item
with no decision behind it rather than list syntax invented here.

**Short-circuiting is defined against the *root*, not against each node.**
`And` stops at the first `False`, `Or` at the first `True`. Under a `Not` that
is all it may do, since `Not` has to tell `False` from `Unknown`; everywhere
else the caller cannot tell them apart — `Unknown` and `False` both drop the
row — so an `And` may also stop at the first `Unknown`, which is exactly the
short-circuit a bare conjunction had. `ResolvedExpr::eval` carries that as one
`exact` flag, set only by `Not` and inherited downward. The property is
asserted rather than argued: every tree of height three over the three truth
values gets the same root verdict either way.

*Rejected: exact Kleene everywhere, with no flag.* It is four lines shorter,
and it makes a conjunction whose leading column is often NULL walk every
remaining term of every such row — a regression in exactly the cost model this
section states, bought for nothing, since no caller can observe the difference
the flag hides.

**A term's cost is mostly finding its field, and the row is split once for all
of them.** Every consumer of a row — each term, then `RowBatcher::push_row` —
reads its field out of one `copy::RowSplit`, which the replay's row arm resets
per row and hands to the filter first. Each boundary is found by exactly one
`memchr`, by whichever consumer asks first, and every later ask is an index
into what is already there. Measured over the 16-column control, with nothing
surviving to be decoded: a term one field in costs 0.033 µs a row, one thirteen
fields in 0.09–0.12 — wall readings from a sitting the machine was not quiet
for, so magnitudes rather than exact figures — and before the sharing, a
five-term disjunction at that depth spent **49% of the whole query's user
instructions** on the walk alone, which is the deterministic half of that
reading ([`measurements.md`](measurements.md), "What a filter term costs").

**The split extends only as far as it is asked to, and that is what makes the
sharing unconditional.** A term reading field 3 finds four boundaries and stops,
so a row the filter rejects is never walked past its deepest term — the eager
whole-row split, which would have needed a term-count gate to avoid costing a
shallow filter 31%, is not what is built. What is left on the cost side is the
memoization: a boundary now costs a `Vec` push and a read back as well as the
`memchr`, about **9 instructions a field**, paid by every row that reaches the
batcher whether a filter read anything or not. On the 3.00 GiB control, warm and
untyped: a five-term disjunction thirteen fields in falls **39.7%**, the same
five terms over rows that all survive **11.7%**, and five shallow terms 2.2%;
against that, one deep term on rejected rows is **4.5%** worse, a bare
`--no-columns` count 2.9%, and a full-projection query with no filter 0.4%.

**Those losses are upper bounds and the wins are not, which is what makes the
unconditional sharing a bounded trade rather than an open one.** `push_row`
walks the whole row whatever the projection is, so on a row that *survives* the
filter the split is found once and shared for nothing; every loss above is
measured on a shape where no row survives, which is the pessimal end of the
selectivity axis. Move along that axis and the 4.5% shrinks toward zero while
the 39.7% does not. **The largest loss is a filtered shape, not the unfiltered
one** — one term against a late column, rejecting every row, which is an
ordinary ad-hoc query and the shape to weigh the trade against; the unfiltered
readings are the smaller half of the cost side, not its worst case. Both losing
shapes are *registered figures* — `predicate-terms`' one-term-thirteenth-column
row and `projection-widths`' zero-column row, which passes no filter at all
([`measurements.md`](measurements.md), "What a filter term costs" and "What a
column costs") — so a sweep republishes the regression rather than anyone having
to remember it, and the absolute magnitudes on that instrument are 0.155 G lost
against 2.78 G and 3.35 G won.

**Two or three of those nine instructions are unclaimed, and the shape that
would take them is known.** `restart` keeps the `Vec`'s capacity and every row
of a block has the same width, so from the second row on, `Vec::push`'s
capacity load-compare-branch and its length store are both known-redundant:
sizing `ends` to the width once per block and writing `ends[n] = end` against a
counter removes them, bounds-checked and without `unsafe`. It is left because
the reading that would size it is the *unfiltered* shape — the one the
memoization is a pure cost on, at 0.4% of a full-projection query — and 2–3
instructions a field is below what any instrument this project owns resolves
there. Nobody owns it; it is written down so that a session reaching for the
`unsafe` version meets the cheaper safe one first.

*Rejected: a `shared` flag on `RowSplit`, set once per block from the filter's
term count, so a single-term filter's `field` walks without memoizing.* It is
the term-count gate the eager arithmetic implied, moved out of `batch.rs` where
the second-call-site trap below lives, and it looks like a free recovery of the
4.5%. It is not: with one term the memoization is a loss on a *rejected* row
and a win on a *kept* one, since `complete` reuses the boundaries that term
found. The flag trades one selectivity regime for the other instead of removing
a cost, which makes it a heuristic needing a reading of its own — and the shape
that would decide it, a single deep term over rows that survive, is measured
nowhere.

**There is one `push_row`, and giving the unfiltered case its own was measured
and refused.** A second entry point that walked the row directly cost the
unfiltered path **2.4%** of a query's user instructions, against the 0.4% of
bookkeeping it would have saved there. The cause is not the
split: a second `push_field` call site in `batch.rs` is enough on its own, even
when the branch to it never runs, because `push_field` stops being inlined into
`push_row` and `GenericByteViewArray::value` stops being inlined into
`render_field` beside it. `#[inline]` and `#[inline(always)]` did not recover
it, and neither did sharing one loop body behind a closure.

**A `RowSplit` is bound to its row by an assertion, not by a type.** `field`
and `complete` take the row on every call, so nothing in the signatures says it
is the row the boundaries were found in: a missed `restart` yields **in-range
indices into the wrong row** — wrong fields, wrong comparisons, wrong rows
emitted, and no panic anywhere. That silent failure is the price of a split
that holds offsets rather than borrows, and it is only affordable because a
`#[cfg(debug_assertions)]` row length, checked in both accessors, turns the
class into a `cargo test` failure. The guard is a **length**, which the
accessors already hold, so it costs nothing in either build and survives a row
that moved; what it does not catch is a single equal-length pair, and it does
not need to — a missed `restart` sits on a row path and runs on every row of a
block, and no real dump has a block whose rows are all one length. *Rejected:
`unsafe` on the borrow, or a `RowSplit<'a>` that holds the row.* The first is
what the offsets exist to avoid, and the second puts a lifetime on a buffer the
replay reuses across every row of every block, which is the reuse the sharing
is for.

**A decode failure therefore surfaces only where evaluation reaches it**, so
which rows error depends on where the term sits in the tree and on whether a
`Not` sits above it. That is a property, not a defect: when `a=1 OR b<2`
short-circuits past a corrupt `b`, the row's answer was already settled by a
field that did decode, so nothing wrong is returned. It is the asymmetry
projection already has, where deciding needs strictly less than materializing.
*Rejected: evaluating every leaf of every row so that a corrupt field errors
regardless of expression shape.* It gives up the one-walk hot path to buy
determinism about which error message appears, on a file that is already
contradicting its own DDL.

**`stream::resolve_expr` is the single site where a predicate meets a block.**
Every refusal a term can earn — the unknown column, the ordering refusals
below, and a literal that is not a value of the column's type under *any*
comparing operator — is raised there, against that block's own **unprojected**
`ResolvedSchema`, before a row of the block flows. **Resolution walks the
whole tree**, so a leaf the evaluator would short-circuit past is still
validated: a filter's refusals must not depend on the data. It takes the whole
`ResolvedSchema` rather than its `schema` because the ordering refusal reads
`columns`, `plans` and `comparisons` too, and those are positional and
parallel: splitting them across two lookups is how they would come to
disagree. A table whose blocks carry different schemas can therefore refuse at
the third block after rows from the first two were emitted; that is true of
`UnknownPredicateColumn` as well and adds no new shape of failure.

**A `ResolvedTerm` carries its own operator**, so a resolved tree is
evaluable without the `Expr` it came from. The alternative — walking the two
trees in lockstep to pair each leaf with its predicate — is an invariant
nothing checks, and it is the invariant the flat parallel-vector form used to
rest on.

**Nothing folds two terms together.** Two terms on one column are evaluated
independently, so a contradictory pair is a query with no rows rather than an
error, and a redundant pair costs a second comparison. There is no simplifier
and no plan — what the shared split removed is the second *walk*, not the
second evaluation.

*Rejected: keeping `filters` and adding an expression field beside it.* Two
ways to say one thing, with a rule needed for how they combine.

*Rejected: normalizing to disjunctive normal form and evaluating a flat list
of conjunctions.* That is a planner, and there is deliberately none; DNF also
multiplies term count, which is no longer a multiplied walk but is still a
multiplied comparison.

**On the command line a filter repeats rather than splits**, exactly as a
projection does: `pgdq query --filter <term>`, once per term, ANDed into
`Expr::all`. Each repetition is parsed on its own — a malformed one is refused
before the dump is opened — and the CLI decides nothing else about them; the
column lookup and its refusal are the library's, identical for an embedder.
The tree's other shapes are `pgdq query --where <expr>`, a separate flag over
the same terms — see "`--where` builds an expression out of those terms"
below. **Nothing below L4 parses an expression** any more than it parses a
term: `Expr` is a struct an embedder fills in, and both grammars are the
CLI's.

**A string comparison agrees with PostgreSQL more often than it deserves to**,
and the reason is a property of the input rather than of the comparison: every
value in a dump is already in canonical *output* form — discrete ranges
canonicalize on input, array input whitespace is dropped — so the literal in
the file is the one `*_out` would write. That is what still carries `=`/`!=`
on a **nested** column, and on any column the register has no comparison for:
the field's own bytes are the value, so comparing them is right. What the
argument never covered is the *literal the user typed*, which is what typed
equality closes — see "Equality is typed too" below. It is also what a nested
column's own comparison had to be built around, since the input-side grammar
I20's scope limit flags as considerably more permissive than the `*_out`
inverse is exactly what a nested literal needs — see "Nested columns compare
structurally".

### Ordering operators compare typed, and the register says where that differs

**The heading is narrower than the section.** `=` and `!=` come through the
same register — see "Equality is typed too", below — and the heading is left as
it is because a dozen documents cite it by name; a keystone re-files this by
subject anyway.

`<`, `<=`, `>` and `>=` do **not** compare text. Each side is decoded with the
column's own decoder — the field per row, the filter's literal once, when the
block's schema resolves — and the decoded values are compared. That is the
whole reason they exist: `9 > 10` is false as text and true as an `integer`,
and a text comparison would be actively misleading on every numeric and
temporal column.

**They are available on a column that resolved `Mapped` and that the register
gives an order to, and refused on any other**, with the reason named: a column
whose type did not resolve has no order of its own (which is also what makes
`SchemaMode::Strings` refuse every ordering operator — it resolves nothing, so
the rule needs no case for it), and a column whose declared type the register
models no comparison for is refused too. A **nested** column is in the first
group where its shape and every position beneath it compare — an array, a
composite, a range and a multirange all do — and is refused otherwise, naming
the position; see "Nested columns compare structurally" below. Every
refused column keeps `Eq`/`Ne`, for the reason the paragraph above gives.

**Two faults are held apart, and both are named before the rows they would
otherwise corrupt.** A *literal* that is not a value of the column's type is
`Error::PredicateValueDecode`, raised once when the block resolves rather than
per row — which is also what makes decoding it once, rather than per row,
sound. A *field* that is not a value of its mapped type is
`Error::FieldDecode`, worded exactly as the typed build path words it, because
it is the same fault about the same value; projecting the column away does not
escape it, since the filter named the column.

*Rejected: rounding a literal finer than the column's scale.* A
`numeric(10,2)` column compared against `1.005` is refused rather than
silently rounded, because the literal is decoded with the column's own
decoder and that decoder does not drop non-zero digits. Rounding would need
PostgreSQL's half-even rule to be a comparison the user could trust, and a
refusal that names the value costs the user one edit.

*Rejected: excluding a row whose field does not decode.* It is the same
"unknown collapses to false" the NULL rule makes, and it would silently drop
exactly the rows a damaged file is made of. A NULL is a value the file
*states*; an undecodable field is the file contradicting its own DDL, which
everywhere else in this system is an error.

**PostgreSQL's special values are ordered, not undecodable**, and they are the
population that fault is held apart *from*. `infinity`, `-infinity` and a
`numeric`'s `NaN` are legal values of their declared types with a total order
PostgreSQL defines (I34): `-infinity` below every finite value, `infinity`
above, `NaN` above `infinity` and equal to itself. What cannot hold them is
**Arrow** — `Date32` has no infinity, `Decimal128` no NaN — so
`predicate.rs`'s `OrderKey` carries each as its *position* in that order
rather than as a number, and `compare_keys` decides a rank before it decides a
value. A sentinel would have been enough for `Date32`, whose `i32` leaves both
ends of an `i64` free, and not for `Timestamp`, where `i64::MAX` micros since
1970 is a date PostgreSQL itself accepts; widening the key to `i128` to keep
the sentinel literally free buys nothing the rank does not, and puts a wider
integer on the per-row path.

**`OrderKey` derives no `Ord`, and must not.** Its variants are comparable
only through `compare_keys`, which settles the rank first and descends to a
value pair only when both sides are finite — so a special against a finite is a
legitimate cross-variant pair that never reaches the value match. A derived
lexicographic order over the variant list would happen to agree with that
today and would stop agreeing the moment a variant is inserted.

The set is closed and each spelling is the one that type's own `*_out` writes,
so a `date` reading `Infinity` is still a decode failure — `date_out` writes
`infinity` and `numeric_out` writes `Infinity`, and each type is held to its
own — while a `text` column holding either word still compares bytewise.
`real`/`double precision` are the one absence, and it is load-bearing: IEEE
represents all three and their decoder already returns them, so nothing about
them is carried as a position.

**Which `numeric` column can hold an infinity is decided by the typmod, not by
the type.** `apply_typmod_special` rejects `±Infinity` under any typmod
restriction (I34), so a `numeric(p,s)` — whether it reached a `Decimal128`, a
`Decimal256` or, past 76 digits, `Utf8View` — can hold a `NaN` and never an
infinity, and the register's plan for it refuses the two spellings in a
*filter's literal* as well. Only a **bare** `numeric` admits all three. That is
the same shape as `oid`'s `UnsignedInt`: a distinction that exists purely to
refuse a literal, since no field could ever carry one.

**A filter is therefore exact where the batch still cannot hold the value.**
`--filter 'v_date < 2020-01-01'` selects `-infinity`'s row, and building the
`Date32` column for that row still raises `Error::FieldDecode`. That is a
property, not a defect: deciding an order needs strictly less than
materializing a value, and the two paths have different powers — the same
asymmetry projection already has, where projecting a column away escapes its
decode failure. Materialization is the open question, and it is `KD8`.

*Rejected: excluding the row instead, as a NULL is excluded.* A NULL is the
absence of a value and has no order; `infinity` has one. Excluding returns a
silently wrong answer, which is worse than the error it replaced.

*Rejected: waiting for the build path to represent these values.* It holds the
cheap correct answer hostage to the expensive one for a symmetry nobody asked
for. The error it kept in the meantime also named an escape that does not
exist for this case: `--schema-mode strings` resolves no column, so it refuses
ordering outright.

**The register** is which declared types such a comparison lands on, and
whether it means what PostgreSQL means. `pgtype.rs`'s `comparison_for` is the
authority, and it is **L2**: a pure function of the declared type string, the
column's own `COLLATE` clause and the database's own `CREATE TYPE`/`DOMAIN`
list, walking that string exactly as `resolve_declared_type` does — array
first, then the built-in table, then the user-defined lookup — so a domain
compares as whatever it bottoms out at and the two answers cannot disagree
about which types exist. Its result is a `ComparisonPlan`, carried per column
as a fourth positional vector in `ResolvedSchema` beside `columns`, `notes` and
`plans`. The table below is the human-readable rendering.

**It is asked per column, not per declared type**, and the collation is why:
two `text` columns of one table can compare differently, because one of them
declares `COLLATE "C"` and the other declares nothing. The comparison itself is
bytewise in every case — **reading the clause moves the verdict, never the
answer**. Four verdicts, and `pg_dump` writing a clause only where the collation
differs from the *type's* default (I37) is what makes the last one decidable:

| The column says | Verdict |
|---|---|
| a collation this dump declares `deterministic = false` | **Diverges** (`NonDeterministicCollation`), and it is the only collation verdict that reaches `=` |
| `COLLATE "C"` or `COLLATE "POSIX"`, qualified `pg_catalog` or bare | **Agrees**, on every server and under every libc |
| any other collation — a libc locale, an ICU collation, a user's own | **Diverges** (`NonBytewiseCollation`) |
| nothing, and the type's default collation is `C` — i.e. `name` | **Agrees** |
| nothing, and the type's default collation is the database's — `text`, `varchar`, `char(n)` | **Diverges** (`UnknownCollation`), because a plain dump does not record it (I32) |

**The determinism row is first because it is read off a statement, where the
other three are read off a name.** A `COLLATE` clause is joined against the
database's own `CREATE COLLATION` list (see "The preamble grammar and
`DumpMetadata`"), so what the register answers for one clause depends on what
the *rest of the file* said about it. The join parses both sides rather than
comparing them as text — the two spellings come from different `pg_dump` code
paths and either may quote what the other leaves bare — and an unqualified
reference matches on the name alone, which is the announcing direction and is
reachable only from a hand-written file, `pg_dump` writing both sides
schema-qualified.

The order of the four costs nothing: a non-deterministic collation is ICU-only
and therefore user-defined (I42), and `collation_is_bytewise` answers yes only
for `pg_catalog."C"`/`"POSIX"`, which no user-defined collation can be. It is
put first so that reading the branches top to bottom reads them in order of
evidence.

The schema is checked and not only the name: a collation called `"C"` in some
other schema is not `pg_catalog."C"`, and an *unquoted* `COLLATE C` names the
collation `c`, which the built-in is not. Answering "agrees" wrongly is the one
direction of error this register must not make, so anything it cannot resolve
to the two built-ins diverges.

**What the three verdicts are really sorting is whether the answer depends on
the server that wrote the file.** Call that server `S`. A collation whose
ordering is `memcmp` is **`S`-irrelevant**: it needs no libc version, no ICU
version, no platform and no major, so pgdq computes it exactly and the verdict
is *agrees, on every server* (I43). Every other collation is **`S`-relevant** —
its order is a function of `S`'s provider version, which no plain dump carries
(I32, I42) — and pgdq answers bytewise and says so. `UnknownCollation` and
`NonBytewiseCollation` are then the same verdict reached two ways: the first
does not know which collation `S` used, the second knows the name and not the
version, and neither is recoverable from the bytes.

That framing is what the register's own shape is for, and it is also the
boundary of what any amount of work here could close: the `S`-irrelevant set
can be widened by reading the file more carefully, and the `S`-relevant
remainder cannot be closed from the file at all, only by binding to an
environment a user asserts matches `S`. The roadmap's Future item
"Collation-aware comparison" is that second half, and it is unscheduled by
decision rather than by oversight — see [`roadmap.md`](roadmap.md).

**Some collations are bytewise in fact and divergent by this rule**, and that
is the asymmetry costing what it is supposed to cost rather than a defect. The
register decides from the collation's **name**, where I43 decides from its
provider and locale, so the `S`-irrelevant set is knowably wider than the two
names this build resolves. Three shapes sit in the gap, and they are not
equally closable:

- **`pg_catalog."ucs_basic"`** is `C` ordering in every supported major (I43),
  by two different routes — `initdb` writes `collcollate = 'C'` through v15,
  the catalog carries it in 16, and 17 moved it to the builtin provider. It is
  reported divergent because it is named neither `C` nor `POSIX`.
- **A collation the dump itself declares under the builtin provider** —
  `CREATE COLLATION x (provider = builtin, locale = 'C.UTF-8')`. This one is
  decidable *from the file*: `pg_dump` writes a user-defined collation's
  provider verbatim (I42) and the builtin provider's ordering is `memcmp`
  whatever its locale (I43), so nothing about the environment is being assumed.
- **glibc's `C.utf8` under the libc provider**, which sorts by code point on
  glibc and is not required to anywhere else. This one is *not* decidable from
  the file, and the difference from the case above is the whole point: it is a
  property of one libc, and the file names only the collation.

A **user-defined** collation is the same case at its sharpest: a dump can carry
`CREATE COLLATION public.c_collation (provider = libc, locale = 'C')` and a
column of it, saying in the same file that the collation is bytewise, and the
register still answers `NonBytewiseCollation` — because the name is not in
`pg_catalog`, and nothing stops a different user defining a non-bytewise
collation of their own with any name at all. Closing any of these would mean
the register deciding a collation's *behaviour* from somewhere other than its
name — for `ucs_basic` a second hardcoded name, for the builtin and libc
provider cases a second parser over `CREATE COLLATION` whose answer would still
not cover an ICU or a provider this build does not model.

**That is a property, not a deficiency, and the distinction is the row set.**
What a spurious divergence produces is *correct rows* with an advisory note the
user did not need: there is nothing to remedy and nothing to fix. `KD7` is the
list of places the order genuinely differs from the server's, and this is the
opposite shape — which is why a user collation earns no register entry and no
oracle case. `fixtures/<13-18>/types/default.sql`'s `t_collate.v_user` is what
stands in front of the cell, since "conservative and knowably wrong" is exactly
the answer a later session is most tempted to improve.

**`character(n)` is the fourth arm, and it reaches the same three verdicts
through a comparison of its own.** Its values are written blank-padded to `n`
while `bpcharcmp` calls `bcTruelen` on **both** operands before it looks at a
collation at all (I38), so the padding is not part of the value: `CompareKind::PaddedText`
trims trailing `0x20` off each side and what is left is exactly the `text`
question. Order matters and only one order is sound — trim, then consult the
clause — because the padding is what a collation would otherwise be asked to
rank. A `char(n)` column declaring `COLLATE "C"` therefore **agrees**, where
before every `char` column was told it diverged for a reason no clause could
fix.

The trim is admissible on the per-row path because it is not a decode: a
reverse scan for `0x20` yielding a shorter slice, no allocation and no `*_in`,
and only on a `char(n)` column. *Rejected: padding the literal out to `n`
instead.* It is sound for equality and unsound for ordering — a byte below
`0x20` sorts *under* the pad space where the server, having stripped the pad,
ranks the longer string above (I38's corollary), which is the disagreement the
oracle's tab-bearing `character(10)` cases are built out of.

**Keyed on the declared type, not on the Arrow one.** Six unrelated declared
types reach `Utf8View` — a text type, a bare `numeric`, a `timetz`, an `inet`,
a `jsonb`, and `json`, which PostgreSQL does not order at all — and they are
six *different* comparisons under one Arrow type, so the Arrow type cannot say
which one a column wants. Two pairs go further and are not even
distinguishable by declared type alone once the plan is built: `cidr` differs
from `inet` only in refusing a literal below its netmask, and `macaddr8` from
`macaddr` only in its width, so the plan carries each fact rather than
re-deriving it from a name that is gone by then. Neither can it reach the enum's
labels, which are what closes the enum row: they live on `TypeKind::Enum`, in
declaration order, and `CompareKind::Enum` carries them. That is why the plan
is not `Copy` — an enum's labels and a `numeric`'s typmod flag are facts about
the column, not about its Arrow type. The rows below are therefore in
declared-type order, with the Arrow type beside each as information rather than
as the key. `text`, `varchar` and `char(n)` are spread over four of the rows,
split by what the column's own `COLLATE` clause says — and `char(n)` is on a
different *comparison* from the other two, which is the second reason the
declared type has to be the key.

*Rejected: carrying the enum's labels in the Arrow field's metadata, so the
`DataType` still suffices as the key.* It smuggles a PostgreSQL fact into a
structure nothing type-checks it in — that metadata is *Arrow's* vocabulary,
which is why the two canonical extension names there are validated against
Arrow's own storage types — and it does it to preserve a key that is being
replaced for the reasons above.

*Rejected: leaving the register in L4 and threading its extra inputs through
`resolve_term`.* That signature grows by one argument per type that needs one
more fact, and it leaves L4 asking L2-shaped questions. `comparison_for` takes
four inputs today — the declared type, the column's own clause, the database's
types and its collations — all in hand at one call site in `resolve.rs`, where
resolution already reads them.

**The exhaustiveness check moved with it, and is stronger for the move.**
`builtin_scalar` answers both questions in one arm — which Arrow type a
built-in maps to, and how two of its values compare — so a mapping added
without a comparison does not compile, where before the two lived in different
layers and a new mapping only had to land on a `DataType` arm that already
existed. The user-defined half is an exhaustive `match` over `TypeKind` with
no wildcard, so a kind added to the preamble grammar has to choose too. And
`ComparisonPlan` makes "refused" and "has no way to decode a value" one
variant rather than a pairing that could come to disagree.

**`predicate.rs` reads the plan and never the Arrow type.** L4 asks how a
column compares; what it was mapped to is L2's business and stays there. The
three refusals an ordering operator can earn are still raised in
`resolve_term`, and the first two — not `Mapped`, and a nested `NestedPlan` —
are still decided before the plan is consulted, so a nested column is refused
with the reason that names its nesting rather than with "no order defined".

**One refusal is not an ordering refusal at all.** `ComparisonPlan` has a
fourth outcome, `Unanswerable`, for a column the file *states* the server does
not compare the way any reading of the file could — today exactly one producer,
a range type declaring a `canonical` function (see "Nested columns compare
structurally"). It is the one answer `Error::UnorderedPredicateColumn` cannot
carry, because that message ends by offering `=`/`!=` and here they are refused
too; it raises `Error::UncomparablePredicateColumn` instead. Everything else in
this table that is refused is refused for *order*, and answers `=` bytewise
over the file's own canonical text.

| Declared type | Arrow type | Agrees with PostgreSQL | What would close the gap |
|---|---|---|---|
| `boolean` | `Boolean` | yes — `false < true` (I33) | — |
| `smallint`, `integer`, `bigint` | `Int16`/`Int32`/`Int64` | yes | — |
| `oid` | `UInt32` | yes, over every value a column can hold — `oidgt` is C's `>` on two unsigned `Oid`s and so is this (I39). A filter *literal* carrying a minus sign is refused rather than wrapped the way `oidin` wraps it (`-1` is 4294967295 at every major from 13): a refusal the user can see, not a comparison that means something else | — |
| `real`, `double precision` | `Float32`/`Float64` | yes, **given the NaN rule** — `NaN` is above every value including infinity and equals itself (I33), which is neither IEEE's answer nor Rust's | — |
| `numeric(p,s)`, `p ≤ 76` | `Decimal128`/`Decimal256` | yes — both sides carry the column's own scale, because the literal is decoded with the column's own decoder, and `NaN` orders above every other value (I34) | — |
| `date` | `Date32` | yes — `infinity` and `-infinity` included (I34) | — |
| `time without time zone` | `Time64(µs)` | yes | — |
| `timestamp`, `timestamptz` | `Timestamp(µs[, tz])` | yes — compared as the stored instant, the two infinities included (I34) | — |
| `uuid` | `FixedSizeBinary16` | yes — `uuid_internal_cmp` is `memcmp` over 16 bytes (I33) | — |
| `bytea` | `Binary` | yes — `byteacmp` is `memcmp`, then length (I33) | — |
| enum types | `Dictionary(Int32, Utf8)` | yes — by *declaration* order, which is what `pg_enum.enumsortorder` records (I33) and what the dump carries verbatim in `CREATE TYPE … AS ENUM (…)`. A label the type does not declare is not a value of the column, so it is a decode fault on either side rather than a comparison | — |
| `text`, `varchar` declaring `COLLATE "C"`/`"POSIX"`, and `name` with no clause | `Utf8View` | yes — bytewise *is* what those collations order by, on every server and under every libc (I37) | — |
| `text`, `varchar` with no clause | `Utf8View` | **no** — their collation is the database's, which a plain dump does not record (I32); bytewise equals PostgreSQL only if that collation is `C`/`POSIX` | a collation the file does not carry |
| any column declaring a collation that is not `C`/`POSIX` | `Utf8View` | **no** — PostgreSQL orders it by that collation, which this build does not implement | one comparison per collation, i.e. a collation library |
| any column declaring a collation this dump declares `deterministic = false` | `Utf8View` | **no**, and under `=` as well as under `<` — a non-deterministic collation makes `texteq`/`bpchareq` something other than a byte comparison (I42), so two values spelled differently can be equal to the server | the same collation library, which supplies the equality with the order |
| `char(n)` declaring `COLLATE "C"`/`"POSIX"` | `Utf8View` | yes — the dump's blank padding comes off both sides first, which is `bcTruelen` (I38), and bytewise is what those collations order the remainder by | — |
| `char(n)` with no clause | `Utf8View` | **no** — the padding is handled, and what is left is the `text` row's residue: its collation is the database's, which a plain dump does not record (I32) | a collation the file does not carry |
| **bare `numeric`**, and `numeric` beyond 76 digits | `Utf8View` | yes — compared as an arbitrary-precision decimal over the text the file holds, which is `cmp_var_common`'s own value order and so insensitive to display scale: `1.5` and `1.50` are one value (I33). The bare form carries all three of `Infinity`, `-Infinity` and `NaN`; the constrained one carries only `NaN` (I34) | — |
| `interval` | `Interval(MonthDayNano)` | yes — compared as `interval_cmp_value`'s 128-bit span, so `1 mon`, `30 days` and `720:00:00` are one value (I40), with the v17 infinities read on every file (I34) | — |
| `time with time zone` | `Utf8View` | yes — the UTC-equivalent instant, then the stored zone, so two spellings of one instant are ordered rather than equal (I40) | — |
| `inet`, `cidr`, `macaddr`, `macaddr8` | `Utf8View` | yes — family, then the shorter netmask's worth of address bits, then the netmask, then the address (I40); the MAC types are their octets' own order | — |
| `jsonb`, and any domain over it | `Utf8View` | **no**, and only because of its strings — the structure is compared exactly as `compareJsonbContainers` compares it (kind, then a container's size, then member-wise, with a top-level scalar inside the pseudo-array that makes it outrank `[]`), while every string leaf and object key goes through `varstr_cmp` under the *database's* collation (I41), which a plain dump does not record (I32) | the same collation the text row wants, one level down |
| `json`, and any domain over it | `Utf8View` | **no** — PostgreSQL defines *no* comparison for `json` at all, so bytewise offers more than the server does rather than less | nothing, since there is no order to agree with |

| an **array** or **composite**, and any domain over one | `List(…)` / `Struct(…)` | **inherited** — compared structurally (I45), and it agrees exactly when every element or field type beneath it does. A `text[]` column carries the `text` rows' collation residue at its element; a `json[]` column, or a composite with a `json` field, has its *ordering* refused, because the server has no comparison for one either, and its `=` falls back to bytewise carrying that position's `json` row above | whatever would close the position that diverges |
| `int2vector` | `List<Int16>` | yes — the type names no operator of its own, so the server compares it through `anyarray` polymorphism: `array_lt`/`array_eq` over `smallint` elements (I47), which is I45's comparison with a fixed element and no position that could diverge. `'2' < '10'` is true, where a byte comparison of the same text says false | — |
| a **range** or **multirange**, and any domain over one | `Struct(…)` / `List(Struct(…))` | **inherited**, on the same rule, once both sides are put into the form the server stores them in (I46): `empty` below everything, then the bounds, with a multirange's members sorted, coalesced and emptied out first. Both discrete canonicalizations are reproduced — the successor shift, and the collapse of `[1,1)` to `empty` — so `int4range '[1,10]'` is the `[1,11)` the file holds. **A user-defined range that declares a `canonical` function is refused outright**, under every operator | — |

A domain has no row of its own: it compares as the row its base type is on,
through any chain, which is how the last row already covers "any domain over
them", and how a domain over `interval` picks up that row's comparison for
free.

**A literal is read in the type's own output form and no wider**, which is
where these four rows cost something. Each type's `*_in` accepts far more than
its `*_out` writes — `1.5 hours` and `P1Y2M` for an interval, an abbreviated
`10` for an IPv4 address, `08-00-2b-01-02-03` for a MAC (I40) — and the
register implements the output grammar alone, so a filter using one of the
other spellings is `Error::PredicateValueDecode` naming the value rather than a
comparison that means something else. That is `oid`'s refusal on a wider
grammar, and it is a **property rather than a deficiency**: the remedy is in
the user's hands, since every value the file holds is already in the accepted
form and `pgdq query` names the literal it would not read. Implementing more of
`*_in` would be re-implementing four input functions to accept spellings that
no dump contains.

**The integer arms are the one place the grammar is *wider* than the output
form, and the direction is harmless.** `order_key` reads them with
`str::parse`, which takes a leading `+` and any number of leading zeros —
spellings `int4out` never writes — so `--filter 'v_smallint=+0'` matches a
column holding `0`, and `--filter 'a={+1,02}'` an `integer[]` holding `{1,2}`.
Nothing is misread by it: every value the file holds is canonical, so the extra
spellings are unreachable from real output and appear only in something a user
typed. This is a property too, not a deficiency — but state it, because the
sentence above reads as absolute and is not.

The consequence worth naming is on the *other* side, and it is accepted rather
than unnoticed. `order_key` takes no `input` flag, so the same leniency reads
the dump: an `integer` field spelled `+1` is taken quietly instead of being
flagged as a file contradicting its own type, which is a fault everywhere else
here. Tightening it means a second scanner per `CompareKind` and an `input`
flag threaded to the bottom of every nested tree, bought to refuse spellings
`pg_dump` cannot emit — and the single shared function is what guarantees a
field and a literal denoting one value compare equal, which is the property the
split would put at risk.

**`jsonb` is the exception, and it is the exception for a reason.** Its
literal side implements the whole of `jsonb_in` — JSON with PostgreSQL's own
refusals, keys sorted into storage order and duplicates resolved to the last
(I41) — rather than only the spelling `jsonb_out` writes, because a JSON
document's canonical form is not something a person types: `{"a":1}`, with no
space after the colon, is what everyone writes and is not what the file holds.
The cost is a parser, and it is paid once rather than per spelling, which is
exactly what the four types above have no equivalent of. A number normalizes
through `numeric_cmp`'s own order, so `1`, `1.0` and `1e2` are one value, and
an exponent past ±100000 is refused — a bound of ours, reachable only by a
literal, since `jsonb_out` prints through `numeric_out` and that writes no
exponent at all.

*Rejected: normalizing an unknown literal by round-tripping it through the
type's own decoder.* For three of the four there is no decoder to round-trip
through — `time with time zone`, the network types and the MAC types have no
Arrow representation at all, which is why their comparison is a key rather
than a value, and building one to widen a literal grammar inverts the cost.
`interval` has one and it buys nothing: `decode_interval` and the ordering's
span read the *same* walk over `interval_out`'s grammar, so a literal that
fails the filter fails the decoder too, and round-tripping through it would
turn a refusal into the same refusal one step later.

*Rejected: an exception for `boolean`, on the grounds that `boolin` is cheap.*
The cost argument above genuinely does not reach it. `boolin` is two sentences
long — whitespace stripped, then a case-insensitive match against `t`/`true`/
`y`/`yes`/`on`/`1` and their negatives, with unique-prefix matching on the
words — so widening `boolean` would not be re-implementing an input function in
the sense the paragraph above means. What stops it is the test `jsonb` passes
and `boolean` fails. `jsonb` earns its exception because its canonical form is
*unreachable by hand*: nobody types the space in `{"a": 1}`, so an output-only
grammar would leave the type unfilterable. `boolean`'s canonical form is one
keystroke, and every value in the file is already in it. A widening that
stopped at `true`/`false` would be an arbitrary line through the same grammar,
and one that went the whole way would inherit prefix matching — `--filter
'flag=tr'` — a spelling nobody wants and this build would then owe forever.

**What the refusal owes the user is the accepted form, and that is a diagnostic
rather than a grammar.** `Error::PredicateValueDecode` names the value, the
declared type **and the form that column's `CompareKind` reads**, so a user who
writes `true` is told that a `boolean` is written `t` or `f` rather than told
that `true` does not parse as a `boolean` — the second is a claim about the
type, where what the user is short of is this build's grammar. `interval`,
`inet` and `macaddr`, the other three types the property is hard on, are
answered by the same clause list.

**The clause lives beside the grammar, not on the type.** `predicate.rs`'s
`accepted_form` is one clause per `CompareKind`, filed next to `order_key` and
`equality_comparison` — the two functions that decide what is accepted —
rather than on `CompareKind` in `pgtype.rs`, so a widening or tightening has
its own description on the same screen. Carrying the phrase on the L2 type
would put the sentence a file away from the L4 code it describes, which is how
a diagnostic goes quietly stale. `jsonb` needs the least of it, its literal
grammar being the whole of `jsonb_in`, and `text`/`character(n)` need none:
every string is a value of a text column, so their arm is unreachable and is
written out rather than made a panic on a diagnostic path.
[`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=`
compare values, not spellings", carries the same forms in prose.

**Two arms render the kind's own payload, because there the payload is the
answer.** Nothing forbids a diagnostic naming resolved schema data — the
question at each arm is whether the payload answers the user's question, and
for most of the table it does not. `CompareKind::Enum` carries the declared
labels and a mistyped or wrong-case label is the *only* way to fail an enum
filter, so the arm that could not answer was the arm that always fires; it now
names the labels, single-quoted with any interior quote doubled, the spelling
`CREATE TYPE … AS ENUM (…)` writes and a `--filter` value reads back, so a
printed label pastes straight into the term that was refused. `Decimal(scale)`
is the arm that was *false* rather than narrow: it refuses a literal finer than
the column's scale — `decimal_unscaled_digits` drops a trailing digit only when
it is zero — and told the user a `numeric(10,2)` is written "as a number, or
`NaN`", which `1.005` is. The clause now branches on the sign of the scale: at
most *s* decimal places, a whole number at `s = 0`, and a multiple of 10⁻ˢ for
the negative scales legal from PostgreSQL 15. Precision is not carried and
constrains no literal, since a value wider than the column can hold still
compares. The clause is a `String` built at the raise site, where the kind is
in scope: `Error::PredicateValueDecode` already carries three owned strings, so
the allocation is the existing price, and keeping the rendering in
`accepted_form` keeps it beside the grammar rather than pointing `error.rs` at
a type from a layer above it.

*Rejected: an uncapped label list.* `accepted_form` prints at most twelve
labels and then ", and *k* more; see `info --verbose`". A count cap keeps every
label it prints intact where a length cap on `map.rs`'s `TEXT_CAP` pattern
would truncate one mid-word and offer a spelling that is not a label; and the
overflow clause has somewhere to send the reader because `info --verbose`
prints an enum column's labels and lists every user-defined type's, both
uncapped ("CLI surface"). Uncapped fails on one real input — a generated schema
with a few hundred labels — where the message scrolls the error itself off
screen. The asymmetry with `info --verbose` is the same shape as
`arrow_type_label`'s elisions: a terse rendering is licensed by a complete one
existing where the user can reach it.

**The refusals are the rest of the table, and they are stated rather than
listed**: a nested column with an uncomparable position beneath it, and every
declared type this build maps to nothing at all. No container kind is refused
for being one — see "Nested columns compare structurally" below, which is where
the nested rows of this register live.

**A divergent comparison is announced by the CLI, once, on stderr**, after the
schema resolves, naming the column and the divergence — including for a query
that selected no rows, which is the case a user most wants explained. It is
announced per **term**, not per column, because a divergence is
operator-conditional: three of the six are divergences of *order* alone, so a
`text` column filtered with both `<` and `=` is warned about once. **A term can
raise more than one**, because a nested column has a divergence per *position*:
`ComparisonNote` carries a `path` — `[]` for an array's elements, `.label` for
a composite's field, `.bound` for a range's bounds and `[].bound` for a
multirange's, appended as the nesting descends — and the declared type
at that position rather than the column's, so `public.tagged`
(`(label text, tags text[])`) says both of the things it has to say. The
library's side of it is `TableStream::comparison_notes`, a **third channel** and
deliberately not a widening of either existing one: `DumpIndex.diagnostics` is
the L1 file-level channel and `ResolvedSchema.notes` is the L2 per-column one,
while this signal is per-column *and* conditional on a predicate — L4 — so
writing it into either is the layering violation `layering.md` forbids. What
an *embedder* should be handed instead is filed in
[`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md),
whose sink is the designated unification point for diagnostic channels.

**The rows reading "no" are three different things, and only one of them is a
deficiency.** The table's last column is what tells them apart — it says what
would close each row, and two of the three answers are "nothing anyone could
write".

**Two of them are properties of a plain dump, not of this build.** A
`text`/`varchar`/`char(n)` column with no `COLLATE` clause is on the
*database's* collation, and a plain dump does not record it (I32) — so there is
no fact to read and no code to write; the same statement one level down is
every string leaf and object key inside a `jsonb` document (I41), which is why
that row's structure is exact and its strings are not. And `json` has no server
order to disagree with at all: PostgreSQL defines no `=`, no `<` and no
operator class for it, so bytewise offers *more* than the server does. None of
the three has a remedy anybody could hold, which is what makes them properties
and not register entries. Each announces itself per query, so the answer is a
stated one rather than a silent one.

<!-- deficiency: KD7 -->
**The one that is a deficiency is `KD7`**: a column that *states* a collation
this build does not implement — a libc locale, an ICU collation — and whose
comparison therefore genuinely differs. There the file does carry the fact, so
the operators return a row set the server would not and something could be
written that closes it: a comparison per named collation, which is a collation
library.

**It reaches a nested column through its positions**, since divergence is
inherited: a `text[]` column, and a composite with a collatable field, land on
whichever of these four rows their element or field lands on, and announce it
with the position named ("Nested columns compare structurally"). Nothing about
the statement changes one level down — the same clause, the same four verdicts,
the same remedy.

**It reaches two operator families, and the file says which.** For a
*deterministic* collation — every libc one, and every ICU one the dump does not
say otherwise about — `varstr_cmp` returns zero exactly when the bytes are
equal, so `texteq`/`bpchareq` are byte comparisons and what is wrong is the
ordering alone: the data is right and `=` is right. A collation the dump
declares `deterministic = false` is the other half: `texteq` is then not a byte
comparison either, so `=` returns a row set the server would not. Both are
announced per term, the second under `=` as well as under `<`, so the wrong
answer is a stated one — but announcing is all this build does, and the row set
is bytewise either way.

That split is knowable because `pg_dump` writes `, deterministic = false`
unconditionally (I42) and this build reads it. It is the one thing about a
stated collation a plain dump settles: the *order* still needs a provider
version the file never carries, which is the next paragraph.

**The two families fail differently, and `=` fails in the safer direction.**
Byte-identical values compare equal under any collation, so under a
non-deterministic one this build's `=` returns **only** rows the server would
return — it misses the pairs whose bytes differ and whose collation makes them
equal, and admits none the server would reject. The filter is a subset: sound,
incomplete, and usable as such. Bytewise ordering carries no comparable
guarantee — it can place a pair in the opposite order the server would — so a
user who can act on a partial `=` has nothing equivalent under `<`. This is not
an invariant entry: it rests on a comparator returning zero for identical
input, not on any behaviour a `pg_dump` or server release could change, and an
invariant with no meaningful re-verification step is one nobody will check.

**The fix closes it up to a provider version, not absolutely**, and the entry
says so rather than promising more. A plain dump carries a collation's *name*
and never its version (I42), and PostgreSQL applies whatever the platform
provides for a name (I32) — `en_US.utf8` is `strcmp` on musl and glibc's
collation on glibc. So a collation library would agree with *a* server, not *the*
server, and the register's verdict for such a column would have to stay
conditional on the provider matching. That is still a fix, because the answer it
replaces is not approximate: it is bytewise, which is a different order
entirely.

**The entry is `(c) unowned`, and the intent that exists does not make it
`(b)`.** Complete collation support is the goal, **ICU included** —
`roadmap.md`'s Future item "Collation-aware comparison" holds it, in two
increments split by whether an answer depends on the source server: widening the
`S`-irrelevant set, which the file settles on its own, and then delegating the
`S`-relevant remainder to a provider in the environment pgdq runs in, which
reaches ICU without this build carrying a collation rule. An ICU collation
under the *oracle* still costs a guard the libc cases do not, since its
`collversion` moves with the base image, and that cost belongs to whichever
increment puts ICU cases in committed bytes. But a Future
item is not a phase, and `(b)` names a destination the phase index can resolve.
That item is what would promote the entry. *Rejected: a promotion
trigger reading "a dump whose text columns state a real locale".* Our own
`fixtures/<13-18>/types/default.sql` is such a dump — `v_text_locale text
COLLATE pg_catalog."en_US.utf8"` — so the trigger was satisfied by the apparatus
that tests the arm, while koji, 784 GB across 75 tables, carries no `COLLATE`
clause at all. A trigger a fixture fires and no real input does is not a trigger.
*That shape has now been proposed twice.* The second candidate — "a dump
declaring a collation `deterministic = false`" — looks sharper, being an
unambiguous fact the file states rather than a name that may or may not matter,
and it fails identically: the fixture family acquires exactly such a collation
to test this arm, so the apparatus fires it and no real input has yet.

*Rejected: a separate `KD<k>` for the equality half.* The case for splitting was
that the file settles equality outright where the order needs a provider version
— but that conflates *detecting* the divergence with *closing* it. `texteq`
takes its memcmp shortcut only `if (locale_is_c || pg_locale_deterministic(…))`
and otherwise falls through to `varstr_cmp(…) == 0`, so answering which values
are equal needs the ICU provider exactly as the order does; I42 says the file
settles *that* `=` diverges and stops there. The two halves therefore share a
trigger, a fix and a blocker, and two entries would carry the same sentence, the
same `(c)` stance and the same owner. What genuinely differs is the *note* — the
equality announcement cannot be spurious where the ordering one can — and that
is a property of the announcement, not a second defect. Reasoning in
[`../status/history/2026-09-01.md`](../status/history/2026-09-01.md), "`KD7`'s
equality half is the same defect, not a second one".

*Rejected: closing this as a property alongside the other three, so that `KD7`
could be struck as the typed-predicates work had undertaken to.* The three that
closed are properties because nothing anybody could write would change the
answer — the database default is not in the file, and `json` has no server order
at all. This one is the
opposite: the file states the collation, the row set is wrong, and the code that
would fix it is nameable. Calling it a property to complete a strike is the
"property filed as a deficiency" rule running backwards, and it is the one
direction the register cannot recover from — a struck number reinstated is a
state it has no way to express, where an entry kept and struck later costs one
edit. The undertaking was amended instead; reasoning in
[`../status/history/2026-09-01.md`](../status/history/2026-09-01.md), "`KD7`
survives at one statement rather than being struck".

A stated collation that is bytewise **in fact** and not named `C`/`POSIX` —
`ucs_basic`, a user's `CREATE COLLATION … FROM "C"` — is not this entry: it is
the spurious-divergence case above, where the register answers on the strength
of the name rather than the behaviour, which produces correct rows and an
advisory note the user can ignore. There is nothing there to fix.

The rest of what this entry once covered has closed as the register was
re-keyed off the declared type: the enum, by reading the declaration order the
dump carries; the bare `numeric`, by comparing arbitrary-precision decimal
rather than text; `interval`, `time with time zone` and the four network types,
each by its own comparison (I40); `jsonb`'s structure, by walking two
containers (I41); the collation *verdict*, by reading the clause (I37); and
`char(n)`'s blank padding, by trimming both sides before the clause is
consulted (I38).

**Adding an arm here obliges an oracle case**, and that is checked rather than
remembered: `scripts/oracle_register.py` fails when a register arm has no case
answering for it (see "The register-to-oracle reconciliation"). It is the one
reason to touch `scripts/comparison_oracle.py` while closing a row of the table
above. What that check cannot say is whether the arm's cases reach every branch
of the answer, so the cases are owed by whoever writes the arm — and once
written, every one of their cells is asserted against this register (see "The
register against the oracle's answers"), which is where a row that closes and
then quietly stops agreeing is caught.

*Rejected: a test asserting the Markdown table above and `comparison_for`
agree row for row.* Its own failure mode is bit-rot in the doc parser, and the
table is small enough to be re-read whenever the register changes. What
`pgtype.rs` carries instead is a hand-written Rust table naming every arm of
`builtin_scalar` and its plan — an ordinary test over the register, not a
parser over the document.

**On the command line the operators are spelled as they read.** How a term is
split into its three parts is the next section.

### Equality is typed too

`=` and `!=` route through the same per-column `ComparisonPlan` the ordering
operators do, and for the same reason: the justification the old text
comparison rested on — every value in a dump is already in canonical `*_out`
form — is true of the **field** and says nothing about the **literal the user
typed**. `--filter 'v=1.5'` matched no row of a `numeric(10,2)` column written
`1.50`; `--filter 'v=a'` matched no row of a `char(5)` column, whose values are
blank-padded. A plausible command line and an empty result that reads like an
answer is the shape this closes.

**Three canonicalizations, not two states**, decided by one question about each
`CompareKind`: is the file's `*_out` text a *unique* spelling of the value it
holds?

| How | Which kinds | What happens per row |
|---|---|---|
| **Canonicalize the literal once** | everything not below — `text`, `varchar`, `name`, the integers, `oid`, `numeric(p,s)`, `date`, `time`, `timestamp`, `timestamptz`, `uuid`, `bytea`, an enum, `macaddr`/`macaddr8` | a byte comparison; nothing moved |
| **Narrow the field per row** | `character(n)` | trailing `0x20` off both sides, then a byte comparison |
| **Decode both sides per row** | `real`, `double precision`, bare `numeric`, `interval`, `jsonb`, `time with time zone`, `inet`/`cidr` | the same `OrderKey` comparison `<` makes, asked for `Equal` |

So `=` on a `text` or `varchar` column is exactly the compare it always was —
`CompareKind::Text` renders the literal to itself — which is the rare shape
where correctness improves and the hot path does not move.

**Five kinds decode because `*_out` is not injective over the values one file
can hold.** A bare `numeric` keeps its display scale, so `1.5` and `1.50` are
one value written two ways (I33); an `interval` collapses months to 30 days and
days to 86400 s, so `1 mon`, `30 days` and `720:00:00` are one value written
three ways (I40); `jsonb` prints its numbers through `numeric_out` and compares
them by value, so `{"a": 1.50}` and `{"a": 1.5}` are one document written two
ways (I41); and `real`/`double precision` have two zeros, `-0` being a value a
dump can write and `float8eq` calling it equal to `0`. No rendering of the
literal makes any of those a byte comparison.

**Two more decode for a reason about this build rather than about PostgreSQL.**
`time with time zone` and `inet`/`cidr` *are* uniquely spelled by their output
functions, but reproducing those spellings means re-implementing
`EncodeTimeOnly` plus `EncodeTimezone`, and `pg_inet_net_ntop`'s IPv6 zero-run
compression. Writing an output function to save a fixed-size parse per row is
the wrong trade, and getting one subtly wrong is a silently empty result rather
than an error. `macaddr` goes the other way on the same test: its output rule
is two sentences long — lowercase hex pairs joined by colons — so it renders.

**A literal is read on the same output-form-only grammar the ordering operators
use**, so `--filter 'flag=true'` on a `boolean` column is
`Error::PredicateValueDecode` naming the value, where `boolout` writes `t`, and
`--filter 'v=2020-01-01'` on a `timestamp` column is refused, because
`timestamp_out` writes a time part and `decode_timestamp_micros` requires one.
Both replace an empty result with a named refusal, which is the failure shape
that motivated the change.

**That second one is not the same class as the cases this mechanism closes**,
though it was first filed as though it were. `1.5` against a `numeric(10,2)`
written `1.50`, and `a` against a padded `char(5)`, are cases where the literal
*is* a well-formed value of the column's type and only the file's spelling of
it differs — rendering the literal closes them, and does. `2020-01-01` against
a `timestamp` is not a `timestamp_out` spelling at all, so no canonicalization
of the literal reaches it; closing it means accepting more *input* grammar,
which puts it in the `*_in`-wider-than-`*_out` property two sections up beside
`1.5 hours` for an interval and an abbreviated `10` for an IPv4 address. A
spelling problem and a grammar problem look alike from the command line and
have nothing in common behind it.

**A match there would also be a trap.** PostgreSQL reads
`'2020-01-01'::timestamp` as midnight exactly, so what a user would get from
`v=2020-01-01` is the instant and not the day — which is usually not the
question they meant. The refusal costs one edit; the match would cost a wrong
answer that reads like a right one, which is the shape this section exists to
remove.

**The divergences are operator-conditional**, which is why the note channel is
`TableStream::comparison_notes` and its notes are per *term*. Three of the six
reach ordering alone — `UnknownCollation`, `NonBytewiseCollation`,
`JsonbStringCollation` — and one fact settles all three: those collations are
*deterministic*, so `varstr_cmp` returns zero exactly when the bytes are equal
and `texteq`/`bpchareq` are byte comparisons whatever the collation is. A
`text` column with no clause therefore warns under `<` and is silent under `=`,
which is not a softening: it is the register saying the row set an `=` returns
is the server's. `ComparisonDivergence::affects_equality` is the whole rule.

**Determinism is what that rule turns on, and one variant is the exception the
file states.** `NonDeterministicCollation` is a column whose `COLLATE` clause
names a collation the same dump declared `deterministic = false` (I42) — an ICU
collation, always, the server refusing the option for every other provider.
Under one, `texteq` is not a byte comparison, so two values spelled differently
can be equal to the server and `=` here returns a row set the server would not.
It is the **only equality divergence a plain dump carries outright**: everything
else the register cannot answer for a text column is a fact the file omits (the
database's own collation, I32) or a version it never records (the provider's,
I42), where this one is written into the file as a clause. The register reads
it and announces under both operator families; the comparison itself does not
move, and the deficiency is `KD7`.

**Both halves of that read are pinned by committed bytes**, not by a
hand-written statement: `fixtures/<13-18>/types/` carries `CREATE COLLATION
public.nd_collation (provider = icu, deterministic = false, locale = 'und')`
beside a deterministic `libc` one, and `t_collate.v_nd` is a column of it whose
clause is spelled exactly as `v_user`'s is. The two columns are
indistinguishable by name and answer differently, which is what makes the
verdict a read of the *statement* rather than of the reference.

The other two `true` answers are not about a collation at all: `AsText`, where
PostgreSQL defines no `=` for `json` any more than it defines an order, and
`UnmodelledType`, below.

<!-- deficiency: KD10 -->
**A column the register has no comparison for still answers `=`, as text, and
`UnmodelledType` is where that stops being safe.** The four ordering operators
are refused on such a column; equality is not, because for most of these types
the field's own bytes are the value. `box` is the member that proves it is not
all of them: `box_eq` compares *areas*, so the server calls `(1,1),(0,0)` and
`(3,3),(2,2)` equal and a byte comparison does not — which the committed oracle
holds at six majors (`fixtures/<13-18>/oracle/comparisons.tsv`,
`public.box_domain`, and two entries in the register-against-oracle exception
set). That is `KD10`, and the note is announced for exactly the two column
resolutions that mean *the file named a type and this build models nothing for
it*: `UnknownType` and `OpaqueBaseType`. Whether a given member is like `box`
or like `money` — whose `=` is its value's and whose `*_out` is unique per
value, so bytewise agrees — is what the register does not know, and announcing
is the conservative answer.

*Rejected: refusing `=` on such a column as ordering is refused.* It takes a
working capability away from every type in the group to protect the geometric
handful, and for `xml`, whose `=` PostgreSQL does not define at all, filtering
by exact text is a thing a user legitimately wants.

**The other two populations reaching this fallback are a nested column that
fell out of its tree, and a column with no DDL behind it at all.** The second
is silent and has nothing to say: `--data-only` and `--schema-mode strings`
leave nothing said about the type to qualify, and `ResolvedSchema::notes`
already reports that on L2. The first **announces per position** — see
"Nested columns compare structurally", which is where the rule lives.

**`box` beneath a container is `KD10` one level down, and it is not
announced.** `box_eq` compares areas, so a `box[]` column's bytewise `=` is
wrong exactly as a `box` column's is — but it is not the nested-tree
population: I22 resolves the *column* to text, so the announcement is keyed on
`ColumnResolution::OpaqueElementType`, which is not one of the two outcomes
above. Widening the keyed set is the fix and it belongs with whatever closes
`KD10`, not beside the nested rule: the position is not what makes the answer
wrong there, the element type's own `=` is.

**The oracle asserts all six operators now.** `=` and `<>` are asked over a
wider population than the other four, since equality is never refused, and both
operands of a cell are values the server itself stored — so for a canonicalized
kind the assertion is that the file's own spelling compares byte for byte, and
the content is in the kinds where it cannot. Every one of those has a case:
`real`'s `-0` against `0`, a bare `numeric`'s `1.5` against `1.50`, a `jsonb`
number written two ways, and a `character(10)` value padded against one that is
not. See "The register against the oracle's answers".

### Nested columns compare structurally

Every container kind — array, composite, range, multirange and `int2vector` —
gets the four ordering operators **and** `=`/`!=`, compared the way
`array_cmp`, `record_cmp`, `range_cmp` and `multirange_cmp` compare them rather
than as text (I45, I46, I47).

**The plan is a tree, and it is built by the same walk the register already
makes.** `pgtype.rs`'s `NestedCompare` has one node per nesting level —
`Array(element)`, `Record([(name, field)])`, `Range { bound, discrete }`,
`Multirange { bound, discrete }`, and a `Leaf` carrying the position's declared
type, its `CompareKind` and its `ComparisonDivergence`. Each position is filled
by asking `comparison_for` the question a *column* of that type would have been
asked, so nesting composes with no special case and a domain beneath a
container bottoms out where a domain always does. `ComparisonPlan` gains a
`Nested` variant for it, and the question "does this column have an order"
becomes `ComparisonPlan::orders()` rather than a match on the variant — because
a nested plan is not by itself an order.

**One `Array` node whatever the dimensionality.** An `array_out` literal
carries its own shape and `nested::ArrayLiteral` flattens it row-major, so
`integer[]` is one node whether its values are vectors or matrices — which is
also why the plan's depth may legitimately differ from the *Arrow* list depth
the census settled on.

**`Int2Vector` is a sixth node and it carries nothing.** The type's element is
`smallint` in the catalog and nothing about a column can vary it (I47), so
there is no position beneath it that could refuse an order or announce a
divergence — the walk visits it as the leaf it effectively is, declaring
`int2vector`. It is a node rather than an `Array` over a `smallint` leaf
because the two sides are read in a *different grammar*, which is the same
distinction `NestedPlan` draws one layer down; the comparison itself is
`array_cmp`'s, reached through the `NestedKey::Array` the two grammars both
build. Its `dims` and `lower_bounds` are `[n]` and `[0]` for every value, the
empty vector included — `int2vectorin` sets `ndim = 1` and `lbound1 = 0`
unconditionally, where `array_out`'s `{}` is zero-dimensional — so
`array_cmp`'s dimension tie-breaks are constant between two values of this type
and only the elements and their count decide.

**A range is compared in the form the server stores it in, not the form it was
written in**, and that is the one container whose comparison has a rewriting
step in front of it. `range_in` runs every literal through `make_range`, which
is two things: `range_serialize`'s type-independent checks — a lower bound above
its upper is `22000`, bounds that meet without both ends including the point are
`empty`, an absent bound is never inclusive — and then, for a range type that
has one, the *canonical function*, after which those same checks run again.
`predicate.rs` reproduces both, over the decoded bound **keys** rather than over
text, so `int4range '[1,10]'`, `'[1,9]'`, `'(0,10)'` and `'(0,9]'` are the one
value the file holds as `[1,11)`, and `'(1,2)'` is `empty` (I46).

**The successor is `OrderKey::Int` plus one, and that pattern *is*
`daterange_canonical`'s infinity guard** rather than an approximation of it. All
three built-in canonical functions differ only in the width they overflow at,
and `daterange` additionally skips a bound that is `DATE_NOT_FINITE` — which
here is a bound whose key is `PositiveInfinity` or `NegativeInfinity` (I34), so
it matches no arm and is left exactly as written. That is what keeps
`[2020-01-01,infinity]` inclusive at the top while `[-infinity,2020-01-01]`
still becomes `[-infinity,2020-01-02)`. The overflow is refused where the key
can see it (`int8range` at `i64::MAX`) and not where it cannot: a leaf literal
is read as `i64` whatever the column's width, which is the property "a literal
outside a `smallint`'s range still orders correctly" one level down, so
`int4range '[1,2147483647]'` is accepted here and refused by the server. It is
an over-acceptance whose only effect is a filter that matches nothing.

**Canonicalization is keyed off the range *type*, never off its subtype.** Only
`int4range`, `int8range` and `daterange` — and their three multirange
companions — have a canonical function among the built-ins, so `numrange` does
not canonicalize even though a fixed-scale decimal has a successor, and
`public.myrange`, a user range over `double precision`, does not either. The
flag comes from `builtin_range_subtype`'s hardcoded table for a built-in name
(I10) and is `false` for every user-defined range.

**A user-defined range that declares a `canonical` function is refused under
every operator, `=` and `!=` included.** `pg_dump` writes the parameter —
`,\n    canonical = %s` whenever `rngcanonical` is set, after
`multirange_type_name` and before `subtype_diff` (I10) —
`parse_create_type`'s `AS RANGE` arm keeps it beside `subtype`, and
`comparison_user_type` answers `ComparisonPlan::Unanswerable` for any column
whose type is or contains such a range: the range itself, its multirange
companion, an array of it, a composite holding one, a domain over it. A filter
term on such a column raises `Error::UncomparablePredicateColumn`, naming the
range type and the function; `IS NULL`/`IS NOT NULL` still answer, since they
read no value.

*Rejected: comparing it without the canonicalization.* A user's canonical
function is arbitrary server-side code, so knowing it exists licenses declining
the column and nothing more — and the server applies the rewriting **before the
value is stored or compared** (I46), so a column of such a type answers
`[1,10]` ≠ `[1,11)` where the server calls them one value. The shell
declaration does not save it: `pg_dump` emits `CREATE TYPE x;` ahead of a range
whose canonical function it dumps, and `record_type`'s I11 rule makes the shell
lose, so the column resolves on its subtype and the ordinary bound-wise
comparison is reached. Refusing costs almost nobody an answer — a canonical
function must be declared against the shell type, and a SQL function cannot
take one (`ERROR: SQL function cannot accept shell type`), so it takes a C or
internal-language function, in practice an extension or a hand-loaded module.
`fixtures/*`'s two user ranges declare none, which is why the end-to-end
evidence is a hand-written dump in `tests/ordering.rs` rather than a fixture
column.

*Rejected: `ComparisonPlan::Refused`.* `Refused` on a *mapped* column refuses
the four ordering operators and lets `=`/`!=` fall through to
`Comparison::Canonical`, a bytewise text comparison announcing nothing — which
is right at every site that answers it (an empty enum, a C-level base type, a
column with no DDL behind it) and leaves exactly the half this defect is made
of. Hence a fourth outcome rather than a reason field on the third: a new
variant leaves those sites alone and makes every match on a `ComparisonPlan`
choose, which is the property that surfaced the gap in the first place.

*Rejected: an `Uncomparable` position in the tree.* A `NestedCompare` tree can
say "this position has no order" and let the column still answer `=` as text;
it has no way to say "and no equality either", which is a fact about the whole
column rather than about one position. So `nested_position` returns a `Result`
and an unanswerable position short-circuits out of every walk that reaches it —
comparability is inherited by the walk, unanswerability by propagation.

**A multirange is normalized before it is a value at all.**
`multirange_canonicalize` sorts its members, drops the empty ones, and merges
any two that overlap or touch — so `{[5,10),[1,5)}`, `{[1,5),[5,10)}`,
`{[1,1),[1,10)}` and `{empty,[1,10)}` are all `{[1,10)}`. After it, no member is
empty and no two members meet, which is what lets the comparison itself be a
plain sequence walk: member-wise, then the shorter multirange first. Whether two
members *touch* is again the range type's question — `{[1,5),[6,10)}` stays two
members in `int4multirange`, because `[5,6)` is a real range, while
`{[1,5],[6,10)}` becomes one, the first member having already canonicalized to
`[1,6)`.

**Both sides go through the rewriting, not only the literal.** It is idempotent
on a `range_out` field by construction — the server applied it before writing
the file — so one code path serves both grammars, exactly as `nested_key`'s
`input` flag does one level up. *Rejected: rewriting the literal alone.* It
saves nothing measurable and adds a second path whose only coverage would be
the assertion that the field side needed none.

**A range's bound is asked with no `COLLATE` clause, and that is knowingly
weaker than the file.** A range type carries its own `collation` parameter —
`fixtures/*/types/default.sql`'s `public.textrange` declares
`collation = pg_catalog."C"` — and the preamble grammar keeps `subtype` and
`multirange_type_name` alone, so a `text`-bounded range reaches the no-clause
arm and is told its collation is the database's. It is the same shape as the
user-defined collation above: a range declaring `C` gets correct rows and an
advisory note it does not need, and one declaring anything else gets exactly
`KD7`'s statement. Reading the parameter would move the verdict and never the
answer, which is what makes it a property here rather than a deficiency.

The note it produces reads `` `v_textrange.bound` (text) is compared bytewise:
the column declares no COLLATE clause `` — true of the column, and the sentence
is `UnknownCollation`'s own rather than one written for this position. *Rejected:
a divergence variant, or a message arm, saying "the range type's clause is not
read".* It would be a second sentence to keep in step with the first for a case
whose row set is correct, and it invites the reader to add a `COLLATE` clause to
a column that cannot carry one — a range type is not collatable, so the clause
lives on the `CREATE TYPE` or nowhere. The honest fix is to read the parameter,
not to reword the note.

**Comparability is inherited, and so is divergence.** A position whose declared
type has no order here is a `NestedCompare::Uncomparable` leaf; one anywhere in
the tree makes `orders()` false, refuses the ordering operators, and lets the
refusal name the position and its type — `` `[]` inside it is `json` ``. A
position that *is* ordered but not the server's way carries its own divergence,
so a `text[]` column diverges for the reason its element does, one level down,
and the note's `path` says where.

**`json` beneath a container refuses the order and announces the equality**,
and that is the one inheritance rule that is not simply "recurse". At top level
`ComparisonDivergence::AsText` means bytewise where the server orders not at
all, which is *more* than the server offers. Inside a container the order is
not available: `array_cmp` looks up the element type's comparison proc and
raises when there is none (I45), so a `json[]` column and a composite with a
`json` field have no `=` and no `<` on the server either.

The column does not stop answering `=` for that, though: it falls back to a
byte comparison of the container's whole `array_out`/`record_out` text. So the
position carries a `ComparisonDivergence::AsText` of its own, and the note names
it the way the ordering refusal does — `` `v[]` (json) is compared bytewise:
PostgreSQL defines no comparison for this type at all ``. It is the same
sentence a top-level `json` column earns, one level down, and it answers the
question the refusal otherwise leaves hanging: *the ordering operators told me
why they stopped; what did `=` just do?*

**Only the `json` arm carries it.** `NestedCompare::Uncomparable` also holds
positions that `ComparisonPlan::Refused` produced — an empty enum, a C-level
base type, a range with no declared subtype — and that answer says this
*build* has no order for the position, which is not a claim about the server's
equality in either direction. Those carry no divergence and announce nothing.
Neither do the two positions `array_comparison` refuses ahead of the walk (I22,
I26): the same refusals resolve the column itself to text, so `resolve_term`
drops the plan on the resolution one branch before it reaches the tree, and
what such a column announces is keyed on that resolution instead — which is
where `box[]` sits, and why it stays silent ("Equality is typed too").

*Rejected: announcing `ComparisonDivergence::UnmodelledType` for every
uncomparable position, or `AsText` for every one.* Each is one sentence over a
population that does not share a fact. `AsText` says the server has no
comparison here, which is true of `json` and false of `public.intarr[]` —
PostgreSQL orders it through `array_ops` and the bytewise answer agrees with
it, so announcing would be a warning about a correct row set. `UnmodelledType`
says the opposite and is false of `json`. Carrying the divergence on the
position is what lets each arm say only what it knows, and it costs one
`Option` field on a variant that already carries the declared type.

**That announcement is the one place the nested comparison's evidence is
thinner than its code**, and it cannot be thickened from either side that
usually thickens it: no fixture carries a `json[]` column, and no oracle case
could carry one either, since `json[] = json[]` is an error on the server
rather than an answer. So it is asserted by unit test where every other nested
rule is asserted against committed bytes at six majors. Adding the fixture
column is the whole cost of a six-major regeneration for a case no server can
be asked about.

**The two sides read two grammars, and the leaf grammar does not widen with the
container.** `predicate.rs`'s `nested_key` walks the plan against one text
value and takes a flag saying which: a *field* comes out of the dump in
canonical `*_out` form and is read with `nested.rs`'s strict `decode_*`, while
a *literal* is what the user typed and is read with the `array_in`/`record_in`
supersets (`parse_*`, I44) — so `--filter 'tags={a, b}'` means what it looks
like. **The flag reaches the container arms and stops there.** A **leaf** is
read by `order_key` with no flag at all, so a field and a literal go through
the identical function — which is what makes the two sides comparable in the
first place, and is why `--filter 'p=( 1 , a )'` is refused where `record_in`
would have kept the blanks and had `int4in` throw them away. That is the
property "A literal is read in the type's own output form and no wider" asked
one level down, and it keeps one rule at every depth.

**The rule is not a nested one, and it already bites at top level.**
`order_key` is `str::parse` for a number, so `--filter 'v_integer=" 42 "'` —
quoted, where the term grammar's outside-quotes trim does not reach — is
refused today, while `fixtures/<13–18>/oracle/literals.tsv` records the server
accepting `integer ' 42 '` and writing `42` at all six majors. The container
walk did not make that call; it made it visible one level down, where no trim
stands in front of it.

*Rejected: trimming a record field's or a range bound's whitespace before
applying the leaf grammar.* It is what the server does — but only for the types
whose `*_in` skips whitespace, and which those are has to be read rather than
guessed: `uuid_in`, `byteain`, `enum_in` and `network_in` do not skip, while
`macaddr_in` — a chain of `sscanf("%x:%x:…%1s")`, where `%x` eats leading
blanks and the trailing guard matches only non-whitespace — does. For two of
the four a trim is worse than lenient, because it **silently changes the
value**: `byteain`'s escape format takes a leading blank as a data byte, and an
enum label may legitimately be `' a '`, so the trim would compare against a
different value rather than accept a wider spelling of the same one. A per-type
rule is cheap in code — one predicate over `CompareKind` — and expensive in
**evidence**, which is what actually stops it: every value in the
register-to-oracle test is put through the server's `*_out` form by
construction, so no oracle cell can check an input-grammar arm, and twenty
hand-asserted claims about twenty input functions is the kind of unbacked
agreement this register exists to refuse. And the one spelling it would buy is
one `array_in` already accepts a level up.

*Rejected: carrying the refusing position as structured data on the refusal
itself* — a payload on `ComparisonPlan::Refused`, or a variant of its own for
"refused because a position inside it has no order". **The plan already is the
structured channel.** `ComparisonPlan` and
`NestedCompare` are public, `ResolvedSchema::comparisons` is public, and
`NestedCompare::uncomparable()` answers `(path, declared type)` — so an
embedder branches on *which* position refused without reading a sentence, and
`pgdq info` reaches it exactly where it already reads an enum's labels off the
same plan. `Error::UnorderedPredicateColumn::reason` is a rendering of that
answer for a person, which is why it is a `String` and why nothing is lost by
its being one; a payload would be a second copy of `NestedCompare`'s own
result, kept in step by hand. What is genuinely absent is an enumeration —
`uncomparable()` answers the *first* such position, and a composite with a
`json` field and a `box` field names one. That is deliberate: fixing the first
position is what the user must do next, and a second name does not help until
the first is gone.

**One comparison path, not the scalar's three.** `Comparison::Nested` serves
both operator families: the key walk decides the order, and `=` is that walk
answering `Equal`, which is `array_eq`'s and `record_eq`'s answer too because
`array_cmp` returns zero on exactly the condition `array_eq` returns true.
*Rejected: canonicalizing the literal back into the file's spelling and
comparing bytes, as a scalar `=` does.* A nested value is byte-comparable only
when every leaf beneath it canonicalizes — one `numeric` or `character(n)`
element makes the rendered literal the wrong answer — so the fast path is a
second literal-side walk whose *fallback* half is the one that must be right,
and the comparison oracle carries no nested case that exercises it. One path,
covered by every nested cell of the oracle, beats two of which the oracle
exercises one.

**The NULL rule is one rule at every level: two NULLs are equal, and NULL sorts
above not-NULL** (I45). It covers ordering and equality alike, which is what
makes a nested comparison **two-valued throughout** — it never yields
`Truth::Unknown`. Only the whole field being NULL (`\N`) makes a nested term
unknown, exactly as for a scalar.

**A census-refused array column compares as the column resolved.** The two
shapes that resolve to text while still holding array literals —
`NestedArrayElement` and `VaryingArrayShape` ("Joining a header against the
metadata") — keep comparing as text, because `resolve_columns` zeroes the
comparison plan for any column the census took out of `Mapped`. The register
agrees rather than merely being overridden: `comparison_for` mirrors
`resolve_array`'s two refusals, so an array whose element is opaque (I22) or is
itself an array (I26) answers `Uncomparable` at that position. One rule —
compare as the column resolved — and no column whose meaning depends on which
rows a query happened to scan.

### A filter term is parsed for two audiences

`parse_filter` in `pgdump_query-cli/src/main.rs` is the grammar, and it is the
CLI's alone: `Predicate` is a plain public struct an embedder fills in field by
field, so nothing below L4 parses `column=value` and none of the trimming or
unquoting below reaches an embedder's values. It is reached two ways and they
are not the same entry point: a `--where` leaf calls it directly, while a
`--filter` argument goes through `parse_filter_flag`, which first refuses a
term the expression grammar would read as structure — see "`--where` builds an
expression out of those terms" below.

Two kinds of user read `--filter` differently and the grammar serves both
rather than choosing. Sysadmin-shaped users find the bare `column=value`
spelling natural. SQL-fluent users assume a string literal must be quoted and
write `--filter 'foo = "the answer"'` — double quotes rather than SQL's single
ones, because the term is already inside shell single quotes. Both spellings
mean what they look like.

**A term is split at the earliest operator position outside quotes, longest
spelling first.** Longest-first is what keeps `>=` from being read as `>` with
a stray `=`, exactly as `!=` has always beaten `=`; earliest-position is what
keeps a *value* containing an operator byte from stealing the split, so
`name=alpha>x` is `name` equal to `alpha>x`. `split_filter_op` walks bytes and
compares bytes — every character it looks for is ASCII and no byte of a
multi-byte UTF-8 character is, so a match is always at a character boundary,
where matching through `str` would panic on the interior byte of one. That is
not hypothetical: trimming is Unicode's, so a non-breaking space is a character
a term legitimately carries.

**`IS DISTINCT FROM` and `IS NOT DISTINCT FROM` are candidates at the same
positions, not a pass of their own.** They are the two operators three-valued
evaluation makes necessary, so the term grammar has to spell them, and putting
them through the one positional scan is what keeps the earliest-operator rule
true of the whole operator set: `note=a is distinct from b` is the equality it
reads as, because `=` sits further left, and `a is distinct from b=c` is the
distinctness test, because the phrase does. A separate pass in either
direction would have reinterpreted one of those. Any run of whitespace
separates the words and the case is free, as in SQL.

**The phrase needs whitespace on both sides**, and that is what keeps the
addition from re-reading a term that parsed before it existed: a column named
`is distinct from` is still askable unquoted as `is distinct from=x`, since
what follows the phrase there is `=` and not a space. The one string whose
meaning does change is a term whose *column* is spelled with the phrase in it
between spaces: `a is distinct from b=c` named the column `a is distinct from b`
and now names `a`.

That is loud wherever `a` is not itself a column, which is the ordinary case —
the term fails to resolve. It is silent only against a schema carrying **both**
a column named `a is distinct from b` and one named `a`, and the grammar does
not distort itself for that: reserving the phrase outright would make a column
named with it unaskable, and requiring the left side to be quoted would put a
quoting rule on a term that has no ambiguity in it. Quoting is the remedy, and
it is the one the grammar already teaches — `"a is distinct from b"=c` names
the long column.

**Whitespace outside quotes is not data**, on both sides of the operator, using
Rust's `str::trim` — one definition of whitespace for the whole parser,
matching the `trim_end` the column side always did, and the reason a
non-breaking space pasted out of a web page is caught rather than searched for.
Untrimmed, the failure was loud on a typed column (`Error::PredicateValueDecode`,
naming the value) and *silent* on a text column under `=`, where a leading space
made an empty result that read as an answer. An all-whitespace value collapses
to the empty string with no special handling.

**A quoted part is taken exactly as written**, which is what restores every
value trimming would otherwise make unaskable: `--filter 'foo = " x"'` is a
leading space, so a space-padded `char(n)` value is expressible from the command
line and not only through the API.

**Both `'` and `"` open a quoted part, matching pairs only, with an interior
quote doubled** as SQL does it. Which one a user reaches for is decided by the
shell rather than by taste, so accepting one would punish whichever half of the
audience picked the other. Doubling keeps the grammar closed — no escape
alphabet, and so no second decision about what a backslash-n or a doubled
backslash mean — and two quote characters give a lazier escape for free, since
a part holding one quote character can be written in the other.

*Rejected: backslash escaping.* A backslash inside a shell double-quoted
argument is itself shell-processed, so the correct spelling is one nobody
writes right twice.

**Quotes work on the column side too, and that is what the quote-aware split is
for**: `"a=b"=x` names a column `a=b`. The quotes are stripped and nothing else
happens — column names are matched verbatim and there is no case-folding to
reproduce. The cost is that a bare quote character left of the operator now
opens a quoted region, so a column named `it's` must be written `"it's"`; that
is the price of quotes carrying boundary information, and it is loud rather than
silent.

**The `IS NULL` / `IS NOT NULL` forms are the fallback, not the first test**,
and the next reader must not restore the order that reads more naturally. An
operator outside quotes is looked for first; the `IS` suffix is stripped only
from a term that has none. Stripping it from the whole term first made
`--filter 'note=this is null'` an `IS NULL` on a column named `note=this`.
Under this order it is an equality against `this is null`, which is what it
says. Quote-awareness is needed on top rather than instead — that term goes
wrong with no quotes anywhere — and it is what lets `--filter '"is null" = x'`
name a column `is null`.

**A malformed quote is refused, never reinterpreted.** If what remains after
trimming opens with a quote, it must close with the matching one at the very
end with every interior occurrence doubled; an unterminated quote, trailing text
after the closing one, and a quote the split scan never saw closed are one fault
with one message. The alternative — falling back to the unquoted reading — hands
a user who mistyped one quote a value nobody meant and an empty result that
reads as an answer, which is the shape this whole grammar exists to remove.

**A part that opens and closes with a matching quote is quoted, always.** There
is no telling a SQL user quoting a string from someone searching a `json` column
for a quoted word, and a rule that guessed from the column's type would make a
term's meaning depend on a schema resolved much later. The escape is the
doubling rule.

**Quoting stops at `--filter`.** `--column` and `--table` take their names
verbatim, which is the same reasoning rather than an inconsistency with it: a
filter term is one string that must be split into three parts, so quotes carry
boundary information there, while the shell has already delimited a `--column`
argument. Quotes there would be decoration that made a column genuinely named
with quote marks unaskable. It is also what keeps the rejection of
`--columns a,b,c` above intact — that flag would have to *invent* a grammar,
where `--filter` already has one. The failure stays loud, and `quoted_name_note`
adds the missing sentence to it: a name that was not found and that opens and
closes with a matching quote says it was matched literally, quote marks
included.

### `--where` builds an expression out of those terms

`where_expr.rs` in `pgdump_query-cli` is the boolean grammar, and it is the
CLI's alone for the same reason the term grammar is: `Expr` is a plain public
enum an embedder fills in variant by variant, so nothing below L4 parses an
expression. Parens group; `NOT` binds tighter than `AND`, which binds tighter
than `OR`; the three keywords are case-insensitive and recognised only outside
quotes. Everything that is not a paren or a keyword is a **leaf**, handed to
`parse_filter` unchanged. Given both flags, the expression and every
`--filter` term are one conjunction.

**It is a second flag rather than a widening of `--filter`, and that is the
decision the rest of the grammar rests on.** Under a widened `--filter`,
`--filter 'note=a or b'` — an equality against the string `a or b` — would
silently become a disjunction with the command line unchanged, and a wrong row
set from a command that did not change is this grammar's worst failure. What
that string gets instead is a refusal, loud and with the remedy in it.

**A string both flags accept means the same thing under both, and
`refuse_where_structure` is what buys it.** Two flags do not avoid the widening
hazard by having it between them instead of inside one: `--where 'v_text=hello
and v_char=hi'` is a conjunction where `--filter` of the same string was an
equality against the literal `hello and v_char=hi`, and both exited 0 with
different row sets, so the user had no way to see which reading they got. So a
`--filter` term is now run through `tokenize` and refused unless it comes back
a single `Leaf`, naming what it found and both remedies — quote the part that
holds it, or write the expression with `--where`.

**The refusal set is defined by the tokenizer, not restated beside it.** That
makes it *exactly* the disagreeing set by construction, where a second scan for
the reserved spellings would be a copy of a rule, free to drift the moment
either grammar moves — and drift here is silent again. It also gets the
boundary conditions right for free: the keyword rule below leaves
`--filter 'v_text=not a'` one leaf under both flags, and `IS NULL` and
`IS NOT DISTINCT FROM` keep the `NOT` the term grammar claims. The narrow set
is what survives, so `--filter 'note=a b'` still needs no quotes.

**A term with no operator is refused on the same rule**, and it is the one
string the refusal reaches that was never returning wrong rows: `--filter 'and
is null'` named a column `and`, where `--where` of it is a hard parse error
rather than a different row set. Refusing it anyway is what keeps the refusal
set "whatever tokenizes to more than one token", which is the whole of its
value; a string one flag accepts and the other rejects cannot be moved between
them either. `--filter '"and" is null'` is the remedy, and it is the quoting the
term grammar already teaches.

**An empty term is left to the term grammar.** Nothing tokenizes to no tokens
but whitespace, which holds no reserved spelling to disagree about, so the
usage message is the accurate one — and neither flag accepts it, so the
single-meaning property is untouched.

**The check runs on the `--filter` path alone**, in `parse_filter_flag`, which
puts a `main.rs` → `where_expr` dependency where none was before; both are L4,
so [`layering.md`](layering.md) permits it. A `--where` leaf is what came *out*
of the tokenizer, and text that is one leaf inside its expression need not be
one on its own — `--where 'x=(and b)'` cuts a leaf `and b`, whose leading `and`
had a paren before it there and nothing standing alone.

*Rejected: a second scan beside the tokenizer looking for the reserved
spellings.* It over-refuses today and is free to drift tomorrow, both silently.

*Rejected: exempting a term with no operator, since it returned no wrong rows.*
The refusal set stops being the tokenizer's the moment it carries an exception,
and that is the only reason it cannot drift.

*Rejected: dropping `--filter` and leaving `--where` as the only flag.*
`--where` is a capability superset — every `Predicate` reachable through
`--filter` is reachable through it, quoting being always available and
round-tripping — but not a spelling superset, and the difference falls entirely
on the simple audience: `--filter 'note=a b'` needs no internal quoting and one
shell array element is one term. Refusing the overlap buys what dropping the
flag buys and keeps the cheap spelling.

**A keyword is recognised only against whitespace or a paren**, which is
stricter than a word boundary and has to be: `=` is not a word byte, so a bare
boundary rule would read `--where 'tag=and'` as the term `tag=` followed by
`AND`. With this rule that string is one leaf, which is the equality it reads
as. `v_and=1`, `nota=1` and `name=android` fall out of the same rule, and a
byte above ASCII counts as a word byte so that a keyword can never start
inside a multi-byte character.

**A `NOT` preceded by the word `is` belongs to the term.** `IS NOT NULL` and
`IS NOT DISTINCT FROM` both carry one, and reading either as the expression's
negation is the one way this grammar could silently mean something other than
it says — `v IS NOT NULL` would become a negation of a leaf called `NULL`,
which is a fault only because `NULL` happens not to parse as a term.

**Juxtaposition is not an implicit `AND`.** Only a paren or a keyword ends a
leaf, so `a=1 b=2` is one leaf and the term grammar's earliest-operator rule
makes it `a` equal to `1 b=2` — the same thing the same string means under
`--filter`. Inventing an implicit conjunction here would be a second grammar
laid over the first.

**A value holding a paren must be quoted**: a bare `(` groups, so a composite
literal is written `--where "v='(1,a)'"`. Unquoted it is a loud structural
fault, never a reinterpretation. `--filter` is unaffected, having no parens to
recognise.

*Rejected: recognising `&&`, `||` and `!` as alternate spellings.* One
spelling, and those symbols collide with values.

*Rejected: treating `(` as grouping only where an expression is expected, so
that a composite literal needs no quotes.* It makes tokenization
context-sensitive to buy back a case the quoting rule already covers, and the
quoting rule is one the term grammar already teaches.

*Rejected: a `--where` of its own that re-implements the term grammar with
SQL-shaped literals.* Two grammars for one thing, and the second would
immediately disagree with the first about quoting.

## Where a scan's time goes

The cost decomposition of the three shapes a user runs — `parse`, `query
--schema-mode strings` and `query --schema-mode typed` — read off sampling
profiles rather than off subtractions. It is written here because it is the
durable half of the scan-performance work: a lever may turn out to be worth
little, and knowing which function spends the time is worth having either way.

**How to read the percentages, and it is the one thing that invalidates every
number below if it is forgotten.** `perf_event_paranoid = 2` permits user-space
sampling only, so a profile's event is `cpu/cycles/Pu` and **its 100% is the
process's user time, not its wall time**. On this workload that distinction is
not a detail: a warm 3.00 GiB `parse` is 0.40 s of wall made of 0.30 s system
and 0.11 s user, and `dd` over the same file is 0.29 s of wall that is *all*
system. So a `parse` profile describes the quarter of the scan that is not the
kernel handing the bytes over, and a share of it must be multiplied by the user
time before it can be compared with anything in
[`measurements.md`](measurements.md).

The apparatus is `runs/profile-*.{data,txt}`, taken with the sequence `cd
scripts && uv run measure.py --profile-recipe` prints. Every figure quoted as
seconds is a `measurements.md` table; every percentage is a profile, which is a
proportion and never a median.

**On this workload the allocation is the cost and the bounds check is not**,
which is the one generalization the scan-performance work earned and the reason
none of these paths contains `unsafe`. It came up four times — slicing a row
out of a bulk-validated `&str` rather than `from_utf8_unchecked`, a hex table
of `&'static str` rather than a `Vec<u8>` finished with
`String::from_utf8_unchecked`, offsets pushed and indexed in `RowSplit` rather
than written raw, and escaping an array element in place through
`String::as_mut_vec` — and in three of the four the safe shape was also the
*faster* one, because it did strictly less work rather than the same work
unchecked. Only the fourth would have been faster, and it buys one allocation
per array value. Each refusal is filed beside its own mechanism; the pattern is
here because a session reaching for `unsafe` on a hot path in this codebase
should expect to be measuring the wrong thing.

**These headings state a proportion, and that is deliberate.** A heading here is
the section's one-line finding, met for free by a reader skimming the contents,
so it is written to say what the profile found and rewritten whenever the finding
moves: `parse`'s has read "half the wall is the kernel, and half of what is left
is copying", then "…and the rest is two SIMD passes", then its present form,
each rewrite following a slice that removed the term the previous one named.
*Rejected: naming the mechanism instead* — "`parse`: where the user time
goes" never goes stale because it never says anything, and buys the saving of an
edit with the finding itself.

**So a section whose heading states a finding a measurement can move carries an
`<!-- section: <id> -->` marker on the line above it, and that id — not the
heading — is what a citation names.** Four sections do: `parse-profile`,
`query-profile`, `insert-profile` and `attach-text-profile`. The id names the
mechanism and holds still; the heading says what the profile found and moves
with it. That is the idiom [`measurements.md`](measurements.md)'s figures and
the deficiency register
already use, for the reason `scripts/deficiencies.py` gives in those words: a
heading is rewritten whenever the thing under it moves. The heading is then free
to be vivid and free to change at the same time, and neither freedom costs a
retarget.

**The rule is the test, and the four are only what it selects today.** A slice
that writes a heading stating a proportion owes that heading a marker in the
same change; the set is not a closed list to be re-derived. Marking happens at
that moment rather than pre-emptively, which is why two of the four are marked
with no citation yet — the first citation of one then costs nothing.

The rest of this file is cited by heading, which is right where the heading
names a mechanism rather than a finding — a marker on every section would be
ceremony around strings that do not move. *Rejected: marking every cited
section.* The tree holds 238 citations of this file across 46 distinct target
strings, of which 38 resolve to a heading exactly and 5 to a truncated leading
clause of one, so the cost is 46 judgements and 238 mechanical substitutions
rather than the two hundred deliberations it looks like. It was refused on the
reading rather than the arithmetic: at 238 sites, a citation reading *the map is
built once, from the preamble* tells a reader what they will find and one
reading *file-map-build* does not, and a heading naming a mechanism
does not move, so a marker over it buys nothing back. The strictness given up is
bounded — 38 of the 46 already resolve exactly — and adding a marker later is
purely additive.

**A citation names the id in the position the heading occupied**, in one of two
shapes: `` (`docs/design/architecture.md`, "parse-profile") `` where the doc is
named, and a bare `"parse-profile"` where the surrounding sentence already named
it — which is the idiom the notes docs use when one doc is named and several of
its sections are then quoted in a list. The second is resolvable without a
second grammar because an id is matched by *set membership*, not by parsing: a
quoted string that exactly equals a declared marker id is a citation, and ids
are kebab-case, so no sentence of English collides with one.

**Citing a marked section by its heading is an error**, not a lenient match. The
marker is worth nothing otherwise: the heading is still there, a lenient matcher
resolves it happily, and the citation breaks silently at the next re-measure —
the failure the marker exists to prevent, arrived at by the route that leaves no
trace. What that leaves is a citation whose target has to be *found* rather than
resolved, and the check that resolves every form — a marker id strictly, a
heading leniently, and a marked section's heading as a failure naming the id to
use instead — is `scripts/citations.py`, over every citation in the tree. It is
what makes marking a section cost nothing later: mark one, and the citations
that then name its heading are named rather than left to rot. It reads comments
and prose and never string literals, because a citation is something written to
a reader; the grammars it has to survive, and the one thing it cannot see, are
in its own module docstring.

<!-- section: parse-profile -->

### `parse`: three-quarters of the wall is the kernel, and the rest is two SIMD passes

Warm, on tmpfs, over the 3.00 GiB brace-free control (16 scalar columns,
814,362 rows). Shares are of user time; the last column converts them at **this
profile's own sitting**, where a warm host `parse` is 0.44 s of wall made of
0.12 s user and 0.32 s system — a busier window than the 0.40 / 0.11 / 0.30 the
preamble above quotes, which is why an absolute here may not be set beside one
from another sitting.

| Share | ≈ | Symbol | What it is |
|---|---|---|---|
| 45.1% | 0.054 s | `memchr::One::find_raw_avx2` | the LF search inside `CopyScanner::next_event` — the `COPY` grammar itself |
| 33.8% | 0.041 s | `memchr::Two::find_raw_avx2` | `memchr2(b'{', b'[', raw)`, the array-shape census's pre-filter |
| 7.2% | 0.009 s | `stream::map_file::{closure#0}` | the event callback into `map::Builder` |
| 3.6% | 0.004 s | `memchr::memchr_raw::find_avx2` | the same LF search, at its out-of-line entry point |
| 2.6% | 0.003 s | `CopyScanner::next_event` | the state machine around both |
| 1.2% | 0.001 s | `memchr::memchr2_raw::find_avx2` | the same brace pre-filter, out of line |

Medians of **eight** consecutive profiles, and the row count is why there are
eight: the split between the two searchers does not reproduce — `One` spans
39.8–51.8% and `Two` 28.8–37.8% across the set, anti-correlated, which is
sample skid between two adjacent hot loops rather than a difference between
runs. What reproduces is their **sum**, 78.7% ± 3. Read the two rows as one
number split by an instrument that cannot quite split it.

**Nothing copies bytes any more, and that is the whole of what changed.**
`__memmove_avx_unaligned_erms` was 38.0% of user time — the chunk copy into
each read loop's own buffer, plus the `drain` that shifted the remainder down —
and it takes no samples above the 0.5% floor in any of the eight profiles,
because a read loop now carries one line rather than one chunk ("The scanner
never owns the bytes it scans": the carry). `__memset_avx2_…` went the same way
one slice earlier, at 23.2%, when the read path stopped allocating a fresh
`Vec<u8>` per chunk ("Execution model and API surface": the buffer pool). What
is left is the grammar and the census, and they are the two SIMD passes this
scan was always going to have to make.

**The removals are exact rather than readings.** User instructions per warm
`parse` fall **1,882,404,237 → 1,705,902,670** with the allocation gone
(−9.4%) and **1,703,555,926 → 1,403,769,503** with the copy gone (−17.6%),
every one of those numbers a median of five reps whose spread is under 0.003%.
The copy removal is the one that reaches wall time as well, on interleaved reps
in one window: 0.48 s → 0.40 s, user 0.17 s → 0.11 s, system unchanged at
0.30 s, and peak RSS 7.9 MB → 6.0 MB, the megabyte buffer per read loop being
what leaves. It also holds on a brace-bearing file, where it is the same
**absolute** 0.2996 G instructions over the same 3.00 GiB (18.080 G →
17.781 G) — the copy is per byte of file, so the two shapes subtract the same
amount and only the share differs.

**The grammar now costs more than the census, and the census reconciles with
the subtraction that measures it.** Grouping the out-of-line entry points with
their inlined ones puts the LF search at 48.8% and the brace pre-filter at
35.0%, or 0.059 s against 0.042 s — and that second figure is the check, since
the census-on/census-off pair reads **+0.057 s** where it stands today, against
+0.030 s under the stamp before the read path stopped allocating and copying
([`measurements.md`](measurements.md), "The census on brace-free rows"). A
share that agrees with a subtraction taken by a different instrument is the
evidence that the user-time correction above is being applied correctly.

*Deferred: deleting the LF search outright, by seeking straight to the block
terminator.* Inside a `COPY` block the only thing that ends the block is a line
holding exactly `\.`, and that line is unambiguous — COPY TEXT doubles a
backslash in a value, so `LF 5C 2E` never begins a data line (I7). One
`memchr::memmem` for that three-byte needle would therefore find the end
without looking at a single row, which is the natural shape for a pass whose
whole job is to *skip*. **What stops it is that two things do look at every
row**: `CopyEnd::row_count`, which the cache stores and `info` reports, and the
array-shape census, which every mapping pass records and a query reads back to
retype its array columns before the first batch. Counting without enumerating
is available — `memchr::count` over the block extent is the same kind of SIMD
pass — but the census is not, because it needs the bytes. So adopting the
needle search means deciding what happens to the census, and the whole prize is
the LF search's share of a path the table above puts at a quarter-second per
3 GiB warm and inside the noise cold. It is written down because the invariant
that makes it *safe* is the expensive half and is already established; what is
missing is a reason.

**On a brace-bearing file the picture inverts, and the field split behind it
was the whole of the inversion.** Over the `--arrays --composite` file
`map::Builder::on_row` was **85.6%** of the `parse` profile and the chunk copy
5.7% before it went. What that share was made of turned out to be the *splitter*
rather than the census: the census's `copy::split_fields` walked the row a byte
at a time through a closure, and putting it on `memchr` — which is all
"A row's bytes are validated once, in bulk" did to it — takes a whole-file
`parse` of that file from **17.781 G user instructions to 2.919 G**, a factor
of **6.1**, with the cache it writes byte-identical. `on_row`'s *self* share falls from 87.9–90.2% to 5.96–6.25%
across three profiles each, its work moving into `memchr`, and what is left of
that scan is three SIMD searches: the LF search (52.2%), the field split
(20.1%) and the brace pre-filter (14.7%).

**Both original readings were true and they are about different inputs**: the
pre-filter is what a brace-free scan pays, and the split behind it is what a
brace-bearing one paid. The correction is that the split's cost was never the
census deciding anything — it was the byte-at-a-time walk underneath, which is
why a change aimed at the *query* path moved a `parse` figure by six-fold
([`../status/history/2026-09-04.md`](../status/history/2026-09-04.md), "The
census's field split was the byte loop, not the census").

<!-- section: query-profile -->

### `query`: the library's batch stream is the larger bucket in both modes

Same file, same regime, `--dqcache none` (which is what every published `query`
figure times, so the mapping pass is inside these numbers). Structure is read
off the call graph; the flat shares agree with a `release` build's. The whole
table is **one sitting of the shipped binary**: wall/user/system are medians of
five runs with no `perf` on them, and the shares come from two `--call-graph
fp` profiles taken in the same window (`runs/m53-query-profile.sh`).

| | `strings` | `typed` |
|---|---|---|
| wall / user / system | 3.53 / 2.63 / 0.84 s | 4.90 / 4.01 / 0.85 s |
| `poll_next` — the library's whole batch stream | **73.5%** | **61.5%** |
|  ↳ `RowBatcher::push_row` | 55.6% | 50.2% |
|  ↳ `copy::RawRow::decode` | 32.4% | 20.7% |
|  ↳ `batch::append_typed` | — | 16.3% |
| `pgdq::print_batch` — the CLI turning the batch back into TSV | **25.2%** | **37.4%** |
|  ↳ `batch::render_field_into` | 8.0% | 21.7% |

**The mode difference is still mostly the CLI, and it is a much smaller
difference.** A typed query costs 1.38 s of user time more than a `strings`
one, of which 0.84 s is `print_batch` and 0.53 s the library — the CLI is 61%
of that gap, where it was 79% of a gap four and a half times the size.

**The read path has no row here, because it takes no user samples.** Everything
the `read_range` thread did in user space was the per-chunk memset, and the
`pread` itself is system time a `Pu` profile cannot see; with the buffer pool
and the carry in place neither profile has a frame under `read_exact_at` above
the 0.5% floor, where the thread was once 7.2% and 2.6% of the two runs
("Execution model and API surface"; "The scanner never owns the bytes it
scans"). The carry took the replay loop's second copy of each chunk with it,
which is **−2.1%** of a `strings` query's user instructions and **−1.4%** of a
typed one's.

**The decode row names one function, because the validation left it.** The
per-field `std::str::from_utf8` became a single bulk pass over the row ("A
row's bytes are validated once, in bulk"), which is why the row reads
`copy::RawRow::decode` rather than the `copy::decode_field` that used to be a
symbol on this path: whole-query user instructions fell **6.11%** for `strings`
and **2.42%** for typed, and `core::str::converts::from_utf8` went from 7.81%
and 2.60% of the two profiles to nothing at all.

**The builder's row is a share of a run that has since halved.** The scalar
decoders took `batch::append_typed` from 13.8% to **7.48%** of the profile they
were measured in — `decode_bytea` alone from 3.75% to 0.67% and `decode_uuid`
below the floor — with a `strings` query unmoved at 24.772 G user instructions
on both sides, the control that says the change was confined to the typed path
("Decoders and render-back"). It reads 16.3% here only because the render-path
work below took the run it is a share of down by more than it took the builder.

**The CLI's render-back was two-thirds of a typed query and is 37.4%.** Two
changes did it on this file, each with a control that says where it landed
("Decoders and render-back"). The hex pair table took a typed control query
from **89.725 G to 49.074 G** user instructions (−45.3%) and its wall from 9.40
to 6.03 s, with a `strings` query unmoved at 24.7935 G — the control that
confined it to the typed render path. The date and time renderers and the row
sink took it **49.074 G → 34.610 G** (−29.5%), wall 6.07 → 4.75 s, and unlike
the hex table reached `strings` as well, **24.793 G → 19.610 G** (−20.9%),
where the control is a `parse` instead: flat to 0.0004%, so no scan figure
moved. Before them `print_batch` was 62.8% of a typed profile and `core::fmt`
was the shape of it, `format_inner`, `fmt::write`, `Formatter::pad_integral`,
`<u8 as LowerHex>::fmt` and `String::write_str` together about a fifth of the
run with the allocator traffic they generate on top.

**What is left inside `render_field_into` is the floats.** `render_f64` is
**5.0%** of the typed run and is still a `format!`, against `render_decimal`'s
2.1%, `render_bytea`'s 1.2% and 2.0% for every date/time renderer together.

**A lever's reach is a property of the columns a file has, which is why two
render-path changes are absent from this table entirely.** `nested::needs_quote`
and the array arm's sink both live in `nested.rs`, and the control has no array,
composite or range column, so neither is ever reached over it: the force-quote
set reads **90.719 G** user instructions on both sides and the array sink
**+0.14%** with the two legs' ranges overlapping. Where they land is the shape
this file does not have — over the `--arrays --composite` file the same typed
query falls **−12.47%** (162.807 G → 142.500 G) and then **−19.4%** (104.094 G
→ 83.925 G, wall 12.43 → 10.19 s), and that file's 50-element array column
projected alone falls **−26.8%**. A profile of it after both reads `poll_next`
**63.05%** against `print_batch`'s **36.42%**, the same side of the line the
control is on. So this table is a **control-file decomposition**: it sizes what
a *scalar* row costs, and a nested column's per-element cost is
[`measurements.md`](measurements.md), "Nested decode costs what it copies".

**What that means for reading the published `query` figures**, which are the
two `query` rows of [`measurements.md`](measurements.md), "Which allocator a
figure was taken under" — the only table that times both modes against a
co-measured warm floor. They are `pgdq query` figures: an embedder that
consumes `RecordBatch`es pays `poll_next` and nothing under `print_batch`. A
typed query is **15.0×** that floor and a `strings` one **10.9×** (4.75 s and
3.44 s against 0.317 s), where before this section's render-path work they
were 31× and 13.3×. The library's own typed extraction is 3.10 µs a row
against `strings`'s 2.42 — a factor of 1.3, where the CLI's wall times showed
2.4 and now show **1.38**. The gap between the library's factor and the CLI's
is what `print_batch` costs, and it is now close to closed.

#### The library's own per-row budget

A profile's shares converted at each mode's user time over the control's
814,362 rows — **its own shares, not the table above's**, which is a different
sitting. **This is the decomposition an embedder pays and the only one a
library change can move**, so it is what a proposed optimization is sized
against — and no figure in `measurements.md` states it, because every figure
there times the CLI ("A mode difference and a per-column delta are CLI
numbers"). It is a set of proportions, so it carries no marker and is not a
figure itself; a lever that lands re-reads it the same way it re-takes a table.

Its own sitting, and an earlier one than the table above: the shares are from a
pair of profiles of the shipped binary and the user times (3.22 s and 9.10 s,
medians of five) from the same binary and input without `perf` on it. **The
totals survive the sitting above it**, which converts to 2.37 µs and 3.03 µs on
its own shares and user times — within 3% of this table, on a run whose typed
user time is less than half of this one's, which is the check that says the
render-path work took nothing out of the library.

| Per row, library only | `strings` | `typed` |
|---|---|---|
| `batch::append_typed` — the builder, decoders included | — | **0.84 µs** |
| `copy::RawRow::decode` — unescape, and the borrow | **1.06 µs** | **1.03 µs** |
| the row split and walk inside `push_row` | **0.75 µs** | **0.66 µs** |
| `copy::validated_prefix` — the bulk UTF-8 pass | 0.06 µs | 0.06 µs |
| the stream and scan machinery around it | 0.55 µs | 0.51 µs |
| **total (`poll_next`)** | **2.42 µs** | **3.10 µs** |

Two things fall out of it that the percentages hide. **The decode and the field
split are each about a microsecond a row and neither depends on the mode** —
they are what `strings` spends nearly all of its time on, and they are
unchanged when typing is switched on, so they are the only part of the library
that a `strings` consumer can be made faster by. And **the builder is no longer
the largest library bucket**: the unescape is, in both modes. `append_typed`
read 1.64 µs against the unescape's 1.04 until the scalar decoders it calls
stopped allocating per field ("Decoders and render-back"), which took a typed
`pgdq query` over the control from **95.046 G to 90.719 G user instructions**,
−4.55%, every after rep below every before rep, with a `strings` query
unmoved at 24.772 G — the control that says the change is confined to the typed
path. Typing a row now costs less in Arrow assembly
than in parsing, which reverses what this paragraph said for as long as the
builder's bucket carried three `malloc`/`free` pairs per row that were not the
builder's.

**What is left in that row is the decoders, and the Arrow appends are 1.6% of
a typed library row.** Splitting `append_typed`'s bucket by what sits under it
— a call graph resolved through the inline frames, over the same control, the
same regime and the same `--dqcache none` shape — gives **0.68 µs of
decoders, 0.06 µs of its own dispatch, and 0.052 µs of Arrow builder
appends**, against a `poll_next` the same sitting reads at 3.25 µs. So the
builder-append half of the lever this budget sizes is **52 ns a row**: below
what any of this campaign's instruments resolves, and an order of magnitude
under the arithmetic that made `append_typed` look like the larger half before
the decoders stopped allocating. The dispatch is a jump table over
`batch::ColumnBuilder`'s twenty variants and the appends are `arrow-rs`'s own;
there is nothing between them to take.

*Rejected: pre-sizing the builders.* `new_column_builder` builds every one with
`::new()` rather than `with_capacity(max_rows)`, so each grows by doubling —
about thirteen reallocations per column per 8192-row batch, which is the shape
that looks like free money. The profile prices the whole of it: `reserve` and
`capacity` under `append_typed` are 0.018% and 0.034% of the control run,
together **under 6 ns a row**, below every instrument this project owns. And it
would make a one-row batch allocate 8192 slots in every column, so it is a
memory regression bought with nothing measurable.

**A nested column moves that row and does not change the answer.** The same
split over the `--arrays --composite` file reads **0.19 µs a row** of Arrow
appends for two `integer[]` columns of 54 elements between them, a composite
and the sixteen scalars — about 2.5 ns an element — and over a file whose two
list columns are `text[]` instead, **0.55 µs**. Isolated on that file with
`--column`, one 50-element `text[]` builds its `List<Utf8View>` in **0.383 µs
a row**, 7.7 ns an element, inside a row that costs 11.08 µs end to end and
5.46 µs in the library. A `Utf8View` element costs about three times an `Int32`
one in the builder and the builder is still not where a nested row's time goes:
`nested::decode_array` is 3.88 µs of that same isolated row, ten times the
build it feeds.

*Rejected: a viewing builder for a nested `Utf8View`, replacing that copy with
a view over the read chunk the way `push_utf8view_field` already does at the
top level.* **The prize is the build minus the view write that replaces it, not
the build**, and `measurements.md`'s "Nested decode costs what it copies" floors
one `append_view_unchecked` at 3.03 ns — a floor that bounds the real cost from
above, since the borrowed arm also walks the chunk deque and calls `block_for`.
So the swap is worth **at most 4.7 ns an element**, 0.24 µs of that 11.08 µs
row, before any of the recursive chunk-retention and block-invalidation the
change needs. **The copy is one arm** — `batch::append_nested`'s
`ColumnBuilder::Utf8View` — and it serves a `Struct`'s fields as well as a
`List`'s elements, so the bound reaches both: the registered `--arrays
--composite` file, whose composite is a `Struct{Int32, Utf8View}` with a
quoted-but-unescaped text field, spends 0.19 µs a row on *all* its Arrow
appends across nineteen columns. The `text[]` file was only ever needed for the
`List` leg.

**What would reopen it is element *width*, and then element count.** At or below
`arrow`'s 12-byte view-inlining threshold a view and a copy are the same
instruction sequence and the prize is exactly zero; above it the copy grows with
the element and the view write does not, so the ratio of prize to build rises
with width while count scales both together. On the 21-byte shape below the
prize reaches 1 µs a row at roughly **210 elements a row**. That is the trigger
to re-read this on, and it is not the whole argument: the recursive
chunk-retention and block-invalidation the change needs at every level of `List`
and `Struct` nesting is a fixed cost in correctness surface that does not shrink
as the prize grows.

**The `text[]` file is not a registered input, and this is its specification**,
because rebuilding it is what re-reading the bound above costs. It is
`scripts/generate_perf_data.py`'s `--arrays` shape with the two array columns
changed from `integer[]` to `text[]` — the same sixteen scalars from `COLUMNS`,
the same 3–5 and 50 element counts, `--seed 42` — every element the literal
token `lorem_ipsum_dolor_sit`. Three of its properties are load-bearing and all
three are chosen to be **favourable to the change**, so that a refusal read off
it is safe in the direction it is read: **21 bytes an element**, above the
inlining threshold, since at or below it the prize is zero by construction and
the file would have refused the lever rather than the evidence doing it; **no
whitespace in an element**, so `array_out` quotes none of them and the literal's
grammar matches the `integer[]` one, leaving the child builder as the only
difference between the two files; and **fifty elements in every row**, never
NULL and never ragged, the widest the registered apparatus uses.

*Rejected: committing it as a `generate_perf_data.py` flag, with or without a
registered figure to consume it.* The paragraph above is the record and
rebuilding the generator from it is minutes. A flag would buy re-running a
reading that is already discharged — it refused a lever and published no number
— and [`measurements.md`](measurements.md)'s rule that a figure whose
regeneration command is gone should be deleted has its mirror here: a
non-figure owes no command. A registered figure would be worse again, making
every future sweep pay an hour a time for a question that is closed.

**The split row is now shared, and this budget is still the unfiltered one.**
A row's boundaries are found once and read by the filter's terms as well as by
`push_row` ("Predicates"), so on a query that passes a filter this row is
smaller than the terms and the batcher used to cost between them. The budget
above is a profile of a query with **no** filter, where there is nothing to
share with: on that shape the row grew by the memoization, about 9 instructions
a field, which is 0.3% of the `strings` query it was measured on and below what
a re-profile would resolve.

**The validation row is new and the split row is what shrank.** The per-field
`std::str::from_utf8` sat inside `push_row`'s split-and-walk bucket, never in
the decode's, which is why removing it moved a row that does not name it; what
replaced it is a tenth of a microsecond outside `push_row` entirely. Read the
two sittings against each other only that far: a warm absolute resolves to
about ±8% across sessions ([`measurements.md`](measurements.md), "A move
smaller than the apparatus resolves is not a finding"), and the numbers that
carry the change are the within-sitting ones under "A row's bytes are validated
once, in bulk".

**The decode is the largest single library bucket in `strings` mode** and
stays largest when arrays are added (14.6% on the `--arrays --composite` file),
so it is the shared row machinery rather than the nested path. What is left in
it is the *unescape*: `copy::unescape_field` alone is 22.45% of a `strings`
profile against `RawRow::decode`'s own 2.05%, so a further cut there is an
escaping question, not a validation one. The zero-copy
premise holds up in the same profile: `GenericByteViewArray::value` and the
`StringViewBuilder` appends together are under 8%, and no `Utf8View` column
copies field bytes.

**Nested columns move the weight into the codec, not out of the CLI.** On the
`--arrays --composite` file the typed profile reads `print_batch` **59.1%** and
`poll_next` **40.4%**, with `nested::decode_array` **15.5%** under
`append_typed`'s 28.8% and `nested::needs_quote` **14.1%** — that last one the
largest single symbol in the run *as that sitting was taken*, and split roughly
evenly between the two directions: `push_token` re-quoting each element on the
way out, and `scan_token` rejecting an element `array_out` would have quoted on
the way in. It is no longer a symbol at all; the force-quote set inlined it
away, and the two sinks after it moved the `print_batch` share to the other
side of the line (the paragraphs above, and the array-file profile they end on).

**The codec no longer allocates per element**, which is what moved those
shares: an array element is a borrowed slice of the field unless it carried an
escape ("The nested literal codec"). Measured on the same file and shape, a
whole `pgdq query --schema-mode typed` fell **176.38 G → 165.37 G user
instructions**, −6.2%, and 19.02 → 17.40 s of user time, every one of three
after reps below every before rep; the glibc allocation symbols all fall with
it (`_int_malloc` 5.0% → 4.1%, `malloc` 3.7% → 2.4%, `cfree` 2.6% → 1.9%).
Because more than half this shape's work is `print_batch`, which no library
change touches, the library's own half moved by roughly twice that. The
proportions above are their own sitting, taken with the change in hand rather
than folded from the sweep's.

<!-- section: insert-profile -->

### The `INSERT` path is one `memchr`-bound scan in L1

Over the 3.00 GiB `INSERT`-run file, warm, a `parse` — the profile that chose
the fast path's layer, and the profile of the path that replaced it:

Flat shares under the `ba2fc12` stamp, before the statement scan replaced
the accumulator:

| Share | Symbol | Reached from |
|---|---|---|
| 58.2% | `preamble::scan_buf` | `statement_complete` ← `Builder::step` ← `Builder::feed_line` |
| 17.3% | `Utf8Chunks::next` | `String::from_utf8_lossy(raw).into_owned()`, the first line of `Builder::feed_line` |
| 3.6% | `Cursor::parse_ident` | `parse_insert_target` |
| 3.1% | `__memmove_avx_unaligned_erms` | `push_stmt_line`'s copy into the accumulator |
| 0.6% | `CopyScanner::next_event` | the scanner |

**Three-quarters of an `INSERT` scan used to be two L1 functions with the
scanner under 1% of it** — which is what settled where the fast path went; see
"Bulk regions" above. `scan_buf` walked the *accumulated statement* with
`chars().peekable()` on every line, so a one-line `INSERT` statement was one
char-by-char pass over its own bytes, and `feed_line`'s
`from_utf8_lossy(...).into_owned()` was a validation plus an allocation per
line before the mode machine saw it.

After it, the same scan — a third as long in wall time — is a call graph
rather than a list of symbols, because `StatementScan::feed` inlines into its
caller and spends itself in `memchr`:

| Share | Path |
|---|---|
| 74.3% | `Builder::feed_line` → `insert_run_line` → `StatementScan::feed_line` |
| 14.1% | `CopyScanner::next_event` (5.2% of it `scan_dollar_quotes`) |

A third row that sitting read — `__memmove_avx_unaligned_erms` at 2.5%, the
read path's chunk copy into the scanner's own buffer — is gone with the carry
("The scanner never owns the bytes it scans"). The two shares above are left as
taken, of a denominator that still included it, so each is understated by about
2.6% of itself.

**Nearly 80% of the flat profile is now `memchr`**, split across the four
needle widths `feed` uses: `memchr` for a string's closing quote and for a
comment's newline, `memchr3` for the next mode-changing byte outside them, and
`memchr2` for the parens in between. `feed`'s own non-SIMD code is **6.9%**.
There is no allocation, no UTF-8 validation and no second walk left in the
path; what remains is the bytes themselves, which an `INSERT` statement's end
cannot be found without crossing.

### What a query reads twice, and when

A `--dqcache none` query reads the file **exactly twice**: 6,443,503,731 bytes
of `pread64` against a 3,221,227,790-byte file, counted with `strace`. Pass 1
is the mapping pass and pass 2 is the extraction replay
("Query: mapping and streaming are separate passes"). With a cache the same
query reads **1.0000×** and a `parse` reads 1.0003×, so the second pass is the
cost of not having run `parse` — a property of the two-pass design, with the
remedy already in the user's hands, rather than a defect.

<!-- section: attach-text-profile -->

### `attach_text` does not appear in a profile

The concern was that filling `Span::text` re-slices the schema regions once per
block, so a block-rich dump would pay for it repeatedly. It does not: it runs
at most twice per map (once after the preamble prepass, once at the end), and
in a 100,000-sample profile of a 4000-block `parse` — the input built to make
per-block costs visible — it takes **zero samples**. That same profile is
43% `stream::splice` and 57% allocator traffic underneath it, which is `KD5`
and nothing else; it was taken on the build that spliced at every `CopyEnd`, so
those two numbers describe the cost the gate has since removed rather than what
that command costs today.

### The allocator is the binary's choice

`pgdq` links the **platform allocator** — glibc's `malloc` on the measured
apparatus — and `pgdump_query-cli/src/alloc.rs` is the whole of the mechanism:
two off-by-default Cargo features (`jemalloc`, `mimalloc`), one
`#[global_allocator]` behind each, a `compile_error!` if both are asked for,
and a `VERSION` string that `pgdq --version` prints.

**`--all-features` does not build this workspace, and that is the mechanism
working.** Asking for both allocators asks for two `#[global_allocator]`s, and
`alloc.rs` refuses it with a `compile_error!` naming the reason rather than
letting `rustc` report a symbol collision. The repair that suggests itself — a
precedence rule, so the build succeeds and one feature quietly wins — must not
be made: it would let the harness label a measured leg by the feature it passed
rather than by the allocator it got, which is the single guarantee the
`--version` interrogation below exists to provide. So the two features are one
choice rather than a matrix, `--all-features` is knowingly unsupported, and a
session that meets the error is reading a decision rather than a defect.

**The choice is the binary's and never the library's.** A `#[global_allocator]`
in `pgdump_query` would impose one on every embedder, which is exactly the
audience the embedding work is for. The consequence is stated rather than
hidden: every figure in [`measurements.md`](measurements.md) is a **CLI**
figure taken under whatever `pgdq` links against, and an embedder inherits
whatever their own binary chose.

**`--version` names the allocator so the harness can ask rather than assume.**
`scripts/measure.py` reads it out of each binary before timing it — the
reference leg included — and puts it in the session stamp. Without that, the
day this default changes, `target/release/pgdq` becomes a different binary and
every apparatus line still naming the old allocator is wrong with nothing to
notice; with it, a leg whose build silently dropped its feature cannot be
published as a comparison of two identical binaries.

The reading is [`measurements.md`](measurements.md), "Which allocator a figure
was taken under", and the decision it exists to make is **settled**: over the
three headline shapes `jemalloc` is 1.02× / 1.12× / 1.10× and `mimalloc`
0.97× / 0.97× / 0.99×. The lever's stake — a factor, on the evidence that two
stock libcs differ by 1.8–2.4× — did not survive contact with two allocators
that are both tuned for this shape of work: what is on the table is single
percentage points, in a table whose own instrument does not resolve them. The
features stay in `pgdump_query-cli`'s manifest so a later re-take is five
minutes.

*Rejected:* a third leg. `tcmalloc` and its neighbours are a fishing expedition
on the strength of the two named replacements both losing; `ALLOCATOR_LEGS`
takes one by a single entry if anyone wants it, so nothing has to be designed
for that day.

**The claim is "nothing beats it by more than the instrument's own noise", and
it used to be "nothing beats it".** Three sittings have given three answers for
`mimalloc` — 0.98× / 0.96× / 0.96×, then 1.00× / 0.99× / 1.01×, then the row
above — so two of the three now put it marginally ahead on every shape. Every
cell's spread overlaps the reference's and the largest gap is 3%, against a
1.6% median drift between two sittings of identical binaries; a within-sitting
ratio does not inherit that excuse automatically, which is why the weaker
sentence is written rather than the reading dismissed.

**Both readings that once argued for adopting were measuring the read path, not
an allocator.** The figure has been taken three times — before the per-chunk
read buffer was pooled, after, and at the wrap sweep — and the two cells that
had made adoption a live question are exactly the two that did not survive the
pooling.

- **`jemalloc`'s `parse` was 1.87×.** All of it was system time (0.27 s →
  0.80 s, with user time slightly *lower*), and `strace -c` counted 3,161
  `madvise` calls against glibc's 50 over a file read in 3,072 chunks: it was
  `LocalFileSource::read_range`'s per-chunk `vec![0u8; 1 MiB]` handed back to
  the kernel and re-faulted once per chunk. With the buffer pooled ("Execution
  model and API surface") it is 1.02× and has stayed there, while that leg's
  two `query` cells — 1.12× and 1.10× — are the clearest losses in the table.
- **`mimalloc`'s `typed` was 0.96×**, twice, on non-overlapping within-sitting
  spreads — the one cell of that table that reproduced its magnitude and the
  whole of the case for adopting. Pooled, it read 1.01× with its spread *above*
  the reference's, and at the wrap sweep 0.99× with the spreads overlapping. A
  cell that has read below, above and below again across three sittings is
  measuring the apparatus.

*Rejected:* adopting `mimalloc`. It has been refused twice on two different
numbers, and the reason is the same both times. The one-line default flip was
never the cost — the cost is that every other table in
[`measurements.md`](measurements.md) becomes a figure of a binary no longer
shipped, with no mechanical oracle to acknowledge it, so the whole document
reads stale until the next full sweep. **On the 3–4% `typed` win it showed
before the buffer pool**, paying that would have been buying the decision at
its least informative moment, on a ranking whose largest number was measuring
an allocation about to be deleted; the next sitting agreed, reading that cell
at 1.01×. **On the 1–3% margin it shows now**, the price is the same and the
evidence is weaker still: no cell's spread clears the reference's, and a lever
whose sign has changed twice across three sittings is one more sitting away
from changing again. What would settle it is not another sitting of this
figure but an instrument that resolves a 1% wall difference, which this
campaign does not own and which the deterministic one — user instructions —
cannot supply, since an allocator moves where time goes and not how many
instructions retire. Reopen it on an instrument, not on a sign.

*Rejected:* a `--global-allocator` flag or an environment variable. A global
allocator is chosen when the binary is linked, so a runtime switch would have
to link all three and dispatch through a vtable on every allocation, which
prices the mechanism above what it is measuring.

## The cache

A **best-effort accelerator, never required for correctness**: a stale
structural index costs a rescan and nothing else. What is *not* best-effort is
what happens to the file at the cache path — a cache that describes another
source is refused rather than scanned over, which is this section's "the library
never replaces cache data automatically" below.

**The format-version integer is not tracked in any document.** Pre-1.0 a bump
is free and nothing migrates, so the number carries no information a reader can
act on; `cache.rs`'s `FORMAT_VERSION` is the only place it exists, and `git log
-p` on that constant is its history. The envelope exists so a stale cache is
*detected* rather than misread — the rule is to bump whenever a persisted field
is added, removed or reshaped, and never to record which bump that was.

**Reads and writes have deliberately opposite failure modes.** An unrecognised
`format_version`/`container_kind`, or bytes that do not parse as a cache at
all, are as unusable as a missing file: reading one costs a scan and nothing
else. `cache::save` propagates I/O failures as `Error::Io`.

**Unusable is four named outcomes, not one, and nothing collapses them.**
`CacheStatus` distinguishes `Missing`, `Unreadable` (bytes that do not decode),
`UnsupportedVersion` (another build's envelope), and `SourceChanged {
cached_stored_size, live_stored_size }`; `CacheMode::load` carries each one
across to a caller holding a live source as its own `CacheLoad` variant. `pgdq
info` cannot scan and has a different sentence for each: a wrong path, a stale
build, and "your file changed since you parsed it" send a reader to three
different places. A caller that *can*
scan does not treat them alike either, which is what having the reason before
it does any work buys — see the refusal below, and "The CLI's two refusals are
worded as one" for how that split reaches the sentences.

**The library never replaces cache data automatically.** A cache whose recorded
stored size is not this source's is a cache that describes some *other* file,
and a scan started past it overwrites that file's index within the first
throttled save ("`parse` resumes, and saves as it goes"). So the three scan
entry points — `map_file`, `table_stream`, `preamble_only` — refuse it with
`Error::CacheSourceMismatch`, naming the path, what the cache was written for
and what this source is, before a byte of the dump is read. The other three
unusable statuses still start cold: there is nothing at that path worth keeping.
`Disabled` is not a refusal either — the caller consulted no path, so there was
never a cache to protect. Reading a mismatched cache stays unusable; being
unusable to read stops licensing a write.

**The CLI reaches that verdict a step earlier, and the library keeps it
anyway.** `cache::claim` compares the recorded stored size against a `stat`
before a source is built, so `pgdq` refuses without opening the file at all —
which is what stops an `.xz` source walking its stream footers to reach an
answer already on disk ("The compressed source", "The CLI's two refusals are
worded as one"). The three entry points still refuse on their own: the
guarantee is the library's, and an embedder that never goes through this binary
holds it unchanged. That is one rule read at two moments rather than two rules,
because the comparison lives in `cache.rs` both times.

This reverses the settled "an unusable outcome, not a new hard-error path"
below, which was right about reading and never separated writing out. The
mistake it guards is an ordinary operational slip — a path flag aimed at the
wrong path — whose cost is unbounded: on a koji-scale dump the destroyed index
is an hour of scanning.

*Rejected:* an override — a `--force` flag, or a `CacheMode` variant meaning
"replace regardless". It exists to be set once in a script and never
reconsidered, and from that moment the guarantee is gone for exactly the runs
where the path was wrong. Removing the file is already in every caller's
vocabulary and is visible in the code that does it. *Rejected:* refusing only
where `--dqcache <path>` was given explicitly, leaving the colocated default
silently replacing — the guarantee is worth more without an asterisk on it, and
there is no workflow in which discarding an index is the intended outcome.
*Rejected:* prompting; `parse` runs detached, under `setsid` and in containers,
where blocking on stdin hangs instead of failing. *Rejected:* the guard inside
`cache::save`, which would cover a future embedder too but turns a write into a
policy decision an embedder cannot override, and pays an envelope decode on
every throttled save — for a many-streams `.xz`, re-decoding a 31,150-entry seek
table repeatedly. The mistake being guarded is made once, at the start.

**Incompleteness is not mismatch**, and neither is an mtime. A partial cache
that still matches its source resumes exactly as it did — `Incomplete` is a
statement about coverage, not identity. A changed mtime over an unchanged stored
size remains a usable cache carrying `CacheMtimeChanged`: mtime moves without
content moving routinely (`rsync`, a restore from backup, a `touch`, a
filesystem copy), and a refusal that fires on healthy input is one people learn
to work around.

`CacheLoad` is that answer: `Index` for a usable cache — `Valid` and
`Incomplete` alike, which is the one distinction `load` exists to erase — plus
the four unusable statuses and `Disabled`. `Disabled` is a statement about the
*caller* rather than about anything found at a path, so it has no `CacheStatus`
to correspond to and is the reason `CacheLoad` is its own type rather than
`CacheStatus` handed back verbatim; handing back `CacheStatus` would also split
the two usable outcomes every one of these callers treats as one. The three
scan entry points — `map_file`, `table_stream`, `preamble_only` — spell all five
out rather than wildcarding them, so a reason added later has to be answered at
each rather than falling through. Enumerating them is also what made the refusal
itself a compiler-checked edit: one arm at each of three exhaustive matches, with
"is there a scan entry point we missed" answered by the compiler rather than by a
search. *Rejected:* keeping `Option<DumpIndex>` and
adding a second, reporting method beside it — the collapsing one stays the
shorter call, which is how the reason came to be out of reach of the scan entry
points in the first place. *Rejected:* an accessor collapsing the reasons back
to an `Option` for callers that do not care; it rebuilds the collapse under a
shorter name at exactly the sites that must not have it, and the callers that
genuinely do not care are tests.

**The path in the refusal comes from the mode, not from the load.**
`CacheMode::source_mismatch` is the one-line constructor each of the three sites
calls: it matches `Enabled(path)` and answers `Error::CacheModeMismatch` for the
other two modes, which is the caller-contract answer `load` already gives
`Offline`. `CacheLoad` carries no path deliberately — `Disabled` has none — and
every caller that can reach the refusal holds the `&CacheMode` anyway, so
nothing is re-`stat`ed or re-resolved to say which file is being protected, and
a CLI printing the error re-derives nothing either.

**"Leaves it alone" is asserted against the bytes.** The test that puts a grown
dump to all three entry points reads the cache file back and compares it byte
for byte, because a refusal that raised the error *and* wrote anyway is
indistinguishable from one that did not — the variant, the path and the two
sizes are identical either way, so the only assertion that can fail is the one
over the file.

**A cache *write* failure is a hard error.** If the resolved path cannot be
written (read-only mount, permissions, disk full), the library returns an error
rather than silently proceeding without a cache. This is a different axis from
best-effort: *reading* a missing, foreign or stale cache falls back to scanning
without complaint, because that is the routine cold-cache case, whereas a write
failure means the caller asked for something and did not get it.

**Cache location is exactly two options, with no fallback**: colocated with the
dump file as `<dump-path>.dqcache`, or an explicit caller-supplied path. No
XDG-style or other implicit location. The colocated default is a trap worth
knowing about when the dump is mounted read-only — see `CLAUDE.md`,
"Long-running processes".

**Source identity is an opaque enum, and the number it records is the cheap
one.** Every cache records the source's `stored_size()` and mtime at save time
and re-observes on load. Size mismatch invalidates (`SourceChanged`, which a
scanning caller refuses on rather than starting cold past — see above); mtime
mismatch surfaces as a
`CacheMtimeChanged` diagnostic on an otherwise usable cache.
`ByteRangeSource::modified()` exists for this.

`stored_size()` rather than `size()` is what makes the check a `stat` even for a
decompressing source, whose addressable length costs a stream-index walk to
observe — using that as the identity would make the *check* the most expensive
thing in `pgdq info`, and would check a derived number rather than the file.
`SourceIdentity` is an enum with one variant today, `LocalFile { stored_size,
mtime }`, and the sites that read it destructure through the variant rather than
through a shared accessor: a remote source's identity is an ETag, which is not a
size-plus-mtime pair under a different name, so there is nothing worth naming
generically and a second variant is an added match arm rather than a redesign of
the type. *Rejected:* handing `cache::load` the inner source beside the outer
one, so that one could be interrogated about the other — every caller would then
carry two sources, putting the composition back in front of the layer the
dyn-compatible trait cleared it out of.

That diagnostic matters more
than it used to: under the old `info` a suspicious cache was about to be
overwritten by a rescan anyway, and now it is the answer being reported — which
is why `Diagnostic::cache_mtime_changed` is `pub` where its siblings are
`pub(crate)`. A caller that matches on `CacheStatus` itself still wants the
library's answer to "what severity is this", rather than composing its own
warning beside the ones the index carries.

**`CacheStatus::Valid`/`Incomplete` both carry `total_size`**, which is the
cache's own recorded field of that name — the *addressable* length at save time,
recorded separately from the identity's stored size precisely because the two
diverge for a decompressing source, where coverage arithmetic is about
decompressed bytes and the staleness check is about the file on disk. It is
sound to report from the cache because a stored-size mismatch produces
`SourceChanged` before either status is reached, and a cache-only caller has no
live size to stat at all. `Incomplete` is `scanned_through` short of it.
Two helpers serve both `load` and `load_offline`: `read_cache_file` does the
read plus the envelope check (returning the `CacheFile` or the unusable status
directly), and `status_from_file` turns the file into a status. `load` adds the
live-size check on top, which is the one outcome `load_offline` cannot reach.

**Diagnostics are recomputed on load, not persisted** (see "`DumpIndex`: one
owner per fact"): `status_from_file` re-derives the tiling check and the
TOC-coverage figure from the spans it just read, both pure functions of those
spans and O(spans). The cost is not close — koji's cache is 833 spans, and a
whole `pgdq info --dqcache` run against it, load and recompute and render,
is **3 ms**. What makes it load-bearing rather than an optimization: `pgdq
info` reports from a cache without ever scanning, so anything not recomputed
there is simply lost.

**`CacheMode::load` treats `Incomplete` exactly like `Valid`**, and the
completeness question has two forms, one per kind of caller. A caller holding a
live source (`map_file`, `table_stream`) compares `DumpIndex::scanned_through`
against the size it already had to stat; answering "no index" for `Incomplete`
would instead make `map_forward` restart from byte 0 every time, when a partial cache
is the *normal* shape there — a cold query's map is designed to stop short once
its target settles, and a resumed `parse` builds on exactly such a cache. A
caller that instead *reports* what a cache holds has no live source to compare
against, so it reaches for `cache::load`/`CacheMode::load_offline` and matches
on the full `CacheStatus`. Neither form is CLI plumbing: an embedder asking
"does this cache already cover what I need" reaches for whichever matches what
it holds.

**`CacheMode::Offline(PathBuf)`** is never produced by `CacheMode::resolve`;
the CLI constructs it directly when `pgdq info` gets no `--source`. The split
is enforced library-side, not just by the CLI: `load`, `save` and
`require_enabled` reject `Offline`; `load_offline` rejects
`Enabled`/`Disabled`; `read_table` (L4) rejects `Offline` explicitly, ahead of
`table_stream`'s own `load` call, so a caller does not have to trace through a
generator to learn that `query` never accepts a cache-only mode.
`load_offline` returns the full `CacheStatus` rather than `load`'s `CacheLoad`,
because a cache-only caller has to tell `Valid` from `Incomplete` — the
distinction `load` erases — with no scan to fall back on. It cannot reach
`SourceChanged` at all — there is no live file to compare against, which is
exactly what its `CacheOffline` diagnostic warns about.

**A compression index is a sibling of `ContainerKind`, not a value of it.**
`ContainerKind` means *what produced the indexed blocks' byte offsets*, and a
compressed source's offsets are uncompressed ones — byte for byte what a plain
scan of the same content produces — so it stays `Plain` and an archive format's
entry-relative offsets remain the only thing that ever changes it, which is what
the tag was reserved for. What varies instead is `CacheFile::compression:
Option<CompressionIndex>`, an enum from the day it was added and holding xz's
seek table as its only variant. Built at save time from
`ByteRangeSource::seek_table()`, so persisting a cache never re-walks the file
to get one, and round-tripped on load. It is an enum rather than a bare table
because a gzip index is a different *shape* — a set of checkpoints carrying
~32 KiB of dictionary state each, not a list of independently decodable blocks —
so a later codec adds a sibling variant rather than restructuring this field or
guessing its shape from one instance. Size is not a concern in any shape: the
motivating file's table is 31,150 entries against a koji cache that is already
833 spans.

**Anything persisted is expressible in L1's vocabulary** — declared type
strings, not resolved Arrow types. That is `layering.md`'s rule 5 and it
applies to everything the cache grows later.

## CLI surface

**The output shape is provisional**, pending real user trials. Nothing depends
on it.

### `info` reads; `parse` scans

Three verbs, and the boundary between them is drawn once: **`parse` is the only
command that reads a dump for its structure, and `info` never does.**

`info` reports whatever the cache holds, however partial, and marks the
coverage. With no usable cache it errors and names `pgdq parse` rather than
starting an hours-long scan on the user's behalf. `--dqcache none` — "ignore
the cache" — is rejected for the same reason `parse` rejects it: it leaves the
command with nothing to do.

*Rejected:* keeping a scanning fallback on `info --source` while `--dqcache`
alone kept refusing, or a `--no-scan` flag to opt into the cheap answer. The
first silently changes what an existing command answers; the second adds a flag
whose behaviour duplicates a command that already exists. The surprise a user
reports is not the *duration* of an unrequested scan but that one happened at
all.

`--preamble-only` therefore hangs off `parse`: its name states a scan extent,
and after this `info` has none. `index::preamble_only` is unchanged; only the
verb moved, and `info` reads the partial cache it leaves like any other.
*Rejected:* keeping it on `info` as the one documented exception (a rule with
one exception is a rule nobody can state), and keeping it as a pure display
filter against a complete cache (`--verbose`/`--map` already control detail,
and the name would talk about scanning while doing none).

`query` is untouched, and the asymmetry is deliberate: `query` is asked for
rows that exist only in the file, where `info` is asked what is known. Its
two-path model is what makes a cold query on a 784GB dump affordable.

### The CLI's two refusals are worded as one

**"This cache was written from another file" reaches a user two ways, and both
end in the same clause.** One is the stored-size mismatch, raised by the
library as `Error::CacheSourceMismatch` and surfaced verbatim by `parse` and
`query` ("The cache"). The other is a compression claim the file contradicts,
which recognition catches before a source exists, so no `CacheStatus` describes
it and `load` never sees it ("The compressed source"); `open_with_cache` refuses
it for all three commands, having read nothing. Two conditions, two sites, one
tail: **remove it, or name a different cache path**, which is the whole of what
a caller may do about it — there is no override.

**Both conditions stop before the file is opened, and only one of them is a
sentence `open_with_cache` can write.** The contradicted compression claim
reads the same to all three commands, so that one bails where it is found,
rather than handing a `Recognized` back to three call sites that each write the
same refusal; only `CacheMode::Enabled` claims anything about the file, so only
it can be contradicted, and it is the mode holding the path the message names.
The stored-size mismatch does not read the same to all three — the two scanning
commands surface the library's error and `info` prints the sentence below — so
it comes back as an `Opened::SourceChanged` the caller narrows: `open_for_scan`
wraps it for `parse` and `query` and raises `CacheMode::source_mismatch`, the
same constructor the three scan entry points use, and `info` renders the
`CacheStatus::SourceChanged` that condition *is*. Three wordings stay where they
were; what changes is that none of them costs an `.xz` file's footer walk to
reach. *Rejected:* refusing both conditions in `open_with_cache` with one
sentence, which is the shorter helper and silently retires `info`'s arm — the
one that names the two ways out ahead of a command that would only refuse
again.

**Three of `info`'s four `CacheStatus` sentences reach `pgdq parse` directly
and the fourth does not.** `parse` scans over `Missing`, `Unreadable` and
`UnsupportedVersion` — nothing at that path is worth keeping — and refuses
`SourceChanged`, so that sentence names the two ways out *before* it names the
command, a reader sent straight to `parse` otherwise meeting a second refusal.
That ordering is asserted rather than left to review: `refusals_name_both_ways_out`
(`pgdump_query-cli/tests/partial_reporting.rs`) puts the mismatch to all three
commands and checks the clause in each, the wording living in two crates.

*Rejected:* a fifth arm in `unusable_cache_message` for the contradicted
compression claim. That function matches exhaustively on `CacheStatus` so a new
status has to be answered; this condition is not one, and an arm for it would
have to be reached by a synthetic value. It borrowed `Unreadable`'s sentence
until the refusal made the two answers differ — "check the path, or run `pgdq
parse`" is advice `parse` cannot take, being the command that just refused, and
the bytes at that path *are* a pgdq cache, for another file.

*Rejected:* re-wording `Error::CacheSourceMismatch` in the CLI so `parse` names
the dump path as well as the cache path. The library sentence already carries
what the message owes — what it found, what it expected, and the two ways out — and the
dump is the argument the user just typed; catching and re-rendering one library
error at one call site puts a second authority over a message the same error
prints everywhere else.

### `parse` resumes, and saves as it goes

`stream::map_file` is the bounded preamble prepass, then `map_forward` with no
stop target, then the three whole-file facts only a scan reaching EOF may state
— metadata recomputed over every span, diagnostics recomputed, and a final
save. It returns the frontier it started from, which is the one line `parse`
prints *about the invocation* before the listing that describes the file.

Of the diagnostics a resumed run loaded with its cache, only `CacheMtimeChanged`
survives into that recompute. The tiling check and the TOC-coverage figure are
pure functions of the spans and are re-derived both at load and here (see "The
cache"), so carrying the loaded list forward whole would simply double them;
the mtime warning is the one nothing else can re-derive, since it is a fact
about the load.

**The metadata is stated at every legal boundary, not only at EOF.**
`preamble::dump_metadata_from_spans` may be called at exactly two kinds of
point (I1): end of file, or the start of the current database's first `COPY`
block — anywhere else makes the trailing database's `preamble_complete` a lie.
The second kind *recurs*, once per `\connect`ed database, and the mapping loop
already sees the event with the governing database tracked, so it recomputes
there: at a `CopyStart` whose governing database differs from the one the
metadata in hand covers. That is what makes an interrupted `parse` come back
*typed* for every database segment it finished, not merely for the one the
prepass captured.

**Once per database, not once per block.** Recomputing at every `CopyStart` and
relying on idempotence would put a whole-index-sized cost in the loop at every
block — the very shape the gate below took the splice out of; a single-database
dump — koji included — recomputes nothing here at all, the prepass having stood
on that same offset. The comparison state is seeded from
`metadata.databases.last()`, since `dump_metadata_from_spans` finalizes the
last database it walks, so that entry is the one whose boundary the metadata in
hand stands on.

**A cache holding no metadata heals on the next resume**, which is what kept
this from needing a format bump: the prepass is skipped on a resumed scan, but
"no metadata" differs from any database, so the first `CopyStart` recomputes —
and a resume reaching EOF instead is covered by `map_file`'s own recompute.

**Resume is the default**, and removing the cache file is how to force a fresh
scan. *Rejected:* a `--restart` flag — deleting the file says the same thing
without one.

The cache is persisted at `CopyEnd` watermarks as the scan advances, which is
what makes resuming worth building: a watermark is a resumable point by
construction — the scanner is back in `Outside` there — so `parse` is a second
caller for an existing loop, not new machinery. *Rejected:* teaching
`index::build_index` to resume. It is the eager, whole-file producer; giving it
a frontier makes it a second implementation of `map_forward`'s
splice-onto-a-prefix logic with a different set of bugs.

**The save throttle is self-tuning, not an interval.** Every save serializes
the *whole* index and the index grows with the block count, so saving at every
watermark is O(blocks²): koji's 74 blocks cost +1.5% wall, while 4000 small
blocks cost 47 s against a file of 1.9 MB (`measurements.md`, "Per-block cache
saving"). `SaveThrottle` skips a block's save unless at least `K = 20` times
the last save's own *measured duration* has elapsed since it, which bounds save
overhead at roughly `1/K` of scan time in every regime with no constant that
has to be right in two of them — a cheap cache saves often, an expensive one
saves rarely, koji is untouched. *Rejected:* "every N seconds" and "every N
bytes"; both choose a number against one dump shape, and the cost tracks block
count rather than bytes read.

**One gate decides both halves of that quadratic.** The save is one; the
*splice* is the other, and it is the larger — every `CopyEnd` used to clone the
whole span list (`map::Builder::snapshot`, then `stream::splice` over the
prefix) whether or not anything read the result. So `map_forward` rebuilds
`index.spans` at the gate's openings rather than at every watermark, which
takes a 4000-block `parse` from 20.8 s to **0.113 s** (`measurements.md`,
"Per-block cache saving"). The two costs were multiplying rather than adding:
the splice inflated the scan, and a longer scan is what the throttle reads as
licence to save again, so removing one shrank the other with it.

**Three things open the gate**, and the third is what a `parse` never needs:
the throttle being due, a pending cancellation, and a completed block whose
`COPY` header names the queried table. That last is there because
`target_settled` is the one reader of `index.spans` inside the loop, and only a
block it counts can turn its answer from false to true — so splicing for those
keeps the early stop exact while every other block skips. It is matched on the
**header alone**, deliberately a superset of `target_settled`'s own test: a
block the database selector excludes cannot settle the target either, but
repeating that test at the gate would tie the gate's width to `target_settled`'s
body, where a later narrowing there would silently cost the early stop. The
metadata recompute at `CopyStart` is not a fourth opener — it splices its own
copy and never touches `index`.

**What it costs is the interrupt's promise, and the bound is time rather than
blocks.** A graceful interrupt banks the last *spliced* watermark, not the last
completed block, so it loses whatever completed since the gate last opened. The
throttle is self-tuning against the last save's own duration, so that window is
~1/`K` of elapsed scan time wherever saving costs anything, and *nothing at all*
where blocks are far apart — koji's are ~45 s apart with sub-second saves, so
its gate clears at every `CopyEnd` and its interrupt still loses only the block
in flight. The degradation is confined to the shape where the lost blocks are
small. **The first block of a segment always splices** (`SaveThrottle::new`
starts due), so a scan that dies early still leaves a resume point.

**The byte-identical-resume property is untouched**: the resume point moves, the
structural record it reproduces does not.

<!-- deficiency: KD5 -->
**What the gate does not fix**: the splice itself is still a whole-list rebuild,
so the map is O(blocks × splices) rather than O(blocks). With a cache that costs
something the throttle holds the splice count to dozens; **with `--dqcache none`
it holds nothing** — `cache.save` is a no-op, so `due()` is always true and the
map is rebuilt per block exactly as it was, 19.1 s for 4000 blocks under `query
--dqcache none` (`measurements.md`, "Per-block cache saving"). koji cannot show
either shape: 74 blocks over 784 GB. That is deficiency `KD5`
(`../status/STATUS.md`, "Known deficiencies"). A profile of the pre-gate
4000-block `parse` puts `stream::splice`'s subtree at **43%** of it, over an
allocator that is **57%** of the whole — `_int_malloc`, `_int_free_chunk`,
`memmove` and `__libc_malloc2` alone are 12% each — which is what cloning a
span list per block looks like from underneath (see "Where a scan's time
goes").

*Rejected:* giving the throttle a floor so that a disabled cache throttles too.
It reads as the obvious repair for the paragraph above, and it buys the
`--dqcache none` case by taking the in-memory map's own interrupt: `map_file`
under a disabled cache returns its `DumpIndex` to the caller, and a gate that
never opens would hand back one that maps almost nothing. The throttle's rule is
a ratio against a *measured* cost, and there is no cost to measure there.

*Rejected:* keeping the frontier's spans appendable rather than rebuilt. It is
the structurally right fix and it is a rework of an already-tested core path for
a series the gate has already flattened by 184×; it is also what a parallel
splitter wants, so it belongs to the phase that reworks `splice` anyway —
`roadmap.md`'s parallel-scan phase, which owns `KD5`'s remainder.
`map::Builder::snapshot` `debug_assert!`s `Mode::Idle`, so the chunk-top check
cannot re-derive the spans mid-block — which is why the coupling could not be
worked around locally and the promise had to move with the fix.

<!-- deficiency: KD14 -->
**The same series measured in memory grows too, and by more than the spans
account for.** Peak resident set is flat in *bytes* — 1535× the bytes of a
one-block dump moves it by less than the readings' own spread — and it is **not**
flat in *blocks*: ~7.7 KB a block at 500 and ~9.9 KB at 4,000, so a 4,000-block
`parse` sits at 43.6 MiB where a one-block one sits at 5.9 MiB
([`measurements.md`](measurements.md), "What a scan holds resident"). Three
mechanisms could produce that and the figure separates none of them: the span
list itself, the whole-list clone above, and glibc returning little of what a
churn of clones frees. That is deficiency `KD14`
(`../status/STATUS.md`, "Known deficiencies"), unowned, and what would promote it
is a dump with tens of thousands of blocks — which nothing in hand is, koji
having 74. What it already changes is how the design's memory claim reads: the
~5.9 MiB every consumer above quotes is the *one-block* reading, and they say so.

**A save count is a property of the apparatus, not only of `K`.** The throttle
is a ratio against the last save's own duration, so a faster machine, libc or
allocator saves *fewer* times rather than the same number more cheaply — and,
since the gate above put the splice on the same rule, so does a faster *loop*:
the 4000-block count read 105 on glibc, 110 on musl and 195 on the SSD-warm
session that first recorded it, and reads **5** once the map stops being
rebuilt per block. It is therefore a reading about the whole apparatus and
never a number to compare across builds.

**Every exit saves unconditionally** — EOF, a settled target, and an interrupt.
The throttle's whole risk is the window between saves, and those three are
where that window would cost something real: skipping the save at the last
watermark before an early stop would throw a query's scan away.

**The interrupt guard is a cooperative flag** (`ScanOptions::cancel`, an
`Option<Arc<AtomicBool>>` defaulting to `None`), read **at both extremes**:
once per chunk, and at every completed block. Neither alone is enough — koji's
largest block is hundreds of gigabytes, so a Ctrl-C waiting for the next
watermark cannot be told from a hang, while a 4000-block 2 MB dump spends its
whole 23 s scan inside two chunks — and together they bound the response by the
shorter of a chunk and a block. **The rule is the principle, not the
enumeration**: read the flag at every point the loop can cheaply reach, because
any list of sites is one the next loop invalidates.

*Rejected:* `tokio::select!` in the CLI over `ctrl_c` and the scan future. It
reads as the obvious form and it is the one that silently discards the work:
`map_file` owns the `DumpIndex` for the whole scan, so cancelling that future
drops the map rather than saving it.

Two limits the principle does not remove. The flag is read *before*
`source.read_range`, so a scan blocked in a slow read notices only when that
read returns — irrelevant for a local file, potentially seconds for a remote
store. And `scan::scan` — hence `index::scan_preamble` — ignores the flag on
purpose: stopping there is indistinguishable from reaching the first `COPY`
header, so a truncated preamble would be cached as complete and its DDL
believed. The preamble is an uncancellable region bounded by its own length,
and a Ctrl-C during a query's prepass is honoured at `map_forward`'s first
chunk check immediately after, with the prepass's metadata already saved.

**A graceful interrupt and a `SIGKILL` lose the same thing**, because the
splice, the roles, the tablespaces and `scanned_through` all move at the gate's
openings rather than at every watermark — so during a `parse`, where the gate's
only opener is the throttle, the last spliced watermark *is* the last save. The
guard's remaining value is that the graceful case says where it stopped and
exits 130/143, and that a query's settled stop banks a watermark the throttle
would have skipped. What the throttle's window costs is stated above and is the
same for both: whatever completed since the gate last opened.

**A resumed scan reproduces an uninterrupted one exactly.** Not the same
totals — the same structural record, span for span. Verified against the 784 GB
koji sample: signalled 1200 s in, 2% through, reported, then resumed to
completion, and the resulting `.dqcache` is **byte-identical** to a
straight-through scan's (`measurements.md`, "koji full scan"). That is the
strongest statement the guard and the resume path can make together, and it is
what justifies treating an interrupted `parse` as a saving of work rather than
a partial result to be distrusted. `pgdump_query/tests/map_file.rs` asserts the
index half of it at fixture scale.

`map_file` reports an interrupted run as `MapRun::interrupted` rather than
returning an index that would claim to describe the whole file: the three
finishing steps above are exactly the ones a partial map may not state. The CLI
catches `SIGINT` and `SIGTERM`, prints where the scan stopped and that
re-running resumes, and exits 130/143 so a script can tell an interrupt from a
failure; a second signal of either kind exits immediately, so a save that
wedges cannot hold the process. `query` shares the loop and the throttle; a
*cancelled* query is an `Error::ScanCancelled` rather than a short stream,
because rows from the blocks a stopped mapping pass happened to reach are a
prefix of the answer with nothing saying so.

### Coverage is stated once, at the top

`Scan completion: 76% (12345 bytes)` heads every `info` listing — partial or
complete — and **nothing below it is qualified**. `--json` carries the
components (`scanned_through`, already on the index, beside `total_size`)
rather than the rendered string, so a script computes its own ratio. The
percentage floors, so it reads 100% only for a genuinely finished scan — a
user checking whether an interrupted koji parse finished must not be told 100%
by rounding — and a zero-byte source reads 100%, being trivially covered in
full. The block and span summaries below it no longer repeat a byte count:
stating one number twice in a listing invites the two to disagree, and the
coverage line owns it.

*Rejected:* a per-record partiality flag. It would always carry the same value,
which reads as if it could vary. **A partial index lacks records, not
confidence**: a block enters the map only at a `CopyEnd` watermark and every
mapping pass censuses, so every record it holds is complete in itself. There is
no half-known block, only blocks past the frontier that are not there at all.
That is the non-obvious part — "partial" suggests every answer is provisional,
where here the file's extent is the only thing that is.

`pgdq info` prints `roles`/`tablespaces`/`object kinds` summaries by default
(nothing at all when a set is empty), plus `diagnostics:`. `--map` lists every
span as `[start, end) <one-line label>` in file order, grouped by database the
same way the block listing is. `span_summary` is the one place in the codebase
that matches every `SpanBody`/`DataBlock` variant for display, and a future
`--filter-kind` should extend it rather than duplicate the match. 

**`--verbose` lists the user-defined types beneath the count that had been
their only trace.** `print_metadata` prints `user-defined types: <n>`, and
nothing else in `info` names a user-defined type at any verbosity — `object
kinds:` beneath it counts `TYPE`/`DOMAIN`/`SHELL TYPE` TOC entries, which do
not even sum to it (`types` is keyed on the type, not the statement). So a user
could not learn from `info` that `public.mood` exists, let alone what it holds.
Under `--verbose` the count becomes that listing's heading: one line per type
per database, in declaration order, name then `type_kind_summary`'s rendering
of its `TypeKind`.

**Every arm renders, with whatever payload it carries** — the enum's labels,
the domain's base type and `COLLATE` clause, the composite's fields (each with
its own clause), the range's subtype. A listing headed `user-defined types`
that showed only enums would be a lie about what the dump holds. Kind is in
hand for every arm, so this is a rendering of `DatabaseMetadata::types`, not
new resolution. `Composite { fields: None }` says `(fields not parsed)`
explicitly, because that is the one arm whose absence changes how a column of
the type resolves; a `Range` naming no subtype says so for symmetry. Neither
shape is one `pg_dump` writes, so both are pinned as a unit test on
`type_kind_summary` rather than against a fixture.

The name column is padded to the widest name the database declares and the
right edge is left ragged, an enum's label list being as long as the type is.
**Uncapped**, for the reason the per-column labels line is: `--verbose` is the
mode that exists to be the complete rendering. The cost accepted is a wide,
ragged block for a dump declaring hundreds of types.

*Rejected:* keying the listing on the column instead, carrying the enum labels
alone off each column's `ComparisonPlan`. The labels are a property of `CREATE
TYPE public.mood AS ENUM (…)`; the plan is a derived intermediary that happens
to carry them, and it refuses arrays, composites and ranges for reasons that
have nothing to do with whether the labels are knowable — so a `public.mood[]`
column would print `List(Dictionary(Int32, Utf8))` and nothing else, which is
the same "resolved them and showed them to nobody" failure one level down.
Keying on the type also prints the list once per dump instead of once per
column per block, and never has to say *where* in a column an enum sits: a
composite with two enum fields raises a format question a type listing never
meets.

**`--verbose`'s per-column line is a complete statement of the Arrow schema.**
One line per column that has something to say: an unmapped column gets
`resolution_words`' sentence, a mapped one gets its Arrow type — unless that
type is `Utf8View`, the no-information answer and the only type a non-`Mapped`
resolution produces, so the two never both fire. `arrow_type_label`
(`pgdump_query-cli/src/main.rs`) renders it as arrow-schema's own `Display` —
terse, reversible, carrying a composite's real field names — with one
substitution: the five-field range struct is identical for every range column
in every dump, so it collapses to `Range<T>`. That is dispatched on the
[`NestedPlan`](#nested-columns-nestedplan-travels-beside-the-datatype), never
on the field names, since a user composite may declare five fields with exactly
those names. A built-in multirange and an array of the matching range therefore
render identically (`List(Range<Int32>)`) — correct rather than a collision:
they are the same Arrow type, and the declared PostgreSQL type is on the same
line. `docs/manual/type-handling.md` states the struct's real layout once,
which is what makes the elision lossless.

*Rejected:* printing the type only where the plan is not `Scalar`. It keeps
every existing line's width untouched, which is its whole appeal, but "what
does this column become in Arrow" is not a question only a nested schema
raises — a `numeric` column's `Decimal128` precision is exactly as invisible
and exactly as consequential to a caller building against the schema.
*Rejected:* a compact lowercase rendering of our own
(`list<struct<a: int32>>`). It is a second spelling of a type vocabulary the
reader already meets everywhere else Arrow is named.

**An enum column's declared labels also ride a continuation line beneath it,
uncapped.** `Dictionary(Int32, Utf8)` says *that* a column is an enum and never
*which* labels, and the type listing above answers that only for a reader
willing to carry the type name up to it. This is the in-place answer, and it
matters most to a user whose `--filter` was refused — a mistyped or wrong-case
label is the only way to fail an enum term, and `--verbose` is where that
error's clause sends them. Duplicating a short list is cheaper than the trip;
the type listing carries the completeness obligation, this line carries
locality.

They come from the column's own
[`ComparisonPlan`](#ordering-operators-compare-typed-and-the-register-says-where-that-differs)
(`enum_labels`, `pgdump_query-cli/src/main.rs`), which is where resolution
already put them, so **a scalar enum column and a domain over one carry
labels and nothing else does** — an enum inside an array or a composite has a
`Refused` plan of its own. That is the same set a label-valued `--filter` term
can name, which is the question the line answers; an empty enum has no plan
either, and the line above it already says `empty enum`.

Each label is single-quoted with any interior quote doubled. **Quoting is
forced by the data**: a label is arbitrary text, and the type fixtures declare
`has space`, `has,comma` and `has'quote` for exactly this reason, so a bare
comma-joined list cannot be read back apart. Single quotes are then the one
spelling two precedents agree on — it is what the dump's own `CREATE TYPE … AS
ENUM (…)` writes, and what a `--filter` value accepts, so a printed label
pastes straight into `--filter "mood=<label>"`. *Rejected:* Rust's `{:?}`,
which `arrow_type_label` uses for a composite's field names. Unambiguous too,
but it spells a PostgreSQL literal in Rust's escape vocabulary, and the double
quote it produces is the one the filter grammar treats as the *other* quote.
*Rejected:* the labels inline on the column's own line, which is already four
fields wide — one eight-label enum would wrap and break the alignment of every
row around it. *Rejected:* a count cap. `--verbose` is the mode that exists to
be the complete rendering, and it is what a capped rendering elsewhere can send
a reader to; capping here leaves the labels reachable nowhere.

**A domain over an enum carries the labels too**, through any chain, and no
fixture column is one — `public.derived_domain` bottoms out at `integer` and
`public.text_c` at `text`. `comparison_user_type`'s recursion is the only thing
that makes the claim true, so a unit test on `comparison_for` against a
synthetic `TypeDef` list is what checks it, rather than a fixture column that
would cost six majors of regeneration to add.

**`pgdq info --json` dumps the internal struct, not a designed format.**
`IndexJson` (`pgdump_query-cli/src/main.rs`) flattens `DumpIndex` and adds the
three things it does not carry: `total_size`, `diagnostics` (the one field
`#[serde(skip)]` drops for the cache's reasons) and `resolution`. It is **not**
a second output shape to maintain — it carries zero compatibility promise, so
restructuring a `DumpIndex` field for internal reasons is free to change the
JSON with it. It exists so an alpha user can get everything the listing shows,
and the raw span/TOC detail no text view surfaces, without pgdq committing to a
flag for their specific need before those needs converge. It is incompatible
with `--verbose`/`--map`, which only add formatting detail the full struct
already carries.

*Rejected:* a hand-shaped JSON schema (renamed fields, a stable top-level
contract) — that is the CLI-output work this project is deferring pending real
trials. **No `version` field either**: that is precisely the compatibility shim
`roadmap.md`'s "Pre-1.0" forbids, and it would be the only one in the tree.

### Machine-readable resolution

`resolution` is what `--verbose` prints per column, in a form a script can
branch on: per column the name, the declared PostgreSQL type, the outcome as a
stable token, the Arrow type as the *exact string* `--verbose` renders, and the
`NestedPlan` structurally (which is the one thing the Arrow type cannot say —
`int4range[]` and `int4multirange` share it).

**It carries no per-column `labels` field.** `IndexJson` flattens `DumpIndex`,
so `metadata.databases[].types[]` is already in the export in full — every
user-defined type by name with its whole `TypeKind`, `public.mood` as
`{"name":"public.mood","kind":{"Enum":{"labels":[…]}}}`. A per-column field
would duplicate that, and duplicate it worse: a consumer joining
`resolution[].declared` against the type list gets an answer for `public.mood[]`
too, where the field did not. `resolution`'s job is to say what each column
resolved *to*; the labels are a fact about the type. The text listing is where
the per-column line earns its keep, because there the reader is a person
standing at one refused column rather than a script that can join.

**One resolution pass, two renderings.** `block_resolutions` is the single
pass that `print_index` and `print_index_json` both consume, and
`resolution_words` returns the token and the sentence from *one* exhaustive
match, so a new variant cannot be given one spelling without the other —
a second implementation would drift into describing a different vocabulary from
the listing. The cross-check reconstructs every expected `--verbose` line out
of the JSON and finds it in the text, which works because **every sentence
begins with its token's words**, underscores replaced by spaces; a variant
breaking that property fails the test rather than quietly weakening it. The
labels are held to the same standard one level up: the `--verbose` type
listing and `metadata.databases[].types[]` are checked to name the same types,
and `public.mood`'s printed list against the exported one.

**Keyed by `COPY` block** — `(database, qualified name, header_offset)` — not
rolled up per table. *Rejected:* keying by table, which is what a script most
likely wants and is not well-formed yet: one table can span blocks (I2), and a
header-less block takes placeholder `column1…N` names from its first row, so a
rollup needs a rule for disagreeing blocks and for column identity. P6's
`TableProvider` has no choice but to write that rule, so guessing at one here
would mean the embedded API had to contradict it; per-block keying leaves the
grouping with the consumer, where it honestly sits. *Rejected:* shipping both,
which is the same guess with a fallback bolted on — a rollup is purely additive
once the merge rule exists.

A header-less block is exported with an empty column list rather than omitted,
so the document's shape does not vary per block. Same reason
`MetadataNotScanned` is a value rather than an absent key: a value a consumer
can branch on is worth more than a hole.

**`query`'s text output is byte-identical whether typing is on or off**: every
value is rendered back to the PostgreSQL text `pg_dump` itself wrote. The point
is that the two `--schema-mode`s stay comparable, which is exactly what you want
when checking whether a mapping is right, and it keeps `pgdq query` pipeable
into anything expecting COPY TEXT.

*Rejected:* Arrow's own display formatting, the tempting default. It renders
timestamps in its own format, and `Decimal128`/`FixedSizeBinary` in forms that
do not round-trip back into PostgreSQL at all.

**`parse --dqcache none` is rejected at the CLI**: `parse`'s whole purpose is to
persist a cache, so disabling it is a contradiction. `info` and `query` accept
it (ignore any existing cache, persist nothing).

All three subcommands take `--source <path>` and `--dqcache <path>`; `query`'s
table argument is `--table`. There are no positional arguments. **Omitting
`--source` on `info` *is* the cache-only trigger** (clap's
`required_unless_present = "source"` then requires `--dqcache`), so there is no
separate flag and no "guess why the open failed" ambiguity.

**Both `info` invocation forms stay**, because they answer different questions.
`--source X` locates and validates against `X.dqcache` — only this form can
detect that the file changed. `--dqcache P` alone reads P with no live file to
check against, and says so (`CacheOffline`). The cache records the source's
size, so the coverage line works either way. `info_offline` refuses the same
three unusable outcomes and reports `Incomplete` like any other cache.

`preamble_only` returns `(DumpMetadata, Vec<Diagnostic>)`: it was the one
library entry point answering with `DumpMetadata` alone, so its diagnostics had
nowhere to travel.

## Errors

`Io`, `Join`, `UnterminatedCopyBlock`, `UnterminatedLargeObjectRegion`,
`LineTooLong`, `InvalidUtf8`, `CacheEncode`, `CacheDisabled`,
`CacheSourceMismatch`, `UnknownPredicateColumn`, `UnknownProjectionColumn`,
`DuplicateProjectionColumn`, `ResumeQueryMismatch`, `AmbiguousTable`,
`MetadataNotScanned`, `FieldDecode`. The CLI uses `anyhow` over these.

`CacheSourceMismatch` is the only one of these a *cache* raises, and it is
raised for what a scan would do rather than for what the read found: three of
the four unusable statuses start cold, and the fourth would overwrite an index
that is valid for another file ("The cache").

A value that contradicts its declared type is an **error, not a null**: a dump
is machine-generated, so the file is damaged or the mapping is wrong, and both
are worth hearing about with the byte offset.

`FieldDecode`'s message names `--schema-mode strings`, which is a real remedy
for every cause it has: reading more of the file cannot help any of them. The
array-shape refusal is the case that made this true — before the census was
consumed, scanning further *was* the answer for a top-level array column, and
a message naming one remedy would have been wrong half the time.

## Fixtures


**Adding a shape is the default, not a last resort** — the rule and its
evidence are `roadmap.md`, "Expand the generated fixtures freely". **Half of it
is enforced here.** `tests/pgtype.rs`'s
`every_resolution_outcome_is_produced_by_a_real_fixture_column` resolves every
column of every `COPY` block of every fixture and requires each
`ColumnResolution` variant to have a real column behind it, so a new refusal
cannot land without the `pg_dump` output that reaches it — and its exhaustive
match makes a new variant a compile error until it is listed. The other half —
a shape resolving to an outcome already covered and merely *working* — stays a
judgement call, which is what the five multi-hop shapes in `t_nested_array`
are. Naming that limit beats a check implying coverage it does not have.

`ColumnResolution::MetadataNotScanned` is that rule's one exemption, listed in
the test with its reason inline: it is a property of *how much of the file was
read*, not of a declared type, and every fixture there is scanned to EOF, so no
fixture column can produce it. It is covered against a hand-truncated index in
`pgdump_query-cli/tests/partial_reporting.rs` instead, in both directions — the
later database's blocks report it, and the same blocks resolve properly once
the parse finishes.

`fixtures/<version>/<schema>/<flag-set>.sql`, real `pg_dump` output across the
six routine versions (13–18). **"Absent" in this tree always means
"deliberately absent", never "not yet generated."**

**One image family for all six, pinned at an exact minor with the `-trixie`
suffix.** The minor is pinned so a regeneration is reproducible rather than
drifting to whatever the tag resolves to that day; the suffix is what pins the
**libc**, and the libc is what the comparison oracle's text answers are taken
under — on Alpine, musl's `strcoll` is `strcmp`, so every text comparison would
silently be the `C`-collation answer. The unsuffixed `postgres:16` has already
moved Debian suites once, which is the drift the suffix exists against.
`meta.tsv` records the platform triple and the default collation's version, and
the cross-major differ guards both, so a major generated on a different base is
a reported fault rather than five hundred silent differences.

*Rejected: keeping the musl family and taking the glibc answers from a separate
one-major witness file.* It is much the smaller diff — no regeneration, no
correctness test re-run — and it was the plan until the question was put the
other way round: what should this project's evidence *be* taken under? A witness
bolted onto a musl apparatus answers "musl, plus a footnote", and every later
reader of the oracle has to remember the footnote before reading a text cell.
Moving the family answers "glibc" once, in the place the answers come from. The
cost was paid in full and bought an apparatus that needs no footnote; what it
did **not** cost is the agreeing half, which musl gave by accident — asking each
text pair under `COLLATE "C"` as well recovers it deliberately, so neither half
now depends on which image was used.

**One consequence of the family reaches the `.sql` tree**, and it is the only
one that did: a Debian build sets `--with-extra-version`, so every fixture's
version header now reads `16.15 (Debian 16.15-1.pgdg13+2)` where an Alpine
build wrote `16.15` (I36). The koji sample carries the bare form, so the tree
holds both shapes rather than only one. Nothing parses those strings — they are
reported verbatim — and no snapshot covers them.

`fixtures/<version>/oracle/` and `fixtures/<version>/adbc/` are the two
directories that are not `pg_dump` output — see
[The comparison oracle](#the-comparison-oracle) and
[The ADBC floor oracle](#the-adbc-floor-oracle). Both sit beside the schema
directories rather than inside one because neither has a flag set; the test
vocabulary's `all_fixtures()` walks for `*.sql` and steps over them.

`fixtures/oracle-differences.tsv` is the one file at the tree's top, because it
belongs to no single major — see
[The cross-major differ](#the-cross-major-differ). Both fixture walks recurse
into directories only, so it is invisible to each.

`scripts/generate_fixtures.py [--version N] [--schema <name>] [--skip-dumps]
[--skip-oracle] [--skip-floor]` regenerates; **the dump and oracle passes
across all six versions are 80 seconds** in throwaway 512MB containers, never
a host-run Postgres, with the images already pulled, and the floor sweep adds
**under 30 seconds** — that being the whole `--skip-dumps --skip-oracle` run,
its six container starts included, which a combined run does not pay twice.

The three `--skip-*` flags split the three passes, and skipping the dumps is
the ordinary way to re-take either answer table: a dump regeneration is not
byte-reproducible (below), so touching the `.sql` tree to change an answer
table would bury the change in noise. Within that 80, an oracle re-take
(`--skip-dumps`) is **63 seconds** and one schema's dumps (`--schema <name>`)
**43** — each paying its own six container starts, which is why the parts
overshoot the whole rather than dividing it. **Scope a regeneration to the
schema that moved**: a wider one
rewrites every file it touches with a fresh `\restrict` token and adds
`--binary-upgrade`'s OID drift on top, both of which bury the change under
review.

**That figure has a mechanical trigger, not a comment asking someone to notice
it.** The script prints its own elapsed time at the end of every run, and past
**30 minutes** prints that this paragraph is stale and must be updated — 30
being where a job stops fitting inside one session and has to be handed off
(`CLAUDE.md`, "Long-running processes"). What the number is for is that
decision: at 80 seconds a regeneration is something a session runs twice
without planning around it.

**A regeneration is not byte-reproducible, and four things move on their own.**
`\restrict`/`\unrestrict` carry a random token per dump (v17.6+/v18+);
`logs.events.logged_at` and its `pgdq_tenant` counterpart default to `now()`;
`--verbose`'s `-- Started on` / `-- Completed on` header lines are wall-clock
times, so `objects/verbose.sql` moves where the other `objects` sets do not;
and `--binary-upgrade`'s OIDs can shift by one, because `create_fixture_db`
retries `createdb` against the image's transient init instance and a failed
attempt still consumes an OID. The first three move on every run, the fourth
only sometimes and per version — two consecutive runs agreed while both
differed from what was committed. So a regeneration diff is expected to touch
every flag set of the schema, not only the fixture that motivated it, and
**nothing may assert on those four**. No insta snapshot covers any of them.

**Those four are the whole of it, which is checked rather than believed.** A
regeneration with no schema change was run and diffed against the committed
tree: 109 files moved, and every changed line was a `\restrict` token, a
`now()` timestamp or a `--verbose` header — no DDL, no OID drift on that run.
That is what makes a schema change's diff readable, since the alternative is a
reviewer unable to tell an added table from noise.

| Schema | Flag sets | What it is for |
|---|---|---|
| `edge_cases` | `default`, `create`, `no-comments`, `dumpall`, … | Scanner and preamble robustness shapes; `public.escapes` holds one row per codepoint |
| `types` | `default`, … | One table per Arrow-mappable type family, including boundary values |
| `objects` | `default`, `verbose` | One object per TOC `Type:` kind neither other schema produces, plus two large objects, a non-default tablespace, and a solo `REVOKE` |
| `objects/stats.sql` | v18 only | `TOC_PREFIX_STATS` |
| `partitions` | `default`, `--load-via-partition-root` | The multi-block-per-table shape |

**The `types` schema's array tables are divided by what a reader may conclude
from them, not by shape.** `t_array_shape` is read as "these are the shapes the
census reports", so it holds exactly the census's own cases — uniform 2-D,
mixed dimensionality across rows, an `[lb:ub]=` prefix — and every column whose
census entry is correct but misleading is kept out of it. `t_nested_array`'s
`intarr[]` column censuses `(1, 1)`, correctly, because the literal is one
brace deep (I26); `t_array_spelling`'s four columns census nothing at all,
since their values are ordinary 1-D arrays and the novelty is entirely in the
DDL. `t_delimiter` is separate from `t_base_type` for the same reason one level
over — the delimiter trap (I22) in one table and the opaque-element case it is
easily confused with in the other is what lets a test say which of the two it
means.

**`t_int2vector` is deliberately *not* among them**, even though the type's
floor answer is `list<item: int16>`. The array recursion is a spelling test
over the declared name (I28) and `int2vector` carries no array spelling, so no
array mechanism reaches the column and its census entry does not exist —
filing it beside `t_array_shape` would invite exactly the conclusion the
division above exists to prevent. It sits with the scalar families instead,
next to `t_oid`, the other `pg_catalog` type the floor pulled into the tree.

Its five rows are the whole of `int2vectorout`'s grammar, which writes
space-separated `int16` with no quoting, no escaping and no possible NULL
element: a three-element value, an empty one, both `int16` bounds, a single
element, and SQL NULL. **The second is the row that decides a mapping.** An
empty vector is legal and `int2vectorout` writes it as the empty string, so
COPY TEXT writes an empty *field* for it and `\N` for the NULL beside it —
which is what leaves an empty list and a NULL still distinguishable in the
file. The column also pins the declared spelling at six majors: `v_vec
int2vector`, bare, because `pg_catalog` is on the search path implicitly
whatever `search_path` is set to (I8) — a `pg_catalog` type with no standard
SQL spelling is written bare all the same.

**`t_collate` is the `types` schema's collation table, and it is separate from
`t_text` for the same reason**: `t_text`'s three columns carry no `COLLATE`
clause, so they are read as "what a column that says nothing about its
collation does", which is the *divergent* half of the register's collation rule.
`t_collate` is the agreeing half — `COLLATE "C"`, a bare `name`, and a column
of a domain declared `AS text COLLATE "C"` — plus the clauses that must still
diverge: `en_US.utf8`, `ucs_basic`, and `public.c_collation`, a user-defined
collation the same dump declares as `locale = 'C'` and the register still calls
non-bytewise. It also carries the only column in
the tree whose type is a composite with a collated attribute, which is I37's
third emission site and had rested on a source grep until this table existed.

**`v_nd` is the one column whose verdict is read off a statement**, and the
`types` schema's second `CREATE COLLATION` is the statement: `public.nd_collation`
is `provider = icu, deterministic = false`, which the server allows for no other
provider (I42), so a column of it is the only equality divergence a plain dump
states outright. Its clause is spelled exactly as `v_user`'s — schema-qualified,
unquoted, outside `pg_catalog` — and the two answer differently, which is what
makes the pair evidence rather than a single case. This is also the tree's only
ICU collation, and the exception is deliberately narrow: ICU stays out of the
*comparison* columns and out of the oracle because a `collversion` moves with
the base image, and that objection is about answers, which a determinism clause
is not. **The version does reach one flag set**, `types/binary-upgrade.sql`,
where `pg_dump` appends `version = '<collversion>'` — the one place in the tree
an ICU release number is a committed byte, and itself the evidence for I42's
claim about which flag set carries it.

**That byte is guarded rather than merely tolerated**, by
`tests/preamble.rs`'s
`the_icu_collversion_reaches_binary_upgrade_alone_and_agrees_across_majors`,
which reads it out of the fixture text — the parser drops the field, so no
`CollationDef` carries it. The guard asserts **agreement, never the literal**,
which is the shape `oracle_differences.py` already uses for `meta.tsv`'s
`default_collversion`: all six majors read one version because all six pins are
`-trixie` and resolve to one libicu, and the documented drift is *across* base
images (`und-x-icu` is `153.128` on `13.23-alpine` against `153.136` on
`18.6-alpine`), so a split means the pins have drifted apart — the fault the
across-majors identity check exists to report. The other half is the shape
claim the doc comment used to make in prose: the option is absent from
`default`, absent from `data-only` (which emits no `CREATE COLLATION` at all),
and absent from `c_collation` under every flag set, a `C` libc collation having
no `collversion` to record. *Rejected: asserting the literal `153.128`.* It
fires on every routine image bump, which is a deliberate regeneration whose
diff already shows the change, and a check that fires on the expected event is
a signal that is always on. *Rejected: leaving it unwatched.* An environment
release number in a committed fixture byte is established practice here —
every fixture header carries a Debian package revision, and
`fixtures/<v>/oracle/meta.tsv` commits glibc's `default_collversion` on purpose
— so what was new about this one was only that nothing demanded of it what the
differ already demands of its sibling.

*Rejected: `und-u-ks-level2` as the locale.* It is the realistic thing a person
creates — a case-insensitive collation is why anyone reaches for
`deterministic = false` — and `preamble.rs`'s unit test spells it that way. The
fixture takes ICU's root locale `und` instead, chosen for **existence** rather
than behaviour: it is present at every ICU version with no locale generated, and
nothing here reads it, since the register branches on the `deterministic = false`
clause and never on the provider or the locale. A behavioural spelling would put
a behavioural claim in the one file whose entire case for admitting ICU is that
no behaviour is being claimed — the collation would then look like it were
asserting an order, which is exactly what keeps ICU out of the comparison
columns and out of the oracle. Realism is the cost, and it is paid knowingly:
what the fixture exists to pin is the *shape* `dumpCollation` writes, and a
locale nothing reads cannot drift into an ordering claim.

**Three of its columns exist for the clause's *placement* rather than its
value.** `pg_dump` appends `COLLATE` after `DEFAULT`/`GENERATED` and after
`NOT NULL` (I37) whatever the input said, so `v_text_def` (one displacer) and
`v_gen_nn` (all three, plus a nested call and a quoted literal inside the
generated expression) are what make that a committed fact at six majors instead
of a hand-transcribed string, and `v_gen_nn` is the only real-dump stress on
`extract_collation`'s paren- and quote-aware scan. `v_src` is `v_gen_nn`'s
source, and the `COALESCE` in the generated expression is load-bearing: the
alphabet's ninth row is NULL and `v_gen_nn` is `NOT NULL`.

**`v_gen_nn` is asserted in `tests/preamble.rs`, not `tests/ordering.rs`,
and the split is by reachability.** A `STORED` generated column is excluded
from the `COPY` column list, which is exactly why `NOT NULL` costs the table
nothing — no row has to hold a value for it — and equally why no
`TableStream` can filter or project it. `DatabaseMetadata` is the only place it
appears.

Its nine rows are one alphabet replicated across every column that carries
data, so a filter over
two of them differs by nothing but the collation: `A`, `a`, `B`, `é`, `f`, `_x`,
`ax`, the empty string and a NULL — the pairs the comparison oracle already
answers as divergent on glibc 2.41. **Do not replace them with placeholders.**
The Rust assertions over this table are about *notes*, which are independent of
the data, so any values at all would pass them and it would be easy to pick a
set that could never support a stronger test.

`SCHEMAS` uses two sentinels: `None` as a flag set means "swap the binary,
don't append flags" (that is how `dumpall` is produced), and a value may be
`(min_version, flags)` instead of a bare list for version-gated sets.

`--verbose` is the `objects` flag set's whole reason: it is the one documented
way to widen the TOC comment block past three lines. The `partitions` schema's
`_a`/`_m`/`_z` naming is load-bearing — `TABLE DATA` entries sort by the
*partition's* own name, so `evt_m`/`feel_m` exist to be emitted *between* two
partitions of one root; `feel` is hash-partitioned on an enum column, which
makes `pg_dump` force load-via-partition-root with no flag at all, so the
`default` set exercises the multi-block shape on all six majors.

**Multi-database coverage comes in two shapes and needs both.** A
`pg_dumpall` is *not* several `pg_dump --create` outputs concatenated: it
passes `--create` for ordinary databases but writes `template1`/`postgres`'s
`\connect` itself, ahead of their version headers rather than after (I9). So
`edge_cases/dumpall.sql` covers the cluster shape, and
`tests/{map_file,preamble,pgtype,stream}.rs` cover the concatenated one by
building it at test time — reading `create.sql`, writing a byte-substituted
copy with the database name changed, and appending it. The parser cares only
about the `\connect`-delimited shape, not which process produced it, which is
what makes the synthetic half legitimate.

**The `dumpall` fixture carries two data-bearing databases**, `pgdq_fixture`
and `pgdq_tenant` (`scripts/fixture_schema_edge_cases_tenant.sql`, created and
dropped for the `edge_cases` schema only). One is not enough: I1's *recurring*
metadata boundary — the mapping pass restating a database's DDL at that
database's first `COPY` block — is reached more than once only by a file whose
second segment also has data, and `template1`/`postgres` carry none, so a
single-data-segment file has every database that matters closed out by the
first block's offset. By I30 the segments follow `datname` order, so the name
alone lands the tenant between `pgdq_fixture` and `postgres`: two data-bearing
segments, then an empty one, which leaves the EOF recompute a case of its own.

`pgdq_tenant.public.widgets` repeats the other database's table name under the
same five column names with a **different type on every one of them** —
`Int64`/`FixedSizeBinary(16)`/`Binary`/`Int16`/`Date32` against
`Int32`/`Utf8View`/`Utf8View`/`Boolean`/`Timestamp`. That turns resolving a
block against the wrong database's DDL into a wrong answer a test asserts on,
rather than an absent error it has to infer from silence
(`tests/map_file.rs`'s
`one_table_name_in_two_databases_resolves_to_each_databases_own_types`, which
covers block-to-database attribution end to end where `resolve.rs`'s
`database_selects_by_attributed_name_not_by_first_match` covers the lookup
against hand-built metadata). Its `tenant` schema exists in no other fixture
database, so DDL that reaches those tables was read here rather than inherited
from the segment before.

Three construction facts, each confirmed against `pg_dump` source rather than
guessed:

- `SEQUENCE OWNED BY` needs `serial`, **not** `GENERATED ALWAYS AS IDENTITY`
  (`dumpSequence` folds identity ownership into the identity clause).
- A separate `DEFAULT` entry needs a *view* column or a `--binary-upgrade`
  dropped column (`dumpAttrDef`'s `separate` flag is set for nothing else).
- `PUBLICATION TABLES IN SCHEMA` is PG15+, and the schema carries a
  `\gset`/`\if` gate on `server_version_num`.

**Cluster-global objects need explicit teardown.** A subscription blocks
`dropdb` on its own database, and roles and tablespaces are cluster-wide, so
they leak into whatever schema runs next in the same container.
`drop_fixture_db` clears `objects_sub`, `fixture_reader` and `fixture_ts` **by
name**; a new schema adding its own must grow that function rather than inherit
a generic sweep. `pgdq_tenant` is dropped there for the same reason — a leaked
database would show up in a later schema's `pg_dumpall` output — and created in
`create_fixture_db`, gated on its schema the way `prepare_tablespace_dir` is,
so `dump_flag_set` stays a pure dump-and-write function. `CREATE TABLESPACE` additionally needs its `LOCATION`
directory to exist and be owned by the `postgres` OS user, which
`prepare_tablespace_dir` does via `docker exec` `mkdir`+`chown` before the
schema SQL runs.

### The comparison oracle

**"Agrees with PostgreSQL" is a check, not an assertion.** `fixtures/<version>/oracle/`
holds what the server itself answered for a table of typed comparisons, one
directory per routine major, generated by `scripts/generate_fixtures.py` and
committed. `scripts/comparison_oracle.py` is the case table and the SQL; the
generator owns only the container.

*Rejected: continuing to argue every claim from PostgreSQL's source alone.*
That is the method that left two traps to be found by accident —
`array[1,null] = array[1,null]` is **true** and `row(1,null) = row(1,null)` is
**true**, both NULL-*aware* rather than NULL-propagating — and the comparison
work multiplied that surface by an order of magnitude. Reading the source is
still how a claim is *understood*; what it stopped being is how a claim is
*checked*.

*Rejected: checking against the local koji replica behind a `PGDQ_KOJI_PG_URL`
env var.* `CLAUDE.local.md` scopes that replica to ad-hoc local validation and
forbids anything committed from assuming it exists. A generated, committed
answer table has neither problem and covers six majors where the replica is
one — and it is evidence a checkout with no containers can still read.

It runs **against the `types` schema's own database**, in the same container,
from the same DDL as `fixtures/<version>/types/*.sql`. That is what lets a case
name `public.mood`, `public.point2d`, `public.myrange` or `public.intarr[]` — a
case's type string is the spelling `pg_dump` writes in a `CREATE TABLE`, so it
resolves against a register arm by string equality. That join is checked in
both directions; see "The register-to-oracle reconciliation" below. **A case
naming a type this schema does not declare answers `E42704` in every cell**,
which is coverage that tests nothing, so add it to
`scripts/fixture_schema_types.sql` first.

Three files, all in PostgreSQL's own COPY TEXT encoding, because the server
writes them with `COPY … TO STDOUT` and this repo already reads that encoding
in L1:

| File | Columns |
|---|---|
| `meta.tsv` | `key`, `value` — the apparatus: server version, the platform triple, the database's collation and that collation's version, and every session GUC that moves an output spelling |
| `literals.tsv` | `type`, `literal`, `status`, `output` — whether the server accepted the input, and the exact `*_out` text it canonicalizes to |
| `comparisons.tsv` | `type`, `left`, `right`, `collation`, then one cell per operator in `<`, `<=`, `>`, `>=`, `=`, `<>` order |

A comparison cell is `t`, `f`, `u` (the comparison yielded SQL NULL) or
`E<sqlstate>`; a literal's `status` is `ok` or `E<sqlstate>`. **Recording the
rejection is the point, not a fallback**: the cross-major differ reads "the
older major rejects this literal, the newer one accepts it and answers" as
*additive*, which is what licenses implementing the newest semantics with no
branch on the dump's version. Nothing is version-gated for that reason — a
multirange answers `E42704` on 13, which is the transition rather than a hole.

**A pair is asked through two typed columns.** Each comparison case builds a
temp table declared `(a <typ>, b <typ>)`, inserts the two literals through a
cast, and asks `a <op> b`. A column is what a dump holds, and its
`attcollation` is the declared type's own default wherever the DDL writes no
clause — so a case naming no collation measures what a bare column does, and a
case naming one declares it on the column, which is where `pg_dump` writes it
(I37). *Rejected: casting the two `text` parameters instead.* A cast derives
its collation from its **input**, so every bare case would carry the
parameter's `default`, and `name` — whose type default is `C` — would record
the database locale's order under a case that names none.

**Neither side raises**, and building the pair is its own subtransaction ahead
of the six comparisons. Each is a PL/pgSQL `EXCEPTION` block, so a malformed
literal cannot abort the surrounding `COPY`, and the split is what decides how
far a rejection reaches: a type the server does not have, or a literal it
refuses, is a pair that cannot exist and answers all six cells, while a type
with no `<` fails that cell alone.

**Materialising the pair is also what stops the planner answering for the
server.** A comparison written as a cast of two parameters is an expression,
and `eval_const_expressions` folds a strict operator holding a constant NULL
without evaluating its other argument — so a refused literal asked against SQL
`NULL` recorded `u` rather than its rejection, and whether it did depended on
the type's input function being `stable` rather than `immutable`. The cells
were reporting input-function volatility. Two consequences were live in the
committed answers: `interval '-infinity'` on 13–16, where the literal is
genuinely refused, read `u` and so agreed spuriously with 17's real answer,
hiding an additive transition from the cross-major differ; and `numeric`'s
immutable input function raised where the datetime family's stable one did
not, for no reason a reader could see. A column cannot be folded away, so
every such cell now agrees with `literals.tsv`, and the differ sees the
transition — 24 rows of it at 16→17.

The session pins `standard_conforming_strings`,
`DateStyle`, `IntervalStyle`, `extra_float_digits`, `TimeZone`,
`client_encoding`, `bytea_output` and `array_nulls`; the first four match
`pg_dump`'s own `_doSetFixedOutputState`, so `output` is the form a dump
writes.

**`output` is the type's own output function, not a cast to `text`**, reached
through `textin(<typoutput>(…))` with the function looked up per type. The two
differ for `character(n)`: `bpchar::text` strips the blank padding that
`bpcharout` — and therefore the dump — keeps, and "what does this literal look
like in the file" is the entire question the column answers.

*Rejected: one row per `(type, left, right, operator)`.* Six times the
committed lines to say the same thing, and the six answers for one pair are
exactly what has to be read together.

*Rejected: a ladder of consecutive comparisons instead of all ordered pairs.*
Sound only if both orders are already known to be total, which is the property
under test.

**The file carries no case identifiers, so its rows mean what they mean only
by being in the case table's order.** `scripts/test_comparison_oracle.py` is
what stops an edited case table from landing beside stale answers: it walks
every committed file and asserts the `(type, left, right)` triples equal
`comparison_cases()` row for row, in order, for every major present in
`fixtures/`. The `collation` column is part of that key: a text pair is asked
under two collations, so a `(type, left, right)` triple no longer names one
row.

**The collation is a dimension of the case, not an accident of the base
image.** The fixture containers are the Debian (`-trixie`) images, so the
server is glibc and `datcollate`'s `en_US.utf8` means glibc's collation. Every
text pair is asked twice — once under `COLLATE "C"`, once under `COLLATE
"default"`, which is the database's own — so one file holds both halves of the
ordering register's text row: pgdq compares bytewise, which **is**
PostgreSQL's answer under `C` and is **not** under a libc locale. Equality
comes along for free and agrees under both, every libc collation being
deterministic. The eight pairs that earn the alphabet are named in the case
table with the reason each was chosen; four diverge on glibc 2.41 (case below
letter, letter before case, an accent sorting with its base letter, punctuation
ignored at the primary level) and four agree, the agreeing half kept
deliberately so the file cannot be read as "these two orders never coincide".

`"default"` rather than the locale's own name because `pg_catalog."default"`
*is* the database's collation by definition and exists on every server, where a
locale-named collation object exists only if `initdb` imported one.

**`jsonb`'s values are one per branch of the comparison, not a sample.**
`compareJsonbContainers` decides on the kind, then on a container's size, then
on a scalar leaf (I41), and a case list that reaches only some of those passes
the reconciliation below while proving nothing about the rest. So the list
carries both booleans (`false < true` is a branch; `true` alone only ever
reaches the kind order), a string, both empty containers, a one-pair object
whose key sorts *after* a two-pair object's — otherwise the count and the key
point the same way and the case is silent about which decided — and the pair
that separates storage order from alphabetical. `{"a": "a"}` against
`{"a": "A"}` is the string leaf, and it is the one `jsonb` pair this build
answers differently. The grid is all ordered pairs, so each value costs
quadratically; sixteen of them make `jsonb` about an eighth of the file, which
is proportionate to its being the widest arm in the register.

**`TypeCases.collation = None` means one thing: the register does not branch on
the clause for this type.** It used to mean two, and the difference was
invisible: a bare `text` or `character varying` case is on the database's
collation, which is the *divergent* population, while a bare `name` case is on
that type's own `C` default, which is the *agreeing* one. A join resolving
`None` per type would be a second copy of the register's type-default rule
living in Python — the fork the reconciliation exists to prevent, arriving from
inside the check — so every case of a type whose arm reads a clause states its
collation, and `None` is left to the types where there is no branch to name.
`text` and `character varying(10)` therefore say `"default"` explicitly, which
changes no answer: `pg_catalog."default"` is what a bare comparison already
used.

`name` is one of the two deliberate `None`s among the collatable types —
`public.text_c` below is the other — and it is asked *bare*, through two `name`
columns whose `attcollation` is the type's own `C`. *Rejected: relabelling its
cases `collation="C"`.* True, and it is the register's claim rather than the
oracle's observation — a `name` pair asked under an explicit clause is a
different question from one asked bare, and only the bare one shows what a bare
column does.

**A refused literal answers every cell of every comparison it appears in**,
the left side winning where both are refused — a property `literals.tsv` can
be read against, and `test_comparison_oracle.py` does. It follows from the pair being
built before it is compared, and it is the second reason the cast form is
rejected: it broke the property in fifty-two of this tree's rows, twice over
and both times as something that reads like an answer. A strict comparison
against a constant NULL is folded away by the planner before the other side is
evaluated, so a rejected literal paired with SQL NULL reads `u` wherever the
type's input function is `stable` and leaves the pair non-constant — the
datetime family — while `numeric`'s immutable one raises; and a type with no
`<`, such as `json` or `xml`, fails at analysis time ahead of its own malformed
literal. It is also why the differences file's 16→17 `interval` transition
covers the pairs where an infinity meets SQL NULL: under a fold there is
nothing there to move.

**`public.text_c` is the case standing under the general rule.** What the
column form fixes belongs to any type whose `typcollation` is not `default`,
and `name` is the only *case* type where that holds — `text`, `varchar`,
`bpchar` and `text[]` all read `"default"`. `text_c`, the domain declared `AS
text COLLATE "C"`, is the other type in the oracle's own schema where it does:
a column of it is bytewise while nothing in the case names a collation, which
makes it the case that would have caught the cast. It joins `base_domain`,
`derived_domain` and `box_domain` on the register's existing `user/Domain` arm
and needed no register change.

The register's `name` row rests on I37 and `pg_type.dat`'s `typcollation =>
'C'`, on `tests/ordering.rs`'s assertion over `t_collate.v_name` — and now on
these cells, which say the same thing rather than the opposite.

*Rejected: inlining the literals.* `format('SELECT (%L::%s) …')` derives no
collation either, since an unknown literal has none to give, and it is a much
smaller diff. It swaps every case from a cast-from-`text` to the type's input
function, so any cell may move; the property worth having is that the diff is
confined and checkable, not that it is small.

*Rejected: emitting the type's `typcollation` as an explicit `COLLATE`.*
Cheapest of the three, and it reads the catalog rather than our claim, so it is
not the relabelling rejected above. It still asserts that a column inherits
`typcollation` instead of observing that it does, and that step is the one
under scrutiny.

**`character(10)` is asked under both collations**, and it was labelled one
slice before the register read the labels: while `char(n)` was compared padded
it diverged for a reason no clause could fix, so its labels bought nothing, and
they became coverage the moment the trim made a `char(n)` compare under its own
collation. Labelling it in a regeneration that was happening anyway cost
nothing; labelling it later would have cost a six-major run of its own.

Its values carry `"a"` followed by a **tab**, which is I38's ordering
corollary and the one thing its other values cannot reach: pad-and-compare and
trim-and-compare disagree only against a byte below `0x20`, and every other
value in the case is printable.

**A literal is asked once per type, wherever it is first named**, and collation
does not enter — an input function and an output function do not consult one.
Deduplicating rather than skipping a collated case is what keeps a value that
exists only in one, such as `character(10)`'s tab or the text alphabet, in
`literals.tsv`.

**"Only glibc" is a limitation of the evidence, not a claim about servers.** A
musl deployment orders text differently and this oracle does not speak for it.
What stays closed by *statement* is the residue: a `default`-collation column
carrying no `COLLATE` clause is on the database's collation, which a plain dump
does not record (I32), so there is no answer in the file to check it against.

### The cross-major differ

`scripts/oracle_differences.py` reads the committed oracles as a **chain of
adjacent majors** and records every cell that moved, in
`fixtures/oracle-differences.tsv`. It is what turns the union rule — implement
the newest semantics unconditionally, with no branch on the version the dump
header records — from an assertion into a check (I35).

Three verdicts, and only the first is benign:

| Older major | Newer major | Verdict |
|---|---|---|
| rejects the input | accepts, answers | **additive** — the value could not previously exist |
| accepts | accepts, **different** answer | **non-additive** — a real break |
| accepts | accepts, same answer | unchanged, and not recorded |

**Every other transition is non-additive too**, deliberately: a newer major
*rejecting* what an older accepted, two rejections whose SQLSTATE moved, and an
accepted literal whose canonical `output` spelling moved. The last is what
`comparisons.tsv` cannot see on its own and is the reason the differ covers
both tables — the canonicalize-the-literal-once path renders a literal into the
form the *file* holds, so a spelling that differs between majors is a rendering
no single implementation can get right. The strict reading is the conservative
direction; the one place it could raise an alarm that means nothing is a
SQLSTATE change between two rejections, which has never fired.

**Adjacent pairs rather than all pairs.** If any two majors disagree then some
adjacent pair does, so the chain is complete, and it names *where* the
transition happened rather than only that one exists.

Two things are refused rather than diffed, because both would make a diff mean
something other than it says. The answer files carry no case identifiers, so
the differ re-checks every file against `comparison_cases()`/`literal_cases()`
before zipping — mis-aligned files are a silent wrong answer, not an error. And
every major's `meta.tsv` must agree on the pinned session GUCs, the database's
collation, the **platform triple** and the default collation's **version**: a
`DateStyle` that differed between two runs would move every spelling in the
file, which is a fault in the generation rather than a difference between
majors. The last two keys are what make that guard load-bearing once a second
libc is possible at all — `version()` is expected to differ, so it is excluded,
and it was the only key carrying the triple; a major left on or reverted to a
different base would otherwise pass in silence while `datcollate` still read
`en_US.utf8` and meant a different order. The triple is parsed out of
`version()` server-side so the version number itself may still move, and the
`collversion` beside it is the server's own notion of "this collation may have
changed underneath you", read off the collation object `initdb` imported for
the database's locale (`pg_collation` records no version for `default` itself,
and `pg_collation_actual_version(100)` answers SQL NULL before v15).

**The file is committed and asserted, and the break check is separate.**
`test_oracle_differences.py` asserts the committed file against a fresh
computation, so a regeneration that moves an answer cannot land without being
re-filed (`uv run oracle_differences.py --write`); a second assertion says no
difference is non-additive, so *filing* a real break does not silence it. An
oracle pass of `generate_fixtures.py` ends by running the same check, and the
reconciliation below beside it.

*Rejected: classifying a two-rejection SQLSTATE change as unchanged.* It is the
only strictness with no consequence today, and buying it costs a third verdict
that every reader of the file then has to learn.

### The register-to-oracle reconciliation

`scripts/oracle_register.py` joins the comparison register in `pgtype.rs`
against the oracle's case table and **fails on either direction**: every
register arm must have at least one oracle case, and every oracle case must
reach an arm it actually exercises. The register grows one arm at a time as
types are closed, so the direction that decays is the first — an arm added with
no case is a claim nothing checks — and the second is smaller but real, since a
case for a type the oracle's own database does not declare answers `E42704` in
every cell and reads as coverage.

**The join is existence, not branch coverage, and that is the boundary of what
it promises.** An arm is satisfied by one case, so a row whose cases exercise
one branch of a multi-branch comparison passes identically to one whose cases
exercise every branch. `jsonb` is the widest arm and was the worked example:
`compareJsonbContainers` decides on kind, then on a container's size, then on a
scalar leaf (I41), and the arm counted as covered by cases reaching four of the
six kinds and no container structure at all. Nothing mechanical closes that — a
branch-coverage check over a `match` written in another language is not
something this join can become — so an arm's cases are owed by whoever writes
the arm, and "the reconciliation passes" is not the question to ask of a new
row. What makes that cheap to act on is that the remedy costs almost nothing:
adding cases is `generate_fixtures.py --skip-dumps`, which rewrites the
`oracle/` TSVs and touches no `.sql` file (["Fixtures"](#fixtures)).

**An arm is one answer that can be closed on its own, at the finest
granularity for which evidence can exist.** That is one rule, and the table
below is it applied to three shapes of source rather than three policies —
which is what makes a declared *name* the key on one side and a *match arm* on
the other. A built-in name is separately closable: the eleven names answering
`(Utf8View, text)` are a queue closed one name at a time, so one case must not
excuse the rest. A `TypeKind` sharing an arm with another shares one decision
and is not separately closable until that arm splits — and for one of them,
`Shell`, no oracle case can exist at all. The empty-enum arm is the same
rule a third time: its guard is separately closable and `public.empty_enum` is
its case.

**An arm is read out of the source, because a `match` is not data.** Three
functions are parsed, and each is anchored on a string the parse must find, so
a rewrite is a reported problem rather than a shorter list that passes:

| Read from | An arm is | Why that granularity |
|---|---|---|
| `builtin_scalar` | one **declared base name** | Several names share `(Utf8View, text)` and each is separately closable, so `text` having a case does not answer for `character varying`. |
| `comparison_user_type` | one **match arm** over `TypeKind`, a guarded arm counting as its own | `Composite \| Range` and `Base \| Shell` are each one decision, so neither is separately closable yet; both halves of the first already have cases against the day it splits. Exhaustiveness over the kinds is rustc's job; what this check adds is that each *answer* has evidence. |
| `comparison_for` | one per branch that is not a match arm at all, its own two plus `comparison_user_type`'s early return | The array shape, the built-in name nothing recognises, and a type absent from the dump's `CREATE TYPE` list (I10's multirange companion lands there). |
| `collated_text` | one per **collation branch**, four in all | A collatable built-in's answer depends on the column's clause — and on what the dump's `CREATE COLLATION` list said about the collation that clause names — as well as on its declared type, so one match arm carries four separately closable answers. |

**The collation is a second dimension, and joining on the declared type alone
collapses it.** A clause naming a collation the dump declares non-deterministic
diverges under equality too, an explicit `C`/`POSIX` clause agrees, an explicit
clause that is not bytewise diverges, and no clause at all falls back to the
type's own default; without the dimension all four are answered by one `text`
case and a fifth could be added with nothing behind it. **Which built-in arms
branch is read from the source too** — an arm whose body calls `collated_text`
is one — so `character` started counting on the day its arm began consulting a
clause, rather than being listed here and going stale.

**One of the four can have no case, and the exemption names where the evidence
is.** No oracle case can reach `collation/non-deterministic`: a
non-deterministic collation is ICU-only (I42), and an ICU case would carry a
`collversion` that moves with the base image — the drift the oracle excludes
ICU to avoid. That reason says why the *oracle* cannot cover the arm and
nothing about whether anything else does, so an exemption stopping there is a
claim nothing checks — the failure this whole join exists to make loud,
arriving inside the join. An exemption therefore carries `Evidence` pointers as
well: `(file, needle)` pairs the check resolves, in the idiom the arm anchors
already use. Today that is three unit tests —
`pgtype.rs`'s `a_collation_the_dump_declares_non_deterministic_diverges_under_equality_too`
and `only_the_non_deterministic_collation_reaches_equality`, and `predicate.rs`'s
`a_non_deterministic_collation_announces_under_equality_and_ordering`. The arm
carries its reason, the report prints reason and pointers under their own
heading, and it is not counted uncovered.

**An exemption goes stale from both sides, and both are reported.** It acquires
an oracle case — an exempt arm that gains one is no longer an arm no evidence
can exist for, and leaving it exempt is how the next arm gets hung off the same
reason. Or the evidence it stands on disappears: a renamed or deleted test
resolves nowhere, which is the state a reason asserting its own sufficiency
cannot reach. The pointers name **sufficient** evidence rather than exhaustive,
so covering the arm a second way — the fixture dumps now carry the shape, where
the same text carries no version — adds evidence and owes no edit here.

*Rejected: an exemption that is a reason and nothing more.* Its first use went
false on the day it was written, citing fixture bytes that do not exist, and
the check could not see it — the same decay as an arm added with no case, in
the one place the check does not look. This is `measure.ACKNOWLEDGED`'s rule
([`measurements.md`](measurements.md), "A commit can be acknowledged") applied
to the register that grew the same shape without the evidence half: an excuse
carries mechanical evidence and goes inert on its own.

*Rejected: moving the arm out of the check's population entirely,* so the
arms-to-cases relation stays total. Something must still say which arms the
oracle can reach, and that rule is the exemption under another name — the same
mechanism with its report suppressed.

`TypeKind::Shell` is the neighbouring shape and not this one: it needs no
exemption at all, being answered by *granularity* — it shares `Base`'s arm,
which `public.mybase` covers (below). This arm cannot be, the other three
collation branches all having cases of their own.

Each branch is also **anchored on a string the parse must find** — for this one
the `states_non_deterministic(` call that implements it — so a branch named in
the arm list and deleted from the source is a reported problem rather than an
arm nothing can reach.

**Three arms, two case groups.** The oracle asks each text pair under `COLLATE
"C"` and under `COLLATE "default"` only, so the non-`C`-clause arm has no group
of its own: it joins to the `default` group, as the no-clause arm does, because
"asked under something that is not `C`" is one population and the database's own
collation is a member of it. *Rejected: asking a third collation by name.*
`datcollate` is `en_US.utf8` at all six majors, so `COLLATE "en_US.utf8"` and
`COLLATE "default"` are the same collation and every added cell would be
byte-identical to one already in the file.

**That mapping is asserted rather than assumed.** It holds only while the
database's collation is not itself bytewise, so the check reads `datcollate` out
of each major's `meta.tsv` and fails on `C` or `POSIX`. Without it, an apparatus
initdb'd under `C` would invert the `default` group's meaning and the join would
go on passing while meaning the opposite thing.

A case is placed by re-walking `comparison_for`'s three steps — array, then
schema-qualified, then the built-in table — and that walk is the only thing the
check restates: it decides which arm a case belongs to, never what the arm
answers. The `TypeKind` a schema-qualified name carries comes from
`scripts/fixture_schema_types.sql`, which is the DDL the oracle's database is
loaded from, so the classification reads the same source the server did.

The evidence half of the second direction is the oracle's own answers: a case
every major answers `E42704` — `undefined_object` — for is a case about a type
no server had. Every other rejection *is* an answer, `json`'s missing operator
(42883) and a malformed literal (22P02) among them.

**One arm needs no case, and the granularity is what says so.** A shell type
(`TypeKind::Shell`) cannot be a column's declared type — the server refuses a
column of a type that never got its I/O functions — so no dump can ask how one
compares and no oracle case can be written for it; it shares `Base`'s arm,
which `public.mybase` covers. Splitting that arm per kind would demand evidence
that cannot exist.

That is only true because `DumpMetadata::types` holds **one entry per type**,
the completion winning over the shell `pg_dump` writes ahead of it (I11, "The
preamble grammar and `DumpMetadata`"). A list carrying both entries would hand
`public.mybase` to the first-match lookup as a `Shell`, putting the case on the
kind that can have none and leaving `Base` unreachable from any real dump —
agreeing with this check on the arm while disagreeing about which side of it
the case sits, which is a trap for whoever splits the arm. `oracle_register.py`'s
`parse_schema` keys its own walk the same way for the same reason.

*Rejected: keying the built-in half on the match arm rather than the name.* It
is the same rule as the user half and it would be wrong here: the four names
sharing `(Utf8View, text)` are exactly the queue the phase closes one at a
time, and one case would excuse the other three.

*Rejected: a Rust test holding the arm list by hand.* The list already exists —
`the_register_answers_every_builtin_scalar` — and its completeness rests on
discipline, which is the thing a reconciliation is for. Reading the `match`
itself is the only mechanical enumeration available.

### The register against the oracle's answers

The two checks above are about which rows *exist* — arms against cases,
majors against each other. `the_register_answers_every_committed_oracle_cell`,
in `predicate.rs`'s unit tests, is the one that compares an answer to an
answer: it walks every cell of `fixtures/<13–18>/oracle/comparisons.tsv` and
puts the same question to the register through `resolve_term` and
`ResolvedTerm::eval`, the path a `--filter` takes. 52,338 cells today.

**It is the check the oracle was built for**, and until it existed the oracle's
answers had only ever been compared by hand. Two register defects had reached
the committed tree behind that gap, and both were found in one throwaway run of
the walk that became this test.

**The one-column schema it puts each case to is built by `resolve_columns`**,
from a synthetic one-table `DumpMetadata` — the same function a real block
resolves through, not a hand-built `ResolvedSchema`. That matters now that `=`
is asserted: the resolution, the nested plan and the comparison plan have to
agree the way they do in a real query, because which of them `resolve_term`
consults is what decides whether an equality term announces a divergence. A
schema claiming every case is `Mapped` and `Scalar` would put `integer[]` and
`box` in one population no dump ever produces.

Three things are skipped, and each is a different fact:

- **An `E<sqlstate>` cell**, which records what the server *refused*. It says
  nothing about how the type compares, and it is why `json` — every one of
  whose cells is `E42883` — is skipped as a server refusal rather than listed
  as a refusal of ours.
- **A NULL right operand**, because SQL NULL has no spelling in the filter
  grammar at all; `IS NULL` is how a filter asks for one. A NULL *left*
  operand is not skipped: it is the field, this build collapses unknown to
  "excluded" for every operator, and the server's `u` is what a `WHERE` clause
  does with the same row.
- **A column the register refuses an ordering operator on**, *for the four
  ordering operators only*: every nested shape, `xml`, an enum with no labels,
  a user-defined base type. That set is asserted exactly, so a type that
  quietly stops comparing fails here rather than passing as one more skip.
  `=` and `<>` are still asked of those columns, because equality is never
  refused — which is how `box`'s area comparison reached the exception set.

**Both sides are put in the type's own `*_out` form**, read from
`literals.tsv`'s `output` column — the field because that is what a dump holds,
and the literal because a filter's value is the canonical spelling by contract.
The oracle's non-canonical `inputs` are an input-grammar question, not a
comparison one, and asking them here would report the filter grammar's
deliberate strictness as an ordering disagreement.

**The exception set is enumerated by pair, and it is met.** Forty
`(type, collation, left, right)` cases are permitted to disagree; every one of
them does disagree in the committed files, and every disagreement is one of
them — so a case that starts agreeing fails as loudly as one that stops.
Alongside it, a disagreeing term must *announce* its divergence through
`ComparisonNote` — **under that operator**, read off the term rather than off
the register's plan — so an exception cannot be claimed for a column the
register tells the user it is confident about. Four populations, and the first
three are **one statement asked at three depths** — a collation the file does
not carry (I32), reached through a column, through an array's element, and
through a string inside a document:

| Population | Why | Cases |
|---|---|---|
| a `jsonb` string leaf | `compareJsonbScalarValue` passes `DEFAULT_COLLATION_OID` to `varstr_cmp`, so a leaf takes the database's collation (I32, I41) | 2 |
| `text` under the database's own collation | glibc `en_US.utf8`: case is lower-weight than letter, an accent sorts with its base letter, and punctuation is ignored at the primary level, so `_x` sorts where `x` does | 30 |
| a `text[]` element under the same collation | the element is ordered by `varstr_cmp` exactly as a `text` column's values are, one level down — the same two disagreements, reached through an element | 6 |
| `box`'s area equality | `box_eq` compares the two rectangles' areas, so the server calls `(1,1),(0,0)` and `(3,3),(2,2)` equal and a byte comparison does not; the column announces `UnmodelledType` and the deficiency is `KD10` | 2 |

The third is the only one this build could close by writing code, and it is the
only one whose cells are `=` rather than the four ordering operators — the
first two are divergences of *order* alone, so their equality cells agree and
would fail the announcement check if they did not.

The first row is the one the `jsonb` cases were grown for: everything
structural above the leaf — the kind order, a container's size, storage order,
the raw-scalar wrapper — is *asserted*, which is what keeps that population two
entries rather than the whole arm.

**`character(10)` used to be a third population and is not one now.** Its eight
tab-bearing cases are the ordering corollary of I38, and they disagreed for as
long as the padding was compared rather than trimmed; the trim retired all
eight at once, under both collations and at all six majors. That they were in
the file *before* the trim landed is what let the change be checked against
evidence it did not produce.

**Met means met everywhere it is permitted.** The key is the case, so a union
over the six majors and the six operators would count an entry satisfied by one
major alone — or by `<` while `>=` agrees, which is a comparator that has
stopped being antisymmetric — and neither is the property the list claims. So
an entry must disagree in **every** cell of its case that its divergence
*reaches*: 24 for the two collation populations, six majors by four ordering
operators, and 6 for `box`, six majors by `=` alone, PostgreSQL defining no
`box <> box`. Counting only the cells where a disagreement is permitted is
what keeps a `text` case from being asked to disagree under `=`, which it must
not. That is one assertion against the cells each
case was walked over rather than a table by major and operator, which was
costed at ~400 rows and rejected — it multiplies exactly the churn the pair
list was kept to avoid.

**`character varying(10)` announces a divergence and appears in no row**, and
that is a fact about its case list rather than about the register: its three
values are `""`, `a` and `hello`, which glibc and `memcmp` order identically,
so the arm's `UnknownCollation` verdict is correct and unexercised. The list is
what disagrees, not what could.

**The pairing is one-directional on purpose.** A disagreement must be
announced; an announcement need not disagree, and no assert asks it to. It
could not: `json` announces `AsText` with every one of its cells `E42883`, and
`character varying(10)` announces `UnknownCollation` over 264 asserted ordering
cells with no disagreement among them — both honest. Over-announcement is caught by
reading the register table in "Ordering operators compare typed" as a table,
not here.

*Rejected: permitting any disagreement on a column that announces a
divergence, in place of the pair list.* Four lines against forty, churn-free,
and it is only what the register already says — but the two text-shaped
populations are asked **twice**. `text` and `character(10)` each get the same
pair set under `COLLATE "C"` and under the database's own collation, and
`collated_text` returns the same bytewise comparison for both, so their
`default`-collation cells assert nothing about this build's comparator that the
`C` twin does not already assert. What those cells carry is *which* pairs glibc
puts in a different order — the pair list itself. Permitting them by
announcement therefore makes 6,432 of the 45,394 cells inert while they keep
running, and the `asserted > 40_000` floor cannot see it, since it counts cells
walked rather than cells that constrain; extending the same rule to `jsonb`
reaches 11,976 and reopens exactly the hole this test was built to close, since
`JsonbStringCollation` is announced unconditionally and is only ever about the
leaves. What it would buy off is one 40-line block re-edited when the fixture
images' collation moves — a reported event, `meta.tsv` committing `datcollate`
and `default_collversion` beside the answers, and one that moves the block as a
block, since all six majors run glibc 2.41 off a single base image.

*Rejected: living in `tests/` beside the other fixture-driven suites.* The
question is what the register answers, which is `resolve_term`'s and
`compare_keys`', and neither is public API. Exporting them to be tested would
widen the surface for the test's convenience.

### The ADBC floor oracle

**The floor is somebody else's shipped driver, so it is taken rather than
argued.** `fixtures/<13–18>/adbc/floor.tsv` records what the Arrow ADBC
PostgreSQL driver returns for every `pg_catalog` type a user could declare a
column of, one file per routine major, generated by
`scripts/generate_fixtures.py` and committed. `scripts/adbc_floor.py` is the
sweep and the file format; the generator owns only the container, exactly as it
does for the comparison oracle.

What the floor *is* — the rule, which rows sit outside it and with what stance
— is ["The floor: the ADBC driver's answer bounds
ours"](#the-floor-the-adbc-drivers-answer-bounds-ours), beside the mapping
decisions it constrains; `scripts/floor_mapping.py` is the join that reads this
file against them. This section is the apparatus.

**One row per declarable type, chosen by catalog sweep rather than by a
list.** `typtype` in `b`, `e`, `r`, `m`, `d`, in `pg_catalog`, `typisdefined`,
and the array types left out. Committing the sweep's output is what makes a new
major's new type a diff somebody reads; a curated list can never produce that
signal, because nothing prompts anyone to extend it.

**"An array type" is a back-reference, not a shape test.** The array types are
excluded because our own resolution reaches them by recursing from the element
type, so a row for `integer[]` would be evidence about a mechanism this file
does not measure. The exclusion is therefore *"some other type names this as
its `typarray`"*.

*Rejected: testing the shape instead* (`typelem <> 0 AND typlen = -1`). Over
this sweep's own row set the two predicates differ on exactly two types —
`int2vector` and `oidvector`, both varlena with a non-zero `typelem` and
neither named as any type's `typarray` — and both are declarable in their own
right, with no array recursion reaching them. `int2vector` is one of the rows
the floor is meant to close, so the shape test deletes it.

*Rejected: testing the declared spelling* (`format_type(oid, NULL)` ending in
`[]`), which is what the exclusion's reason literally names, the resolution's
array recursion being a spelling test over the declared name (I28) rather than
a catalog lookup. It is coextensive with the back-reference over this row set,
so it buys nothing; and it would make `format_type` both the filter and the
join key, coupling which rows exist to how the join key is spelled.

**Taken from the host, over a published port.** The driver is a pip wheel
pinned in `scripts/pyproject.toml`; installing it into a `postgres:` image
would be a second build system. So `generate_fixtures.py` publishes the fixture
container on `127.0.0.1:55432` and the sweep connects as an ordinary client.
The sweep needs no fixture DDL — it is a `pg_catalog` question — so it runs
against the `postgres` database and owes no `create_fixture_db`.

**A probe is a cast.** The comparison oracle asks its pairs through declared
columns because a column carries a collation and a cast does not; nothing here
reads a collation, the driver mapping a result field by its type OID alone. So
`SELECT NULL::<type>` is both the cheap probe and the honest one.

Nine columns, and two of them are apparatus repeated on every row:

| Column | What it holds |
|---|---|
| `driver` | the `adbc_driver_postgresql` version the row was taken with |
| `server` | the PostgreSQL major |
| `declared` | `format_type(oid, NULL)` — how `pg_dump` spells the type |
| `typname` | the catalog name, which is what an `arrow.opaque` reports |
| `oid` | the catalog OID, which is what an *unnamed* opaque reports |
| `typtype` | `b`/`e`/`r`/`m`/`d` |
| `status` | `ok`, or `E<sqlstate>` where the driver could not read the type |
| `extension` | the Arrow extension name (`arrow.opaque`, `arrow.json`), else `\N` |
| `arrow` | the storage type where `extension` is set, the field's own type otherwise |

`declared` is the join key, and it is `format_type(oid, NULL)` — the same
function `pg_dump` writes a column's type with, at the typmod every arm here is
keyed on. A `pg_catalog` type comes back bare and in its standard SQL spelling
(I8), so a floor row resolves against a `builtin_scalar` arm by string
equality, the way an oracle case already does.

*Rejected: the apparatus in a sibling `meta.tsv`, as the comparison oracle files
it.* The driver version is the axis this whole file is a claim about, so a row
lifted out of it by a grep or a diff has to still say which release it speaks
for; and the reconciliation reads the version off the rows it is already
joining instead of off a second file.

**`status` is `ok` or `E<sqlstate>`, and two rows are refusals on every
major.** `aclitem` and `gtsvector` answer `E42883` — the server has no binary
output function for them, and binary is the only encoding the driver reads.
That is a floor of *nothing*, recorded rather than omitted, because a type
missing from the file cannot be told from a type the sweep never asked about.

**The file's order is the sweep's, not the server's.** Rows are sorted by
`declared` in code-point order in Python, so a regeneration diff is the type
set moving and never the database collation.

`scripts/test_adbc_floor.py` checks the committed files without a container or
a driver: one driver version across all six majors, each row naming its own
major, the order and uniqueness above, a cell answered or refused but never
both, and that the type set is **additive** across 13→18 — the six multirange
types and the two BRIN summary types arrive at 14 and nothing goes. A major
that removed a declarable type is a thing to read rather than to re-baseline,
since every stance resting on that row has just stopped being about anything.

**A floor pass ends by running the reconciliation**, the way an oracle pass ends
by running the differ and the register join, and for the same reason: a driver
release that answers a type differently has to be met with a mapping or a stance
at the moment it is taken. `scripts/test_floor_mapping.py` is that check's own
suite — the Rust parse against synthetic sources, the verdict against synthetic
rows, and the whole join against the committed tree.

## Testing philosophy

**The primary correctness tool is a round trip with no hand-transcribed
expected values**, because a test cannot encode the same misreading twice. Two
independent round trips, each owned by the layer it tests:

- `tests/decode.rs` queries the same real fixture in both `SchemaMode::Typed`
  and `SchemaMode::Strings` and asserts every field renders identically.
  `Strings` mode is byte-for-byte the untyped path, covered by its own suite,
  so it is a trustworthy oracle for "did `decode_*`/`render_*` stay exact
  inverses" — entirely in decoded-text terms, never touching on-disk escaped
  bytes.
- `tests/scan.rs::copy_text_escaping_round_trips_through_postgres` requires
  `encode_field(decode_field(raw))` to equal the literal on-disk bytes of every
  field of `public.escapes`, across every routine version — the
  escaping/unescaping leg the other round trip deliberately never exercises.

**One fixture column is deliberately outside the round-trip suite**, because
there a round trip asserts nothing. `t_delimiter.v_box_domain_array` holds two
`box` values, which `array_out` separates with `;` (I22); `decode_array` splits
on the hardcoded `,` into **seven** elements, none of which contains a
quote-forcing character, so `render_array` puts them back byte-for-byte
identical — wrong element boundaries, an exact round trip, and no error
anywhere. `tests/nested.rs` leaves the column out of `NESTED_COLUMNS` and pins
that property directly instead
(`the_delimiter_trap_round_trips_while_splitting_on_the_wrong_character`), so
the fixture is not later mistaken for codec coverage. It is the one place where
the primary correctness tool is blind, and the refusal in [type
resolution](#type-resolution) is what stands in for it.

Boundary values a fixture round trip cannot reach on its own (`NaN`,
`±Infinity`, `infinity`/`-infinity` dates and timestamps, year 0001/9999,
38-vs-39-digit numeric, negative-scale numeric) are hand-written unit tests in
`decode.rs`, pinned from the type's documented semantics.
`t_numeric`/`t_date`/`t_timestamp`'s boundary rows are genuine `FieldDecode`s
by design, so they get tests asserting the error names the right
table/column/declared-type/value, plus a check that `Strings` still shows the
raw text with no error.

**Untyped tests stay pinned to `SchemaMode::Strings`** — they prove
scanner/batch-assembly properties unrelated to types, and new typed coverage is
added alongside rather than by retyping their expectations. The two
`public.escapes` tests are the deliberate exception, run in **both** modes
since `text` maps to `Utf8View` either way and the results must match. The
column-rendering helper (`rows_of`) goes through the public `render_field`
rather than a hardcoded `StringViewArray` downcast.

**The fixture vocabulary is one module per test crate** —
`pgdump_query/tests/common/mod.rs` and `pgdump_query-cli/tests/common/mod.rs`,
holding the fixture paths, the tempdir-sandboxing helpers and the shared
expectations (`rows_of`, `widgets_expected`). Each `tests/*.rs` file is its own
crate, so without them every file carries its own copy of each path helper, and
copies drift silently because nothing compares them. What *drives* the library
— a `collect`, a `drain`, a `census_of` — stays in the file whose subject it
is: those differ per file in ways that matter, and pooling them would rebuild
the same problem one level up.

Chunk-boundary correctness is asserted, not assumed: the event stream is
identical across chunk sizes 1…4096, and the batch tests exercise the
zero-copy, straddling, and decode-copy paths against the same expected output.

The hand-written `tests/data/edge_cases.sql` is deliberately timestamp-free so
its snapshots stay stable; the generated `fixtures/` tree gets structural
assertions instead, since its timestamps change on regeneration.

**Tiling is checked three ways**, and the runtime check is not a substitute for
the test:

- `tests/map.rs::every_fixture_tiles_exactly` walks every file under
  `fixtures/` plus `tests/data/edge_cases.sql`, including every degenerate
  shape: `--data-only` (no DDL), `--schema-only` (no data),
  `--inserts`/`--column-inserts` (zero `COPY` blocks), and the concatenated
  multi-`\connect` `dumpall.sql`. Fixture discovery is directory-driven, so a
  new schema is pulled in without touching the test.
- `build_index_spans_match_build_map_exactly` pins the two producers to
  byte-identical spans, so they cannot drift now that they share `Builder`.
- `check_tiling` at runtime, reporting through the diagnostic channel, so a
  hole on a dump shape no fixture covers surfaces as a high-severity diagnostic
  on a map that stays usable.

**The CLI has its own integration suite**,
`pgdump_query-cli/tests/partial_reporting.rs`, driving the real binary through
`env!("CARGO_BIN_EXE_pgdq")`. It is the only
place an exit status or an error sentence can be asserted, which is most of
what a verb split changes, and it is where the partial-cache reporting is
pinned: its truncated caches are **built by hand** from a complete index (per
the clamp rule under "A span's `end` is fixed up at push time"), so the
assertions do not depend on where a real interruption happened to land.
`query_projection.rs` beside it is the other half a library test cannot reach:
the flags' exit statuses, and the *rendered* stream — a zero-column query's
missing header line above all, since a stray line there breaks the row-count
idiom silently. It also runs `measure.py`'s own registered projection widths
against a generated input, so a command shape the harness would only execute
mid-sweep is executed by the suite instead — and the flags are *read out of*
the harness rather than transcribed, so a width added there is exercised here
without anyone remembering to. The failure that guards against has no other
guard: a flag the binary does not accept is otherwise discovered minutes into
a figure, with the figure lost. `query_filter.rs` does the same
for the conjunction: what a repeated flag *accumulates* is a property of the
parser, so the test that says a second `--filter` neither replaces nor is
ignored has to count rows out of the real binary, with each term asserted
alone in the same test as the pair. `query_where.rs` is its sibling for the
expression grammar, and the assertion only the binary can make is the one
about the *pair* of flags: that the identical string means a refused
conjunction under `--where` and an equality under `--filter`, which is the
whole reason there are two flags and cannot be seen from either alone.
`pgdump_query/tests/map_file.rs` separately covers that a real interruption
leaves that same shape.

**`xz_source.rs` is the compressed source's end-to-end half**, and the only
place `.xz` reaches the real binary; the library-level parity lives beside it in
`pgdump_query/tests/cache.rs`. Its inputs are **compressed at test time and not
committed** — `xz` shelled out over the same 2,352-byte
`tests/data/edge_cases.sql` — because regenerating is cheaper than carrying a
binary blob in git, and `xz` is not `mise`-pinned, so the helpers assert it is
runnable rather than skipping (`roadmap.md`, "A test may assume the tools `mise`
pins"). What is asserted is **differential parity, not a transcript**: `parse`
then `info --json` against the plain file and each `.xz` shape, the parsed JSON
documents compared whole, with only the non-seekable file's extra diagnostic
lifted out before the comparison and its *rendered* wording checked separately
for naming both remedies; then `query`, byte-identical on stdout across all
three sources. Both fixtures are `parse`d before the query legs run, so those
reads go through a persisted cache and exercise the round trip of the
compression index rather than only the live decoder.

**A block size is asserted, never assumed.** A helper compressing with
`--block-size=65536` claimed to force several blocks and did not — `xz` never
splits an input smaller than one block, so every caller was silently exercising
the *non-seekable* shape under the label "seekable", and nothing could see it
because the two shapes produce identical indexes. The non-seekable diagnostic is
what finally gave that claim something to disagree with. The helpers now use 512
and assert `is_seekable()` at each call site, which is the general form: a
fixture whose *shape* is the thing under test states the shape as an assertion
rather than in a comment. Row counts are asserted the same way for the same
reason — through `query`'s own `N row(s)` line rather than by counting stdout
lines, since a value in that fixture carries an embedded newline and a line
count overcounts.

**An interrupt is delivered at a file offset, not at a wall-clock moment.**
`map_file.rs`'s `CancelsPast` is a `ByteRangeSource` that trips the cancel flag
once a read asks for a byte at or past a chosen offset, which makes "stopped
inside the second database's data" a deterministic test rather than a race; its
sibling `FailsPast` returns an `Err` instead, simulating a death mid-scan. The
**signal** path itself has no automated test: a fixture parses in
milliseconds, so anything racing a signal against it is a coin flip. It is
verified by hand against a 4000-block bench file (`SIGTERM` → 143, `SIGINT` →
130, each leaving a cache that resumes to a byte-identical result) and at real
scale on koji (`measurements.md`, "koji full scan").

Benchmarks are a **regression tripwire**, not an optimization campaign:
`benches/decoders.rs` (one `decode`/`render` pair per mapped type family, plus
a `nested` group carrying two controls of its own — a same-byte-count
`String::from` and one `append_view_unchecked` — since a nested value's cost is
only readable against the copy it cannot avoid and the view it does not get)
and `benches/whole_file.rs` (one warm-cache end-to-end `Typed` scan, since
`Strings` would not exercise the decoders at all). `text`/`varchar`/`char`
(zero-copy) and `enum` (unparsed dictionary-key append) have no decode step to
benchmark. Figures and their re-run commands: [`measurements.md`](measurements.md).
