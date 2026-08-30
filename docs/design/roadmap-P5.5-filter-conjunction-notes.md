# P5.5 — The filter conjunction: notes

What the next slices inherit from turning one filter into a list of ANDed
terms. The spec is [`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md); the
mechanism lives in [`architecture.md`](architecture.md), "Predicates" (plus
its additions to "Resume" and "Testing philosophy"); what has landed is
[`../status/STATUS.md`](../status/STATUS.md).

## What is there now

`QueryOptions::filter: Option<Predicate>` is **`filters: Vec<Predicate>`**,
defaulting to empty. `Predicate` itself is untouched — it is now one *term* of
a conjunction rather than the whole filter, and its doc says so.

One new free function in `predicate.rs`, `matches_all(&[Predicate],
&[usize], raw_row)`, folds the terms with `AND` and short-circuits.
`stream::resolve_predicate_index` is **`resolve_predicate_indices`**, returning
one index per term; `Active`'s fourth element is a `Vec<usize>` parallel to
`filters` rather than an `Option<usize>`.

`pgdq query --filter` is repeatable (`Vec<String>`), and every term is parsed
before the source is opened.

`pgdump_query-cli/tests/query_filter.rs` is new — six tests driving the real
binary. `pgdump_query/tests/batch.rs` gains four.

## Calls made here, and why

**The empty conjunction is "no filter", and no code above `matches_all`
branches on it.** `Option<Vec<Predicate>>` would have carried two spellings of
the same query — `None` and `Some(vec![])` — differing in nothing except the
fingerprint they hash to. With a bare `Vec`, `resolve_predicate_indices`
returns an empty vector, `matches_all` returns `true`, and the row path has no
"is there a filter" case at all; the previous `match (filter, index)` tuple in
the replay loop is gone rather than widened.

**Each term walks the row itself; there is no shared pass.** `Predicate::
matches` already owns the walk-and-unescape of one field, and a shared pass
would mean collecting the needed fields into a buffer before evaluating
anything — which pays for every term on every row, where short-circuiting pays
for the first failing one. The ordinary conjunction is two or three terms with
a selective leading one, so the common case is one walk. The phase makes no
performance claim about filters (the spec's "no claim about rejected rows
getting cheaper"), so this is not a figure's business; if a conjunction whose
leading terms nearly always pass ever matters, the shared pass is the known
answer.

**Nothing folds or simplifies two terms.** Two terms on one column are
evaluated independently, which makes a contradictory pair a query with no rows
rather than an error — pinned in both suites. A simplifier would be the first
thing in this system resembling a planner, and there is nothing yet for it to
plan against.

**The unknown-column refusal is per term, in term order.**
`resolve_predicate_indices` is a `map(...).collect::<Result<Vec<_>>>()`, so the
first term naming a column the block does not carry raises
`Error::UnknownPredicateColumn` with *its* column. That keeps the one-place
rule "a predicate is validated against a block where the block resolves" —
`P5.6`'s ordering refusal is specified to fire at exactly this site, and it
gets a per-term loop already in place.

**The fingerprint hashes arity first, then each term in order.** Two
conjunctions differing only in term order are the same query and fingerprint
differently, so resuming across a reorder is `ResumeQueryMismatch`. The
alternative — canonicalizing the list before hashing — makes the stamp own an
ordering rule of its own, and the cost of not doing it is an error on a resume
nobody would write. The per-field explicit `match` on `PredicateOp` is kept, so
`P5.6`'s four new operators are a compile error here until they are hashed.

**The CLI parses every term up front and rejects nothing else.** `parse_filter`
is unchanged; the call site collects. A malformed term is refused before
`LocalFileSource::open`, and the column lookup stays the library's, so an
embedder and the CLI give one sentence for one fault — the same split `P5.4`
made for `--column`.

## What the next slices must not break

**`P5.6` adds the ordering operators.** Everything it needs is per-term
already: `PredicateOp` gains four variants (a compile error in
`query_fingerprint` and in `Predicate::matches` until handled), and the refusal
on a column that is not `Mapped` with a `Scalar` plan belongs beside
`resolve_predicate_indices`, which is the one site that already sees each term
and the block's resolved schema together. Note that function currently takes
`&SchemaRef` — the refusal needs `ResolvedSchema`'s `columns` and `plans`, so
it will take the whole `&ResolvedSchema` instead; both call sites have it in
hand (the replay loop's `full`, `resume_state`'s `full`).

**`P5.7` updates the manual.** The manual still says nothing about `--filter`
being repeatable — `docs/manual/type-handling.md` mentions the flag once, in
the singular, and that sentence stays true. The manual pass is that slice's,
along with projection's per-column decode escape, which is also still
undocumented for users.

**No inbox entry was filed.** The one fact P11 (typed predicates, which owns
`OR`/`NOT`) inherits here — that the filter is a flat ANDed list and admitting
`NOT` obliges a three-valued evaluator rather than one more operator — is
already written beside the mechanism, in `architecture.md`'s "Predicates"
rejected paragraph, which points at that inbox. Filing it again would put the
same sentence in two places, which is what makes an inbox stop being read.

## Staleness

This slice edits `pgdump_query/src/batch.rs`, `pgdump_query/src/stream.rs`,
`pgdump_query/src/predicate.rs` and `pgdump_query-cli/src/main.rs`. `--stale`
reads the **same eight** figures `P5.3` and `P5.4` left stale —
`census-brace-free`, `census-arrays`, `scan-throughput-cold`,
`scan-throughput-warm`, `nested-end-to-end`, `census-attribution`,
`cross-file-floor` and `map-only` — and no more. `predicate.rs` is declared by
no figure, and correctly so: no registered command shape passes `--filter`, so
nothing published times a predicate. Its new per-row entry point is reached
from `stream.rs`, which `SCAN` declares, so the call site is covered either
way.

None is acknowledgeable, for the reason `CLAUDE.md` states: a library change
has no cheap oracle, and this one replaces the per-row filter test with a loop
over a vector. They stay stale until `P5.7`'s sweep.
