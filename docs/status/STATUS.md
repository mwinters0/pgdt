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
| Best-effort structural cache (v9) with source-identity checking and cache-only inspection | working |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, and cache-only `info` | working, text output shape provisional; `--json` carries no shape promise at all |
| Arrays, composites, ranges, multiranges | still resolve as strings; the literal codec (`nested.rs`) and `ColumnBuilder`'s `List`/`Struct` arms are built and tested, but nothing resolves to them yet — Phase 4.4 |
| Predicate and projection pushdown; per-row-group statistics | not started — Phase 5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — Phase 6 |
| Device-bound scan performance campaign, sparse row index | not started — Phase 7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — Phase 8 (the map already locates and attributes `INSERT` runs) |

Last updated: 2026-08-25 (phase 4.1.1: the domain-over-`box` array and
zero-field composite fixture values).

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
- [ ] **4.4** Flip type resolution: recursive mapping, built-in range
      subtypes, opaque-element refusal, the `ColumnResolution` surgery
      (`OpaqueElementType` in, `Deferred`/`DeferredKind` out), the
      `(DataType, NestedPlan)` pair threaded through `ResolvedSchema` and
      `RowBatcher::new`, and `render_field`'s deletion in favour of the
      plan-taking one. Nested columns decode end-to-end on the optimistic
      path.
- [ ] **4.4.1** The presentation half: compact `info` rendering,
      `resolution_label`'s new arms, and the `type-handling.md` rewrite (what
      each family becomes, the three ways one is still a string, the shape
      error and its remedies, and that predicates still match literal text).
- [ ] **4.5** The shape census: cache v10, per-block per-column recording, the
      whole-file completeness rule, and the manual's statement of the
      optimistic and exact paths plus the planned representation knob.
- [ ] **4.6** The array stress section in `generate_perf_data.py` and the
      `measurements.md` ratio.

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items. Pooled here so far: **per-column resolution has no
  machine-readable path.** `--json` exports `DumpIndex`, which carries no
  resolved schema, so `pgdq info --verbose`'s per-column outcomes are
  human-only. Phase 4 sharpens this — after 4.4 "why is this column a string"
  has five distinct answers a script might branch on — but does not answer it:
  resolution is per `COPY` *block*, not per table (a header-less block gets
  placeholder column names from its first row), so "a resolved schema per
  table" is not well-formed without deciding what to do about that.

## Known gaps

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

None outstanding — both Phase 4 entries were reviewed on 2026-08-25 and
ratified as built. See [`history/2026-08-25.md`](history/2026-08-25.md).

