# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

P1–P5, P7, P9, P11–P13 and P17 are complete and were struck at keystone
reviews; how each mechanism works is
[`../design/architecture.md`](../design/architecture.md), filed by subject,
which is where a session touching one meets its rejected alternatives and its
limitations. The capability table below says what state each is in.

[`../design/measurements.md`](../design/measurements.md) carries the `af15eac`
stamp of 2026-09-05, and **seventeen of its eighteen tables come from one
sitting**: sixteen from the scan-performance wrap sweep and `session-drift`,
which no sweep can take because it is derived *across* two, from that sweep and
a second begun the minute it finished. The eighteenth is `peak-rss`, taken alone
at `7ee5db5` and saying so at its own table; it shares no reading with any of
the others, so no table carries a partial-sitting note, no absolute in the
document is a cross-sitting reading, and `measure.UNTAKEN` is empty. The fresh
stamp spent all six of the previous acknowledgements, which were deleted rather
than kept as sediment; the two the register carries now are
P7's wrap and keystone, and they are comment-only. Under the previous `ba2fc12`
stamp ten of the seventeen stood outside the sweep.

**The largest correction the register has carried is `census-arrays`.** Its warm
census cost read 1.045 s and 1.49 µs a row under the old stamp and now reads
**0.201 s and 287 ns** — a factor of five — because the census's field split
went behind `memchr` and the split was the byte loop, not the census. It was
red on `pgdump_query/src/copy.rs` throughout, so nothing about the register
missed it; what a red figure never says is *how far* a number has moved.

**Four other findings changed shape rather than magnitude.** The census's two
tiers are now within a factor of four of each other — the pre-filter is 25% of
what an inspected row costs, against 4% before — so "one tier, not two" no
longer holds. `census-attribution`'s baseline gap fell from +0.874 s to
+0.033 s, which is inside the drift figure, so the untyped baseline's
file-dependence is now invisible without the census-off binary rather than
obvious with it. `predicate-terms` measures the shared field split, and its
term-count axis has gone flat: five terms at one depth cost +0.05 µs a row over
one, against +0.37 before. And the I/O-defaults ceiling fell from 8.9% to
**5.8%**, because the read-path work took the parse below the device by more
than it took the device — every I/O lever declined at 8.9% is declined harder
now.

**The query path roughly halved.** A typed control query went 10.36 s → 4.75 s
and the `--arrays --composite` file 19.93 s → 9.12 s, against `strings` legs of
4.42 s → 3.44 s and 5.36 s → 3.45 s; the nested-column increment went
**13.5 µs → 6.5 µs a row** and every row of `projection-widths` about halved
with the same ordering intact. The scalar decoders, the read path and the four
render-path changes are what moved them.

**One reading weakened a settled claim without reopening it.** `mimalloc` now
reads 0.97×, 0.97× and 0.99× on the three headline shapes — marginally ahead of
the platform allocator on all three, where the previous sitting read
1.00×/0.99×/1.01×. Every cell's spread overlaps the reference's and the largest
gap is 3%, so the decision stands and the platform allocator is kept, but the
statement behind it has weakened from "nothing beats it" to "nothing beats it by
more than the instrument's own noise". Reviewed and affirmed at P7's wrap; what
would reopen it is an instrument that resolves a 1% wall difference, not another
sitting of this one ([`../design/architecture.md`](../design/architecture.md),
"The allocator is the binary's choice").

