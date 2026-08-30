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
- [ ] **P5.8** Special values are ordered — `infinity`/`-infinity`/`NaN`
      answered exactly rather than raising `FieldDecode`, which is kept for
      genuinely malformed text. Runs before `P5.7` so the sweep measures it.
- [ ] **P5.7** Take the figure, fold it in, re-read its consumers, delete
      `composite-isolated` and re-scope the cross-file figures it supersedes,
      update the manual — including `G2`'s second escape, which the error
      message does not name.

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

The gap register. Every known deficiency carries a stable `G<k>`, allocated on
discovery and never reused, and **one line here**: what it costs, its stance,
and the file whose paragraph holds the rest. That paragraph sits beside the
mechanism, where `CLAUDE.md`'s read-triggers already send a session that is
about to touch it. This is an index, not the document.

Three stances, because these are not one kind of thing and the difference
decides whether anyone should act. **(a)** a consequence of a deliberate
tradeoff, never to be worked. **(b)** a defect with a known fix and a named
destination. **(c)** a defect with a known fix and no owner — a legitimate
resting state, said in those words, naming whatever would promote it. A
limitation whose remedy the user already has today is not here at all: it is a
property of how the system works, and it lives beside its mechanism with no
identifier.

A coverage statement is not a gap either. [`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)
tracks which `pg_dump` variants have been exercised, and its `Unsupported` and
`Untested` rows are scope and evidence rather than deficiency — a row there
earns a `G<k>` only when it names one.

`cd scripts && uv run gaps.py` reconciles this index against those paragraphs
and against the source-code markers, and fails on either half.

- **G1** — a `--disable-triggers` dump loses TOC attribution on every data
  span, `COPY` and `INSERT` alike (I31), costing the coverage diagnostic and
  `Span::toc`. **(c) unowned**; promoted by a dump in hand whose data spans
  need attribution. Detail:
  [`../design/architecture.md`](../design/architecture.md), "TOC enrichment".

- **G2** — an array nested inside a composite is decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode`. **(c) unowned**; promoted by a schema that holds one,
  the per-path census being deferred on frequency. Detail:
  [`../design/architecture.md`](../design/architecture.md), "What the census
  decides, and who may believe it".

- **G3** — two array shapes come back as text with no way to ask for more,
  `NestedArrayElement` and `VaryingArrayShape`, though both are fully
  understood. **(c) unowned**; promoted by a caller whose arrays are matrices
  or scientific data, for whom a string is the wrong answer. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Joining a header
  against the metadata".

- **G4** — a type name that needs quoting resolves `Unknown` (I29): a weaker
  type, never a wrong one. **(c) unowned**; promoted by a dump whose type names
  are not ordinary identifiers, which neither any fixture nor koji is. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Type resolution".

- **G5** — mapping is O(blocks²): every `CopyEnd` rebuilds `DumpIndex::spans`
  whole, and the save throttle only halved the series. **(b) owned by P7**,
  whose parallel-scan plans rework the same code. Detail:
  [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md),
  "Mapping is O(blocks²) after the save throttle".

- **G6** — a conflicting table past a query's stopping point is never seen, so
  `Error::AmbiguousTable` is not raised for it and the query returns the
  candidate it found. **(b) owned by P6**, where what the embedded API promises
  is decided. Detail:
  [`../design/architecture.md`](../design/architecture.md), "One target per
  query".

- **G7** — four rows of the ordering register diverge from PostgreSQL under
  `<`/`>`: a bare `numeric`, text under any collation but `C`/`POSIX`, an enum,
  and every other text-held type. **(b) owned by P11**, which holds the
  per-type worklist. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Ordering operators
  compare typed".

- **G8** — a typed column cannot hold `infinity`, `-infinity` or `NaN`, so
  materializing one raises `Error::FieldDecode` and there is no typed way to
  read the value. **(c) unowned**; promoted by whichever phase takes typed
  materialization, which is where the choice between a null, a sentinel and the
  error belongs. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Decoders and
  render-back".

- **G9** — an `INSERT` run is folded into one span but every line is still
  decoded, at 14.4× a `COPY` scan's per-byte CPU. **(b) owned by P7**, since
  the fix is a second scanner-level fast path. Detail:
  [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md),
  "An `INSERT`-run scan is CPU-bound at 14.4× a `COPY` scan's per-byte cost".

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

- **`G6` is filed as a gap rather than evicted as a property.** Building the
  register applied one test to every entry — is the remedy already available to
  the user today? — and it is what turned `DumpIndex::roles`/`tablespaces`
  into a property with no identifier, since a full scan gives the complete set.
  `G6` carries the same remedy sentence, `ScanExtent::Full` or a query after
  `pgdq parse`, and was kept anyway: what it costs is not a partial answer the
  user can ask for again, it is a possibly *wrong* one with no signal at all,
  and the cheap fix P6 is weighing — a `Diagnostic` on every early stop — does
  not exist today. Read the other way, the index line is deleted, the paragraph
  stays where it is in
  [`../design/architecture.md`](../design/architecture.md)'s "One target per
  query", and P6's inbox entry carries the question by itself.

The entry `P5.6` raised was answered and reversed: special values are ordered,
settled in
[`../design/roadmap-P5-pushdown.md`](../design/roadmap-P5-pushdown.md) and
scheduled as `P5.8` ([`history/2026-08-30.md`](history/2026-08-30.md)).
