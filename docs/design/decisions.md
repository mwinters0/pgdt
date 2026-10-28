# Decisions

The decisions the code cannot explain: shapes kept for a deliverable not yet built, defaults chosen over an alternative,
and obvious changes measured or argued and refused. Nothing here says how the code works (the named module does) or
quotes a number (`measurements.md` does, by figure id, as the invariant registers do by `I<n>`/`RT<n>`). Each entry lives
in its topic's file under `decisions/`, which opens with what the topic is and is read on its own; this file is the
index and the citation target: cite as `docs/design/decisions.md`, "D12". The rules, the caps and the meter included,
are `docs/process.md`, "The decision register".

<!-- decision-watermark: D126 -->

## I/O, memory and parallelism
[`decisions/io.md`](decisions/io.md)
- **D1** The library never spawns threads by surprise
- **D2** A plain file recommends one worker; a local compressed one the machine's cores
- **D3** The memory constants, and what each one is
- **D83** `--memory` states what the process may hold; one carving serves stated and discovered
- **D4** A budget is solved against a source's cost, never divided by it
- **D5** A wait is a permission the read loop grants
- **D6** `ByteRangeSource` mirrors `object_store` and is dyn-compatible
- **D7** `partitions()` says where and at what cost, never whether
- **D8** The cut is one unit wide, sized apart from the charge
- **D10** mmap, `fadvise` and double-buffered readahead are refused
- **D11** Limit discovery takes a root; the seam is `internal`'s
- **D12** Workers are `spawn_blocking`; no runtime flavour is imposed
- **D13** The allocator is the binary's choice; the one binary composes DataFusion's CLI and links mimalloc

## The compressed source and the cache
[`decisions/compressed.md`](decisions/compressed.md)
- **D14** Recognition sniffs content; `xz-seek` is a vendored read-only copy
- **D15** A read decodes whole blocks and retains them; reading one in pieces is the fallback
- **D18** The seek table is persisted and identity is three-state
- **D19** A budget decline is a `PlanNote`; a non-seekable file is warned, not refused
- **D112** `gz-seek` inflates through zlib-rs, by `libz-rs-sys`'s zlib C API
- **D113** gzip's checkpoint spacing is the block `xz -T0` writes by default
- **D114** A gzip cache's windows follow its encoded file, and every save writes them all
- **D115** A gzip first pass spills its windows to a file with no name, beside the cache or in the temp directory
- **D116** A forward pass coalesces BGZF members as the walk does, so a fetched BGZF file is read by a first pass
- **D117** A parallel first pass speculates with a decoder of `gz-seek`'s own, and hands out only the engine's plaintext
- **D118** A checkpoint inside a static block is placed by walking its codes beside the engine
- **D120** A parallel first pass gives decodes a room of their own, and places its speculations a lead ahead
- **D121** A parallel first pass cuts its file at the input chunk every gzip decode pulls
- **D122** A speculation's guess is cut at a bound on its events and on its bytes past its chunk, and continued
- **D123** A taken chunk's decode records a BGZF member as its start, header and end, three events a chain read counts
- **D124** A gzip first pass charges a task what `gz-seek` bounds it to write, and frees no decode buffer while it runs
- **D125** A parallel first pass hands out no speculation from when one finds nothing until its frontier crosses a block start
- **D126** A local gzip first pass keeps its frontier on one blocking thread from read to read
- **D20** The library never replaces cache data automatically
- **D21** Identity is `stored_size()` plus weak signals, in an opaque enum
- **D87** A remote cache is named from the URL, and the origin it records is advisory
- **D90** The DataFusion provider reads only a complete cache, never maps
- **D22** `Held` is its own type and `CACHE_FORMAT_VERSION` is bumped freely

## The scanner
[`decisions/scanner.md`](decisions/scanner.md)
- **D23** The scanner never owns the bytes it scans, and only the whole `COPY` grammar is structural
- **D25** Parallelize what is CPU-bound
- **D26** Cancellation is `map_file`'s argument, polled per chunk or leader window by the mapping passes alone
- **D27** UTF-8 is validated once per chunk; `unsafe` is denied everywhere and allowed at the site that needs it
- **D28** One row split, shared unconditionally
- **D29** Refused micro-optimizations on the row path

## The file map and the preamble
[`decisions/map.md`](decisions/map.md)
- **D30** A span is classified by the grammar; the TOC anchors and enriches, never required
- **D31** `Span::toc` is "belongs to", vetoed after classification; boundary rules that read as bugs
- **D33** Bulk regions are one span kind; large objects skip at the scanner, `INSERT` runs at the map
- **D34** `DumpIndex` stores no fact twice but sortedness, `metadata` and `scanned_through`; whole-file facts need `is_complete`
- **D35** The census is type-blind, records both dimension bounds, and runs at the data level
- **D96** The unrepresentable count rides the census, typed by the DDL, in two tiers, under a recorded calendar
- **D36** The preamble grammar dispatches on fixed keywords and never guesses

## Type resolution and decoders
[`decisions/types.md`](decisions/types.md)
- **D37** The bar: the dump alone determines the value
- **D38** The ADBC driver's shipped release is a floor, swept from the catalog
- **D40** The comparison is decided per declared type, in the same arm
- **D43** `NestedPlan` travels beside the `DataType`; the census speaks after the DDL and moves the pair
- **D45** Strict decoders apart from permissive parsers; decode and render allocate only what they return

