# P27.8 — The per-row account of the costing input: notes

What the round after this one inherits. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Slices",
item 8. **The account sums to the costing row's Δ, and its numbers are
[`measurements.md`](measurements.md), "What DataFusion's dynamic filters buy
a query"**, taken at `e653505`, the commit after the one landing the
instruments. What each shape of removing duplicate decoding saves is there;
the mechanisms beyond it are named below, and the choice among them is under
STATUS's "Decisions worth another look".

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
  sorted stop once a row (`DynamicRead::stop`), and — missed here, named by
  the profile — `DynamicRead::at_row` asking every row which group it is in,
  where only a row entering one needs the answer.
- **Per chunk**: `Chunk`, which asks DataFusion's filters their generation
  (`ReplayFilter::generation`) and, the join's having moved once, translates
  and resolves the state once per block.
- **Per query**: the join's filter built from the build side and translated
  once (`ReplayFilter::current` keeps it until its generation moves); and —
  missed here, named by the profile — **the byte cut at the first poll**
  (`TablePartitions::cut_under`), asking the state's truths of every
  group, where a membership's truths fold each term's, and each term keys
  the group's bounds afresh (`ResolvedTerm::bounded`): groups × list length
  key parses, paid whether or not a row is then evaluated.

So **Δ = rows × (Row + outside) + chunks × Chunk + fixed**, and the code
predicts no size for any term: the one reading bearing on a leaf is
`predicate-terms`' per-term steps, of a text `=` against a shared split,
which is not this leaf's comparison. **Duplicate decoding is two `Unescape`s
and one `Key` a row**; the profile prices each, the introspection build's
calibrated spans under-reading both (below).

**What each shape of removing it would save**, in spans a row on this filter,
priced in [`measurements.md`](measurements.md)'s account:

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
- **The introspection build's calibrated parts are not the account.** Its
  calibration, taken back to back, subtracts more from `Row` than the whole
  build costs over the release one, so its short spans read near zero; its
  counts are exact and match the code's account, and the per-part split is
  the profile's. A reading that needs a leaf's own time reads a profile, or
  calibrates against a span among real work.
- **The split's walk is moved, not added**: what `push_row` loses of it the
  first `Locate` takes, to within the profile's sampling, so row evaluation's
  cost is net of it and the `Locate` span's size says nothing of what
  evaluation adds.
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

## The mechanisms beyond duplicate decoding

What the account leaves once duplicate decoding goes, each named where it
lives; none is chosen here ("Decisions worth another look").

- **The lookup**: a membership's `Lookup::Canonical` hashes the field's text with
  std's SipHash and compares the string on the hit, an integer's `=`
  comparing the spelling. The hash is about half of it; a keyed lookup would
  add back a `Key` the bounds' removal takes away.
- **The byte cut's keying**: a membership's group truths key each group's
  bounds once per term (above). Keyed once per group, or answered by a
  search of the sorted list within the bounds, it stops scaling with the
  list; it is a cost of the cut, so it is paid with row evaluation off.
- **The walk**: the `And` over three children and each leaf's `Result`, most
  of which the bounds' removal takes with them.

The spec's criterion needs about two thirds of the Δ gone at the figure's
spreads, were they to stay where they are; dropping the implied bounds falls
short of that even at its upper bound, so by this account 27.9 meets it only
with a second mechanism beside it.

## The readings

```sh
cd scripts && uv run measure.py --profile-recipe   # steps 0-4 and 7-9 are this slice's
```

Taken at `e653505`, with a second profile pair and two `perf stat` sittings
beside the recipe's steps; every artifact is named in
[`measurements.md`](measurements.md)'s account. Neither falsification this
slice set came true: the instrument's `Row` net of the moved split falls
short of Δ, and the profile names what fills it outside evaluation — the
cut, the per-row reads — with the calibration's over-subtraction bounded by
the builds' own difference; the two instruments differ on the split among
leaves exactly where the calibration errs, and agree on their ranking.
