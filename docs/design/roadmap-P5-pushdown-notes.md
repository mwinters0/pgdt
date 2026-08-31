# P5 — Pushdown: notes

What the phase leaves behind that subject-filing has no home for. How the
mechanisms work is [`architecture.md`](architecture.md) — "Projection",
"Predicates", "Ordering operators compare typed", "A filter term is parsed for
two audiences", "Three flush triggers" and "Resume"; what the figure says is
[`measurements.md`](measurements.md), "What a column costs"; what exists is
[`../status/STATUS.md`](../status/STATUS.md).

This is an audit rather than a transcription. Every mechanism the nine slice
notes described already had a subject section, so the wrap moved in what they
held and it did not say: that `max_bytes` counts only projected fields, when
each of projection's two refusals fires, that `OrderKey` derives no `Ord` and
must not, that a refused type carries no `OrderKind`, that the projection-width
test reads its flags out of the harness, and — in `measurements.md` — that a
declared path is matched by prefix, so `QUERY_CLI` naming one file makes
splitting the CLI a change that has to move the declaration with it.

## The slice numbers are not the landing order, twice over

`P5.8` ran before `P5.7`. It was discovered when `P5.6`'s ordering register was
reviewed, and the sweep had to measure the finished library, so the number is
allocation order and the position is the spec's slice table.

`P5.9` was earned rather than planned. `P5.5` and `P5.6` had shipped a term
grammar nobody had written down: the value side kept its leading whitespace,
deliberately, on the reasoning that a value may legitimately begin with a space
and nothing else could restore one. Real use of the CLI found the other half of
that trade first — `--filter 'v_date < 2020-01-01'` looks for the date
` 2020-01-01` — and quoting is what restores a leading space once trimming is
the rule. It is a slice and not an out-of-band item because it changed a
documented contract of a mechanism those two slices built, while the phase was
open.

## Negative results

The category that is recoverable from nothing. Each is filed beside its
mechanism as well; this is the list, so a later session knows to look.

**An instrument was built, held out of every sweep, and deleted unpublished.**
`composite-isolated` declared one column two ways over byte-identical rows, to
resolve a composite column's cost that a cross-file subtraction could not: the
subtraction's own floor is ±0.5 µs/row and the quantity under it turned out to
be 0.77. Projection made the same isolation available with no second file, so
the entry went and its whole apparatus went with it — the `--weak-composite`
generator flag, the `composite_text` input, and the fidelity case pairing the
two files. Keeping generator support behind a deleted register entry is the one
outcome that is wrong either way, since `measure.UNTAKEN` exists so that a
built-and-unrun instrument is named rather than latent.

**A refusal shipped and was reversed inside the same phase.** `P5.6` refused
`infinity`/`-infinity`/`NaN` under an ordering operator as `Error::FieldDecode`,
on the reading that a value Arrow cannot hold is a value this system cannot
answer about. The escape that refusal named does not exist — `--schema-mode
strings` resolves no column, so it refuses ordering outright — and deciding an
order needs strictly less than materializing a value. `P5.8` reversed it. The
general form is worth carrying: *the file contradicting its DDL* and *Arrow
having no representation* had been merged into one error, and separating them
was the whole fix.

**A sentinel was the obvious representation and does not work.** `OrderKey::Int`
is an `i64` where `Date32` is an `i32`, so both ends look free — but
`Timestamp`'s key is already the full `i64`, and `i64::MAX` micros from 1970 is
294247-01-10, a timestamp PostgreSQL accepts. Widening to `i128` keeps the
sentinel free and puts a wider integer on the per-row path for nothing. A rank
compared before the value costs one comparison and needs no argument about
reachable ranges.

**The ordering register's spec table was one row short, and the code found it
rather than a reader.** `interval`, `time with time zone`, `json`/`jsonb` and
the four network types all reach `Utf8View` with a `Scalar` plan, so they reach
an ordering operator and compare bytewise, and each has a server-side operator
that bytewise is not. It surfaced from splitting the classification (keyed by
Arrow type, because the exhaustive `match` is what keeps the register honest)
from the message (keyed by the declared type, because four unrelated situations
reach `Utf8View`).

**A latent parser bug was found by widening, not by failing.** The pre-`P5.6`
two-operator parser tried operators in turn, so `name=alpha>x` would have
parsed as a filter on a column called `name=alpha` the moment a second operator
family existed. Nothing had hit it. The rule is now earliest-position,
longest-spelling, and quote-aware.

**Matching an operator through `str` is a panic, not a style choice.**
`spec[i..].starts_with(symbol)` panics on the interior byte of a multi-byte
character, and trimming is Unicode's — so a non-breaking space pasted out of a
web page reaches it. The pre-`P5.9` parser walked `char_indices` and was safe by
accident; the byte loop is safe on purpose.

## What the phase inherits forward

Nothing new was filed at the wrap: each fact went to its destination as the
slice found it.

- **P11 (typed predicates)** holds the `OR`/`NOT` deferral, the nested-column
  comparison deferral, and the bare-`numeric` column — the one column that can
  hold all three special values and still compares as text
  ([`roadmap-P11-typed-predicates-inbox.md`](roadmap-P11-typed-predicates-inbox.md)).
  `KD7`'s four divergent rows are that phase's per-type worklist.
- **P6 (embeddable engine)** holds what an embedder should be handed in place
  of `ordering_notes`, whose sink is the designated unification point for
  diagnostic channels
  ([`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md)).
  The register's third channel was predicted there before this phase built it,
  and nothing needed adding.
- **P7 (scan performance)** is unchanged by this phase. Pushdown deletes no
  parser code, which is why the ordering argument that once rested on it was
  withdrawn.

## What the wrap owes and has not paid

**Eleven figures read stale and the sweep that clears them has not been folded
in.** Nine declare `scripts/generate_perf_data.py`, which `P5.7` edited to
delete `composite-isolated`'s apparatus; `session-drift` declares
`scripts/measure.py`; `map-only` is `P5.9`'s, through `QUERY_CLI`. None is
acknowledgeable — `--verify-additive` settles generator changes only, and a
library or CLI change has no cheap oracle — so the whole range is spent by a
sweep that re-stamps every table at once. It is launched detached at the wrap
commit and read by a later session; `runs/`'s handoff names the log.
