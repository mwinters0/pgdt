# P11.3 — The comparison plan moves to L2

What the next slices inherit. How the register *works* now is
[`architecture.md`](architecture.md), "Ordering operators compare typed"; this
doc is the part that is not description — the calls that are not obvious from
the code, and what the move turned out to cost.

## What landed

- `pgtype.rs` gains the register: `CompareKind`, `OrderingDivergence` (moved
  down from `predicate.rs`), `ComparisonPlan`, and `comparison_for(declared,
  types)`.
- `builtin_scalar` — the built-in scalar mapping table, now answering the Arrow
  type *and* the comparison in one arm. `map_builtin` is what is left: that
  table plus the twelve built-in range names.
- `ResolvedSchema` gains `comparisons: Vec<ComparisonPlan>`, filled by
  `resolve_columns` and cut by `stream::project` with the other four vectors.
- `predicate.rs` reads it. `ordering_register`, `OrderingSupport` and
  `OrderKind` are gone; nothing in that file names a `DataType` any more
  outside its tests.
- `lib.rs` re-exports `CompareKind`, `ComparisonPlan`, `OrderingDivergence` and
  `comparison_for` from `pgtype`; `OrderingDivergence` no longer comes from
  `predicate`.

**No answer changes**, which was the slice's review property. Every ordering
test in the tree still passes with its assertions untouched —
`predicate.rs`'s 23 unit tests (only their `one_column` helper was rewired),
`tests/ordering.rs`'s 13 and the CLI's 12 in `query_ordering.rs` — and the only
user-visible string that moved is on a path nothing reaches (below). The six
new tests are all in `pgtype.rs` and `resolve.rs`, over the register itself.

## Why the register is not still an exhaustive `match` over `DataType`

The old check made a *new Arrow type* a compile error. It could not make a new
**declared** type one, and that is the direction the phase moves in: the enum
row needs `TypeKind::Enum`'s labels and the bare-`numeric` row needs to be told
apart from `text`, neither of which the Arrow type carries.

What replaces it is stronger rather than weaker, and it is worth knowing why
before 11.4 or 11.5 adds a type:

- **`builtin_scalar` answers both questions in one arm.** A built-in added to
  the mapping table without a comparison does not compile. The old pairing let
  a new mapping land on a `DataType` arm that already existed — a silent
  classification, which is exactly what the exhaustive match was supposed to
  prevent and could not.
- **`comparison_user_type` is exhaustive over `TypeKind` with no wildcard**, so
  a kind added to the preamble grammar has to choose.
- **`ComparisonPlan` is one enum, not a `(support, kind)` pair.** "No order
  here" and "no way to decode a value" were two facts that could disagree; they
  are now one variant.

The one arm that is *not* covered mechanically is the built-in **range** table
(`builtin_range_subtype`): a name added there is nested by construction and
refused by the array/plan check above it, so there is nothing for it to say.

## The three refusals are still decided in that order

`resolve_term` reads `columns[i]`, then `plans[i]`, then `comparisons[i]`, and
the order is what keeps each refusal's message true: a nested column is refused
with `NESTED` — naming its nesting — rather than with `NO_ORDER`, even though
the register also answers `Refused` for it. Two agreeing answers, and the
sharper one wins because it is asked first.

`NO_ORDER`'s text changed from "the column's resolved Arrow type" to "the
column's declared type", which is the slice's only user-visible string move.
It is unreachable: everything `pgtype.rs` maps to a `Mapped` scalar has a
comparison in the same arm. The test that pins it asserts the constant, not the
sentence.

## Calls worth knowing about

**The census can take a column out of `Mapped` after the declared type has been
read**, so `resolve_columns` re-answers the plan against the outcome that
survived rather than against what the DDL alone gave. A `VaryingArrayShape`
column holds *array* literals in a `Utf8View`, and comparing those bytewise is
not an order this build defines — so it is `Refused`, not `AS_TEXT`. Getting
this wrong would have been invisible: the column is refused a step earlier on
`ColumnResolution` anyway, and only an embedder reading `comparisons` directly
would have seen the lie.

