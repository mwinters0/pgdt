# Status

Snapshot of implementation state. Rewritten in place as state changes — this
doc describes what *is*, not how it got there. For dated notes on what a future
session should pick up, or on discoveries that changed the plan, see `history/`
(one file per day, `YYYY-MM-DD.md`) — not a changelog, only entries worth
keeping.

## What exists

Phases 1-3 are complete and were struck at the keystone review, so there is no
per-phase checklist here any more. How the system works is
[`../design/architecture.md`](../design/architecture.md); what is still ahead is
[`../design/roadmap.md`](../design/roadmap.md).

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, and cache-only `info` | working, text output shape provisional; `--json` carries no shape promise at all. **`parse` is the only scanner** — it resumes from a matching cache and banks its progress at `COPY` block boundaries, throttled to ~5% of scan time and saving unconditionally on Ctrl-C (exit 130/143); `info` reports from the cache and never scans |
| Partial reporting | `info` reports a cache from an unfinished scan for as far as it got, with `Scan completion: N% (M bytes)` stated once at the top; `--json` carries the coverage components and per-`COPY`-block type resolution. An interrupted cache is **typed** for every database segment the scan finished — the mapping pass states each database's DDL at that database's first `COPY` block (I1) |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<` range struct `>`, and `List<List<T>>` for a uniformly multi-dimensional array column. Three shapes stay a string, each with its own resolution outcome: an array whose element type is opaque (`box`, a C base type, a shell type, through any chain of domains), an array whose element type is itself an array (I26), and an array column whose values disagree on shape |
| Array shape census | recorded by every mapping pass (`CopyBlock::array_shapes`) and **consumed**: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| Predicate and projection pushdown; per-row-group statistics | not started — Phase 5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — Phase 6 |
| Device-bound scan performance campaign, sparse row index | not started — Phase 7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — Phase 8 (the map already locates and attributes `INSERT` runs) |

Last updated: 2026-08-27 (Phase 9 is **complete**: 9.1-9.5.1 landed — `parse` resumes, throttles its saves, saves on Ctrl-C, and states its metadata at every legal boundary. Next is the koji wrap run, then the two queued out-of-band drive-bys, then Phase 4's **4.4.4** and **4.6**, then the Phase 9 wrap — see "Not started". A phase boundary: an unattended loop stops here.)

## Phase 4 progress

Specified in
[`../design/roadmap-phase4-composite-decoding.md`](../design/roadmap-phase4-composite-decoding.md).

- [x] **4.1** The fixture value shapes the phase needs — lower-bound
      decoration, a mixed-dimensionality column, array-of-composite and
      composite-containing-array, a text-subtype range, array-of-enum,
      `mybase[]`. Generator plus regenerated fixtures; no library code. Notes:
      [`../design/roadmap-phase4.1-fixture-shapes-notes.md`](../design/roadmap-phase4.1-fixture-shapes-notes.md)
- [x] **4.1.1** Two fixture values found after 4.1 landed: an array over a
      domain whose base is `box` (I22 — a domain inherits its base type's
      array delimiter, so the value is semicolon-separated and escapes the
      opaque-element refusal as 4.1 knew it), and a zero-field composite
      (I23). Generator plus regenerated fixtures; no library code. Notes:
      [`../design/roadmap-phase4.1.1-delimiter-and-empty-composite-notes.md`](../design/roadmap-phase4.1.1-delimiter-and-empty-composite-notes.md)
- [x] **4.2** The nested literal codec (`nested.rs`, L2): parameterized
      quoted-token scanner plus array/record/range instantiations, decode and
      render, round-tripped against 4.1's literals. Notes:
      [`../design/roadmap-phase4.2-nested-codec-notes.md`](../design/roadmap-phase4.2-nested-codec-notes.md)
- [x] **4.3** `ColumnBuilder`'s `List`/`Struct` arms, unit-tested directly;
      nothing resolves to them yet. Notes:
      [`../design/roadmap-phase4.3-nested-builders-notes.md`](../design/roadmap-phase4.3-nested-builders-notes.md)
- [x] **4.4** Flip type resolution: recursive mapping, built-in range
      subtypes, opaque-element refusal, the `ColumnResolution` surgery
      (`OpaqueElementType` in, `Deferred`/`DeferredKind` out), the
      `(DataType, NestedPlan)` pair threaded through `ResolvedSchema` and
      `RowBatcher::new`, and `render_field`'s deletion in favour of the
      plan-taking one. Nested columns decode end-to-end on the optimistic
      path. Notes:
      [`../design/roadmap-phase4.4-resolution-flip-notes.md`](../design/roadmap-phase4.4-resolution-flip-notes.md)
- [x] **4.4.1** The presentation half: the resolved Arrow type on `pgdq info
      --verbose`'s per-column line for every column that is not `Utf8View`,
      rendered with arrow's `Display` except the range struct, which collapses
      to `Range<T>`; and the `type-handling.md` rewrite. The manual documented
      **two** ways a column of these types is still a string, not the spec's
      three; 4.5.1 added the third. Notes:
      [`../design/roadmap-phase4.4.1-presentation-notes.md`](../design/roadmap-phase4.4.1-presentation-notes.md)
- [x] **4.4.2** The array-of-array-typed-element refusal, earned from a defect
      4.4 shipped (I26): the shape resolves `Utf8View` with
      `ColumnResolution::NestedArrayElement`, `retype_from_census`'s guard is
      gone, and the resolution-outcome coverage test now enforces
      `roadmap.md`'s fixture rule. `types/default.sql` gained
      `t_nested_array` (both faces of the refusal plus the five
      working-but-unpinned shapes) and `t_enum_domain.v_empty_enum`, which the
      coverage test needed and which produced I27. Notes:
      [`../design/roadmap-phase4.4.2-nested-array-refusal-notes.md`](../design/roadmap-phase4.4.2-nested-array-refusal-notes.md)
- [x] **4.4.3** The array-declaration spellings, earned from 4.4.2:
      `pgtype::array_element` normalizes the whole `Typename` array-bounds
      production (`[]`, `[n]`, repeated, `ARRAY`, `ARRAY[n]` — I28) to the
      element type plus one array level, so `integer[][]` resolves as
      `integer[]` does and the census decides its depth; a declaration
      PostgreSQL rejects stays `Unknown`. The I26 refusal is unchanged and is
      now the only shape that reaches it. `t_array_spelling` (four spellings,
      all dumping as `integer[]` on all six majors) is the fixture half.
      Notes:
      [`../design/roadmap-phase4.4.3-array-spellings-notes.md`](../design/roadmap-phase4.4.3-array-spellings-notes.md)
- [ ] **4.4.4** The array arm folded into one function, earned from 4.4.3's
      unattended call: `resolve_array(element, types) -> TypeOutcome` replaces
      `element_is_opaque`/`element_is_array` and holds the whole array
      decision over one domain walk, so which function normalizes the
      array-bounds production stops being a question. Behaviour-preserving —
      no existing test may change. Also corrects `array_element`'s
      "Not quote-aware" paragraph, which documents a limit the code does not
      have (I29).
- [x] **4.5** The shape census, **recording half**: `ArrayShape` on every
      `CopyBlock`, recorded per column, and the cache format bump that
      persists it; `DumpIndex::is_complete`. Nothing consumed it — that is
      4.5.1's half. The slice was specified as one row and split mid-slice;
      the spec's table carries the earned `4.5.1`. Notes:
      [`../design/roadmap-phase4.5-census-recording-notes.md`](../design/roadmap-phase4.5-census-recording-notes.md)
- [x] **4.5.1** The shape census, **consuming half**: the census is
      unconditional (`Builder::censusing` gone, `CopyBlock::array_shapes` a
      plain `Vec<ArrayShape>`, cache format bumped, `tests/query_cache.rs`
      comparing spans for span again); `resolve_columns` takes the census and
      retypes the `(DataType, NestedPlan)` pair from it;
      `ColumnResolution::VaryingArrayShape` and its `resolution_label` arm;
      `Error::FieldDecode` names `--schema-mode strings`; the manual's
      statement of what an array column becomes, the one case still decided
      optimistically, and the planned representation knob. Notes:
      [`../design/roadmap-phase4.5.1-census-consumption-notes.md`](../design/roadmap-phase4.5.1-census-consumption-notes.md)
- [ ] **4.6** The array stress section in `generate_perf_data.py` and the
      `measurements.md` ratio.

## Phase 9 progress

Specified in
[`../design/roadmap-phase9-partial-reporting.md`](../design/roadmap-phase9-partial-reporting.md).
Taken ahead of the rest of Phase 4 (**4.4.4** and **4.6** remain there). **9.5**
was earned after 9.1-9.4 landed and reopened the phase; **9.5.1** was earned by
9.5's verification. All slices have landed; the phase is ready to wrap.

- [x] **9.1** `parse` resumes from a matching cache and persists after every
      completed block, via `stream::map_forward`; the resume-point line
      (`stream::map_file`, the CLI's resume line,
      `pgdump_query/tests/map_file.rs`). The koji write-amplification figure —
      +1.5% wall and 18 MB written against 784 GB read, so **no save
      throttle** — is in
      [`../design/measurements.md`](../design/measurements.md), "koji full
      scan". Notes:
      [`../design/roadmap-phase9.1-parse-resume-notes.md`](../design/roadmap-phase9.1-parse-resume-notes.md)
- [x] **9.2** `info` stops scanning: `CacheStatus::Absent` splits four ways,
      the "run `pgdq parse`" errors, the mtime warning, `--preamble-only`
      moves to `parse`. Both invocation forms unchanged, and cache-only mode
      now reports an incomplete cache instead of refusing it. Notes:
      [`../design/roadmap-phase9.2-info-stops-scanning-notes.md`](../design/roadmap-phase9.2-info-stops-scanning-notes.md)
- [x] **9.3** The coverage line — `Scan completion: 76% (12345 bytes)` in
      text, the components as separate fields in JSON. Notes:
      [`../design/roadmap-phase9.3-coverage-line-notes.md`](../design/roadmap-phase9.3-coverage-line-notes.md)
- [x] **9.4** `--json` carries per-block resolution, including
      `ColumnResolution::MetadataNotScanned`. Notes:
      [`../design/roadmap-phase9.4-machine-readable-resolution-notes.md`](../design/roadmap-phase9.4-machine-readable-resolution-notes.md)
- [x] **9.5** The self-tuning save throttle (`SaveThrottle`, `K = 20`) and the
      interrupt guard (`ScanOptions::cancel`, read per chunk **and** per
      completed block; `SIGINT`/`SIGTERM` exit 130/143), plus
      `scripts/generate_block_count_bench.py` and the block-count series. The
      throttle hits its `1/K` target — 4003 saves become 195 and ~21s of saving
      becomes ~1.2s — but the series still quadruples per doubling, because
      **the map is quadratic too** and that half is not the cache's; see the
      Known gaps entry. Notes:
      [`../design/roadmap-phase9.5-save-throttle-notes.md`](../design/roadmap-phase9.5-save-throttle-notes.md)
- [x] **9.5.1** `parse` states its `DumpMetadata` at every legal boundary: the
      preamble prepass in `map_file` (cold scans only), *and* a recompute in
      `map_forward` at each `\connect`ed database's first `COPY` block — the
      recurring one of the two points `dump_metadata_from_spans` may be called
      at (I1). An interrupted `parse` now comes back **typed** for every
      database segment it finished, where it used to report `not declared` —
      the final answer — for every column of every block it had banked. The
      recompute fires once per *database*, not per block. Absorbed the queued
      out-of-band move of the EOF recompute into `map_forward`: `table_stream`
      reads `metadata` after the mapping pass, so a cold query types a
      `pg_dumpall`'s later databases exactly as a query after `parse` does.
      `ColumnResolution::MetadataNotScanned` and `Error::MetadataNotScanned`
      are consequently unreachable from any mapping scan; both stay for
      metadata some other scan built, and the narrowing is filed into
      [`../design/roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md).
      The recurring half is tested on the concatenated two-database fixture,
      not `edge_cases/dumpall.sql` as the spec expected — that file spans three
      databases but only one has `COPY` blocks, so it cannot reach the boundary
      twice. Notes:
      [`../design/roadmap-phase9.5.1-metadata-boundaries-notes.md`](../design/roadmap-phase9.5.1-metadata-boundaries-notes.md)

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items. Nothing is pooled here at present.

