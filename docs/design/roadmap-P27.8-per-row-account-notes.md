# P27.8 — The per-row account of the costing input: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 8. **The instruments have landed and the code's own account is below;
the readings are not taken**, a reading being taken from the commit that
lands its instrument ([`measurements.md`](measurements.md), "A figure may be
published outside the sweep", whose rule the `go` skill applies to an
instrument's readings too). What remains is taking them, and the account and
the mechanism they name.

## What exists

- **`pgdump_query::instrument`'s evaluation account**, under
  `datafusion-cli-pgdump`'s new `introspect` feature: each row a dynamic
  filter's state is evaluated on is timed whole (`EvaluationPart::Row`,
  around `DynamicRead::rejects`), and inside it each leaf's `Locate`,
  `Unescape`, `Key`, a term's `Compare` and a membership's `Lookup`; each
  chunk's read of the state is `Chunk`. The tree walk is the row less its
  leaf parts. The report is `evaluation_*` lines in the file
  `PGDT_INTROSPECT_OUT` names, raw ticks beside calibrated nanoseconds; what
  the counter can and cannot see is [`measurements.md`](measurements.md),
  "What an instrument can see, and what only a sitting can".
- **`measure.py --profile-recipe` prints the costing row's pair**:
  `perf` profiles of both legs off the `profiling` build, then three runs of
  each leg on the introspection build, each leg's environment and arguments
  `dfcli_invocation`'s, the function `_script` times the figure through.

## The code's own account

What the on leg runs that the off leg does not, by what it scales with
(`evidence` skill, rule 1). The costing filter reaches a row as
`bucket >= lo AND bucket <= hi AND bucket IN (…150 values)`; neither bound
rules a row out, so every row reaches all three leaves, and no row is
rejected, so nothing downstream of the evaluation changes.

- **Per row, inside `Row`**: the state's lookup; the `And`'s walk over three
  children; three `Locate`s, the first extending the row's shared split to
  `bucket`, the last column — a walk the off leg's `push_row` makes anyway
  (`RowSplit::complete`), so moved rather than added — and two reading it
  back; **three `Unescape`s of one field**; **two `Key`s**, each bound parsing
  the same text as an `i64`; two `Compare`s; and **one `Lookup` that keys
  nothing**: an `integer`'s `=` compares the text the file spells, so the
  membership probes a `HashSet<String>` of the literals, hashing the field's
  text once a row (`tests/evaluation_instrument.rs` pins these counts).
- **Per row, outside `Row`**: the row loop looking up the dynamic filter's
  sorted stop once a row (`DynamicRead::stop`), which only the profile sees.
- **Per chunk**: `Chunk`, which asks DataFusion's filters their generation
  (`ReplayFilter::generation`) and, the join's having moved once, translates
  and resolves the state once per block.
- **Per query**: the join's filter built from the build side and translated
  once (`ReplayFilter::current` keeps it until its generation moves);
  nothing that scales with the probe.

So **Δ = rows × (Row + outside) + chunks × Chunk + fixed**, and the code
predicts no size for any term: the one reading bearing on a leaf is
`predicate-terms`' per-term steps, of a text `=` against a shared split,
which is not this leaf's comparison. **Duplicate decoding is two `Unescape`s
and one `Key` a row**; the reading prices each span, so it prices them.

**What each shape of removing it would save**, in spans a row on this filter,
priced by the reading's own per-span nanoseconds:

- **A field decoded once a row for every leaf reading it**: two `Unescape`s
  and one `Key`. General: it also serves a TopK's chain, which reads a column
  at every arm, and `predicate-terms`' repeated terms.
- **The bounds an `IN` implies dropped from row evaluation**, kept for group
  pruning: two `Locate`s, two `Unescape`s, both `Key`s, both `Compare`s and
  two children of the walk — strictly more here, and only where an `IN` and
  bounds over one column meet, which is a join's filter under
  `hash_join_inlist_pushdown_max_distinct_values`.

## Negative results

- **`Instant` as the timer** is refused where `rdtsc` exists: it is a call
  through the vDSO around a read of the same counter. `rdtscp` or a fence is
  refused too: serializing each read prices a span as if the pipeline held
  nothing else, which overstates every short one.
- **The static filter is not timed**: it is the empty conjunction in both
  legs, so it is in neither's Δ, and timing it would put statistics' pruning
  evaluations, which call the same leaves, into the leaf parts.
- **An `#[inline(always)]` function taking the span's body as a closure** is
  refused: in a release `pgdt` built without the feature it changed the code
  of every function it wrapped, where `timed!` and `row_evaluated!`, which
  expand to the body alone, leave every function of `pgdt` the size it was at
  `be2af8f` (`nm -S`, both built in this round). The one edit that still moved
  one was a key's drop moving ahead of its comparison's result, now back where
  its temporary was.
- **Per-thread accumulators** are refused: the figure runs one partition, so
  a relaxed atomic is uncontended, and the calibration pays what a span pays.

## Tests

- `pgdump_query/tests/evaluation_instrument.rs`, run with
  `--features introspect`: a join-shaped state over `ordered` times one row
  a row, three `Locate`s and `Unescape`s, two `Key`s and `Compare`s and one
  `Lookup`; the same tree as a static filter is timed nowhere; the reading's
  derived times are finite. Mutation: leaf parts timed outside a row fails it.
- `scripts/test_measure.py`, `ProfileRecipe`: the pair and the introspected
  runs state what `_script` times, read out of its line, over the figure's
  own cache; `the_variable_matches_the_binarys` holds the report's variable
  to `datafusion-cli-pgdump`'s.

## The readings

```sh
cd scripts && uv run measure.py --profile-recipe   # prints; steps 0-4 and 7-9 are this slice's
```

Minutes, not a detached job. **What would falsify the account**: `Row` ×
rows plus the chunks' time falling short of Δ by more than the legs'
spreads, with the profiles' difference naming nothing outside evaluation
that fills it; or the two instruments disagreeing on evaluation's share,
since they differ exactly where the error could live — an unordered counter
smearing short spans against a sampler that attributes by the instruction.
Each part then goes where "Where does this fact go?" sends it, the numbers
into [`measurements.md`](measurements.md), and the mechanism beyond duplicate
decoding, if the account names one, to STATUS's "Decisions worth another
look" unless the record and the account settle it.
