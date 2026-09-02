# P12 — The ADBC type floor

Declare the Arrow type the **Arrow ADBC PostgreSQL driver** returns for a given
PostgreSQL type to be our **floor**: wherever that driver yields a real Arrow
type, ours is never a widening of it. Doing better is expected and already
happens; doing worse becomes a defect with a name rather than an unbounded
backlog item.

What the phase replaces is the Future item *"exhaustive built-in type coverage,
with tests to match"* — an open-ended "every built-in type, eventually" becomes
a bounded target set by somebody else's shipped driver, against which "are we
there" is a check rather than a judgement.

This doc is the binding spec. Progress lives in
[`../status/STATUS.md`](../status/STATUS.md), never here.

## D1 — The floor is the latest **release**, not upstream main

**The floor is `apache-arrow-adbc-24`** — Python `adbc_driver_postgresql`
1.12.0, tagged 2026-07-24 — and the oracle records the exact version it was
taken at. Upstream main is tracked as a watch item, never as a target.

The driver is two different things, and the gap falls precisely on this phase's
largest obligation. Three commits sit between release 24 and main at
`be5f50f08` (2026-08-30):

| Commit | What it does | In release 24? |
|---|---|---|
| `64f555f56` | `json`/`jsonb` as the `arrow.json` extension | **yes** |
| `6e0d7a135` | `uuid` as `FixedSizeBinary(16)` + `arrow.uuid` | no — main only |
| `223664fee` | `POSTGRESQL:type` on every non-root field | no — main only |

Three reasons, and the first is the one that decides it. **A floor is a promise
about what a user could otherwise get, and users get releases** — measuring
against code nobody can install declares us below a floor that does not exist in
the world. **Main can still change before it ships**: both relevant commits are
weeks old and unreleased, so a mapping written against them may be written
against a design that is revised. And **only a release can be taken
mechanically** — a pip-installable wheel is reproducible in the fixture
container, where building main's C++ driver is a second build system this phase
would acquire.

**The consequence is that the metadata obligation shrinks to nothing, and that
is a finding rather than a let-off.** Against release 24 we already meet or beat
the metadata floor on every column: no modeled field carries any metadata at
all, and we stamp `arrow.uuid` on a real `FixedSizeBinary(16)` where the release
hands back opaque bytes. So "carry the declared PostgreSQL type name on every
column", which the inbox recorded as this phase's, is **not** this phase's — it
is what release 25 will likely demand, and the spec says so rather than
building it early against an unshipped spelling.

*Rejected:* measuring against main, so the phase is ahead of the curve. It
inverts what a floor is for — the point is that no user can do better elsewhere,
not that we match the newest unreleased commit — and it makes the oracle
unreproducible, since main has no artifact to pin.

Evidence: [`../status/history/2026-09-02.md`](../status/history/2026-09-02.md),
"The ADBC floor is a release, not a branch".

## D2 — One rule, over declared types, with every exception carrying a stance

**The floor holds for a declared type where ADBC yields a non-opaque Arrow type
*and* the COPY TEXT and binary encodings denote the same value. Everywhere else
the floor is *undefined*, not violated — and every such row declares why.**

Three things put a row outside the rule, and they are different in kind, which
is why one rule with a stance beats three exceptions:

- **ADBC answered `arrow.opaque`.** Its bottom is raw *binary* wire bytes plus
  a type name; ours is the file's own text. The two are incomparable and ours
  is the more useful — nobody can read ADBC's `inet`, and anybody can read
  `192.168.1.0/24`. This bucket is large: `bit`, `varbit`, `inet`, `cidr`,
  `macaddr`, `macaddr8`, `xml`, `tsvector`, `pg_lsn`, `timetz`, the geometric
  family, every range and multirange — and, at both revisions, **`numeric`**,
  whose `string` answer is tagged `arrow.opaque` and so is not a floor row at
  all.
- **The two encodings denote different values.** `regprocout` returns the
  function's *name* — schema-qualified when the bare name would not resolve, and
  `-` for `InvalidOid` — where the binary encoding ADBC reads is the OID. So
  "narrower than `Int32`" is not a question that can be asked of the text. This
  is stated as a rule over the `reg*` family rather than as a per-type
  exception list.
