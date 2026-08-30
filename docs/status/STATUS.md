# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

P1–P4 and P9 are complete and were struck at a keystone review. **P5 is open**
and one slice of it has landed, which registers a figure and adds no library
code; its checklist is below the table.

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
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — twelve figures, eleven taken by a sweep and one derived across two, each declaring what invalidates it and which documents repeat it, plus two instruments built and not taken |
| Column projection; richer predicates (conjunction, typed ordering) | not started — P5, specified |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign, sparse row index | not started — P7 |
| Per-row-group column statistics | not started — P10, which needs P7's sparse row index. `CopyBlock::column_stats` stays a reserved `None` |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

**Figures.** Every figure in
[`../design/measurements.md`](../design/measurements.md) comes from the
`4c2c3e7` sweep of 2026-08-30, folded in whole, each table carrying an
apparatus line. `--check` reconciles twelve markers against twelve figures.
`session-drift` is derived across that sweep and a second one taken two minutes
later on the same commit, which is the pair `--drift` reads.

**Two instruments are built and unrun**, both registered under
`measure.UNTAKEN` — a sweep takes neither and the doc carries no table for
either, so `--figure <id>` is the only way to reach one.

- `composite-isolated` isolates the composite column's decode cost by declaring
  one column two ways over byte-identical rows, which removes the normalization
  that gives the cross-file subtraction its floor. It will not be published:
  the standing figure keeps reading that cost as a bound, and **P5 supersedes
  it** — with column projection the same isolation is a subtraction between two
  widths of one file, needing no second file and no cross-file floor, so `P5.7`
  deletes it.
- `projection-widths` is that replacement: one file at five projection widths,
  warm and typed, where every attribution the cross-file apparatus makes is a
  subtraction between two adjacent rows. **It cannot be taken until `P5.4`**,
  because its command shapes name `--column` and `--no-columns`; `P5.7` takes
  it and folds it in
  ([`../design/roadmap-P5-pushdown.md`](../design/roadmap-P5-pushdown.md)).

## P5 progress

The spec is
[`../design/roadmap-P5-pushdown.md`](../design/roadmap-P5-pushdown.md).

- [x] **P5.1** Register the figure — five projection widths over the existing
      19-column input, under `measure.UNTAKEN`. No generator change, no
      library code. Notes:
      [`../design/roadmap-P5.1-projection-figure-notes.md`](../design/roadmap-P5.1-projection-figure-notes.md)
- [ ] **P5.2** The source-span flush trigger — bounds what an in-flight batch
      pins, closing the known gap below.
- [ ] **P5.3** Projection in the library — the `QueryOptions` field and API
      move, the projected `ResolvedSchema`, `push_row` skipping, the
      zero-column `RecordBatch`.
- [ ] **P5.4** The CLI for projection — `--column`, `--no-columns`.
- [ ] **P5.5** The filter conjunction — repeatable `--filter`, terms ANDed.
- [ ] **P5.6** Typed ordering operators — `<`, `<=`, `>`, `>=`, and the refusal
      on a column that is not `Mapped` with a `Scalar` plan.
- [ ] **P5.7** Take the figure, fold it in, re-read its consumers, delete
      `composite-isolated` and re-scope the cross-file figures it supersedes,
      update the manual.

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **Four phases are sketched and none grilled** — P6, P11, P7, P10, P8, in that
  schedule order. Numeric order is not plan order: P9 was taken ahead of P5,
  P10 was allocated when P5's grilling split statistics out of it, and P11 when
  the same grilling deferred full boolean structure and typed nested
  comparison. `process.md` step 6 re-grills the roadmap before the next phase is
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
  pays quadratic CPU with the cache disabled entirely — 20.9 s for 4000 blocks,
  which is all but a fraction of what a throttled `parse` of the same file
  costs. Nothing koji-shaped is affected: 74 blocks over 784 GB pay it 74 times, at +1.5%.
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

- **A bare `numeric` column will order lexicographically.** `map_numeric`
  returns `Utf8View` when the declaration carries no typmod (and when the
  precision exceeds `Decimal256`'s 76 digits), and that column still resolves
  `Mapped` — so once P5's ordering operators exist, `v > 5` on an
  unconstrained `numeric` compares text and `"9" < "10"` is false. Equality is
  unaffected, and every value still decodes as the text the file holds. It is
  the register row a user is likeliest to hit without suspecting anything,
  which is why the CLI announces it; closing it needs an arbitrary-precision
  decimal comparison and nothing external is missing. Register and remedy:
  [`../design/roadmap-P5-pushdown.md`](../design/roadmap-P5-pushdown.md), the
  ordering register; worklist:
  [`../design/roadmap-P11-typed-predicates-inbox.md`](../design/roadmap-P11-typed-predicates-inbox.md).

- **An aggressive filter pins read chunks in proportion to `1/selectivity`.**
  The `Utf8View` path gives `StringViewBuilder::append_block` a clone of the
  read chunk's Arrow `Buffer`, so the **in-flight batch** holds every chunk it
  took a view into until it flushes; the `chunks` deque's own eviction at the
  scanner position cannot release them. Neither flush trigger bounds it —
  `max_rows` counts *selected* rows and `max_bytes` counts *selected* field
  bytes — so with the default 1 MiB chunks a 1%-selective filter holds on the
  order of 80 MiB and a 0.01%-selective one on the order of 8 GiB. Correctness
  is unaffected; what is lost is the flat-memory goal, and only for a query
  that filters hard. A caller can bound it today by lowering `max_rows` or
  `ScanOptions::chunk_size`. The fix is a third flush trigger on the source
  byte span a batch covers, specified in
  [`../design/roadmap-P5-pushdown.md`](../design/roadmap-P5-pushdown.md).

- **An `INSERT` run is folded into one span but every line is still decoded**,
  unlike the large-object region, which is skipped unread. That costs **14.4×**
  the per-byte CPU of a `COPY` scan warm — ~40 minutes for a koji-scale 1 TB
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

*Nothing is open.* The two entries the 2026-08-30 sweep pair raised are closed:
the floor check is now stated directionally and inside a tolerance, and the one
open spec that repeated a figure's number no longer does
([`history/2026-08-30.md`](history/2026-08-30.md)).
