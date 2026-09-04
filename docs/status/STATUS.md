# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

P1–P5, P9, P11 and P12 are complete and were struck at keystone reviews; how
each mechanism works is [`../design/architecture.md`](../design/architecture.md),
filed by subject, which is where a session touching one meets its rejected
alternatives and its limitations. The capability table below says what state
each is in.

[`../design/measurements.md`](../design/measurements.md) carries the `ba2fc12`
stamp of 2026-09-03: the scan-performance baseline sweep pair was taken and
folded in whole, so seven of its fifteen tables come from one sitting. Seven
were taken on their own afterwards and each says so in its own section —
`allocator` twice, by 7.3 and again by 7.13, whose ratios are within-sitting
and whose second sitting reversed both cells the adoption argument rested on;
`per-block-quadratic`, `map-only` and `preamble-prepass` by 7.4, which moved
the first of the three by two orders of magnitude and re-took the other two in
the same sitting because they share its readings and its subject; and
`scan-throughput-cold`, `scan-throughput-warm` and `census-brace-free` by 7.5,
the three of them in one sitting because the two throughput tables' `COPY` row
*is* the census table's census-on column. The fifteenth, `predicate-terms`,
stands outside the sweep for the other reason and is the doc's newest table:
it did not exist when the sweep ran, being the first to pass a filter at all,
and 7.7 took it on 2026-09-04 at `42b1611` plus this slice's own uncommitted
harness change. **It reads stale and is current in fact**, and that is
mechanical rather than a finding: `--stale` diffs every declared path against
the doc's session stamp, so a table taken *after* that stamp is red on every
path that moved between the two — which for this one is the whole set 7.6,
7.13 and 7.13.1 moved, all of them before it was taken. 7.12's sweep clears
the stamp and the whole column with it.

**All thirteen sweep figures read stale against the `ba2fc12` stamp, and none
of them is current in fact.** `allocator` is the newest of the thirteen,
re-taken by 7.13 on the pooled read path, and 7.13.1 has since moved it too;
`per-block-quadratic`, `map-only` and `preamble-prepass` (7.4) and the three 7.5
re-took were current until 7.13. Every one of them times a `pgdq` run, and the
two slices that follow the stamp both changed what such a run costs per byte of
input.