- **Two out-of-band items, queued as one round**: the `--dqcache none` error
  naming `pgdq parse --dqcache <path>` as its remedy, and `TOC_PREFIX_STATS`
  recognition. They are decision-free and share one review surface. A third —
  the `DumpMetadata` recompute moving out of `map_file` — was folded into
  **9.5.1** and has landed with it.

- **Order from here**, settled 2026-08-27: launch the koji wrap run first —
  detached, log under `runs/`, read by a later session — since it is an hour of
  wall time nothing else depends on, and 4.6's measurement wants its log. Its
  checklist carries one item from 9.5.1: an interrupted koji cache must come
  back **typed**, not reporting `not declared` for every column.
  Then the two out-of-band drive-bys above, while Phase 9's context is fresh,
  then back to Phase 4 for **4.4.4** and **4.6**, then the Phase 9 wrap. Every
  Phase 9 slice has landed, so this is a phase boundary and an unattended loop
  stops here regardless.

## Known gaps

- **An array nested inside a composite** is still decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode` naming the column. Deliberate and permanent as things
  stand: the census is keyed by column and has nowhere to record a shape at
  that depth, so scanning more of the file cannot help. `--schema-mode
  strings`, which the message now names, returns the literal verbatim. A
  *top-level* array column no longer reaches this — 4.5.1 retypes it from the
  census on any query, cold or full — and an array whose *element type* is an
  array no longer reaches it either, since 4.4.2 refuses that shape outright
  (I26). Keying the census by path is a roadmap "Future" item and would be
  purely additive.
- **An array type this build declines to represent comes back as text with no
  way to ask for more.** Two shapes are in that state: an array whose element
  type is itself an array (`ColumnResolution::NestedArrayElement`) and an
  array column whose values disagree on shape (`VaryingArrayShape`). Neither
  is opaque — both are fully understood, and a representation that is lossless
  for every array (dimensions, lower bounds and elements in one value) would
  cover both. Accepted, not scheduled: it is a roadmap "Future" item, and
  adding it later only ever touches columns these two refusals leave as
  `Utf8View`, so it strictly widens coverage.
- **A type name that needs quoting comes back `Unknown`.** A type or domain
  whose name contains a space, a bracket, or the `ARRAY` keyword is legal, and
  `pg_dump` writes it quoted in both its `CREATE` statement and every column
  declaration using it (I29). `parse_ident` dequotes it into `TypeDef.name`
  while the declaration keeps its quotes, so the lookup never matches: the
  column resolves `Unknown` and stays `Utf8View`, and an array *of* such a type
  resolves `List<Utf8View>` — the array-ness is still read correctly, since the
  `[]` falls outside the quotes. **No spelling is misread**: a column of a type
  named `"x ARRAY"` or `"d[3]"` is not mistaken for an array, because
  `array_element`'s strip helpers bail on the trailing `"`. So the cost is a
  weaker type, never a wrong one, and every value still decodes as the text the
  file holds. Unreachable from any dump whose type names are ordinary
  identifiers, which is every fixture and the koji sample. The fix is the
  roadmap "Future" item "A real type-name tokenizer", and it is strictly
  additive.

