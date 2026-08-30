# P5.8 — Special values are ordered: notes

What the next slice inherits from making `infinity`, `-infinity` and `NaN`
answer an ordering operator instead of ending the query. The spec is
[`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md), "PostgreSQL's special
values are ordered, not undecodable"; the mechanism lives in
[`architecture.md`](architecture.md), "Ordering operators compare typed"; what
has landed is [`../status/STATUS.md`](../status/STATUS.md).

## What is there now

One function of new behaviour, in `predicate.rs`, reached from the single site
both sides of a comparison already go through:

- **`special_order_key(kind, text)`** answers the closed set, keyed by
  `OrderKind` so the spelling belongs to the type: `Date` and
  `Timestamp { .. }` take `infinity`/`-infinity`, `Decimal` takes `NaN`,
  everything else takes nothing. `order_key` calls it first and falls through
  to the decoders unchanged, so the field side and the literal side gained the
  behaviour together and neither `decode.rs` nor the build path moved.
- **`OrderKey` has three non-finite variants** — `NegativeInfinity`,
  `PositiveInfinity`, `NotANumber` — and an `OrderKey::rank`.
  `compare_keys` compares ranks first and only descends to the value pairs
  when both sides are `FINITE`, which is also what keeps the `unreachable!`
  arm honest: a special against a finite is now a legitimate cross-variant
  pair, and it never reaches the match.

Register entry **I34** is new and carries the whole external argument: the two
spellings and the `numeric` ones as their `*_out` writes them, the order as
each type's own representation or `cmp_numerics` defines it, the typmod rule,
and the live re-confirmation against the koji replica (16.15). The ordering
register's `Date32`, `Timestamp` and `Decimal` rows now cite it — they read
*Agrees* before this slice and the claim was false wherever the server
answered and this project errored.

The spec anticipated this evidence under I33 and cites it there; it landed as
**I34**, because I33 is scoped to the operators the register claims *agreement*
with and this is a different claim about a different set of values. The spec's
citation is left as written — a spec records intent, not the number the
evidence ended up with.

Tests: `predicate.rs`'s unit module goes 20 → 23, `pgdump_query/tests/
ordering.rs` 11 → 13, and one CLI test is added. `deficiencies.py` and
`measure.py --check` both still pass.

## Calls made here, and why

**The specials are carried as positions in the order, not as sentinel
numbers.** The spec proposed a sentinel, on the evidence that `OrderKey::Int`
is an `i64` while `Date32` is an `i32` — true, and it does not extend to
`Timestamp`, whose key is already the full `i64`: `i64::MAX` micros since 1970
is 294247-01-10, a timestamp PostgreSQL itself accepts (its own ceiling is
294276 AD), so `i64::MAX` as `infinity` would tie against a real value. A rank
costs one comparison, needs no argument about reachable ranges, and says in
the type what the spec says in prose — that these are a separate population.
Widening the key to `i128` was the other way to keep the sentinel literally
free; it buys nothing the rank does not and puts a wider integer on the
per-row path.

**A `numeric` infinity is not implemented, and that is a fact rather than a
deferral.** `apply_typmod_special` rejects `Infinity` under any typmod, and a
`numeric` without one is held as `Utf8View` — so no column that reaches
`OrderKind::Decimal` can hold an infinity, and code for it would be
unreachable. This is the sharpest thing I34 records; without it the obvious
reading is that the `Decimal` case was left half-done. The bare-`numeric`
column *can* hold all three, and that is filed in P11's inbox, since P11 is
where that column stops being compared as text.

**Only the spelling that type's `*_out` writes is special.** A `date` field or
literal reading `Infinity` is still a decode failure, and a `text` column
holding the word `infinity` still compares bytewise below `zzz`. That is the
strictness the nested codec already applies, and it is what keeps the change
from reaching a column whose values merely look like these.

**`real`/`double precision` were not touched.** IEEE represents all three, so
their decoder already returns them and `pg_float_cmp` already implements
PostgreSQL's NaN rule. Routing them through the new variants instead would
have been a rework of a path `P5.6` had just tested, for no answer that
differs.

**The `FieldDecode` integration test moved to a value that is still
undecodable.** It used `t_date`'s `infinity`, which now answers. Its
replacement is `t_timestamp`'s `294276-12-31 23:59:59.999999` — PostgreSQL's
own documented maximum, which overflows `i64` micros counted from the Unix
epoch. That is a third population, and worth naming: not a special value with
an order, and not malformed text, but a finite value this mapping cannot
represent. It stays `Error::FieldDecode`, as `architecture.md` has always said
it should.

**Two commits were acknowledged in `scripts/acknowledged.py`.** `881379e` and
`b32ed24` — the deficiency-register application and its rename — changed
`map.rs`, `batch.rs`, `pgtype.rs` and `stream.rs`, all of them declared paths,
and every changed line in them is a Rust comment. `--stale` read nine figures
against `STATUS.md`'s eight because of it. Each entry carries a `verified`
command that prints nothing, which is the mechanical evidence the register's
weaker entries do not have. Both are spent by `P5.7`'s re-stamp and should be
deleted then, along with the other three.

## What the next slice must not break

**`P5.7` takes the figure and updates the manual, and this slice adds a fourth
thing owed there.** Beside repeatable `--filter`, projection's per-column
decode escape and the ordering operators themselves: a filter answers
`infinity`/`-infinity`/`NaN` exactly, *and* the column holding one still fails
to build if it is projected. `docs/manual/type-handling.md` currently says
`numeric(10,2)` maps to `Decimal128(10,2)` and that "the mapping is exact",
which is where a reader will look for the `NaN` a typmod does not exclude.

**Staleness is unchanged by this slice.** It edits `predicate.rs`, which no
figure declares, plus tests and documents. The eight figures `P5.3`–`P5.6`
left stale are the same eight, for the same paths, and none is acknowledgeable
— a library change has no cheap oracle. The acknowledgement above removed the
ninth, which was never real.

**The rank, not the variant order, is the comparison.** `OrderKey` derives no
`Ord` and must not: two variants of it are only comparable through
`compare_keys`, and a derived lexicographic order over the variant list would
happen to agree today and stop agreeing the moment a variant is inserted.
