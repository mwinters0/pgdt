# P12.3 — `interval`: the decoder and the resolution arm

What 12.4 inherits, and the calls the code does not explain itself.

The mechanism is filed by subject:
[`architecture.md`](architecture.md), "Type resolution" (the mapping, the two
value classes it loses, and why text was rejected) and "Decoders and
render-back" (the three sign-conditional formatting rules, and the one walk
with two consumers). This doc holds only what the next slice needs.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/decode.rs` | `interval_count`, `interval_time_micros` and `interval_parts` — moved here from `predicate.rs` — plus the `decode_interval`/`render_interval` pair |
| `pgdump_query/src/predicate.rs` | `interval_span` is now four lines over `decode::interval_parts` |
| `pgdump_query/src/pgtype.rs` | `builtin_scalar`'s arm: `(Interval(MonthDayNano), agrees(K::Interval))` |
| `pgdump_query/src/batch.rs` | `ColumnBuilder::IntervalMonthDayNano`, and `render_field`'s arm |
| `scripts/floor_mapping.py` | `ARROW_RENDERING` gained `Interval(MonthDayNano)`; the `waiting` `Disposition` is gone |

## The walk is shared; the fusing is not

D4 said the grammar walk is reusable and the fusing is not, and the split
landed exactly there. `interval_parts` returns `(months, days, micros)` in
PostgreSQL's own units — `i64`, `i64`, `i128`, all wider than any `Interval`
struct — and its two consumers narrow differently:

- `decode_interval` checks the three against Arrow's widths (`i32`, `i32`, and
  microseconds×1000 into an `i64`), which is where the two lost value classes
  are refused;
- `predicate.rs`'s `interval_span` fuses them into `interval_cmp_value`'s
  128-bit span, months at 30 days and days at 86400 s.

**The consequence 12.4 should not undo:** because the walk is one function, a
literal the filter refuses and a field the decoder refuses are the same set. A
decoder with its own parser would have drifted from the ordering's, silently,
and only a filter over a value one of them accepts would have shown it.

The i128 in the middle is the *span's* requirement, not the decoder's — it is
what lets `interval_time_micros` overflow-check a tail no `Interval` could hold
before either consumer narrows.

## Render-back is the plain inverse; the refusal is 12.4's

`render_interval(months, days, nanos)` divides `nanos` by 1000 and reproduces
`EncodeInterval`. It **assumes a whole number of microseconds**, which is all
`decode_interval` produces and all PostgreSQL can store; a sub-microsecond
value truncates. That is the value D4 says render-back must *refuse*, and the
refusal is 12.4's row rather than this one because it changes `render_field`'s
contract: `render_field` returns `Option<String>` where `None` already means
SQL NULL, and `pgdq query` prints `\N` for it (`main.rs`,
`unwrap_or_else(|| "\\N")`), so a refusal has nowhere to go without either a
panic or a signature change. That is a rework of a public, tested path, which
is the seam D11 drew between these two slices.

Nothing this build produces can reach it: the only writer of an
`Interval(MonthDayNano)` column here is `append_typed`, from `decode_interval`.
It is reachable only through the public `render_field` on an array an embedder
built.

## Three formatting rules, and why a fixture cannot check them

`fixtures/*/types/default.sql`'s `t_interval` holds four values, all of which
round-trip through the naïve rendering. Every rule that is actually hard is
sign-conditional and none of them fires there: the `s` on a unit whose count is
not exactly `1` (so `-1 mons`, and `1 mon` without), and the `+` on a part that
follows a negative one — including on the time tail, which is what makes
`-1 days +01:00:00`.

So `render_interval`'s test table is **a live `postgres:16`'s own answers**,
twenty spellings taken by hand (`CLAUDE.local.md`'s koji replica) rather than
read off `EncodeInterval`. It is committed as a literal table in
`decode.rs`, not as a fixture: no test depends on that server existing, and
the values are what `pg_dump` would have written for the same intervals.

*Rejected: adding the sign cases to `t_interval` and regenerating six majors.*
It is 12.5's shape for `int2vector` and would be right if the values were
otherwise unreachable, but these are pure function inputs — the fixture would
buy a slower version of a unit test, and would not reach the `i32` and
nanosecond ceilings at all, since no `pg_dump` can write a value past them.

## Two claims elsewhere that this slice made false, and what it did with them

- **The P10 inbox's `interval` entry is deleted.** It was filed while the type
  was `Utf8View`, and its fact — "`interval` is a `Utf8View` because two values
  have no encoding" — is what D4 reversed; its "why P10 cares" was that a
  census would decide the type per column, and there is no longer a type to
  decide. `architecture.md`'s "Type resolution" now carries the census as a
  rejected alternative instead, which is where the reasoning belongs once the
  question is closed rather than deferred.
- **`docs/manual/type-handling.md` is corrected here, which is where D9 puts
  it.** D9 makes the correction an obligation of the slice whose arm falsifies
  the claim, and that is this one: the manual stated the Arrow type outright —
  "an `interval` column arrives as the text the dump holds" — so it went false
  the moment the arm changed. What is corrected is the *type* and its two lost
  value classes; the register's own prose (`interval` is still one of the types
  whose ordering is a fused span) needed no change. D11's table said 12.4 and
  has since been amended to drop the clause, D9 owning it; the general rule is
  `process.md`'s "A falsified claim is corrected by the change that falsifies
  it".

## Measurement

`decode.rs`, `pgtype.rs` and `predicate.rs` are declared by no figure;
`batch.rs` is declared by `nested-end-to-end`, `cross-file-floor` and
`projection-widths`, all three of which were already red and none of which
moves. So `uv run measure.py --stale` is unchanged: all thirteen figures were
stale before this slice and are stale after it, for the reasons `STATUS.md`
already gives. `--check` still reconciles thirteen markers.
