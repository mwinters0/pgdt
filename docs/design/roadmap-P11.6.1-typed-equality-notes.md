# P11.6.1 — typed `=` / `!=`

What the rest of P11 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "Equality is typed
too".

## What landed

`=` and `!=` stopped being the two operators that did not consult the register.

- **`equality_comparison`** in `predicate.rs` — one exhaustive `match` over
  `CompareKind` beside `order_key`'s, answering the three-way canonicalization
  in the same place a new kind has to answer how it decodes.
- **`Comparison`** replaced `OrderTerm`'s ordering-only shape: `Ordered`,
  `Canonical`, `Trimmed`, `Decoded`. `ResolvedTerm` now carries a
  `ComparedTerm` for every operator but the two NULL tests, which is what gives
  equality a divergence and an `Error::FieldDecode` context it did not have.
- **`ComparisonDivergence::affects_equality`** — the operator dimension.
  `OrderingNote`/`ordering_notes`/`OrderingDivergence` became
  `ComparisonNote`/`comparison_notes`/`ComparisonDivergence`, and the notes are
  per **term** rather than per column.
- **`ComparisonDivergence::UnmodelledType`** — new, and the only answer-moving
  discovery of the slice; see below.
- **The oracle test asserts all six operators**, 45,394 cells over six majors,
  and builds its one-column schema through `resolve_columns` rather than by
  hand.

## The rule that decides which kind canonicalizes

**Is the file's `*_out` text a unique spelling of the value it holds?** Five
kinds answer no — bare `numeric` (display scale), `interval` (months collapse),
`jsonb` (numbers through `numeric_out`), and `real`/`double precision` (two
zeros) — so no rendering of the literal makes them bytewise and both sides
decode per row.

**Two more decode for a reason about this build**, and this is the part a later
type will be judged by rather than by the list: `time with time zone` and
`inet`/`cidr` *are* uniquely spelled, and reproducing those spellings means
re-implementing `EncodeTimeOnly` plus `EncodeTimezone`, and
`pg_inet_net_ntop`'s IPv6 zero-run compression. Writing an output function to
save a fixed-size parse per row is the wrong trade, and getting one subtly
wrong is a *silently empty result*, not an error. `macaddr` went the other way
on the same test — its output rule is one sentence — which is what keeps the
rule a rule rather than a preference.

**`character(n)` is the third category and cost nothing.** 11.6's
`CompareKind::PaddedText` is the canonicalization equality needed, so `=` on a
`char(n)` column is `Eq` over two trimmed slices. The manual's "they are not
blank-insensitive, though" paragraph and its two workarounds are gone.

## `box` is why `UnmodelledType` exists

Turning the oracle's `=` cells on found **exactly two disagreeing cases in the
whole corpus**, both `public.box_domain`, at all six majors: `box_eq` compares
the two rectangles' *areas*, so the server calls `(1,1),(0,0)` and
`(3,3),(2,2)` equal and a byte comparison does not. Nothing else moved — every
nested type, the whole text population, `jsonb`, the floats, `char(n)` and
`numeric` all agree under `=`.

That is a defect this build already had and never announced, so it is `KD10`,
`(c) unowned`, with the detail beside the mechanism. The note fires for exactly
two column resolutions — `UnknownType` and `OpaqueBaseType`, the two that mean
*the file named a type and this build models nothing for it*. **Not** for a
nested column (its `array_out` text is a faithful rendering; the inherited
comparability a `box[]` would need is the nested slice's) and **not** for a
column with no DDL behind it (`--schema-mode strings` would otherwise warn once
per term for a typing the caller gave up deliberately).

*The divergence is synthesized in `resolve_term`, not returned by
`comparison_for`.* `ComparisonPlan::Refused` is what refuses an ordering
operator, and making the register return `Compared { Text, UnmodelledType }`
for `box` instead would replace that refusal with a wrong answer under `<`. So
L4 reads the column's resolution and picks the L2 vocabulary word; the variant's
doc comment in `pgtype.rs` is where its meaning lives.

## What the oracle assertion is actually worth

**Both operands of a cell are values the server itself stored**, so for a
canonicalized kind the `=` assertion is that the file's own spelling compares
byte for byte — near-trivial. The content is entirely in the kinds where it
cannot be, and the case table happens to carry one of each: `real`'s `-0`
against `0`, a bare `numeric`'s `1.5` against `1.50`, a `jsonb` number written
two ways, a `character(10)` value padded against one that is not.

*Rejected: feeding the raw right literal instead of `literals.tsv`'s `output`,*
which 11.6's notes proposed as the way to exercise the canonicalization. It
asks a different question. The oracle's `input` column is a value put through
an **assignment cast** into a typed column — `'12345678901'::varchar(10)`
truncates, `'abc'::bytea` is `\x616263` — where a filter's literal is compared
against the column, and this build reads it in the type's own `*_out` form and
no wider by a documented decision. Feeding raw literals would report that
decision as dozens of comparison disagreements. The canonicalization is covered
by unit tests instead, where the two sides can be stated.

**The synthetic schema became a real one.** `schema()` now builds a one-table
`DumpMetadata` and calls `resolve_columns`, because a hand-built
`ResolvedSchema` claiming every case is `Mapped` and `Scalar` put `integer[]`
and `box` in one population no dump produces — and which population a column is
in is exactly what decides whether its equality term announces.

## What 11.6.2 inherits

**The channel exists and is empty for the right reason.** `comparison_notes`
carries equality divergences, `affects_equality` is the switch, and the one
variant that answers `true` today is `AsText` (`json`, where the server defines
no `=` either) plus `UnmodelledType`. All three collation variants answer
`false`, and the doc comment says why: every libc collation is deterministic.
A non-deterministic ICU collation is the shape that breaks it, and 11.6.2 adds
the variant plus the parse.

**The parse is the whole cost, and it is not in `predicate.rs`.**
`CREATE COLLATION` reaches the file map as `SpanBody::Unparsed` — `map.rs`
matches no `COLLATION` anywhere — so reading `deterministic = false` needs a new
`SpanBody` variant (with the fan-out `architecture.md`'s "`Event` is the
scanner's contract" names), a `preamble.rs` parse, a field on
`DatabaseMetadata`, a `cache.rs` `FORMAT_VERSION` bump, and a fourth argument
to `comparison_for`. `fixtures/<13-18>/types/default.sql` line 25 already
carries `CREATE COLLATION public.c_collation (provider = libc, locale = 'C');`
— a *deterministic* one — so the parse has a committed shape to read before
11.12 supplies the non-deterministic one.

## What the rest of the phase inherits

**Nested `=` is asserted and agrees.** Every nested case in the oracle —
arrays, composites, ranges, multiranges, and the array with a NULL element —
answers `=` correctly as text at all six majors. That is the canonical-form
argument holding, and it is now checked rather than argued; the nested slice
changes those columns to a structural comparison and inherits the assertion as
a regression guard.

**`ComparisonNote` still has no path.** The spec owes each note an array
element / composite field / range bound position, and nothing here added one;
the struct is still `(column, declared_type, divergence)`.

**One spec example did not land as a match.** `--filter 'v=2020-01-01'` on a
`timestamp` column is a named refusal, not a match against
`2020-01-01 00:00:00`, because `decode_timestamp_micros` requires a time part
and widening the literal grammar past `*_out` would contradict the register's
own stated property. The failure the spec objected to — an empty result that
reads like an answer — is removed either way. It is flagged under `STATUS.md`'s
"Decisions worth another look".