- **Below the floor by decision.** `money` is refused by our own bar, not by
  effort: `cash_out` reads the monetary locale — `frac_digits`,
  `mon_decimal_point`, `mon_thousands_sep`, `currency_symbol`, `mon_grouping` —
  and `pg_dump` sets neither `lc_monetary` nor `IntervalStyle` anywhere, so
  `1.234,56` and `1,234.56` cannot be told apart from the file. ADBC escapes
  this only because binary hands it the raw `int64`, and it documents its own
  answer as lossy.

**A stance, not a verdict.** This follows the precedent the comparison register
set in P11: a row records *why* it is where it is, so a later session meets the
reasoning instead of re-deriving it. `money` is the phase's first deliberate
below-floor row and earns a `KD<k>` under stance **(a)** — a consequence of a
deliberate tradeoff, never to be worked.

*Rejected:* stating the floor over all types and listing exceptions. The
exceptions are not one kind of thing, and a flat list cannot say that `regproc`
is unanswerable while `money` is refused — which is the distinction that stops
either from being re-proposed.

## D3 — Scope the rule to fidelity, and state it over types rather than columns

**A narrower Arrow type bought by losing data is not a floor we must match, and
the floor is a claim about declared types, not about what a particular dump
resolves to.**

Two rows are wider than ADBC for reasons that are not mapping choices, and this
decision puts both outside the rule:

- **The array census.** ADBC names `List<T>` from the type alone and *flattens*
  a multi-dimensional array into a one-dimensional list — its reader reads
  `n_dim` and rejects only `n_dim < 0`. We resolve `List<T>` optimistically and
  let the census demote a column of mixed dimensionality or `[lb:ub]` decoration
  to `Utf8View`. Strictly read we are wider; scoped to fidelity we are not,
  because their narrower type is a wrong answer.
- **Name resolution.** A type name that needs quoting (`KD4`), or a composite
  whose body the grammar could not read, resolves `Utf8View` where ADBC —
  resolving by OID against a live catalog — always gets a real type. These are
  per-*dump* facts, not per-type mapping choices: they are what the file says,
  not what the mapping decided.

**What this buys is a check that stays clean.** A floor stated over columns
would report both every run, and a signal that is always on is no signal. `KD3`
and `KD4` therefore keep their existing stances and this phase acquires neither.

*Rejected:* adopting the Future item *"the shape-general array
representation"* (`Struct{dims, lbounds, elements}`) as the demotion target, so
the floor holds with no carve-out and `KD3` closes. It is the tempting answer
and it over-reaches: that item is scoped as a **caller-selected knob**, and
making it the automatic demotion target pre-empts the choice it deliberately
leaves to the caller, while also removing `List` — the signal every generic
Arrow consumer reads as "this is an array" — from columns that have it today.
The knob remains wanted; it is not this phase's.

## D4 — `interval` is mapped, and joins `KD8` rather than earning a stance

**`interval` resolves to `Interval(MonthDayNano)`.** It is the phase's one real
mapping win and the only floor row whose closure changes what a user gets for a
type they are likely to have.

Nothing blocks it any more. `pg_dump` runs `SET INTERVALSTYLE = POSTGRES` on its
source connection at every supported major (I4), so the text is determined
rather than session-dependent; I40 states that grammar in full — parts, units,
signs, field widths — and `predicate.rs`'s `interval_span` is a working parser
of exactly it, checked against every committed oracle cell. The shape matches
too: Arrow's `Interval(MonthDayNano)` carries months, days and a time part as
three independent fields, which is exactly PostgreSQL's `Interval`.

**What it costs is two value classes, and they are `KD8`'s shape, not a new
one.** An infinite interval (v17, I34) and any interval whose time part exceeds
`2562047:47:16.854775807` — PostgreSQL's field is `int64` *microseconds* against
Arrow's `int64` nanoseconds, and nothing normalizes hours into days (I40) — have
no `MonthDayNano` encoding, so a field holding one is an `Error::FieldDecode`.
That is exactly what `Date32` having no infinity and `Decimal128` no NaN already
mean for `date`, `timestamp` and `numeric(p,s)`. So **`KD8` is rewritten to name
`interval` as a third case** rather than a stance-(a) row being opened beside
it; `--schema-mode strings` is the stated recourse for all three, and
materialization stays the open question `KD8` already describes.

