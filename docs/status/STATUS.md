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
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, and cache-only `info` | working, text output shape provisional; `--json` carries no shape promise at all |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<` range struct `>`, and `List<List<T>>` for a uniformly multi-dimensional array column. An array whose element type is opaque (`box`, a C base type, a shell type, through any chain of domains) stays a string, and so does an array column whose values disagree on shape |
| Array shape census | recorded by every mapping pass (`CopyBlock::array_shapes`) and **consumed**: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| Predicate and projection pushdown; per-row-group statistics | not started — Phase 5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — Phase 6 |
| Device-bound scan performance campaign, sparse row index | not started — Phase 7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — Phase 8 (the map already locates and attributes `INSERT` runs) |

Last updated: 2026-08-26 (4.5.1: the census is unconditional and resolution consumes it).

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
- [ ] **4.4.2** The array-of-array-typed-element refusal, earned from a defect
      4.4 shipped: `x public.intarr[]` where `CREATE DOMAIN intarr AS
      integer[]` types as `List<List<Int32>>` and no value can fill it (I26).
      The same defect reaches a composite with a field of that type, and a
      domain-over-domain chain. Fixture first — `types/default.sql` gains
      `t_nested_array` on all six majors, carrying both faces plus the five
      working-but-unpinned shapes the sweep found — then the
      refusal and its `ColumnResolution::NestedArrayElement`, a `pgtype.rs`
      unit test for the chain, the manual line, and the deletion of
      `retype_from_census`'s now-unreachable guard. Plus the
      resolution-outcome coverage test — every `ColumnResolution` variant must
      be produced by at least one real fixture column — which is what makes
      `roadmap.md`'s fixture rule mechanical. Lands before 4.6.
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
Nothing has started; the phase runs after Phase 4 wraps.

- [ ] **9.1** `parse` resumes from a matching cache and persists after every
      completed block, via `stream::map_forward`; the resume-point line. Plus
      the koji write-amplification measurement. No output shape changes.
- [ ] **9.2** `info` stops scanning: `CacheStatus::Absent` splits, the "run
      `pgdq parse`" errors, the mtime warning, `--preamble-only` moves to
      `parse`. Both invocation forms unchanged.
- [ ] **9.3** The coverage line — `Scan completion: 76% (12345 bytes)` in
      text, the components as separate fields in JSON.
- [ ] **9.4** `--json` carries per-block resolution, including
      `ColumnResolution::MetadataNotScanned`.

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items. Nothing is pooled here at present.

- **Phase 9 — partial reporting and machine-readable resolution.** Specified,
  not started — see the checklist above.

## Known gaps

- **An array column whose element type is itself an array does not decode at
  all** — `x public.intarr[]` where `CREATE DOMAIN intarr AS integer[]` (I26).
  It resolves to `List<List<Int32>>` from the DDL, correctly, but the value is
  written one brace deep (`{"{1,2}","{3}"}`, elements force-quoted per I25)
  and `batch::append_typed` reads a nested `List` chain as *dimensionality*
  only, so every row is an `Error::FieldDecode`. `--schema-mode strings`
  returns it verbatim. Present since 4.4's resolution flip, not introduced by
  the census; found on 2026-08-26 while reviewing 4.5.1 against real `pg_dump`
  output. **Not accepted, not yet scheduled**: the fix changes what
  `NestedPlan::Array(Array(…))` means, so it needs a spec amendment and a
  slice rather than an out-of-band patch. **Scheduled as 4.4.2**, before 4.6:
  the shape is refused at resolution and comes back as text with its own
  resolution label, the way `box[]` already does. Reasoning:
  [`history/2026-08-26.md`](history/2026-08-26.md).

- **An array nested inside a composite** — or inside another array's element
  type — is still decided optimistically, so a multi-dimensional or
  `[lb:ub]=`-decorated value there is a hard `Error::FieldDecode` naming the
  column. Deliberate and permanent as things stand: the census is keyed by
  column and has nowhere to record a shape at that depth, so scanning more of
  the file cannot help. `--schema-mode strings`, which the message now names,
  returns the literal verbatim. A *top-level* array column no longer reaches
  this — 4.5.1 retypes it from the census on any query, cold or full. Keying
  the census by path is a roadmap "Future" item and would be purely additive.
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

*Empty.* 4.5.1's two entries were reviewed on 2026-08-26. The
`MAX_ARRAY_DIMS` verdict **stands** — a run past `MAXDIM` is not evidence, so
the column keeps its optimistic type and the row surfaces as a `FieldDecode`;
the durable half is in [`../design/architecture.md`](../design/architecture.md),
"The array shape census", and the scan-time diagnostic it does *not* raise is
now a roadmap "Future" item. The spec/manual divergence is **resolved by
amending the spec**: its "What the manual must say" section now describes one
path, matching the census section the same reversal rewrote. Reasoning:
[`history/2026-08-26.md`](history/2026-08-26.md).
