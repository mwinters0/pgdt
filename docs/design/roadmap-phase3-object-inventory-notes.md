# Phase 3 (full DDL object inventory) — Implementation Notes

Phase 3 is complete: every functional item in
[`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md) is
implemented, across fifteen slices: the six its spec originally listed, six
earned by mid-slice re-sizing, one by a wrong contract, and three by the
end-of-phase grilling. That doc remains the specification —
*what* the object inventory does and why. This one records *how* it landed:
where each piece lives, the non-obvious calls, and the implementation-level
facts later phases inherit. Not a changelog and not a status doc — for what is
and isn't built now, see [`../status/STATUS.md`](../status/STATUS.md).

## Module map

| Concern | Where |
|---|---|
| `Span`/`SpanBody`/`DataBlock`/`TocHeader`, the boundary+classification state machine (`Builder`), `check_tiling`, `attach_text` | `pgdump_query/src/map.rs` (L1, new) |
| `Diagnostic`/`DiagnosticKind`/`Severity` — the file-level channel | `pgdump_query/src/diagnostic.rs` (L1, new) |
| `DumpIndex`: `spans` primary, `blocks()` derived, `roles`/`tablespaces`, `diagnostics`; `build_index`/`scan_preamble`/`preamble_only` | `pgdump_query/src/index.rs` (L1, reworked) |
| DDL statement grammar as `map.rs`'s statement parser: `classify_statement`, `statement_complete`, `in_open_quote`, `extract_statement_cross_refs`, `dump_metadata_from_spans` | `pgdump_query/src/preamble.rs` (L1, reworked) |
| `Event::DollarQuoteEnd`, `Event::LargeObjectStart`/`LargeObjectEnd`, `State::InLargeObjectRegion` | `pgdump_query/src/scan.rs` (L1, extended) |
| Source identity (size+mtime), format version, `CacheStatus::Incomplete`, `CacheMode::Offline`/`load_offline` | `pgdump_query/src/cache.rs` (L1, extended) |
| The mapping/streaming split: `map_forward`, replay, `target_settled`, `ScanExtent` | `pgdump_query/src/stream.rs` (L4, reworked) |
| `pgdq info` summaries, `--map`, `--source`/`--dqcache`, cache-only mode | `pgdump_query-cli/src/main.rs` (extended) |
| `objects`/`partitions` fixture schemas, tablespace preparation, version-gated flag sets | `scripts/fixture_schema_{objects,partitions}.sql`, `scripts/generate_fixtures.py` |
| Synthetic large-object and `INSERT`-run benchmark generators | `scripts/generate_{large_object,insert_run}_bench.py` |

`map.rs` and `diagnostic.rs` are this phase's additions to
[`layering.md`](layering.md)'s table (both L1, both covered by its Arrow-free
grep). No module changed layer, and the `objects.rs` split the spec
conditioned on `preamble.rs` passing ~1500 lines never triggered —
`preamble.rs` is 1348 lines.

Integration tests mirror the split: `tests/map.rs` is new and holds both the
whole-fixture-set invariants and the targeted span-shape tests; `tests/cache.rs`,
`tests/query_cache.rs`, `tests/scan.rs` and `tests/stream.rs` grew the
cache-state, bulk-region and mapping-pass cases.

## The span model

`Span { start, end, database, toc: Option<TocHeader>, toc_owned: bool, text:
Option<SpanText>, body: SpanBody }`, ordered, tiling the file exactly. Its two
enums, in full:

- `SpanBody`: `Table`, `TypeDef`, `Extension`, `Data(DataBlock)`, `Connect`,
  `VersionHeader`, `AlterTypeAddValue`, `Framing`, `Unparsed`, `Unscanned`.
- `DataBlock`: `Copy(CopyBlock)`, `InsertRun(InsertRun)`,
  `LargeObjects(LargeObjectRegion)`.

`SpanBody`'s vocabulary is deliberately not the TOC's ~63 kinds; those ride on
`TocHeader::kind` and attach to `Unparsed` spans, which is what distinguishes
known-but-unhandled from unrecognized.

**A span's `end` is fixed up at push time, not in an end-of-scan pass.** The
tiling invariant makes a span's true end exactly the next span's start, so
`Builder::push_span` closes the previous span the moment a new one is pushed.
That is what makes `Builder::snapshot(&self, end)` — a non-consuming "spans so
far" read — possible at all, and `finish` then only has to close the one span
still open.

**Three things close a statement**, and anything adding a fourth should check
`on_dollar_quote_end`'s `Mode::Statement`-only guard:

1. `preamble::statement_complete` — the accumulator, hardened this phase to
   track double-quoted identifiers (`""` doubling) and `--` line comments;
   before that an apostrophe inside either opened a string that never closed.
2. A `--`-prefixed line reasserting a boundary, *unless*
   `preamble::in_open_quote` says the buffer is mid-string (a value spanning
   physical lines legitimately starts a line with `--`).
3. `scan::Event::DollarQuoteEnd` — position-only, no text.

The third exists because `scan.rs` emits no `Event::Line` for a dollar-quoted
line, *including* the one carrying a `CREATE FUNCTION`'s own closing `;`: a
detector watching only for statement completion can never observe such a
statement ending. Its detection is `touched && tag.is_none()` from
`copy::scan_dollar_quotes`, not a tag transition — testing "had a tag, now
doesn't" misses `AS $$ SELECT 1 $$;`, a shape `pg_dump` emits.

Real `pg_dump` output is unaffected by any of this: every entry carries a
`-- Name:` header, which already reasserts a boundary, and every fixture's map
is byte-identical before and after 3.2.3. What it buys is the claim the
phase's "Scanning" decision rests on — a header-less file degrades to one span
per object, not to one span for the rest of the file.

### Two boundary corrections that predate their slice

Both were pre-existing bugs that a later slice's more precise tests were the
first thing to catch. They are recorded here because each was invisible to the
tiling check by construction.

**A blank line no longer forces a pending comment block to close.**
`looks_like_toc_name_line` only matches `-- Name: `, never `-- Data for Name: `,
and `_printTocEntry()` always writes a blank line after the closing `--`. So
before 3.3, *every* real `COPY` block's TOC comment closed as its own
`Framing`/`Unparsed` span before `on_copy_start` ever saw it, and
`on_copy_start`'s comment-absorption arm was dead code for real fixtures.
`check_tiling` cannot see this — it does not care which span owns which bytes.

**`scan_preamble` retreats its stop point rather than guessing.** It
deliberately never feeds the first `COPY` block's `CopyStart` to its `Builder`
(it would open a `Data` span it cannot close). Once the fix above made a
pending comment survive to that stop point, `finish`'s `flush_pending` would
have guessed it closed as `Framing`/`Unparsed` — the wrong answer, since a
later scan classifies it as part of the `Data` span. `Builder::pending_comment_start()`
lets the caller retreat `end` to the comment's own start, and `flush_pending`
discards a pending comment whose `start >= end` instead of pushing a
zero-length span.

## `DumpIndex`: one owner per fact

The through-line of the phase, and the thing to preserve: **no fact is stored
twice.**

- `spans` is primary. `blocks()` is a filtered iterator
  (`SpanBody::Data(DataBlock::Copy(b)) => Some(b)`), `blocks_for`/`total_rows`
  build on it. The old `blocks` field is gone, so a block's byte offsets have
  exactly one owner.
- `metadata: DumpMetadata` is a **memoized derived view** —
  `preamble::dump_metadata_from_spans(&[Span])`, a pure function keyed on span
  kinds rather than line prefixes. `PreambleBuilder` and `PendingStmt` are
  deleted. Making that possible is why `SpanBody` grew `Connect`,
  `VersionHeader` and `AlterTypeAddValue`: the derived view needs the payload,
  not just "this was framing".
- `roles`/`tablespaces: BTreeSet<String>` are the one exception, and
  deliberately so: they are **accumulated during the scan and persisted**, not
  derived. Recovering a role reference from `Span::text` means re-parsing raw
  statement text on every load, unlike filtering an already-typed `SpanBody`.
  The spec's "Both sets are stored flat and per-file" is the sentence to trust
  where its "What a span carries" phrasing reads as if the set were per-span.
- `diagnostics: Vec<Diagnostic>` is `#[serde(skip)]` and recomputed per load.
  Not persisting is load-bearing, not an optimization: a stored
  `CacheMtimeChanged` would replay a warning about a check *this* run
  performed successfully. `diagnostics_do_not_round_trip_through_the_cache`
  pins it.

**The cutover to the derived view was verified, not asserted.**
`metadata_from_spans_matches_preamble_builder_exactly` ran
`dump_metadata_from_spans(&index.spans)` against the old
`PreambleBuilder`-derived `index.metadata` across all 96 fixtures plus
`edge_cases.sql` — multi-`\connect` concatenation, every `--binary-upgrade`
flag set, and the version-header staging case included — and passed *before*
`PreambleBuilder` was deleted. That is what made the deletion safe.

### Diagnostics: one severity scale, two types

The spec originally asked for one enum spanning L1 file diagnostics and
Phase 2's per-column outcomes. That cannot be built: `DumpIndex` is L1 and
`resolve::ColumnResolution` is an L2 conclusion about PostgreSQL type
semantics, so a `DiagnosticKind` variant carrying one would have L1 name an L2
type. What is shared is the `Severity` scale (`Ord`: `Info < Warning < Error`)
and the `{severity, kind}` shape. `Diagnostic` is the file-level type in L1;
`resolve::ColumnNote` is the per-column *record* in L2 — one per column,
always present, the ordinary case being a clean resolution. Its severity is
derived from `resolution`, not stored, for the same one-owner reason as
everything above. The spec was amended to match.

Producers today: `TilingBroken` (`check_tiling`), `CacheMtimeChanged`
(`CacheMode::load`), `TocCoverage` (always `Info`), `CacheOffline`
(`Severity::Warning`, pushed by `load_offline` on every successful load).

## Mapping and streaming are separate passes

`stream.rs`'s `table_stream` is two phases with a hard boundary, and nothing
yields until the first is done:

1. **`map_forward`** — a free `async fn`, not part of the generator. Walks
   from `index.scanned_through`, drives a `map::Builder`, and after every
   `CopyEnd` splices `snapshot` onto the base spans, advances
   `scanned_through`, merges the builder's `roles()`/`tablespaces()`, and
   persists. Returns when the target is settled or at EOF.
2. **Replay** — a scanner over exactly `[block.header_offset, block.end_offset)`
   per matching block, in file order, producing batches.

`Segment`, `Recorder`, `block_start`, the incremental live ambiguity check and
`current_database` are all gone with the split.

**A live pass never parses a row.** `Event::Row` is `{}` — the scanner only
needs to find `\.` to know the extent — so the mapping phase allocates no
`SourceChunk`s and takes no zero-copy views. That is why the target block's
double read is cheaper than it sounds.

**`splice` owns the seam, not `Builder`.** A `Builder` beginning partway
through a file opens its first span at its own first recognized content, which
is *after* its start byte whenever blank lines separate the two. `splice`
closes that by extending the *preceding* span to where the next one starts —
the same rule `push_span` applies everywhere else. An earlier attempt put a
start floor on `Builder` instead: it tiles, but it hands the blank line after
a block to the following span where a single-pass scan hands it to the block,
so a map assembled from several scans stopped agreeing with one built in a
single pass. `full.spans == eager.spans` is now a test, and it is the property
to preserve: **how a map was assembled must not be visible in it.**

**`target_settled` is deliberately conservative.** Two things veto an early
stop: a matching block carrying `partition_root` (I2 — those blocks are not
adjacent, so only EOF enumerates them), and **any `Connect` span anywhere in
the map** (a `pg_dumpall`, a concatenation or a `--create` dump can define the
same qualified name in a later database, and a `batch_options.database`
selector does not lift it — two `\connect` segments can name the same
database). So early stopping applies to exactly the common case: a
single-database, non-partition-root dump. koji is one. `BatchOptions::ScanExtent`
is the explicit opt-out (`Full`).

**Ambiguity is now detected before any row goes out**, once, over every
candidate the map holds, between the two phases. The Phase 2 gap about a cold
query emitting rows before `Error::AmbiguousTable` surfaced is closed for
everything the scan reached; what remains is narrower and lives in `STATUS.md`'s
"Known gaps".

**Resume has no fallback path any more.** A resume point is inside a mapped
block by construction, so `resume` is "skip blocks ending at or before the
token's offset, start the first survivor at the token's offset instead of its
header". `ResumeToken` lost its `database` field with the live segment;
`InCopyResume::database` stays, since that one is the paused block's own
attribution. The Phase 1 known gap about a resumed cache replay degrading to a
live scan is gone.

## Bulk regions

`SpanBody::Data(DataBlock)`, one span *kind* with three payload shapes. Only
`CopyBlock` carries inner offsets (`header_offset`/`data_offset`/
`terminator_offset`/`end_offset`), because it is the only one anything reads
rows out of; the `span.start ≤ header_offset < data_offset ≤ terminator_offset
< end_offset ≤ span.end` invariant is a one-off for `COPY`, not a pattern to
copy.

**The large-object region gets a scanner-level fast path; `INSERT` runs get a
map-level one.** That asymmetry is the phase's one real cost decision, and I12
is the reason: on v17+ the region can run to hundreds of gigabytes, and a bare
`BEGIN;`/`COMMIT;` pair never appears anywhere else in plain `pg_dump` output
(`StartRestoreLOs`/`EndRestoreLOs`, `pg_backup_archiver.c`), so
`scan::CopyScanner` can recognize it with no TOC context at all — every line
between is skipped **unread**, no `Event::Line`. An unterminated region is
`Error::UnterminatedLargeObjectRegion`, mirroring `UnterminatedCopyBlock`.

An `INSERT` run instead reuses the statement accumulator and simply stops
pushing a span per statement: `Mode::InsertRun`, entered from `Mode::Statement`'s
first line via `parse_insert_target`, folding completed statements into one
span until a different table's `INSERT INTO` arrives or the next TOC comment
reasserts a boundary. Every line is still decoded into `Event::Line`. The
measurement that decides whether that is enough is under "Verification" below.

`extract_statement_cross_refs` is deliberately **not** run over `INSERT`
bodies (`push_insert_run` calls `push_span` directly) — a text value
containing `" TO "` or `"GRANT "` would otherwise be a false-positive role
reference.

**The v17+ multi-entry merge needed less machinery than the spec's prose
suggested.** `on_large_object_start`/`on_large_object_end` push no span; they
extend a `pending_large_objects: Option<(start, end, toc)>` field, and
`push_span` unconditionally flushes it first — so "no span was pushed since
the last `COMMIT;`" *is* "the next `BEGIN;` continues the same region". I12's
priority-band proof (`pg_dump_sort.c`'s `dbObjectTypePriorities`) is what
makes that sound: nothing but another `BLOBS` entry can legitimately arrive
between two of a file's large-object data entries, so no per-entry `Type:`
re-inspection is needed. The merged span's `Span::toc` is the *first* entry's
header. Each merged-in entry's own `Owner:`/`Tablespace:` still feeds the
cross-reference set independently.

## The TOC enrichment layer

`Span::toc: Option<TocHeader>` is filled whenever the preceding comment block
contains a line matching `_printTocEntry()`'s grammar (I3/I18) — `-- Name:
<name>; Type: <kind>; Schema: <schema>; Owner: <owner>[; Tablespace: <ts>]`,
or its `-- Data for Name: ` sibling. `parse_toc_header_line` is a fixed
sequence of `split_once` calls on the literal separators, not a general
grammar, and a field that fails to parse just leaves `toc: None` — the same
graceful degradation header-less input already produces. `--verbose`'s
`-- TOC entry N (class C OID O)` / `-- Dependencies: ...` lines are ordinary
comment lines to it: they contribute nothing and do not stop a Name line after
them from parsing. **The comment block is not a fixed height** — three lines at
minimum, more under `--verbose` — so a reader runs to the closing `--` rather
than assuming an offset.

`classify` recognizes any complete statement starting `SET ` or `SELECT
pg_catalog.set_config(` (case-insensitive) as `Framing` wherever it appears,
including the `SET default_tablespace = …;` that `_selectTablespace()` emits
ahead of an individual object's definition. Treating both producers uniformly
was simpler than distinguishing them by position; the cross-reference
extractor reads that statement's tablespace either way.

**`Span::toc` means "the TOC entry this span belongs to", not "the TOC comment
this span starts with".** A follow-on statement — `ALTER … OWNER TO`, `ADD
MAPPING FOR`, `ALTER EVENT TRIGGER … DISABLE` — inherits the governing entry's
header from `Builder::governing_toc`, and `Span::toc_owned` records whether
the span carried the header text itself. Any future code reading `toc` to mean
the latter must check `toc_owned`, or it gets a false positive for every
inherited follow-on.

`governing_toc` is updated by every `push_span`, keyed on the pushed span's
body: a plain statement or `Data` span becomes the new governing entry;
`Framing`, `Connect` and `VersionHeader` clear it. **The one place inheritance
must be vetoed after the fact** is `push_statement_span`: a mid-file `SET
default_tablespace = …;` only classifies as `Framing` once `classify(buf)`
runs, which is after the `Idle`→`Statement` transition that seeded the
inheritance, so `(toc, toc_owned)` is overridden to `(None, false)` there.

Two figures come out of this layer and they count different things:

- **`TocCoverage { attributed, spans }`** (an `Info` diagnostic) counts
  *attributed* spans — `toc.is_some()`, inheritance included. Counting only
  header-bearing spans reported ~50% on a healthy, fully-TOC'd dump, which
  cannot distinguish the degraded case the figure exists to name.
- **`pgdq info`'s `object kinds:`** is an object *census* and counts
  `toc_owned` spans — one per archive entry, so it reads "eight tables", not
  "eight tables plus their owner statements".

**Grouping remains unimplemented and unassigned.** An object's trailing
`ALTER … OWNER TO` is still its own adjacent span (now attributed, not
orphaned). If a later phase wants real grouping, the TOC's `Dependencies:`
field — captured only under `--verbose`, parsed by nothing — is the documented
mechanism.

## Cross-references

`Builder` accumulates both sets as it classifies:
`push_span` reads `toc.owner`/`toc.tablespace` off every span's `TocHeader`
regardless of kind; `push_statement_span` (the entry point every `classify`
call site now goes through) first runs
`preamble::extract_statement_cross_refs` over the statement text, reaching the
three shapes no TOC field covers — `ALTER … OWNER TO`, `GRANT`/`REVOKE`/`ALTER
DEFAULT PRIVILEGES FOR ROLE`, and `SET default_tablespace` (I19). Matching is
by marker substring plus `Cursor::parse_ident`, the same tolerance
`parse_toc_header_line` documents: this only ever feeds enrichment, never a
span boundary.

**The `PUBLIC` filter must be case-insensitive.** `Cursor::parse_ident`
lowercases every *unquoted* identifier, including the literal `PUBLIC` that a
`GRANT`/`REVOKE`'s empty grantee list writes, so it arrives at the filter as
`public`. This is also an irreducible ambiguity in the dump text: `GRANT … TO
public;` unquoted means the pseudo-role to PostgreSQL's own grammar too — a
role genuinely named `public` can only be granted to when quoted, which
`fmtId` does and `parse_ident`'s quoted branch preserves case for.
`insert_role`/`insert_tablespace` are the single shared filters
(`PUBLIC`/`pg_default` are never recorded).

Per-database filtering is not implemented and is not a missing feature: a
caller filters `spans` by `Span::database` rather than `DumpIndex` growing a
third structure.

## Span text

`Span::text: Option<SpanText> { text, truncated }`, filled by
`map::attach_text(source, &mut spans)` and persisted. `Data` and `Unscanned`
spans never store text.

**It is a pass over finished spans, not a slice taken as each span closes.**
The spec suggests the latter and its rationale (the bytes are in the scan
buffer already) is true, but only the *driver* owns the buffer while only the
`Builder` knows where a span started — and a span can be the whole file
(`--inserts`). Close-time slicing therefore means feeding a retention
watermark from the builder back into `scan.rs`'s buffer management, in two
drivers. The pass gets the identical result, because text is a pure function
of the span's offsets, with no coupling to buffer lifetime.

**It is not one read per span.** Spans tile, and text-bearing spans are
exactly the ones *between* `Data` blocks, so `attach_text` coalesces each
contiguous run into a single `read_range` — one read per gap, over
schema-sized regions the scan just walked — capped at `spans_in_run * TEXT_CAP`
so a multi-gigabyte `Unparsed` region is never pulled in whole. The mapping
pass re-slices wholesale at every checkpoint rather than incrementally,
deliberately: the last span's `end` grows as the scan advances, so its text has
to be re-taken anyway. On koji that is roughly 200 × 154KB against an
hour-long scan.

## Cache

Format version **v9** at the end of the phase (v1→v2 identity, v2→v3 spans,
v3→v4 `partition_root`, v4→v5 span text, v5→v6 `Span::toc`, v6→v7
roles/tablespaces, v7→v8 `DataBlock`, v8→v9 `toc_owned`). Pre-1.0, each bump
is free; nothing migrates.

**Source identity.** Every cache records the source's size and mtime at save
time and re-observes on load. Size mismatch invalidates (folded into the
module's existing best-effort `Absent` bucket, not a new hard-error path);
mtime mismatch surfaces as a `CacheMtimeChanged` diagnostic on an otherwise
`Valid` cache. `ByteRangeSource` gained `modified()` for this.

**`CacheStatus::Incomplete { index, mtime_changed, total_size }`** is
`scanned_through` short of the cache's *own recorded* `SourceIdentity::size` —
which is sound because once `Valid` is reached, the recorded and live sizes
are guaranteed equal (a mismatch would have produced `Absent` first). One
helper, `status_from_file`, serves both `load` and `load_offline`.

**`CacheMode::Offline(PathBuf)`** is never produced by `CacheMode::resolve`;
the CLI constructs it directly when `pgdq info` gets no `--source`. The split
is enforced library-side, not just by the CLI: `load`, `save` and
`require_enabled` reject `Offline`; `load_offline` rejects `Enabled`/`Disabled`;
`read_table` (L4) rejects `Offline` explicitly, ahead of `table_stream`'s own
`load` call, so a caller does not have to trace through a generator to learn
that `query` never accepts a cache-only mode. `load_offline` returns the full
`CacheStatus` rather than `load`'s collapsed `Option<DumpIndex>`, because a
cache-only caller has to tell `Valid` from `Incomplete` to know whether it has
enough.

**`CacheMode::load` treats `Incomplete` exactly like `Valid`**, which is a
deliberate departure from a literal reading of the spec's "live mode still
treats it like Absent". Folding it into `None` there would break the two
`Option`-returning callers (`table_stream`, `preamble_only`): a cold query's
map is *designed* to stop short of the file's size once its target settles, so
a partial cache is the normal shape, and `map_forward` would restart from byte
0 on every subsequent query. The one caller that genuinely needs "is this the
whole file" — `pgdq info`'s default/`--map` fallback — makes that check itself
against the live source's size, which it stats regardless.

## CLI surface

`pgdq info` prints `roles`/`tablespaces`/`object kinds` summaries by default
(nothing at all when a set is empty), plus `diagnostics:`. `--map` lists every
span as `[start, end) <one-line label>` in file order, grouped by database the
same way the block listing is; `span_summary` is the one place in the codebase
that matches every `SpanBody`/`DataBlock` variant for display, and a future
`--filter-kind` should extend it rather than duplicate the match. `--map` and
`--preamble-only` are mutually exclusive, rejected before any scan runs.

All three subcommands take `--source <path>` and `--dqcache <path>`; `query`'s
table argument is `--table`. There are no positional arguments left. Omitting
`--source` on `info` *is* the cache-only trigger (clap's
`required_unless_present = "source"` then requires `--dqcache`), so there is no
separate flag and no "guess why the open failed" ambiguity. `info_offline`
errors on `Absent`, errors on `Incomplete` for the default/`--map` listing, and
succeeds on `Incomplete` for `--preamble-only` when the index's own
`preamble_complete` is set — a preamble-only cache is always `Incomplete` in
the whole-file sense, so that flag is load-bearing, not redundant.

`preamble_only`'s return type widened to `(DumpMetadata, Vec<Diagnostic>)`: it
was the one library entry point answering with `DumpMetadata` alone, so its
diagnostics had nowhere to travel.

**The output shape is provisional**, pending real user trials — see
`STATUS.md`'s "Not started". Nothing else depends on it.

## Fixtures

Two new schemas plus a version-gated flag set, all real `pg_dump` output
across the six routine versions (13–18):

- **`objects`** (`scripts/fixture_schema_objects.sql`), under
  `{default, verbose}` — one object per TOC `Type:` kind neither `edge_cases`
  nor `types` produces, plus two large objects (built by hand with
  `lo_from_bytea`, since no `pg_dump` flag conjures one), a non-default
  tablespace (`objects.tablespaced_table`), and a solo `REVOKE`
  (`objects.no_public_execute()`). `--verbose` is the flag set's whole reason:
  it is the one documented way to widen the TOC comment block past three
  lines.
- **`objects/stats.sql`** (v18 only) — `TOC_PREFIX_STATS`. `SCHEMAS` gained a
  version-conditional flag-set form: a value may be `(min_version, flags)`
  instead of a bare list.
- **`partitions`** (`scripts/fixture_schema_partitions.sql`), under
  `{default, --load-via-partition-root}`. The `_a`/`_m`/`_z` naming is
  load-bearing: `TABLE DATA` entries sort by the *partition's* own name, so
  `evt_m`/`feel_m` exist to be emitted *between* two partitions of one root.
  `feel` is hash-partitioned on an enum column, which makes `pg_dump` force
  load-via-partition-root with no flag at all — so the `default` set exercises
  the multi-block shape on all six majors.

Three construction facts, each confirmed against `pg_dump` source rather than
guessed: `SEQUENCE OWNED BY` needs `serial`, not `GENERATED ALWAYS AS
IDENTITY` (`dumpSequence` folds identity ownership into the identity clause);
a separate `DEFAULT` entry needs a *view* column or a `--binary-upgrade`
dropped column (`dumpAttrDef`'s `separate` flag is set for nothing else);
`PUBLICATION TABLES IN SCHEMA` is PG15+ and the schema carries a
`\gset`/`\if` gate on `server_version_num`.

**Cluster-global objects need explicit teardown.** A subscription blocks
`dropdb` on its own database, and roles and tablespaces are cluster-wide, so
they leak into whatever schema runs next in the same container.
`drop_fixture_db` clears `objects_sub`, `fixture_reader` and `fixture_ts` by
name; a future schema adding its own must grow that function rather than
inherit a generic sweep. `CREATE TABLESPACE` additionally needs its
`LOCATION` directory to exist and be owned by the `postgres` OS user, which
`generate_fixtures.py::prepare_tablespace_dir` does via `docker exec`
`mkdir`+`chown` before the schema SQL runs.

**`--with-statistics` does not exist.** The real flag is `--statistics`
(`{"statistics", no_argument, NULL, 22}`, v18+). Every prior reference in this
project was wrong the same way, presumably extrapolated from `--no-statistics`;
corrected in the invariants register, the compatibility matrix and `map.rs`'s
doc comments. Source-only reasoning got the *shape* right (`TOC_PREFIX_STATS`,
the `STATISTICS DATA` kind, PG18+) and the flag name wrong — which is the
class of surprise the evidence-gathering slices exist to catch.

## Verification

Tiling is checked three ways, and the runtime check is not a substitute for
the test:

- **`tests/map.rs::every_fixture_tiles_exactly`** walks every file under
  `fixtures/` plus the hand-written adversarial `tests/data/edge_cases.sql`,
  including every degenerate shape the spec names: `--data-only` (no DDL),
  `--schema-only` (no data), `--inserts`/`--column-inserts` (zero `COPY`
  blocks), and `dumpall.sql` (concatenated, multi-`\connect`). Fixture
  discovery is directory-driven, so a new schema is pulled in without touching
  the test.
- **`build_index_spans_match_build_map_exactly`** pins the two producers to
  byte-identical spans, so they cannot drift now that they share `Builder`.
- **`check_tiling` at runtime**, reporting through the diagnostic channel, so
  a hole on a dump shape no fixture covers surfaces as a high-severity
  diagnostic on a map that stays usable.

Three measurements back the phase's cost claims. Every figure below is on the
hardware `CLAUDE.local.md` describes; each synthetic input is regenerable with
`--seed 42` (`scripts/generate_{large_object,insert_run}_bench.py` and Phase
2's `generate_perf_data.py`) and none of them are committed.

**Large-object skip — the gating measurement.** A 3GB synthetic region
(`scripts/generate_large_object_bench.py`, `LOBBUFSIZE`-chunked to match real
`pg_dump`) on the SSD, `pgdq info --verbose` with the cache disabled, in a
512MB-limited container: **~1.6–1.7s (~1.9 GB/s) page-cache-warm, ~5.7–6.4s
(~560 MB/s) cold**, against an ~840 MB/s `cat`-to-`/dev/null` floor for the
same file on the same disk. The warm figure is the one that measures the scan
path rather than the device, and it is an order of magnitude above Phase 1's
243 MB/s `COPY`-decode baseline, which does real decode work this pure skip
does not. Read either figure as a *ratio*, not as a disk throughput number.
That gap, plus completing inside a 512MB limit, is the "skipped, not walked"
evidence: walking the region statement by statement would mean one span *and
one stored text string* per `lowrite` call, hundreds of thousands of them.

**koji full re-scan — the regression check.** `exit=0`, **74 COPY blocks,
19,575,829,920 rows, 784,019,857,152 bytes**, byte-for-byte identical to the
pre-Phase-3 baseline including every block's header/data/terminator/end
offset. That identity, over a slice that changes only what happens *between*
blocks, is the regression check. The run's raw throughput (~110 MB/s
wall-clock, ~120–130 MB/s by `node_exporter`'s disk-read counter, against
~240–256 MB/s for the baseline) is **not** a like-for-like figure: a Postgres
restore was writing ~30 MB/s to the same physical HDD throughout, on a disk
~93–95% busy either way. A clean re-measurement is deferred past this phase
and tracked in `STATUS.md`.

**`INSERT`-run throughput — out-of-band item M3.** The comparison 3.6 argued
rather than measured, taken on three 3.00 GiB dumps on the same disk in one
session, three runs each, `pgdq info --dqcache none` in a 512MB-limited
container:

| Input | Wall | Rate |
|---|---|---|
| `COPY` block | 2.55–3.27 s | ~1.0–1.26 GB/s |
| Large-object region | 3.61–4.99 s | ~645–890 MB/s |
| `INSERT` run | 14.55–14.79 s | ~218–221 MB/s |

`cat`-to-`/dev/null` on the same files takes 3.67–3.85 s, so the first two are
at the I/O floor and the third is four times above it — about 11 s of CPU per
3 GiB that the other two do not spend. **`Event::Line` decode plus the
statement accumulator dominates an `INSERT`-run scan**, which is the outcome
3.6 named as earning `INSERT` runs a scanner-level path of their own. Max RSS
is ~35–42 MB across all three, so the fold-into-one-span half of 3.6's work
does what it claimed and memory is not the issue.

What this costs is the phase's cost claim for `--inserts` input specifically —
correctness, tiling and row counts are unaffected. Building the scanner-level
path changes a decision and therefore wants a slice rather than an out-of-band
item; it is filed in [`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) and
flagged in `STATUS.md`. Evidence:
[`../status/history/2026-08-25.md`](../status/history/2026-08-25.md).

## Facts carried forward

- **`scan::Event` is no longer the five Phase 1 variants.**
  `DollarQuoteEnd`/`LargeObjectStart`/`LargeObjectEnd` joined
  `CopyStart`/`Row`/`CopyEnd`/`Line`. Every site matching `Event` exhaustively
  (no `_` arm) needs updating when a variant is added, and that set is:
  `index::build_index`/`scan_preamble`, `stream::map_forward` and its replay
  loop, `map::build_map`, and three spots in `tests/scan.rs`. The exhaustive
  match is the intended mechanism — `stream.rs`'s replay ignores both
  `DollarQuoteEnd` and `Event::Line` explicitly, because a replay covers
  exactly one `COPY` block and nothing outside a block can fall inside one.
- **Coverage increases monotonically** (the spec's standing rule, which
  outlives the phase): a later phase may subdivide a span or attach detail to
  it, never reduce coverage. Splitting the one large-object span into one span
  per object is the intended shape of "more specific". The fixture tiling test
  is what keeps that from decaying into an aspiration.
- **`map::parse_toc_header_line` does not recognize `TOC_PREFIX_STATS`.** A
  deliberate deferral, not a gap — the entry degrades to an ordinary
  `Unparsed` span with `toc: None`. Fixture evidence exists
  (`fixtures/18/objects/stats.sql`) for whoever implements it. Related fact,
  free: a `STATISTICS DATA` header has no owner at all, not even the usual
  placeholder — `dumpRelationStats` never sets `te->owner`, and
  `sanitize_line`'s NULL-hyphen substitution is what turns that into the
  literal `-`, which `parse_toc_header_line` already treats as "no owner".
- **`roles`/`tablespaces` are complete only once `scanned_through` reaches
  the file's size** — the same partiality `preamble_complete` already carries,
  and for the same reason. koji's `backup` role, this phase's motivating case,
  is reachable only through a post-data `GRANT`. `ScanExtent::Full`, or a
  query after `pgdq parse`, is the way to a complete set.
- **`Span::text` is `None` for every `Data` span**, so `COPY`/`INSERT`/
  large-object bytes are never in the cache under any design. That, not a
  scoping choice, is why `query` cannot be cache-only.
- **The completeness check is a reusable primitive**, not CLI plumbing: an
  embedder asking "does this cache already cover what I need" matches on
  `CacheStatus::Incomplete` rather than re-deriving it from `scanned_through`
  and a live stat.
- **`Builder` does not carry `governing_toc` across instances.** A live
  segment's mapping pass always resumes exactly at a `Data` span's own
  boundary (the frontier advances only at `CopyEnd`), and real `pg_dump`
  output never follows a `TABLE DATA` or large-object entry with a
  comment-less statement, so nothing observable depends on it. Noted in the
  field's own doc comment rather than engineered around.
- **A TOC-commented `COPY` block is one span**, comment absorbed — as of the
  3.3 boundary fix. Anything reasoning about `Data` span starts should use
  `span.start`, not `header_offset`.
- **Phase 8 Track A's `INSERT`-run reader has its target already located and
  attributed** (`InsertRun::table`, `row_count`): that phase adds parsing, not
  scanning. Phase 6's caller-supplied diagnostic sink can drain both
  `Diagnostic` and `ColumnNote` — unifying at the drain point stays open.
  Facts filed for phases with no spec yet are in
  [`roadmap-phase6-inbox.md`](roadmap-phase6-inbox.md),
  [`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) and
  [`roadmap-phase8-inbox.md`](roadmap-phase8-inbox.md).