**The decoder is new, though the grammar walk is not.** `interval_span` fuses
months, days and time into one `i128` on PostgreSQL's 30-day-month convention
because ordering needs a scalar; a decoder needs the triple preserved. The walk
is reusable, the fusing is not. Render-back must refuse a sub-microsecond
nanosecond value, which is a value it could never have produced.

## D5 — `int2vector` is closed, not carved out

**`int2vector` resolves to `List<Int16>`.** It is a floor row under D2 —
non-opaque, and the two encodings denote the same value — and closing it is what
makes the floor hold with no exception beyond the three D2 names.

D3 declined to let the floor drag work in, and this does not contradict it: that
argument was about rows where our width buys **fidelity**, and here our width
buys nothing. `int2vectorout` writes space-separated `int16` with no quoting, no
nulls and no escaping, so the codec is the cheapest in the type system and the
`List` machinery already exists.

*Rejected:* carving it out as a catalog type no real schema declares. True —
neither koji nor any fixture has one — but a carve-out needs a stance, and the
only honest one available is "we could not be bothered", which is not among the
three. A rule with an exception nobody can justify is weaker than the code it
saves.

## D6 — Its own oracle, on the comparison oracle's apparatus

**`fixtures/<major>/adbc/floor.tsv`, taken by `generate_fixtures.py` against the
container it already stands up, recording the driver version and the server
major in the file.** Beside it, a reconciliation that joins the oracle against
`builtin_scalar` and **fails in both directions** — every mapped arm is checked
against the floor, and every floor row resolves to an arm — with D2's stance
rows as *declared* exemptions rather than silent absences.

This is the register-to-oracle reconciliation's shape, deliberately: that check
already parses `builtin_scalar`'s arms out of `pgtype.rs` (a `match` is not
data) and already fails on the direction that decays quietly. The floor has the
same failure mode — a type mapped without evidence, and evidence for a type the
oracle's own database does not have.

**Separate from the comparison oracle**, on two counts. It answers a different
question against a different reference: the comparison oracle asks what the
*server* answers, this asks what somebody else's *driver* returns. And the
driver version is a third axis `meta.tsv` has no column for, which is exactly
the axis D1 makes load-bearing.

**Per major**, because the type set genuinely moves — multiranges arrive at 14,
and a row absent at 13 is not a row we are below.

## D7 — The oracle enumerates by catalog sweep, and the sweep is committed

**Every `pg_type` row a user could declare a column of**, not a curated list:
`typtype` in `b`, `e`, `r`, `m`, `d`, restricted to `pg_catalog`, `typisdefined`
true, and the array types excluded (`NOT EXISTS (SELECT 1 FROM pg_type e WHERE
e.typarray = t.oid)`) since the recursion covers those separately.

*Rejected:* spelling that exclusion as a shape test, `typelem <> 0 AND typlen =
-1`. It also matches `int2vector` and `oidvector`, which are declarable types in
their own right that no array recursion reaches — and the first of those is the
row D5 commits this phase to closing, so the shape test deletes the type this
phase is about and leaves D6's both-ways check demanding an exemption for the
arm D5 mandates. An array type is exactly one that some other type names as its
`typarray`, which is what the back-reference asks.

Evidence: [`../status/history/2026-09-02.md`](../status/history/2026-09-02.md),
"The floor sweep's array exclusion".

The committed TSV is what keeps a sweep auditable. The sweep proposes and the
reviewed file disposes, so a regeneration diff *is* the "a new major added a
type" signal — which a curated list can never produce, because nothing prompts
anyone to extend it.

The split this buys is between the phase's **work** and its **coverage**. The
long opaque tail costs one stance line each under D2 and no code, so coverage
can be unbounded while the work stays bounded. A curated list inverts that: it
bounds the work by bounding what the floor is allowed to notice, which is
exactly the property a floor exists to deny.

**The oracle is taken from the host**, through the pinned `uv` environment
against the container `generate_fixtures.py` already stands up — not from inside
the container, which would mean installing the driver into the Postgres image.

## D8 — The driver is pinned, and the oracle's recorded version must equal the pin

**`scripts/pyproject.toml` pins `adbc_driver_postgresql`, and the reconciliation
asserts the version recorded in every `floor.tsv` equals that pin.** Bumping the
pin is therefore the deliberate act that obliges re-taking the oracle, and the
two cannot drift without failing.

