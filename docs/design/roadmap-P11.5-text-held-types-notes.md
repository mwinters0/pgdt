# P11.5 — The text-held type queue

What the next slices inherit. How the register *works* is
[`architecture.md`](architecture.md), "Ordering operators compare typed"; the
external facts are [`postgres-invariants.md`](postgres-invariants.md)'s I40,
and the amended I4 and I34. This doc is the part that is neither — the calls
that are not obvious from the diff, and the two things the slice found that
changed the plan.

## What landed

- **Four new `CompareKind` arms**, and six declared types move off
  `ComparisonPlan::AS_TEXT`: `Interval`, `TimeTz`, `Network { cidr }` and
  `MacAddr { octets }`.
- **`OrderKey` gains `Interval(i128)`, `TimeTz { utc, zone }` and
  `Network(NetworkKey)`**; `macaddr` reuses `Bytes`.
- **`special_order_key` covers `CompareKind::Interval`** with `date_out`'s two
  spellings, unconditionally — the union rule, since no server below v17 could
  have written one.
- **I4 corrected**, **I34 widened**, **I40 added**. The manual's
  "`interval` is a string, and will stay one" section is rewritten, and it
  gains a section on the six types that arrive as text and order typed.
- **The spec's 11.5 row is rewritten and `jsonb` becomes 11.5.1** — see "Why
  the split", below.

## Calls worth knowing about

**Each of the four parsers reads the type's `*_out` grammar and nothing
wider.** `interval_in` takes `1.5 hours` and `P1Y2M`, `inet_in` takes an
abbreviated `10`, `macaddr_in` takes four separator conventions — and every
one of those is refused with `Error::PredicateValueDecode` naming the value.
That is `oid`'s `UnsignedInt` refusal on a much wider grammar, and it is the
right trade here for a reason `oid` did not have: **there is no decoder to
normalize through.** These types have no Arrow representation, so their
comparison is a key built straight from text; widening the literal grammar
means implementing four input functions, and the spellings it would buy are
ones no dump contains. A user's remedy is to write the value the way the file
writes it, which makes this a property rather than a `KD<k>`.

**`NetworkKey` is a compared pair, not a sortable key**, and it is the one of
the four that could not have been. `network_cmp_internal` compares the
*shorter* of the two netmasks' worth of address bits first, so how many bits
are significant is a fact about the pair: `10.1.0.0/8` sorts below
`10.0.0.0/16` because their first eight bits agree and `8 < 16`, where any
address-then-netmask key puts it above. `compare_keys` therefore dispatches to
`NetworkKey::cmp`, and `bitncmp` is a whole-byte `memcmp` plus one masked byte
— the same answer as PostgreSQL's bit-at-a-time loop, since that loop stops at
the first differing bit.

**`cidr` and `macaddr8` exist as plan variants only to refuse a literal.**
`cidr` compares exactly as `inet` does; the difference is that `cidr_in`
rejects a value with a bit set below its netmask. `macaddr8` compares exactly
as `macaddr` does at a different width. Both facts have to ride on the plan,
because the declared name is gone by the time a value is read — the same shape
as `numeric`'s `infinities` flag and `oid`'s `UnsignedInt`.

**A `timetz`'s zone is kept, not discarded.** `decode::extract_offset` is
reused from the `timestamptz` path, where the offset is subtracted out and
thrown away; here it is negated into PostgreSQL's stored `zone` (seconds
*west*) and kept as the second half of the key, because `timetz_cmp_internal`
says two values are equal only when the zone matches too. `00:00:00+00` and
`01:00:00+01` are one instant and are ordered.

**`interval_time_micros` is not `decode_time64_micros`.** The hour field is
unbounded — `720:00:00` is an ordinary interval — and every field is checked to
be digits, which `parse_time_of_day` does not do: `"−5".parse::<i64>()`
succeeds, so `04:-5:06` would otherwise parse as a negative minute count. The
sign belongs to the whole tail, because `EncodeInterval` prints one `minus` for
the group and then absolute values.

**The span is `i128` because PostgreSQL's is.** `interval_cmp_value` widens to
`INT128` before scaling days by `USECS_PER_DAY`; the same product overflows
`i64` at the top of the type's range. v13 splits the time field into whole days
and a remainder before summing, which is the identical value — one formula
covers all six majors.

## The evidence, and how it was taken

**A throwaway cross-check ran the register against every committed oracle
cell** — `order_key` and `compare_keys` over
`fixtures/<13–18>/oracle/comparisons.tsv`, 2454 ordered pairs across all types
and all six majors, with the expected order derived from the `<`/`<=`/`>`
cells. It was deleted before the slice landed, because
[`architecture.md`](architecture.md), "The comparison oracle" makes that file
Python-side evidence and a Rust reader would be a second consumer of a format
whose rows mean what they mean only positionally. The permanent tests are
hand-written, with their expected answers read out of the same files — 11.4's
method.

