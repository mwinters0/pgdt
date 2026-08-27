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

Last updated: 2026-08-27 (Phase 9 is **complete**: 9.1-9.5.1 landed — `parse` resumes, throttles its saves, saves on Ctrl-C, and states its metadata at every legal boundary. The koji wrap run is in flight and the two out-of-band drive-bys have landed as **M5**/**M6**; next is Phase 4's **4.4.4** and **4.6**, then the Phase 9 wrap — see "Not started". A phase boundary: an unattended loop stops here.)

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

- **A second data-carrying database in the `pg_dumpall` fixture** — out-of-band
  work, **its own round**, settled 2026-08-27. `edge_cases/dumpall.sql` spans
  three databases but has `COPY` blocks in only one, so 9.5.1's recurring
  per-database metadata boundary is verified only against a hand-concatenated
  file `pg_dump` never wrote. `generate_fixtures.py` gains **`pgdq_tenant`**,
  loaded with about four tables: `public.widgets` under the same name as the
  first database's but with **different column types** (so resolving a
  database-2 block to database 1's types is a visible failure, not merely an
  absent error), plus at least one table unique to it (so the DDL is provably
  read rather than inherited). By I30 that name lands it between
  `pgdq_fixture` and `postgres` — two consecutive data-carrying databases, then
  an empty one, which is the case only the EOF recompute covers. It is created
  and dropped for the `edge_cases` schema only, in `create_fixture_db` /
  `drop_fixture_db`, the way `prepare_tablespace_dir` is already gated on
  `objects` — so `dump_flag_set` stays a pure dump-and-write function. Its DDL
  is `scripts/fixture_schema_edge_cases_tenant.sql`, loaded by an explicit
  second call in that branch: `schema_file()` stays a pure fixture-directory →
  file mapping, and the name says the file belongs to `edge_cases` rather than
  naming a fifth fixture set. Its header comment states why the database
  exists — a second `COPY`-carrying segment for I1's recurring boundary —
  since a bare four-table schema file with no explanation is what gets
  "simplified" away later. Regenerated across all six majors.

  `map_file.rs`'s recurring-boundary test moves onto it and **keeps** its
  concatenated case beside it: a real `pg_dumpall` and a bare `cat a.sql
  b.sql` are different shapes (I9), and nothing else covers the concatenation.
  `pgtype.rs`'s and `partial_reporting.rs`'s own copies of the helper stay as
  they are; sweeping them is unrelated churn. The name is deliberately *not*
  `pgdq_fixture_2`, which all three synthetic helpers already use — with both
  constructions in one test file, a failure naming `pgdq_fixture_2` would not
  say which it came from.

  No insta snapshots cover `dumpall.sql`, and the three tests that read it
  (`map.rs`, `preamble.rs`, `pgtype.rs`) assert properties rather than content,
  so the blast radius is the regeneration itself. Kept apart from the **M5**/**M6**
  drive-bys, which have since landed: a six-major regeneration and two
  error-message changes in one diff would have buried the latter.

- **The koji wrap run is in flight**, launched 2026-08-27 and read by a later
  session — nothing waits on it. `runs/koji-wrap.sh` drives three checks in one
  detached pass: the interrupt guard at real scale (SIGTERM 20 minutes in,
  against a cache holding dozens of completed blocks), 9.5.1's claim that the
  interrupted cache comes back **typed**, and the identity check (74 blocks / 19575829920 rows / 784019857152 bytes, per the 9.1
  run). Orchestration log: `runs/koji-wrap.log`; scan output
  `runs/koji-wrap-scan.log`; the two reports `runs/koji-wrap-interrupted-info.log`
  and `runs/koji-wrap-final-info.log`. A later session reads
  `runs/koji-wrap.log` end to end — it states each check's verdict inline. The
  static binary it runs was built before **M5**/**M6** landed, which is
  immaterial to all three checks: M5 changes one error message, and koji is a
  `pg_dump 16` file, a version with no `--statistics` flag to produce the
  entries M6 recognizes.

- **Order from here**, settled 2026-08-27: the koji wrap run is launched and the
  two out-of-band drive-bys (**M5**, **M6**) have landed, so what remains is
  Phase 4's **4.4.4** and **4.6**, then the Phase 9 wrap. 4.6's measurement wants
  the wrap run's log, so read `runs/koji-wrap.log` before starting it. Every
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

**Pending: one.**

*M6's boundary-signal asymmetry (2026-08-27).* The queued description of the
`TOC_PREFIX_STATS` drive-by was "one prefix plus the object-census kind". It
turned out to be two functions with **different** answers:
`parse_toc_header_line` reads all three of `_printTocEntry()`'s prefixes, while
the boundary signal `looks_like_toc_name_line` accepts `"Statistics for "` and
still refuses `"Data for "`. The reason is what follows each — a statistics
entry heads an ordinary statement its span must run into, a data entry heads a
`COPY` block that arrives as its own scanner event. Making the two functions
agree is the mistake available here, and it would split every `COPY` block's
header off from its data. If reconsidered, the alternative is a single
predicate plus an explicit "is this prefix followed by a `COPY`" test at the
call site, which says the same thing with more machinery. Pinned by
`a_statistics_entry_is_one_attributed_span` and
`a_statistics_entry_parses_and_opens_a_span` in `map.rs`.

The notes below say how the earlier ones went.

*9.5.1's three entries were reviewed on 2026-08-27.* The **prepass running
even when the cancel flag is already set** **stands**, now with numbers behind
"bounded by its own length": koji's preamble is 63,333 bytes of 784 GB, and the
most preamble-heavy shape available (4000 tables, 49% preamble by bytes) maps
in 0.04 s — both in [`../design/measurements.md`](../design/measurements.md),
"The preamble prepass is bounded by the schema". The **two
`MetadataNotScanned`s** are **both kept, and the record corrected**: grilling
found the entry's own claim false, since `stream::resolve_block` is private and
all three call sites read `metadata` after the mapping pass, so a carried
`ResumeToken` cannot reach it either — `Error::MetadataNotScanned` is
unreachable through every public entry point, while
`ColumnResolution::MetadataNotScanned` stays reachable because `resolve_columns`
is public. The guard is kept as what stands between a future reordering and a
wrongly-typed row, and is now pinned by a unit test in `stream.rs` rather than
left as untested defence. The **`pg_dumpall` fixture** entry is **reversed**:
`edge_cases/dumpall.sql` gains a second data-carrying database rather than
leaving the recurring boundary verified only against a hand-concatenated file
`pg_dump` never wrote — queued under "Not started". Reasoning:
[`history/2026-08-27.md`](history/2026-08-27.md).

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