D1 makes the driver version load-bearing, and nothing otherwise notices when it
moves: the floor would quietly become a claim about a release nobody runs.
`postgres-invariants.md` is the wrong home — it records *PostgreSQL* behaviour
and its walk-the-file ritual fires on a Postgres release, not an ADBC one — so
the pin carries it instead, which is the fixture discipline already in force for
base images applied to a second upstream.

*Rejected:* checking against whatever `uv` resolves as latest. It turns somebody
else's release into a spurious local failure, and makes the check fire on an
event nobody in this project chose.

## D9 — An internal check now; the promise waits for the surface that makes it

**The rule and its stance register are filed beside "The bar" in
[`architecture.md`](architecture.md). Nothing goes in the manual this phase.**

*"The schema you get is at least as good as ADBC's"* is an embedder-facing
promise, and the surface an embedder would read it against is P6's, which does
not exist. Publishing it now would state a guarantee about an API that has not
been presented, against the standing rule that nothing pre-1.0 carries one. What
this phase produces is the **mechanism that makes the promise checkable**
whenever P6 chooses to make it — which is the sequencing the roadmap already
argues for in scheduling P12 ahead of P6.

`docs/manual/type-handling.md` is re-read for **claims**, not for a promise:
prose asserting that `interval` or `int2vector` comes back as a string goes
false when D4 and D5 land, and correcting it is an obligation of those slices.

## D10 — A waiting floor row is an exemption, not a register entry

**`interval` and `int2vector` are declared exemptions in the reconciliation's
own table, each naming the slice that closes it — not `KD<k>` entries.**

12.2 lands a check that reports both as below the floor, three and five slices
before they close, so something must carry them in the meantime. The register's
`(b)` stance is the near miss: it would have them name 12.3 and 12.6 with
`deficiencies.py` holding the pairing. What rules it out is what a `KD<k>`
*means* — known, and **not being fixed now**. These are being fixed in this
phase, and allocating two numbers only to strike both inside it turns the
register into a progress tracker, which is `STATUS.md`'s job and not its.

`money` is the contrast that draws the line: it earns a register entry precisely
because no slice will ever close it.

## D11 — Slices

Ordered so the evidence lands before the mechanism it checks — the oracle has to
be able to catch a mis-mapping it did not produce.

| Slice | What it does |
|---|---|
| **12.1** | The floor oracle: the catalog sweep of D7, `fixtures/<13–18>/adbc/floor.tsv`, and D8's driver pin. No library code. |
| **12.2** | The reconciliation: joins the oracle against `builtin_scalar` and fails both ways, with D2's stances as declared exemptions and D10's two waiting rows naming their slices. The rule is filed beside "The bar"; `money` earns `KD13` under stance (a). |
| **12.3** | `interval`: the triple-producing decoder and the resolution arm. |
| **12.4** | `interval`: render-back's sub-microsecond refusal, the comparison register's `interval` arm, and `KD8` rewritten to name it. |
| **12.5** | `int2vector`: the fixture column, and the six-major regeneration. |
| **12.6** | `int2vector`: the codec, the resolution arm, and the type's comparison-oracle case. |

**Two splits are deliberate, and both are the seam `../process.md` names.**
12.3/12.4 separates a self-contained new decoder from a rework of paths P11
already tested — the comparison register's arm and render-back — because
bundling them forces one review to accept both at one confidence. 12.5/12.6 is
the evidence-first seam again, small but real.

**12.4's row said "and `type-handling.md` corrected", and that clause is
struck: D9 owns it.** D9 makes correcting a falsified manual claim an
obligation of the slice that falsifies it, and the resolution arm is what
falsifies `type-handling.md`'s `interval` prose — so scheduling the correction
a slice later contradicted this spec's own decision, and the schedule yields to
the decision. The split argued above is the *code* split; it never bore on the
manual. Reasoning:
[`../status/history/2026-09-02.md`](../status/history/2026-09-02.md), "A
falsified manual claim is corrected by the change that falsifies it".

**`interval` needs no evidence slice and `int2vector` does**, because
`fixture_schema_types.sql` already carries `t_interval` and has no
`int2vector` column. That asymmetry is stated rather than smoothed over: the two
types look alike in the delta table and are not alike in what they cost.

`KD13` is allocated by 12.2, in the change that writes the entry — not here.
