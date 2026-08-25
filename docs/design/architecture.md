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
| `preamble.rs`, the DDL grammar, `\connect` handling | [The preamble grammar and `DumpMetadata`](#the-preamble-grammar-and-dumpmetadata) |
| `pgtype.rs`, `resolve.rs`, the type mapping table | [Type resolution](#type-resolution) |
| `decode.rs`, a new type's decode/render pair | [Decoders and render-back](#decoders-and-render-back) |
| `batch.rs`, the zero-copy `Utf8View` path | [Arrow assembly and the zero-copy path](#arrow-assembly-and-the-zero-copy-path) |
| `stream.rs`, `map_forward`, replay, resume, predicates | [Query: mapping and streaming are separate passes](#query-mapping-and-streaming-are-separate-passes) |
| `cache.rs`, the format version, cache modes | [The cache](#the-cache) |
| the CLI's flags or output | [CLI surface](#cli-surface) |
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
| Declared-type string → Arrow `DataType`; domain/enum/range/multirange resolution | `pgdump_query/src/pgtype.rs` | L2 |
| `ResolvedSchema`/`ColumnResolution`/`ColumnNote` — joins a `COPY` header against `DumpMetadata` | `pgdump_query/src/resolve.rs` | L2 |
| Per-type field decode + render-back | `pgdump_query/src/decode.rs` | L2 |
| Arrow batch assembly (`ColumnBuilder`, `RowBatcher`), push-mode `read_table` | `pgdump_query/src/batch.rs` | L3 |
| Pull-mode `table_stream`, `map_forward`, replay, `ResumeToken`, `ScanExtent` | `pgdump_query/src/stream.rs` | L4 |
| Post-parse predicate | `pgdump_query/src/predicate.rs` | L4 |
| CLI (`pgdq parse` / `info` / `query`) | `pgdump_query-cli/src/main.rs` | above L4 |

`error.rs` and `lib.rs` are cross-cutting and belong to no layer. Which layer a
module is in constrains what it may depend on and what it may know:
[`layering.md`](layering.md), which also records the two known deviations and
the greps that enforce the rest.

Integration tests mirror the split: `tests/{scan,map,preamble,pgtype,decode,batch,stream,cache,query_cache}.rs`,
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
reasserts a boundary. **Every line is still decoded into `Event::Line`**, and
that costs about 5× a `COPY` scan per byte — see
[`measurements.md`](measurements.md). Correctness, tiling and row counts are
unaffected; what it costs is throughput on `--inserts` input. A scanner-level
`INSERT` path is the fix and is filed in
[`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md).

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

`map::parse_toc_header_line` does not recognize `TOC_PREFIX_STATS`
(`"Statistics for "`, a v18+ `--statistics` component). A deliberate deferral,
not a gap: the entry degrades to an ordinary `Unparsed` span with `toc: None`,
and fixture evidence exists (`fixtures/18/objects/stats.sql`). Free related
fact: a `STATISTICS DATA` header has no owner at all, not even a placeholder —
`dumpRelationStats` never sets `te->owner`, and `sanitize_line`'s NULL-hyphen
substitution turns that into the literal `-`, which `parse_toc_header_line`
already treats as "no owner".

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
file's size** — the same partiality `metadata`'s `preamble_complete` carries,
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
Phase 6's; a sink can drain this list, so nothing here forecloses it.

Producers today: `TilingBroken` (`check_tiling`), `CacheMtimeChanged`
(`CacheMode::load`), `TocCoverage` (always `Info`), `CacheOffline`
(`Severity::Warning`, pushed by `load_offline` on every successful load).

### Reserved slots

Two `CopyBlock` fields are serialized but never constructed:
`sparse_index` (`SparseRowIndex`) and `column_stats` (`RowGroupStats`).
Populating either is additive, not a format-version bump — that is why they
were reserved. The types are placeholders; their real shape is the design work
of whoever fills them. See [`roadmap.md`](roadmap.md) for what each is for.

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
independent of dump size. `table_stream` runs it once, up front, whenever
caching is enabled and the first database's `preamble_complete` is not already
known, persisting immediately (not deferred to a later segment's save) so even
a caller that polls once and drops the stream leaves a cache with that
metadata. Skipped under `CacheMode::Disabled` (pure streaming, no side
effects). This is also what makes `pgdq info --preamble-only` cheap.

**A later `\connect`-ed database's preamble is populated only by a full scan** —
there is no incremental equivalent for anything past the first database.

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

`TypeOutcome` is `Mapped(DataType)`, `Unknown`,
`Deferred(DeferredKind::{Array,Composite,Range})`, `OpaqueBaseType`, or
`EmptyEnum`.

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
| array, composite, range, multirange | `Utf8View` | Deferred — `roadmap-phase4-composite-decoding.md`. They do **not** share a quoting rule (I20); one parameterized scanner covers all three forms |

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

`ColumnResolution::Deferred`'s `kind` does not distinguish a built-in range
from a user-defined one, and both multirange shapes land in
`DeferredKind::Range` rather than a new kind — nothing downstream needs the
distinction yet. Whoever writes the range decoder will need the *subtype*, held
in `TypeDef::Range::subtype` for a user-defined range and **absent entirely**
for a built-in one (PostgreSQL encodes it in the catalog, not in DDL text).

**Array dimensionality is not in the catalog** (I21). `integer[][]` and
`integer[3]` both come back from `pg_dump` as plain `integer[]`, identical to a
one-dimensional column — PostgreSQL arrays carry no fixed dimensionality in the
type system, and one column may hold values of differing dimensionality and
lower bound. An array decoder must infer nesting from the literal's own brace
structure (`{{1,2},{3,4}}`), not from the declared type.

### Joining a header against the metadata

`resolve_columns(qualified_table, columns, metadata, mode, database) ->
ResolvedSchema` joins a `COPY` header's column list against `DumpMetadata` by
name, requiring the caller's `database` — resolved from the matched block's own
attribution, **never guessed** — to pick the right `DatabaseMetadata`.

`ResolvedSchema { schema, columns, notes }`: `columns` is positional (parallel
to `schema.fields()`); `notes` carries one `ColumnNote` per column, mapped and
unmapped alike, with the raw declared-type string attached, since `pgdq info`'s
display needs both.

`SchemaMode::Strings` never looks anything up — every column is
`NotDeclared`/`Utf8View`, at zero lookup cost. In `SchemaMode::Typed` only,
`resolve.rs` checks that the block's attributed database is both present in
`metadata.databases` and `preamble_complete`; otherwise
`Error::MetadataNotScanned { database }`, reachable only through the
incremental path since a full scan always leaves every database it found
`preamble_complete`. Its remedy is real: `--schema-mode strings` bypasses the
check entirely.

### One schema per stream, resolved up front

A stream's Arrow schema is fixed before its first batch. If the metadata needed
to type a column has not been scanned, the query says so
(`Error::MetadataNotScanned`) rather than silently returning untyped columns.

*Rejected:* lazy resolution — typing a column once its DDL happens to have been
seen. It makes the Arrow schema depend on scan progress.

*Rejected for the same reason, in a different hat:* having the live scan
accumulate preamble as it goes. It looks strictly better — a stream passing
database 2's DDL on its way to database 2's data could just read it, and
`MetadataNotScanned` would become nearly unreachable — but that DDL sits
*before* its `COPY` block, so the stream would learn a column's type partway
through its own run and the schema would again depend on how far the scan had
got.

**A full metadata scan is `pgdq parse`, not a new flag.** `parse` already means
"scan the whole file eagerly and persist what you find". An incremental typed
query captures exactly one database's preamble — the first, via the
`scan_preamble` prepass — and nothing more; the error names both the detection
and the remedy in one message. A `--full` flag on `query` would be a third way
to say the same thing.

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
   the builder's `roles()`/`tablespaces()`, and persists. Returns when the
   target is settled or at EOF.
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

Format version **v9**. Pre-1.0 each bump is free and nothing migrates; the
`format_version` envelope exists so a stale cache is *detected* rather than
misread.

**Reads and writes have deliberately opposite failure modes.** An unrecognised
`format_version`/`container_kind`, or bytes that do not parse as a cache at
all, load as `Ok(None)` — indistinguishable from a missing file. `cache::save`
propagates I/O failures as `Error::Io`.

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
time and re-observes on load. Size mismatch invalidates (folded into the
best-effort `Absent` bucket, not a new hard-error path); mtime mismatch
surfaces as a `CacheMtimeChanged` diagnostic on an otherwise `Valid` cache.
`ByteRangeSource::modified()` exists for this.

**`CacheStatus::Incomplete { index, mtime_changed, total_size }`** is
`scanned_through` short of the cache's *own recorded* `SourceIdentity::size` —
sound because once `Valid` is reached, the recorded and live sizes are
guaranteed equal (a mismatch would have produced `Absent` first). One helper,
`status_from_file`, serves both `load` and `load_offline`.

**`CacheMode::load` treats `Incomplete` exactly like `Valid`.** Folding it into
`None` would break the two `Option`-returning callers (`table_stream`,
`preamble_only`): a cold query's map is *designed* to stop short of the file's
size once its target settles, so a partial cache is the normal shape there, not
a defect, and `map_forward` would restart from byte 0 on every subsequent
query. The one caller that genuinely needs "is this the whole file" — `pgdq
info`'s default/`--map` fallback — makes that check itself against the live
source's size, which it stats regardless.

**`CacheMode::Offline(PathBuf)`** is never produced by `CacheMode::resolve`;
the CLI constructs it directly when `pgdq info` gets no `--source`. The split
is enforced library-side, not just by the CLI: `load`, `save` and
`require_enabled` reject `Offline`; `load_offline` rejects
`Enabled`/`Disabled`; `read_table` (L4) rejects `Offline` explicitly, ahead of
`table_stream`'s own `load` call, so a caller does not have to trace through a
generator to learn that `query` never accepts a cache-only mode.
`load_offline` returns the full `CacheStatus` rather than `load`'s collapsed
`Option<DumpIndex>`, because a cache-only caller has to tell `Valid` from
`Incomplete` to know whether it has enough.

**The completeness check is a reusable primitive**, not CLI plumbing: an
embedder asking "does this cache already cover what I need" matches on
`CacheStatus::Incomplete` rather than re-deriving it from `scanned_through` and
a live stat.

**Anything persisted is expressible in L1's vocabulary** — declared type
strings, not resolved Arrow types. That is `layering.md`'s rule 5 and it
applies to everything the cache grows later.

## CLI surface

**The output shape is provisional**, pending real user trials. Nothing depends
on it.

`pgdq info` prints `roles`/`tablespaces`/`object kinds` summaries by default
(nothing at all when a set is empty), plus `diagnostics:`. `--map` lists every
span as `[start, end) <one-line label>` in file order, grouped by database the
same way the block listing is. `span_summary` is the one place in the codebase
that matches every `SpanBody`/`DataBlock` variant for display, and a future
`--filter-kind` should extend it rather than duplicate the match. `--map` and
`--preamble-only` are mutually exclusive, rejected before any scan runs.

**`pgdq info --json` dumps the internal struct, not a designed format.**
`IndexJson`/`MetadataJson` (`pgdump_query-cli/src/main.rs`) flatten
`DumpIndex` (or, with `--preamble-only`, `DumpMetadata`) and add back
`diagnostics` — the one field `#[serde(skip)]` drops for the cache's own
reasons (above, "`diagnostics: Vec<Diagnostic>` is `#[serde(skip)]`"), which
don't apply to a one-shot export. This is deliberately **not** a second
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
"provisional" label for the same reason.

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
separate flag and no "guess why the open failed" ambiguity. `info_offline`
errors on `Absent`, errors on `Incomplete` for the default/`--map` listing, and
succeeds on `Incomplete` for `--preamble-only` when the index's own
`preamble_complete` is set — a preamble-only cache is always `Incomplete` in
the whole-file sense, so that flag is load-bearing, not redundant.

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

## Fixtures

`fixtures/<version>/<schema>/<flag-set>.sql`, real `pg_dump` output across the
six routine versions (13–18). **"Absent" in this tree always means
"deliberately absent", never "not yet generated."**

`scripts/generate_fixtures.py [--version N] [--schema <name>]` regenerates;
everything across all six versions is roughly two minutes in throwaway 512MB
containers, never a host-run Postgres.

| Schema | Flag sets | What it is for |
|---|---|---|
| `edge_cases` | `default`, `create`, `no-comments`, `dumpall`, … | Scanner and preamble robustness shapes; `public.escapes` holds one row per codepoint |
| `types` | `default`, … | One table per Arrow-mappable type family, including boundary values |
| `objects` | `default`, `verbose` | One object per TOC `Type:` kind neither other schema produces, plus two large objects, a non-default tablespace, and a solo `REVOKE` |
| `objects/stats.sql` | v18 only | `TOC_PREFIX_STATS` |
| `partitions` | `default`, `--load-via-partition-root` | The multi-block-per-table shape |

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

**Multi-database coverage does not need a second real fixture.**
`tests/{preamble,pgtype,stream}.rs` each build a two-database shape at test
time by reading `create.sql`, writing a byte-substituted copy with the database
name changed, and concatenating — the parser only cares about the
`\connect`-delimited shape, not which process produced it. A real
`pg_dumpall` fixture exists separately because `pg_dumpall` is *not* just
several `pg_dump --create` outputs concatenated: it passes `--create` for
ordinary databases but writes `template1`/`postgres`'s `\connect` itself, ahead
of their version headers rather than after (I9).

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
a generic sweep. `CREATE TABLESPACE` additionally needs its `LOCATION`
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

Benchmarks are a **regression tripwire**, not an optimization campaign:
`benches/decoders.rs` (one `decode`/`render` pair per mapped type family) and
`benches/whole_file.rs` (one warm-cache end-to-end `Typed` scan, since
`Strings` would not exercise the decoders at all). `text`/`varchar`/`char`
(zero-copy) and `enum` (unparsed dictionary-key append) have no decode step to
benchmark. Figures and their re-run commands: [`measurements.md`](measurements.md).
