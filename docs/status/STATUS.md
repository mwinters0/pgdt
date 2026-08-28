# Status

Snapshot of implementation state. Rewritten in place as state changes — this
doc describes what *is*, not how it got there. For dated notes on what a future
session should pick up, or on discoveries that changed the plan, see `history/`
(one file per day, `YYYY-MM-DD.md`) — not a changelog, only entries worth
keeping.

## What exists

**No phase is open.** P1–P4 and P9 are complete and were struck at a keystone
review, so there is no per-phase checklist here. How the system works is
[`../design/architecture.md`](../design/architecture.md); what is still ahead
is [`../design/roadmap.md`](../design/roadmap.md), whose index table is the
schedule.

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
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables; twelve figures, each declaring what invalidates it and which documents repeat it |
| Predicate and projection pushdown; per-row-group statistics | not started — P5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign, sparse row index | not started — P7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

Last updated: 2026-08-28 — the keystone sweep, then the measurement sweep that
followed it. Every completed phase's spec and notes are struck and the
out-of-band ledger with them; `architecture.md` is the
single authority on how the built system works. **Choosing the next phase is a
re-grilling the maintainer has claimed**, so this is a phase boundary: an
unattended loop stops here.

**Every figure in
[`../design/measurements.md`](../design/measurements.md) comes from one sweep**,
taken against `ff9c8f3` on 2026-08-28 in 43 minutes and folded in whole:
`uv run measure.py --stale` reports no figure's declared paths touched, and
`--check` reconciles twelve markers against twelve figures. What moved, and
what it changed elsewhere: [`history/2026-08-28.md`](history/2026-08-28.md).

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; the resulting changes land as
  out-of-band items. Nothing is pooled here at present.

- **No out-of-band work is queued.** `M1`–`M19` are spent; the next item takes
  `M20` ([`../design/roadmap.md`](../design/roadmap.md), "Out-of-band work").

- **No phase is specified.** Four are sketched and none grilled — P5, P6, P7,
  P8 — and numeric order is not plan order, since P9 was taken ahead of P5.
  `process.md` step 6 re-grills the roadmap before the next phase is
  specified, and each of the four has an inbox that must be drained as part of
  that grilling.

## Known gaps

- **A `--disable-triggers` dump loses TOC attribution on every data span**,
  `COPY` and `INSERT` runs alike. I31: `pg_dump` writes `ALTER TABLE … DISABLE
  TRIGGER ALL;` — and `SET SESSION AUTHORIZATION DEFAULT;` ahead of the first
  entry — between each `-- Data for Name:` block and the data, so the comment
  is no longer adjacent to what it heads and closes as its own `Framing` span,
  which clears `governing_toc`. Measured against 16.15, two tables: TOC
  coverage 2/24, the two being the comment blocks. **Accepted, not a
  correctness hazard**: tiling stays byte-exact, the table name comes from the
  `COPY` header or the `INSERT INTO` line, the object census still reads
  `TABLE DATA: 2`, and roles still come off the entry's `Owner:`. What is lost
  is the coverage diagnostic and `Span::toc` on data spans. Opt-in and gated on
  `--data-only`/`--section=data`. Not fixed opportunistically because a fix is
  three coordinated changes, one of them to a decision `architecture.md`
  states — so by `roadmap.md`'s admission rule it is a graded slice, not a
  drive-by. Wanted but unscheduled, with the three changes named:
  [`../design/roadmap.md`](../design/roadmap.md), "Future — wanted,
  unscheduled".

