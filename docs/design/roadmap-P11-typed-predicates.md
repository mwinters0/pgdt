# P11 — Typed predicates

What a filter *means*. Two things the pushdown phase deferred, plus the
per-type worklist its ordering register left behind:

- **Full boolean structure** — `OR` and `NOT` over the conjunction that ships
  today, which is a real three-valued evaluator rather than two more operators.
- **Type-aware comparison on nested columns** — array, composite, range and
  multirange, which compare as text today.
- **`KD7`** — the four ordering-register rows that do not agree with
  PostgreSQL.

The mechanisms this phase reworks are described in
[`architecture.md`](architecture.md), "Predicates", "Ordering operators compare
typed, and the register says where that differs", and "A filter term is parsed
for two audiences". Read those first: this doc states what changes, not how the
current thing works.

## What this phase closes, and what it declares

**`KD7` is narrowed by this phase to one statement, not retired.** Its rows
close three different ways, and the parts that close by *statement* rather than
by code are not the weaker outcome: there the register was asking a question
the file has no answer to. One of them turned out to be asking a question the
file *does* answer, in part — see the text row — and that part is what survives:
a column that *states* a collation other than `C`/`POSIX` is ordered bytewise,
which the file gives enough information to close and this build does not. It is
`(c) unowned`, and the collation work that would close it is a Future item
rather than a phase.

*This doc originally bound `KD7` to be retired here.* It was written before
11.11 gave a stated non-`C` clause its own verdict, so the text row below spoke
only of the no-clause residue and the stated case had nowhere to be counted.
Amended rather than delivered as written, because striking the entry would have
been the "property filed as a deficiency" rule run backwards. Reasoning:
[`../status/history/2026-09-01.md`](../status/history/2026-09-01.md), "`KD7`
survives at one statement rather than being struck".

| Register row | Disposition |
|---|---|
| `Dictionary(Int32, Utf8)` from an enum | **Closed by code.** The dump carries `CREATE TYPE … AS ENUM (…)` verbatim and `TypeKind::Enum { labels }` already holds the labels in declaration order. |
| `Utf8View` from bare `numeric` | **Closed by code.** An arbitrary-precision decimal comparison, carrying all three of `Infinity`, `-Infinity` and `NaN`. |
| `Utf8View` from `interval`, `time with time zone`, `inet`/`cidr`/`macaddr`/`macaddr8`, and domains over them | **Closed by code**, one comparison per type. |
| `Utf8View` from `jsonb` | **Closed by code, with a residue that closes by statement.** `compareJsonbContainers` is structural — type rank, then length, then member-wise — and its *scalar string* leaves, keys included, go through `varstr_cmp` under `DEFAULT_COLLATION_OID`. That is the database's collation, which a plain dump does not record (I32), so a `jsonb` column reaches exactly the residue the text row reaches, one level down. The structural half is worth closing and the collation half is the same statement made twice. |
| `Utf8View` from `text`/`varchar`/`name` | **Closed by code *and* statement.** The file states more than the row assumed: `pg_dump` emits a `COLLATE` clause wherever a column's collation differs from **its type's** default, and `name`'s type default is `C` — so a bare `name` column, and any column carrying an explicit `COLLATE "C"`/`"POSIX"`, **agree exactly, on every server**. What stays open splits in two, and only the first closes by statement: a `default`-collation column with no clause is on the database default, which no plain dump records (I32); a column that *states* a collation other than `C`/`POSIX` carries the fact in the file, so bytewise is a wrong answer to a question the file asked — that is the one statement `KD7` survives at. Bytewise remains the answer throughout; what changes is which columns are told they diverge. |
| `Utf8View` from `char(n)` | **Closed by code, down to the collation statement.** A `character(n)` value is written blank-padded to `n` and `bpcharcmp` strips trailing blanks from *both* sides before it consults a collation at all (I38), so trimming both sides is the comparison — after which `char(n)` is the text row exactly, with the same three collation arms and the same residue. An explicit `COLLATE "C"` therefore does close it, but only once the trim is there, which is why 11.11 could not promote it and 11.6 can. |
| `Utf8View` from `json` (and `xml`) | **Closed by statement.** PostgreSQL defines *no* comparison for these types at all — no `=`, no ordering, no default operator class — so "agrees with PostgreSQL" is not a question they can be asked. Our text comparison offers more than the server does, and that is what gets said. |

The `json`/`xml` row is not a deficiency once stated: `STATUS.md`'s rule is
that a limitation whose remedy the user already has today is a property of the
mechanism, and that is the strongest answer the file supports. So is the
residue of the text row — a database default that is not in the file — once the
part that *is* in the file has been read.

**The text row was not merely weak, it was wrong**, and on every server rather
than only under an unusual collation: `'A'::name < 'a'::name` is true where
`'A'::text < 'a'::text` is false, because `pg_type.typcollation` is `C` for
`name` and `default` for `text`. A blanket "text diverges" told a `name`
column's user to distrust an answer that was exact. That is the shape of error
this phase exists to remove — a statement standing where the file has
something specific to say.

**It was also wrong in the other direction, about `char(n)`**, and that half
was found while 11.11 was being built: this doc originally put `char` with
`text` and `varchar` and promised that an explicit `COLLATE "C"` closed all
three. It does not close `char`, because `char`'s divergence is not a
collation at all — see the row above, and
[`../status/history/2026-08-31.md`](../status/history/2026-08-31.md) for the
evidence. Promoting it would have been the error this row exists to remove,
pointing the other way: a claim of agreement where the file supports none.

*Rejected: closing only the two rows that change the register's shape, and
leaving the rest of `KD7` open.* The remainder would be an unowned queue
sitting behind an owned phase, which reads exactly like an oversight and is
the state the register exists to prevent. Once the register is re-keyed off the
declared type — which the enum and bare-`numeric` rows force — each remaining
type is one comparison function and one register row.

*Rejected: splitting boolean structure into its own phase and leaving nested
comparison and `KD7` for later.* The three bodies share one surface and one
evaluator; separating them means designing the expression model twice, once
without knowing what its leaves can do.

### The text row's closure, and the libc its evidence is taken under

**PostgreSQL promises to apply what the platform provides, not that a collation
name means the same order everywhere.** `en_US.utf8` on a musl server is
`strcmp`, so `'A' < 'a'` answers `t`; on glibc 2.41 it is glibc's collation and
the same comparison answers `f`. The server tracks this itself —
`pg_collation.collversion` reads `2.41` for a libc-provider collation, which is
the libc version verbatim — and claims no more than that. pgdq claims no more
either: it compares bytewise, registers the divergence, and says which libc its
evidence was taken under. What stays closed by *statement* is the residue — a
`default`-collation column carrying no `COLLATE` clause, whose collation is the
database's and so absent from the file (I32). Where the file states a column's
collation, 11.11 reads it.

**The evidence is glibc's, by choice.** The fixture family moves from the
Alpine images to the Debian ones, so every text answer in the oracle is a glibc
answer — which is what nearly every deployment runs. **"Only glibc collation"
is then a stated limitation of the evidence**, not a claim about servers: a
musl deployment orders text differently and the oracle does not speak for it.
That is the honest shape, because the alternative is an apparatus whose text
answers were C-collation answers by accident of the base image, with nothing
saying so.

**The agreement half becomes a case-table dimension rather than an accident.**
Under musl the oracle's text cells *were* the `C` answers — the case where pgdq
**agrees**, and the half a naive family switch would silently trade away. It is
recovered by asking each text pair twice, under `COLLATE "C"` and under the
database collation, so one file shows both halves: bytewise equals PostgreSQL
under `C`, and does not under `en_US.utf8`. Neither half then depends on which
image happened to be used.

