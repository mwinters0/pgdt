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
with the comparison oracle, its cross-major differ, the fixture family's move
to glibc, the comparison plan's move to L2, the register-to-oracle
reconciliation, the declared collation and the fixture columns that observe
it — the displaced clause included — the enum and bare `numeric`, the
whole text-held type queue, `jsonb` included, `character(n)`'s trim and typed
`=`/`!=` landed. Its checklist is below.

[`../design/measurements.md`](../design/measurements.md) carries the `b70589f`
stamp, and **`uv run measure.py --stale` names seven of its thirteen figures**
— the paragraphs below name each and what would settle it. A stale figure no longer obliges a sweep and neither does a
wrap: a full sweep is an hour of a quiet machine and belongs to the phase that
is about performance, which will re-take every table under its own apparatus
([`../design/measurements.md`](../design/measurements.md), "A stale figure does
not oblige a sweep").

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working; a batch flushes on whichever of `max_rows`, `max_bytes` or `max_source_span` comes first, the last of which is what bounds the read chunks an in-flight batch pins ([`../design/architecture.md`](../design/architecture.md), "Three flush triggers") |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working. `oid` is `UInt32` — PostgreSQL's one unsigned integer type, mapped where the ADBC driver's `Int32` turns an OID at or above 2^31 negative. A `uuid` column's field carries the canonical `arrow.uuid` extension name and a `json`/`jsonb` column's `arrow.json`, top level only and written through arrow-rs's own extension types, so neither changes a byte ([`../design/architecture.md`](../design/architecture.md), "Type resolution") |
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
| Typed `=` / `!=` | working, library and CLI: they route through the same per-column `ComparisonPlan`, so `--filter 'price=1.5'` matches a `numeric(10,2)` written `1.50` and `--filter 'code=ab'` matches a padded `char(10)`. **Three canonicalizations**: the literal rendered once into the file's `*_out` form (everything not below — `=` on a `text` column is the byte comparison it always was), the field narrowed per row (`character(n)`), and both sides decoded per row for the seven kinds where the file's spelling is not unique or where reproducing `*_out` would mean re-implementing an output function — bare `numeric`, `interval`, `jsonb`, `real`/`double precision`, `time with time zone`, `inet`/`cidr`. Equality is never *refused* on a column the register does not compare; it falls back to text, which is right for a nested column and a guess for an unmodelled scalar (`KD10`). A literal that is not a value of the column's type is `Error::PredicateValueDecode` before any row, on the same output-form-only grammar, and the refusal names the form that column's comparison reads rather than only the value it turned down — a `boolean` is written `t` or `f` ([`../design/architecture.md`](../design/architecture.md), "Equality is typed too"; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings") |
| Seven string-shaped types that still order typed | working: `interval` by `interval_cmp_value`'s 128-bit span, so `1 mon`, `30 days` and `720:00:00` are one bound; `time with time zone` by the UTC instant then the stored zone; `inet`/`cidr` by family, shorter prefix, netmask, address; `macaddr`/`macaddr8` by their octets (I40); and `jsonb` by `compareJsonbContainers`' walk down two documents — kind before value, a container's size before its members, and a top-level scalar inside the pseudo-array that makes it outrank `[]` (I41). None has an Arrow type, so the *ordering* is the only path that decodes them. The first six read a literal in the type's own `*_out` spelling and no wider — `1 month` and `08-00-2b-01-02-03` are refused by name, which is a property rather than a deficiency since every value a dump holds is already in the accepted form. `jsonb` is the exception and takes the whole of `jsonb_in`, because `{"a":1}` is what a person types and `{"a": 1}` is what the file holds ([`../manual/type-handling.md`](../manual/type-handling.md), "Seven string-shaped types still order the way PostgreSQL orders them") |
| PostgreSQL's special values under an ordering operator | answered exactly, not raised as a fault: `-infinity` below every finite value, `infinity` above, a `numeric`'s `NaN` above `infinity` and equal to itself (I34), each in the spelling its own type writes — `date` writes `infinity` and `numeric` writes `Infinity`, and neither answers to the other's. A **bare** `numeric` carries all three; one with a typmod carries only `NaN`, since any typmod rejects an infinity, so the two infinity spellings are refused there as a filter literal. Carried as a position in the order rather than as a number, since no Arrow type has one. **A filter is therefore exact where the batch still cannot hold the value** — the row `--filter 'v_date<2020-01-01'` selects for `-infinity` fails to build if `v_date` is projected, which is a property of two paths with different powers, not a defect (`KD8` is the materialization question). ([`../manual/type-handling.md`](../manual/type-handling.md)) |
| The comparison register | **L2**, in `pgtype.rs`: `comparison_for(declared, collation, types)` answers a `ComparisonPlan` per **column**, carried as `ResolvedSchema::comparisons` and rendered as a table in [`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed". `predicate.rs` reads the plan and names no `DataType`. Seventeen of its twenty-two rows agree with PostgreSQL (I33, I34, I37, I38, I40, I41); five diverge, and only one of the five is a deficiency — a column that *states* a collation this build does not implement (`KD7`). The other four are properties: a collation no plain dump records, reached through a column and through a `jsonb` string leaf, and `json`, which the server does not order at all. **A divergence is operator-conditional**: `ComparisonDivergence::affects_equality` answers `true` only for `json` and for `KD10`'s unmodelled scalar, every libc collation being deterministic — so a `text` column with no clause warns under `<` and is silent under `=`. The plan is `Clone` rather than `Copy`, because two of its comparisons carry a fact about the column: an enum's labels, and whether a `numeric`'s typmod excludes the infinities. Exhaustiveness is `builtin_scalar` answering the Arrow type and the comparison in one arm, plus a wildcard-free `match` over `TypeKind`. A divergence is announced by `pgdq query` once on stderr, and read by an embedder from `TableStream::comparison_notes` — a third channel, since the signal is per-**term** *and* predicate-conditional (L4) |
| The declared collation | read, and it moves the verdict rather than the comparison: `ColumnDef::collation` keeps a column's `COLLATE` clause verbatim (`pg_catalog."C"`), `TypeKind::Domain` keeps a domain's own as the type default a column-level clause overrides, and the register answers **agrees** for an explicit `C`/`POSIX` and for a bare `name` column, **diverges** for any other stated collation and for a `text`/`varchar` column with no clause at all (I32, I37) — a user-defined collation the same dump declares `locale = 'C'` included, which is correct rows plus a note the user can ignore, and so a property rather than a `KD<k>`. The clause is found wherever `pg_dump` displaced it, past a `DEFAULT`, a `GENERATED … STORED` expression and a `NOT NULL`, which `fixtures/<13–18>/types/default.sql` now carries. `character(n)` is the fourth collatable arm and reaches the same three verdicts: `CompareKind::PaddedText` takes the dump's blank padding off both sides first, which is `bcTruelen` (I38), and the clause then decides exactly as it does for `text` — so a `char(n)` declaring `COLLATE "C"` agrees ([`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise") |
| Comparison oracle | `fixtures/<13–18>/oracle/` holds what PostgreSQL itself answers for 2020 typed comparisons and 317 literals per major, each cell recording whether the server *accepted* the input, generated by `scripts/generate_fixtures.py --skip-dumps` and committed ([`../design/architecture.md`](../design/architecture.md), "The comparison oracle"). The answers are **glibc's** — every fixture container is the Debian (`-trixie`) image — and every text pair is asked twice, under `COLLATE "C"` and under the database's own collation, so both halves of the register's text row are in the file rather than argued. `character(10)` is asked under both too, with a tab-bearing value that puts I38's ordering corollary in the file ahead of 11.6, and a case's `collation` now means one thing only: `None` says the register does not branch on the clause for that type. A pair is asked through two *columns* of the declared type, so a bare case measures what a bare column in a dump does: `name`'s cells are its own `C` type default, and `public.text_c` — the domain whose own DDL carries the clause — is in the file beside it on the `user/Domain` arm. A literal the server refuses therefore never reaches an operator, and answers all six cells with the rejection `literals.tsv` records for it. `jsonb`'s sixteen values are one per branch of `compareJsonbContainers` — both booleans, a string, both empty containers, a one-pair object whose key sorts after a two-pair object's, and the pair separating storage order from alphabetical — so I41's facts are in the file rather than in a probe |
| Register against the oracle's answers | working: `the_register_answers_every_committed_oracle_cell`, a unit test in `predicate.rs`, puts every committed cell to the register through the same `resolve_term`/`Predicate::matches` path a `--filter` takes — **45,394 cells over six majors, all six operators**. Its one-column schema is built by `resolve_columns` from a synthetic `DumpMetadata`, so resolution, nested plan and comparison plan agree the way they do in a real query. It skips an `E`-cell (what the server refused), a NULL right operand (which the filter grammar cannot spell), and — *for the four ordering operators only* — a column the register refuses an ordering operator on, whose set is asserted exactly; `=`/`<>` are still asked of those columns, because equality is never refused. **Thirty-four cases are permitted to disagree and every one of them does, in every cell its divergence reaches**: a `jsonb` string leaf (2) and `text` under glibc's `en_US.utf8` (30), one statement at two depths, over 24 ordering cells each; and `box`'s area equality (2) over 6 `=` cells, PostgreSQL defining no `box <> box`. A disagreeing term must announce a `ComparisonNote` **under that operator** ([`../design/architecture.md`](../design/architecture.md), "The register against the oracle's answers") |
| Cross-major differ | working: `scripts/oracle_differences.py` walks the majors as a chain of adjacent pairs and files every cell that moved in `fixtures/oracle-differences.tsv` — **533 differences across 13–18, every one of them additive** (I35), so the union rule is checked rather than asserted. `test_oracle_differences.py` asserts the committed file against a fresh computation and, separately, that no difference is non-additive; an oracle pass of `generate_fixtures.py` ends by running the same check ([`../design/architecture.md`](../design/architecture.md), "The cross-major differ") |
| Register-to-oracle reconciliation | working: `scripts/oracle_register.py` reads the register's arms out of `pgtype.rs` — one per declared base name in `builtin_scalar`, one per `TypeKind` match arm in `comparison_user_type`, the three branches of the walk that are not match arms, and the three branches of `collated_text` — and joins them against the case table both ways, failing on either. **37 arms, 54 cases, nothing uncovered and nothing unplaced.** The collation is a second dimension: a case's label picks the arm, `C` reaching the bytewise branch and `default` the other two, and the `datcollate` that makes that mapping sound is read out of `meta.tsv` rather than assumed. An oracle pass of `generate_fixtures.py` ends by running it beside the differ ([`../design/architecture.md`](../design/architecture.md), "The register-to-oracle reconciliation") |
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
`measure.ACKNOWLEDGED` carries five entries, each with mechanical evidence
attached — see the staleness paragraphs below.

**Nothing is built and unrun.** `measure.UNTAKEN` is empty: `projection-widths`
was taken and moved into `FIGURES`, and `composite-isolated` was deleted
unpublished along with its whole apparatus — the `--weak-composite` generator
flag, the `composite_text` input and the fidelity case pairing them — because
the projection table makes the same isolation a subtraction between two adjacent
rows of one file.

**Five commits since the stamp touched a declared path, and all five are
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
pair taken past this commit. The scan-performance phase will supply one.

**11.3 turned seven more red; four are acknowledged and three stay red on
their merits.** Moving the comparison register to L2 (`a6e713f`) edited
`stream.rs` — one more `Copy` vector cut per block in `project` — and
`batch.rs` inside `#[cfg(test)]` only, which are declared paths of
`census-brace-free`, `census-arrays`, `scan-throughput-cold`,
`scan-throughput-warm`, `nested-end-to-end`, `cross-file-floor` and
`projection-widths`. The four `parse`-shaped figures are excused by the
reachability oracle, exactly as `9ed21d4`'s entry uses it: every timed run of
theirs is `parse` or `dd`, and `pgdq parse` enters `map_file`, which reaches
neither `resolve_block` nor `project` — their only non-test call sites are
inside `table_stream`. The entry's evidence is the stamped sweep's own
`raw.json`, which records the command shape of every reading it took.

**The three query-shaped figures are not excusable.** `projection-widths` runs
one `comparison_for` per column per block *and* the extra cut in `project`.
**`nested-end-to-end` and `cross-file-floor` are the case to be careful with**:
their *declared*-path change is the `#[cfg(test)]`-only one in `batch.rs`, so
an entry excusing it would read as "no reading moved" while the change that
could move them sits in `resolve.rs`, which no figure declares — the register's
known false negative, arriving from the side that tempts an over-broad entry.
The reasoning is in
[`../design/roadmap-P11.3-comparison-plan-l2-notes.md`](../design/roadmap-P11.3-comparison-plan-l2-notes.md),
"What was left out, and why".

**`M28` turned `preamble-prepass` red, and it is acknowledged.** Keying
`DumpMetadata::types` on the type name puts a `find` over the types so far in
front of every `CREATE TYPE`, in `preamble.rs` — a declared path of that
figure. The command shape argues it executes, since the figure *is* `pgdq parse
--preamble-only`; the input settles that it does not. `M28`'s only executable
change is `record_type` and its one call site inside the `SpanBody::TypeDef`
arm, and every timed run of this figure is on `blocks4000`, which
`generate_block_count_bench.py` builds out of `CREATE TABLE`s alone — no type
DDL, so no `TypeDef` span, so `record_type` is never called. That is
reachability, the same oracle `a6e713f`'s entry uses, and the entry carries the
grep that re-checks it. **The figure is red all the same**: `eca96be` touched
`preamble.rs` after it and is deliberately not excused there, which makes
`682819d`'s entry inert until a sweep retires both. `--stale` says so under the
figure.

**11.4 re-reddened the four `a6e713f` had cleared, and `c614c4b` is
acknowledged for them.** It edits `pgtype.rs`, `predicate.rs` and `resolve.rs`,
which no figure declares, plus one line of `stream.rs` — `project` cloning a
comparison plan rather than copying it, once per projected column per block.
`stream.rs` was already a changed declared path, and that is exactly why an
entry was owed rather than excused: the register accounts for a **commit**, and
a path is clean only when every commit that touched it is, so one unexamined
commit makes every earlier entry on that path inert. The entry is the same
reachability argument `a6e713f` carries, over a strictly smaller diff — one
line inside `project`, which no `parse`-shaped run reaches.

**M29 touched `batch.rs` and is deliberately not acknowledged.** The `oid` work
added a `ColumnBuilder::UInt32` variant and its match arms, which is real code
on the decode path — it runs in any figure whose input has an `oid` column, so
"those figures were red already" would not be evidence. `nested-end-to-end` and
`cross-file-floor` stay red, now for `7c018b3` as well as for the reason 11.3
gave.

**11.11 turned nothing new red, and `eca96be` is acknowledged for seven
figures.** Its touches to declared paths carry no work: `map.rs` is a type
change — `Vec<(String, String)>` becomes `Vec<ColumnDef>`, with no new call —
`batch.rs` is one line inside `#[cfg(test)]`, and `cache.rs` is
`FORMAT_VERSION` 12 → 13, a constant compared once per cache open. The
commit's one executable addition on a scan path is `extract_collation` in
`preamble.rs`, once per column of DDL, and every input those seven figures are
taken on comes from `generate_perf_data.py`, which writes **exactly one**
`CREATE TABLE` per file — a couple of dozen calls against timed legs measured
in seconds. The entry carries the grep that re-checks it.

**11.6.1 touches two declared paths and no figure is newly stale on their
account**, but an `ACKNOWLEDGED` entry cannot be written for it yet: entries are
keyed by commit sha and this work is uncommitted, so `--stale` reports the
seven figures declaring `stream.rs` or the CLI's `main.rs` as "held red by an
uncommitted change". The reason to file once it lands is reachability, the same
oracle `a6e713f`'s entry uses. Both touches are renames plus doc comments —
`ordering_notes` → `comparison_notes` in `stream.rs`, `announce_ordering` →
`announce_comparisons` in `main.rs` — adding no work anywhere: the notes
function already ran once per resolved block and still iterates a term list
that is empty for every registered command shape, since none passes `--filter`.
The equality mechanism itself is in `predicate.rs` and `pgtype.rs`, which no
figure declares.

**Three figures are deliberately left out of that entry**, because their input
is `blocks4000` — 4000 tables of four columns each — so the same addition runs
16000 times in them. `map-only` and `per-block-quadratic` stay red for that
reason, and `preamble-prepass` stays red because it *is* the measurement of the
prepass the work was added to. Unlike `M28`, reachability does not excuse it
here: `blocks4000` has no type DDL but every one of its tables has columns.

## P11 progress

The spec is
[`../design/roadmap-P11-typed-predicates.md`](../design/roadmap-P11-typed-predicates.md);
the slice order is fixed there and the boxes below are the only record of
progress.

- [x] **11.1** The comparison oracle — per-major answer tables, generated by
      `scripts/generate_fixtures.py` and committed, recording whether the
      server accepted each input. No library code. Notes:
      [`../design/roadmap-P11.1-comparison-oracle-notes.md`](../design/roadmap-P11.1-comparison-oracle-notes.md)
- [x] **11.2** The cross-major differ — the differ, the committed differences
      file, and the suite assertion that keeps it from going stale. Notes:
      [`../design/roadmap-P11.2-cross-major-differ-notes.md`](../design/roadmap-P11.2-cross-major-differ-notes.md)
- [x] **11.2.2** The fixture family moves to glibc — every image Debian rather
      than Alpine and every fixture regenerated, text cases asked under
      `COLLATE "C"` and the database collation for `<` and `=`, and the
      platform triple and `collversion` guarded as apparatus keys. No library
      code. Earned from the grilling of 11.1's musl finding. Notes:
      [`../design/roadmap-P11.2.2-glibc-fixtures-notes.md`](../design/roadmap-P11.2.2-glibc-fixtures-notes.md)
- [x] **11.3** The comparison plan moves to L2 — the register out of
      `predicate.rs` into `pgtype.rs`, keyed on the declared type, carried in
      `ResolvedSchema` as a fourth positional vector. No answer changes.
      Notes:
      [`../design/roadmap-P11.3-comparison-plan-l2-notes.md`](../design/roadmap-P11.3-comparison-plan-l2-notes.md)
- [x] **11.2.1** The register-to-oracle reconciliation — every register arm
      resolves to at least one oracle case and every case back to an arm,
      failing on either direction. Earned from 11.2, whose row asked for a
      check against a register that 11.3 creates. Notes:
      [`../design/roadmap-P11.2.1-register-oracle-reconciliation-notes.md`](../design/roadmap-P11.2.1-register-oracle-reconciliation-notes.md)
- [x] **11.11** The declared collation is read — `COLLATE` captured in the
      preamble parser rather than stopped at, and carried to the register:
      explicit `C`/`POSIX` and a bare `name` agree, an explicit non-`C` clause
      diverges, no clause on a `default`-collation type is unknown. `char(n)`
      is not promoted with them, for a reason that is not collation (I38).
      Notes:
      [`../design/roadmap-P11.11-declared-collation-notes.md`](../design/roadmap-P11.11-declared-collation-notes.md)
- [x] **11.11.1** The collated fixture columns — `public.t_collate` in the
      `types` schema, a collated domain and a collated composite attribute, all
      six majors regenerated, `oracle_register.py` taught the collation
      dimension, `character(10)` asked under both collations with a tab-bearing
      value, one assertion per column in `tests/ordering.rs`, and
      `generate_fixtures.py` reporting its own elapsed time. No library code.
      Earned on entry to 11.11. Notes:
      [`../design/roadmap-P11.11.1-collated-fixture-columns-notes.md`](../design/roadmap-P11.11.1-collated-fixture-columns-notes.md)
- [x] **11.11.2** The displaced `COLLATE` clause, observed — four more
      columns on `t_collate` and a `CREATE COLLATION public.c_collation FROM
      "C"` beside it, all six majors regenerated; I37 amended for v18's
      `CONSTRAINT <name> NOT NULL`, `NO INHERIT` and virtual `GENERATED`, its
      placement claim moved from a lost container to committed bytes, and a
      twenty-second probe recipe added inline for a major with no fixture yet;
      a `pg-dump-compatibility.md` row marking those two v18 shapes untested
      and naming the blocker; assertions in two files, split by reachability.
      No oracle cases, no `KD<k>` and no library code. Earned from grilling
      11.11.1's leftover. Notes:
      [`../design/roadmap-P11.11.2-displaced-collate-notes.md`](../design/roadmap-P11.11.2-displaced-collate-notes.md)
- [x] **11.4** Enum and bare `numeric` — declaration order for the enum;
      arbitrary-precision decimal carrying `Infinity`, `-Infinity` and `NaN`,
      the last two only where a typmod does not exclude them.
      `ComparisonDivergence::EnumLabels` is retired and `ComparisonPlan` gives up
      `Copy`. Notes:
      [`../design/roadmap-P11.4-enum-and-bare-numeric-notes.md`](../design/roadmap-P11.4-enum-and-bare-numeric-notes.md)
- [x] **11.5** The text-held type queue — `interval` (with v17 infinities),
      `time with time zone` and `inet`/`cidr`/`macaddr`/`macaddr8`, each
      ordered by its own comparison (I40) against the literal grammar its
      `*_out` writes; I4 corrected for `INTERVALSTYLE`, I34 widened to
      `interval`'s infinities. Six of `KD7`'s eight text-held types. Notes:
      [`../design/roadmap-P11.5-text-held-types-notes.md`](../design/roadmap-P11.5-text-held-types-notes.md)
- [x] **11.5.1** `jsonb` — the structural container comparison keyed off
      `JsonbValue`'s own type codes, a `jsonb_in` literal parser, and the
      string-leaf residue that closes by statement rather than by code; I41
      added, and `KD7`'s text-held row is down to `json`. Earned on entry to
      11.5, whose row promised a closure the register cannot support. Notes:
      [`../design/roadmap-P11.5.1-jsonb-notes.md`](../design/roadmap-P11.5.1-jsonb-notes.md)
- [x] **11.6** The `character(n)` trim — `CompareKind::PaddedText` takes the
      dump's blank padding off both sides before the collation is consulted
      (I38), so `character` becomes the fourth collatable arm and reaches the
      same three verdicts `text` does; `ComparisonDivergence::BlankPadded` is
      retired and the oracle's eight tab-bearing `character(10)` exceptions go
      with it. `KD7` is **rewritten, not struck**: its `char(n)`, `json` and
      unknown-collation statements close and the one that survives — a column
      stating a collation this build does not implement — drops to
      `(c) unowned`. Typed `=`/`!=` was split out; see 11.6.1. Notes:
      [`../design/roadmap-P11.6-char-trim-notes.md`](../design/roadmap-P11.6-char-trim-notes.md)
- [x] **11.6.1** Typed `=` / `!=` — routed through the comparison plan, with
      the canonicalize-the-literal-once fast path and its seven decode-per-row
      exceptions; `ordering_notes` becomes `comparison_notes` and its
      divergences become operator-conditional; the oracle's `=`/`<>` cells
      asserted, which found `box`'s area equality (`KD10`). Earned from
      11.6, whose row paired a self-contained register correction with a new
      mechanism over every `CompareKind`; **split in turn**, the collation
      half becoming 11.6.2. Notes:
      [`../design/roadmap-P11.6.1-typed-equality-notes.md`](../design/roadmap-P11.6.1-typed-equality-notes.md)
- [ ] **11.6.2** The non-deterministic collation is read — `CREATE COLLATION …
      deterministic = false` out of the preamble and carried to the register,
      so a column of one announces that `=` is not a byte comparison (I42).
      Needs a `SpanBody` variant, a `DatabaseMetadata` field and a cache
      format bump; nothing in `predicate.rs`. Earned from 11.6.1, whose row
      paired it with a mechanism it shares no code with.
- [ ] **11.12** The non-deterministic collation, observed — a `CREATE
      COLLATION` with `provider = icu, deterministic = false` and one
      `t_collate` column of it, regenerated across six majors, so I42 rests on
      committed bytes and not on `pg_dump.c` alone. No oracle case, which is
      what keeps the ICU exclusion intact. No library code. Earned by the
      ordering-register review, which scoped that exclusion to behaviour; see
      [`../design/roadmap-P11-typed-predicates.md`](../design/roadmap-P11-typed-predicates.md).
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
- **Five phases are sketched and none grilled** — P7, P12, P10, P6, P8, in that
  schedule order — a `P<k>` is an identifier and the roadmap's table is the
  schedule, so the numbers say nothing about the order they run in.
  `process.md` step 6 re-grills the roadmap before the next phase is specified,
  and each of the five has an inbox that must be drained as part of that
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

An entry is struck by the change that closes its last part, not at a phase
boundary, and a part closing into a *property* migrates beside its mechanism
rather than being deleted. <!-- deficiency-watermark: KD10 -->
**`KD1`–`KD10` are allocated; none is struck yet.** That watermark is what keeps
a `KD<k>` in an old commit message resolvable once one is, and the marker beside
it is what a citation resolves against — the sentence is rewritten at every
strike, and again at the keystone that deletes the named struck entries.

Where a `(b)` entry's owning phase has been sliced, the entry names the slice
and the slice names the entry, so landing one re-reads the other and a re-slice
is obliged to re-target. The two directions are asymmetric: a checklist line is
a **record**, so its `KD<k>` is a citation that may name a struck entry and may
not name a number nobody allocated; an entry is **present tense**, so it may
name only live slices, and naming a ticked one is an error — closing a part
rewrites the entry in the change that ticks the box. A `(b)` stance also needs
its destination to exist: an entry owned by a phase the roadmap's index calls
`Complete` or `Struck`, or does not list, drops to `(c) unowned` unless a phase
actually absorbs it.

`cd scripts && uv run deficiencies.py` reconciles this index against those
paragraphs, against the source-code markers, against the slice checklist above
and against the roadmap's phase index, and fails on any of them. An entry owned
by a phase with no checklist yet names no slice and is not asked to. That last
read is pinned at both ends: a phase carrying a checklist is `Current` in the
index and a `Current` phase carries one, so a wrap that dropped the checklist
and left the state, or a slicing that wrote the checklist and left it, fails
here rather than reading as a phase nobody has sliced.

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

- **KD7** — a column that *states* a collation this build does not implement —
  a libc locale, an ICU collation — is ordered bytewise under `<`/`>`, so the
  row set is not the server's; the fix is a comparison per named collation, and
  closes the row only up to a provider version. **(c) unowned**; promoted by
  [`../design/roadmap.md`](../design/roadmap.md)'s Future item
  "collation-aware comparison", which is intent without a phase. Detail:
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

- **KD10** — a column whose declared type this build models no comparison for
  answers `=`/`!=` bytewise, which is not the server's answer for the geometric
  types (`box_eq` compares areas), so the row set is wrong; it is announced per
  query and the ordering operators are refused outright. **(c) unowned**;
  promoted by a dump whose queried columns are geometric or hold a
  `money`-shaped extension type. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Equality is typed
  too".

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

- **`M39` lists an enum's labels only where the *column's own* comparison plan
  is an enum's** — a scalar enum column and a domain over one. An enum nested
  inside an array or a composite has a `Refused` plan and gets nothing, in
  `--verbose` and in `--json` alike, so a dump whose only enum-valued column is
  `mood[]` still shows its labels nowhere and sends the reader back to grepping
  the `CREATE TYPE` — the failure `M39` was admitted to end. It was decided
  that way because the plan is the source the admission named, and because the
  set it yields is exactly the set a label-valued `--filter` term can reach, so
  the line answers the question that motivated it. Reconsidering means deciding
  what a nested rendering says: a composite with two enum fields needs the line
  to name the *position*, not just the labels, and that is a format question
  `M39` does not answer. Detail:
  [`../design/architecture.md`](../design/architecture.md), "CLI surface".