What it found, and both are worth re-taking the same way if a comparison is
ever added:

- **492 cells over the six types this slice closed, zero order mismatches.**
  Every remaining report was an acceptance boundary: `infinity` accepted where
  13–16 refuse it (the union rule), and `1.5 hours`/`P1Y2M`/`1 century`/
  `08-00-2b-01-02-04` refused where the server accepts them (the output-form
  grammar). No cell in the other direction.
- **Two mismatching families, neither ours**: `jsonb`, which is the open row,
  and `name`, which is the oracle bug below.

## Two findings that changed the plan

**I4 said `pg_dump` never sets `IntervalStyle`. It does, at every supported
major.** `pg_dump.c` runs `ExecuteSqlStatement(AH, "SET INTERVALSTYLE =
POSTGRES")` immediately after `SET DATESTYLE = ISO`, guarded at v13 by a
`remoteVersion >= 80400` every supported server exceeds. Without that, an
`interval` comparison would have been unimplementable — the same value is
`1 day 02:03:04` and `P1DT2H3M4S` under two settings and nothing in the file
would say which. With it, the grammar is fixed and I40 states it. The knock-on
is that the *mapping* rationale is also gone: `interval` is a `Utf8View`
because nothing revisited it, not because the format prevents it, and
Arrow's `Interval(MonthDayNano)` fits. That is filed under STATUS's "Decisions
worth another look" rather than acted on.

**The oracle's `name` cases measure the wrong collation.** `pgdq_cmp` casts a
`text` parameter, and a cast derives its collation from its input, so
`$1::name` is compared under `default` rather than under `name`'s own `C` type
default. The committed cells therefore say `'A'::name < 'a'::name` is false
where the same server answers true for two `name` literals and for two `name`
columns. Only `name` is affected — every other collatable case states a
collation, which overrides the derived one. Recorded beside the mechanism and
in "Decisions worth another look"; not fixed, because re-asking the case
regenerates six majors.

## Why the split

11.5's spec row listed five type families as "repetitive and additive". Four
are. `jsonb` is a recursive container comparison with its own type ordering,
whose literal side needs a JSON parser that sorts and uniqueifies object keys
(the *field* side arrives already sorted, since `jsonb_out` writes the stored
order), and whose numeric leaves are `numeric_cmp` over a spelling `jsonb_in`
normalizes. Different confidence, one review cycle — the argument that already
separated 11.4 from 11.5.

**And the row promised more than the register can deliver.**
`compareJsonbScalarValue` passes `DEFAULT_COLLATION_OID` to `varstr_cmp` for
every string leaf *and every object key*, so a structurally correct `jsonb`
comparison still diverges wherever a string decides it — the database
collation, which a plain dump does not record (I32). `jsonb`'s row closes to a
*conditional* agreement with the same residue the text row has, not to a clean
one. 11.5.1 inherits that, and `KD7` is retargeted at it.

## What later slices inherit

**`AS_TEXT` has two members left**, `json` and `jsonb`, and they are there for
opposite reasons: PostgreSQL defines no comparison for `json` at all — no `=`,
no order, no operator class — so bytewise offers *more* than the server does,
while `jsonb` has a full order bytewise does not implement. The `AsText`
message ("PostgreSQL orders this type by its own operator, not bytewise") is
false for `json` and will need splitting when 11.6 strikes `KD7`, or a
`json`-specific sentence before then.

**11.6 inherits both decode-per-row exemptions, and both now exist.** The spec
names bare `numeric` and `interval` as the two types whose equality cannot use
the canonicalize-the-literal-once path. `CompareKind::Numeric` was the first;
`CompareKind::Interval` is the second, and `interval_span` is exactly the
per-row decode that exemption describes.

**Two of the four new arms are silent about equality and should not stay so.**
`inet` and `cidr` compare *equal* under `network_cmp_internal` only when family,
netmask and address all match, which is what a bytewise `=` over the output
text already gives — `inet_out` is injective. The same holds for `macaddr` and
`timetz`. So 11.6's fast path covers all four with no exception; only
`interval` needs the decode.

**`oracle_register.py` needed no edit.** Splitting one four-name match arm into
four one-name arms leaves the base-name set unchanged, and the check counts
names rather than arms — still 37 arms, 53 cases, both directions green.

**No figure was re-taken and none turned newly stale.** `pgtype.rs`,
`predicate.rs` and `decode.rs` are declared by no figure, and nothing else in
the library moved. `uv run measure.py --stale` names the same seven figures it
named before.