**Equality comes along for free, and it is the more interesting half.** A case
is already asked for all six of `OPERATORS`, so a collation-qualified case
carries `=` and `<>` without anything being added. Every libc collation is
*deterministic*, so `texteq` stays bytewise and equality **agrees** under both
collations where ordering diverges under one — exactly the split 11.6 inherits,
now visible in the file rather than argued from the source. **The
non-deterministic case does not close by statement**, and this doc originally
said it did: it needs an ICU collation declared `deterministic = false`, and a
plain dump *does* record that — `dumpCollation` writes `, deterministic = false`
into the `CREATE COLLATION` unconditionally for a user-defined collation. So the
file can state that `=` on such a column is not `texteq`, and this build answers
bytewise with no note at all. 11.6.1 designs against that rather than declaring
it away.

**The cases, measured on glibc 2.41 before being written down.** Four reasons
diverge — case ordering (`A` vs `a`), letter before case (`a` vs `B`), an accent
sorting with `e` rather than after `z` (`é` vs `f`), and punctuation ignored at
the primary level (`_x` vs `ax`) — and four pairs agree (`de luge`/`deluge`,
`e`/`é`, `1`/`a`, `co-op`/`coop`). **The agreeing pairs are kept
deliberately**: a set in which every row diverges reads as "these two orders
never coincide", which is false and is the wrong intuition to leave behind.

**A second libc would make the differ's apparatus guard load-bearing, so it is
fixed first.** `oracle_differences.py` requires nine `meta.tsv` keys to be
identical across majors and deliberately excludes `version`, which is expected
to differ — but `version` is the only key carrying the **platform triple**
(`on x86_64-pc-linux-musl`). A major left on, or reverted to, a different base
would pass the guard in silence while `datcollate` still read `en_US.utf8` and
meant something else. The triple becomes a guarded key of its own, parsed out
of `version` so the version number may still differ, and the default
collation's `collversion` joins it — that being the server's own notion of
"this collation may have changed underneath you".

**The apparatus, fixed: `-trixie` at every pinned minor.** One suite for all
six, never the unsuffixed tag — that pin is what keeps libc drift out of the
cross-major differences, and the unsuffixed `postgres:16` has already moved
suites once. Verified at both ends of the range: `13.23-trixie` and
`18.6-trixie` are both glibc 2.41 with `collversion` `2.41`, `datcollate`
`en_US.utf8` and `datlocprovider` `c` where that column exists, and both answer
`'A' < 'a'` false and `'a' < 'B'` true. The two images report Debian revisions
`2.41-12` and `2.41-12+deb13u3`, so uniformity holds at the granularity
PostgreSQL itself tracks — which is also the granularity the guard can check.

**The collation is a fourth field on the case, not a column on the row.** A
text pair appears as two cases — `(text, 'A', 'a', C)` and
`(text, 'A', 'a', en_US.utf8)` — rather than one row carrying `lt_c`/`lt_db`
columns that are empty for the 1200 non-text rows. The committed files mean
what they mean only by matching the case table in order, and their test walks
`(type, left, right)` row for row; a fourth field extends that check unchanged,
where extra columns add a second shape to learn. It also keeps the differ
honest: a collation-qualified case is just a case, so comparing it across
majors needs no special rule. The reason a pair was chosen stays a comment in
the case table — the file records answers, and a rationale is not an answer.

**The regeneration is diffed, not just re-tested.** Every fixture is
regenerated on a different base image, so the slice regenerates into a scratch
tree and diffs it against the committed one before accepting anything. The
expectation is that the dumps come back **byte-identical** — same `pg_dump`
minor, same schemas, and `pg_dump`'s TOC sort is `strcmp` rather than
locale-collated — but that is a prediction, and the fixture tree is what every
snapshot test reads. Whichever way it comes out is a fact worth one line on the
page: byte-identity says the switch is confined to the oracle's answers, which
is the claim the whole slice rests on, and a difference is something to read
deliberately rather than meet inside a snapshot review.

**What the switch touches, so none of it is missed.**
`generate_fixtures.py`'s `FIXTURE_IMAGES` and the comment above it;
`comparison_oracle.py`'s docstring, which states the musl limitation;
`architecture.md`'s "The text answers are bytewise, and that is an artifact of
the apparatus"; `I35`'s paragraph in `postgres-invariants.md`, which says the
text answers are musl's; `pg-dump-compatibility.md`'s `postgres:*-alpine`
mention in the `SECURITY LABEL` row, where the claim survives and only the
image name moves; and 11.1's notes, whose musl paragraph becomes false rather
than merely dated. **Not** `postgres-invariants.md`'s other
`postgres:16-alpine` probe citations (I15, I17, I22): they record where a
libc-independent observation was made, and re-taking them would change nothing
but the sentence.

**The manual gains its first sentence about collation**, in the same slice. A
`grep` over `docs/manual/` finds none today, though text ordering shipped in P5
and `pgdq query --filter 'name<x'` already answers in an order a user's server
may not share — warned only by a line on stderr. It lands here rather than in
11.6 because this is where the evidence exists, so the sentence can be
concrete: `A` sorts before `a` here and after it on a `en_US.utf8` server,
while `=` is unaffected. It closes a gap that exists today rather than one the
phase creates.

*Rejected: keeping the Alpine family and taking the glibc answers from a
separate one-major witness file.* It is the smaller diff, and it was the plan
until the question was put the right way round: what should the project's
evidence *be* taken under? A witness bolted onto a musl apparatus answers "musl,
plus a footnote", and every later reader of the oracle has to remember the
footnote. Moving the family answers "glibc", once, in the place the answers come
from. The cost is real and accepted — every generator edited, every fixture
regenerated, every correctness test re-run — and it buys an apparatus that
needs no footnote. No performance figure is affected: none declares
`fixtures/` or `generate_fixtures.py`, since the measured inputs come from
`generate_perf_data.py` and `generate_block_count_bench.py`.

*Rejected: an ICU collation as the second one.* It was free — the Alpine images
are built `--with-icu`, hold 870–908 ICU collations, and
`('a' COLLATE "und-x-icu") < 'B'` already answered `true` where bytewise
answered `false`, so no image change would have been needed at all. But the
evidence should show what deployments actually run, and that is `C` and the
UTF-8 locale. ICU's identity also floats with the base image — `und-x-icu`'s
`collversion` is `153.128` on `13.23-alpine` against `153.136` on
`18.6-alpine` — so an ICU case would need a guard the libc case does not. That
argument is about an *oracle case*, whose cells are the server's answers, and it
does not carry to a fixture column whose committed bytes hold no version; 11.12
is where that distinction is spent.

## The predicate model becomes an expression tree

`QueryOptions::filters: Vec<Predicate>` is **replaced**, not supplemented:

```
Expr = Term(Predicate) | And(Vec<Expr>) | Or(Vec<Expr>) | Not(Box<Expr>)
```

`And` and `Or` are n-ary. The list that ships today is `And(terms)`, which is
what the CLI's repeated `--filter` keeps building, so nothing about the common
shape changes. Pre-1.0 carries no compatibility obligation
([`roadmap.md`](roadmap.md), "Pre-1.0"), so the old field goes rather than
staying beside the new one.

*Rejected: keeping `filters` and adding an expression field beside it.* Two
ways to say one thing, with a rule needed for how they combine.

*Rejected: normalizing to disjunctive normal form at parse time and evaluating
a flat list of conjunctions.* That is a planner, and
[`architecture.md`](architecture.md), "Predicates" states plainly that there is
no simplifier and no plan. DNF also multiplies term count, and each term walks
the row separately.

*Rejected: binary `And`/`Or`.* The CLI's repeated `--filter` is n-ary by
construction, and binary nesting would make the ordinary case a right-leaning
chain that every reader has to flatten mentally.

### The evaluator is three-valued

An evaluation yields `True`, `False` or `Unknown`, and **a row survives only if
the root is `True`**. Every operator that exists today is restated in those
terms — `Eq` against a NULL field is `Unknown`, not `False` — which is
*observably identical* for every query expressible today, since a conjunction
containing an unknown still fails at the root. It is a restatement, not a
behaviour change, and that is what makes it safe to land under a phase whose
worst bug is a silently wrong row set.

`IsNull` and `IsNotNull` stay two-valued by definition: they are the operators
that exist because unknown collapses everywhere else.