- **Mapping is O(blocks²), and the save throttle only halved it.** Every
  `CopyEnd` rebuilds `DumpIndex::spans` whole — `map::Builder::snapshot` clones
  the builder's spans, `stream::splice` clones the prefix — so a block-rich,
  byte-poor dump pays quadratic CPU with the cache disabled entirely: 19.7 s
  for 4000 blocks under `query --dqcache none`, against under 10 ms for the
  same bytes in one block. 9.5's throttle removed the other half (44.3 s → 23.6
  s for a 4000-block `parse`). Accepted for now, not scheduled: the fix is to
  stop rebuilding the span list per block, which is the same code Phase 7's
  parallel-scan plans would rework and which
  [`roadmap-phase7-inbox.md`](../design/roadmap-phase7-inbox.md) already flags
  for assuming coverage is a contiguous prefix — so the two belong in one
  decision. A **cheap** version exists and was weighed: for `parse` nothing
  reads `index.spans` between saves, so gating the splice on the throttle the
  same way the save is gated would cost `n/20` splices instead of `n` (~19.7 s
  → ~1 s at 4000 blocks). It is not taken, because it would make an interrupt
  bank the last *saved* watermark rather than the last *completed block* —
  reversing a guarantee 9.5 established — and `Builder::snapshot` asserts
  `Idle`, so the chunk-top interrupt check cannot re-derive the spans mid-block
  to compensate. The analysis is in the phase-7 inbox so it is not re-derived. Nothing koji-shaped is affected: 74 blocks over 784 GB pay this
  74 times, and the figure there is +1.5%. Figures and commands:
  [`../design/measurements.md`](../design/measurements.md), "Per-block cache
  saving is quadratic in block count, and so is the map".

