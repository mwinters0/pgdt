# P27.2.1 — A float compared with a zero: notes

What the slices after this one inherit. The spec is
[`roadmap-P27-dynamic-filters.md`](roadmap-P27-dynamic-filters.md), "Scope".
The slice is earned: 27.2 shipped the translator under a contract that a
replay pruning by it would lose rows under.

## What exists

- **`dynamic_filter::loosened` keeps every row a filter's producer keeps by
  its own order**, as well as every row DataFusion's evaluation of the filter
  keeps, which is all 27.2 promised. The two part at a float's zeros, which
  DataFusion 55.1's TopK and ungrouped `MIN`/`MAX` order apart while the
  evaluation of the thresholds they publish reads them equal, as the library
  does (`comparison`'s rustdoc, with the upstream sources).
- **The rule: a float compared with a zero of either sign has no term**
  (`comparison`, `is_zero`) — every operator, either zero, `Float32` and
  `Float64`, the literal on either side — and stands as whatever keeps every
  row where it sits. Only the dynamic translator carries it: a static filter's
  answer *is* DataFusion's evaluation, which the library matches.
- **It is the widest rule, and that is what makes it sound whichever order a
  producer keeps its zeros in.** Against any other literal the two orders
  answer alike, `-0` and `0` lying on the same side of it, so with no
  comparison against a zero given a term, the translation keeps what either
  order keeps whatever shape the filter takes. It costs pruning only where a
  bound or a threshold is itself a zero.

## Negative results

- **A narrower rule is refused**: a term kept except where the zeros' order
  can change the answer — a strict `>` or `<` against the zero on the other
  side, `!=`, and `=` beneath a `NOT`. It keeps pruning at a zero threshold,
  but it is sound only as far as it has been followed through every shape a
  producer publishes, each chain a TopK builds included
  (`c1 = v1 AND c2 < v2`), where the widest rule needs no knowledge of shapes.
  As first worded it already missed two: `<= -0` and `>= 0` beneath a `NOT`
  each drop the zero on the other side, which the producer's order keeps.
- **No runtime-invariants entry**: an entry is a property a decision treats
  as guaranteed, and the rule treats nothing about a producer as guaranteed.
  Only whether it is *needed* depends on DataFusion 55.1's producers; whether
  it is *sound* depends on nothing outside it.
- **The generated check cannot catch a producer's unsoundness**: it holds
  each translation to DataFusion's evaluation of the filter, which equates the
  zeros as the library does, and so passes with the rule removed. Only a check
  against a producer's own answer can: the flags-on/off sweep,
  `datafusion-pgdump/tests/dynamic_filters.rs`, and the blind session below.

## Tests

- `a_float_compared_with_a_zero_has_no_term`, beside the generated check in
  `datafusion-pgdump/src/dynamic_filter/tests.rs`: `f > -0`, `NOT (f < 0)` and
  `-0 = g`, over a `Float64` and a `Float32` column, have no term under either
  parity, where the same comparisons with `1` or `-0.5` have one. The
  generated check marks a float leaf against a zero inexact, its translation
  keeping rows DataFusion's evaluation does not.
- **What exposes the loss is a replay pruning by the filter, checked against
  a producer's own answer**: with 27.3's pruning and without the rule,
  `datafusion-pgdump/tests/statistics.rs`'s blind session, the one without
  `aggregate_statistics`, answers `MAX(max_neg_first)` over `zeros` as `-0`
  for `0`, failing `an_extreme_answers_where_its_stored_bound_is_the_value`
  and `statistics_never_change_an_answer`.

## What would retire the rule

A DataFusion release whose producers' thresholds hold with the zeros made
equal, under which the rule costs pruning at a zero and saves nothing
(`comparison`'s rustdoc). Retiring it there rests the translator's soundness on
that release's producers, so it owes the runtime-invariants entry this slice
does without, and a check against a producer's own answer to show the release
is one, the generated check being unable to.