## The oracle is generated, committed, and version-swept

**"Agrees with PostgreSQL" becomes a check rather than an assertion.**
`scripts/generate_fixtures.py` already spins a throwaway memory-limited
container per major (13–18); this phase adds a pass that runs a table of
`(declared type, left literal, right literal, operator)` through the server and
commits the answers as a fixture. The test suite then diffs this evaluator
against that file with **no live dependency**, and regenerating re-verifies
across every major.

This is the phase's **first** slice. The two traps the inbox carries —
`array[1,null] = array[1,null]` is **true** and `row(1,null) = row(1,null)` is
**true**, both NULL-*aware* rather than NULL-propagating — were found by
measuring, not by reasoning about the source, and this phase multiplies that
surface by an order of magnitude.

*Rejected: continuing to argue every claim from the PostgreSQL source alone.*
That is the method that left two traps to be discovered by accident.

*Rejected: testing against the koji replica behind `PGDQ_KOJI_PG_URL`.*
`CLAUDE.local.md` scopes that replica to ad-hoc local validation and forbids
anything committed from assuming it exists. A generated, committed answer table
has neither problem and covers six majors instead of one.

## Comparison is an L2 conclusion, carried in `ResolvedSchema`

`ordering_register`'s exhaustive `match` over `DataType` **moves out of
`predicate.rs` and into L2**, beside `pgtype.rs`'s mapping table. Resolution
already holds everything the closed rows need — the declared type string and
`DumpMetadata`'s `TypeKind::Enum { labels }` — and `resolve_columns` is where
both are in hand at once.

The result is a per-column comparison plan, a **fourth positional vector** in
`ResolvedSchema` beside `columns`, `notes` and `plans`. `predicate.rs` consumes
it and stops inspecting `DataType` at all; L4 asks "how does this column
compare", never "what Arrow type is it".

This is inside L2's rules rather than a stretch of them: it is a pure
synchronous function over data L1 already produced, it names `DataType` without
building an array, and it is not persisted (the cache holds L1 vocabulary only,
[`layering.md`](layering.md) rule 5). L2 already owns *how a value of this
column decodes*; *how two values of this column compare* is the same kind of
fact.

**The exhaustiveness property is strengthened, not lost.** It is what makes a
new mapping in `pgtype.rs` a compile error rather than a silent
misclassification, and after the move the arm that maps a type and the arm that
says how it compares are edited in the same file instead of two layers apart.

*Rejected: keeping the register in L4 and threading its extra inputs through
`resolve_term`.* That signature grows by one argument per type that needs one
more fact, and it leaves L4 asking L2-shaped questions.

*Rejected: carrying the enum's labels in the Arrow field's metadata so that the
`DataType` key still suffices.* It smuggles a PostgreSQL fact into a structure
nothing type-checks it in, to preserve a key that is being replaced for good
reasons.

## `--where` is a new flag; `--filter` does not change meaning

