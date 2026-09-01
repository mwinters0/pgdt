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
| `scan.rs`, `copy.rs`, a new `Event` variant | [Bytes and structure](#bytes-and-structure) |
| `map.rs`, spans, tiling, TOC headers, `INSERT`/large-object regions | [The file map](#the-file-map) |
| `index.rs`, what the index owns, diagnostics | [`DumpIndex`: one owner per fact](#dumpindex-one-owner-per-fact) |
| array dimensionality, `ArrayShape`, what a scan records per row and what retypes a column from it | [The array shape census](#the-array-shape-census) |
| `preamble.rs`, the DDL grammar, `\connect` handling | [The preamble grammar and `DumpMetadata`](#the-preamble-grammar-and-dumpmetadata) |
| `pgtype.rs`, `resolve.rs`, the type mapping table | [Type resolution](#type-resolution) |
| `decode.rs`, a new type's decode/render pair | [Decoders and render-back](#decoders-and-render-back) |
| `batch.rs`, the zero-copy `Utf8View` path, a batch's flush triggers | [Arrow assembly and the zero-copy path](#arrow-assembly-and-the-zero-copy-path) |
| `stream.rs`, `map_forward`/`map_file`, replay, resume, projection, predicates, the `--filter` term grammar | [Query: mapping and streaming are separate passes](#query-mapping-and-streaming-are-separate-passes) |
| `cache.rs`, the format version, cache modes | [The cache](#the-cache) |
| the CLI's flags or output, the save throttle, the interrupt guard | [CLI surface](#cli-surface) |
| `scripts/`, a new fixture schema | [Fixtures](#fixtures) |
| what PostgreSQL answers for a comparison, a new comparison case | [The comparison oracle](#the-comparison-oracle) |
| whether two majors disagree, the committed differences file | [The cross-major differ](#the-cross-major-differ) |
| a comparison-register arm, whether the oracle still covers one | [The register-to-oracle reconciliation](#the-register-to-oracle-reconciliation) |
| a comparison's *answer*, or the set of cases it is knowingly wrong on | [The register against the oracle's answers](#the-register-against-the-oracles-answers) |
| adding or changing a test | [Testing philosophy](#testing-philosophy) |

## Module map

| Concern | Where | Layer |
|---|---|---|
| Byte-range I/O trait + local-file impl | `pgdump_query/src/io.rs` | L1 |
| `COPY` block and large-object structure discovery | `pgdump_query/src/scan.rs` | L1 |
| COPY TEXT field splitting / escaping / unescaping | `pgdump_query/src/copy.rs` | L1 |
| `Span`/`SpanBody`/`DataBlock`/`TocHeader`, the boundary+classification state machine (`Builder`), `check_tiling`, `attach_text` | `pgdump_query/src/map.rs` | L1 |
| DDL statement grammar: `classify_statement`, `statement_complete`, `in_open_quote`, `extract_statement_cross_refs`, `dump_metadata_from_spans`; `DumpMetadata` and friends | `pgdump_query/src/preamble.rs` | L1 |
| `DumpIndex`, `CopyBlock`, `build_index`/`scan_preamble`/`preamble_only` | `pgdump_query/src/index.rs` | L1 |
| Structure cache (envelope, `CacheMode`, `CacheStatus`, source identity) | `pgdump_query/src/cache.rs` | L1 |
| `Diagnostic`/`DiagnosticKind`/`Severity` — the file-level channel | `pgdump_query/src/diagnostic.rs` | L1 |
| Declared-type string → Arrow `DataType`; domain/enum/range/multirange resolution; `NestedPlan`; the comparison register (`comparison_for`, `ComparisonPlan`) | `pgdump_query/src/pgtype.rs` | L2 |
| `ResolvedSchema`/`ColumnResolution`/`ColumnNote` — joins a `COPY` header against `DumpMetadata`, and carries each column's comparison plan | `pgdump_query/src/resolve.rs` | L2 |
| Per-type field decode + render-back | `pgdump_query/src/decode.rs` | L2 |
| Array / record / range / multirange literal decode + render-back | `pgdump_query/src/nested.rs` | L2 |
| Arrow batch assembly (`ColumnBuilder`, `RowBatcher`), push-mode `read_table` | `pgdump_query/src/batch.rs` | L3 |
| Pull-mode `table_stream`, `map_forward`, `map_file` (`pgdq parse`'s scan), replay, `ResumeToken`, `ScanExtent` | `pgdump_query/src/stream.rs` | L4 |
| Post-parse predicate | `pgdump_query/src/predicate.rs` | L4 |
| CLI (`pgdq parse` / `info` / `query`) | `pgdump_query-cli/src/main.rs` | above L4 |

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
trait ByteRangeSource {
    async fn read_range(&self, offset: u64, len: usize) -> Result<Bytes>;
    async fn size(&self) -> Result<u64>;
    async fn modified(&self) -> Result<Option<SystemTime>>;
}
```

`read_range`/`size` are shaped to match `object_store`'s `get_range`/`head`
semantics **on purpose**, so an `object_store`-backed implementation is a
drop-in addition rather than a redesign. There is exactly one implementation
today: a local-file backend (`std::fs::File::read_at` wrapped in
`spawn_blocking`, since `tokio` has no native async positioned-read).

*Rejected:* depending on the `object_store` crate now. It pulls in a cloud-SDK
dependency tree that buys nothing for "read one local file". The trait shape is
what keeps the addition additive; it arrives behind a default-off Cargo feature.

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

## Bytes and structure

### The scanner never owns the bytes it scans

`CopyScanner` is a synchronous state machine: the caller holds the buffer,
passes a slice, and the scanner reports how much it consumed. That is what
keeps events zero-copy and lets one state machine back both the async driver
(`scan()`) and the pull-mode `Stream` without a second parser.

The consequence is that the caller's buffer is the only thing bounding memory,
which is why `ScanOptions::max_line_bytes` exists and why exceeding it is a
hard error rather than a truncation.

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
- Input is assumed to be **already-decompressed plain SQL text**. The library
  does not handle `.gz`/`.xz`/etc. itself; that is caller-side preprocessing.
  This is a rule about *plain-format input specifically*, not a global policy —
  archive formats compress per entry, internally, and will need streaming
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

1. `preamble::statement_complete` — the accumulator, which tracks
   double-quoted identifiers (`""` doubling) and `--` line comments. Without
   that, an apostrophe inside either opens a string that never closes.
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

An `INSERT` run instead reuses the statement accumulator and simply stops
pushing a span per statement: `Mode::InsertRun`, entered from `Mode::Statement`'s
first line via `parse_insert_target`, folding completed statements into one
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

**Every line is still decoded into `Event::Line`**, and that costs
**mid-teens times** a `COPY` scan per byte, warm — 16.7× in the sweep this doc
is stamped against, 14.4–16.7× over the sweeps taken under a witnessed-quiet
apparatus; see [`measurements.md`](measurements.md), "Scan
throughput by input shape". Correctness, tiling and row counts are unaffected;
what it costs is throughput on `--inserts` input, ~45 minutes of CPU for a 1 TB
dump against the `COPY` path's ~3. A scanner-level `INSERT` path is the fix and
is filed in
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md).

*Rejected:* re-prioritising that fix on the strength of the ratio alone. The
number is three times what the entry was filed under, which is exactly the kind
of change that argues for moving work forward — and where it sits belongs to
the scan-performance phase's grilling, against that phase's other candidates,
not to whichever fold-in happened to correct the figure.

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
(`Severity::Warning`, pushed by `load_offline` on every successful load).

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
brace-free data — the koji shape — a row pays the pre-filter alone, 48 ns per
16-column row, +8% of a scan reading from memory; a row that passes pays field
splitting and `observe` on top, 1.61 µs over 19 columns, +225% warm. Both
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
receives every row of every block it maps — which is 48 ns a row with the bytes
in memory and vanishes behind the device a cold query reads from. What it cost
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
`ColumnDef` recover server and `pg_dump` versions, extensions, user-defined
types, and each column's declared type **as the literal string `pg_dump` wrote**
— never a parsed pair — per the layering rule that L1 stores what the dump said,
not what a later layer concludes.

**Grammar approach: dispatch directly off five fixed line-start keywords**
(`CREATE TABLE`, `CREATE TYPE`, `CREATE DOMAIN`, `CREATE EXTENSION`, `ALTER
TYPE`).

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
`Struct<…, List<Utf8View>>`. A leaf whose own type does not map — `interval`,
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
| `interval` | `Utf8View` | Two values a dump can hold have no `Interval(MonthDayNano)`: v17's `infinity`/`-infinity` (I34), and any interval whose time part exceeds `2562047:47:16.854775807` — PostgreSQL's field is `int64` *microseconds* against Arrow's `int64` nanoseconds, and nothing normalizes hours into days (I40). Neither is a format limit: `pg_dump` pins `INTERVALSTYLE = POSTGRES` (I4), so the text is determined. See below |
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

***`interval` is `Utf8View` because two of its values have no Arrow encoding,
and the deciding evidence is per column rather than per type.*** Arrow's
`Interval(MonthDayNano)` carries months, days and a time part as three
independent fields, which is exactly PostgreSQL's `Interval`, so the shape
matches. The value space does not: an infinite interval sets every field
extremal, and the time part is microseconds against Arrow's nanoseconds, a
thousandth of the range, reachable by an ordinary unnormalized value like
`interval '100000000 hours'`. The ADBC driver takes the mapping anyway and
pays with `EINVAL` on the whole batch — its reader guards the multiply against
`kMaxSafeMicrosToNanos` and errors, and because an infinite interval's time
field is `INT64_MAX` it errors there too, by a message about nanosecond
overflow rather than about infinity.

*Rejected: mapping it and making the two a `FieldDecode`, as `numeric(p,s)`
does for `NaN`.* It would behave better than the reference implementation, one
value lost rather than a batch. But a typmod is a promise the DDL makes about
the range, which is what makes the `Decimal128` row's residue rare by
construction; `interval` has no typmod, so nothing bounds how often the
fallback fires, and that is the same argument that puts bare `numeric` in this
column.

**What would settle it is a census, and the statistics work is where one gets
taken.** Whether a *given* column needs the escape hatch is answerable from its
values — decode to `Interval(MonthDayNano)` where every value is finite and
under the ceiling, and keep the column in text where one is not. That is the
array-shape census's own shape, and it makes the Arrow type data-dependent in
exactly the way that census already does, `List<T>` against `List<List<T>>`
being no smaller a difference. It is filed at
[`roadmap-P10-row-group-statistics-inbox.md`](roadmap-P10-row-group-statistics-inbox.md)
rather than built here, because a pass that already reads every value to
summarize it gets the answer for nothing, and one taken for this alone does
not.

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
type. It closes the same hole and pays by degrading `interval[]`, `money[]` and
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

<!-- deficiency: KD8 -->
**No typed column can hold `infinity`, `-infinity` or `NaN`** — deficiency
`KD8`, and unowned. `Date32` has no infinity and `Decimal128` no NaN, so a field holding
one is an `Error::FieldDecode` and there is no typed way to read the value.
The file is not at fault: `pg_dump` emits these from any healthy database
(I34). `--schema-mode strings` returns the literal verbatim. *Comparing* one is
a separate question and is already answered — see "Ordering operators compare
typed" below, which is why a filter may legitimately select a row the output
column then cannot represent. What stays open is **materialization**, where the
choices are a null, a sentinel indistinguishable from a real date, or the
error; it belongs to whichever phase owns typed materialization.

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

Three properties are load-bearing and easy to lose:

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

`tests/nested.rs` is the conformance test: every nested column of every
`types` fixture, on all six majors, read in `SchemaMode::Strings` and required
to round-trip. A force-quote predicate transcribed even slightly wrong from
`arrayfuncs.c`/`rowtypes.c`/`rangetypes.c` fails there against values
PostgreSQL itself wrote.

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

`render_field(column, row, plan)` is the single entry point: there is
deliberately **no plan-less sibling**, because one that panicked on a nested
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
edges at every level of nesting; that is a scan-performance change P7 owns
and should measure first (`roadmap-P7-scan-performance-inbox.md`).

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
   `index.scanned_through`, drives a `map::Builder`, and after every `CopyEnd`
   splices `snapshot` onto the base spans, advances `scanned_through`, merges
   the builder's `roles()`/`tablespaces()`, and persists. At a `CopyStart`
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
the projection, the filter terms and the schema mode, hashed — and resuming a stream
whose options hash differently is `Error::ResumeQueryMismatch`. That defends
"one schema per stream, resolved up front", which nothing else defends: the
token never named even the table, so resuming against a different one was
silently accepted, and a projection makes changing the schema without changing
the table an ordinary thing to do rather than an exotic one. The hash is over
an explicit `match` per field rather than a derived `Hash`, so a new operator
or option is a compile error instead of a stamp that quietly stops covering it.
The filter list is hashed arity-first and in order, so two conjunctions
differing only in term order fingerprint differently — a `ResumeQueryMismatch`
on a resume nobody would write, against a canonicalization rule the stamp
would otherwise have to own. `database` and `scan_extent` are deliberately
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
decides what is *built*, never what may be *tested*: `Predicate::matches` walks
the raw row itself, so every term's column index is resolved against the
block's **unprojected** schema. That is also what makes a filtered row count —
zero columns plus a filter — expressible.

**A projection narrows what is decoded, never what is walked.**
`RowBatcher::push_row` skips `decode_field` and the builder append for a column
nobody asked for; it does not stop at the last needed field. The walk is
`memchr` and is the cheap half, and it is the system's **only** field-count
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
widths over one file"): `--no-columns` costs **3.28 µs a row** where all 19
columns cost **27.50**, so the replay a projection cannot avoid — the block
read, every row walked and field-counted, the predicate evaluated — is an
eighth of a complete typed read. Between those, one `smallint` is +0.13 µs, the
other fifteen scalars +10.30 between them, the composite +0.77, and the two
array columns **+12.98** — 95% of what all three nested columns cost, and more
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
the four ordering ones and `=`/`!=` alike — and they have their own subsection
below. Evaluating predicates *during* the scan is future work; it inverts
control, not dependency (see `layering.md`).

**A filter is a conjunction**: `QueryOptions::filters` is a list of
single-column terms, and a row survives only if every one of them matches.
The empty list is the default and yields every row, so "no filter" is the
degenerate conjunction rather than a case of its own — nothing on the row
path branches on whether a filter exists. Terms are resolved to column
indices once per block, in term order, and a `ResolvedTerm` vector parallel to
the term list travels in the block's `Active` state; a term naming a column the
block does not carry is `Error::UnknownPredicateColumn`, raised for the first
such term. Evaluation short-circuits at the first term that fails, so the
ordinary case costs one walk of the row; each term does walk it separately,
which is only a real cost for a conjunction whose leading terms almost always
pass.

**`stream::resolve_terms` is the single site where a predicate meets a block.**
Every refusal a term can earn — the unknown column, the ordering refusals
below, and a literal that is not a value of the column's type under *any*
comparing operator — is raised there, against that block's own **unprojected**
`ResolvedSchema`, before a row of the block flows. It takes the whole
`ResolvedSchema` rather than its `schema` because the ordering refusal reads
`columns`, `plans` and `comparisons` too, and those are positional and
parallel: splitting them across two lookups is how they would come to
disagree. A table whose blocks carry different schemas can therefore refuse at
the third block after rows from the first two were emitted; that is true of
`UnknownPredicateColumn` as well and adds no new shape of failure.

**Nothing folds two terms together.** Two terms on one column are evaluated
independently, so a contradictory pair is a query with no rows rather than an
error, and a redundant pair costs a second walk. There is no simplifier and
no plan.

*Rejected: `OR` and `NOT` alongside the conjunction.* Not for code volume —
for NULL. A NULL field matches neither `Eq` nor `Ne`: unknown is collapsed to
false at each term, which is sound under `AND` and unsound under `NOT`, since
SQL's `NOT UNKNOWN` is `UNKNOWN` rather than `TRUE`. Admitting `NOT` does not
add an operator, it obliges a real three-valued evaluator and re-opens the
semantics of every operator that already exists. It is specified in
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md), "The
predicate model becomes an expression tree".

**On the command line a filter repeats rather than splits**, exactly as a
projection does: `pgdq query --filter <term>`, once per term, ANDed. Each
repetition is parsed on its own — a malformed one is refused before the dump
is opened — and the CLI decides nothing else about them; the column lookup and
its refusal are the library's, identical for an embedder.

**A string comparison agrees with PostgreSQL more often than it deserves to**,
and the reason is a property of the input rather than of the comparison: every
value in a dump is already in canonical *output* form — discrete ranges
canonicalize on input, array input whitespace is dropped — so the literal in
the file is the one `*_out` would write. That is what still carries `=`/`!=`
on a **nested** column, and on any column the register has no comparison for:
the field's own bytes are the value, so comparing them is right. What the
argument never covered is the *literal the user typed*, which is what typed
equality closes — see "Equality is typed too" below.

*Rejected:* type-aware comparison of a **nested** column. It needs the
*input*-side grammar, which I20's scope limit flags as considerably more
permissive than the `*_out` inverse the decoders commit to, plus
canonicalization for the three discrete built-in ranges. That is one-time work
belonging with typed predicates, and it is specified — with the measured
PostgreSQL semantics — in
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md), "Nested
comparison is structural, two-valued, and inherits comparability".

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

**They are available on a column that resolved `Mapped` with a
`NestedPlan::Scalar` plan, and refused on any other**, with the reason named:
a column whose type did not resolve has no order of its own (which is also
what makes `SchemaMode::Strings` refuse every ordering operator — it resolves
nothing, so the rule needs no case for it), and a nested column is refused
because an order over an `array_out`/`record_out` literal is not a thing this
layer defines. A nested column keeps `Eq`/`Ne`, for the reason the paragraph
above gives.

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
bytewise in both cases — **reading the clause moves the verdict, never the
answer**. Three cases, and `pg_dump` writing a clause only where the collation
differs from the *type's* default (I37) is what makes the third one decidable:

| The column says | Verdict |
|---|---|
| `COLLATE "C"` or `COLLATE "POSIX"`, qualified `pg_catalog` or bare | **Agrees**, on every server and under every libc |
| any other collation — a libc locale, an ICU collation, a user's own | **Diverges** (`NonBytewiseCollation`) |
| nothing, and the type's default collation is `C` — i.e. `name` | **Agrees** |
| nothing, and the type's default collation is the database's — `text`, `varchar`, `char(n)` | **Diverges** (`UnknownCollation`), because a plain dump does not record it (I32) |

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
"Collation-aware comparison" is that second half, and it is deliberately not
scheduled inside P11 — see [`roadmap.md`](roadmap.md).

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

**Keyed on the declared type, not on the Arrow one.** Seven unrelated declared
types reach `Utf8View` — a text type, a bare `numeric`, an `interval`, a
`timetz`, an `inet`, a `jsonb`, and `json`, which PostgreSQL does not order at
all — and they are seven *different* comparisons under one Arrow type, so the
Arrow type cannot say which one a column wants. Two pairs go further and are not even
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
| `char(n)` declaring `COLLATE "C"`/`"POSIX"` | `Utf8View` | yes — the dump's blank padding comes off both sides first, which is `bcTruelen` (I38), and bytewise is what those collations order the remainder by | — |
| `char(n)` with no clause | `Utf8View` | **no** — the padding is handled, and what is left is the `text` row's residue: its collation is the database's, which a plain dump does not record (I32) | a collation the file does not carry |
| **bare `numeric`**, and `numeric` beyond 76 digits | `Utf8View` | yes — compared as an arbitrary-precision decimal over the text the file holds, which is `cmp_var_common`'s own value order and so insensitive to display scale: `1.5` and `1.50` are one value (I33). The bare form carries all three of `Infinity`, `-Infinity` and `NaN`; the constrained one carries only `NaN` (I34) | — |
| `interval` | `Utf8View` | yes — compared as `interval_cmp_value`'s 128-bit span, so `1 mon`, `30 days` and `720:00:00` are one value (I40), with the v17 infinities read on every file (I34) | — |
| `time with time zone` | `Utf8View` | yes — the UTC-equivalent instant, then the stored zone, so two spellings of one instant are ordered rather than equal (I40) | — |
| `inet`, `cidr`, `macaddr`, `macaddr8` | `Utf8View` | yes — family, then the shorter netmask's worth of address bits, then the netmask, then the address (I40); the MAC types are their octets' own order | — |
| `jsonb`, and any domain over it | `Utf8View` | **no**, and only because of its strings — the structure is compared exactly as `compareJsonbContainers` compares it (kind, then a container's size, then member-wise, with a top-level scalar inside the pseudo-array that makes it outrank `[]`), while every string leaf and object key goes through `varstr_cmp` under the *database's* collation (I41), which a plain dump does not record (I32) | the same collation the text row wants, one level down |
| `json`, and any domain over it | `Utf8View` | **no** — PostgreSQL defines *no* comparison for `json` at all, so bytewise offers more than the server does rather than less | nothing, since there is no order to agree with |

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
type's own decoder.* There is no decoder to round-trip through — these four
types have no Arrow representation at all, which is why their comparison is a
key rather than a value, and building one to widen a literal grammar inverts
the cost.

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
`accepted_form` is one `&'static str` per `CompareKind`, filed next to
`order_key` and `equality_comparison` — the two functions that decide what is
accepted — rather than on `CompareKind` in `pgtype.rs`, so a widening or
tightening has its own description on the same screen. Carrying the phrase on
the L2 type would put the sentence a file away from the L4 code it describes,
which is how a diagnostic goes quietly stale. `jsonb` needs the least of it,
its literal grammar being the whole of `jsonb_in`, and `text`/`character(n)`
need none: every string is a value of a text column, so their arm is
unreachable and is written out rather than made a panic on a diagnostic path.
[`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=`
compare values, not spellings", carries the same forms in prose.

**The refusals are the rest of the table, and they are stated rather than
listed**: every nested shape — array, composite, range, multirange — and every
declared type this build maps to nothing at all. The first group is refused
one step earlier, on the `NestedPlan`, so the register's answer for it is the
second of two agreeing ones.

**A divergent comparison is announced by the CLI, once, on stderr**, after the
schema resolves, naming the column and the divergence — including for a query
that selected no rows, which is the case a user most wants explained. It is
announced per **term**, not per column, because a divergence is
operator-conditional: three of the five are divergences of *order* alone, so a
`text` column filtered with both `<` and `=` is warned about once. The
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
order therefore genuinely differs. There the file does carry the fact, so
`<`/`>` return a row set the server would not and something could be written
that closes it: a comparison per named collation, which is a collation library.
It is announced per column, and equality is unaffected for every *deterministic*
collation, which is every libc one — so what is wrong is the ordering, not the
data and not `=`.

**Equality has one column shape it answers wrongly and silently**, and it is
not this entry: a user-defined ICU collation declared `deterministic = false`,
which a plain dump states outright (I42). Under one, `texteq` is not a byte
comparison, so `=` here returns a row set the server would not, and nothing is
announced. The channel to announce it on now exists — `=` routes through the
comparison plan and `comparison_notes` carries an equality divergence — but
nothing reads the `CREATE COLLATION` that states the fact, which is what P11's
11.6.2 is for.

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

*Rejected: closing this as a property alongside the other three, to strike
`KD7` as P11's spec bound.* The three that closed are properties because
nothing anybody could write would change the answer — the database default is
not in the file, and `json` has no server order at all. This one is the
opposite: the file states the collation, the row set is wrong, and the code that
would fix it is nameable. Calling it a property to complete a strike is the
"property filed as a deficiency" rule running backwards, and it is the one
direction the register cannot recover from — a struck number reinstated is a
state it has no way to express, where an entry kept and struck later costs one
edit. The spec's binding was amended instead; reasoning in
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
and the spec filed it as though it were. `1.5` against a `numeric(10,2)`
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

**The divergences become operator-conditional**, which is why the note channel
is `TableStream::comparison_notes` and its notes are per *term*. Three of the
five reach ordering alone — `UnknownCollation`, `NonBytewiseCollation`,
`JsonbStringCollation` — and one fact settles all three: every libc collation
is *deterministic*, so `varstr_cmp` returns zero exactly when the bytes are
equal and `texteq`/`bpchareq` are byte comparisons whatever the collation is.
A `text` column with no clause therefore warns under `<` and is silent under
`=`, which is not a softening: it is the register saying the row set an `=`
returns is the server's. `ComparisonDivergence::affects_equality` is the whole
rule, and `AsText` is the one pre-existing variant that answers `true` —
PostgreSQL defines no `=` for `json` any more than it defines an order.

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

**Everything else stays silent, and the two populations are different
arguments.** A **nested** column's `array_out`/`record_out` text is a faithful
rendering of the value, so bytewise equality is the server's — subject to an
*inherited comparability* (a `box[]` column would have the same problem one
level down) that
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md) gives to
its nested slice. A column with no DDL behind it at all — `--data-only`,
`--schema-mode strings` — has nothing said about its type to qualify, and
`ResolvedSchema::notes` already reports that on L2.

**The oracle asserts all six operators now.** `=` and `<>` are asked over a
wider population than the other four, since equality is never refused, and both
operands of a cell are values the server itself stored — so for a canonicalized
kind the assertion is that the file's own spelling compares byte for byte, and
the content is in the kinds where it cannot. Every one of those has a case:
`real`'s `-0` against `0`, a bare `numeric`'s `1.5` against `1.50`, a `jsonb`
number written two ways, and a `character(10)` value padded against one that is
not. See "The register against the oracle's answers".

### A filter term is parsed for two audiences

`parse_filter` in `pgdump_query-cli/src/main.rs` is the grammar, and it is the
CLI's alone: `Predicate` is a plain public struct an embedder fills in field by
field, so nothing below L4 parses `column=value` and none of the trimming or
unquoting below reaches an embedder's values.

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

## The cache

A **best-effort accelerator, never required for correctness**: a stale
structural index costs a rescan and nothing else.

**The format-version integer is not tracked in any document.** Pre-1.0 a bump
is free and nothing migrates, so the number carries no information a reader can
act on; `cache.rs`'s `FORMAT_VERSION` is the only place it exists, and `git log
-p` on that constant is its history. The envelope exists so a stale cache is
*detected* rather than misread — the rule is to bump whenever a persisted field
is added, removed or reshaped, and never to record which bump that was.

**Reads and writes have deliberately opposite failure modes.** An unrecognised
`format_version`/`container_kind`, or bytes that do not parse as a cache at
all, are as unusable as a missing file, and `CacheMode::load` folds all of them
into `Ok(None)`. `cache::save` propagates I/O failures as `Error::Io`.

**Unusable is four named outcomes, not one.** `CacheStatus` distinguishes
`Missing`, `Unreadable` (bytes that do not decode), `UnsupportedVersion`
(another build's envelope), and `SourceChanged { cached_size, live_size }`.
Every caller that can respond by *scanning* treats them alike — which is why
they were collapsed originally — but `pgdq info` cannot scan, and has a
different sentence for each: a wrong path, a stale build, and "your file
changed since you parsed it" send a reader to three different places even
though all four end in `pgdq parse`. `CacheMode::load`'s single `Ok(None)` arm
is where the collapse still happens, for the callers that want it.

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

**Source identity.** Every cache records the source's size and mtime at save
time and re-observes on load. Size mismatch invalidates (`SourceChanged` — an
unusable outcome, not a new hard-error path); mtime mismatch surfaces as a
`CacheMtimeChanged` diagnostic on an otherwise usable cache.
`ByteRangeSource::modified()` exists for this. That diagnostic matters more
than it used to: under the old `info` a suspicious cache was about to be
overwritten by a rescan anyway, and now it is the answer being reported — which
is why `Diagnostic::cache_mtime_changed` is `pub` where its siblings are
`pub(crate)`. A caller that matches on `CacheStatus` itself still wants the
library's answer to "what severity is this", rather than composing its own
warning beside the ones the index carries.

**`CacheStatus::Valid`/`Incomplete` both carry `total_size`**, the cache's
*own recorded* `SourceIdentity::size` — sound because a live-size mismatch
produces `SourceChanged` before either is reached, and a cache-only caller has
no live size to stat at all. `Incomplete` is `scanned_through` short of it.
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
against the size it already had to stat; folding `Incomplete` into `None` would
instead make `map_forward` restart from byte 0 every time, when a partial cache
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
`load_offline` returns the full `CacheStatus` rather than `load`'s collapsed
`Option<DumpIndex>`, because a cache-only caller has to tell the usable
outcomes from the unusable ones with no scan to fall back on. It cannot reach
`SourceChanged` at all — there is no live file to compare against, which is
exactly what its `CacheOffline` diagnostic warns about.

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
relying on idempotence would put a third whole-index-sized cost in a loop that
already carries two (the throttle and the splice, below); a single-database
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
blocks cost 46 s against a file of 1.9 MB (`measurements.md`, "Per-block cache
saving"). `SaveThrottle` skips a block's save unless at least `K = 20` times
the last save's own *measured duration* has elapsed since it, which bounds save
overhead at roughly `1/K` of scan time in every regime with no constant that
has to be right in two of them — a cheap cache saves often, an expensive one
saves rarely, koji is untouched. Measured at 4000 blocks: 4003 saves become
108, and ~26 s of saving becomes ~1.5 s of a 20.3 s scan. *Rejected:* "every N
seconds" and "every N bytes"; both choose a number against one dump shape, and
the cost tracks block count rather than bytes read.

**What the throttle does not fix**: the *rest* of the same quadratic. Every
`CopyEnd` also clones the whole span list (`map::Builder::snapshot`, then
`stream::splice` over the prefix), so the map itself is O(blocks²) with the
cache disabled entirely — 18.8 s for 4000 blocks under `query --dqcache none`,
which is most of the 20.3 s a throttled `parse` of the same file costs.
That is a separate cost with a separate fix, filed for the scan-performance
phase (`roadmap-P7-scan-performance-inbox.md`).

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

What the guard costs the throttle is close to nothing: the splice, the roles,
the tablespaces and `scanned_through` are updated at every watermark whether or
not the save runs, so a graceful interrupt loses only the block in flight, and
the throttle's window belongs to `SIGKILL`, power loss and panics alone.

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
`UnknownPredicateColumn`, `UnknownProjectionColumn`,
`DuplicateProjectionColumn`, `ResumeQueryMismatch`, `AmbiguousTable`,
`MetadataNotScanned`, `FieldDecode`. The CLI uses `anyhow` over these.

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

**One consequence of the family reaches the `.sql` tree**, and it is the only
one that did: a Debian build sets `--with-extra-version`, so every fixture's
version header now reads `16.15 (Debian 16.15-1.pgdg13+2)` where an Alpine
build wrote `16.15` (I36). The koji sample carries the bare form, so the tree
holds both shapes rather than only one. Nothing parses those strings — they are
reported verbatim — and no snapshot covers them.

`fixtures/<version>/oracle/` is the one directory that is not `pg_dump` output
— see [The comparison oracle](#the-comparison-oracle). It sits beside the
schema directories rather than inside one because it has no flag set; the test
vocabulary's `all_fixtures()` walks for `*.sql` and steps over it.

`fixtures/oracle-differences.tsv` is the one file at the tree's top, because it
belongs to no single major — see
[The cross-major differ](#the-cross-major-differ). Both fixture walks recurse
into directories only, so it is invisible to each.

`scripts/generate_fixtures.py [--version N] [--schema <name>] [--skip-dumps]
[--skip-oracle]` regenerates; **everything across all six versions is 80
seconds** in throwaway 512MB containers, never a host-run Postgres, with the
images already pulled. The two `--skip-*` flags split the two passes, and
`--skip-dumps` is the ordinary way to re-take the oracle: a dump regeneration
is not byte-reproducible (below), so touching the `.sql` tree to change an
answer table would bury the change in noise.

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
`(Utf8View, text)` are exactly the queue P11 closes one at a time, so one case
must not excuse the rest. A `TypeKind` sharing an arm with another shares one
decision and is not separately closable until that arm splits — and for one of
them, `Shell`, no oracle case can exist at all. The empty-enum arm is the same
rule a third time: its guard is separately closable and `public.empty_enum` is
its case.

**An arm is read out of the source, because a `match` is not data.** Three
functions are parsed, and each is anchored on a string the parse must find, so
a rewrite is a reported problem rather than a shorter list that passes:

| Read from | An arm is | Why that granularity |
|---|---|---|
| `builtin_scalar` | one **declared base name** | Several names share `(Utf8View, text)` and each is separately closable, so `text` having a case does not answer for `character varying`. |
| `comparison_user_type` | one **match arm** over `TypeKind`, a guarded arm counting as its own | `Composite \| Range` and `Base \| Shell` are each one decision, so neither is separately closable yet; when 11.10 splits the first, both halves already have cases. Exhaustiveness over the kinds is rustc's job; what this check adds is that each *answer* has evidence. |
| `comparison_for` | one per branch that is not a match arm at all, its own two plus `comparison_user_type`'s early return | The array shape, the built-in name nothing recognises, and a type absent from the dump's `CREATE TYPE` list (I10's multirange companion lands there). |
| `collated_text` | one per **collation branch**, three in all | A collatable built-in's answer depends on the column's clause as well as its declared type, so one match arm carries three separately closable answers. |

**The collation is a second dimension, and joining on the declared type alone
collapses it.** An explicit `C`/`POSIX` clause agrees, an explicit clause that
is not bytewise diverges, and no clause at all falls back to the type's own
default; without the dimension all three are answered by one `text` case and a
fourth could be added with nothing behind it. **Which built-in arms branch is
read from the source too** — an arm whose body calls `collated_text` is one —
so `character` started counting on the day its arm began consulting a clause,
rather than being listed here and going stale.

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
`Predicate::matches`, the path a `--filter` takes. 45,394 cells today.

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

**The exception set is enumerated by pair, and it is met.** Thirty-four
`(type, collation, left, right)` cases are permitted to disagree; every one of
them does disagree in the committed files, and every disagreement is one of
them — so a case that starts agreeing fails as loudly as one that stops.
Alongside it, a disagreeing term must *announce* its divergence through
`ComparisonNote` — **under that operator**, read off the term rather than off
the register's plan — so an exception cannot be claimed for a column the
register tells the user it is confident about. Three populations, and the first
two are **one statement asked at two depths** — a collation the file does not
carry (I32), reached once through a column and once through a string inside a
document:

| Population | Why | Cases |
|---|---|---|
| a `jsonb` string leaf | `compareJsonbScalarValue` passes `DEFAULT_COLLATION_OID` to `varstr_cmp`, so a leaf takes the database's collation (I32, I41) | 2 |
| `text` under the database's own collation | glibc `en_US.utf8`: case is lower-weight than letter, an accent sorts with its base letter, and punctuation is ignored at the primary level, so `_x` sorts where `x` does | 30 |
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
alone in the same test as the pair.
`pgdump_query/tests/map_file.rs` separately covers that a real interruption
leaves that same shape.

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
