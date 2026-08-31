# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

P1–P5 and P9 are complete and were struck at keystone reviews; how each
mechanism works is [`../design/architecture.md`](../design/architecture.md),
filed by subject. **P11 — typed predicates — is open**: grilled and specified
([`../design/roadmap-P11-typed-predicates.md`](../design/roadmap-P11-typed-predicates.md)),
with nothing landed yet. Its checklist is below.

[`../design/measurements.md`](../design/measurements.md) carries the `b70589f`
stamp, and **one figure reads stale** — `session-drift`, named below with what
would settle it. A stale figure no longer obliges a sweep and neither does a
wrap: a full sweep is an hour of a quiet machine and belongs to the phase that
is about performance, which will re-take every table under its own apparatus
([`../design/measurements.md`](../design/measurements.md), "A stale figure does
not oblige a sweep").

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
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — thirteen figures, twelve taken by a sweep and one derived across two, each declaring what invalidates it and which documents repeat it. `measure.UNTAKEN` is empty: nothing is built and unrun |
| Column projection | working, library and CLI: `QueryOptions::projection` names columns, cuts the reported `ResolvedSchema` with the batches, may reorder, and may be empty (`COUNT(*)`); `pgdq query` spells it `--column <name>` repeated, or `--no-columns`, which prints no header so `\| wc -l` is a row count. A filter may name a column the projection does not, and an unprojected column is never decoded, so projecting a column away escapes its `Error::FieldDecode` — including `KD2`'s, which the error message does not name ([`../design/architecture.md`](../design/architecture.md), "Projection"; [`../manual/type-handling.md`](../manual/type-handling.md)). Measured on one 3.00 GiB file at five widths: `--no-columns` is 3.28 µs a row against 27.50 for all 19, the two array columns alone are +12.98 and the composite +0.77 ([`../design/measurements.md`](../design/measurements.md), "What a column costs") |
| Predicate conjunction | working, library and CLI: `QueryOptions::filters` is a list of single-column terms ANDed, the empty list being "no filter"; `pgdq query` spells it `--filter <term>` repeated. Nothing folds two terms, so a contradictory pair is a query with no rows. `OR` and `NOT` are not expressible — the NULL collapse that is sound under `AND` is not under `NOT` ([`../design/architecture.md`](../design/architecture.md), "Predicates") |
| The `--filter` term grammar | working, CLI only — `Predicate` is a struct an embedder fills in, so nothing below L4 parses a term. Whitespace outside quotes is trimmed on both sides of the operator; `'` and `"` both quote either side, matching pairs only, with an interior quote doubled; the operator split skips quoted regions, so a column named `a=b` is askable; and the `IS NULL` forms are the fallback, tried only on a term with no operator, which is what makes `note=this is null` the equality it reads as. A malformed quote is refused, never reinterpreted. `--column` and `--table` take their names verbatim and say so when a quoted-looking name is not found ([`../design/architecture.md`](../design/architecture.md), "A filter term is parsed for two audiences"; [`../manual/type-handling.md`](../manual/type-handling.md), "Writing a filter term") |
| Typed ordering operators (`<`, `<=`, `>`, `>=`) | working, library and CLI: each side is decoded with the column's own decoder — the field per row, the literal once when the block's schema resolves — and the decoded values compared, so `9 > 10` is true on an `integer`. Available on a column that resolved `Mapped` with a `Scalar` plan and refused on any other, which is also why `--schema-mode strings` refuses every one of them. An undecodable literal is `Error::PredicateValueDecode` before any row; a field that is genuinely undecodable is `Error::FieldDecode`, worded as the build path words it ([`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed") |
| PostgreSQL's special values under an ordering operator | answered exactly, not raised as a fault: `-infinity` below every finite value, `infinity` above, a `numeric`'s `NaN` above `infinity` and equal to itself (I34), each in the spelling its own type writes. Carried as a position in the order rather than as a number, since no Arrow type has one. **A filter is therefore exact where the batch still cannot hold the value** — the row `--filter 'v_date<2020-01-01'` selects for `-infinity` fails to build if `v_date` is projected, which is a property of two paths with different powers, not a defect (`KD8` is the materialization question). ([`../manual/type-handling.md`](../manual/type-handling.md)) |
| The ordering register | in code, as an exhaustive `match` over `DataType` in `predicate.rs`, and rendered as a table in [`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed". Nine of its rows agree with PostgreSQL (I33, I34); four diverge — every one of them reaching `Utf8View` or the enum `Dictionary`. A divergence is announced by `pgdq query` once on stderr, and read by an embedder from `TableStream::ordering_notes` — a third channel, since the signal is per-column *and* predicate-conditional (L4) |
| `object_store` I/O, Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign, sparse row index | not started — P7 |
| Per-row-group column statistics | not started — P10, which needs P7's sparse row index. `CopyBlock::column_stats` stays a reserved `None` |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

**Figures.** Every figure in
[`../design/measurements.md`](../design/measurements.md) comes from the
`b70589f` sweep of 2026-08-30, folded in whole, each table carrying an
apparatus line. `--check` reconciles thirteen markers against thirteen figures.
`session-drift` is derived across that sweep and a second one taken three
minutes later on the same commit, which is the pair `--drift` reads.
`measure.ACKNOWLEDGED` carries two entries, both with mechanical evidence
attached — see the staleness paragraph below.

**Nothing is built and unrun.** `measure.UNTAKEN` is empty: `projection-widths`
was taken and moved into `FIGURES`, and `composite-isolated` was deleted
unpublished along with its whole apparatus — the `--weak-composite` generator
flag, the `composite_text` input and the fidelity case pairing them — because
the projection table makes the same isolation a subtraction between two adjacent
rows of one file.

**Two commits since the stamp touched a declared path, and both are
acknowledged with mechanical evidence.** `8b97956` deleted
`composite-isolated`'s generator apparatus — the `--weak-composite` flag and
the `composite_text` input — and `--verify-additive` regenerates all five
surviving inputs at both revisions and finds them byte-identical, which covers
nine figures. `9ed21d4` landed the `--filter` term grammar in the CLI's
`main.rs`, which five figures declare, and **no registered command shape passes
`--filter`**, so none of it runs in a timed command; the one added function a
shape reaches is `quoted_name_note`, called once on `query-nomatch`'s
not-found path. Each entry carries the command that re-checks it.

**`session-drift` stays stale, and that is the honest state.** It declares
`scripts/measure.py`, which `8b97956` also edited to move `projection-widths`
into `FIGURES`, and the harness *is* the apparatus that figure measures — so
neither oracle applies and nothing but taking it settles it. It is derived
rather than measured, so `uv run measure.py --drift <sweep> <sweep>` re-derives
it from two sweeps' `raw.json` without measuring anything; what it lacks is a
pair taken past this commit. The scan-performance phase will supply one. One
red figure with its reason written here is a signal; eleven were not.

## P11 progress

Nothing has landed. The spec is
[`../design/roadmap-P11-typed-predicates.md`](../design/roadmap-P11-typed-predicates.md);
the slice order is fixed there and the boxes below are the only record of
progress.

- [ ] **11.1** The comparison oracle — per-major `(type, left, right, operator)`
      answer tables, generated by `scripts/generate_fixtures.py` and committed,
      recording whether the server accepted each input. No library code.
- [ ] **11.2** The cross-major differ — the differ, the committed differences
      file, the three-way coverage reconciliation, and the suite assertion that
      keeps the file from going stale.
- [ ] **11.3** The comparison plan moves to L2 — `ordering_register` out of
      `predicate.rs`, keyed on the declared type, carried in `ResolvedSchema`
      as a fourth positional vector. No answer changes.
- [ ] **11.4** Enum and bare `numeric` — declaration order for the enum;
      arbitrary-precision decimal carrying `Infinity`, `-Infinity` and `NaN`.
- [ ] **11.5** The text-held type queue — `interval` (with v17 infinities),
      `time with time zone`, `inet`/`cidr`/`macaddr`/`macaddr8`, `jsonb`.
- [ ] **11.6** Typed `=` / `!=` — routed through the comparison plan, with the
      canonicalize-the-literal-once fast path and its two exceptions;
      `ordering_notes` becomes `comparison_notes`.
- [ ] **11.7** Three-valued evaluation — `Expr`, the `True`/`False`/`Unknown`
      domain, `IS DISTINCT FROM`. Library only.
- [ ] **11.8** `--where` — the expression grammar in its own CLI module, leaf
      delegated to `parse_filter`; `--filter` unchanged.
- [ ] **11.9** The nested literal input grammar — the
      `array_in`/`record_in`/`range_in` superset, checked against the oracle's
      malformed cases. No comparison yet.
- [ ] **11.10** Nested structural comparison — element-wise, field-wise and
      bound-wise, the NULL rule, inherited comparability, range
      canonicalization, and paths in the comparison notes.

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **Four phases are sketched and none grilled** — P7, P10, P6, P8, in that
  schedule order — a `P<k>` is an identifier and the roadmap's table is the
  schedule, so the numbers say nothing about the order they run in.
  `process.md` step 6 re-grills the roadmap before the next phase is specified,
  and each of the four has an inbox that must be drained as part of that
  grilling.

## Known deficiencies

The deficiency register. Every known deficiency carries a stable `KD<k>`,
allocated on discovery and never reused, and **one line here**: what it costs,
its stance, and the file whose paragraph holds the rest. That paragraph sits
beside the mechanism, where `CLAUDE.md`'s read-triggers already send a session
that is about to touch it. This is an index, not the document.

Three stances, because these are not one kind of thing and the difference
decides whether anyone should act. **(a)** a consequence of a deliberate
tradeoff, never to be worked. **(b)** a defect with a known fix and a named
destination. **(c)** a defect with a known fix and no owner — a legitimate
resting state, said in those words, naming whatever would promote it. A
limitation whose remedy the user already has today is not here at all: it is a
property of how the system works, and it lives beside its mechanism with no
identifier.

A coverage statement is not a deficiency:
[`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)'s
`Unsupported` and `Untested` rows are scope and evidence, and earn a `KD<k>`
only by naming one.

`cd scripts && uv run deficiencies.py` reconciles this index against those
paragraphs and against the source-code markers, and fails on either half.

- **KD1** — a `--disable-triggers` dump loses TOC attribution on every data
  span, `COPY` and `INSERT` alike (I31), costing the coverage diagnostic and
  `Span::toc`. **(c) unowned**; promoted by a dump in hand whose data spans
  need attribution. Detail:
  [`../design/architecture.md`](../design/architecture.md), "TOC enrichment".

- **KD2** — an array nested inside a composite is decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode`. **(c) unowned**; promoted by a schema that holds one,
  the per-path census being deferred on frequency. Detail:
  [`../design/architecture.md`](../design/architecture.md), "What the census
  decides, and who may believe it".

- **KD3** — two array shapes come back as text with no way to ask for more,
  `NestedArrayElement` and `VaryingArrayShape`, though both are fully
  understood. **(c) unowned**; promoted by a caller whose arrays are matrices
  or scientific data, for whom a string is the wrong answer. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Joining a header
  against the metadata".

- **KD4** — a type name that needs quoting resolves `Unknown` (I29): a weaker
  type, never a wrong one. **(c) unowned**; promoted by a dump whose type names
  are not ordinary identifiers, which neither any fixture nor koji is. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Type resolution".

- **KD5** — mapping is O(blocks²): every `CopyEnd` rebuilds `DumpIndex::spans`
  whole, and the save throttle only halved the series. **(b) owned by P7**,
  whose parallel-scan plans rework the same code. Detail:
  [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md),
  "Mapping is O(blocks²) after the save throttle".

- **KD6** — a conflicting table past a query's stopping point is never seen, so
  `Error::AmbiguousTable` is not raised for it and the query returns the
  candidate it found. **(b) owned by P6**, where what the embedded API promises
  is decided. Detail:
  [`../design/architecture.md`](../design/architecture.md), "One target per
  query".

- **KD7** — four rows of the ordering register diverge from PostgreSQL under
  `<`/`>`: a bare `numeric`, text under any collation but `C`/`POSIX`, an enum,
  and every other text-held type. **(b) owned by P11**, which holds the
  per-type worklist. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Ordering operators
  compare typed".

- **KD8** — a typed column cannot hold `infinity`, `-infinity` or `NaN`, so
  materializing one raises `Error::FieldDecode` and there is no typed way to
  read the value. **(c) unowned**; promoted by whichever phase takes typed
  materialization, which is where the choice between a null, a sentinel and the
  error belongs. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Decoders and
  render-back".

- **KD9** — an `INSERT` run is folded into one span but every line is still
  decoded, at mid-teens times a `COPY` scan's per-byte CPU. **(b) owned by P7**, since
  the fix is a second scanner-level fast path. Detail:
  [`../design/roadmap-P7-scan-performance-inbox.md`](../design/roadmap-P7-scan-performance-inbox.md),
  "An `INSERT`-run scan is CPU-bound at mid-teens times a `COPY` scan's
  per-byte cost".

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

Nothing is open.
