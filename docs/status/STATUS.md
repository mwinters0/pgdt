# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

P1–P4 and P9 are complete and were struck at a keystone review. **P5 is open**
and six slices of it have landed — one registering a figure and adding no
library code, one bounding what an in-flight batch pins, one adding projection
to the library, one giving it a CLI, one turning the single filter into a
conjunction, and one adding the typed ordering operators. What is left is
taking the figure and retiring what it supersedes; the checklist is below the
table.

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working; a batch flushes on whichever of `max_rows`, `max_bytes` or `max_source_span` comes first, the last of which is what bounds the read chunks an in-flight batch pins ([`../design/architecture.md`](../design/architecture.md), "Three flush triggers") |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<`range struct`>`, `List<List<T>>`. Three shapes stay strings, each with its own resolution outcome — an opaque element type, an element type that is itself an array (I26), and values that disagree on shape |
| Array shape census | recorded by every mapping pass and consumed: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, cache-only `info` | working; **`parse` is the only scanner** — it resumes from a matching cache, banks at `COPY` block boundaries under a self-tuning throttle, and saves unconditionally on Ctrl-C (exit 130/143). `info` reports from the cache and never scans. Text output shape is provisional; `--json` carries no shape promise at all |
| Partial reporting | `info` reports an unfinished scan's cache for as far as it got, with `Scan completion: N%` stated once at the top and nothing below it qualified. An interrupted cache is **typed** for every database segment the scan finished (I1) |
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — twelve figures, eleven taken by a sweep and one derived across two, each declaring what invalidates it and which documents repeat it, plus two instruments built and not taken |
| Column projection | working, library and CLI: `QueryOptions::projection` names columns, cuts the reported `ResolvedSchema` with the batches, may reorder, and may be empty (`COUNT(*)`); `pgdq query` spells it `--column <name>` repeated, or `--no-columns`, which prints no header so `\| wc -l` is a row count. A filter may name a column the projection does not, and an unprojected column is never decoded, so projecting a column away escapes its `Error::FieldDecode` ([`../design/architecture.md`](../design/architecture.md), "Projection") |
| Predicate conjunction | working, library and CLI: `QueryOptions::filters` is a list of single-column terms ANDed, the empty list being "no filter"; `pgdq query` spells it `--filter <term>` repeated. Nothing folds two terms, so a contradictory pair is a query with no rows. `OR` and `NOT` are not expressible — the NULL collapse that is sound under `AND` is not under `NOT` ([`../design/architecture.md`](../design/architecture.md), "Predicates") |
| Typed ordering operators (`<`, `<=`, `>`, `>=`) | working, library and CLI: each side is decoded with the column's own decoder — the field per row, the literal once when the block's schema resolves — and the decoded values compared, so `9 > 10` is true on an `integer`. Available on a column that resolved `Mapped` with a `Scalar` plan and refused on any other, which is also why `--schema-mode strings` refuses every one of them. An undecodable literal is `Error::PredicateValueDecode` before any row; an undecodable field is `Error::FieldDecode`, worded as the build path words it ([`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed") |
| The ordering register | in code, as an exhaustive `match` over `DataType` in `predicate.rs`, and rendered as a table in [`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed". Nine of its rows agree with PostgreSQL (I33); four diverge — every one of them reaching `Utf8View` or the enum `Dictionary`. A divergence is announced by `pgdq query` once on stderr, and read by an embedder from `TableStream::ordering_notes` — a third channel, since the signal is per-column *and* predicate-conditional (L4) |
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

**Eight figures read stale** — `census-brace-free`, `census-arrays`,
`scan-throughput-cold`, `scan-throughput-warm`, `nested-end-to-end`,
`census-attribution`, `cross-file-floor` and `map-only` — since `P5.2`
through `P5.6` between them touched `batch.rs`, `stream.rs` and the CLI,
which all eight declare. None is acknowledgeable: a library change has no cheap
oracle, and `P5.3` puts a per-field lookup on the replay path. They stay stale
until the next full sweep, which is `P5.7`'s neighbourhood.

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
  subtraction between two adjacent rows. Its command shapes are executable
  since `P5.4` and the suite runs all five against a generated input; `P5.7`
  takes it and folds it in
  ([`../design/roadmap-P5-pushdown.md`](../design/roadmap-P5-pushdown.md)).

## P5 progress

The spec is
[`../design/roadmap-P5-pushdown.md`](../design/roadmap-P5-pushdown.md).

- [x] **P5.1** Register the figure — five projection widths over the existing
      19-column input, under `measure.UNTAKEN`. No generator change, no
      library code. Notes:
      [`../design/roadmap-P5.1-projection-figure-notes.md`](../design/roadmap-P5.1-projection-figure-notes.md)
- [x] **P5.2** The source-span flush trigger — bounds what an in-flight batch
      pins. Notes:
      [`../design/roadmap-P5.2-source-span-flush-notes.md`](../design/roadmap-P5.2-source-span-flush-notes.md)
- [x] **P5.3** Projection in the library — the `QueryOptions` field and API
      move, the projected `ResolvedSchema`, `push_row` skipping, the
      zero-column `RecordBatch`. Notes:
      [`../design/roadmap-P5.3-projection-notes.md`](../design/roadmap-P5.3-projection-notes.md)
- [x] **P5.4** The CLI for projection — `--column`, `--no-columns`. Notes:
      [`../design/roadmap-P5.4-projection-cli-notes.md`](../design/roadmap-P5.4-projection-cli-notes.md)
- [x] **P5.5** The filter conjunction — repeatable `--filter`, terms ANDed.
      Notes:
      [`../design/roadmap-P5.5-filter-conjunction-notes.md`](../design/roadmap-P5.5-filter-conjunction-notes.md)
- [x] **P5.6** Typed ordering operators — `<`, `<=`, `>`, `>=`, and the refusal
      on a column that is not `Mapped` with a `Scalar` plan. Notes:
      [`../design/roadmap-P5.6-ordering-operators-notes.md`](../design/roadmap-P5.6-ordering-operators-notes.md)
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
  scanning more of the file cannot help. There are now two escapes:
  `--schema-mode strings`, which the message names, returns the literal
  verbatim for the whole table, and — since P5.3 — **not projecting the column
  leaves every other column typed**, because an unprojected column is never
  decoded ([`../design/architecture.md`](../design/architecture.md),
  "Projection"). Neither escape survives *filtering* on the column with an
  ordering operator: that decodes the field itself, so the same
  `Error::FieldDecode` comes back. The message still names only the first; the manual pass in
  `P5.7` is where the second gets written down for users. A *top-level* array
  column does not reach this, and neither does an array whose element type is
  an array (refused outright, I26). Keying the census by path is a roadmap
  "Future" item and purely additive.

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

- **Three kinds of column order differently from PostgreSQL under `<`/`>`.**
  A bare `numeric` (and one past `Decimal256`'s 76 digits) is `Mapped` to
  `Utf8View`, so `v > 5` compares text and `"9" < "10"` is false; a text
  column compares bytewise, which is the server's answer only under
  `C`/`POSIX` (I32); an enum compares by label text where PostgreSQL uses
  declaration order (I33). Every other text-held type — `interval`,
  `time with time zone`, `json`/`jsonb`, the network types — is in the same
  position. Equality is unaffected and every value still decodes as the text
  the file holds. **Each of these announces itself**: `pgdq query` names the
  column and the divergence on stderr, and an embedder reads
  `TableStream::ordering_notes`. Register:
  [`../design/architecture.md`](../design/architecture.md), "Ordering
  operators compare typed"; per-type worklist:
  [`../design/roadmap-P11-typed-predicates-inbox.md`](../design/roadmap-P11-typed-predicates-inbox.md).

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

### An ordering filter errors on a value that contradicts its DDL, rather than excluding the row

**The call.** `--filter 'v > x'` decodes each row's `v` with the column's own
decoder. When a value does not decode — `infinity` in a `date` column, `NaN`
in a `numeric(p,s)` one — `P5.6` raises `Error::FieldDecode`, the same error
with the same wording the typed *build* path raises for that value. The
alternative is to treat the row as not matching, which is the collapse a NULL
already gets.

**Why this way.** A NULL is a value the file states; a value that does not
decode is the file contradicting its own DDL, and everywhere else in this
system that is an error rather than a quiet exclusion. Excluding would also
mean a filter silently reporting fewer rows on a damaged file, which is the
shape of answer the project refuses. The spec settles neither way.

**What changes if reconsidered.** Some queries that error today would return
rows instead — concretely, any ordering filter on a `date`/`timestamp` column
holding `infinity`, which is legal PostgreSQL and appears in the `types`
fixture. Reversing it is a two-line change in `Predicate::matches` and would
retire `Error::FieldDecode`'s only non-`batch.rs` producer. It would also make
`--column`'s decode escape apply to filters, which today it deliberately does
not ([`../design/architecture.md`](../design/architecture.md), "Ordering
operators compare typed").