- A query stops mapping once its target is settled, so a conflicting
  candidate **past** the stopping point is never seen and
  `Error::AmbiguousTable` is not raised for it — the query returns the
  candidate it found, with no signal that another existed. The stop rule
  (`stream::target_settled`) rules out the two shapes that announce
  themselves: a matching block carrying a partition-root marker (I2), and a
  file containing any `\connect` at all (`pg_dumpall`, concatenation,
  `--create`). What is left undetectable is a file whose *first* segment is a
  plain dump with something concatenated after it — nothing in the prefix says
  so. `ScanExtent::Full` (or a query after `pgdq parse`) gives exact
  detection. Rows are never a union either way, and ambiguity is raised
  *before* any row is emitted rather than partway through one candidate's,
  which is what the previous form of this gap cost. Accepted, not
  scheduled — closing it means abandoning early stopping, which is what makes a
  cold query on a large dump affordable. Filed into
  [`roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md) so the
  embedded API's promises get decided against it deliberately.
- `map::parse_toc_header_line` does not recognize `TOC_PREFIX_STATS`
  (`"Statistics for "`, a v18+ `--statistics` component — not
  `--with-statistics`, which does not exist in any version). **Queued as
  out-of-band work in the 9.5 round.** The entry tiles as an ordinary
  `Unparsed` span with `toc: None`, so the map stays byte-exact, but each of
  the fixture's 14 statistics entries costs two unattributed spans: TOC
  coverage reads 126/175 (72%) against 126/147 (86%) for the same schema
  without them, so the diagnostic under-reports how much of the file the map
  understood. Only a dump taken with `--statistics` / `--statistics-only` is
  affected — `dumpStatistics` defaults to false in v18.6 and on master.
  Fixture evidence: `fixtures/18/objects/stats.sql`. Detail:
  [`../design/architecture.md`](../design/architecture.md), "TOC enrichment".

- `DumpIndex::roles`/`tablespaces` are complete only once `scanned_through`
  reaches the file's size — the same partiality `metadata`'s
  `preamble_complete` already carries, for the same reason: a query that
  stops at its target (`ScanExtent::UntilTargetSettled`, the default) never
  reaches a reference past the stopping point, which is exactly koji's
  `backup` role (granted only in a post-data `GRANT`). `ScanExtent::Full` (or
  a query after `pgdq parse`) gives the complete set.
- An `INSERT` run is folded into one `Data` span, but every line in it is
  still decoded into `Event::Line` and pushed through the statement
  accumulator — unlike the large-object region, which is skipped unread at
  the scanner level. Measured at **~218MB/s against ~1.0GB/s for a `COPY` dump
  of the same size on the same disk**, i.e. about 5× the per-byte cost, and
  CPU-bound rather than I/O-bound; figures and re-run commands in
  [`../design/measurements.md`](../design/measurements.md). A koji-scale 1TB
  `--inserts` dump therefore maps in ~75 minutes rather than the ~15 the `COPY`
  rate implies. Correctness is unaffected — the map, the tiling and the row counts
  are the same either way. Not scheduled: the fix is a scanner-level
  `INSERT` path, which changes a decision and so needs a slice, filed into
  [`roadmap-phase7-inbox.md`](../design/roadmap-phase7-inbox.md).

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. An entry leaves this section once
it has been looked at: settled into the design docs, or reversed.

*Three from 9.5.1, all decided by proceeding.*

**A `parse` interrupted before it starts still reads the preamble.**
`scan_preamble` ignores `ScanOptions::cancel` on purpose (a stop there could
not be told from reaching the first `COPY` header), and `map_file` runs it
before `map_forward`'s first flag check — so a Ctrl-C at t=0 leaves a cache
holding the first database's DDL rather than nothing. `map_file.rs`'s
`a_scan_cancelled_before_it_starts_maps_only_the_preamble` used to assert the
opposite ("nothing was read, so nothing is claimed") and was rewritten. Taken
because it is what the 2026-08-27 review reasoned to — the prepass is an
uncancellable region bounded by its own length, and banking it is the whole
point of the slice. What would change if reversed: one branch skipping the
prepass when the flag is already set, and an immediate Ctrl-C behaving
differently from one a millisecond later.

**Both `MetadataNotScanned`s are now unreachable from any mapping scan, and
both were kept.** A database's DDL is stated before any of its blocks can be
banked, so no producer leaves the condition. `stream::resolve_block`'s
pre-resolution check stays as the guard against handing back a wrongly-typed
row, and `ColumnResolution::MetadataNotScanned` stays as the reporting answer;
both remain right for a caller presenting metadata some *other* scan built.
`partial_reporting.rs`'s test now pins that answer against a deliberately
hand-built pairing. The alternative — deleting one or both as dead — would
close the Phase 6 question about which answer an embedder gets before Phase 6
gets to decide it; the narrowing is filed into
[`../design/roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md)
instead.