**The pair that produced this stamp cleared the contention gate on the number
alone**, which the previous stamp did not: no warm floor is near the ~15%
threshold, and the two files' floors moved in *opposite* directions — `control`
−2.5% to −5.3%, `arrays` +3.4% — which is the opposite of the shared slow move
that disqualifies a sweep
([`../design/measurements.md`](../design/measurements.md), "The floor is read
directionally"). Session drift over 92 shared readings is a median absolute
**1.6%** and a largest 14.3%.

**Seventeen of the eighteen figures are stale, and no acknowledgement can
excuse them.** Three rounds of library work did it. Making the buffer pool keep
the chunk size a read loop announces changed `io.rs`, `scan.rs`, `stream.rs` and
the CLI; then the compressed-input work reshaped every `ByteRangeSource`
signature to a boxed future and added a second implementation, touching `io.rs`,
`cache.rs`, `index.rs`, `diagnostic.rs` and the CLI again; then the
cache-replacement work put a refusal in front of all three scan entry points,
touching `cache.rs`, `index.rs`, `stream.rs` and the CLI once more. So every figure that
times a run is red, and `session-drift` has been red on `scripts/measure.py`
since the harness took the derived direction of the borrow graph. Those changes
add executable lines, so neither mechanical oracle applies: reachability excuses
only a diff no command shape executes, and byte-identity settles generator
changes alone. `nested-decode-micro` is the one figure still green, timing
decoders that none of it touched. **One published number actually moves**, the
`chunk-size` table's 16 MiB row, which was taken when a chunk that large missed
the buffer pool; that row is called out where it stands. A stale figure obliges
no sweep ([`../design/measurements.md`](../design/measurements.md), "A stale
figure does not oblige a sweep"), and a sweep is what re-takes these: seventeen
of the eighteen tables from one sitting is the property the `af15eac` stamp has
and a partial sitting would spend.

`peak-rss` is the one whose staleness changed character rather than arriving
with the rest. It was a **false positive** until the compressed-input work: taken
at `7ee5db5`, later than every commit that had then touched the paths it
declares, and red only because the harness reads one commit for the whole
document rather than each figure's own sitting — `M60` is that fix and is queued
rather than landed. It is now genuinely stale, the `io.rs` and `cache.rs`
changes beneath it being executable ones.

The two acknowledgements the register carries still stand and still hold for
what they name. `measure.ACKNOWLEDGED` records P7's wrap and keystone, whose
every hunk is a comment, a docstring, or a `quoted_by` edge into the phase docs
they deleted; each entry names the diff that re-checks it.
`scripts/acknowledged.py` is its own module precisely so that an acknowledgement
edit does not re-stale the stamp it was just given.

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working; a batch flushes on whichever of `max_rows`, `max_bytes` or `max_source_span` comes first, the last of which is what bounds the read chunks an in-flight batch pins ([`../design/architecture.md`](../design/architecture.md), "Three flush triggers") |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working. `interval` is `Interval(MonthDayNano)` — PostgreSQL's own three fields — with v17's infinities and a time part past `2562047:47:16.854775807` an `Error::FieldDecode` and `--schema-mode strings` the recourse. `oid` is `UInt32` — PostgreSQL's one unsigned integer type, mapped where the ADBC driver's `Int32` turns an OID at or above 2^31 negative. `int2vector` is `List<Int16>` — the one built-in whose Arrow type is a container and whose name is not spelled like one, written as space-separated `int16`s with no quoting and no NULL element (I47), so it travels with a `NestedPlan` of its own exactly as a multirange does. A `uuid` column's field carries the canonical `arrow.uuid` extension name and a `json`/`jsonb` column's `arrow.json`, top level only and written through arrow-rs's own extension types, so neither changes a byte. Render-back has two entry points — `render_field`, returning `Result<Option<String>, Error>`, and the sink `render_field_into(.., &mut String)` it wraps, which appends and answers `Result<bool, Error>` so a caller printing a row builds it in one buffer — and a third outcome besides a value and SQL NULL: `Error::FieldRender` is an Arrow value with no PostgreSQL text form — reachable only from an array a caller assembled, since every column this crate fills comes from a decoder whose range its renderer writes back, and carried by `interval` alone, whose nanoseconds are finer than PostgreSQL's microseconds ([`../design/architecture.md`](../design/architecture.md), "Type resolution" and "Decoders and render-back") |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working. `CacheMode::load` answers a `CacheLoad` — an index, or which of five reasons there is none: the four unusable `CacheStatus`es carried across plus the caller's own `Disabled`, which no status can express — so a caller that is about to scan holds the reason before it does any work rather than after. **The library never replaces cache data automatically**: the three scan entry points refuse a `SourceChanged` cache with `Error::CacheSourceMismatch`, naming the path, what the cache was written for and what this source is, before a byte of the dump is read; the other three unusable statuses still start cold, there being nothing at that path worth keeping. There is no override — removing the file or naming another path is the way out, and **every refusal says both in the same words**: the library's size-mismatch error, the CLI sentence a contradicted compression claim gets, and `info`'s `SourceChanged` arm, which names the two ways out *before* `pgdq parse` because `parse` refuses that same condition. The other three `info` sentences still reach `pgdq parse` on its own ([`../design/architecture.md`](../design/architecture.md), "The cache" and "The CLI's two refusals are worded as one") |
| Arrays, composites, ranges, multiranges, `int2vector` | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<`range struct`>`, `List<List<T>>`, and `int2vector`'s `List<Int16>` — a fifth literal form with no wrapper, no quoting and no NULL element (I47), compared through `anyarray` polymorphism's `array_lt`/`array_eq` so `'2' < '10'` is true where a byte comparison says false. Three shapes stay strings, each with its own resolution outcome — an opaque element type, an element type that is itself an array (I26), and values that disagree on shape. **Every container kind now compares structurally** — element-wise, field-wise and bound-wise through a `ComparisonPlan::Nested` tree, `array_cmp`'s shape tie-break, and one NULL rule at every level (I45) — with the literal read through the `*_in` supersets (I44) and each leaf in its own type's output form. Comparability and divergence are both inherited: a `json` position refuses the column's *ordering* and names itself, a `text[]` announces its element's collation. A column whose order one position refuses still answers `=` — over the container's whole text — and **says what that costs at the position that did it**, since `array_cmp` raises for a `json` element rather than comparing, so bytewise is an answer the server does not have; a position the *resolver* declined instead (I22, I26) resolves the column to text before any tree is read and is silent. **A range is put into the form the server stores it in before it is compared** (I46): `range_serialize`'s out-of-order refusal and empty-collapse, then the canonical function the three discrete built-ins have, so `int4range '[1,10]'`, `'(0,10)'` and `'[1,11)'` are one value and `'(1,2)'` is `empty`; a multirange's members are sorted, coalesced and emptied out before the sequence is walked. A user-defined range declaring a `canonical` function is refused under **every** operator, `=` included, since the server rewrites both operands through arbitrary server-side code before comparing them |
| Array shape census | recorded by every mapping pass and consumed: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, cache-only `info` | working; **`parse` is the only scanner** — it resumes from a matching cache, banks at `COPY` block boundaries under a self-tuning throttle, and saves unconditionally on Ctrl-C (exit 130/143). `info` reports from the cache and never scans. Both scanning commands take `--chunk-size <bytes>`, whose 1 MiB default is the fastest of six sizes measured on the one device class where the size makes a difference ([`../design/measurements.md`](../design/measurements.md), "What the read chunk size is worth"); a raised value keeps its pooling, because each read loop announces the length it repeats, and costs up to four buffers of that size in RSS instead. `--verbose` adds each block's byte offsets, a per-column resolution line, an enum column's declared labels beneath it, and — under the `user-defined types` count that heads it — one line per user-defined type, every `TypeKind` arm rendered with its payload. Text output shape is provisional; `--json` carries no shape promise at all, and states the labels once per type in `metadata.databases[].types[]` rather than per column |
| Partial reporting | `info` reports an unfinished scan's cache for as far as it got, with `Scan completion: N%` stated once at the top and nothing below it qualified. An interrupted cache is **typed** for every database segment the scan finished (I1) |
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — eighteen figures, sixteen taken by a sweep, one derived across two and one taken alone, each declaring what invalidates it, which documents repeat it, and which readings it borrows from another figure — that third edge is what lets `--figure` pull in what a figure borrows and name the rest of the set that must be re-taken with it, and `--alone` is how a partial sitting is asked for deliberately. **The boundary of that register is declared rather than inferred**: a section outside it carries an `<!-- outside-register: <id> -->` marker, `--check` resolves those against `measure.NOT_OURS` in both directions and fails on a declared section that also carries a figure marker, and the doc's session stamp is scoped to the markers rather than to everything printed below it ([`../design/measurements.md`](../design/measurements.md), "The apparatus"). Its reverse direction is **named rather than taken**: a table whose row is a difference over another figure's reps — `cross-file-floor` over `nested-end-to-end`, the register's one such edge — is not a closure edge, so `--figure` names it in the run log and in the emitted header when a sitting re-takes the reps it is derived from, and `--check` reports the relationship beside the partial sittings ([`../design/measurements.md`](../design/measurements.md), "The apparatus"). `measure.UNTAKEN` is empty: nothing is built and unrun. **The one binary it refuses to build, it now refuses to trust unstamped**: `runs/pgdq-nocensus` carries a `.stamp` naming the commit it was built from, the way a generated input does, and a `census-*` figure whose stamp is missing, unreadable as a commit, or not the commit being measured is refused in the first second — a census figure being a subtraction that charges everything differing between the two trees to the census ([`../design/measurements.md`](../design/measurements.md), "The census-off binary is a source patch"). The binary in this tree is `f5768e7`, so the next census sitting rebuilds and re-stamps it. It also builds and interrogates the `allocator` figure's three legs, reading each binary's allocator out of `pgdq --version` rather than trusting the flags it passed, and names the shipped one in the session stamp. A leg is rebuilt **once per harness process** rather than reused from `runs/`, which is what stops a fresh reference being timed against last session's legs, and all of them are built before the first reading rather than at the rep that wants one |
| Column projection | working, library and CLI: `QueryOptions::projection` names columns, cuts the reported `ResolvedSchema` with the batches, may reorder, and may be empty (`COUNT(*)`); `pgdq query` spells it `--column <name>` repeated, or `--no-columns`, which prints no header so `\| wc -l` is a row count. A filter may name a column the projection does not, and an unprojected column is never decoded, so projecting a column away escapes its `Error::FieldDecode` — including `KD2`'s, which the error message does not name ([`../design/architecture.md`](../design/architecture.md), "Projection"; [`../manual/type-handling.md`](../manual/type-handling.md)). Measured on one 3.00 GiB file at five widths: `--no-columns` is 1.61 µs a row against 12.97 for all 19, the two array columns alone are +6.03 and the composite +0.75 ([`../design/measurements.md`](../design/measurements.md), "What a column costs") |
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
| Compressed input (`--source foo.dump.xz`) | **working** — `.xz` reads end to end through `parse`/`info`/`query`, in all three container shapes, the span offsets it produces being uncompressed ones so nothing above L1 knows. `pgdump_query::open_local` recognises a source by **content**, not by name, and hands the CLI's three commands an `XzSource` or a `LocalFileSource`; `XzSource` answers `size()` from the stream index and `stored_size()` from a `stat`, decodes through one streaming reader restarted on seek, and hands its seek table to the cache, which persists it in a `CompressionIndex` envelope field beside a `ContainerKind` that stays `Plain`. **That table is read back**, so the stream-footer walk — one read for a single-stream file, **85 s** for the 31,150-stream koji download — is paid by the command that first parses a file and by nothing after it: `cache::known_compression` answers a three-state `KnownCompression` before any source exists, `open_local` takes it and builds the source through `XzSource::with_table` without walking, and a claim the file contradicts is `Recognized::Mismatch` — the whole cache unusable, since table and span index were one `save`, and refused by all three commands having read nothing, in a sentence of its own rather than `CacheStatus::Unreadable`'s. A file with no more than one block is **warned about, never refused** (`DiagnosticKind::NonSeekableCompressedSource`, naming `xz -T0` and `--block-size=<size>`). The decoder is a frozen read-only vendored copy of `xz-seek` at `vendor/xz-seek/` (`CLAUDE.local.md`), not a published dependency, since this project is its first consumer and that consumption is what vets the interface. gzip/zstd are not read — `pg_dump -Fp --compress=…` output is unreadable today and is P15's ([`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)) ([`../design/architecture.md`](../design/architecture.md), "The compressed source") |
| Remote input (`--source https://…`), over `object_store` | not started — P14, carved out of P6. `ByteRangeSource` is already shaped against `get_range`/`head`, and there is exactly one implementation: `LocalFileSource` |
| Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign | **complete** — P7, single-threaded throughout and aimed at the row-extraction path; parallelism is P16. Twelve library changes on timed paths, four measured refusals, and the decomposition that is its durable half ([`../design/architecture.md`](../design/architecture.md), "Where a scan's time goes"). Warm on the 3.00 GiB control a typed `pgdq query` is 15.0× the `dd` floor where it was 31×, a `strings` one 10.9× where it was 13.3×, and a `parse` 1.43×; cold on the SATA SSD every scan shape is inside the device, and cold on NVMe the `COPY` path is 1.06× it. What it refused, and why, is beside each mechanism as a rejected alternative |
| Per-row-group column statistics, sparse row index | not started — the index is built by whichever of P16 (parallel splits) or P10 (row groups) runs first; `CopyBlock::sparse_index` and `CopyBlock::column_stats` stay reserved `None`s |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

