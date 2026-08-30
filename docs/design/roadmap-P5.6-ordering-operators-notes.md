# P5.6 — Typed ordering operators: notes

What the next slice inherits from making `<`, `<=`, `>`, `>=` real. The spec
is [`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md); the mechanism lives in
[`architecture.md`](architecture.md), "Ordering operators compare typed"
(inside "Predicates"); what has landed is
[`../status/STATUS.md`](../status/STATUS.md).

## What is there now

`PredicateOp` has four more variants. Everything they need beyond an index is
settled once per block, in `predicate.rs`:

- **`ResolvedTerm`** replaces the bare `usize` a term used to resolve to. It
  carries the field index and, for an ordering term only, an `OrderTerm`: the
  column name, an `OrderKind` (how the column's text becomes a comparable
  value), the filter's literal already decoded, the declared type string, and
  the divergence if there is one. `Active`'s fourth element is
  `Vec<ResolvedTerm>`.
- **`ordering_register`** is the register in code: one exhaustive `match` over
  `DataType`, no wildcard, returning `(OrderingSupport, Option<OrderKind>)`.
  Refused types pair with `None`, which is what makes "refused" and "has no
  decoder" one fact rather than two that could disagree.
- **`stream::resolve_predicate_indices` is `stream::resolve_terms`** and takes
  the whole `&ResolvedSchema` rather than `&SchemaRef`, exactly as `P5.5`'s
  notes said it would.
- **`TableStream::ordering_notes()`** is a new public method over a new
  `Arc<Mutex<Vec<OrderingNote>>>`, set at the same three sites the resolved
  schema is. `pgdq query` prints each once on stderr.

New errors: `Error::UnorderedPredicateColumn` (the refusal, with one of three
`&'static str` reasons) and `Error::PredicateValueDecode` (a literal that is
not a value of the column's type). `Error::FieldDecode` is reused verbatim for
a *field* that does not decode, which is why `matches_all` now takes the table
name and the row offset.

Register entry **I33** is new: the server-side comparison semantics the
register's *Agrees* rows and the enum row depend on, quoted from seven
worktrees. **I32**'s forward pointer now names `architecture.md` rather than
the P5 spec, because the register moved there as the spec said it would.

Tests: `predicate.rs`'s unit module goes from 8 tests to 20, and two new
files are added — `pgdump_query/tests/ordering.rs` (11, over the generated
`types` fixture) and `pgdump_query-cli/tests/query_ordering.rs` (11).

## Calls made here, and why

**The literal is decoded once, at block resolution, and a literal that does
not decode is an error there.** The alternative — decoding per row, or letting
a bad literal match nothing — makes `--filter 'n>abc'` a query with no rows
instead of a mistake, which is the class of answer this project refuses
everywhere else. PostgreSQL errors on the same input. It also means a
`numeric(10,2)` column compared against `1.005` is refused rather than
rounded: the literal goes through the column's own decoder, and that decoder
does not drop non-zero digits. Rounding would need PostgreSQL's half-even rule
before it were a comparison anyone could trust.

**A field that does not decode is `Error::FieldDecode`, not an excluded row.**
It is the same fault, about the same value, that the typed build path already
reports, so it gets the same sentence and the same escape hatch. The tempting
alternative is the NULL collapse — unknown to false — and it is wrong for a
different reason than it is right for NULL: a NULL is a value the file
*states*, while an undecodable field is the file contradicting its own DDL.
Consequence worth knowing: `--column id --filter 'v_date>2000-01-01'` on the
`types` fixture errors, because `t_date` holds `infinity`. Projecting a column
away escapes its decode failure; *filtering* on it does not.

**The divergence classification is keyed by the Arrow type, the message by the
declared one.** The spec's table has three divergent rows and two of them are
`Utf8View`; the code cannot tell them apart from the `DataType` alone, and a
classification that took the declared type as well would stop being the
exhaustive `match` the register's compile-time check depends on. So
`OrderingDivergence` has two variants (`AsText`, `EnumLabels`) and
`OrderingNote::message` picks the sentence from the declared type. That is
also what surfaced the register's fourth divergent row, below.

**The register has a row the spec's table did not.** `interval`,
`time with time zone`, `json`/`jsonb` and the four network types all map to
`Utf8View` and resolve `Mapped` with a `Scalar` plan, so they reach an
ordering operator and compare bytewise — and each has a server-side operator
of its own that bytewise is not. They are registered as divergent (the honest
answer, and the one the rule already produces) rather than refused, and the
table in `architecture.md` carries them as a fourth row. This changes no
decision: the spec's classification rule is unchanged and the code follows it.

**`--filter` is split at the earliest operator position, longest spelling
first**, rather than by trying operators in turn. Trying `>` before `=`
anywhere in the string would make `name=alpha>x` a filter on a column called
`name=alpha`; the old two-operator parser had the same latent bug with `!=`
and nothing had hit it. Column names now also lose their trailing whitespace,
as the `IS NULL` forms already did — strictly widening, since a trailing space
previously produced `UnknownPredicateColumn`. The value keeps its leading
whitespace, because a value may legitimately begin with a space and nothing
else could restore it; so `--filter 'v >= 5'` names column `v` and value
` 5`, and ` 5` does not parse as an integer. That is a refusal with a message,
not a wrong answer.

**The announcement is a third channel, and stays one.** `ordering_notes` is
per-column *and* conditional on a predicate — L4 — where
`DumpIndex.diagnostics` is L1 and `ResolvedSchema.notes` is L2. The P6 inbox
entry that predicted this is unchanged and still accurate; nothing new was
filed, because nothing new was learned.

## What the next slice must not break

**`P5.7` updates the manual**, and the ordering operators are the third thing
owed there, beside repeatable `--filter` and projection's per-column decode
escape. Three user-facing facts have no manual home yet: that `<`/`>` compare
typed and refuse a nested or unresolved column; that a `numeric` without a
typmod, a `text` column and an enum order differently from the server and say
so on stderr; and that a filter naming a column does not get projection's
escape from `Error::FieldDecode`.

**Staleness is unchanged.** This slice edits `predicate.rs`, `stream.rs`,
`error.rs`, `batch.rs` (one accessor) and the CLI. `--stale` reads the **same
eight** figures `P5.3`–`P5.5` left stale and no more; `predicate.rs` is
declared by no figure, correctly, since no registered command shape passes
`--filter`. None is acknowledgeable, for the reason `CLAUDE.md` gives, so they
stay stale until `P5.7`'s sweep.

**The register's compile-time check is the thing to preserve.** If
`pgtype.rs` ever maps a declared type to an Arrow type not already in
`ordering_register`'s match, the build fails until someone classifies it —
that is the whole mechanism keeping the table in `architecture.md` honest, and
a wildcard arm added for convenience would silently retire it.
