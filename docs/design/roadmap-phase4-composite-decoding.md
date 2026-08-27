# Phase 4 — Composite value decoding

Arrays, composite types, ranges and multiranges: the four families
`resolve_declared_type` currently answers with `TypeOutcome::Deferred`, leaving
the column `Utf8View`. This phase gives each one a structured Arrow
representation and an exact render-back.

There is no `roadmap-phase4-inbox.md` — no earlier phase filed a fact for this
one.

## What the dump actually contains

The four families are **not** one quoting rule wearing three hats. I20 records
the register: an array backslash-escapes inside a quoted element (`"` → `\"`),
while a composite field and a range bound *double* (`"` → `""`), and the three
force-quote on different character sets. A composite inside an array is
therefore escaped both ways at once — `array[row(1,'x,y')::c]` reaches
`copy::decode_field` as `{"(1,\"x,y\")"}`, one backslash layer for the array
and one doubling layer for the record.

So what gets written once is a **quoted-token scanner parameterized by
wrapper, separator, force-quote set and escape convention** — not a single
decoder. Three instantiations sit on top of it; a multirange needs none, since
`multirange_out` concatenates its members' `range_out` results with no escaping
step at all.

The other load-bearing fact is I21: an array's dimensionality and lower bounds
belong to the *value*, not the column. `integer[][]`, `integer[3]` and
`integer[]` are all written `integer[]`, and one column can hold `{1,2}`,
`{{1,2},{3,4}}` and `[0:2]={7,8,9}` in three consecutive rows.

## Type mapping

Each family resolves through the *same* `resolve_declared_type`, recursively,
so nesting composes without a special case:

| Declared | Arrow |
|---|---|
| `T[]` | `List<resolve(T)>` — unless `T` resolves to an array or an opaque type, which are refused (below) |
| composite (`CREATE TYPE … AS (…)`) | `Struct<` one field per declared field, in declaration order `>` |
| range (built-in or `CREATE TYPE … AS RANGE`) | `Struct{lower: S, upper: S, lower_inclusive: Boolean, upper_inclusive: Boolean, empty: Boolean}` |
| multirange | `List<` the range struct `>` |

`public.comp[]` is `List<Struct<…>>`; a composite with a `text[]` field is
`Struct<…, List<Utf8View>>`. A leaf whose own type does not map — `interval`, an
opaque base type, an unknown — is `Utf8View` **in that position**, exactly as it
would be at top level. Nothing about recursion introduces a new failure mode:
the recursion bottoms out on the existing mapping table, whose worst answer is
already a string.

*Rejected:* one-level-only containers, with a scalar-element requirement and a
whole-column `Utf8View` fallback otherwise. The depth check is more code than
the recursion it forbids, and it would strand `text[]`-inside-a-composite —
which the recursion handles for free.

**A composite's field list must be all-or-nothing.** `parse_create_type` builds
`Composite { fields }` with a `filter_map`, so a field fragment it cannot parse
is dropped and the type keeps a short list. That is harmless while composites
are strings and unacceptable after the flip, because **`record_out` is
positional with no field names**: unlike a `CREATE TABLE` column list, which
`resolve_columns` joins against the `COPY` header *by name* — so a dropped
column merely resolves `NotDeclared` and stays a string — a composite has no
join to fall back on. A three-field type parsed as two makes the field-count
refusal below fire on every row of entirely valid data, and any later
relaxation of that check turns the same defect into a silent type shift, with
field 3's text decoded as field 2's type.

So a `CREATE TYPE … AS (…)` whose body contains any unparseable fragment
yields **no** field list, and the column resolves to `Utf8View` with a reason,
like anything else the grammar does not recognize. The `CREATE TABLE` path
keeps its `filter_map` — the name-join is what makes a dropped column safe
there, and the asymmetry is deliberate rather than an oversight to be tidied.

**A zero-field composite maps.** `CREATE TYPE x AS ();` is legal, the parser
already yields `Composite { fields: [] }`, `record_out` writes `()`, and a
zero-field `StructArray` is well-formed Arrow. *Rejected:* refusing it as an
`EmptyComposite` by analogy with `EmptyEnum`. The analogy fails: a zero-label
enum is refused because there is no value its dictionary can ever hold, whereas
`()` is a real value that round-trips exactly.

**The range struct's fifth field is not redundant.** `empty` and `(,)` are
different ranges and both have absent bounds, so inclusivity flags alone cannot
tell them apart. A range bound can never be SQL NULL (I20), which is what makes
a null `lower`/`upper` mean "unbounded" without ambiguity.

**Twelve built-in names need subtypes hardcoded, not six.**
`TypeDef::Range::subtype` is populated only for a user-defined range, because
PostgreSQL keeps a built-in's subtype in the catalog rather than in DDL text.
So `int4range`→`integer`, `int8range`→`bigint`, `numrange`→`numeric`,
`tsrange`→`timestamp without time zone`, `tstzrange`→`timestamp with time
zone`, `daterange`→`date` — **and the same six subtypes again for the PG14+
multirange companions** `int4multirange`, `int8multirange`, `nummultirange`,
`tsmultirange`, `tstzmultirange`, `datemultirange`, which `map_builtin` already
recognizes by bare name (I10). `numrange`'s bounds land on `Utf8View`, since
bare `numeric` does.

**The flip must introduce a range/multirange distinction the current code does
not carry.** All twelve names answer `Deferred(DeferredKind::Range)` today,
because nothing downstream needed them told apart; after 4.4 they produce
different Arrow types and different plans. The same holds one level over: a
name with no `CREATE TYPE` of its own that `resolve_user_type` recognizes as a
range's auto-created multirange companion (via `multirange_type_name`, I10)
also answers `Deferred(Range)` today and must answer a multirange after the
flip. Hardcoding the six range entries and letting the other six inherit them
produces a `Struct` where the file holds `{[1,10),[2,3)}`, and fails at the
first row.

*Rejected:* leaving ranges and multiranges as `Utf8View` on the grounds that
Arrow has no range type and the struct is our invention. It is an invention,
and pre-1.0 explicitly frees the shape from compatibility obligations; against
that, a `tstzrange` whose bounds arrive as real timestamps is the same
information recovery every other row in the mapping table exists for. koji has
no range columns either way — it has 6 array columns, all `text[]`, and no
composites.

