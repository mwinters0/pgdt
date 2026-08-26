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
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, and cache-only `info` | working, text output shape provisional; `--json` carries no shape promise at all. **`parse` is the only scanner** — it resumes from a matching cache and persists after every completed block; `info` reports from the cache and never scans |
| Partial reporting | `info` reports a cache from an unfinished scan for as far as it got, with `Scan completion: N% (M bytes)` stated once at the top; `--json` carries the coverage components and per-`COPY`-block type resolution |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<` range struct `>`, and `List<List<T>>` for a uniformly multi-dimensional array column. Three shapes stay a string, each with its own resolution outcome: an array whose element type is opaque (`box`, a C base type, a shell type, through any chain of domains), an array whose element type is itself an array (I26), and an array column whose values disagree on shape |
| Array shape census | recorded by every mapping pass (`CopyBlock::array_shapes`) and **consumed**: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| Predicate and projection pushdown; per-row-group statistics | not started — Phase 5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — Phase 6 |
| Device-bound scan performance campaign, sparse row index | not started — Phase 7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — Phase 8 (the map already locates and attributes `INSERT` runs) |

Last updated: 2026-08-26 (Phase 9: `parse` resumes and is the only scanner; `info` reports from the cache, coverage line included; `--json` carries per-block resolution. Phase 4.4.3: every array-declaration spelling resolves as `integer[]` does).

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
Taken ahead of the rest of Phase 4 (only **4.6** remains there).

- [ ] **9.1** `parse` resumes from a matching cache and persists after every
      completed block, via `stream::map_forward`; the resume-point line. **The
      code landed** (`stream::map_file`, the CLI's resume line,
      `pgdump_query/tests/map_file.rs`); **the koji write-amplification
      measurement did not** — a full koji `parse` is ~54 minutes, so the run
      was launched detached and a later session reads
      `runs/koji-9.1-scan.log`, then adds the figure to
      [`../design/measurements.md`](../design/measurements.md) and ticks this
      box. See [`history/2026-08-26.md`](history/2026-08-26.md). Notes:
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

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items. Nothing is pooled here at present.

- **The koji write-amplification measurement** (slice 9.1) — launched
  detached, not yet read. Nothing depends on it; it decides only whether
  per-block cache saves need a throttle.

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
  `--with-statistics`, which does not exist in any version). A deliberate
  deferral rather than a gap: fixture evidence exists
  (`fixtures/18/objects/stats.sql`) and the entry just degrades
  gracefully, tiling as an ordinary `Unparsed` span with `toc: None`, the
  same as any other unhandled TOC comment shape. Detail:
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

- **A full scan now recomputes `DumpMetadata` over every span, so `pgdq parse`
  types every `\connect`ed database, not just the first.** Phase 9 needed it
  (a resumed `parse` must produce what `build_index` produces), and it is
  strictly more information — but it means a `pg_dumpall` cache written by
  `parse` now answers typed queries against later databases where before only
  `--schema-mode strings` would. It lives in `stream::map_file`, not in
  `map_forward`, so the streaming path is untouched; reversing it means
  `parse`'s index no longer equals `build_index`'s.

- **Loading a cache now recomputes the tiling and TOC-coverage diagnostics.**
  `diagnostic.rs` always said they are recomputed rather than persisted, but
  nothing did it on the load path until `info` stopped scanning and would
  otherwise have lost the TOC-coverage figure. Cost is O(spans) per load,
  including every query's. It also makes `save`→`load` round-trip
  `DumpIndex` exactly, diagnostics included, which three `tests/cache.rs`
  assertions previously had to work around.

- **`info --dqcache none` is now an error.** With no scan to fall back on it
  would have nothing to report, so it is rejected the same way `parse`
  rejects it. The spec did not name this case; the alternative was reporting
  an empty index, which reads as "this dump has no tables".

- **The coverage percentage floors and the byte counts left the summary
  lines.** `N COPY block(s), M row(s)` and `N span(s)` no longer restate
  `scanned_through`, since the coverage line above owns it. This is the
  phase's one formatting decision and the one most likely to come back — 9.3
  is a separate slice precisely so it can.

- **`t_enum_domain` gained `v_empty_enum`, which the slice's spec row does not
  name.** The resolution-outcome coverage test cannot pass without it —
  `EmptyEnum` was the only outcome no generated fixture reached — and the
  alternative was exempting the outcome, which would have put a hole in the
  check on the day it landed. `roadmap.md`'s "Expand the generated fixtures
  freely" covers it. It also produced **I27**, a real ambiguity worth having
  written down: a plain dump writes a label-less enum with the same empty body
  `--binary-upgrade` writes for *every* enum.

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
