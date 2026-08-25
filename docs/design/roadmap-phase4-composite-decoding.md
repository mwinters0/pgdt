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
| `T[]` | `List<resolve(T)>` |
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

**The range struct's fifth field is not redundant.** `empty` and `(,)` are
different ranges and both have absent bounds, so inclusivity flags alone cannot
tell them apart. A range bound can never be SQL NULL (I20), which is what makes
a null `lower`/`upper` mean "unbounded" without ambiguity.

**The six built-in ranges need their subtypes hardcoded.**
`TypeDef::Range::subtype` is populated only for a user-defined range, because
PostgreSQL keeps a built-in's subtype in the catalog rather than in DDL text.
So `int4range`→`integer`, `int8range`→`bigint`, `numrange`→`numeric`,
`tsrange`→`timestamp without time zone`, `tstzrange`→`timestamp with time
zone`, `daterange`→`date`, alongside the bare-name recognition `map_builtin`
already does for them. `numrange`'s bounds land on `Utf8View`, since bare
`numeric` does.

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

### The shape census is an upgrade, not a gate

A full data scan buys exact dimensionality across every `COPY` block, and the
project spends it as an **option the caller already owns**, not a precondition.

- **No census** — an ordinary cold query. The column is `List<T>`, optimistic
  1-D, and a value that disagrees is the hard `FieldDecode` above.
- **Census present** — because `pgdq parse` ran, or the query ran
  `ScanExtent::Full`. Every array column's shape is known before the schema is
  fixed, and the error becomes unreachable.

This is the same shape as `Error::MetadataNotScanned`: two paths, the cheap one
default, the exact one reached by a command the user already has a reason to
run. **Both paths are the user's to choose, so both belong in
`docs/manual/type-handling.md`** — the optimistic path's failure mode and its
remedy stated together, not left to be discovered.

*Rejected:* gating — erroring up front with `ShapeNotScanned` unless a census
exists. koji's array columns are certainly 1-D (they are entirely NULL), so
gating would fail a cold query and demand an hour of `pgdq parse` to learn
nothing. *Rejected:* a transparent query-time prepass over the target blocks.
It needs no user action and silently doubles the read of every cold array
query, which is the cost this project is least willing to hide.

**Only arrays need the census.** A composite's shape is fixed by its `CREATE
TYPE` and a range's is fixed outright, so the scan is spent entirely on array
dimensionality and lower bounds. It cannot avoid per-row work — reaching field
*N* means splitting the row — so censusing a block then streaming it costs
about twice streaming it, which is why the result is persisted in the cache
beside `DumpIndex` (a `format_version` bump) and paid once per file.

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

**So an array whose element type is `box`, or any `TypeKind::Base`, stays
`Utf8View`, and the separator stays hardcoded to `,`.** Such an element resolves
to `Utf8View` anyway, so `List<Utf8View>` would recover nothing a plain string
does not — it would only add a way to split on the wrong character.

*Rejected:* hardcoding `box` and parsing `DELIMITER` out of `CREATE TYPE` into
`TypeKind::Base`. It handles the trap instead of removing it, and buys a
`List<Utf8View>` over values that are opaque by construction.

## What the census stores, and when it may be believed

Per `CopyBlock`, per column: the maximum dimension count seen, and whether any
value carried an `[lb:ub]=` prefix. Per-block is what lets the same splice that
already extends the map write it incrementally.

**A table's census may be consumed only once `scanned_through` has reached the
file's size** — the test `pgdq info` already performs against the live source.
Anything less falls back to the optimistic path. The reason is I2: one table's
data can occupy several blocks, so a table's census can be partial even when
every block that *was* scanned is completely censused, and a query that stops
at its target never learns what the blocks past it hold. This is the same
partiality `DumpIndex::roles`/`tablespaces` already carry, for the same reason.

Believing a partial census would produce a *confidently wrong* schema — a
column typed `List<T>` because the one block that was walked happened to be
uniform. That is strictly worse than the optimistic path, which reaches the
same schema while still treating a disagreeing value as an error.

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
- array-of-enum; and `mybase[]`, the opaque-element case this phase declines.

`pgdq info` prints a resolved nested type in compact lowercase —
`list<struct<a: int32, b: string>>` — not Arrow's `DataType` Debug, which is
unreadable in a column listing.

**The three ways a column can still be a string are told apart**, and only the
first is a success: a resolved nested type; a census-driven degradation
(varying dimensionality or a lower-bound prefix); an opaque element type. The
last two get their own `DiagnosticKind` variants so they are distinguishable in
`--json` and not only in prose. The optimistic path emits **no diagnostic** —
there is nothing to report, and inventing one would imply a check that did not
happen.

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

No number gates the phase. Array decoding is per-element CPU work, so an
array-heavy column will not stay device-bound; a gate demanding it would be one
the phase cannot pass, and a looser one would never fire. The `INSERT`-run
measurement is the precedent for why the ratio is the useful artifact: it is
what tells a later reader, and Phase 7, what the per-element cost actually is.

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

The cache format goes **v9 → v10**. The census lives on `CopyBlock`, not in a
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

**With 4.5**, the two paths are stated together in one place:

- the default is optimistic — fast, no extra reading, and it can fail on an
  array shape it did not expect;
- after `pgdq parse` (or a full scan), the shapes are known: the failure
  becomes impossible, and a column whose arrays genuinely vary comes back as
  text instead;
- how to tell which path a given run is on.

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
| 4.2 | The nested literal codec: parameterized quoted-token scanner, three instantiations, decode **and** render, round-tripped against 4.1's literals. Pure L2, no Arrow |
| 4.3 | `ColumnBuilder`'s `List`/`Struct` arms, unit-tested by constructing nested values directly. Nothing resolves to them yet |
| 4.4 | Flip resolution: recursive mapping, built-in range subtypes, opaque-element refusal, the new diagnostics, compact `info` rendering, **the manual rewrite above**. Nested columns decode end-to-end on the optimistic path |
| 4.5 | The shape census: cache v10, per-block per-column recording, the completeness rule, and **the manual's statement of both paths plus the planned representation knob** |
| 4.6 | The array stress section in `generate_perf_data.py`, and the `measurements.md` ratio |

**4.2 keeps decode and render together deliberately** — they are inverses, and
landing decode alone leaves it with no oracle.

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
  and raises `FieldDecode` **without** one, pinning both paths.
- The koji scan still reports identical block, row and byte counts.
- **A column resolving to a nested type never silently changes what a predicate
  matched** — the existing predicate tests run against the fixture's array
  columns both before and after 4.4 flips their Arrow type. This is the one
  place the phase could regress behaviour invisibly, and it is cheap to nail
  down.

No performance number gates the phase; see above.