**The spec's third-database assertion was replaced, not dropped.** It asks for
`MetadataNotScanned` on the third database's blocks after cancelling inside the
second — but such a scan has banked none of the third's blocks, so there is
nothing to resolve. The test asserts the checkable half instead (both finished
databases `preamble_complete` with real DDL, every banked block resolving
without `MetadataNotScanned`, and the resumed index matching an eager pass span
for span), which a prepass-only implementation still fails. The fixture also
had to change: `edge_cases/dumpall.sql`, which the spec names, has `COPY`
blocks in only one of its three databases. Detail in the slice notes.

*The notes below say how earlier entries went.*

*9.5's two-granularity guard entry was reviewed on 2026-08-27 and **stands**,
restated as a principle.* The amendment is right — chunk granularity alone
leaves a block-rich dump unresponsive for its whole scan — and the finding
underneath it was that a spec *enumerating* check points is what let the case
through. Both the spec and `architecture.md` now say the rule is "read the flag
at every point the loop can cheaply reach", with the two current sites as its
instances, and both record the two limits it does not remove: the flag is read
before `read_range`, not during it, and `scan::scan`/`scan_preamble` ignore it
on purpose, because a stop there could not be told from reaching the first
`COPY` header and would cache a truncated preamble as complete. The remote-I/O
half of that is filed into
[`../design/roadmap-phase6-inbox.md`](../design/roadmap-phase6-inbox.md).
Reasoning: [`history/2026-08-27.md`](history/2026-08-27.md).