The expression grammar is **opt-in**: `pgdq query --where '<expr>'`, with
parens, `AND`/`OR`/`NOT`, and the existing term grammar
([`architecture.md`](architecture.md), "A filter term is parsed for two
audiences") as its **leaf**. `--filter` keeps its current meaning exactly —
one term, repeatable, ANDed — and a query using both ANDs the two.

*Rejected: widening `--filter` itself to accept an expression.* It is the
`IS NULL` hazard one level up and worse. `--filter 'note=a or b'` is an
equality against the string `a or b` today; under an expression grammar the
same unchanged command line would silently become a disjunction. A wrong row
set from a command that did not change is this phase's worst failure class, and
the simple audience the two-audience grammar was built for is exactly the
audience that would hit it.

*Rejected: landing the tree library-only and leaving the CLI for later.* The
phase's point is what a filter means; reachable only through an embedder, it
means nothing to the tool's actual users.

## Nested comparison is structural, two-valued, and inherits comparability

A nested column gets `=`/`!=` **and** the four ordering operators, compared
structurally rather than as text:

- **Array** — dimension count, dimensions and **lower bounds** first, then
  element-wise. `array_eq` memcmps `dims` and `lbs` before it looks at an
  element, so `'[0:1]={1,2}'` and `'{1,2}'` are unequal and the `[lb:ub]=`
  decoration is semantically load-bearing.
- **Composite** — field-wise in declaration order, which is also the order the
  dump writes them in, so `record_eq`'s positional rule costs nothing here.
- **Range** — lower bound then upper, after canonicalizing the three discrete
  built-in ranges (`int4range`, `int8range`, `daterange`); `numrange`,
  `tsrange` and `tstzrange` do not canonicalize.
- **Multirange** — member-wise over canonicalized members.

**One NULL rule, at every level: two NULLs are equal, and NULL sorts above
not-NULL.** `array_cmp` and `record_cmp` carry that sentence verbatim, and it
covers equality and ordering alike. So a nested comparison is **two-valued
throughout** — it never yields `Unknown`. Only the whole field being NULL
(`\N`) makes a nested term unknown, exactly as for a scalar.

**Comparability is inherited.** A nested column is comparable exactly when
every element or field type beneath it is, and is **refused** otherwise, naming
the element type that has no order — the refusal shape `resolve_terms` already
raises. That mirrors PostgreSQL, which looks up the element type's comparison
proc and raises when there is none: a `json[]` column, and a composite with a
`json` field, have no `=` and no `<` on the server either.

Inheritance runs through divergence as well as refusal: a `text[]` column
inherits the collation boundary and is a *divergence*, not a refusal. So
`OrderingNote` has to be able to name a nested position rather than only a
column.

*Rejected: falling back to today's text comparison for a nested column whose
elements are not comparable.* It is the worse half of both options — a `json[]`
column would silently answer a question PostgreSQL declines to answer, under an
operator that on every other array column means something structural.

*Rejected: keeping nested columns as text and adding range canonicalization
alone.* It leaves the array and composite traps in place, which are the two the
inbox had to measure to find.

## The literal side gets a bounded input grammar

The **field** side is unchanged: the file holds canonical `*_out` form and
`nested::decode_*` already reads it. The **literal** side gets its own parser,
implementing the enumerable superset `array_in`/`record_in`/`range_in` accept
over what the matching `*_out` writes — which is what makes
`--filter 'tags={a, b}'` mean what it looks like.

**They are three grammars, not one, and whitespace is where they first
disagree.** `array_in` skips ASCII whitespace around elements and braces;
`record_in` does not, so `'( 1 , a )'` into a composite preserves the blanks
around an unquoted field and force-quotes them back on output as `(1," a ")`,
while `'{ 1 , 2 }'` into `integer[]` canonicalizes to `{1,2}`. A parser written
once against the array rules and reused for the other two strips spaces a
composite field is entitled to keep, and matches nothing. Both literals are in
the oracle's `literals.tsv` on all six majors.

For arrays the superset is:

- ASCII whitespace skipped around elements and braces — `array_isspace`, which
  is deliberately *not* the locale's `isspace`.
- An **unquoted** `NULL` matched case-insensitively (`pg_strcasecmp`, under the
  `array_nulls` GUC, on by default), where `array_out` writes exactly `NULL`
  and force-quotes an element whose text is `NULL`.
- Quotes and backslash escapes accepted anywhere, not only where `needquote`
  would have forced them.
- The `[lb:ub]=` decoration accepted even when every lower bound is 1, which
  `array_out` omits.

**The risk runs toward over-acceptance**, not under: a literal we take that the
server would reject is a divergence in the direction nothing else in this
system permits. The oracle fixture is where that is checked — its table carries
malformed literals, and refusing what the server refuses is part of the diff.

*Rejected: accepting canonical output form only, by reusing `nested::decode_*`
for the literal.* A space after a comma is what a person types, and refusing it
makes nested comparison worse to use than the text comparison it replaces.

*Rejected: normalizing the literal with a cheap pre-pass and feeding the strict
decoder.* Stripping whitespace is wrong inside a quoted element, so the
pre-pass has to parse the literal to know where it may strip — at which point
it is the parser, written informally.

**Canonicalization is not total.** `daterange_canonical` skips any bound that
is `DATE_NOT_FINITE`, so `[2020-01-01,infinity]` keeps its inclusive upper
rather than becoming `[…,infinity)`. The exception is exactly the values I34
covers, and the range comparison must reproduce it.

## Evaluation short-circuits, and an error surfaces only where it is reached

Evaluation is left to right and stops as soon as the root's value is
determined, as `matches_all` already does. A decode failure under an ordering
operator stays a hard `Error::FieldDecode`, raised where it is reached — so
**which rows error depends on where the term sits in the expression**.

That is a property, not a defect. When `a=1 OR b<2` short-circuits past a
corrupt `b`, the row's answer was already settled by a field that did decode,
so nothing wrong is returned; it is the asymmetry
[`architecture.md`](architecture.md) already records for projection, where
deciding needs strictly less than materializing.

*Rejected: evaluating every leaf of every row so that a corrupt field errors
regardless of expression shape.* It gives up the documented one-walk hot path
to buy determinism about which error message appears, on a file that is already
contradicting its own DDL.

## The operator surface, and what it deliberately excludes

`OR` makes two candidate operators redundant before they are proposed: `IN` is
a disjunction of `=`, and `BETWEEN` is two ordering terms ANDed. Neither is
added.

`LIKE` is not added: it is a matching engine of its own — pattern syntax,
escapes, and case-folding that is collation-dependent, which is the boundary
this phase is otherwise declaring. Column-to-column comparison is not added
either; every `Predicate` is one column against one literal, and changing that
is a different feature.

**`IS DISTINCT FROM` / `IS NOT DISTINCT FROM` are added**, and they are the one
addition three-valued logic makes necessary rather than redundant: `NOT
UNKNOWN` is `UNKNOWN`, so `NOT (a = 1)` drops a NULL row while `a IS DISTINCT
FROM 1` keeps it. Without them a user has no spelling at all for "different,
counting NULL as a value". The cost is in the term grammar — an infix keyword
means a third parse path beside the operator split and the `IS NULL` fallback,
in the file whose ordering hazards are already documented at length.

*Rejected: adding `IN` for the ergonomics of a long list.* A phase about what a
filter means should not also be inventing list syntax. If repeated `OR` proves
painful in use, `IN` is an out-of-band ergonomics item with no decision behind
it, which is what that ledger is for.

**The type fixtures already carry the columns this needs.**
`scripts/fixture_schema_types.sql` has enum (including labels with a space, a
comma and a quote), typed and untyped `numeric`, `interval`, `time with time
zone`, `json` and `jsonb`, `inet`/`cidr`/`macaddr`/`macaddr8`, arrays with NULL
elements, the `[lb:ub]=` decoration, multi-dimensional arrays, composites with
array fields, built-in and user-defined ranges, multiranges, and domains. What
the oracle slice adds is the answer table, not a schema.

## A divergence names its position, not just its column

`OrderingNote` gains a **path** — an array element, a composite field name, or
a range bound — and its message is built from the declared type *at that
position*. Notes stay per-stream, deduplicated by `(column, path, divergence)`,
and are announced once after the schema resolves, as today.

The path is what keeps the existing message rule working one level down: the
sentence is chosen from the *declared* type, because several declared types
reach one Arrow type for different reasons. A composite may diverge at one
field and agree at another, and a `text[]` diverges at its element while the
array structure around it agrees.

*Rejected: keeping the note per-column and describing the nesting in prose.*
The message-from-declared-type rule then has no structure under it, and the
composition is re-derived per call site.

*Rejected: reporting only that the column's comparison diverges.* It discards
the only part of the note a user can act on.

## `--where`'s grammar

Parens group; `NOT` binds tighter than `AND`, which binds tighter than `OR`;
keywords are case-insensitive and recognised **only outside quotes**. Anything
that is not a paren or a keyword is a leaf, handed to today's `parse_filter`
unchanged.

Delegating the leaf is what makes the hazard tractable rather than merely
avoided: `--where 'note=a and b'` tokenizes to `note=a` AND `b`, and `b` is a
term with no operator and no `IS` suffix, so it is **refused loudly**. The same
string under `--filter` is still the equality it reads as, which is what the
flag split buys.

`&&`, `||` and `!` are **not** accepted as alternate spellings: one spelling,
and those symbols collide with values.

The parser is a new module in the **CLI crate**, not the library. `Predicate`
and now `Expr` stay plain structs an embedder fills in field by field, so
nothing below L4 parses text — the rule
[`architecture.md`](architecture.md), "A filter term is parsed for two
audiences" already states.

## The per-row field walk is left alone

Each term walks the row itself (`split_fields(raw_row).nth(index)`), and a tree
multiplies that: a five-way disjunction is up to five walks per row. **This
phase does not change it.**

P7 owns scan performance, runs next, and already owns `KD5` and `KD9`; its
zero-copy work touches this same row splitting, so doing it here means doing it
twice. And this phase's worst bug is a silently wrong row set, which
`../process.md` ("Size a slice by its review") says must not share a review
cycle with a rework of an already-tested core path. The sharpened fact — that
the cost is no longer bounded by "a conjunction whose leading terms nearly
always pass" — is filed in
[`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md).

## `=` and `!=` become typed too

They route through the same per-column comparison plan wherever the column has
one, and fall back to text where it does not — so equality and ordering agree
with each other and with the server by one mechanism, and the register's rows
describe both.

The justification the current text comparison rests on — every value in a dump
is already in canonical `*_out` form — is true of the **field** and says
nothing about the **literal the user typed**. Three consequences, none of them
registered anywhere before this phase:

- `--filter 'v=1.5'` matches no row of a `numeric(10,2)` column written `1.50`.
- `--filter 'v=a'` matches no row of a `char(5)` column: the dump writes the
  value blank-padded, and `bpchareq` compares after `bcTruelen` strips the
  padding.
- `--filter 'v=2020-01-01'` matches no row of a `timestamp` column written
  `2020-01-01 00:00:00`.

In each case PostgreSQL says equal and we say no, with no error — a plausible
command line and an empty result that reads as an answer, which is the shape
the `--filter` trimming rule was written to remove one level up.

*Rejected: leaving equality as text and documenting the three cases as
properties.* A phase named for typed predicates would ship an untyped `=`.

*Rejected: typed equality only for the types where the divergence is cheapest
to close.* The same thing with an arbitrary line through it.

## A census-refused array column compares as the column resolved

Two array shapes resolve to text while still holding array literals —
`NestedArrayElement` and `VaryingArrayShape`
([`architecture.md`](architecture.md), "Joining a header against the
metadata"). **They keep comparing as text.** The comparison plan follows the
column's *resolution*, never the shape of an individual value, so there is one
rule — compare as the column resolved — and no column whose meaning depends on
which rows a query happened to scan.

Comparing them structurally would break the property the census exists to
create: a column's type is a union over the blocks a query will replay, so a
structural comparison on a census-refused column would mean different things
for different queries over one file.

**This does not close `KD3`**, which is about materialization and stays
unowned. `KD8` is untouched for the same reason.

## Version-varying semantics: implement the union, and check the assumption

The newest semantics are implemented unconditionally, with **no branch on the
version the dump header records**. An older server cannot have produced the
value, so a reader that understands v17's `interval` infinities is never wrong
about a v13 file. Making `interval` typed is what puts this in play: I34's
scope limit excused interval infinities on the grounds that interval is held as
text and never reaches a typed comparison, which this phase makes false.

The union is safe exactly where the difference is *which values can exist*. It
stops being safe if two majors disagree about what the same text **means**, and
that condition is not left as a sentence to be trusted — it is checked.

### The oracle is cross-version, and the check is three-way

Each cell of the answer table records **whether the server accepted the input**,
not only the answer, and a table is generated and committed **per major**. A
cross-major diff then classifies mechanically:

| Older major | Newer major | Verdict |
|---|---|---|
| rejects the literal | accepts, answers | **additive** — the value could not previously exist; the union rule holds |
| accepts | accepts, **different** answer | **non-additive** — a real break, and the check fails |
| accepts | accepts, same answer | unchanged |

**The differences that already exist are committed**, not merely computed, and
the ordinary test suite asserts against that file — so a stale differences file
cannot be committed, and a regeneration that discovers a change forces it to be
filed. This is the discipline [`measurements.md`](measurements.md) uses for its
tables.

A **three-way reconciliation** keeps the check from quietly covering less over
time, in the style of `scripts/deficiencies.py`, failing on any direction:
every arm of the L2 comparison register resolves to at least one oracle case;
every oracle case resolves to a register arm; every major present in
`fixtures/` has an answer file. Coverage decaying as types are added to the
register without cases is the real failure mode, not the differ going wrong.

**It pays before the next major.** Run across 13–18 on first generation, the
differ names every place the six supported majors already disagree —
`interval` infinity (v17), `numeric` infinity (v14) and multiranges (v14) would
each have announced themselves that way instead of being found by reading
release notes.

**What it does not catch, said plainly.** A change to a type or operator no
case constructs — the case table is a living artifact and adding a case is
cheap, which is the only answer available. And a change in a value's *output
spelling* rather than its comparison answer, which is the fixture
regeneration's job and already appears there as a file diff.

*Rejected: a "things to check when a new major lands" process document.*
Nothing fails when nobody follows a checklist, and this project has twice
chosen a script over that — `measure.py --stale` and `deficiencies.py`. The
ritual keeps the home it already has:
[`postgres-invariants.md`](postgres-invariants.md) opens with "When a new major
lands, walk this file", and that paragraph gains the mechanical half — add the
version, regenerate, run the differ — while the prose half, re-running each
entry's `Re-verify` grep, stays as it is. A second document would compete with
that one for the same trigger.

*Out of scope, and named so it is not lost:* `Verified against:` is prose, so
nothing reports which invariant entries have never been checked against the
newest major. A parser over that field would make the "walk this file" ritual
auditable — but it is about all invariants rather than about comparison, so it
is out-of-band work, not part of this phase.

### Typed equality canonicalizes the literal once, not the field per row

The field is always in canonical `*_out` form, so the **literal** is decoded
once when the block resolves and rendered back into that same form; the per-row
comparison stays bytewise. `--filter 'v=1.5'` on a `numeric(10,2)` column
becomes a bytewise compare against `1.50`, and a `timestamp` literal renders
`2020-01-01 00:00:00`.

**`char(n)` needs the field narrowed per row, and is a third category.** Padding
the literal to `n` is sound for `=` and unsound for `<`: padding to a fixed
width is a bijection on the trailing-blank equivalence classes, so equality
survives it, but a byte below `0x20` sorts *under* the pad space where the
server, having stripped the pad, ranks the longer string above (I38's
corollary). The canonicalization that holds for both operators is to trim
trailing blanks from **both** sides — which touches the field per row, the thing
this section exists to avoid. It is admissible because it is not a decode: a
reverse scan for `0x20` yielding a shorter length, then a compare of that many
bytes. No allocation, no `*_in`, and only on a `char(n)` column. So the plan
carries three canonicalizations, not two states: decode per row (below),
canonicalize the literal once (everything else), and narrow the field per row
(`char(n)` alone). **11.6 owns it**, and closing it is what retires
`OrderingDivergence::BlankPadded`, which therefore lives exactly one slice.

**Two types in scope are exceptions**, and the comparison plan carries the flag
that says so:

- **bare `numeric`** preserves scale, so `1.5` and `1.50` are equal and both
  writable;
- **`interval`**, whose `interval_cmp_value` collapses months to 30 days and
  days to 86400 s, so `'1 mon'`, `'30 days'` and `'720:00:00'` are equal and
  written differently.

Those two decode per row; everything else compares bytewise. The result is the
rare shape where correctness improves and the hot path does not move: `=` on a
`text` or `varchar` column stays exactly the compare it is today.

*Rejected: decoding both sides per row, as ordering does.* It makes every
column pay for the two that need it, on the commonest operator.

*Rejected: canonicalizing the literal everywhere with no exception list.*
Unsound for both types above.

### The note channel is renamed

`TableStream::ordering_notes` becomes `comparison_notes`. Equality can diverge
for the same reason ordering does — under a non-deterministic collation neither
`texteq` nor `bpchareq` is bytewise — so a name saying "ordering" would leave
the equality divergence homeless. It diverges *knowably* where the collation is
user-defined, since the dump carries the `deterministic = false` outright, and
unknowably where the column has no clause and the database default is not in the
file (I32). The entry in
[`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md)
names the old method and is updated with it.

## Slices

Ordered so each makes the next one's mistakes visible: evidence first, and
never a pure refactor in the same diff as a change that moves answers.
Progress is tracked in [`../status/STATUS.md`](../status/STATUS.md), never
here.

| | Slice | What it lands |
|---|---|---|
| **11.1** | The comparison oracle | Per-major answer tables generated by `generate_fixtures.py` and committed. No library code. |
| **11.2** | The cross-major differ | Differ, committed differences file, suite assertion. Exercised against 13–18 the day it is written. |
| **11.2.2** | The fixture family moves to glibc | Every image Debian rather than Alpine, every fixture regenerated; text cases asked under `COLLATE "C"` and the database collation, for `<` and `=`; the platform triple and `collversion` as guarded apparatus keys. No library code. **Earned, not planned** — see below. |
| **11.3** | The comparison plan moves to L2 | `ordering_register` out of `predicate.rs`, keyed on the declared type, carried in `ResolvedSchema`. **No answer changes** — that is the review property. |
| **11.2.1** | The register-to-oracle reconciliation | Every arm of the comparison register resolves to at least one oracle case, and every oracle case to an arm; failing on either direction. **Earned, not planned** — see below. |
| **11.11** | The declared collation is read | `COLLATE` captured in the preamble parser instead of stopped at, and carried to the register: explicit `C`/`POSIX` and a bare `name` register *Agrees*, an explicit non-`C` clause *Diverges*, no clause on a `default`-collation type *unknown, therefore diverges*. Changes no comparison — only which columns are told they diverge. |
| **11.11.1** | The collated fixture columns | A `t_collate` table in the `types` schema — `COLLATE "C"`, `COLLATE "en_US.utf8"`, `COLLATE "ucs_basic"`, a bare `name` column and a domain `AS text COLLATE "C"` — regenerated across six majors, so the *agreeing* halves of 11.11's collation rule have a real dump behind them; plus `oracle_register.py` taught the collation dimension, so the three collated arms stop collapsing to one; `character(10)` asked under both collations and given a `"a\t"` value, putting I38's ordering corollary in the oracle and sparing 11.6 a regeneration; a composite with a collated attribute, closing I37's last unobserved emission site; one assertion per column in `tests/ordering.rs`; and `generate_fixtures.py` reporting its own elapsed time, with the recorded figure in `architecture.md` and a stale-past-30-minutes warning. No library code. **Earned, not planned** — see below. |
| **11.11.2** | The displaced `COLLATE` clause, observed | Four more columns on `t_collate`, none costing the one-alphabet property: `v_text_def text DEFAULT 'x' COLLATE "C"`, which holds the alphabet, enters the stream and puts I37's *placement* claim — the clause written after `DEFAULT`/`GENERATED`/`NOT NULL` rather than beside the type — into committed bytes at six majors instead of one lost container; `v_gen_nn text GENERATED ALWAYS AS (upper(v_src)) STORED NOT NULL COLLATE "C"` with its `v_src`, which stacks all three displacers in one fragment at no cost to the alphabet, because a `STORED` generated column is absent from `COPY` — and which is the only real-dump stress on `extract_collation`'s paren-aware scan, the existing generated fixture column being `integer` with nothing after its expression; and `CREATE COLLATION public.c_collation FROM "C"` with a column of it, which observes the user-collation reference form (schema-qualified, unquoted, outside `pg_catalog`) and pins `collated_text`'s deliberately conservative `NonBytewiseCollation` answer for a collation the same dump shows to be `locale = 'C'`. Plus I37 amended for v18's two new displacers and given a re-runnable probe recipe, and a `pg-dump-compatibility.md` row marking those two shapes untested and naming the blocker. **Asserted in two files**: the reachable columns in `tests/ordering.rs` beside the existing five, and `v_gen_nn` in `tests/preamble.rs`, which walks `DatabaseMetadata` over all six majors — because a column with no data never enters a `TableStream`. **No oracle cases and no `KD<k>`**: the user collation is a pgdq verdict rather than a server answer, and a spurious note over correct rows is a property, not a deficiency — its paragraph is mirrored into `architecture.md`, "Ordering operators compare typed". No library code. **Earned, not planned** — see below. |
| **11.4** | Enum and bare `numeric` | The two rows the re-key was for: declaration order, and arbitrary-precision decimal with all three specials. |
| **11.5** | The text-held type queue | `interval` (with v17 infinities), `time with time zone`, `inet`/`cidr`/`macaddr`/`macaddr8`. Repetitive and additive; the oracle checks each. |
| **11.5.1** | `jsonb` | The one text-held type whose comparison is a container walk rather than a scalar decode, and the one whose leaves reopen the collation question `text` already has. **Earned, not planned** — see below. |
| **11.6** | The `character(n)` trim | `CompareKind::PaddedText` — trailing blanks off both sides, then the clause — which retires `OrderingDivergence::BlankPadded` and gives `character` the same three collation arms `text` has. Rewrites `KD7` to the one statement that survives. **Rewritten to the scope that landed** — see below. |
| **11.6.1** | Typed `=` / `!=` | Routed through the now-complete plan, with the canonicalize-once fast path and its decode-per-row exceptions. Renames the note channel — which is also where the one equality divergence the file *states* gets reported: a column on a user-defined collation the dump declares `deterministic = false` (I42), for which `texteq` is not a byte comparison and nothing is raised today. **Earned, not planned** — see below. |
| **11.7** | Three-valued evaluation | `Expr`, the `True`/`False`/`Unknown` domain, `IS DISTINCT FROM`. Library only. |
| **11.8** | `--where` | The expression grammar, its own CLI module, leaf delegated to `parse_filter`. |
| **11.9** | The nested literal input grammar | Parser for the `array_in`/`record_in`/`range_in` supersets — **three grammars, not one** — checked against the oracle's malformed cases. No comparison yet. |
| **11.12** | The non-deterministic collation, observed | A `CREATE COLLATION` with `provider = icu` and `deterministic = false`, and one `t_collate` column of it, regenerated across six majors — so I42's claim that a plain dump *states* non-determinism rests on committed bytes rather than on `pg_dump.c` alone. **No oracle case**, which is what keeps the ICU exclusion above intact: the oracle builds its own temp tables per case, so the column obliges none, and the dump text holds no `collversion` to drift. Asserted in `tests/ordering.rs` beside the other collated columns, and the `fixture_schema_types.sql` comment rewritten to say which end ICU is now out of. No library code. **Earned, not planned** — see below. |
| **11.10** | Nested structural comparison | Element-wise/field-wise/bound-wise, the NULL rule, inherited comparability, range canonicalization, paths in the notes. |

**11.2.1 was earned, not planned.** The reconciliation was written into 11.2's
row and is not buildable there: two of its three directions join against the L2
comparison register, which 11.3 creates, and the third — every major present in
`fixtures/` has an answer file — is a precondition the differ checks for itself
before it may zip two files. So it sits after 11.3 instead, which is also where
its second direction stops being a moving target: an arm added in 11.4 or 11.5
without a case is exactly the decay the check exists to catch. That the seam
was found on entry rather than at spec time is where this plan was weak.

**11.2.2 was earned too, and runs before 11.3.** It is apparatus work with no
library code, exactly like 11.1 and 11.2, and it runs next rather than after
11.3 for the phase's own reason: evidence before the code that leans on it. It
is the largest of the three by diff — every generator, every fixture, every
correctness test — and the smallest by decision, since nothing it produces
changes what pgdq answers. Once 11.3 moves the register to L2 the
collation row's closure is a paragraph someone is reading rather than writing.
That a `.2` lands before a `.1` is not a defect in the numbering — the number
is identity and the table is the schedule, which is the case those rules exist
for.

*Rejected: folding the reconciliation into 11.3's own scope.* It is the
cheaper-looking option — two small slices become one — but 11.3's review
property is "no answer changes", and a diff that also adds a coverage check is
a diff where that question is harder to ask. The check is an increment of its
own for the same reason 11.3 is: what a reviewer must hold in mind at once is
the thing being kept small.

**11.11.1 was earned on entry to 11.11.** No fixture in the tree carries a
`COLLATE` clause or a `name` column, so nothing 11.11 could write would exercise
the two cases where its answer *changes to agreement* — the half that matters,
since a wrong "agrees" is the one error the register must not make. Adding them
means editing `fixture_schema_types.sql` and regenerating all six majors, which
rewrites every file under `fixtures/` (the `\restrict` token is fresh per dump),
and that diff cannot share a review with the library change whose whole question
is "did the right columns change verdict". It is apparatus work with no library
code, exactly like 11.1, 11.2 and 11.2.2, and it lands as its own increment for
the same reason those did. 11.11 ships with unit tests over the exact strings
`pg_dump` writes — pinned by I37, which was taken from the source and observed
on a throwaway container — and with the *divergent* halves exercised end to end
on the existing `t_text`.

*Rejected: regenerating the fixtures first, so no slice's code lands ahead of
its evidence.* This phase orders evidence first and 11.1, 11.2 and 11.2.2 all
obeyed it, so 11.11 is the one place it inverts — and the inversion is still
right. The two halves ask different review questions: "did exactly the right
columns change verdict" against "is this the DDL a real server writes". And a
regeneration rewrites all 109 files under `fixtures/` whether or not their
schema moved, because `\restrict` carries a fresh random token per dump, so a
combined diff buries the library change in churn that says nothing about it.
The agreeing cases are not unevidenced either: I37 records the clause's emission
condition, placement and spelling from `pg_dump`'s source and observed on a live
server at all six majors, which is the stronger of the two kinds of evidence.
11.11.1 adds a regression guard on top of it rather than the first proof.

**Which five columns, and why no ICU.** Every fixture database is `en_US.utf8`
at all six majors (`fixtures/<v>/oracle/meta.tsv`, `datcollate`), so a no-clause
`text` column already carries the non-bytewise case and `COLLATE "C"` really
does emit a clause. `en_US.utf8` named explicitly emits one too — `pg_dump`
compares collation *OIDs*, not semantics, so a column pinned to the collation
that happens to be the default is still not the default — and it is the only
genuinely non-bytewise choice available at every major without generating a
locale. `ucs_basic` earns its column for the opposite reason: it is
`collcollate = C`, bytewise in fact, and not named `C`, so the register must
call it divergent. It is the one place the asymmetry rule is pinned by a dump
rather than by a sentence. **ICU stays out of the *comparison* columns** —
`unicode` and the `*-x-icu` family carry a `collversion` that moves with the ICU
release, which is a new apparatus key guaranteed to drift, and the oracle
already refused a locale-named collation for the same class of reason.

*That exclusion is scoped to behaviour, and 11.12 adds the one ICU shape it does
not reach.* A non-deterministic collation is always ICU (I42), and its dump text
— provider, locale, `deterministic = false` — carries no `collversion`, because
`pg_dump` emits `version =` only under `--binary-upgrade`. So a column of one
puts the shape in committed bytes without importing the drift, provided it earns
no oracle case: the oracle builds its own temp tables per case, so a `t_collate`
column obliges none.

**Why the reconciliation comes along.** 11.11 split one `builtin_scalar` arm
into three that branch on the clause, and `oracle_register.py` joins on the
declared type alone — so the three collapse to one `text` arm and a fourth
could be added with nothing behind it. The oracle already carries the
dimension, since every text pair is asked under `COLLATE "C"` and under
`default`; what is missing is that `TypeCases.collation` describes the
comparison rather than the column, and the join never reads it. Both halves are
apparatus with no library code, which is the property that separated 11.11.1
from 11.11 in the first place, so they share a review without costing the
distinction the split was for.

**Three arms, two case groups, and the mapping that joins them.** The oracle
asks each text pair under `COLLATE "C"` and under `COLLATE "default"` only, so
the non-`C`-clause arm has no group of its own. It joins to the `default` group,
as the no-clause arm does: "asked under something that is not `C`" is one
population, and the database's own collation is a member of it. *Rejected:
asking a third collation by name.* `datcollate` is `en_US.utf8` at all six
majors, so `COLLATE "en_US.utf8"` and `COLLATE "default"` are the same
collation — every added cell would be byte-identical to one already in the file.
**The mapping's soundness is asserted, not assumed**: it holds only while the
database's collation is not itself bytewise, so the check reads `datcollate`
from `meta.tsv` and fails if it is `C` or `POSIX`. Without that, an apparatus
initdb'd under `C` would invert the `default` group's meaning and the join would
go on passing while meaning the opposite thing.

**`collation=None` is made to mean one thing, and the case table is edited to
keep it that way.** It currently means two: for the plain `text` and `character
varying(10)` groups, asked with no qualifier, the server uses the database's
collation — the *divergent* population, identical in meaning to `"default"` —
while for `name` it is the *agreeing* one, that type's own default being `C`. A
join that resolved `None` per type would be a second copy of the register's
type-default rule living in Python, which is the fork 11.2.1 exists to prevent,
arriving from inside the check. So the plain `text` and `character varying(10)`
groups state `collation="default"` explicitly, and `None` is left to mean **the
register does not branch on the clause for this type**. That covers `name` and every
non-collatable type at once, and it leaves a collation label only where the
register genuinely branches.

**`character(10)` is asked under both collations too, ahead of 11.6.** It looks
today like an instance of the `None` rule — the register is clause-blind there,
one arm, no branch — and it stops being one the moment 11.6 lands: a `char(n)`
compared *trimmed under its collation* splits into the same three arms as
`text`. Its cases are labelled now, in the regeneration that is already
happening, rather than costing 11.6 a six-major run of its own. The added rows
carry real information, unlike the third collation rejected above: under
`default` a
`char` comparison runs the locale over the trimmed text, which nothing in the
file holds. `name` is then the one deliberate `None` among collatable types.

*Rejected: relabelling `name`'s cases `collation="C"`.* It is true, and it is
the register's claim rather than the oracle's observation. A `name` case asked
under an explicit `COLLATE "C"` is a different question from one asked bare, and
only the bare one shows what a `name` column does.

**The `character(10)` cases gain `"a\t"`.** They are `("a", "a" + nine blanks,
"hello", None)` today, which pins the equality half of I38 — a padded and an
unpadded value are equal — and cannot reach the ordering half at all, since
pad-and-compare and trim-and-compare disagree only against a byte below `0x20`
and every value there is printable. Two rows appear, against `"a"` and against
the padded `"a"`, and they are exactly that disagreement. It lands here rather
than in 11.6 because it is oracle apparatus and this slice already regenerates
all six majors: 11.6 then opens with its evidence committed instead of producing
it as a side effect of the change it is meant to check, and the differ sweeps
the corollary across 13–18, which the single probe behind I38 cannot claim.
`escape_copy_text` already maps `"\t"`, so the TSV needs no format work.

**A composite with a collated attribute closes I37's last unobserved site.**
That entry's **Observed** paragraph covers a `CREATE TABLE` column only; the
`CREATE DOMAIN` and `dumpCompositeType` emissions rest on source greps, and the
container that produced even the one observation is gone. `t_collate`'s domain
covers the second site; a `CREATE TYPE public.collated_pair AS (plain text, c
text COLLATE "C")` with a column holding it covers the third — which 11.11 has
already leaned on, having widened `TypeKind::Composite`'s field list to
`Vec<ColumnDef>` on the strength of it. 11.10 inherits the fixture for a
composite whose field carries a collation boundary. I37's **Observed** paragraph
is rewritten to cite the committed dumps instead of a throwaway container.

**`t_collate` is populated with the divergent alphabet, not with placeholder
values.** The Rust assertions above test *notes*, which are independent of the
data, so any values would pass them and it would be easy to pick a set that can
never support a stronger test. The rows are `A`, `a`, `B`, `é`, `f`, `_x`, `ax`,
`''` and a NULL, replicated across all five columns — the pairs the oracle
already identifies as divergent on glibc 2.41, so `--filter 'v_text_c>A'` and
its `en_US.utf8` sibling genuinely return different rows. Deciding this after
the regeneration costs a second one.

**A non-additive cross-major difference is a stop, not a fix.** The new case
groups have never been swept, and a `char` comparison asked under a locale is
the shape most likely to move a cell from one *answer* to another rather than
from unsupported to supported — which is what I35 says never happens.
`oracle_differences.py` fails on it either way; what needs saying ahead of time
is that the response is to re-plan, never to widen the assertion or drop the
case that tripped it. The published counts move in the same pass — `1616 typed
comparisons`, `509 differences`, `33 arms, 50 case types`, in
[`../status/STATUS.md`](../status/STATUS.md) and the differences count also in
[`architecture.md`](architecture.md) — and each is read out of the regenerated
file rather than recomputed by hand.

**The baseline regeneration is also the timing probe, and the number is
recorded.** Nothing records how long a six-major run takes, and the slice runs
one twice. The baseline run above supplies the figure as a side effect of work
already required; it goes in [`architecture.md`](architecture.md), "Fixtures",
beside the mechanism. **The trigger is mechanical, not a comment**:
`generate_fixtures.py` prints its total elapsed time at the end, and past
**30 minutes** prints that the recorded figure is stale and must be updated —
30 being the threshold at which a job stops fitting inside one session and has
to be handed off. A prose "update this if it ever gets slower" would be a
trigger nobody reads at the moment it fires. If the baseline run itself comes in
near or over the threshold, the second regeneration is dispatched as a detached
job rather than run inline.

**The regeneration is run twice, and the first run is the instrument.** 11.11.1
opens by regenerating with `fixture_schema_types.sql` *unchanged* and asserting
that `git diff -I'^\\(un\\)?restrict '` is empty. That proves the token is the
only nondeterminism in a plain dump — the assumption this split's whole argument
rests on, and one nothing has tested — and it leaves a baseline against which
the second regeneration's diff is exactly the new table. A non-empty result is a
finding worth more than the slice, and the plan changes rather than proceeds.

**The five columns are asserted from Rust, in `tests/ordering.rs`.** "No library
code" is about `pgdump_query/src/`, as it was for 11.1 and 11.2.2; a fixture
nothing reads is inert, and the reconciliation's claim is coverage — that an arm
has a case — not that the arm answers correctly. One assertion per column:
`COLLATE "C"` and the bare `name` produce no note, `en_US.utf8` and `ucs_basic`
each produce one, and the domain inherits `C` and is silent.

**11.11.2 was earned from grilling 11.11.1's leftover.** 11.11.1 closed with a
"Decisions worth another look" entry asking whether I37's *placement* consequence
deserved a fixture column, and priced it at a `NOT NULL` exception to
`t_collate`'s one-alphabet property. Re-running the probe showed the price was
imaginary: `DEFAULT` alone displaces the clause and `NOT NULL` alone displaces
it, so a *nullable* column with a default observes the placement and still holds
the alphabet with `NULL` in its ninth row. The entry's other premise — that the
16.15 observation could not be re-run — was also false; every `-trixie` image is
cached locally and the probe takes twenty seconds. What is genuinely missing is
not the observation but its *form*: I37's strongest claim rests on a string
hand-transcribed into a unit test, where every other claim in that entry rests
on committed bytes at six majors.

The same probe turned up two things 11.11.1 could not have known. **I37 is wrong
by omission about v18**: `dumpTableSchema`'s append order there admits
`CONSTRAINT <name> NOT NULL` and `NO INHERIT`, and a *virtual* `GENERATED ALWAYS
AS (expr)` with no `STORED`, none of which v13–v17 can write and none of which
I37 names — so a reader building a parser from that entry meets a v18 dump it
does not describe. And **`objects.c_collation` exists as an object that no column
references**, leaving the user-collation reference form unobserved and the
register's most interesting cell — conservative and knowably wrong, kept
deliberately — with no test standing in front of it.

**The stack costs nothing, because the column that carries it has no data.**
A `STORED` generated column is excluded from the `COPY` column list, so
`v_gen_nn` can be `NOT NULL` without any row having to hold a value for it —
which puts `GENERATED`, `NOT NULL` and `COLLATE` in one fragment while
`t_collate` keeps one alphabet replicated across every column that has one.
The cost is that no ordering test can reach it: filtering or projecting a
column absent from `COPY` is not a query the stream can answer. So this slice
is the first where `t_collate`'s columns are asserted in two files rather than
one, and the split is by reachability, not by subject.

**The v18 shapes get a coverage row, not an owner.** I37 must name them or it
stays wrong, but "named and not exercised" is a coverage statement, and
`pg-dump-compatibility.md` is the register whose whole value is telling tested
from assumed — without a row there, someone walking the invariants at a PG19
release reads the amended claim with no way to see that two of its shapes rest
on source reading alone. The row says the blocker is schema-level version
conditioning: `generate_fixtures.py` conditions flag *sets* on version — that
is how `fixtures/18/objects/stats.sql` exists — but one schema `.sql` runs
against every major. That gap gets no further home. It is not a deficiency,
since nothing is wrong and no user is affected, and a roadmap "Future" row
would manufacture intent nobody holds; the matrix row names the blocker at the
one place a session wanting a version-specific DDL fixture will actually hit
it.

**Neither the oracle nor the deficiency register gains anything.** The oracle
records what the server answered, and `public.c_collation` is `FROM "C"`, so a
third collation column would duplicate the `C` column by construction while
being the first row in that file to exist for a decision of ours rather than a
fact of PostgreSQL's. And the register's answer for it — `NonBytewiseCollation`
on the strength of the schema, not the behaviour — produces *correct rows* with
a spurious advisory note, which is the definition of a property rather than a
deficiency: there is nothing for a user to remedy and nothing to fix. `KD7` is
the list of places the order genuinely differs, and this is precisely not one.
The reasoning already half-exists in `collation_is_bytewise`'s doc comment;
this slice mirrors it where a reader looks.

**The probe recipe stays even though the fixture supersedes it**, because the
two answer different questions. Once the columns land, the existing
`grep -n -A9 'CREATE TABLE public.t_collate' fixtures/*/types/default.sql`
re-verifies placement from committed bytes at the six majors we generate. The
recipe answers it for a *seventh* — a newly released major, before any fixture
for it exists — which is exactly the walk-the-register ritual a release
triggers, and it should not have to wait on a regeneration. It goes inline in
`Re-verify` rather than into `scripts/`, where nothing would consume it and it
would rot unrun.

**The two v18-only shapes are named in I37 and not fixtured**, which is the one
piece of this deliberately left undone. `generate_fixtures.py` carries
`min_version` for dump *flags* only; one schema `.sql` runs against all six
majors, so an 18-only DDL shape fails on 13–17. Version-conditional schema SQL is
a fixture-family capability, not a collation errand, and it wants its own slice
rather than riding in on this one.

**11.11 is discovered scope, not a split**, so it takes the next free number
rather than hanging off a parent; the table is the schedule, which is why it
reads third and numbers last. It sits after 11.3 because it consults the
register the re-key produces, and outside 11.3 because that slice's review
property is "no answer changes". It is not folded into 11.6 either: it touches
the preamble parser, and the note channel is only its consumer.

**11.5.1 was earned on entry to 11.5, and the seam is a decision rather than a
size.** 11.5's row named five type families as "repetitive and additive", and
four of them are: an `interval` is a 128-bit span, a `timetz` is a pair, a
network address is a family and a prefix, a MAC is its octets. `jsonb` is none
of those. Its comparison is a recursive container walk with its own type
ordering, its literal side needs a JSON parser that sorts and uniqueifies
object keys where the *field* side arrives already sorted, and its numeric
leaves are `numeric_cmp` over a spelling `jsonb_in` normalizes. That is a
different confidence from "`inet` compares by family then bits", which is
exactly the argument that already separated 11.4 from 11.5, and
`../process.md`'s "Size a slice by its review" says the review is the unit.

**And the row it was written under is wrong about it.** This doc promised
`jsonb` "closed by code, one comparison per type"; `compareJsonbScalarValue`
passes `DEFAULT_COLLATION_OID` to `varstr_cmp` for every string leaf and every
object key, so a structurally correct `jsonb` comparison still diverges wherever
a string decides it — the same database-collation residue the text row has, and
one a plain dump cannot close (I32). The closure table above is amended to say
so. Landing that under a row promising a full closure would have shipped a
`KD7` strike the register cannot support, which is the failure the spec/notes
split exists to make visible. The evidence is in
[`../status/history/2026-09-01.md`](../status/history/2026-09-01.md).

**11.12 was earned by a review, and it amends a decision rather than adding
one.** This doc rejected ICU twice — once for the oracle, once for the fixture
columns — and both arguments are about `collversion` drifting with the base
image, which is a statement about *answers*. Reviewing `KD7` turned up a shape
neither argument reaches: a non-deterministic collation is ICU-only (I42), its
dump text carries no version, and what it puts in the file is a fact about
*equality* rather than about order. The exclusion is therefore scoped rather
than reversed, and the slice takes the next free number rather than hanging off
a parent, since it is discovered scope and not a split. It runs after 11.11 and
so lands after 11.6.1 consumes I42; that is deliberate — the invariant rests on
upstream source, which is the register's strongest proof class, and the column
is a regression guard on top of it exactly as 11.11.1 was. Reasoning:
[`../status/history/2026-09-01.md`](../status/history/2026-09-01.md), "A plain
dump *does* record `deterministic = false`".

**11.6 was mis-sized, and 11.6.1 was earned from it.** Its row paired two
things with different review questions. The `char(n)` trim is a **register
correction**: one `CompareKind`, one arm, and an oracle that already carried
its evidence — 11.11.1 put the tab-bearing `character(10)` cases in the file
one slice early, precisely so this change could be checked against evidence it
did not produce, and it retires eight exception entries under both collations
at six majors. Typed equality is a **new mechanism over every `CompareKind`**:
a canonical rendering per kind, whose exception list turned out to be longer
than this doc's two — `jsonb` re-renders a number through `numeric_out`, so
`{"a": 1.50}` and `{"a": 1.5}` are one value written two ways, and a
`double precision` field written `-0` equals the literal `0`. Landing both
together would have made a reviewer accept the second at the first's
confidence, which is what `../process.md`'s "Size a slice by its review"
forbids. The evidence for the split is in
[`../status/history/2026-09-01.md`](../status/history/2026-09-01.md).

**And the trim is the better half to land first**, which is why it keeps the
number: equality's `char(n)` canonicalization is *this* trim, so 11.6.1 inherits
a comparison rather than inventing one, and the phase's own rule — evidence
before the code that leans on it — is obeyed twice over.

Five seams are deliberate. **11.3 stands alone** because a refactor whose
review question is "did anything change?" cannot share a diff with one that
changes answers. **11.4 is apart from 11.5** because arbitrary-precision
decimal with three ordered specials is a different confidence from "`inet`
compares by family then bits". **11.6.1 follows 11.5** so equality inherits every
type's comparison at once rather than being revisited per type. **11.9
precedes 11.10** because the input grammar is the part with an external oracle
and the part most likely to be wrong. And **11.5.1 is apart from 11.5** for the
reason above.
