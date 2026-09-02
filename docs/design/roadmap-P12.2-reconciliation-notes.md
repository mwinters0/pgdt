# P12.2 — The floor-to-mapping reconciliation

What 12.3–12.6 inherit, and the calls the code does not explain itself.

The mechanism is filed by subject:
[`architecture.md`](architecture.md), "The floor: the ADBC driver's answer
bounds ours" (the rule and its five stances) and "The ADBC floor oracle" (the
evidence). This doc holds only what the next slices need.

## Module map

| File | What it is |
|---|---|
| `scripts/floor_mapping.py` | the join, `ARROW_RENDERING`, `DISPOSITIONS`, `reconcile` |
| `scripts/test_floor_mapping.py` | the parse and the verdict on synthetic sources; the whole join on the committed tree |
| `scripts/generate_fixtures.py` | a floor pass now ends by running `floor_mapping.check()` |
| `docs/design/architecture.md` | the rule, the stance table, and `KD13` |

## What a closing slice has to do here, in one sentence each

**12.3 and 12.6 each delete a `Disposition`.** `interval` and `int2vector` are
`waiting` rows naming 12.3 and 12.6, and the check reports a row that has
started meeting the floor while a stance still stands over it — so the slice
that maps the type fails `floor_mapping.py` until its four-line entry in
`DISPOSITIONS` goes. That is the mechanism D10 asked for, and it is why neither
type got a `KD<k>`.

**The Arrow type has to render.** `ARROW_RENDERING` maps each `DataType`
expression `builtin_scalar` writes onto the pyarrow spelling `floor.tsv` holds,
and an expression it does not carry is a *problem*, not a guess — so 12.3 adds
`Interval(MonthDayNano)` → `month_day_nano_interval` and 12.6 adds
`List(…)` → `list<item: int16>` in the same change as the arm. The rendering
is over the **value space**, not the layout: `Utf8View` renders `string`
because a floor is a claim about which values a column can hold.

Watch the `List` case: pyarrow writes `list<item: int16>`, with the field name
in it. `builtin_scalar` does not build the `List` arm's field, so 12.6 will
have to render whatever expression it writes — most likely a fixed string
keyed on the whole expression, as every other entry is.

## Why the arm's `DataType` is parsed rather than emitted

`oracle_register.py`'s argument, unchanged: a `match` is not data. The parse
takes the first element of each arm's `(DataType, ComparisonPlan)` tuple,
stopping at the top-level comma so a type carrying its own commas and parens
survives. `numeric` is the one arm that opens no tuple — it delegates to
`map_numeric(typmod)`, whose answer depends on the typmod — and that is
recorded as *unreadable* rather than as a parse failure: it is reported only
where the floor actually needs our type, which for `numeric` it does not, the
driver answering `arrow.opaque`. A driver release that gave `numeric` a real
type would fail loudly on an unread mapping instead of passing.

*Rejected:* a Rust test emitting the mapping to a committed file the Python
reads. It removes the parse and adds a third artifact to keep in step, and the
parse's failure mode is already covered — every anchor has a test that removes
it, and a `DataType` the table cannot render is reported.

## Five stances, and why `oid` is one of them

The spec's D2 names three things that put a row outside the rule; the check
found a fourth shape that is *inside* it. `oid` answers `UInt32` where the
driver answers `Int32`: not equal, and not a widening either. Since "not equal"
is all the check can compute on its own, the row carries a `narrower`
disposition saying why the difference is permitted — the argument
`builtin_scalar`'s `oid` comment already made, now in a place something reads.

`BELOW_STANCES` is what keeps the four kinds apart mechanically: a
`below-by-decision`, `different-encodings` or `waiting` row must be one we model
*no* Arrow type for, and a `narrower` row must be one where both sides are real
types. A mapping that gave `money` an `Int64` without dropping its stance fails
on that rather than passing.

## Both citations are resolved, not trusted

A `waiting` disposition names a slice and `money`'s names `KD13`, and both are
pointers into `STATUS.md` that go stale silently — a re-slice renumbers the
first, a strike deletes the second. `_citation_problems` resolves them through
`deficiencies.py`'s own `parse_checklists` and `parse_index`, which is that
module's argument for the slice pairing reused against the same file. So a
re-slice of P12 fails this check as well as `deficiencies.py`.

Consequence at the wrap: P12's checklist is deleted when the phase completes,
so a `waiting` disposition surviving the wrap fails here. That is correct — a
phase that wrapped with `interval` still unmapped has a stance pointing at work
that no longer exists.

## What the opaque tail costs: nothing

58 of the 82 rows a major are placed by the file's own `status` and `extension`
columns, with no per-type line. That is the split the spec's D7 bought — the
work is bounded by what the rule reaches (24 rows, 19 of them met) while the
coverage is the whole catalog — and `test_the_opaque_tail_carries_no_hand_written_line`
asserts it, so a stance quietly written over an opaque row is a failure rather
than dead text.

## Two things this slice deliberately does not check

**The extension metadata.** Release 24 stamps `arrow.json` on `json`/`jsonb`
and nothing else; we stamp those two plus `arrow.uuid`. The metadata floor is
therefore met everywhere by construction, and a join on it would check
something that cannot fail. `extension` is read here only for `arrow.opaque`,
which is a statement about the *type*. This changes at release 25, whose
unreleased `POSTGRESQL:type` commit would make it a real obligation.

**Built-in ranges.** `builtin_range_subtype`'s twelve names are mapped
elsewhere and every one of their floor rows is `arrow.opaque`, so joining them
would add twelve "floor undefined" lines and answer nothing. The join is
`builtin_scalar` only, which is what D6 says.

## Measurement

No Rust source changed, and the only `scripts/` paths any figure declares are
`generate_perf_data.py`, the three `generate_*_bench.py` generators and
`measure.py` — none of which this slice touches. So `uv run measure.py --stale`
is unchanged: all thirteen figures were already red and none moved. `--check`
still reconciles thirteen markers.