## Batches, streams and the leader
[`decisions/batches.md`](decisions/batches.md)
- **D46** Zero-copy views are top-level `Utf8View` only, and `max_source_span` alone bounds pinned bytes
- **D84** The batch span is derived from the budget and the count, and spent before the count is cut
- **D48** Mapping and replay are separate passes, and `splice` owns the seam
- **D49** One target per query, the early stop conservative, and `ResumeToken` opaque, fingerprinting the query
- **D51** A segment's offsets are search bounds, and a resync is a real read
- **D52** The worker is fused, and the earliest failing piece is the error
- **D93** A dynamic filter's rows are evaluated only under `pgdump.dynamic_filter_rows`, off by default; its state is read at each group entered and each chunk
- **D95** A dynamic filter enters by `TablePartitions::under` alone, and the byte cut is made once per handle, at the first poll
- **D98** The typed mode nulls what its front end cannot hold, tested before decode where the block's count says it holds one
- **D99** The refuse mode refuses at planning, per materialized column, from the map's count over every block
- **D100** The untyped mode widens a column per table from the map's count, and compares it in each semantics
- **D103** The value modes are one option each for every column, fixed where a dump is opened, apart from the schema mode
- **D119** A projection is the request's names, each once, in its order; the CLI repeats `--column`

## Predicates
[`decisions/predicates.md`](decisions/predicates.md)
- **D53** The operator set is closed but for membership and the unrepresentable test
- **D54** One tree, no planner yet, short-circuit defined against the root
- **D55** A literal is read in the type's `*_out` form at least and its `*_in` grammar at most
- **D56** Special values are a rank in the key; equality has three canonicalizations, by injectivity of `*_out`
- **D59** Divergence is per position on its own channel: per term, and per column at registration
- **D60** `--where` is a second flag and the tokenizer defines the refusal set
- **D88** A pushed filter is `Exact` where the plan resolves it; a literal is the renderer's text
- **D101** The unrepresentable test reads the text against the declared type; DataFusion's is a scan's alone
- **D94** A dynamic filter is translated loosened into the library's tree, never read through `PruningPredicate`

## Statistics
[`decisions/statistics.md`](decisions/statistics.md)
- **D75** Pruning takes only what each operator family proves
- **D76** Gathering never keys a bytewise value; a bound past the cap is a prefix and a successor
- **D77** The statistics request is the mapping pass's argument, and the metadata level is a selection
- **D78** Statistics share the cache file, its identity and its `CACHE_FORMAT_VERSION`
- **D79** Bounds go in each order a semantics compares by exactly, a dictionary where it equates exactly
- **D81** The statistics account sums allocation sizes as they change; an instrument scope checks it
- **D82** A block merges pairwise, exactly, between its minimum and its maximum, never mid-scan while a piece lives
- **D85** Statistics are billed against the margin, and a block that cannot fit declines
- **D89** Statistics reach DataFusion on a leaf plan node, and `Exact` means the bound is the value
- **D91** A sum is kept wrapped and handed over as `SUM` wraps; a byte size is the text's length
- **D97** A group keeps a view per tier only where it holds such a value, the values its type holds the base

## The CLI
[`decisions/cli.md`](decisions/cli.md)
- **D61** `info` never scans
- **D62** The save throttle is a ratio, and its gate opens splice too
- **D63** The interrupt flag is read at both extremes; resume is the default
- **D64** An omitted `--jobs` asks the source; provenance is the CLI's fact
- **D66** Output of what `pg_dump` writes is byte-identical whether typing is on or off, but for a value its type cannot hold or an `escape` `bytea`
- **D67** `--json` is the internal struct; a flag's help is its doc comment

## Layering
[`decisions/layering.md`](decisions/layering.md)
- **D68** Seven crates, the module assignment Cargo enforces
- **D74** Below the codec no Arrow, the codec pure, and the core's error naming no dependency
- **D106** A crate's API is a designed surface, and what a boundary forces public that is not API is `internal`'s
- **D107** A library refusal names its remedy in the library's terms, and each embedder spells it after the sentence

## Fixtures and tests (`scripts/`, `fixtures/`)
[`decisions/fixtures.md`](decisions/fixtures.md)
- **D69** Fixtures are real `pg_dump` output on a pinned glibc image family
- **D71** Agreement with PostgreSQL and `pg_dump`'s forms are generated checks, read out of their producers and joined by existence
- **D73** Round trips, asserted shapes, and a hand-verified signal path
- **D102** The unrepresentable category is read off hand-written extremes through DataFusion's own path, never listed
- **D108** A hot-path change is priced by `--count-pair`, and a rise it resolves is placed by callgrind, then reworked or accepted

## Releases (`release/`, `scripts/release.py`, `scripts/release_ci.py`, `scripts/debian_window.py`)
[`decisions/releases.md`](decisions/releases.md)
- **D104** A release is `pgdt` for Linux, linked against one digest-pinned Debian-stable image that also builds what the register times
- **D105** The shipped build is fat LTO in one codegen unit, the leg retiring the fewest instructions on the typed query
- **D109** arm64's archive is built on the maintainer's machine and attested by its release alone; x86-64's is CI's, with build provenance
- **D110** The release suite is the round's, less the never-shipped measurement apparatus
- **D111** CMake is a `mise.toml` pin for the apparatus, and a build is refused where a crate `pgdt` depends on ran it