*Three Phase 9 entries were reviewed on 2026-08-26.* The **save-throttle**
entry is **reversed**: the quadratic regime was measured, it costs 44s on a
4000-block dump, and closing it is slice **9.5** rather than a Phase 7
question. The **`parse` types every `\connect`ed database** entry is
**reversed in the direction of agreement**: the recomputation moves into
`map_forward` so a cold query and a warm one answer alike — an out-of-band
change, since the divergence it removes was never a decision anyone took. The
**coverage-line** entry **stands**: the text line prints unconditionally, since
its absence would leave a user unsure rather than reassured, and `--json`
carries the components without a rendered line, because a caller can divide.
Reasoning: [`history/2026-08-26.md`](history/2026-08-26.md).

*The three remaining Phase 9 entries were reviewed on 2026-08-26 and all three
**stand**.* The diagnostics recompute is 3 ms for koji's 833-span cache — a
full `info --dqcache` run, load and render included — so the O(spans) cost is
below process startup; the figure is in
[`../design/architecture.md`](../design/architecture.md), "The cache".
`info --dqcache none` stays an error, with the message to name `pgdq parse
--dqcache <path>` as its remedy the way `Error::FieldDecode` names
`--schema-mode strings` (out-of-band, riding with 9.5). And `v_empty_enum`
needed no ruling at all: fixture expansion is a standing rule, so additions
under it stop being logged here — flagging each one dilutes a section meant for
decisions.

*4.4.3's normalization-placement entry was reviewed on 2026-08-26 and is
**settled by refactor**.* Grilling established that both placements produce
identical outcomes on every input — normalization can only add an array level,
and no normalized terminal is ever the literal name `box` or a `TypeDef.name` —
so the choice was free and the real finding was that two predicates walking the
domain chain separately is what made placement a question. Slice **4.4.4** folds
the array arm into one function; the reasoning is in
[`history/2026-08-26.md`](history/2026-08-26.md), and the policy it was decided
under is `roadmap.md`, "Refactor when the shape stops fitting".

*4.4.2's `integer[][]` entry was reversed by 4.4.3* — the spelling resolves as
`integer[]` does again, and the refusal it was flagging now has exactly one
DDL shape behind it (I26).

*4.5.1's two entries were reviewed on 2026-08-26.* The
`MAX_ARRAY_DIMS` verdict **stands** — a run past `MAXDIM` is not evidence, so
the column keeps its optimistic type and the row surfaces as a `FieldDecode`;
the durable half is in [`../design/architecture.md`](../design/architecture.md),
"The array shape census", and the scan-time diagnostic it does *not* raise is
now a roadmap "Future" item. The spec/manual divergence is **resolved by
amending the spec**: its "What the manual must say" section now describes one
path, matching the census section the same reversal rewrote. Reasoning:
[`history/2026-08-26.md`](history/2026-08-26.md).