**Profiles are not figures.** `cd scripts && uv run measure.py
--profile-recipe` prints the sampling-profile sequence and runs none of it; a
profile is a `runs/` artifact with no median, no apparatus gate and no marker.
Some three dozen sit in `runs/` from the scan-performance campaign, over every
shape the recipe knows and both build configurations. **One profile input is
not a registered one**: reading the refused viewing builder's gate needed a
`List<Utf8View>` column and no committed input carries one, so that reading was
taken on a purpose-built `text[]` file whose specification is beside the
refusal ([`../design/architecture.md`](../design/architecture.md), "The
library's own per-row budget"); the generator lives in `runs/` and nothing
consumes its output. **What a scan spends its time on is
[`../design/architecture.md`](../design/architecture.md), "Where a scan's time
goes"**.

**Figures.** All sixteen sweep figures in
[`../design/measurements.md`](../design/measurements.md) come from the
`af15eac` sweep of 2026-09-05, folded in whole, each table carrying an
apparatus line and none carrying a partial-sitting note. `--check` reconciles
eighteen markers against eighteen figures, **and the register's boundary as
well**: the two sections the harness does not own — koji and the `cargo bench`
tripwires — each carry an `<!-- outside-register: <id> -->` marker reconciled
against `measure.NOT_OURS` both ways and held to carrying no figure marker, so
the session stamp's "every figure below" now claims only what the register
holds. `session-drift` is derived across that sweep and a second begun the
minute it finished on the same commit, which is the pair `--drift` reads;
`peak-rss` is the eighteenth, taken alone at `7ee5db5`.
`measure.ACKNOWLEDGED` carries the two commits of P7's wrap and keystone, both
comment-only against a declared path; the previous six were spent by this stamp
and `--check` named them so they were deleted rather than kept as sediment.

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
16.5× against 16.7× — and the `INSERT` statement scan took it to 4.3×, where the read path's own gains have since put it at 4.9×.

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
uv run citations.py`, and it is green: 552 citations in 247 files, none
dangling.** The nine the check first reported were repaired, each read for
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

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **No phase is open**, and none carries centering. The cache-replacement work
  wrapped and was struck at a keystone review, as the compressed-input and
  scan-performance work were before it. What each built and what each refused is
  beside its mechanism in
  [`../design/architecture.md`](../design/architecture.md), filed
  by subject. Six phases are sketched — P16, P10, P14, P6, P15, P8, in the
  roadmap table's schedule order; a `P<k>` is an identifier, so the numbers say
  nothing about the order they run in. Each gets its own full
  grilling when it becomes current, and every one that carries an inbox must
  have it drained as part of that grilling.

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
rather than being deleted. <!-- deficiency-watermark: KD15 -->
**`KD1`–`KD15` are allocated, and nothing at or below `KD15` is reused** — a
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

- **KD9** — an `INSERT` run costs **4.9×** a `COPY` scan's per-byte CPU warm
  and **2.62×** the device's own time cold on NVMe against 1.06×, and two cuts
  against that remainder are known and untaken. **(b) owned by P8**, whose
  Track A row reader extends the very scan both cuts are in; the cold-NVMe
  figure confirmed the entry where it might have retired it. Detail:
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

- **KD14** — peak resident set is flat in dump bytes but grows ~9.9 KB per
  `COPY` block, so a 4,000-block `parse` holds **43.6 MiB** against a one-block
  one's 5.9 MiB, and what accumulates is not attributed. **(c) unowned**;
  promoted by a dump with tens of thousands of blocks, which nothing in hand is
  — koji has 74. Detail:
  [`../design/architecture.md`](../design/architecture.md), "`parse` resumes,
  and saves as it goes".

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

None open.