## An array column's Arrow type cannot be settled from DDL alone

I21 is a direct conflict with "one schema per stream, resolved up front"
(`architecture.md`). `List<T>` commits to one dimension before the first batch,
and the file is allowed to disagree at any row.

The baseline is that **`List<T>` means 1-D, and a value with `ndim ≠ 1` or an
`[lb:ub]=` prefix is a hard `Error::FieldDecode` naming the column.** It is the
asymmetry the `COPY` grammar already chose — not recognizing something is
recoverable and inspectable, misreading it is not — and the escape hatch
already exists: `--schema-mode strings` hands back the literal verbatim.

*Rejected:* flattening a multi-dimensional value into the flat list. It loses
the shape silently, which is the one failure mode this project does not accept.
Dropping an `[lb:ub]=` prefix is the same defect in smaller print: the elements
survive, the index origin does not, and render-back stops being exact.

**After 4.5.1 the error has one reachable cause and one remedy, and says so.**
A top-level array column is retyped from the census before the schema commits,
so the refusal cannot fire there. What survives is an array nested inside a
composite, or inside another array's element type: the census is keyed by
column and has no slot for a value at that depth (see "Only arrays need the
census"). Scanning more of the file cannot help — the evidence has nowhere to
go — so `--schema-mode strings` is the only way to the data, and the message
names it. Stating it unconditionally is safe precisely because the other cause
is gone; while both were reachable, a message naming one remedy would have been
wrong half the time.

### The shape census is an upgrade, not a gate

The mapping pass already reads every row of every block it maps, so the census
rides a read that happens anyway: a caller does not opt into it, does not
choose a slower command to get it, and does not pay a second pass for it. What
varies is not whether a census exists but **how much of the file it covers**,
and that only matters where a claim outruns the rows being handed back.

- **A streamed schema is censused, always.** `table_stream` fixes its schema
  after mapping and before emitting, over exactly the blocks it will replay
  (see "What the census stores, and when it may be believed"). So every
  top-level array column's shape is known before the schema commits, and the
  `FieldDecode` refusal above is unreachable for those columns — on a cold
  query as much as on a full scan. An array nested inside a composite keeps the
  optimistic path, for the reason below.
- **A reported schema is censused only as far as the map reaches.** `pgdq info`
  answers "what is this table's schema" from an index alone, with no replay to
  bound the claim, so a block past the frontier has no census and its columns
  read back optimistically. `pgdq parse`, or a query run with
  `ScanExtent::Full`, closes the gap — the same two-path shape as
  `Error::MetadataNotScanned`, and **both paths belong in
  `docs/manual/type-handling.md`**, the optimistic case and its remedy stated
  together rather than left to be discovered.

  Today no CLI surface can reach this case: `pgdq info --source` re-scans
  whenever the cache does not cover the file, and cache-only mode refuses an
  incomplete cache outright. That is being reversed — `info` will report what a
  partial index knows and mark it partial — as part of the machine-readable
  resolution work, which has no spec yet (`STATUS.md`, "Not started"). Neither
  the census nor 4.5.1 waits on it.

*Rejected:* gating — erroring up front with `ShapeNotScanned` unless a census
exists. koji's array columns are certainly 1-D (they are entirely NULL), so
gating would fail a cold query and demand an hour of `pgdq parse` to learn
nothing.

*Rejected:* a transparent query-time prepass over the target blocks. That is a
*second* read added for the census's sake, and it silently doubles the cost of
every cold array query — the cost this project is least willing to hide. What
lands instead is not that: the mapping pass exists regardless (mapping and
streaming are separate passes, so the target block's bytes are read twice
whatever the census does), and the census is per-row work folded into a read
already being performed.

**Only arrays need the census.** A composite's shape is fixed by its `CREATE
TYPE` and a range's is fixed outright, so the scan is spent entirely on array
dimensionality and lower bounds. Its per-row cost is a `{`/`[` test over the
row's bytes, and a field split only on the rows that pass it. The result is
persisted in the cache beside `DumpIndex` (a format bump) so that even that is
paid once per file.

### What the census says

A censused array column resolves by what the file actually holds:

- every value 1-D → `List<T>`; every value 2-D → `List<List<T>>`, and so on
  for any uniform depth;
- **mixed dimensionality across rows, or any value carrying an `[lb:ub]=`
  prefix → `Utf8View`, with a diagnostic naming the reason.** Arrow lists are
  0-based and have no lower bound, so the index origin has no home; and no
  fixed Arrow list type is honest about a column where `{1,2}` and
  `{{1,2},{3,4}}` are both present.

The degradation is informed rather than fatal: it is decided before the schema
is fixed, not discovered at row 40 million, and the column round-trips exactly
as the string it already is today.

*Rejected for Phase 4, kept as a roadmap Future item:*
`Struct{dims, lbounds, elements}`, which is lossless for every array
PostgreSQL can produce. Two things decided it, and **only the first is
evidence — the second is a judgement that this project's flagship dataset is
not every dataset.** First, as a *fallback* it saves no scan: you still need
the census to know the column is mixed, so its "no census required" property is
realized only if it becomes the universal array representation. Second,
universal is what koji argues against — +16 bytes per row on every array
column (worst where arrays are shortest: +80% for a single-element array,
+400% for an empty one), and the loss of `List` as the signal every generic
Arrow consumer reads as "this is an array".

**A schema of matrices or scientific data inverts that arithmetic entirely**,
and the answer for those users is the struct, not a string. That is why it is
filed as a *knob* under the roadmap's "Future" section rather than discarded:
the choice is reversible in exactly one direction. Adding the struct later only
ever touches columns Phase 4 left as `Utf8View`, so it strictly widens
coverage; committing to it now and retreating later makes every array consumer
rewrite.

## Render-back must be exact, so the delimiter trap is removed rather than handled

`pgdq query` renders values back to COPY TEXT form, and Arrow's own display
formatting was already rejected for not round-tripping. That carries to nested
values unchanged: I20's `needquote` predicates are reimplemented exactly — the
case-insensitive `NULL` test, the whitespace test, both escape conventions —
and pinned by a round-trip test over every fixture value.

The trap that would otherwise break it: an array's separator is the *element
type's* `typdelim`, not always `,`. `box` is the only built-in that sets `;`,
and a user-defined base type can set one too (`pg_dump` does emit `DELIMITER =`
in its `CREATE TYPE` when it is not the default).

**So an array stays `Utf8View` when its element type — resolved through any
chain of domains — is `box`, `TypeKind::Base` or `TypeKind::Shell`; and the
separator stays hardcoded to `,`.** Such an element resolves to `Utf8View`
anyway, so `List<Utf8View>` would recover nothing a plain string does not — it
would only add a way to split on the wrong character.

**The refusal tests the element *after* domain unwrapping, not the declared
string**, because a domain inherits its base type's `typdelim` and its own DDL
records nothing about it (I22). `CREATE DOMAIN d AS box` makes `d[]` a
semicolon-separated literal that is named neither `box` nor `TypeKind::Base`;
resolution already unwraps domains transitively, so the fix is for that walk to
report its terminal rather than only its outcome. Unwrapping preserves
`OpaqueBaseType`, so a domain over a *user-defined* base type is already
refused — the built-in `box` is the only leak, because it is the only one
recognized by name.

I22 also bounds the problem: `box` is the only built-in with a non-default
delimiter in any supported version, and `record_out` writes `,` unconditionally,
so arrays are the entire exposure.

*Rejected:* refusing any array whose element does not resolve to a mapped Arrow
type. It closes the same hole, but pays for it by degrading `interval[]`,
`money[]` and every unknown-element array to a whole-column string, where
`List<Utf8View>` recovers real information — the element boundaries — and splits
on the right character.

*Rejected:* hardcoding `box` and parsing `DELIMITER` out of `CREATE TYPE` into
`TypeKind::Base`. It handles the trap instead of removing it, and buys a
`List<Utf8View>` over values that are opaque by construction.

## An array whose element type is itself an array is refused too

*Added 2026-08-26, after 4.4 shipped this shape broken; reasoning in
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).*

`CREATE DOMAIN d AS integer[]` followed by a column of type `d[]` is legal, and
`pg_dump` writes exactly that (I26). It is the **only** DDL shape that reaches
a nested `Array` plan in real dump output — `integer[][]` does not, because
PostgreSQL collapses it in the catalog and `pg_dump` writes `integer[]` (I21).

The value is written **one brace deep** — `{"{1,2}","{3}"}` — because
`array_out` force-quotes any element whose text contains `{` (I25). So a
literal's leading brace run and its column's resolved `List` depth are
independent for this shape alone, and `NestedPlan::Array(Array(…))` acquires a
second meaning: *one literal whose elements are themselves literals*, beside
the *one literal, two dimensions* meaning the census produces. That is the
collision `NestedPlan` exists to prevent, one level down.

**So an array whose element type — resolved through any chain of domains — is
itself an array stays `Utf8View`,** with its own `ColumnResolution`
(`NestedArrayElement`) beside `OpaqueElementType`. The precedent for the
refusal is exact: that one exists because the element's own literal grammar is
not what the outer split assumes, which is this case in a different disguise.

*Rejected:* reusing `OpaqueElementType` rather than adding a variant. The
mechanism matches but the label would lie — `integer[]` is not opaque, it is
perfectly well understood and we are declining to represent it. That is the
same distinction 4.4 drew when it split `OpaqueElementType` out of
`OpaqueBaseType`, and it is the one a reader acts on: opaque means *never*
improves, this means *yes, if anyone needs it*, since the split-plan option
above stays open and additive.

**The refusal composes into a composite for free.** A composite *field* of this
type fails today for the same reason (`(L,"{""{1,2}""}")` — confirmed against
real `pg_dump` output, `../status/history/2026-08-26.md`), and needs no
separate handling: `resolve_nested` maps any non-`Mapped` outcome to `Utf8View`
in that position, which is the rule "Type mapping" already states for every
leaf that does not map. Producing the outcome in `resolve_declared_type` is the
whole change.

**The test belongs beside `element_is_opaque`**, which already walks domains
transitively — a domain over a domain over an array reaches the same shape and
must be refused too.

The refusal also *removes* a subtlety rather than documenting one. With no
nested `Array` plan reachable, the census's transform has no shape to guard
against: `resolve::retype_from_census`'s "only a plan of exactly
`Array(non-array)`" test exists solely for this case and goes with it.

*Rejected:* splitting the plan variant — a dimensional `Array` beside an
element-is-a-literal one — and teaching `append_typed` to recurse into
`decode_array` per element, with `render_field` inverting it. It is the
complete answer and it is contained, but it buys `List<List<T>>` over a shape
almost nobody declares by putting a second meaning into the one type whose
whole purpose is keeping meanings apart, and it lands in `batch.rs`, the
already-tested core path. It stays available: adding it later only ever touches
columns this refusal leaves as `Utf8View`, so it strictly widens coverage.

## What the census stores, and when it may be believed

Per `CopyBlock`, per column: the **minimum and maximum** dimension count seen,
and whether any value carried an `[lb:ub]=` prefix. Per-block is what lets the
same splice that already extends the map write it incrementally, and combining
blocks is min-of-mins, max-of-maxes.

**A maximum alone is not enough**, which is the whole reason both are stored: a
column holding `{1,2}` and `{{1,2},{3,4}}` records `max = 2` and so does a
column holding only 2-D values. With max alone the mixed column resolves to
`List<List<T>>` and every 1-D value then fails the decoder's dimensionality
refusal — a confidently wrong schema, the outcome this section's
"believing a partial census" paragraph exists to rule out. Uniform is
`min == max`. Values that carry no dimensionality contribute to neither bound:
SQL NULL, and `{}`, which fits any depth (I20).

**The census is per column, so a nested array's shape is never learned.** An
array-typed *field* of a composite has per-value dimensionality exactly as
unfixed as a top-level array column — I21 is a fact about values, not about
positions — but the census has nowhere to record it. So an array inside a
composite, or inside another array's element type, **stays on the optimistic
path permanently**: a multi-dimensional or `[lb:ub]`-decorated value there is a
hard `FieldDecode` whether or not a census exists, and `--schema-mode strings`
is the only remedy. Only a top-level array column's error becomes unreachable.
Keying the census by path instead of by column is filed under the roadmap's
"Future"; it is purely additive whenever it lands.

**Every mapping pass censuses, so a mapped block always carries one.**
`build_index`, `build_map` and `stream::map_forward` all census, under either
`ScanExtent`. `scanned_through` advances only at a `CopyEnd` watermark or at
EOF, so a `CopyBlock` that reached the map was walked end to end — there is no
such thing as a half-censused block, and `array_shapes` is therefore a plain
`Vec<ArrayShape>`, never an `Option`. An empty vector means "censused, saw no
array-shaped literal", which is the same answer as a vector of unconstrained
`ArrayShape`s and needs no separate representation.

*Rejected:* censusing only under `ScanExtent::Full`, so a cold query declines
the per-row work. A cold query already receives every row of every block it
maps — the mapping pass calls `on_row` unconditionally and the queried block's
bytes are read twice regardless — so the saving is the pre-filter alone, which
is free on brace-free data. What it costs is a state no user can observe or
repair: a dump mapped by a cold query and *then* by a full one comes out
`is_complete` with its early blocks permanently uncensused, because
`map_forward` splices onto a prefix it does not re-read. Reasoning:
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).

**A stream's census is the union over the blocks it will replay, and needs no
completeness test.** `table_stream` finishes its mapping pass before it emits
anything and then collects `matches` — every block in the map bearing the
target's `(database, qualified name)` — so at the moment the schema commits,
the set of blocks whose rows the stream will hand back is fixed and every one
of them carries a census. Min-of-mins, max-of-maxes over that set is exactly
the evidence for exactly those rows. A cold query that stopped at its target
therefore retypes as confidently as a full scan: `stream::target_settled`
fires only once no further block can share the name (I2), and where it is
fooled — the undetectable concatenation in `STATUS.md`'s known gaps — the
unseen block's rows are not emitted either, so the schema stays true of the
stream's own output.

**`is_complete` qualifies a *reported* schema, not a streamed one.** `pgdq
info` answers "what is this table's schema" from an index alone, with no replay
to bound the claim, and a partial map genuinely cannot speak for blocks past
its frontier. That is where the test belongs, and it is the same partiality
`DumpIndex::roles`/`tablespaces` already carry, for the same reason.

*Rejected:* gating the streamed schema on `DumpIndex::is_complete` too. It
reads the file-level rule one scope too wide: a cold query can never satisfy
it, so every query against a large dump would keep the optimistic path and the
`FieldDecode` refusal this census exists to remove — while the evidence it
needed sat in the blocks it had just walked. Reasoning:
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).

## Nested elements copy; the zero-copy path is not widened here

Every nested value is built by copying, including a `List<Utf8View>` whose
elements would in principle be viewable — an array field with no COPY escapes
arrives as a borrow of the read chunk, and `"` is not in COPY TEXT's escape set
(I15), so `{"a,b","c d"}` is borrowed and each element's content is a
contiguous sub-slice.

Widening the view path into a recursive builder means honouring its three sharp
edges (chunk retention, block-index invalidation on flush, straddling fields)
at every level of `List` and `Struct` nesting. That is a scan-performance
change to an already delicate path, which Phase 7 owns and should measure
before it widens; filed to
[`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md). Copying also leaves this
phase's own measurement an honest baseline rather than a moving target.

## The literal form travels beside the Arrow type, from one producer

The mapping table above stops at the Arrow type, which does not determine the
value: `int4range[]` and `int4multirange` both resolve to
`List<Struct{lower, upper, …}>` and are written `{"[1,10)","[2,3)"}` and
`{[1,10),[2,3)}`. So a `pgtype::NestedPlan` — a `Scalar`/`Array`/`Record`/
`Range`/`Multirange` tree — is passed alongside the `DataType` into the builder
and the renderer, and is what picks a `nested.rs` codec at each level. The
mechanism and its rejected alternatives are in
[`architecture.md`](architecture.md), "Nested columns: `NestedPlan` travels
beside the `DataType`".

The type and the plan are two trees that must agree, and a disagreement is a
panic rather than a wrong value. Two rules keep them agreeing rather than a
third tree structure enforcing it:

- **One producer.** `resolve_declared_type` returns a `(DataType, NestedPlan)`
  pair; nothing else constructs either half of a nested column's pairing.
  `ResolvedSchema` carries the result as a third positional vector,
  `plans: Vec<NestedPlan>`, parallel to `schema.fields()` like `columns` and
  `notes` already are. *Rejected:* a field on `ColumnNote`, which is the
  human-facing per-column record and would become two things, since no display
  ever reads a plan; and a payload on `ColumnResolution::Mapped`, which
  expresses "a plan exists exactly when a column mapped" but breaks the three
  sites that compare the enum by equality, to buy a coupling one producer
  already gives.
- **Transform the pair, never a half.** 4.5's census retypes a column — to
  `List<List<T>>`, or down to `Utf8View` — by rewriting the pair, which is the
  only place after resolution that either half changes.

*Rejected:* a single tree owning both (`Scalar(DataType)` / `Array(Box<…>)` /
…, with a `data_type()` accessor), which makes disagreement unrepresentable.
The drift it defends against is narrow — one producer, one transform site, both
in this phase — and it costs a rewrite of `pgtype.rs`'s public surface plus a
`.data_type()` at every existing `DataType`-shaped call site. It is the fallback
if the pairing ever gains a third writer.

**`render_field` is deleted in 4.4 and `render_field_with_plan` takes its
name.** Two entry points where the plan-less one panics on a nested column
makes "did every caller switch?" a review question; one entry point makes it a
compile error. A scalar caller passes `&NestedPlan::Scalar`, which is its
`Default`.

## Fixtures and what `pgdq info` shows

`fixtures/*/types/default.sql` gains the shapes nothing currently exercises, in
that file rather than a new schema: the round-trip tests want every one of them
coming out of a real `pg_dump` in one dump, and a separate schema would
duplicate the generator plumbing to hold ten rows.

- an `[lb:ub]=` decorated value;
- **a column that mixes dimensionality across rows** — the census's reason for
  existing. It is an input to the census, not to the decoder, and is named so
  that nobody later "fixes" it;
- array-of-composite and composite-containing-array — I20's
  both-conventions-at-once case;
- a range over a text-ish subtype, so quoted bounds using the doubling
  convention appear at all;
- array-of-enum; and `mybase[]`, the opaque-element case this phase declines;
- **an array over a domain whose base is `box`** — the delimiter trap wearing a
  disguise (I22). It is the one fixture value whose *separator* is not `,`, and
  without it the refusal above has no test that distinguishes it from the
  `mybase[]` case it is easily confused with;
- a **zero-field composite** and its `()` value;
- **with 4.4.2**, `t_nested_array`: a column of `intarr[]` (an array over a
  domain over an array, I26) *and* a composite with a field of that type — the
  two faces of the same refusal. It is deliberately **not** a column of
  `t_array_shape`: that table is the census's fixture and is read as "these are
  the shapes the census reports", and this column's census entry is `(1, 1)`,
  correct but the one place where reading it as an ordinary 1-D array is wrong.
  The domain-over-domain chain earns **no** fixture column — the literal it
  produces is identical, so there is no `pg_dump` output shape left to predict;
  `pgtype.rs` pins the transitive walk as a unit test instead.

  **The same table also picks up the five shapes that already work and are
  pinned by nothing** — a domain over a composite, an array of that, a
  composite whose field is a composite, an array of a user range, and a domain
  over a range. All five were observed to round-trip during the sweep that
  found the defect (`../status/history/2026-08-26.md`), and none appears in
  any fixture. They ride along because the generator is already being run:
  "the recursion handles it" is precisely the reasoning that let `intarr[]`
  through, and a shape observed to work is not covered until a fixture holds
  it (`roadmap.md`, "Expand the generated fixtures freely").

The twelve-name table needs no new fixture: `t_multirange` already carries a
bare built-in (`int4multirange`) and `myrange`'s companion, with `{}` and NULL
rows, behind the schema's existing `\if :has_multirange` (PG14+) gate.

`pgdq info` prints a resolved nested type in compact lowercase —
`list<struct<a: int32, b: string>>` — not Arrow's `DataType` Debug, which is
unreadable in a column listing.

**The three ways a column can still be a string are told apart**, and only the
first is a success: a resolved nested type; a census-driven degradation
(varying dimensionality or a lower-bound prefix); an opaque element type. The
last two become **`ColumnResolution` variants** — `OpaqueElementType` in 4.4,
`VaryingArrayShape` in 4.5 — beside the existing `UnknownType`,
`OpaqueBaseType` and `NotDeclared`.

*Rejected:* `DiagnosticKind` variants. There is exactly one of these per
column and the ordinary case is success, which is what
[`architecture.md`](architecture.md), "Diagnostics: one severity scale, two
types" defines a *note* to be; every `DiagnosticKind` variant that exists is a
property of the file (tiling, cache, TOC coverage), so routing a type-semantics
conclusion there also puts an L2 fact in L1's channel. The visibility argument
for it does not hold either: `--json` exports `DumpIndex`, which carries no
`ResolvedSchema`, so a column-level fact is absent from it whichever type
holds the fact. Widening that export is a separate question this phase does not
answer, and nothing here changes what `DumpIndex` holds.

**`ColumnResolution::Deferred` and `pgtype::DeferredKind` are deleted in 4.4.**
These four families are the only things that defer, so after the flip nothing
produces one and the variant would read as a live option that no input can
reach.

The optimistic path emits **no diagnostic** — there is nothing to report, and
inventing one would imply a check that did not happen.

## Predicates are unchanged

A predicate on a nested column keeps comparing the COPY-unescaped field text —
the `array_out`/`record_out`/`range_out` literal — as a plain string, exactly
as before this phase. The column's Arrow type changing underneath is invisible
to a filter that never looked at it: `Predicate` reads the decoded field, which
is available before any nested decoding happens, and `PredicateOp` has no
ordering operator whose string-versus-element-wise semantics could diverge.

It also costs nothing: no per-row render, no new code.

**This agrees with PostgreSQL more often than a string comparison deserves to**,
because every value in a dump is already in canonical output form — discrete
ranges canonicalize on input, and array input whitespace is dropped. The
divergence is one class: a user supplying a non-canonical literal, where
PostgreSQL matches and the comparison silently does not.

*Rejected:* type-aware comparison here. It needs the *input*-side grammar,
which I20's scope limit flags as considerably more permissive than the
`*_out` inverse this phase commits to, plus canonicalization for the three
discrete built-in ranges. That is one-time work belonging with typed
predicates, and it is why Phase 4 precedes Phase 5 in the first place — filed,
with the measured PostgreSQL and DataFusion semantics, in
[`roadmap-phase5-inbox.md`](roadmap-phase5-inbox.md).

## The performance deliverable is a ratio, not a gate

`scripts/generate_perf_data.py` grows an array-heavy stress section, and the
phase records array-column decode throughput in
[`measurements.md`](measurements.md) **as a ratio against the same data as
plain `text` columns** — same file, same session, same cache state, per that
doc's standing rule.

**Three figures, not one** (amended 2026-08-27;
[`history/2026-08-27.md`](../status/history/2026-08-27.md), "4.6 owes three
figures"). The array decode ratio was the only one this section originally
named, and two more have a claim on the same run:

1. **Array decode throughput**, as a ratio against `text` columns in the same
   table. The figure this section was written for.
2. **The census cost on array-bearing rows.**
   [`measurements.md`](measurements.md)'s census section already promises this
   to 4.6 in writing — it measures the census as free on brace-free data and
   says the array-bearing case "is not measured here". It is a different code
   path from (1): the census runs in the *scan*, the decode ratio in the
   *query*. There is no other owner available, since Phase 7 is the consumer.
3. **Composite decode throughput**, the same ratio for a `Struct` column.
   The deferral this phase hands Phase 7 is that **nested** values always copy
   — which covers a composite's per-field copying exactly as much as an array's
   per-element copying. 4.6's stress data is the only thing that will carry a
   composite column, so leaving it out means Phase 7 decides half its question
   on evidence and half on inference.

The generator run and the container cycle are shared across all three, so (2)
and (3) cost two more columns and two more rows in a table.

**The stress columns.** `v_int_array` (`integer[]`, 3-5 elements),
`v_int_array_long` (`integer[]`, ~50 elements) and `v_comp` (a two-field
composite). Two array lengths rather than one because Phase 7's question is what
*per-element* copying costs, and one length cannot separate the per-element
slope from the per-value overhead. Same element type in both, so element count
is the only variable. All uniform 1-D and non-`NULL`, so they resolve to
`List`/`Struct` rather than degrading: the refused shapes are `Utf8View` and
measure nothing `v_text` does not.

*Rejected:* matching koji, whose six array columns are entirely `NULL`. That is
the realistic shape and it measures the null check rather than the decode,
which is not what Phase 7 asked for. The observation is recorded beside the
figure instead, so nobody reads the ratio as "what koji costs".

**It is a section in `generate_perf_data.py`, not a fourth bench script.**
The one-script-per-benchmark-shape precedent (`generate_insert_run_bench.py`,
`generate_large_object_bench.py`, `generate_block_count_bench.py`) is about
whole-*file* shapes; `generate_perf_data.py` is already a multi-column stress
table, where `v_long_text` and `v_escaped` sit beside `v_text` for exactly this
reason. And [`measurements.md`](measurements.md)'s standing rule requires the
ratio be taken on the same file in the same session — which a sibling column
gives for free and a separate file makes awkward.

**The section is gated behind `--arrays`, and that is not a convenience.**
`generate_perf_data.py`'s default output backs two existing figures that depend
on what it does *not* contain: [`measurements.md`](measurements.md)'s census
figure rests on the control holding **no `{` or `[` in any data row**, so every
row is rejected by the census pre-filter after one pass — deliberately the koji
shape — and the scan-throughput table is taken on the same bytes. Adding array
columns unconditionally would leave both figures without a command that
reproduces them, and that doc's own rule then says to delete them. With the
flag, the default output is byte-for-byte what it is today and 4.6's commands
carry one extra flag.

*Rejected:* a second table (`public.perf_nested`) always emitted in the same
file. It preserves the census pre-filter's brace-free table but not the
*file-level* scan figures, which would then cover both tables — it fixes half
the problem it is aimed at.

`--arrays` also emits `CREATE TYPE public.perf_comp AS (…)` into the preamble,
which is the first type definition this generator writes; it emits a bare
`CREATE TABLE` with no TOC comments today.

**Which instrument takes which figure.** Three exist and they answer different
questions, so two are used and one is not:

- **The decode ratio is a `decoders.rs` micro *and* an end-to-end `pgdq query`
  run.** The micro — one array literal against one text field, no I/O, no
  batching — is the direct answer to Phase 7's question, and is where every
  other per-type decode cost already lives. The end-to-end run is `pgdq query
  --schema-mode typed` against `--schema-mode strings` on the same file:
  `strings` resolves everything to `Utf8View`, so it is the control the ratio
  needs without a second file. The two are complements — the slope, and what
  the slope costs a user.
- **The census figure comes from `parse`**, as the existing census measurement
  does, since the census runs in the scan and not in the query.
- *Rejected:* a `whole_file.rs` criterion case. It is already wired to this
  generator, which is its whole appeal, but a whole-file scan dilutes the array
  cost across the other sixteen columns and then duplicates the end-to-end run
  more expensively.

## Where the code goes

The nested literal codec is a new **L2** module (`nested.rs`), recorded in
[`layering.md`](layering.md)'s table in the slice that creates it. It operates
on an already-COPY-unescaped field and its instantiations are chosen by
PostgreSQL type, which is L2's concern — "per-type field decode and
render-back" — not L1's.

*Rejected:* splitting the type-agnostic quoted-token scanner into L1 and
leaving only the three parameter sets in L2. The scanner has no meaning
independent of the types that instantiate it, and the split would put two
modules where the rule that motivates it ("a module that seems to belong to two
layers is two modules") is not actually triggered.

The cache format bumps. The census lives on `CopyBlock`, not in a
parallel map keyed by block — `DumpIndex`'s one-owner-per-fact rule, and the
block is what the census is a property of.

## What the manual must say

`docs/manual/type-handling.md`'s "Arrays, composites, ranges, and multiranges
are strings for now" is accurate until 4.4 and wrong the moment it lands, so
the manual is part of those slices rather than a follow-up. Rationale stays out
of it — these are the user-facing facts, and every one of them is something a
user hits without warning otherwise.

**With 4.4**, that section is replaced by:

- what each family becomes, with a literal and the value it produces;
- the three reasons a column of these types is still a string, told apart:
  its element type is opaque (`box[]`, a C base type), its arrays vary in
  shape (below), or `SchemaMode::Strings` / `--schema-mode strings` was asked
  for;
- that a multi-dimensional value or one carrying an `[lb:ub]=` prefix is an
  **error** on the default path — what the error names, and both remedies;
- that a **predicate on one of these columns still matches the literal text,
  not the value**. `tags = '{a,b}'` keeps working exactly as before, and a
  non-canonical spelling (`{a, b}`) matches nothing even though PostgreSQL
  would match it. This is the one behaviour a user would otherwise have to
  discover by getting zero rows back.

**With 4.5**, there is one path for a query and the manual states it as one
*(amended 2026-08-26; this section previously promised a two-path statement,
which the census reversal below made false —
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md))*:

- an array column's shape is read from the values while the file is mapped, on
  every query — there is no faster path that skips it and no slower path that
  improves it, so a user has nothing to choose between and nothing to check;
- what that produces: uniform depth becomes nested lists, and a column whose
  arrays genuinely vary — or that carries a lower bound — comes back as text
  instead;
- that an array **inside a composite** is not covered by that — it stays on the
  optimistic path however much scanning has happened, and `--schema-mode
  strings` is its remedy.

The two-path statement survives only for a **reported** schema over a partial
index, which no CLI surface can reach until Phase 9.2; it belongs in that
phase's manual work, not this one's.

**Also with 4.5**, one sentence — no more — that a column whose arrays vary in
shape is returned as text today, and that a selectable structured
representation for those columns is planned (`roadmap.md`, "Future"). A user
deciding whether to build a workaround needs to know it is a current limit
rather than a permanent one; they do not need the reasoning.

## Slices

Ordered so each one's mistakes are visible to the next, and sized by review
rather than scope.

| # | Slice |
|---|---|
| 4.1 | The fixture shapes above — generator plus regenerated fixtures. No library code |
| 4.1.1 | The two fixture values found after 4.1 landed — domain-over-`box` array, zero-field composite. Generator plus regenerated fixtures; must precede 4.4, which is where both are acted on |
| 4.2 | The nested literal codec: parameterized quoted-token scanner, three instantiations, decode **and** render, round-tripped against 4.1's literals. Pure L2, no Arrow |
| 4.3 | `ColumnBuilder`'s `List`/`Struct` arms, unit-tested by constructing nested values directly. Nothing resolves to them yet |
| 4.4 | Flip resolution: recursive mapping, built-in range subtypes, opaque-element refusal, the `ColumnResolution` surgery, the `(DataType, NestedPlan)` pair threaded through `ResolvedSchema` and `RowBatcher::new`, `render_field`'s deletion. Nested columns decode end-to-end on the optimistic path |
| 4.4.1 | The presentation half: the resolved Arrow type in `pgdq info --verbose`, **the manual rewrite above** |
| 4.4.2 | The array-of-array-typed-element refusal above, **earned**: 4.4 typed the shape `List<List<T>>` and nothing can fill it. Fixture first — `types/default.sql` gains `t_nested_array` across all six majors, since the shape's absence is why six majors of round-trip tests went green over a broken column — then the resolution refusal and its `ColumnResolution::NestedArrayElement`, a `pgtype.rs` unit test for the domain chain, the manual line, and the deletion of the census transform's now-unreachable guard. The table also picks up the five working-but-unpinned shapes the sweep found, and the slice lands the resolution-outcome coverage test that makes `roadmap.md`'s fixture rule mechanical |
| 4.4.3 | The array-declaration spellings, **earned**: 4.4.2 refused `integer[][]` as a nested array, which PostgreSQL does not agree it is. Fixture first, as its own commit — a new `t_array_spelling` table with one column per non-`[]` spelling (`integer[3]`, `integer[3][4]`, `integer ARRAY`, `integer ARRAY[4]`), whose dumped DDL must read `integer[]` on all six majors, which is the repo's own proof of I28's collapse. Its own table rather than `t_array_shape`, whose contract is "the shapes the census reports" and which these columns have no opinion about. Then, as a second commit, one `pgtype.rs` helper normalizing the whole `Typename` array-bounds production (any number of `[]`/`[n]` pairs, no cap; `ARRAY`/`ARRAY[n]`, case-insensitively; well-formed bounds only) to the element type plus one array level, applied at `resolve_declared_type`'s entry and at the domain-walk terminal test (4.4.4 settles which function holds the second call site; the placement was never the decision). The I26 refusal is untouched. Pinned by a unit table over the six spellings, citing I28, and one hand-built dump exercising the resolution→census→builder seam no generated fixture can reach |
| 4.4.4 | The array arm folded into one function, **earned** from 4.4.3's unattended call: `element_is_opaque` and `element_is_array` each walk the domain chain separately and their safety is an ordering argument spread across two doc comments, which is what made "where does normalization live" a question worth flagging. Replace both with `resolve_array(element, types) -> TypeOutcome` holding the whole array decision — both refusals, the `resolve_nested` recursion, `list_of` and `NestedPlan::Array` — over **one** domain walk, opaque-tested first so I22's precedence stays visible. `resolve_declared_type`'s array arm becomes a single delegation. **Behaviour-preserving**: every existing test passes unchanged, no test may be edited to accommodate it, and **no new behavioural test is wanted** — the `pgtype.rs` suite and the resolution-outcome coverage test already pin every arm, and a refactor with no delta has nothing new to assert. What it owes instead is the standing rule's documentary check, in the notes doc: which later phases the shape is expected to survive, and what it made simpler. **Out of scope: the quoted type-name gap (I29)**, which the corrected doc comment will now cite. It is a roadmap "Future" item, fixing it changes behaviour, and it is not this slice. Also corrects `array_element`'s "Not quote-aware" paragraph, which documents a limitation the code does not have (I29). Taken under `roadmap.md`, "Refactor when the shape stops fitting" |
| 4.5 | The shape census, **recording half**: a cache format bump, per-block per-column recording, and the completeness rule. Nothing consumes it yet |
| 4.5.1 | The shape census, **consuming half**: making the census unconditional (above), retyping the `(DataType, NestedPlan)` pair from a block's census, `ColumnResolution::VaryingArrayShape`, and **the manual's statement of both paths plus the planned representation knob** |
| 4.6 | The array stress section in `generate_perf_data.py`, **gated behind `--arrays`** so the default output stays the brace-free control two existing figures depend on — `v_int_array`, `v_int_array_long`, `v_comp`, plus the `CREATE TYPE` the composite needs. Then **three** `measurements.md` figures: array decode throughput as a ratio against `text` (a `decoders.rs` micro **and** `pgdq query --schema-mode typed` against `strings`), the census cost on array-bearing rows (from `parse`), and composite decode throughput. Scope amended 2026-08-27 from the single decode ratio; see "The performance deliverable is a ratio, not a gate" |

**4.2 keeps decode and render together deliberately** — they are inverses, and
landing decode alone leaves it with no oracle.

**What 4.4.1 renders, precisely.** `pgdq info` prints each column's *declared
PostgreSQL* type and, under `--verbose`, a `resolution_label` for columns that
did not map. After the flip that leaves one thing unanswerable: a column that
mapped does not say what it mapped *to*, so `payload text[]` reads the same
whether it became `List<Utf8View>` or stayed a string. 4.4.1 adds the resolved
Arrow type to the `--verbose` per-column line **for every column whose type is
not `Utf8View`** — `Utf8View` being the no-information answer, and already
explained by the resolution label wherever it is a refusal. So `--verbose`
becomes a complete statement of the Arrow schema rather than a nested-column
annex, and it picks up the scalar mappings that were never visible either
(`numeric` → `Decimal128`, `timestamp` → `Timestamp(Microsecond)`).

*Rejected:* printing only where the column's `NestedPlan` is not `Scalar`. It
keeps every existing line's width untouched, which is its whole appeal, but
"what does this column become in Arrow" is not a question only a nested schema
raises — a `numeric` column's `Decimal128` precision is exactly as invisible
and exactly as consequential to a caller building against the schema.

**Rendering is arrow's own `Display`, with one substitution.** `arrow-schema`'s
`Display` is terse and reversible (`List(Utf8View)`, `Struct("x": Int32, "y":
Utf8View)`), and a composite's field names are the user's own, so it carries
real information. The five-field range struct does not: it is identical for
every range column in every dump, and renders as 137 characters saying so. It
collapses to `Range<T>`, `T` being the bound type — the only part that varies —
so an `int4range` column reads `Range<Int32>` and `int4range[]` reads
`List(Range<Int32>)`. The manual states the struct's real layout once, which is
what makes the elision lossless; that sentence is a 4.4.1 deliverable, not the
census slice's.

Detecting the substitution is the `NestedPlan`'s job, never the field names' —
a user composite is free to declare five fields with exactly those names, and
`RANGE_STRUCT_FIELDS`'s own doc comment already reserves dispatch to the plan.
A built-in multirange and an array of the matching range render *identically*
(`List(Range<Int32>)`), which is correct rather than a collision to fix: they
are the same Arrow type, the plans differ, and the declared PostgreSQL type
sits on the same line.

**4.4.2 was earned after 4.4 landed, from a defect.** 4.4's contract was
"nested columns decode end-to-end on the optimistic path", and for one declared
shape it does not — the type resolves and no value can fill it. That is a slice
that shipped the wrong contract, which the numbering rule answers with a third
level rather than a renumber. It lands **before 4.6**: 4.6 measures the array
path, and measuring it while a declared array shape is known-broken measures
something about to change.

**4.4.3 was earned from 4.4.2, the same way 4.4.2 was earned from 4.4.** 4.4.2's
contract was "an array whose element type is itself an array is refused"; it
implemented "an array whose element type is *spelled* with `[]` is refused",
and `integer[][]` is not the former — PostgreSQL collapses every array-bounds
spelling to one array level and discards the rest (I21, I28), so that column's
element type is `integer`. The refusal therefore states something false about
the column rather than declining to answer, which is the defect. It lands
**before 4.6** for 4.4.2's own reason: 4.6 measures the array path, and the
phase-wrap koji run validates it.

Two things it deliberately does not do. It does not revisit the I26 shape —
`d[]` over `CREATE DOMAIN d AS integer[]` really is an array of arrays, really
is written one brace deep, and stays refused with `NestedPlan::Array` still
meaning one thing. And it does not narrow to the one reported spelling: five of
the six the server accepts fail to resolve today (`integer[3]`, `integer[3][4]`,
`integer ARRAY` and `integer ARRAY[4]` as `Unknown`, `integer[][]` as the false
refusal), and under `roadmap.md`'s input-contract rule they are one defect, at
one code site, not five.

**4.5.1 was earned mid-slice, not planned.** The census was specified as one
row and is two: recording is new, type-blind, L1-only machinery that nothing
reads, while consuming it retypes an already-tested resolution path and
rewrites the test that pins the optimistic path's refusal. Bundling them would
force one review to accept both at one confidence — the same seam 4.3/4.4 was
split at, and the spec's own reason for that split applies here unchanged. The
reasoning is in `docs/status/history/2026-08-26.md`.

**The census's unconditional rule rides in 4.5.1 rather than earning a
`4.5.2`.** It reverses a call 4.5 made, which is the shape that normally earns
its own increment — but what it reverses *is* 4.5.1's consumption test,
collapsing it from two operands to one. Splitting them would put the same
question in two reviews and force the second to be written against a rule the
first had just deleted.

**4.4.1 is a third level made at grilling rather than earned mid-slice**, which
is off-label and deliberate: the seam was visible here, and the alternative —
renumbering the census to 4.6 and the measurement to 4.7 — invalidates every
reference to those numbers and erases the record that a split happened at all.
The seam is review confidence. Everything in 4.4 is one rework of
`pgtype.rs`/`resolve.rs`/`batch.rs` judged against "does a nested column decode
end-to-end"; the presentation half can only be written once that half has
determined what there is to render, and it is judged against a human reading
`pgdq info` and the manual. Bundling them forces one review to accept both at
one confidence.

**4.3 before 4.4 is the load-bearing ordering.** Resolution returning `List<T>`
while `ColumnBuilder` has no `List` arm is a broken intermediate: the schema
promises a type nothing can fill. Combining the two avoids that but pairs a new
mapping with a rework of `batch.rs`, the already-tested core path — the exact
pairing that forces a review to accept both halves at one confidence. Landing
the builders first as tested, unreached code costs nothing and dissolves the
choice.

*Rejected:* slicing by family — arrays end-to-end, then composites, then
ranges. The families have no independent implementations to stage; they are
three parameter sets over one scanner, so this would land that scanner and then
edit it twice more, re-reviewing the same code each time. The census is the one
piece that genuinely belongs to arrays alone, and it is already its own slice.

## Verification

- Every fixture value round-trips byte-for-byte through decode → render — the
  I20 conformance test, and the reason 4.2 is one slice.
- `every_fixture_tiles_exactly` still passes. Nothing here touches the map.
- The mixed-dimension fixture column resolves to `Utf8View` **with** a census
  and raises `FieldDecode` **without** one, pinning both paths — and it is the
  test that fails if the census stores only a maximum.
- The domain-over-`box` array column resolves to `Utf8View`, distinguishably
  from `mybase[]`: the refusal must survive the domain indirection, not just
  the two spellings it recognizes directly.
- A built-in multirange column and an `int4range[]` column resolve to the
  *same* Arrow type and different plans, and each renders back to its own
  literal — the twelve-name distinction, asserted where it would otherwise be
  invisible.
- The koji scan still reports identical block, row and byte counts — **run
  once at phase wrap, not per slice**. Six identical count checks are one
  check, and koji holds no non-scalar values at all, so its value here is
  precisely that it exercises nothing this phase builds: an identity failure
  means a slice touched the scanner by accident. Detached, per `CLAUDE.md`.
- **A column resolving to a nested type never silently changes what a predicate
  matched** — the existing predicate tests run against the fixture's array
  columns both before and after 4.4 flips their Arrow type. This is the one
  place the phase could regress behaviour invisibly, and it is cheap to nail
  down.

No performance number gates the phase; see above.