**One published cell is not merely stale but wrong by a factor of six, and is
not to be quoted until 7.12 re-takes it.** `census-arrays` prices the census on
array-bearing rows; 7.6 put the field split behind it on `memchr`, and a
whole-file `parse` of that input falls **17.781 G → 2.919 G** user instructions
([2026-09-04](history/2026-09-04.md), "The census's field split was the byte
loop, not the census"). It was already red on `pgdump_query/src/copy.rs`, so
nothing about the register missed it; what is new is the size.

**The genuinely stale ones and their reasons.** Every figure that times a
`pgdq` run is red on `pgdump_query/src/io.rs` and on
`pgdump_query/src/scan.rs`/`stream.rs` — 7.13's buffer pool and 7.13.1's read
carry, which between them took **26%** of a warm `parse`'s user instructions and
cannot be argued away — and every figure that reads a `COPY` row is red on
`pgdump_query/src/copy.rs` as well, which is 7.6. That subsumes the case `census-arrays` and
`projection-widths` used to make on their own: both time a single-`COPY`-block
input, where 7.4's gate is open at the one `CopyEnd` there is and 7.5's `INSERT`
fast path is never entered, so those two slices left them doing the same work —
but 7.13.1 changes how every chunk of every input is handed to the scanner, so
there is no shape that escapes it. `nested-end-to-end`, `census-attribution` and
`cross-file-floor` are additionally red on `pgdump_query-cli/src/`, from 7.3's
`mod alloc;` and `--version` string. `session-drift` is red on
`scripts/measure.py`: 7.3 added a real figure function there, 7.4 corrected
three figures' declared paths, 7.13 added a fourth mechanism and fixed the
allocator legs' build cache, and `M48` declared the borrow graph the sweep now
reads, so the reachability oracle that excused the five `--profile-recipe`
commits stretches to none of them, and the wrap sweep is what clears it.
`measure.ACKNOWLEDGED` still carries six entries — two for
`7545dc6`, four for `fbaaa49`, `a6bf6cd`, `305af4b` and `360e144` — every one
of them now inert, held red by the uncommitted change; `7545dc6`'s `batch.rs`
entry names `allocator` in its figure list, because a figure declaring an
already-excused path has to be named there or the old excuse silently stops
covering it. A stale figure obliges
no sweep and neither does a wrap: a full
sweep is an hour of a quiet machine and belongs to the phase that is about
performance, which will re-take every table under its own apparatus
([`../design/measurements.md`](../design/measurements.md), "A stale figure does
not oblige a sweep").

**Three figures were blind to the file they measure.** `per-block-quadratic`
and `map-only` price the map's per-block rebuild and declared
`pgdump_query/src/map.rs` without `pgdump_query/src/stream.rs`, where
`stream::splice` lives — so 7.4 would have read green against the two tables it
moved by two orders of magnitude. `preamble-prepass` *borrows* the first's
4000-block reading for its second row and declared neither. All three now
declare `MAP_BUILD`, and the third declares the edges of the reading it
borrows.

**And every figure was blind to the read path.** Nothing declared
`pgdump_query/src/io.rs`, though every table that times a `pgdq` run reads its
bytes through it — so 7.13, which removed the largest single term in a warm
`parse`'s user time, would have read green against all twelve. It is now its
own mechanism (`READ`) rather than part of `SCAN`, because `nested-end-to-end`
and `census-attribution` declare no scanner path and are moved by it all the
same. 7.13.1's carry needed no such fix: it lives in `scan.rs` and `stream.rs`,
which `SCAN` and `MAP_BUILD` already declared.

**A fifth mechanism arrived with `predicate-terms`.** Nothing in the register
named `pgdump_query/src/predicate.rs` — not blindness this time, since until
7.7 no figure passed a filter and the file could not have moved one. `PREDICATE`
exists as of that figure, and it is the only figure that declares it.

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working; a batch flushes on whichever of `max_rows`, `max_bytes` or `max_source_span` comes first, the last of which is what bounds the read chunks an in-flight batch pins ([`../design/architecture.md`](../design/architecture.md), "Three flush triggers") |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working. `interval` is `Interval(MonthDayNano)` — PostgreSQL's own three fields — with v17's infinities and a time part past `2562047:47:16.854775807` an `Error::FieldDecode` and `--schema-mode strings` the recourse. `oid` is `UInt32` — PostgreSQL's one unsigned integer type, mapped where the ADBC driver's `Int32` turns an OID at or above 2^31 negative. `int2vector` is `List<Int16>` — the one built-in whose Arrow type is a container and whose name is not spelled like one, written as space-separated `int16`s with no quoting and no NULL element (I47), so it travels with a `NestedPlan` of its own exactly as a multirange does. A `uuid` column's field carries the canonical `arrow.uuid` extension name and a `json`/`jsonb` column's `arrow.json`, top level only and written through arrow-rs's own extension types, so neither changes a byte. Render-back has a third outcome besides a value and SQL NULL: `render_field` returns `Result<Option<String>, Error>`, and `Error::FieldRender` is an Arrow value with no PostgreSQL text form — reachable only from an array a caller assembled, since every column this crate fills comes from a decoder whose range its renderer writes back, and carried by `interval` alone, whose nanoseconds are finer than PostgreSQL's microseconds ([`../design/architecture.md`](../design/architecture.md), "Type resolution" and "Decoders and render-back") |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working |
| Arrays, composites, ranges, multiranges, `int2vector` | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<`range struct`>`, `List<List<T>>`, and `int2vector`'s `List<Int16>` — a fifth literal form with no wrapper, no quoting and no NULL element (I47), compared through `anyarray` polymorphism's `array_lt`/`array_eq` so `'2' < '10'` is true where a byte comparison says false. Three shapes stay strings, each with its own resolution outcome — an opaque element type, an element type that is itself an array (I26), and values that disagree on shape. **Every container kind now compares structurally** — element-wise, field-wise and bound-wise through a `ComparisonPlan::Nested` tree, `array_cmp`'s shape tie-break, and one NULL rule at every level (I45) — with the literal read through the `*_in` supersets (I44) and each leaf in its own type's output form. Comparability and divergence are both inherited: a `json` position refuses the column's *ordering* and names itself, a `text[]` announces its element's collation. A column whose order one position refuses still answers `=` — over the container's whole text — and **says what that costs at the position that did it**, since `array_cmp` raises for a `json` element rather than comparing, so bytewise is an answer the server does not have; a position the *resolver* declined instead (I22, I26) resolves the column to text before any tree is read and is silent. **A range is put into the form the server stores it in before it is compared** (I46): `range_serialize`'s out-of-order refusal and empty-collapse, then the canonical function the three discrete built-ins have, so `int4range '[1,10]'`, `'(0,10)'` and `'[1,11)'` are one value and `'(1,2)'` is `empty`; a multirange's members are sorted, coalesced and emptied out before the sequence is walked. A user-defined range declaring a `canonical` function is refused under **every** operator, `=` included, since the server rewrites both operands through arbitrary server-side code before comparing them |
| Array shape census | recorded by every mapping pass and consumed: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, cache-only `info` | working; **`parse` is the only scanner** — it resumes from a matching cache, banks at `COPY` block boundaries under a self-tuning throttle, and saves unconditionally on Ctrl-C (exit 130/143). `info` reports from the cache and never scans. `--verbose` adds each block's byte offsets, a per-column resolution line, an enum column's declared labels beneath it, and — under the `user-defined types` count that heads it — one line per user-defined type, every `TypeKind` arm rendered with its payload. Text output shape is provisional; `--json` carries no shape promise at all, and states the labels once per type in `metadata.databases[].types[]` rather than per column |
| Partial reporting | `info` reports an unfinished scan's cache for as far as it got, with `Scan completion: N%` stated once at the top and nothing below it qualified. An interrupted cache is **typed** for every database segment the scan finished (I1) |
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — fifteen figures, fourteen taken by a sweep and one derived across two, each declaring what invalidates it, which documents repeat it, and which readings it borrows from another figure — that third edge is what lets `--figure` pull in what a figure borrows and name the rest of the set that must be re-taken with it, and `--alone` is how a partial sitting is asked for deliberately. `measure.UNTAKEN` is empty: nothing is built and unrun. It also builds and interrogates the `allocator` figure's three legs, reading each binary's allocator out of `pgdq --version` rather than trusting the flags it passed, and names the shipped one in the session stamp. A leg is rebuilt **once per harness process** rather than reused from `runs/`, which is what stops a fresh reference being timed against last session's legs, and all of them are built before the first reading rather than at the rep that wants one |
| Column projection | working, library and CLI: `QueryOptions::projection` names columns, cuts the reported `ResolvedSchema` with the batches, may reorder, and may be empty (`COUNT(*)`); `pgdq query` spells it `--column <name>` repeated, or `--no-columns`, which prints no header so `\| wc -l` is a row count. A filter may name a column the projection does not, and an unprojected column is never decoded, so projecting a column away escapes its `Error::FieldDecode` — including `KD2`'s, which the error message does not name ([`../design/architecture.md`](../design/architecture.md), "Projection"; [`../manual/type-handling.md`](../manual/type-handling.md)). Measured on one 3.00 GiB file at five widths: `--no-columns` is 3.18 µs a row against 28.56 for all 19, the two array columns alone are +13.21 and the composite +0.98 ([`../design/measurements.md`](../design/measurements.md), "What a column costs") |
| The filter expression, evaluated three-valued | working: `QueryOptions::filter` is one `Expr` — `Term`/`And`/`Or`/`Not`, `And` and `Or` n-ary — evaluated in SQL's `True`/`False`/`Unknown` domain, a row surviving only where the root is `True`. A NULL field is `Unknown` under every comparing operator, which is the row set the old collapse gave for every conjunction and is what makes `Not` expressible at all. `IS DISTINCT FROM`/`IS NOT DISTINCT FROM` come with it, being the one thing `Not` cannot spell. Short-circuiting is defined against the *root*: `And` stops at the first non-`True` unless a `Not` is above it, which is where a decode failure surfaces or does not. Nothing folds two terms, so a contradictory pair is a query with no rows. Reachable from the CLI as well as the library: `pgdq query --where <expr>` builds the tree and a repeated `--filter` still builds the conjunction ([`../design/architecture.md`](../design/architecture.md), "Predicates") |
| The `--where` expression grammar | working, CLI only — `Expr` is an enum an embedder fills in, so nothing below L4 parses an expression. Parens group, `NOT` binds tighter than `AND` and `AND` tighter than `OR`, the keywords are case-insensitive and are keywords only outside quotes, and everything that is not a paren or a keyword is a term handed to the `--filter` grammar unchanged. A keyword is recognised only against whitespace or a paren, so `tag=and` stays an equality; a `NOT` after the word `is` belongs to the term, so `IS NOT NULL` and `IS NOT DISTINCT FROM` survive whole; juxtaposition is not an implicit `AND`; and a value holding a paren must be quoted. Both flags together are one conjunction. **No `--filter` string changes meaning** — that is what the separate flag buys ([`../design/architecture.md`](../design/architecture.md), "`--where` builds an expression out of those terms"; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`") |
| The `--filter` term grammar | working, CLI only — `Predicate` is a struct an embedder fills in, so nothing below L4 parses a term. Whitespace outside quotes is trimmed on both sides of the operator; `'` and `"` both quote either side, matching pairs only, with an interior quote doubled; the operator split skips quoted regions, so a column named `a=b` is askable; and the `IS NULL` forms are the fallback, tried only on a term with no operator, which is what makes `note=this is null` the equality it reads as. `IS DISTINCT FROM`/`IS NOT DISTINCT FROM` are candidates at the same positions the punctuation spellings are, so the earliest operator still wins in both directions, and the phrase needs whitespace on both sides — which is what leaves a column named `is distinct from` askable as `is distinct from=x`. A malformed quote is refused, never reinterpreted. `--column` and `--table` take their names verbatim and say so when a quoted-looking name is not found ([`../design/architecture.md`](../design/architecture.md), "A filter term is parsed for two audiences"; [`../manual/type-handling.md`](../manual/type-handling.md), "Writing a filter term") |
| Typed ordering operators (`<`, `<=`, `>`, `>=`) | working, library and CLI: each side is decoded with the column's own decoder — the field per row, the literal once when the block's schema resolves — and the decoded values compared, so `9 > 10` is true on an `integer`. Available on a column that resolved `Mapped` and that the comparison register gives an order to — which now includes any container whose every position is comparable — and refused on any other, which is also why `--schema-mode strings` refuses every one of them. An undecodable literal is `Error::PredicateValueDecode` before any row; a field that is genuinely undecodable is `Error::FieldDecode`, worded as the build path words it ([`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed") |
| Typed `=` / `!=` | working, library and CLI: they route through the same per-column `ComparisonPlan`, so `--filter 'price=1.5'` matches a `numeric(10,2)` written `1.50` and `--filter 'code=ab'` matches a padded `char(10)`. **Three canonicalizations**: the literal rendered once into the file's `*_out` form (everything not below — `=` on a `text` column is the byte comparison it always was), the field narrowed per row (`character(n)`), and both sides decoded per row for the seven kinds where the file's spelling is not unique or where reproducing `*_out` would mean re-implementing an output function — bare `numeric`, `interval`, `jsonb`, `real`/`double precision`, `time with time zone`, `inet`/`cidr`. **A nested column takes a fourth shape**: both operator families go through one structural walk over a `ComparisonPlan::Nested` tree, so `--filter 'tags={a, b}'` matches an array written `{a,b}` and the same walk answers `<`. Equality is never *refused* on a column the register does not compare; it falls back to text, a guess for an unmodelled scalar (`KD10`) and, for a nested column one of whose positions has no order, an answer the server does not have — announced at that position rather than passed over in silence. A literal that is not a value of the column's type is `Error::PredicateValueDecode` before any row, on the same output-form-only grammar, and the refusal names the form that column's comparison reads rather than only the value it turned down — a `boolean` is written `t` or `f`, an enum's clause lists its declared labels (the first twelve, then a count) and a `numeric(p,s)`'s names the scale it refuses a finer literal against ([`../design/architecture.md`](../design/architecture.md), "Equality is typed too"; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings") |
| Six string-shaped types that order typed, and `interval`, which now has an Arrow type and still does | working: `interval` by `interval_cmp_value`'s 128-bit span, so `1 mon`, `30 days` and `720:00:00` are one bound; `time with time zone` by the UTC instant then the stored zone; `inet`/`cidr` by family, shorter prefix, netmask, address; `macaddr`/`macaddr8` by their octets (I40); and `jsonb` by `compareJsonbContainers`' walk down two documents — kind before value, a container's size before its members, and a top-level scalar inside the pseudo-array that makes it outrank `[]` (I41). Only `interval` has an Arrow type; for the other six the *ordering* is the only path that decodes them, and for `interval` the ordering's fused span and the decoder's triple are two consumers of one grammar walk. The first six read a literal in the type's own `*_out` spelling and no wider — `1 month` and `08-00-2b-01-02-03` are refused by name, which is a property rather than a deficiency since every value a dump holds is already in the accepted form. `jsonb` is the exception and takes the whole of `jsonb_in`, because `{"a":1}` is what a person types and `{"a": 1}` is what the file holds ([`../manual/type-handling.md`](../manual/type-handling.md), "Six string-shaped types still order the way PostgreSQL orders them" and "`interval` keeps its three fields") |
| PostgreSQL's special values under an ordering operator | answered exactly, not raised as a fault: `-infinity` below every finite value, `infinity` above, a `numeric`'s `NaN` above `infinity` and equal to itself (I34), each in the spelling its own type writes — `date` writes `infinity` and `numeric` writes `Infinity`, and neither answers to the other's. A **bare** `numeric` carries all three; one with a typmod carries only `NaN`, since any typmod rejects an infinity, so the two infinity spellings are refused there as a filter literal. Carried as a position in the order rather than as a number, since no Arrow type has one. **A filter is therefore exact where the batch still cannot hold the value** — the row `--filter 'v_date<2020-01-01'` selects for `-infinity` fails to build if `v_date` is projected, which is a property of two paths with different powers, not a defect (`KD8` is the materialization question). ([`../manual/type-handling.md`](../manual/type-handling.md)) |
| The comparison register | **L2**, in `pgtype.rs`: `comparison_for(declared, collation, types)` answers a `ComparisonPlan` per **column**, carried as `ResolvedSchema::comparisons` and rendered as a table in [`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed". `predicate.rs` reads the plan and names no `DataType`. Eighteen of its twenty-six rows agree with PostgreSQL (I33, I34, I37, I38, I40, I41, I45, I47); six diverge, and only one statement among them is a deficiency — a column that *states* a collation this build does not implement, whether the dump calls it deterministic or not (`KD7`, two rows). The other four are properties: a collation no plain dump records, reached through a column and through a `jsonb` string leaf, and `json`, which the server does not order at all. **A divergence is operator-conditional**: `ComparisonDivergence::affects_equality` answers `true` for `json`, for `KD10`'s unmodelled scalar, and for a collation the dump declares `deterministic = false` (I42) — the three collation variants either side of that one answer `false`, a deterministic collation making `texteq` a byte comparison whatever it orders, so a `text` column with no clause warns under `<` and is silent under `=`. **Two rows are nested and neither is a scalar answer**: each carries a `NestedCompare` tree whose verdict is inherited from its positions, so "does this column have an order" is `ComparisonPlan::orders()` and never a match on the variant. The range row's node also carries a `discrete` flag, read off the range *type* rather than off its subtype, which is what says whether the bounds are rewritten on the way in. The plan is `Clone` rather than `Copy`, because three of its comparisons carry a fact about the column: an enum's labels, whether a `numeric`'s typmod excludes the infinities, and a nested column's whole tree. Exhaustiveness is `builtin_scalar` answering the Arrow type and the comparison in one arm, plus a wildcard-free `match` over `TypeKind`. A divergence is announced by `pgdq query` once on stderr, and read by an embedder from `TableStream::comparison_notes` — a third channel, since the signal is per-**term** *and* predicate-conditional (L4) |
| The declared collation, and the dump's own `CREATE COLLATION` list | read, and both move the verdict rather than the comparison: `ColumnDef::collation` keeps a column's `COLLATE` clause verbatim (`pg_catalog."C"`), `TypeKind::Domain` keeps a domain's own as the type default a column-level clause overrides, and the register answers **agrees** for an explicit `C`/`POSIX` and for a bare `name` column, **diverges** for any other stated collation and for a `text`/`varchar` column with no clause at all (I32, I37) — a user-defined collation the same dump declares `locale = 'C'` included, which is correct rows plus a note the user can ignore, and so a property rather than a `KD<k>`. The clause is found wherever `pg_dump` displaced it, past a `DEFAULT`, a `GENERATED … STORED` expression and a `NOT NULL`, which `fixtures/<13–18>/types/default.sql` now carries. `character(n)` is the fourth collatable arm and reaches the same three verdicts: `CompareKind::PaddedText` takes the dump's blank padding off both sides first, which is `bcTruelen` (I38), and the clause then decides exactly as it does for `text` — so a `char(n)` declaring `COLLATE "C"` agrees. **A fourth verdict is read off a statement rather than a name**: `CREATE COLLATION` reaches `DatabaseMetadata::collations` as a `CollationDef { name, deterministic }`, and a clause naming a collation the same dump declared `deterministic = false` answers `NonDeterministicCollation` — the one equality divergence a plain dump states outright (I42), since `pg_dump` writes that clause unconditionally and the server allows it for no provider but ICU. The two spellings are joined parsed rather than as text, and an unqualified reference matches on the name alone, which announces rather than stays silent. **Both determinism answers come off committed bytes**: `fixtures/<13–18>/types/` declares `public.c_collation` (libc) and `public.nd_collation` (`provider = icu, deterministic = false, locale = 'und'`), byte-identically at every major, with `t_collate.v_nd` a column of the second whose clause is spelled exactly as `v_user`'s and which answers differently. The ICU exclusion stays scoped to answers — no oracle case, `fixtures/*/oracle/` byte-unchanged — and the `collversion` reaches one flag set only, `types/binary-upgrade.sql`'s, where a test guards the shape claim and requires the six majors to agree on the version without naming it ([`../design/architecture.md`](../design/architecture.md), "Fixtures"; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise") |
| Comparison oracle | `fixtures/<13–18>/oracle/` holds what PostgreSQL itself answers for 2096 typed comparisons and 331 literals per major, each cell recording whether the server *accepted* the input, generated by `scripts/generate_fixtures.py --skip-dumps` and committed ([`../design/architecture.md`](../design/architecture.md), "The comparison oracle"). The answers are **glibc's** — every fixture container is the Debian (`-trixie`) image — and every text pair is asked twice, under `COLLATE "C"` and under the database's own collation, so both halves of the register's text row are in the file rather than argued. `character(10)` is asked under both too, with a tab-bearing value that puts I38's ordering corollary in the file, and a case's `collation` now means one thing only: `None` says the register does not branch on the clause for that type. A pair is asked through two *columns* of the declared type, so a bare case measures what a bare column in a dump does: `name`'s cells are its own `C` type default, and `public.text_c` — the domain whose own DDL carries the clause — is in the file beside it on the `user/Domain` arm. A literal the server refuses therefore never reaches an operator, and answers all six cells with the rejection `literals.tsv` records for it. `jsonb`'s sixteen values are one per branch of `compareJsonbContainers` — both booleans, a string, both empty containers, a one-pair object whose key sorts after a two-pair object's, and the pair separating storage order from alphabetical — so I41's facts are in the file rather than in a probe |
| Register against the oracle's answers | working: `the_register_answers_every_committed_oracle_cell`, a unit test in `predicate.rs`, puts every committed cell to the register through the same `resolve_term`/`ResolvedTerm::eval` path a `--filter` takes — **52,338 cells over six majors, all six operators**. Its one-column schema is built by `resolve_columns` from a synthetic `DumpMetadata`, so resolution, nested plan and comparison plan agree the way they do in a real query. It skips an `E`-cell (what the server refused), a NULL right operand (which the filter grammar cannot spell), and — *for the four ordering operators only* — a column the register refuses an ordering operator on, whose set is asserted exactly; `=`/`<>` are still asked of those columns, because equality is never refused. A NULL **left** operand is not skipped: the server's `u` cell is asserted against `Truth::Unknown` itself, rather than against the exclusion it collapses to at the root. **Forty cases are permitted to disagree and every one of them does, in every cell its divergence reaches**: a `jsonb` string leaf (2), `text` under glibc's `en_US.utf8` (30) and a `text[]` element under the same collation (6) — one statement at three depths — over 24 ordering cells each; and `box`'s area equality (2) over 6 `=` cells, PostgreSQL defining no `box <> box`. A disagreeing term must announce a `ComparisonNote` **under that operator** ([`../design/architecture.md`](../design/architecture.md), "The register against the oracle's answers") |
| Cross-major differ | working: `scripts/oracle_differences.py` walks the majors as a chain of adjacent pairs and files every cell that moved in `fixtures/oracle-differences.tsv` — **533 differences across 13–18, every one of them additive** (I35), so the union rule is checked rather than asserted. `test_oracle_differences.py` asserts the committed file against a fresh computation and, separately, that no difference is non-additive; an oracle pass of `generate_fixtures.py` ends by running the same check ([`../design/architecture.md`](../design/architecture.md), "The cross-major differ") |
| Register-to-oracle reconciliation | working: `scripts/oracle_register.py` reads the register's arms out of `pgtype.rs` — one per declared base name in `builtin_scalar`, one per `TypeKind` match arm in `comparison_user_type`, the three branches of the walk that are not match arms, and the four branches of `collated_text` — and joins them against the case table both ways, failing on either. **42 arms, 55 cases, nothing uncovered and nothing unplaced.** One arm carries an exemption instead of a case and is reported under its own heading: no oracle case can reach `collation/non-deterministic`, a non-deterministic collation being ICU-only (I42) and an ICU case carrying the `collversion` drift the oracle excludes ICU to avoid. **An exemption names where the arm's evidence is** — `(file, needle)` pointers the check resolves, three unit tests today — because the reason alone says why the oracle cannot cover the arm and nothing about what does; it goes stale from both sides, an exempt arm that acquires a case being a problem and evidence that stops resolving being one too. The pointers name sufficient evidence rather than exhaustive, so the fixture bytes that now carry the shape owe no edit there. Each collation branch is anchored on a string the parse must find, so deleting one is reported rather than shortening the list. The collation is a second dimension: a case's label picks the arm, `C` reaching the bytewise branch and `default` the other two, and the `datcollate` that makes that mapping sound is read out of `meta.tsv` rather than assumed. An oracle pass of `generate_fixtures.py` ends by running it beside the differ ([`../design/architecture.md`](../design/architecture.md), "The register-to-oracle reconciliation") |
| ADBC floor oracle | `fixtures/<13–18>/adbc/floor.tsv` holds what the Arrow ADBC PostgreSQL driver (`adbc_driver_postgresql` 1.12.0, pinned in `scripts/pyproject.toml`) returns for every declarable `pg_catalog` type — 74 rows at 13, 82 at 14–18, taken from the host over a published port by `scripts/generate_fixtures.py` and committed ([`../design/architecture.md`](../design/architecture.md), "The ADBC floor oracle") |
| The floor rule, reconciled | working: `scripts/floor_mapping.py` joins the oracle against `builtin_scalar` and fails both ways — every floor row the rule reaches is met or carries a stance, every arm resolves to a floor row, and every stance is about a row that still needs one. **58 of the 82 rows a major are placed by the file's own columns** (`arrow.opaque`, or a driver refusal), 21 of the remaining 24 are simply met, and three carry a stance: `money` below by decision (`KD13`), `regproc` unanswerable because the two encodings denote different values, and `oid` answering `UInt32` where the driver answers `Int32`, which the rule permits. The fourth stance the rule defines — `waiting`, for a row a slice of the open phase closes — is carried by no row now that `interval` and `int2vector` have both closed, and is exercised against a synthetic row in `test_floor_mapping.py`. D8's pin is asserted here — the driver version every row records must equal `scripts/pyproject.toml`'s ([`../design/architecture.md`](../design/architecture.md), "The floor: the ADBC driver's answer bounds ours") |
| Compressed input (`--source foo.dump.xz`) | not started — P13 for xz, P15 for gzip/zstd. Input is assumed already-decompressed plain SQL text; `pg_dump -Fp --compress=…` output is therefore unreadable today ([`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)). **P13 is grilled, partly specified and blocked**: no crate answers a positioned read over an `.xz` file, so the seekable-xz layer is being carved out into its own repository ([`../design/roadmap-P13-compressed-input.md`](../design/roadmap-P13-compressed-input.md), "Blocked") |
| Remote input (`--source https://…`), over `object_store` | not started — P14, carved out of P6. `ByteRangeSource` is already shaped against `get_range`/`head`, and there is exactly one implementation: `LocalFileSource` |
| Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign | not started — P7, which is single-threaded and aimed at the row-extraction path; parallelism is P16 |
| Per-row-group column statistics, sparse row index | not started — the index is built by whichever of P16 (parallel splits) or P10 (row groups) runs first; `CopyBlock::sparse_index` and `CopyBlock::column_stats` stay reserved `None`s |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

**Profiles are not figures.** `cd scripts && uv run measure.py
--profile-recipe` prints the sampling-profile sequence and runs none of it; a
profile is a `runs/` artifact with no median, no apparatus gate and no marker.
Six were taken from that recipe — `parse`, `strings` and `typed` over the
control and the `--arrays --composite` file — with the instrument's own floor
beside them
([`../design/roadmap-P7.1-profiling-apparatus-notes.md`](../design/roadmap-P7.1-profiling-apparatus-notes.md)),
and seven more sit beside them from the decomposition: the `INSERT`-run pair at
two commits in both build configurations, a 4000-block `parse`, and `release`
profiles of the three control shapes
([`../design/roadmap-P7.2-decomposition-notes.md`](../design/roadmap-P7.2-decomposition-notes.md)).
**What a scan spends its time on is
[`../design/architecture.md`](../design/architecture.md), "Where a scan's time
goes"** — the decomposition, which is the durable half of the phase.

**Figures.** Seven of the fifteen figures in
[`../design/measurements.md`](../design/measurements.md) come from the
`ba2fc12` sweep of 2026-09-03, folded in whole, each table carrying an
apparatus line; `allocator`, `per-block-quadratic`, `map-only`,
`preamble-prepass`, `scan-throughput-cold`, `scan-throughput-warm` and
`census-brace-free` were taken on their own afterwards, and each section says
so. `--check` reconciles fifteen markers against fifteen
figures. `session-drift` is derived across that sweep and a second one taken
three minutes later on the same commit, which is the pair `--drift` reads.
`measure.ACKNOWLEDGED` carries the six entries above: a fresh stamp spends
every entry, and `--check` named the previous six so they were deleted rather
than kept as sediment.

**Nothing is built and unrun.** `measure.UNTAKEN` is empty: `projection-widths`
was taken and moved into `FIGURES`, and `composite-isolated` was deleted
unpublished along with its whole apparatus — the `--weak-composite` generator
flag, the `composite_text` input and the fidelity case pairing them — because
the projection table makes the same isolation a subtraction between two adjacent
rows of one file.

**The `INSERT`-run reading that moved ~10% between two stamps was code layout,
not code.** Cold 9.85 → 10.87 s and warm 8.37 → 9.19 on a byte-identical input;
`release` builds at the two commits retired the same instructions to 0.03% and
differed only in cycles, the whole difference sat inside the `scan_buf` of the
day whose 293 instructions were byte-identical between the binaries, and
forcing 64-byte function alignment collapsed the gap. There was nothing to
bisect to. What it changes is how a figure is read, which is now a standing
rule ([`../design/measurements.md`](../design/measurements.md), "Two builds of
one source can differ by layout"). The ratio was never in doubt at the time —
16.5× against 16.7× — and 7.5 has since taken it to 4.3×.

**The sweep's `control` warm floor sits 22.4% above the previous stamp's**,
which is over the ~15% a co-measured floor is judged against — and the sweep
stands, because the disqualifying signature is a slow move *shared* across the
warm floors and the `arrays` file's sits 6.9% below. Drift alone now reaches
that threshold, so the number no longer separates drift from contention on its
own and the shared-move conjunct is what does; the evidence and what it costs
are beside the rule
([`../design/measurements.md`](../design/measurements.md), "The floor is read
directionally").

**Every `<doc>.md`, "section" citation in the tree is resolved by `cd scripts &&
uv run citations.py`, and it is green: 491 citations in 253 files, none
dangling.** The nine `M50` first reported were repaired by `M51`, each read for
what it meant — **seven of the nine were one keystone's sweep**, which repointed
a citation's *document* at `architecture.md` and left its *section* naming a
heading only the deleted doc ever had; the other two were an inbox that has
since been drained and a phase spec renaming its own section as it was written.
Nothing acknowledges or baselines a citation: the check fails rather than
reports, so a dangling one is repaired rather than lived with. A **dated entry
whose day has closed is not read** — an entry states what was true on its date,
so a keystone
that deletes a phase doc leaves its citations dangling by rule and repairing one
is not owed; today's entry is still read, since a typo is wrong the moment it is
written ([`history/README.md`](history/README.md), "An entry carries yesterday's
truth"). Its headings remain resolvable *targets*, which is what the
out-of-band ledger's `Why` column cites. The convention it enforces —
a section whose heading states a measured finding is addressed by an
`<!-- section: <id> -->` marker, and cited by that id rather than by the
heading — is beside the mechanism
([`../design/architecture.md`](../design/architecture.md), "Where a scan's time
goes").

## P7 progress

The phase's spec, its measured baseline and the lever table each row measures:
[`../design/roadmap-P7-scan-performance.md`](../design/roadmap-P7-scan-performance.md).
**7.1 and 7.2 are ordered; the rest is allocation order, not schedule** — the
phase follows the profile, so a slice landing out of numeric order is the plan
working ([`../process.md`](../process.md), "Slice numbering", which carries the
exception an evidence-led phase runs under). **Both orderings that bound are
discharged**: the allocator decision before the wrap sweep, which 7.13 re-took
and settled, and `7.13.1` ahead of `7.6` and `7.7.1`, which rework how a row is
walked inside the buffer it replaced. What remains is unordered.

- [x] **7.1** The profiling apparatus — `[profile.profiling]`, `perf`, and a
      `measure.py --profile-recipe` that prints the invocation on the
      `--koji-recipe` precedent. Six first profiles, and the instrument's own
      floor. No library code. Notes:
      [`../design/roadmap-P7.1-profiling-apparatus-notes.md`](../design/roadmap-P7.1-profiling-apparatus-notes.md)
- [x] **7.2** The decomposition, published — `architecture.md`'s "Where a
      scan's time goes", the `INSERT` +10% attribution, the `INSERT` fast
      path's layer (`map.rs`), and readings for the three measure-only levers.
      No library code. Notes:
      [`../design/roadmap-P7.2-decomposition-notes.md`](../design/roadmap-P7.2-decomposition-notes.md)
- [x] **7.3** The allocator — measured, and **not adopted**: `jemalloc` is
      1.87×/1.19×/1.07× on the three headline shapes and `mimalloc`
      0.98×/0.96×/0.96×, so the lever's factor-sized stake is 3–4% on `typed`
      alone — the confirming sitting puts `parse` and `strings` inside the
      apparatus. It named the re-take 7.13 owed, on the reading that jemalloc's
      `parse` penalty was 3,161 `madvise` calls from that slice's allocation;
      the re-take took the `typed` win away too. Notes:
      [`../design/roadmap-P7.3-allocator-notes.md`](../design/roadmap-P7.3-allocator-notes.md)
- [x] **7.4** `KD5` — `stream::splice` rides the throttle's gate, with a third
      opener (a completed block whose header names the queried table) that
      keeps the early stop exact. A 4000-block `parse` falls **20.75 s →
      0.113 s** and its saves 105 → 5, the two costs having been multiplying;
      the interrupt now banks the last spliced watermark, ~20 ms of scanning at
      that size and nothing at all on koji. `KD5` is rewritten to the residual —
      `--dqcache none`, where the gate never closes — and re-homed onto P16.
      Notes:
      [`../design/roadmap-P7.4-splice-gate-notes.md`](../design/roadmap-P7.4-splice-gate-notes.md)
- [x] **7.5** `KD9` — the `INSERT` fast path at 7.2's layer:
      `preamble::StatementScan`, one incremental byte-level quote-aware scan
      carried across a run's lines, in place of a `String` per line and a
      statement buffer re-walked per line. A warm 3.00 GiB `INSERT` `parse`
      falls **9.19 s → 2.27 s**, 16.5× a `COPY` scan's per-byte CPU → **4.3×**,
      and cold on the SSD the difference is gone (1.02× the device floor
      against 1.01×). `KD9` is **rewritten to that residual**, not struck: the
      accumulation is gone, two named cuts against the remainder are not.
      Notes:
      [`../design/roadmap-P7.5-insert-fast-path-notes.md`](../design/roadmap-P7.5-insert-fast-path-notes.md)
- [x] **7.6** Bulk `simdutf8` over the chunk's whole-row prefix, with the
      borrow path losing its per-field check: a row travels as `copy::RawRow`
      and `copy::validated_prefix` validates a chunk's rows in one SIMD pass,
      lazily, only where something will decode. A `strings` query loses
      **6.11%** of its user instructions and a typed one **2.42%**, every after
      rep below every before rep, with `core::str::converts::from_utf8` leaving
      both profiles. **No `unsafe`**: the row is sliced out of the validated
      `&str` with `str::get`, so a wrong range costs the fast path rather than
      the process. The splitter it needed also took `map::Builder::on_row` off
      a per-byte closure — an `--arrays --composite` `parse` falls **17.781 G →
      2.919 G** user instructions, which is the census re-split no lever claims
      any more: it happens in the mapping pass, where **7.7.1** cannot reach
      it. Notes:
      [`../design/roadmap-P7.6-bulk-utf8-notes.md`](../design/roadmap-P7.6-bulk-utf8-notes.md)
- [x] **7.7** The predicated reading — `predicate-terms`, the first registered
      figure to pass a filter at all: one file at four term counts and two
      field depths, every term false and OR'd so all of them are evaluated and
      nothing survives to be decoded. The same five terms against a 16-column
      table's thirteenth column and against its first differ by **49% of the
      deep one's user instructions**, and that difference is the walk; a
      walk-free term is 0.033 µs a row against 0.09–0.12 for a deep one. No
      library code; one CLI test runs every registered shape and requires each
      to keep no row, which is what the whole subtraction rests on. Notes:
      [`../design/roadmap-P7.7-predicated-reading-notes.md`](../design/roadmap-P7.7-predicated-reading-notes.md)
- [ ] **7.7.1** One field split per row, shared by the predicate's terms and
      `push_row`. **Earned**: 7.7's row paired the instrument that measures
      this lever with the lever itself, which is two review cycles and not one.
      Reviewed alone, and **bounded** by 7.7's reading rather than merely
      motivated by it — one whole-row split replaces N partial ones, so it wins
      on a many-term predicate and loses on a single shallow one. The census
      left this row's scope for good: it splits in the *mapping* pass, so no
      arrangement of this lever reaches it
      ([2026-09-04](history/2026-09-04.md), "7.7 is smaller and narrower than
      its rows said").
- [ ] **7.8** The I/O defaults — the cold-NVMe figure, then readahead,
      `posix_fadvise` and the chunk-size constant, each landed or rejected
      against it. That figure is also what `KD9` is read against: it is the
      one device we own on which an `INSERT` run's CPU could outrun the read.
- [ ] **7.9** `decode_array`'s `Vec<Option<String>>` intermediate, replaced by
      borrowed slices where the literal carries no escapes.
- [ ] **7.10** Scalar decode and the typed column build, split by the profile
      into a `decode.rs` half and a builder-append half.
- [ ] **7.11** The viewing builder for `List<Utf8View>` — conditional on
      7.10's builder-append half putting the `List<Utf8View>` build above
      1 µs/row, last, reviewed alone.
- [ ] **7.12** The sweep pair and the koji regression run, folded in, plus the
      written statement of what a parallel splitter needs from coverage and
      from the census, filed to P16.
- [x] **7.13** The read path's per-chunk zeroed allocation, pooled behind the
      `object_store` shape: a warm `parse` loses **9.4% of its user
      instructions** (1.882 G → 1.706 G, ±0.00% either side) and
      `__memset_avx2_…` leaves the profile entirely, wall being unchanged
      because the prize was inside the quarter-second discovery already sat
      within. `--figure allocator` re-taken, and **adoption settled: the
      platform allocator stays** — `jemalloc` 1.02×/1.11×/1.06× and `mimalloc`
      1.00×/0.99×/1.01×, both cells that had kept the question open having been
      this allocation rather than an allocator. Notes:
      [`../design/roadmap-P7.13-read-buffer-pool-notes.md`](../design/roadmap-P7.13-read-buffer-pool-notes.md)
- [x] **7.13.1** The copy into each read loop's own buffer, over all three
      loops: `scan::ChunkCarry` carries the one line straddling a chunk's front
      edge and every loop scans the rest of the chunk where it lies. A warm
      3.00 GiB `parse` loses **17.6% of its user instructions** (1.704 G →
      1.404 G, spreads under 0.003%), `__memmove_avx_…` leaves the profile
      entirely, and this one **reaches wall time** — 0.48 s → 0.40 s with
      system flat, peak RSS 7.9 → 6.0 MB. The query path's chunk retention
      needed no change: eviction keys on the scanner's position and a
      straddling row is carried, not scanned. **Earned**: 7.13's row paired one
      contained module with a rework of three already-tested scan loops, which
      is two review cycles. Notes:
      [`../design/roadmap-P7.13.1-read-loop-carry-notes.md`](../design/roadmap-P7.13.1-read-loop-carry-notes.md)

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P7 is open**, grilled and sliced; the checklist above is its progress. Its
  three evidence slices, the allocator reading and five library changes have
  landed — 7.4's gate in `stream.rs`, 7.5's `INSERT` statement scan in
  `preamble.rs`/`map.rs`, 7.13's read-buffer pool in `io.rs`, 7.13.1's read
  carry in `scan.rs`/`stream.rs` and 7.6's bulk UTF-8 pass in
  `copy.rs`/`stream.rs`, all edits to timed paths. Six
  other phases are sketched and one more is
  specified — P13, P16, P10, P14, P6, P15, P8, in the roadmap table's schedule
  order; a `P<k>` is an identifier, so the numbers say nothing about the order
  they run in. P13 is grilled, specified and **blocked** on an external
  seekable-xz crate. Every remaining phase that carries an inbox must have it
  drained as part of its own grilling.

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
rather than being deleted. <!-- deficiency-watermark: KD13 -->
**`KD1`–`KD13` are allocated, and nothing at or below `KD13` is reused** — a
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

- **KD5** — a map rebuild is still a whole-list clone, so mapping is
  O(blocks²) wherever the save throttle's gate does not close it — which is
  every `--dqcache none` scan, since a no-op save leaves nothing to amortize:
  19.1 s for 4000 blocks. **(b) owned by P16**, which reworks `splice` for a
  parallel splitter anyway and is where the appendable-spans fix belongs.
  Detail: [`../design/architecture.md`](../design/architecture.md), "`parse`
  resumes, and saves as it goes".

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

- **KD8** — a typed column cannot hold `infinity`, `-infinity` or `NaN`, nor —
  on an `interval` — a time part past `2562047:47:16.854775807`, so
  materializing one raises `Error::FieldDecode` and there is no typed way to
  read the value. **(c) unowned**; promoted by whichever phase takes typed
  materialization, which is where the choice between a null, a sentinel and the
  error belongs. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Decoders and
  render-back".

- **KD9** — an `INSERT` run costs **4.3×** a `COPY` scan's per-byte CPU warm,
  and two specific cuts against that remainder are known and untaken. **(c)
  unowned**; promoted by slice **7.8**'s cold-NVMe figure, the only reading
  that says whether the remainder reaches a real device — the SATA SSD hides it
  and the project's goal names NVMe. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Bulk regions: one
  span kind, three payloads".

- **KD10** — a column whose declared type this build models no comparison for
  answers `=`/`!=` bytewise, which is not the server's answer for the geometric
  types (`box_eq` compares areas), so the row set is wrong; ordering is refused
  outright, and the announcement misses a type reached through a container
  (`box[]`). **(c) unowned**; promoted by a dump whose queried columns are
  geometric or hold a `money`-shaped extension type. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Equality is typed
  too".

- **KD13** — `money` is below the ADBC floor: the driver answers `int64` and we
  answer `Utf8View`, because `cash_out` renders through the monetary locale and
  `pg_dump` sets `lc_monetary` nowhere, so the file cannot say which locale
  wrote a value. **(a) deliberate tradeoff** — closing it means guessing a
  locale or asking for one, which the bar refuses for every other type. Detail:
  [`../design/architecture.md`](../design/architecture.md), "The floor: the ADBC
  driver's answer bounds ours".

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

### `predicate-terms` prices a filter on a shape that returns no rows

**The call.** All six rows of the new figure ask an all-false disjunction, so
every term is evaluated and no row survives — the decode, the Arrow build and
`print_batch` are identically absent from every row of the table. That is what
lets one table carry both a term-count axis and a field-depth axis, and it is
what makes an adjacent-row difference the predicate and nothing else.

**Why it was made that way.** The alternative shape — a filter every row
satisfies, over a projection that builds everything — cannot be built on the
control file: every column but `id` carries 2% NULLs, so an N-term conjunction
is `Unknown` on 1 − 0.98^N of the rows and drops them, moving the emit cost the
subtraction needs held constant. Confining the terms to `id` fixes that and
leaves a single field depth, which is the axis the lever is sized on.

**What would change if it were reconsidered.** The table stops being an
absolute a user recognises — nobody runs a query for its zero rows — and the
shape outlives this slice, since 7.12's sweep re-takes it. The additive answer
is a second figure on a keeps-everything filter, deliberately not taken here;
reversing this one instead means either giving up the depth axis or generating
a NULL-free input, which is a new apparatus input for one table.
