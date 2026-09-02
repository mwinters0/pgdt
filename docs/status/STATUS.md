# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

P1–P5, P9 and P11 are complete and were struck at keystone reviews; how each
mechanism works is [`../design/architecture.md`](../design/architecture.md),
filed by subject. What P11 — typed predicates — built, in twenty-three
slices: the comparison oracle, its cross-major differ and the reconciliation
that keeps the register's arms and the oracle's cases in step; the fixture
family's move to glibc; the comparison register at L2, keyed on the declared
type; the declared collation — the displaced clause and the non-deterministic
one included — observed in committed bytes; the enum, bare `numeric` and the
whole text-held type queue, `jsonb` included; `character(n)`'s trim; typed
`=`/`!=`; the three-valued expression tree and the `--where` grammar that
reaches it from the command line, with the refusal that keeps the two filter
flags meaning one thing; the four `*_in` supersets a nested literal is read
with; and structural comparison for every container kind, a range's canonical
storage form and a user range's `canonical` refusal included. Every one of
those mechanisms is described by subject in
[`../design/architecture.md`](../design/architecture.md), which is where a
session touching one meets its rejected alternatives and its limitations.

[`../design/measurements.md`](../design/measurements.md) carries the `b70589f`
stamp, and **`uv run measure.py --stale` names all thirteen of its figures** —
the paragraphs below the table say what is a decision and what is an omission.
A stale figure obliges no sweep and neither does a wrap: a full sweep is an
hour of a quiet machine and belongs to the phase that is about performance,
which will re-take every table under its own apparatus
([`../design/measurements.md`](../design/measurements.md), "A stale figure does
not oblige a sweep").

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working; a batch flushes on whichever of `max_rows`, `max_bytes` or `max_source_span` comes first, the last of which is what bounds the read chunks an in-flight batch pins ([`../design/architecture.md`](../design/architecture.md), "Three flush triggers") |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working. `oid` is `UInt32` — PostgreSQL's one unsigned integer type, mapped where the ADBC driver's `Int32` turns an OID at or above 2^31 negative. A `uuid` column's field carries the canonical `arrow.uuid` extension name and a `json`/`jsonb` column's `arrow.json`, top level only and written through arrow-rs's own extension types, so neither changes a byte ([`../design/architecture.md`](../design/architecture.md), "Type resolution") |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working |
| Arrays, composites, ranges, multiranges | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<`range struct`>`, `List<List<T>>`. Three shapes stay strings, each with its own resolution outcome — an opaque element type, an element type that is itself an array (I26), and values that disagree on shape. **Every container kind now compares structurally** — element-wise, field-wise and bound-wise through a `ComparisonPlan::Nested` tree, `array_cmp`'s shape tie-break, and one NULL rule at every level (I45) — with the literal read through the `*_in` supersets (I44) and each leaf in its own type's output form. Comparability and divergence are both inherited: a `json` position refuses the column's *ordering* and names itself, a `text[]` announces its element's collation. A column whose order one position refuses still answers `=` — over the container's whole text — and **says what that costs at the position that did it**, since `array_cmp` raises for a `json` element rather than comparing, so bytewise is an answer the server does not have; a position the *resolver* declined instead (I22, I26) resolves the column to text before any tree is read and is silent. **A range is put into the form the server stores it in before it is compared** (I46): `range_serialize`'s out-of-order refusal and empty-collapse, then the canonical function the three discrete built-ins have, so `int4range '[1,10]'`, `'(0,10)'` and `'[1,11)'` are one value and `'(1,2)'` is `empty`; a multirange's members are sorted, coalesced and emptied out before the sequence is walked. A user-defined range declaring a `canonical` function is refused under **every** operator, `=` included, since the server rewrites both operands through arbitrary server-side code before comparing them |
| Array shape census | recorded by every mapping pass and consumed: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, cache-only `info` | working; **`parse` is the only scanner** — it resumes from a matching cache, banks at `COPY` block boundaries under a self-tuning throttle, and saves unconditionally on Ctrl-C (exit 130/143). `info` reports from the cache and never scans. `--verbose` adds each block's byte offsets, a per-column resolution line, an enum column's declared labels beneath it, and — under the `user-defined types` count that heads it — one line per user-defined type, every `TypeKind` arm rendered with its payload. Text output shape is provisional; `--json` carries no shape promise at all, and states the labels once per type in `metadata.databases[].types[]` rather than per column |
| Partial reporting | `info` reports an unfinished scan's cache for as far as it got, with `Scan completion: N%` stated once at the top and nothing below it qualified. An interrupted cache is **typed** for every database segment the scan finished (I1) |
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — thirteen figures, twelve taken by a sweep and one derived across two, each declaring what invalidates it and which documents repeat it. `measure.UNTAKEN` is empty: nothing is built and unrun |
| Column projection | working, library and CLI: `QueryOptions::projection` names columns, cuts the reported `ResolvedSchema` with the batches, may reorder, and may be empty (`COUNT(*)`); `pgdq query` spells it `--column <name>` repeated, or `--no-columns`, which prints no header so `\| wc -l` is a row count. A filter may name a column the projection does not, and an unprojected column is never decoded, so projecting a column away escapes its `Error::FieldDecode` — including `KD2`'s, which the error message does not name ([`../design/architecture.md`](../design/architecture.md), "Projection"; [`../manual/type-handling.md`](../manual/type-handling.md)). Measured on one 3.00 GiB file at five widths: `--no-columns` is 3.28 µs a row against 27.50 for all 19, the two array columns alone are +12.98 and the composite +0.77 ([`../design/measurements.md`](../design/measurements.md), "What a column costs") |
| The filter expression, evaluated three-valued | working: `QueryOptions::filter` is one `Expr` — `Term`/`And`/`Or`/`Not`, `And` and `Or` n-ary — evaluated in SQL's `True`/`False`/`Unknown` domain, a row surviving only where the root is `True`. A NULL field is `Unknown` under every comparing operator, which is the row set the old collapse gave for every conjunction and is what makes `Not` expressible at all. `IS DISTINCT FROM`/`IS NOT DISTINCT FROM` come with it, being the one thing `Not` cannot spell. Short-circuiting is defined against the *root*: `And` stops at the first non-`True` unless a `Not` is above it, which is where a decode failure surfaces or does not. Nothing folds two terms, so a contradictory pair is a query with no rows. Reachable from the CLI as well as the library: `pgdq query --where <expr>` builds the tree and a repeated `--filter` still builds the conjunction ([`../design/architecture.md`](../design/architecture.md), "Predicates") |
| The `--where` expression grammar | working, CLI only — `Expr` is an enum an embedder fills in, so nothing below L4 parses an expression. Parens group, `NOT` binds tighter than `AND` and `AND` tighter than `OR`, the keywords are case-insensitive and are keywords only outside quotes, and everything that is not a paren or a keyword is a term handed to the `--filter` grammar unchanged. A keyword is recognised only against whitespace or a paren, so `tag=and` stays an equality; a `NOT` after the word `is` belongs to the term, so `IS NOT NULL` and `IS NOT DISTINCT FROM` survive whole; juxtaposition is not an implicit `AND`; and a value holding a paren must be quoted. Both flags together are one conjunction. **No `--filter` string changes meaning** — that is what the separate flag buys ([`../design/architecture.md`](../design/architecture.md), "`--where` builds an expression out of those terms"; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`") |
| The `--filter` term grammar | working, CLI only — `Predicate` is a struct an embedder fills in, so nothing below L4 parses a term. Whitespace outside quotes is trimmed on both sides of the operator; `'` and `"` both quote either side, matching pairs only, with an interior quote doubled; the operator split skips quoted regions, so a column named `a=b` is askable; and the `IS NULL` forms are the fallback, tried only on a term with no operator, which is what makes `note=this is null` the equality it reads as. `IS DISTINCT FROM`/`IS NOT DISTINCT FROM` are candidates at the same positions the punctuation spellings are, so the earliest operator still wins in both directions, and the phrase needs whitespace on both sides — which is what leaves a column named `is distinct from` askable as `is distinct from=x`. A malformed quote is refused, never reinterpreted. `--column` and `--table` take their names verbatim and say so when a quoted-looking name is not found ([`../design/architecture.md`](../design/architecture.md), "A filter term is parsed for two audiences"; [`../manual/type-handling.md`](../manual/type-handling.md), "Writing a filter term") |
| Typed ordering operators (`<`, `<=`, `>`, `>=`) | working, library and CLI: each side is decoded with the column's own decoder — the field per row, the literal once when the block's schema resolves — and the decoded values compared, so `9 > 10` is true on an `integer`. Available on a column that resolved `Mapped` and that the comparison register gives an order to — which now includes any container whose every position is comparable — and refused on any other, which is also why `--schema-mode strings` refuses every one of them. An undecodable literal is `Error::PredicateValueDecode` before any row; a field that is genuinely undecodable is `Error::FieldDecode`, worded as the build path words it ([`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed") |
| Typed `=` / `!=` | working, library and CLI: they route through the same per-column `ComparisonPlan`, so `--filter 'price=1.5'` matches a `numeric(10,2)` written `1.50` and `--filter 'code=ab'` matches a padded `char(10)`. **Three canonicalizations**: the literal rendered once into the file's `*_out` form (everything not below — `=` on a `text` column is the byte comparison it always was), the field narrowed per row (`character(n)`), and both sides decoded per row for the seven kinds where the file's spelling is not unique or where reproducing `*_out` would mean re-implementing an output function — bare `numeric`, `interval`, `jsonb`, `real`/`double precision`, `time with time zone`, `inet`/`cidr`. **A nested column takes a fourth shape**: both operator families go through one structural walk over a `ComparisonPlan::Nested` tree, so `--filter 'tags={a, b}'` matches an array written `{a,b}` and the same walk answers `<`. Equality is never *refused* on a column the register does not compare; it falls back to text, a guess for an unmodelled scalar (`KD10`) and, for a nested column one of whose positions has no order, an answer the server does not have — announced at that position rather than passed over in silence. A literal that is not a value of the column's type is `Error::PredicateValueDecode` before any row, on the same output-form-only grammar, and the refusal names the form that column's comparison reads rather than only the value it turned down — a `boolean` is written `t` or `f`, an enum's clause lists its declared labels (the first twelve, then a count) and a `numeric(p,s)`'s names the scale it refuses a finer literal against ([`../design/architecture.md`](../design/architecture.md), "Equality is typed too"; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings") |
| Seven string-shaped types that still order typed | working: `interval` by `interval_cmp_value`'s 128-bit span, so `1 mon`, `30 days` and `720:00:00` are one bound; `time with time zone` by the UTC instant then the stored zone; `inet`/`cidr` by family, shorter prefix, netmask, address; `macaddr`/`macaddr8` by their octets (I40); and `jsonb` by `compareJsonbContainers`' walk down two documents — kind before value, a container's size before its members, and a top-level scalar inside the pseudo-array that makes it outrank `[]` (I41). None has an Arrow type, so the *ordering* is the only path that decodes them. The first six read a literal in the type's own `*_out` spelling and no wider — `1 month` and `08-00-2b-01-02-03` are refused by name, which is a property rather than a deficiency since every value a dump holds is already in the accepted form. `jsonb` is the exception and takes the whole of `jsonb_in`, because `{"a":1}` is what a person types and `{"a": 1}` is what the file holds ([`../manual/type-handling.md`](../manual/type-handling.md), "Seven string-shaped types still order the way PostgreSQL orders them") |
| PostgreSQL's special values under an ordering operator | answered exactly, not raised as a fault: `-infinity` below every finite value, `infinity` above, a `numeric`'s `NaN` above `infinity` and equal to itself (I34), each in the spelling its own type writes — `date` writes `infinity` and `numeric` writes `Infinity`, and neither answers to the other's. A **bare** `numeric` carries all three; one with a typmod carries only `NaN`, since any typmod rejects an infinity, so the two infinity spellings are refused there as a filter literal. Carried as a position in the order rather than as a number, since no Arrow type has one. **A filter is therefore exact where the batch still cannot hold the value** — the row `--filter 'v_date<2020-01-01'` selects for `-infinity` fails to build if `v_date` is projected, which is a property of two paths with different powers, not a defect (`KD8` is the materialization question). ([`../manual/type-handling.md`](../manual/type-handling.md)) |
| The comparison register | **L2**, in `pgtype.rs`: `comparison_for(declared, collation, types)` answers a `ComparisonPlan` per **column**, carried as `ResolvedSchema::comparisons` and rendered as a table in [`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed". `predicate.rs` reads the plan and names no `DataType`. Seventeen of its twenty-five rows agree with PostgreSQL (I33, I34, I37, I38, I40, I41, I45); six diverge, and only one statement among them is a deficiency — a column that *states* a collation this build does not implement, whether the dump calls it deterministic or not (`KD7`, two rows). The other four are properties: a collation no plain dump records, reached through a column and through a `jsonb` string leaf, and `json`, which the server does not order at all. **A divergence is operator-conditional**: `ComparisonDivergence::affects_equality` answers `true` for `json`, for `KD10`'s unmodelled scalar, and for a collation the dump declares `deterministic = false` (I42) — the three collation variants either side of that one answer `false`, a deterministic collation making `texteq` a byte comparison whatever it orders, so a `text` column with no clause warns under `<` and is silent under `=`. **Two rows are nested and neither is a scalar answer**: each carries a `NestedCompare` tree whose verdict is inherited from its positions, so "does this column have an order" is `ComparisonPlan::orders()` and never a match on the variant. The range row's node also carries a `discrete` flag, read off the range *type* rather than off its subtype, which is what says whether the bounds are rewritten on the way in. The plan is `Clone` rather than `Copy`, because three of its comparisons carry a fact about the column: an enum's labels, whether a `numeric`'s typmod excludes the infinities, and a nested column's whole tree. Exhaustiveness is `builtin_scalar` answering the Arrow type and the comparison in one arm, plus a wildcard-free `match` over `TypeKind`. A divergence is announced by `pgdq query` once on stderr, and read by an embedder from `TableStream::comparison_notes` — a third channel, since the signal is per-**term** *and* predicate-conditional (L4) |
| The declared collation, and the dump's own `CREATE COLLATION` list | read, and both move the verdict rather than the comparison: `ColumnDef::collation` keeps a column's `COLLATE` clause verbatim (`pg_catalog."C"`), `TypeKind::Domain` keeps a domain's own as the type default a column-level clause overrides, and the register answers **agrees** for an explicit `C`/`POSIX` and for a bare `name` column, **diverges** for any other stated collation and for a `text`/`varchar` column with no clause at all (I32, I37) — a user-defined collation the same dump declares `locale = 'C'` included, which is correct rows plus a note the user can ignore, and so a property rather than a `KD<k>`. The clause is found wherever `pg_dump` displaced it, past a `DEFAULT`, a `GENERATED … STORED` expression and a `NOT NULL`, which `fixtures/<13–18>/types/default.sql` now carries. `character(n)` is the fourth collatable arm and reaches the same three verdicts: `CompareKind::PaddedText` takes the dump's blank padding off both sides first, which is `bcTruelen` (I38), and the clause then decides exactly as it does for `text` — so a `char(n)` declaring `COLLATE "C"` agrees. **A fourth verdict is read off a statement rather than a name**: `CREATE COLLATION` reaches `DatabaseMetadata::collations` as a `CollationDef { name, deterministic }`, and a clause naming a collation the same dump declared `deterministic = false` answers `NonDeterministicCollation` — the one equality divergence a plain dump states outright (I42), since `pg_dump` writes that clause unconditionally and the server allows it for no provider but ICU. The two spellings are joined parsed rather than as text, and an unqualified reference matches on the name alone, which announces rather than stays silent. **Both determinism answers come off committed bytes**: `fixtures/<13–18>/types/` declares `public.c_collation` (libc) and `public.nd_collation` (`provider = icu, deterministic = false, locale = 'und'`), byte-identically at every major, with `t_collate.v_nd` a column of the second whose clause is spelled exactly as `v_user`'s and which answers differently. The ICU exclusion stays scoped to answers — no oracle case, `fixtures/*/oracle/` byte-unchanged — and the `collversion` reaches one flag set only, `types/binary-upgrade.sql`'s, where a test guards the shape claim and requires the six majors to agree on the version without naming it ([`../design/architecture.md`](../design/architecture.md), "Fixtures"; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise") |
| Comparison oracle | `fixtures/<13–18>/oracle/` holds what PostgreSQL itself answers for 2020 typed comparisons and 317 literals per major, each cell recording whether the server *accepted* the input, generated by `scripts/generate_fixtures.py --skip-dumps` and committed ([`../design/architecture.md`](../design/architecture.md), "The comparison oracle"). The answers are **glibc's** — every fixture container is the Debian (`-trixie`) image — and every text pair is asked twice, under `COLLATE "C"` and under the database's own collation, so both halves of the register's text row are in the file rather than argued. `character(10)` is asked under both too, with a tab-bearing value that puts I38's ordering corollary in the file ahead of 11.6, and a case's `collation` now means one thing only: `None` says the register does not branch on the clause for that type. A pair is asked through two *columns* of the declared type, so a bare case measures what a bare column in a dump does: `name`'s cells are its own `C` type default, and `public.text_c` — the domain whose own DDL carries the clause — is in the file beside it on the `user/Domain` arm. A literal the server refuses therefore never reaches an operator, and answers all six cells with the rejection `literals.tsv` records for it. `jsonb`'s sixteen values are one per branch of `compareJsonbContainers` — both booleans, a string, both empty containers, a one-pair object whose key sorts after a two-pair object's, and the pair separating storage order from alphabetical — so I41's facts are in the file rather than in a probe |
| Register against the oracle's answers | working: `the_register_answers_every_committed_oracle_cell`, a unit test in `predicate.rs`, puts every committed cell to the register through the same `resolve_term`/`ResolvedTerm::eval` path a `--filter` takes — **47,746 cells over six majors, all six operators**. Its one-column schema is built by `resolve_columns` from a synthetic `DumpMetadata`, so resolution, nested plan and comparison plan agree the way they do in a real query. It skips an `E`-cell (what the server refused), a NULL right operand (which the filter grammar cannot spell), and — *for the four ordering operators only* — a column the register refuses an ordering operator on, whose set is asserted exactly; `=`/`<>` are still asked of those columns, because equality is never refused. A NULL **left** operand is not skipped: the server's `u` cell is asserted against `Truth::Unknown` itself, rather than against the exclusion it collapses to at the root. **Forty cases are permitted to disagree and every one of them does, in every cell its divergence reaches**: a `jsonb` string leaf (2), `text` under glibc's `en_US.utf8` (30) and a `text[]` element under the same collation (6) — one statement at three depths — over 24 ordering cells each; and `box`'s area equality (2) over 6 `=` cells, PostgreSQL defining no `box <> box`. A disagreeing term must announce a `ComparisonNote` **under that operator** ([`../design/architecture.md`](../design/architecture.md), "The register against the oracle's answers") |
| Cross-major differ | working: `scripts/oracle_differences.py` walks the majors as a chain of adjacent pairs and files every cell that moved in `fixtures/oracle-differences.tsv` — **533 differences across 13–18, every one of them additive** (I35), so the union rule is checked rather than asserted. `test_oracle_differences.py` asserts the committed file against a fresh computation and, separately, that no difference is non-additive; an oracle pass of `generate_fixtures.py` ends by running the same check ([`../design/architecture.md`](../design/architecture.md), "The cross-major differ") |
| Register-to-oracle reconciliation | working: `scripts/oracle_register.py` reads the register's arms out of `pgtype.rs` — one per declared base name in `builtin_scalar`, one per `TypeKind` match arm in `comparison_user_type`, the three branches of the walk that are not match arms, and the four branches of `collated_text` — and joins them against the case table both ways, failing on either. **38 arms, 54 cases, nothing uncovered and nothing unplaced.** One arm carries an exemption instead of a case and is reported under its own heading: no oracle case can reach `collation/non-deterministic`, a non-deterministic collation being ICU-only (I42) and an ICU case carrying the `collversion` drift the oracle excludes ICU to avoid. **An exemption names where the arm's evidence is** — `(file, needle)` pointers the check resolves, three unit tests today — because the reason alone says why the oracle cannot cover the arm and nothing about what does; it goes stale from both sides, an exempt arm that acquires a case being a problem and evidence that stops resolving being one too. The pointers name sufficient evidence rather than exhaustive, so the fixture bytes that now carry the shape owe no edit there. Each collation branch is anchored on a string the parse must find, so deleting one is reported rather than shortening the list. The collation is a second dimension: a case's label picks the arm, `C` reaching the bytewise branch and `default` the other two, and the `datcollate` that makes that mapping sound is read out of `meta.tsv` rather than assumed. An oracle pass of `generate_fixtures.py` ends by running it beside the differ ([`../design/architecture.md`](../design/architecture.md), "The register-to-oracle reconciliation") |
| ADBC floor oracle | `fixtures/<13–18>/adbc/floor.tsv` holds what the Arrow ADBC PostgreSQL driver (`adbc_driver_postgresql` 1.12.0, pinned in `scripts/pyproject.toml`) returns for every declarable `pg_catalog` type — 74 rows at 13, 82 at 14–18, taken from the host over a published port by `scripts/generate_fixtures.py` and committed ([`../design/architecture.md`](../design/architecture.md), "The ADBC floor oracle"). Evidence only: nothing joins it against `builtin_scalar` yet, and the floor rule itself is not stated anywhere |
| Compressed input (`--source foo.dump.xz`) | not started — P13 for xz, P15 for gzip/zstd. Input is assumed already-decompressed plain SQL text; `pg_dump -Fp --compress=…` output is therefore unreadable today ([`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)). **P13 is grilled, partly specified and blocked**: no crate answers a positioned read over an `.xz` file, so the seekable-xz layer is being carved out into its own repository ([`../design/roadmap-P13-compressed-input.md`](../design/roadmap-P13-compressed-input.md), "Blocked") |
| Remote input (`--source https://…`), over `object_store` | not started — P14, carved out of P6. `ByteRangeSource` is already shaped against `get_range`/`head`, and there is exactly one implementation: `LocalFileSource` |
| Python bindings, DataFusion `TableProvider` | not started — P6 |
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
attached.

**Nothing is built and unrun.** `measure.UNTAKEN` is empty: `projection-widths`
was taken and moved into `FIGURES`, and `composite-isolated` was deleted
unpublished along with its whole apparatus — the `--weak-composite` generator
flag, the `composite_text` input and the fidelity case pairing them — because
the projection table makes the same isolation a subtraction between two adjacent
rows of one file.

**All thirteen figures are stale, and the per-commit account lives in the
harness rather than here.** `uv run measure.py --stale` names, per figure and
per declared path, which acknowledgement entries have gone inert and which
commits hold the path red. What that output cannot say is why a commit was
*deliberately* not acknowledged, and five such reasons stand:

- **A query-shaped figure executes the comparison register and the filter
  tree.** `projection-widths`, `nested-end-to-end` and `cross-file-floor` run
  one `comparison_for` per column per block and evaluate an expression tree
  once per row where a flat term loop ran before, so neither the
  byte-identity nor the reachability oracle reaches them. "Small" is not
  evidence; only a sweep settles them.
- **`nested-end-to-end` and `cross-file-floor` are the pair to be careful
  with.** Their *declared*-path change is `#[cfg(test)]`-only, so an entry
  excusing it would read as "no reading moved" while the change that could
  move them sits in `resolve.rs`, which no figure declares — the register's
  known false negative, arriving from the side that tempts an over-broad
  entry.
- **`preamble-prepass` is the measurement of the prepass the work was added
  to.** The `COLLATE` extraction and the `CREATE COLLATION` parse are on its
  measured path by construction, so no argument about reachability excuses
  them there.
- **`map-only` and `per-block-quadratic` are taken on `blocks4000`** — 4000
  tables of four columns each — so a per-column addition runs 16000 times in
  them, where every other figure's input is one `CREATE TABLE` per file and a
  couple of dozen calls against legs measured in seconds.
- **The `oid` work added a real decode-path variant.** `ColumnBuilder::UInt32`
  and its match arms run in any figure whose input has an `oid` column, so
  "those figures were red already" would not be evidence.

**Where a commit's touch to a declared path is a rename, a doc comment, a
format-version constant or flag plumbing that no registered command shape
executes, the entry was owed at the commit and was not written.** That is the
failure the register exists to make visible rather than one to reconstruct
from memory afterwards; the figures are held red by those commits either way,
and `--stale` names them.

**`session-drift` stays stale for a reason no oracle reaches, and that is the
honest state.** It declares `scripts/measure.py`, and the harness *is* the
apparatus that figure measures — so neither byte-identity nor reachability
applies and nothing but taking it settles it. It is derived rather than
measured, so `uv run measure.py --drift <sweep> <sweep>` re-derives it from two
sweeps' `raw.json` without measuring anything; what it lacks is a pair taken
past this commit. The scan-performance phase will supply one.

## P12 progress

The ADBC type floor. Spec:
[`../design/roadmap-P12-adbc-type-floor.md`](../design/roadmap-P12-adbc-type-floor.md).

- [x] **12.1** The floor oracle — the catalog sweep, `fixtures/<13–18>/adbc/floor.tsv`,
      and the `adbc_driver_postgresql` pin in `scripts/pyproject.toml`. No library code.
      Notes: [`../design/roadmap-P12.1-floor-oracle-notes.md`](../design/roadmap-P12.1-floor-oracle-notes.md)
- [ ] **12.2** The reconciliation — joins the oracle against `builtin_scalar` and fails
      both ways; D2's stances as declared exemptions, `interval` and `int2vector` naming
      12.3 and 12.6; the rule filed beside "The bar"; `money` earns a register entry
      under stance (a), allocating the next free number in that same change.
- [ ] **12.3** `interval`: the triple-producing decoder and the resolution arm.
- [ ] **12.4** `interval`: render-back's sub-microsecond refusal, the comparison
      register's arm, the interval decode failure folded into the existing
      `infinity`/`NaN` register entry, `type-handling.md` corrected.
- [ ] **12.5** `int2vector`: the fixture column, and the six-major regeneration.
- [ ] **12.6** `int2vector`: the codec and the resolution arm.

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **Six phases are sketched and two are specified** — P13, P7, P12, P10, P14,
  P6, P15, P8, in the roadmap table's schedule order; a `P<k>` is an identifier,
  so the numbers say nothing about the order they run in. P13 is grilled and
  **blocked** on an external seekable-xz crate; P12 is grilled, specified and
  **current**. Every remaining phase that carries an inbox must have it drained
  as part of its own grilling, which `process.md` step 6 re-grills the roadmap
  before.

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
rather than being deleted. <!-- deficiency-watermark: KD12 -->
**`KD1`–`KD12` are allocated, and nothing at or below `KD12` is reused** — a
number the index below does not carry is a struck entry, not a typo. That
watermark is what keeps a `KD<k>` in an old commit message resolvable, and the
marker beside it is what a citation resolves against; the names of the struck
entries went at the keystone, `git log` being what answers *when*.

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

- **KD7** — a column that *states* a collation this build does not implement is
  compared bytewise, so the row set is not the server's: under `<`/`>` always,
  and under `=`/`!=` where the dump declares it `deterministic = false` (I42);
  the fix is a comparison per named collation, up to a provider version.
  **(c) unowned**; promoted by [`../design/roadmap.md`](../design/roadmap.md)'s
  Future item "collation-aware comparison", intent without a phase. Detail:
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
  types (`box_eq` compares areas), so the row set is wrong; ordering is refused
  outright, and the announcement misses a type reached through a container
  (`box[]`). **(c) unowned**; promoted by a dump whose queried columns are
  geometric or hold a `money`-shaped extension type. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Equality is typed
  too".

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

Nothing open.