- **An array nested inside a composite** is still decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode` naming the column. Deliberate and permanent as things
  stand: the census is keyed by column and has nowhere to record a shape at
  that depth, so scanning more of the file cannot help. `--schema-mode
  strings`, which the message now names, returns the literal verbatim. A
  *top-level* array column does not reach this — it is retyped from the census
  on any query, cold or full — and an array whose *element type* is an array
  does not either, since resolution refuses that shape outright (I26). Keying the census by path is a roadmap "Future" item and would be
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
  byte-poor dump pays quadratic CPU with the cache disabled entirely: 19.1 s
  for 4000 blocks under `query --dqcache none`, against under 10 ms for the
  same bytes in one block. 9.5's throttle removed the other half (45.5 s → 19.8
  s for a 4000-block `parse`), which leaves the map as **97%** of what a
  throttled `parse` now costs at that block count. Accepted for now, not scheduled: the fix is to
  stop rebuilding the span list per block, which is the same code P7's
  parallel-scan plans would rework and which
  [`roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md) already flags
  for assuming coverage is a contiguous prefix — so the two belong in one
  decision. A **cheap** version exists and was weighed: for `parse` nothing
  reads `index.spans` between saves, so gating the splice on the throttle the
  same way the save is gated would cost a few dozen splices instead of `n`
  (~19.1 s → ~1 s at 4000 blocks). It is not taken, because it would make an
  interrupt
  bank the last *saved* watermark rather than the last *completed block* —
  reversing a guarantee 9.5 established — and `Builder::snapshot` asserts
  `Idle`, so the chunk-top interrupt check cannot re-derive the spans mid-block
  to compensate. The analysis is in the P7 inbox so it is not re-derived. Nothing koji-shaped is affected: 74 blocks over 784 GB pay this
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
  [`roadmap-P6-embeddable-engine-inbox.md`](../design/roadmap-P6-embeddable-engine-inbox.md) so the
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
  the scanner level. Warm, on the same 3.00 GiB, that costs **16.2× the
  per-byte CPU of a `COPY` scan** — 8.26 s against 0.510 s, and 31× the device
  floor where the `COPY` path is 1.9×. Cold from this SSD the device hides most
  of it, at 1.71× the floor against the `COPY` path's 1.00×, which is why the
  ratio is quoted from the warm table and from one regime. Figures and re-run
  commands in
  [`../design/measurements.md`](../design/measurements.md). A
  koji-scale 1TB `--inserts` dump therefore spends ~43 minutes of CPU that a
  `COPY` dump of the same size does not. Correctness is unaffected — the map, the tiling and the row counts
  are the same either way. Not scheduled: the fix is a scanner-level
  `INSERT` path, which changes a decision and so needs a slice, filed into
  [`roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md).

## Decisions worth another look

Calls made without the maintainer present that are worth weighing in on —
cautionary and informational, not blocking. **An entry leaves this section once
it has been looked at**, settled into the design docs or reversed; the
reasoning that closed it lives in the dated history entry it names, and the
durable half in the doc that holds the decision. Two are open.

*An `INSERT` scan costs **16.2×** a `COPY` scan per byte, not the "~5×" three
documents carried — and what that changes about P7's plan has not been
grilled.* The number itself is folded in wherever it was repeated, since a
document must not keep asserting a measurement known to be wrong. The decision
behind it is untouched: the old ratio was the evidence for filing a
scanner-level `INSERT` path in
[`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md),
and a ratio three times larger — 1 TB of `INSERT` runs is ~43 minutes of CPU
against the `COPY` path's ~3 — may change where that sits in P7's order, or
whether it is P7's at all. That is a phase-planning question and it goes
through grilling, which is why the fold-in stopped here rather than
re-prioritising anything.

*The composite column's end-to-end share now separates from the instrument's
own floor, and the fold-in kept the old bound anyway.* This sweep reads the
composite column at **+0.99 µs/row** against a seed-43 floor of **−0.11**, and
their per-rep ranges do not overlap (+0.31…+1.24 against −0.41…+0.28) — where
the previous sweep read +0.39 against a +0.07 floor and overlapped it across
most of its width. Two takes of one quantity **0.6 µs/row apart** is wider than
the floor itself, so
[`../design/measurements.md`](../design/measurements.md) still reads that
subtraction as a bound rather than a resolution, and its standing rule still
puts the apparatus floor at ~0.5 µs/row. What is open is whether a third sweep
reproducing the separation retires that bound, and whether ~0.5 µs/row is the
right floor now that the instrument's own control spans −0.41 to +0.28. Both
are readings of a figure rather than values in it, which is why the fold-in did
not settle them.
