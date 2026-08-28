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
| `batch.rs`, the zero-copy `Utf8View` path | [Arrow assembly and the zero-copy path](#arrow-assembly-and-the-zero-copy-path) |
| `stream.rs`, `map_forward`/`map_file`, replay, resume, predicates | [Query: mapping and streaming are separate passes](#query-mapping-and-streaming-are-separate-passes) |
| `cache.rs`, the format version, cache modes | [The cache](#the-cache) |
| the CLI's flags or output, the save throttle, the interrupt guard | [CLI surface](#cli-surface) |
| `scripts/`, a new fixture schema | [Fixtures](#fixtures) |
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
| Declared-type string → Arrow `DataType`; domain/enum/range/multirange resolution; `NestedPlan` | `pgdump_query/src/pgtype.rs` | L2 |
| `ResolvedSchema`/`ColumnResolution`/`ColumnNote` — joins a `COPY` header against `DumpMetadata` | `pgdump_query/src/resolve.rs` | L2 |
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

The `objects.rs` split that was once conditioned on `preamble.rs` passing
~1500 lines has not triggered — it is 1348 lines.

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

`pgdq query` is a pull-mode caller by necessity rather than by taste: rendering
a nested column needs the stream's `NestedPlan`s *while* iterating, and push
mode hands the `ResolvedSchema` back only once the stream is drained. It reads
`stream.resolved_schema().plans` per batch rather than once, because a block's
schema is per-block (see "Nested columns" below). That leaves `read_table` with
no non-test caller, which [`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md)
files for the phase that decides whether push mode keeps its place.

**Batch size** is caller-settable by row count and/or in-memory byte size,
whichever is hit first. Default 8192 rows (matching the common
Arrow/DataFusion convention), no default byte cap.

**Every query returns all columns of its table** — there is no projection.

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
TOC header at all is folded in the same way, exactly as `on_copy_start`'s
`Mode::Comment` arm has always folded one for a `COPY` block. `pg_dump` cannot
produce the shape that distinguishes the two (its only header-less block is the
file header, and `SET` statements always separate that from any data), so what
decides it is the one producer class that *can*: a hand-written or
`pg_dump`-compatible dump, where `-- a note` above an `INSERT INTO` is
ordinary. Absorbing gives that file one `Data` span where gating would give it
a `Framing` span plus a `Data` span, which is the coarser-and-cheaper trade the
`COPY` path already makes. The symmetry between the two paths is a consequence
of that call, not the argument for it.

**Every line is still decoded into `Event::Line`**, and
that costs about 5× a `COPY` scan per byte — see
[`measurements.md`](measurements.md). Correctness, tiling and row counts are
unaffected; what it costs is throughput on `--inserts` input. A scanner-level
`INSERT` path is the fix and is filed in
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md).

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

**What the refusal actually buys is narrow, and it is not what it looks
like.** `saw_name` decides one thing only: whether `Mode::Comment`'s close arm
absorbs the block into the statement that follows, or pushes it as its own
span. On a default dump the refusal is a **no-op for `COPY`** — the close arm
never runs, because the blank line after `--` is absorbed in place and
`scan::CopyScanner` intercepts the header line as `Event::CopyStart` before
`feed_line` ever sees it, so `on_copy_start` reads the pending `TocHeader` out
of `Mode::Comment` without consulting `saw_name` at all. It earns its keep on a
`--disable-triggers` dump (I31), where a statement *does* intervene: absorbing
there routes the entry into `push_statement_span`'s `Framing` veto, and a
leading `SET SESSION AUTHORIZATION DEFAULT;` classifies `Framing` — so the
entry is not merely misplaced but **destroyed**, taking TOC coverage on such a
dump from 2/24 down to 1/22. Refusing keeps it on a `Framing` span of its own,
which is worse than attributed and better than gone.

That dump is therefore the **only** input on which the refusal changes an
outcome, which makes it the only thing that can pin it:
`a_data_entry_keeps_its_own_span_when_disable_triggers_intervenes` in `map.rs`
is that test, and patching the predicate to accept the prefix fails it and
nothing else in the suite. A predicate whose only test restates it is a
predicate nothing checks — the tautological assertion that stood there before
is what let three successive wrong reasons for this refusal be recorded.

Under `--inserts` a data entry heads an `INSERT` run, and `Builder::step`'s
`Mode::Comment` close arm opens `Mode::InsertRun` for it directly — the same
absorption from the same mode, for the data format the scanner does not
intercept, so this predicate never has to say yes. Free related fact: a
`STATISTICS DATA` header has no owner at all, not even a placeholder —
`dumpRelationStats` never sets `te->owner`, and `sanitize_line`'s NULL-hyphen
substitution turns that into the literal `-`, which `parse_toc_header_line`
already treats as "no owner".

Recognition matters to the *diagnostic*, not to the tiling, which was
byte-exact either way: while the prefix was unrecognized each statistics entry
cost two unattributed spans, so `fixtures/18/objects/stats.sql` reported TOC
coverage 126/175 (72%) where the same schema without statistics reported
126/147 (86%). It now reports 140/161 (87%), and `STATISTICS DATA` appears in
the object census like any other kind.

The `--inserts` path had the same two-spans-one-attributed shape, closed the
same way and with the same reach: each data entry cost one `Framing` span
holding the entry and one unattributed `Data` span holding the rows, so
`fixtures/16/edge_cases/inserts.sql` reported 30/52 (58%) and now reports 30/47
(64%). The object census is unchanged at `TABLE DATA: 5` — the entry was always
counted, just against the wrong span. A `COPY` dump was never affected:
`edge_cases/default.sql` reads 31/48 either way.

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
See `../status/STATUS.md`'s known gaps for what a fix would cost, and
`roadmap.md`'s "Future — wanted, unscheduled" for the fix itself.

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
above.

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
L1 and cannot know which columns are arrays; it records what each literal
looks like and leaves the interpretation to whoever consumes it. A composite's
`(…)` and a `json` column's `{…}` therefore reach `observe` too — the first
contributes nothing, the second records a depth nothing will ever read,
because a `json` column does not resolve to a list.

**A row is rejected wholesale before it is split.** An array literal always
contains a `{`, and only an `[lb:ub]=` prefix can precede it, so a row holding
neither byte costs one pass over its bytes and no field splitting at all. That
pass is `memchr2`, not a hand-rolled loop, because on the shape a real dump
mostly has it is the *only* census work there is — the scalar loop it replaced
cost 23× as much and was, on that shape, the whole figure. `on_row`'s doc
comment names the two measurements a reader regenerates by patching that
function.
**The cost is one tier in practice: the rows that pass the pre-filter.** On
brace-free data — the koji shape — every row pays the pre-filter alone, 64 ns
per 16-column row, +11% of a scan reading from memory. A row that passes it
pays field splitting and `observe` on top: 1.76 µs over 19 columns, +234%
warm, so the pre-filter is 4% of what the census costs on the rows it does
not reject. Both collapse to +0% and +1% on a cold read of this SSD, where the
device floor hides them entirely
([`measurements.md`](measurements.md), "The census on brace-free rows" and
"…on array-bearing rows"). It runs unconditionally
anyway, because the alternative is a query that cannot retype its array
columns without a second pass over the same bytes.

**Every mapping pass censuses, so a mapped block always carries one.**
`build_index`, `build_map` and `stream::map_forward` all census, under either
`ScanExtent`. `scanned_through` advances only at a `CopyEnd` watermark or at
EOF, so a `CopyBlock` that reached the map was walked end to end — there is no
such thing as a half-censused block, and no state a later pass could repair.
An empty vector means "censused, saw no array-shaped literal", which is the
same answer as a vector of unconstrained `ArrayShape`s and needs no separate
representation.

*Rejected:* censusing only under `ScanExtent::Full`, so a cold query declines
the per-row work. A cold query already receives every row of every block it
maps — `map_forward` calls `on_row` unconditionally and the queried block's
bytes are read twice regardless — so the saving is the pre-filter alone, which
is 64 ns a row even with the bytes in memory and vanishes entirely behind the
device a cold query is by definition reading from
([`measurements.md`](measurements.md), "The census on brace-free rows"). What
it cost was a state no user could observe or repair: a dump mapped by a cold query and *then* by a full one came out
`is_complete` with its early blocks permanently uncensused, because
`map_forward` splices onto a prefix it does not re-read. Reasoning:
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).

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
detect (`STATUS.md`'s known gap) is also one whose extra blocks the stream
never replays.

*Rejected:* gating the streamed schema on `DumpIndex::is_complete` too. A cold
query can never satisfy it, so every query against a large dump would keep the
optimistic path and the `FieldDecode` refusal the census exists to remove —
while the evidence it needed sat in the blocks it had just walked.

**`is_complete` qualifies a *reported* schema instead.** `pgdq info` answers
"what is this table's schema" from an index alone, with no replay to bound the
claim; a mapped block's own census is total, but one table's data can occupy
several blocks (I2), so a map that stopped short cannot speak for a block past
its frontier. `print_index` therefore passes `&[]` unless the index covers the
file. No CLI surface reaches that case today — `info --source` rescans
whenever the cache falls short and cache-only mode refuses an incomplete
cache — which is what makes the reported half a rule rather than a second code
path.

**So a reported schema and a streamed one may legitimately disagree about one
table**, and each is true of what it describes. `pgdq info` lists per `COPY`
block and resolves each block from that block's own census; a query commits one
schema from the union over the blocks it will replay. A table whose first block
holds 1-D values and whose second holds 2-D ones therefore reads `List(Int32)`
and `List(List(Int32))` on two `info` lines and comes back as text from a
query — the listing is about a block, the query's schema about every row it
will hand back. No fixture produces the shape.

**What survives all this is the array nested inside a composite**, or inside
another array's element type. The census is keyed by column and has nowhere to
record a shape at that depth, so those stay on the optimistic path however
much of the file has been scanned: a multi-dimensional value there is a hard
`FieldDecode`, and `--schema-mode strings` — which the message names — is the
only route to the data. Keying the census by path is a roadmap "Future" item
and is purely additive whenever it lands.

The cache format bumps whenever this shape changes, like any other persisted
field ([The cache](#the-cache)).

## The preamble grammar and `DumpMetadata`

`DumpMetadata` / `DatabaseMetadata` / `Extension` / `TypeDef` / `TypeKind`
recover server and `pg_dump` versions, extensions, user-defined types, and each
column's declared type **as the literal string `pg_dump` wrote** — never a
parsed pair — per the layering rule that L1 stores what the dump said, not what
a later layer concludes.

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
| `interval` | `Utf8View` | `IntervalStyle` is never recorded (I4) — the file does not determine the value |
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

**An array whose element type is opaque stays a whole-column string**, and the
separator stays hardcoded to `,`. The refusal tests the element **after domain
unwrapping, not the declared string** (I22): a domain inherits its base type's
`typdelim` and its own DDL records nothing about it, so `CREATE DOMAIN d AS
box` makes `d[]` semicolon-separated while being named neither `box` nor
`TypeKind::Base`. Such an element resolves to `Utf8View` anyway, so
`List<Utf8View>` would recover nothing a plain string does not — it would only
add a way to split on the wrong character, which round-trips byte-for-byte
while being wrong.

*Rejected:* refusing any array whose element does not resolve to a mapped
Arrow type. It closes the same hole and pays for it by degrading `interval[]`,
`money[]` and every unknown-element array to a whole-column string, where
`List<Utf8View>` recovers the element boundaries and splits on the right
character. *Rejected:* parsing `DELIMITER` out of `CREATE TYPE` into
`TypeKind::Base`. It handles the trap instead of removing it, and buys a
`List<Utf8View>` over values that are opaque by construction.

**An array whose element type is itself an array stays a whole-column string
too**, and it is the refusal that keeps `NestedPlan::Array` meaning one thing.
`CREATE DOMAIN d AS integer[]` with a column of `d[]` is the only DDL shape
that reaches a nested `Array` plan at all (I26 — `integer[][]` does not: it is
a spelling of `integer[]`, whose element is `integer`, and it is normalized to
one array level before the refusal is tested, I28). Its value is written **one brace deep** —
`{"{1,2}","{3}"}`, because `array_out` force-quotes any element whose text
contains `{` (I25) — so the literal's leading brace run and the column's
resolved `List` depth are independent for this shape alone, and
`Array(Array(…))` would have to mean both *one literal, two dimensions* (what
the census produces) and *one literal whose elements are literals*. That is
the collision `NestedPlan` exists to prevent, one level down.

Both refusals are decided in `resolve_array`, off the terminal of one
`domain_terminal` walk, for the same reason: a domain's own DDL records neither
the delimiter it inherited nor the array-ness of its base, so what decides
either refusal is visible only at the end of the walk. **Their order decides a
label, not a type** — both answer `Utf8View` — and it is opaque-first because
`OpaqueElementType` is the stronger statement. As written no input reaches
both: the opaque test matches a bare type name where the array test matches
that name with bounds appended, so an array over a domain whose base is `box[]`
answers `NestedArrayElement`, true but silent about the delimiter. Making it
answer I22 instead means testing opaqueness recursively through the element's
own array levels — a behaviour change, for a better diagnostic on a shape
`pg_dump` cannot write (I21).

**The refusal composes into a composite for free**, and *removes* a subtlety
rather than documenting one. A composite field of the refused type is a
`Utf8View` field inside an otherwise typed `Struct` — the rule every unmapped
leaf already follows — and with no nested `Array` plan reachable from the DDL,
[the census's](#the-array-shape-census) transform has no such shape to guard
against.

*Rejected:* reusing `OpaqueElementType`. The mechanism matches — both refuse an
array because the element's own literal grammar is not what the outer split
assumes — but the label would lie: `integer[]` is not opaque, it is understood
and declined. Opaque means *never improves*; this means *yes, if anyone needs
it*. *Rejected:* splitting the plan variant into a dimensional `Array` beside
an element-is-a-literal one, and teaching `append_typed` to recurse into
`decode_array` per element with `render_field` inverting it. It is the
complete answer and it is contained, but it buys `List<List<T>>` over a shape
almost nobody declares by putting a second meaning into the one type whose
whole purpose is keeping meanings apart, and it lands in `batch.rs`. It stays
available and is strictly additive: it would only ever touch columns this
refusal leaves as `Utf8View`.

**Array dimensionality is not in the catalog** (I21). `integer[][]` and
`integer[3]` both come back from `pg_dump` as plain `integer[]`, identical to a
one-dimensional column — PostgreSQL arrays carry no fixed dimensionality in the
type system, and one column may hold values of differing dimensionality and
lower bound. An array decoder must infer nesting from the literal's own brace
structure (`{{1,2},{3,4}}`), not from the declared type.

**So every array declaration is normalized to its element type plus one array
level** before anything else reads the string — `integer[]`, `integer[3]`,
`integer[][]`, `integer[3][4]`, `integer ARRAY` and `integer ARRAY[4]` are one
type, and the parser discards the bracket count and the bounds rather than
recording them anywhere a dump could carry them (I28). `pg_dump` writes only
the first spelling, so this is entirely the hand-written and other-producer
input path (`roadmap.md`, "The input contract is valid PostgreSQL"); reading a
spelling more literally than PostgreSQL does is how `integer[][]` came to be
refused as an array of arrays, which said something false about the column
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
the column resolves `Unknown` (`STATUS.md`, "Known gaps"). Fixing *that* means
a real type-name tokenizer, a roadmap "Future" item.

### Joining a header against the metadata

`resolve_columns(qualified_table, columns, metadata, mode, database) ->
ResolvedSchema` joins a `COPY` header's column list against `DumpMetadata` by
name, requiring the caller's `database` — resolved from the matched block's own
attribution, **never guessed** — to pick the right `DatabaseMetadata`.

`ResolvedSchema { schema, columns, notes, plans }`: `columns` and `plans` are
positional (parallel to `schema.fields()`); `notes` carries one `ColumnNote`
per column, mapped and unmapped alike, with the raw declared-type string
attached, since `pgdq info`'s display needs both. Every column has a `plans`
entry, `NestedPlan::Scalar` included, so no consumer has to ask whether the
vector applies to it.

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
container type can still be a string are told apart**, since "why is this
column text" has more than one answer a reader will want: its element type is
opaque, its element type is itself an array, its arrays do not share one shape,
or strings were asked for. The first never improves; the second is a shape we
decline to represent and could; the third is what the file holds.

`MetadataNotScanned` is the odd one: it is a property of **how much of the file
was read**, not of a declared type, and it is the only outcome no fixture can
produce (`tests/pgtype.rs` names it as the sole legitimate exemption from the
fixture rule, since every fixture is scanned to EOF). It needs a
`pg_dumpall`/`--create` dump whose scan stopped inside a *later* database:
`scan_preamble` always captures the first (I1), so the earlier ones are never
in doubt.

*Rejected:* reporting such a column as `NotDeclared`. It means "the dump never
explained this column" and is final, where this one means "finish the parse and
ask again" — identical-looking output, opposite advice.

`resolve_columns` also takes the block's array-shape census, positional like
the column list — see [The array shape census](#the-array-shape-census) for
what it changes and who may believe it. It is a parameter rather than
something this module looks up so that a caller cannot silently skip it: the
three `stream::resolve_block` call sites, the resumed one included, would
otherwise be free to disagree about one stream's schema.

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

**No mapping scan produces the condition any more**, and the two answers are
not equally reachable because of it. The pass states a database's DDL at that
database's first `COPY` block, which it necessarily passes before any of that
database's blocks can be banked, so a block in the map always has its database
covered.

- `ColumnResolution::MetadataNotScanned` stays **reachable**: `resolve_columns`
  is public and takes its `metadata` from the caller, so an embedder resolving
  against an index it assembled itself can still present the condition.
- `Error::MetadataNotScanned` is **unreachable through every public entry
  point**. `stream::resolve_block` is private, all three of its call sites are
  in `table_stream`, and all three use the `metadata` read after that call's
  own mapping pass — the resumed path included, so a `ResumeToken` does not
  reach it either.

The check is kept anyway, and pinned by a unit test rather than left as
untested defence: it is what stands between a future reordering — moving that
metadata read back above the mapping pass — and a silently wrongly-typed row,
which is the exact failure the whole asymmetry exists to prevent.

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

**A full metadata scan is still `pgdq parse` and not a new flag.** `parse`
means "scan the whole file eagerly and persist what you find"; a `--full` flag
on `query` would be a third way to say the same thing. What changed is that a
cold query and one run after `parse` now type a multi-database dump alike, so
there is less occasion to want it. Reasoning for the rewritten rejection above:
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
`BatchOptions::ScanExtent::Full` is the explicit opt-out.

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
`BatchOptions::database` (library) resolves it by name.

`pgdq info` groups its block listing by `database: <name>` whenever more than
one is present (silent for the common single-database case) — that is what
makes an `AmbiguousTable`'s candidate names actionable, since they are names
the listing already showed.

The residual gap — a file whose *first* segment is a plain dump with something
concatenated after it — is in `STATUS.md`'s "Known gaps".

### Resume

A resume point is inside a mapped block by construction, so `resume` is "skip
blocks ending at or before the token's offset, start the first survivor at the
token's offset instead of its header". There is **no fallback path**.

`ResumeToken` carries a file offset, a cumulative row count, and — for a token
taken mid-block — the block's header and in-block row count, which is what lets
a fresh stream reconstruct the same schema and continue with no gap or repeat.
A reserved `generation` field is always 0.

**`ResumeToken` exposes no public fields, and must stay that way.** A raw file
offset is meaningless inside a compressed archive entry; opaque now means the
representation can change without an API break. It is also valid only within
the producing process.

### Predicates

Post-parse row filtering: a row is fully parsed, then dropped if it fails.
Predicates are untyped — the compared value is a plain string, and
`IsNull`/`IsNotNull` are the two that exist because before typed columns there
was no way to ask for a NULL at all. Evaluating predicates *during* the scan is
future work; it inverts control, not dependency (see `layering.md`).

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

**Diagnostics are recomputed on load, not persisted.**
`DumpIndex::diagnostics` is `#[serde(skip)]`, so `status_from_file` re-derives
the tiling check and the TOC-coverage figure from the spans it just read —
both are pure functions of those spans and O(spans), which is what makes
recomputation cheaper than storage. The cost is not close: koji's cache is 833
spans, and a whole `pgdq info --dqcache` run against it — load, recompute and
render the full table listing — is **3 ms**, below process startup. A stored one would also be a warning about
a check *this* run performed successfully. The consequence that made this
load-bearing: `pgdq info` reports from a cache without ever scanning, so
anything not recomputed there is simply lost.

**`CacheMode::load` treats `Incomplete` exactly like `Valid`.** Folding it into
`None` would break all three `Option`-returning callers (`map_file`,
`table_stream`, `preamble_only`): a cold query's map is *designed* to stop
short of the file's size once its target settles, and a resumed `parse` builds
on exactly such a cache, so a partial cache is the normal shape there rather
than a defect. Folding would make `map_forward` restart from byte 0 every
time. A caller that instead *reports* what a cache holds reaches for
`cache::load`/`CacheMode::load_offline` and the full `CacheStatus`, which is
what `pgdq info` does.

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

**The completeness check has two forms, one per kind of caller.** A cache-only
caller has no live source to compare against, so `CacheStatus::Incomplete` is
the primitive it matches on — that is why `load_offline` returns the full
status. A caller holding a live source (`map_file`, `table_stream`) instead
compares `DumpIndex::scanned_through` against the size it already had to stat;
`CacheMode::load` deliberately does not make that call on its behalf. Neither
form is CLI plumbing: an embedder asking "does this cache already cover what I
need" reaches for whichever matches what it holds.

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
block — anywhere else would make the trailing database's `preamble_complete` a
lie. The second kind *recurs*, once per `\connect`ed database, and the mapping
loop already sees the event with the governing database tracked, so it
recomputes there: a `CopyStart` whose governing database differs from the one
the metadata in hand covers. That is what makes an interrupted `parse` come
back *typed* — for every database segment it finished, not merely for the one
the prepass captured — where before it came back with no metadata at all and
`resolve_columns` answered `NotDeclared`, the final "the dump never explained
this column", for every banked column.

**Once per database, not once per block.** Recomputing at every `CopyStart` and
relying on idempotence would put a third whole-index-sized cost in the loop
that already carries two (see the throttle and the splice below). A
single-database dump — koji included — therefore recomputes nothing here at
all: the prepass already stood on that same offset. The comparison state is
seeded from `metadata.databases.last()`, because `dump_metadata_from_spans`
finalizes the last database it walks, so the last entry is the one whose
boundary the metadata in hand stands on.

**A cache holding no metadata at all heals on the next resume**, which is what
kept this from needing a cache format bump: the prepass is skipped on a resumed
scan, but "no metadata" differs from any database, so the first `CopyStart`
recomputes — and a resume that reaches EOF instead is covered by `map_file`'s
own recompute.

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
blocks cost 49 s against a file of 1.9 MB (`measurements.md`, "Per-block cache
saving"). `SaveThrottle` skips a block's save unless at least `K = 20` times
the last save's own *measured duration* has elapsed since it, which bounds save
overhead at roughly `1/K` of scan time in every regime with no constant that
has to be right in two of them — a cheap cache saves often, an expensive one
saves rarely, koji is untouched. Measured at 4000 blocks: 4003 saves become
106, and ~28 s of saving becomes ~1.2 s of a 20.6 s scan. *Rejected:* "every N
seconds" and "every N bytes"; both choose a number against one dump shape, and
the cost tracks block count rather than bytes read.

**What the throttle does not fix**: the *rest* of the same quadratic. Every
`CopyEnd` also clones the whole span list (`map::Builder::snapshot`, then
`stream::splice` over the prefix), so the map itself is O(blocks²) with the
cache disabled entirely — 19.4 s for 4000 blocks under `query --dqcache none`,
which is 94% of what a throttled `parse` of the same file costs.
That is a separate cost with a separate fix, filed for the scan-performance
phase (`roadmap-P7-scan-performance-inbox.md`).

**Every exit saves unconditionally** — EOF, a settled target, and an interrupt.
The throttle's whole risk is the window between saves, and those three are
where that window would cost something real: skipping the save at the last
watermark before an early stop would throw a query's scan away.

**The interrupt guard is a cooperative flag** (`ScanOptions::cancel`, an
`Option<Arc<AtomicBool>>` defaulting to `None`), read **at both extremes**:
once per chunk, and at every completed block. *Rejected:* `tokio::select!` in
the CLI over `ctrl_c` and the scan future — it reads as the obvious form and it
is the one that silently discards the work, since `map_file` owns the
`DumpIndex` for the whole scan and cancelling that future drops the map rather
than saving it. Chunk granularity alone is not enough either, and neither is
block granularity: koji's largest block is hundreds of gigabytes (a Ctrl-C
waiting for the next watermark cannot be told from a hang), while a 4000-block
2 MB dump spends its whole 23 s scan inside two chunks. Together they bound the
response by the shorter of a chunk and a block. **The rule is the principle,
not the enumeration**: read the flag at every point the loop can cheaply reach,
because any list of sites is a list that the next loop invalidates — a spec
naming only the chunk check is what let the block-rich case through the first
time. Two limits the principle does not remove. The flag is read *before*
`source.read_range`, so a scan blocked in a slow read notices only when that
read returns — irrelevant for a local file, potentially seconds for a remote
store. And `scan::scan` — hence `index::scan_preamble` — ignores the flag on
purpose: stopping there is indistinguishable from reaching the first `COPY`
header, so a truncated preamble would be cached as a complete one and its DDL
believed. The preamble is therefore an uncancellable region bounded by its own
length; a Ctrl-C during a query's prepass is honoured at `map_forward`'s first
chunk check immediately after, with the prepass's metadata already saved. What the guard costs the
throttle is close to nothing: the splice, the roles, the tablespaces and
`scanned_through` are updated at every watermark whether or not the save runs,
so a graceful interrupt loses only the block in flight, and the throttle's
window belongs to `SIGKILL`, power loss and panics alone.

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

**`--verbose`'s per-column line is a complete statement of the Arrow schema.**
One line per column that has something to say: a column that did not map gets
`resolution_words`' sentence, and a column that mapped gets its Arrow type —
unless that type is `Utf8View`, the no-information answer, which is also the
only type a non-`Mapped` resolution ever produces, so the two never both fire.
The type is rendered by `arrow_type_label`
(`pgdump_query-cli/src/main.rs`) as arrow-schema's own `Display` —
terse, reversible, and carrying a composite's real field names — with one
substitution: the five-field range struct is identical for every range column
in every dump, so it collapses to `Range<T>`, `T` being the bound type. The
substitution is dispatched on the [`NestedPlan`](#nested-columns-nestedplan-travels-beside-the-datatype),
never on the field names, since a user composite may declare five fields with
exactly those names. A built-in multirange and an array of the matching range
therefore render identically (`List(Range<Int32>)`) — correct, not a
collision: they are the same Arrow type, and the declared PostgreSQL type sits
on the same line. `docs/manual/type-handling.md` states the range struct's
real layout once, which is what makes the elision lossless.

*Rejected:* printing the type only where the plan is not `Scalar`. It keeps
every existing line's width untouched, which is its whole appeal, but "what
does this column become in Arrow" is not a question only a nested schema
raises — a `numeric` column's `Decimal128` precision is exactly as invisible
and exactly as consequential to a caller building against the schema.
*Rejected:* a compact lowercase rendering of our own
(`list<struct<a: int32>>`). It is a second spelling of a type vocabulary the
reader already meets everywhere else Arrow is named.

**`pgdq info --json` dumps the internal struct, not a designed format.**
`IndexJson` (`pgdump_query-cli/src/main.rs`) flattens `DumpIndex` and adds the
three things it does not itself carry: `total_size` (see "Coverage is stated
once" above), `diagnostics` — the one field `#[serde(skip)]` drops for the
cache's own reasons — and `resolution`. This is deliberately **not** a second
output shape to maintain: it carries zero compatibility promise, so renaming
or restructuring a field on `DumpIndex` for internal reasons is free to
change its JSON along with it, same as any other refactor. It exists so an
alpha user can get everything the human-readable listing shows (and more —
the raw span/TOC detail no text view surfaces) without pgdq committing to a
CLI flag for their specific need before enough of those needs have converged
(`docs/design/roadmap.md`, out-of-band ledger M4). `--json` is incompatible
with `--verbose`/`--map`: both only add formatting detail to the text view,
all of which the full struct already carries.

*Rejected:* a hand-shaped JSON schema (renamed/pruned fields, a stable
top-level contract). That is exactly the CLI-output work this project is
deferring pending real trials — the text output above carries the same
"provisional" label for the same reason. **No `version` field either**: that is
precisely the compatibility shim `roadmap.md`'s "Pre-1.0" forbids, and it would
be the only one in the tree.

### Machine-readable resolution

`resolution` is what `--verbose` prints per column, in a form a script can
branch on: per column the name, the declared PostgreSQL type, the outcome as a
stable token, the Arrow type as the *exact string* `--verbose` renders, and the
`NestedPlan` structurally (which is the one thing the Arrow type cannot say —
`int4range[]` and `int4multirange` share it).

**One resolution pass, two renderings.** `block_resolutions` is the single
pass; `print_index` and `print_index_json` both consume its output, and
`resolution_words` returns the token and the sentence from *one* exhaustive
match, so a new variant cannot be given one spelling without the other. A
second implementation is the failure mode here — the export would quietly drift
into describing a different vocabulary from the listing. The cross-check
reconstructs every expected `--verbose` line out of the JSON and finds it in
the text, which works because **every sentence begins with its token's words**,
underscores replaced by spaces; a variant that breaks that property makes the
test say so rather than quietly weakening it.

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
three unusable outcomes and reports `Incomplete` like any other cache;
refusing a partial one was what this phase removed.

`preamble_only` returns `(DumpMetadata, Vec<Diagnostic>)`: it was the one
library entry point answering with `DumpMetadata` alone, so its diagnostics had
nowhere to travel.

## Errors

`Io`, `Join`, `UnterminatedCopyBlock`, `UnterminatedLargeObjectRegion`,
`LineTooLong`, `InvalidUtf8`, `CacheEncode`, `CacheDisabled`,
`UnknownPredicateColumn`, `AmbiguousTable`, `MetadataNotScanned`,
`FieldDecode`. The CLI uses `anyhow` over these.

A value that contradicts its declared type is an **error, not a null**: a dump
is machine-generated, so the file is damaged or the mapping is wrong, and both
are worth hearing about with the byte offset.

`FieldDecode`'s message names `--schema-mode strings`, which is a real remedy
for every cause it has: reading more of the file cannot help any of them. The
array-shape refusal is the case that made this true — before the census was
consumed, scanning further *was* the answer for a top-level array column, and
a message naming one remedy would have been wrong half the time.

## Fixtures


**Adding a shape is the default, not a last resort** — see `roadmap.md`,
"Expand the generated fixtures freely; never infer what `pg_dump` writes". A
decision that turns on the exact bytes `pg_dump` emits gets a fixture column;
reasoning out what the output must be is what produced I22's, I23's and I26's
late discoveries.

**Half of that rule is enforced.** `tests/pgtype.rs`'s
`every_resolution_outcome_is_produced_by_a_real_fixture_column` resolves every
column of every `COPY` block of every fixture and requires each
`ColumnResolution` variant to have at least one real column behind it — so a
new refusal cannot land without the `pg_dump` output that reaches it, and its
exhaustive match makes a new variant a compile error until it is listed. The
other half — a shape that resolves to an outcome already covered and merely
*works* — stays a judgement call, which is exactly the five multi-hop shapes
`t_nested_array` collects. Naming that limit beats a check implying coverage
it does not have.

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

`scripts/generate_fixtures.py [--version N] [--schema <name>]` regenerates;
everything across all six versions is roughly two minutes in throwaway 512MB
containers, never a host-run Postgres.

**A regeneration is not byte-reproducible, and three things move on their
own.** `\restrict`/`\unrestrict` carry a random token per dump (v17.6+/v18+);
`logs.events.logged_at` defaults to `now()`; and `--binary-upgrade`'s OIDs can
shift by one, because `create_fixture_db` retries `createdb` against the
image's transient init instance and a failed attempt still consumes an OID. The
first two move on every run, the third only sometimes and per version — two
consecutive runs agreed while both differed from what was committed. So a
regeneration diff is expected to touch every flag set of the schema, not only
the fixture that motivated it, and **nothing may assert on those three**. No
insta snapshot covers any of them.

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
since `text` maps to `Utf8View` either way and the results must match. Every
column-rendering test helper (`rows_of`) goes through the public
`render_field` rather than a hardcoded `StringViewArray` downcast.

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