**`predicate.rs`'s test helper derives the plan from the register** rather than
stating one, so those tests exercise the same `declared -> plan` walk
`resolve_columns` makes. That is why `one_column` now takes a type list with an
enum in it: `public.mood` has to reach the enum arm rather than the "no such
type" one.

**A domain has no row of its own.** `comparison_for` recurses through
`TypeKind::Domain` exactly as `resolve_declared_type` does, so a domain
compares as whatever it bottoms out at — and a domain over `money` or over
`integer[]` is refused, because what it bottoms out at is.

## What later slices inherit

**11.2.1's reconciliation joins on the declared type, which is what the
register is now keyed on.** The arms it must enumerate are `builtin_scalar`'s
(one per declared base name, several sharing an arm) plus the `TypeKind` arms
of `comparison_user_type`; the oracle's `TypeCases.type` is a declared type
string spelled the same way. `the_register_answers_every_builtin_scalar` in
`pgtype.rs` is the hand-written list of the first group and is the thing to
keep in step — it is an ordinary Rust table, deliberately not a parser over
`architecture.md`'s Markdown.

**11.4 and 11.5 each add arms, not a mechanism.** A type closes by changing its
`builtin_scalar` arm from `(dt, ComparisonPlan::AS_TEXT)` to a `Compared` with
its own `CompareKind`, adding that variant's `order_key` arm in `predicate.rs`,
and adding a row to the Rust register test. The enum row (11.4) is the one that
needs more: its labels live on `TypeKind::Enum`, which `comparison_user_type`
already has in hand, so the closure is a `CompareKind` carrying the label
vector rather than a new lookup.

**11.11 will need a per-*column* input, which this vector already is.** The
register is a function of the declared type today, but `comparisons` is filled
per column, so a `COLLATE` clause read off that column's DDL has somewhere to
land without moving the vector.

**`ComparisonPlan` is `Copy` and `predicate.rs` reads it by value.** 11.4's
enum labels break that — a label list is not `Copy` — so `resolve_term` will
have to borrow instead. It is a one-line change and it is flagged here because
`stream::project`'s cut copies rather than clones today.

## What was left out, and why

**No CLI surfacing.** Nothing prints a column's comparison plan; `pgdq info
--json` still exports `plans` and not `comparisons`. The slice's contract is
"no answer changes", and adding an output field is an answer.

**No acknowledgement entry for the figures this made stale**, because an entry
in `scripts/acknowledged.py` is keyed on a commit sha and this change has none
yet. Eight figures read stale and the reason is in
[`STATUS.md`](../status/STATUS.md); what is available, once the change is
committed, is a **reachability** acknowledgement for the four `parse`-shaped
figures — `census-brace-free`, `census-arrays`, `scan-throughput-cold`,
`scan-throughput-warm`. `parse` never calls `resolve_columns` or
`stream::project`: `pgdq parse` reaches `map_file` → `map_forward`, and every
production call of `resolve_block` and `project` is in `table_stream` or in
`resume_state`, which only `table_stream` calls. So a `pgdq parse` run
executes none of the changed code. What re-checks the claim is
`rg -n 'resolve_block\(|project\(' pgdump_query/src/stream.rs` against the
function list, and `measure.py`'s `_script`, where every `parse`-shaped
command is a `pgdq parse`.

The three query-shaped figures — `nested-end-to-end`, `cross-file-floor` and
`projection-widths` — are **not** excusable. They execute one `comparison_for`
per column per block, and `projection-widths` the extra `Vec` collect in
`project` as well; both are per block against a per-row figure, but "small" is
not evidence and only a sweep settles it.

The first two are the pair to be careful with. Their *declared*-path change is
the `#[cfg(test)]`-only one in `batch.rs`, so a reachability entry would be
technically true and substantively misleading: what could actually move them is
in `resolve.rs`, which no figure declares. That is the register's documented
false negative arriving from the side that tempts an over-broad
acknowledgement, and the answer is to leave them red.
