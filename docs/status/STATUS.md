# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

**No phase is open.** P1–P4 and P9 are complete and were struck at a keystone
review, so there is no per-phase checklist here.

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<`range struct`>`, `List<List<T>>`. Three shapes stay strings, each with its own resolution outcome — an opaque element type, an element type that is itself an array (I26), and values that disagree on shape |
| Array shape census | recorded by every mapping pass and consumed: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, cache-only `info` | working; **`parse` is the only scanner** — it resumes from a matching cache, banks at `COPY` block boundaries under a self-tuning throttle, and saves unconditionally on Ctrl-C (exit 130/143). `info` reports from the cache and never scans. Text output shape is provisional; `--json` carries no shape promise at all |
| Partial reporting | `info` reports an unfinished scan's cache for as far as it got, with `Scan completion: N%` stated once at the top and nothing below it qualified. An interrupted cache is **typed** for every database segment the scan finished (I1) |
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — twelve figures, eleven taken by a sweep and one derived across two, each declaring what invalidates it and which documents repeat it, plus one instrument built and not taken |
| Predicate and projection pushdown; per-row-group statistics | not started — P5 |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign, sparse row index | not started — P7 |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

**Figures.** Every figure in
[`../design/measurements.md`](../design/measurements.md) comes from the
`fa186ab` sweep of 2026-08-28, folded in whole, each table carrying an
apparatus line witnessing a quiet machine. **Nothing is owed a fold-in**;
`--check` reconciles twelve markers against twelve figures and `--stale`
reports nothing outstanding.

**One instrument is built and deliberately unrun.** `M23`'s
`composite-isolated` isolates the composite column's decode cost by declaring
one column two ways over byte-identical rows, which removes the normalization
that gives the cross-file subtraction its ~0.5 µs/row floor. It is registered
under `measure.UNTAKEN` — a sweep does not take it and the doc carries no table
for it — so the standing figure keeps reading that cost as a bound. Take it
with `uv run measure.py --figure composite-isolated` on an idle machine, and
move the entry into `FIGURES` when publishing it.

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **No out-of-band work is queued.** `M1`–`M23` are spent; the next item takes
  `M24` ([`../design/roadmap.md`](../design/roadmap.md), "Out-of-band work").
- **No phase is specified.** Four are sketched and none grilled — P5, P6, P7,
  P8 — and numeric order is not plan order, since P9 was taken ahead of P5.
  `process.md` step 6 re-grills the roadmap before the next phase is
  specified, and each of the four has an inbox that must be drained as part of
  that grilling.

## Known gaps

Deficiencies that are known and **accepted**. Each says what is lost, why that
is safe, and where the mechanism or the fix is written down.

- **A `--disable-triggers` dump loses TOC attribution on every data span**,
  `COPY` and `INSERT` alike (I31). Not a correctness hazard: tiling stays
  byte-exact, the table name still comes from the `COPY` header or the `INSERT
  INTO` line, the object census still counts the entry, and roles still come
  off its `Owner:`. What is lost is the coverage diagnostic and `Span::toc` on
  data spans. Opt-in and gated on `--data-only`/`--section=data`. Mechanism:
  [`../design/architecture.md`](../design/architecture.md), "TOC enrichment";
  the fix is three coordinated changes and is named in
  [`../design/roadmap.md`](../design/roadmap.md), "Future — wanted,
  unscheduled".

- **An array nested inside a composite** is decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode` naming the column. Permanent as things stand: the census
  is keyed by column and has nowhere to record a shape at that depth, so
  scanning more of the file cannot help. `--schema-mode strings`, which the
  message names, returns the literal verbatim. A *top-level* array column does
  not reach this, and neither does an array whose element type is an array
  (refused outright, I26). Keying the census by path is a roadmap "Future"
  item and purely additive.

- **Two array shapes come back as text with no way to ask for more** —
  `NestedArrayElement` and `VaryingArrayShape`. Neither is opaque: both are
  fully understood, and one lossless representation would cover both. It is a
  roadmap "Future" item, and adding it only ever touches columns these
  refusals leave as `Utf8View`.

- **A type name that needs quoting resolves `Unknown`** (I29): `parse_ident`
  dequotes it into `TypeDef.name` while the declaration keeps its quotes, so
  the lookup misses. **No spelling is misread** — a column of a type named
  `"x ARRAY"` is not mistaken for an array — so the cost is a weaker type,
  never a wrong one, and every value still decodes as the text the file holds.
  Unreachable from any dump whose type names are ordinary identifiers, which is
  every fixture and the koji sample. Fix: the roadmap's "A real type-name
  tokenizer", strictly additive.

- **Mapping is O(blocks²), and the save throttle only halved it.** Every
  `CopyEnd` rebuilds `DumpIndex::spans` whole, so a block-rich, byte-poor dump
  pays quadratic CPU with the cache disabled entirely — 19.1 s for 4000 blocks,
  which is 97% of what a throttled `parse` of the same file costs. Nothing
  koji-shaped is affected: 74 blocks over 784 GB pay it 74 times, at +1.5%.
  Not scheduled, because the fix is the same code P7's parallel-scan plans
  would rework — including a cheap variant that was weighed and refused for
  reversing the interrupt guard's guarantee. The full analysis, so it is not
  re-derived:
  [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md).
  Figures: [`../design/measurements.md`](../design/measurements.md),
  "Per-block cache saving is quadratic in block count".

- **A conflicting table past a query's stopping point is never seen**, so
  `Error::AmbiguousTable` is not raised for it and the query returns the
  candidate it found. The stop rule rules out the two shapes that announce
  themselves — a partition-root marker (I2) and any `\connect` at all — leaving
  one undetectable case: a plain dump with something concatenated after it.
  `ScanExtent::Full`, or a query after `pgdq parse`, gives exact detection.
  Rows are never a union either way, and ambiguity is raised before any row is
  emitted. Accepted because closing it means abandoning early stopping, which
  is what makes a cold query on a large dump affordable; filed into
  [`../design/roadmap-P6-embeddable-engine-inbox.md`](../design/roadmap-P6-embeddable-engine-inbox.md)
  so the embedded API's promises are decided against it deliberately.

- **`DumpIndex::roles`/`tablespaces` are complete only once the scan reaches
  the file's size** — the same partiality `metadata`'s `preamble_complete`
  carries, for the same reason. koji's `backup` role is the motivating case.
  `ScanExtent::Full`, or a query after `pgdq parse`, gives the complete set.

- **An `INSERT` run is folded into one span but every line is still decoded**,
  unlike the large-object region, which is skipped unread. That costs **14.6×**
  the per-byte CPU of a `COPY` scan warm — ~43 minutes for a koji-scale 1 TB
  `--inserts` dump against the `COPY` path's ~3. Correctness, tiling and row
  counts are unaffected. Not scheduled: the fix is a scanner-level `INSERT`
  path, which changes a decision and so needs a slice, filed into
  [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md).

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

**None are open.**
