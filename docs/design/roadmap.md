# Roadmap

Where this project is going. Each phase that has been specified gets its own
doc, `roadmap-P<N>-<slug>.md`; the sections below are the index. Phases
still sketched here are at a level sufficient to keep current work from
painting us into a corner — **each gets its own full grilling session when it
becomes current**, and the resulting spec becomes its own doc.

**`P<k>` is an identifier, not a position** (`../process.md`, "Phase identity
is `P<k>`"). Phases are discovered work: they are allocated numbers in the
order they are *found*, run in the order that suits, and sometimes run two at a
time. **The order is this table, top to bottom** — never the numbers, and never
the slug beside them, which is a caption rather than a name. Numbers are never
reused, including a struck phase's.

| Phase | State | Where it is |
|---|---|---|
| P1–P5, P9 | **Struck** at a keystone review | [`architecture.md`](architecture.md), by subject; git holds the specs |
| P11 — typed predicates | **Current**; sliced, in flight | [`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md) |
| P7 — scan performance | Sketched; design doc ahead of its phase | [`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md); [inbox](roadmap-P7-scan-performance-inbox.md) |
| P12 — ADBC type floor | Sketched; not grilled | this file, below; [inbox](roadmap-P12-adbc-type-floor-inbox.md) |
| P10 — row-group statistics | Sketched; not grilled | this file, below; [inbox](roadmap-P10-row-group-statistics-inbox.md) |
| P6 — embeddable engine | Sketched; not grilled | this file, below; [inbox](roadmap-P6-embeddable-engine-inbox.md) |
| P8 — format coverage | Sketched; not grilled | this file, below; [inbox](roadmap-P8-format-coverage-inbox.md) |

**A row's state is one of `Sketched`, `Specified`, `Current`, `Complete` or
`Struck`**, and the prose after it is a caption. Two of them are set in the same
change as a `STATUS.md` edit: **`Current` when the phase is sliced**, in the
change that writes its checklist, and **`Complete` at the wrap**, in the change
that deletes it. A phase carrying a checklist is `Current` and a `Current` phase
carries one — so a transition that half happened is visible from either side,
and this table can say a phase ran and finished long after the checklist is
gone, with the keystone that would strike it years later. That matters beyond
bookkeeping: `scripts/deficiencies.py` reads this column. It holds the
checklist pairing both ways — a checklist under a row that is not `Current`
fails, and so does a `Current` row with no checklist — and a known deficiency
owned by a phase that is `Complete`, `Struck`, or absent from this table has no
destination, so it drops to `(c) unowned` unless another phase absorbs it
(`../status/STATUS.md`, "Known deficiencies").

The struck phases' mechanisms are described by subject in
[`architecture.md`](architecture.md), not by phase; their specs and notes went
at a keystone review (`../process.md`, "The keystone: striking the
centering"). **Phase numbering continues from `P12`** — nothing at or below it
is reused, whether it was struck, sketched, or never specified.

Two standing-constraint docs cut across everything below.
[`layering.md`](layering.md) assigns each module to one of four layers and
fixes the direction dependencies may point; several phases here are cross-layer
by nature — P10's statistics most of all, which that doc calls the sharpest
test of its own rules — and it holds the decision rules for them. [`postgres-invariants.md`](postgres-invariants.md)
is the evidence layer: every `pg_dump` behaviour a decision treats as
guaranteed, with its proof and its re-verification command.

## Pre-1.0: no compatibility obligations

Everything in this roadmap happens before 1.0, and **nothing here carries a
backwards-compatibility or API-stability guarantee**. Public types, the CLI
surface, the cache format, and the Arrow schema a given table resolves to are
all free to change in any release until 1.0. We are iterating, not publishing
contracts.

This is a standing decision, not a question to reopen each phase: don't design
around hypothetical downstream breakage, don't add compatibility shims, and
don't caveat a proposal with migration concerns. The cache's `format_version`
envelope exists so that a stale cache is *detected* rather than misread — not
as a promise to keep reading old ones.

The one place this genuinely costs something is the Arrow schema drifting as
type coverage grows (a column that resolves to `Utf8View` today may
resolve to `Int32` after the next release, invalidating a downstream query
plan). The Future item "caller-supplied type mapping" below is the answer for
anyone who needs a schema pinned; until 1.0 it is the only one.

## Project goals

Two things distinguish this project from existing `pg_dump` tooling
(`pgdumplib` and friends), and both shape the phase ordering below:

- **Embeddable as a query data source**, not just a dump reader — Arrow-native
  output, and ultimately a DataFusion `TableProvider` (P6).
- **High performance is a core goal, not a later optimization**, specifically
  for the local-file reader. Dumps are routinely hundreds of gigabytes; the
  difference between a saturated-device scan and a merely-correct one is the
  difference between a usable tool and an overnight job. Concretely: the
  local-file path should stay device-bound, not CPU-bound, on hardware from
  HDD through NVMe, at flat memory. See
  `docs/design/roadmap-P7-scan-performance.md`.

  This targets the `COPY`-block/bulk-row path specifically. Preamble and other
  non-data DDL scanning is bounded by schema size, not file
  size — a few thousand lines even for a multi-hundred-GB dump — and isn't
  held to the same device-bound target.

## Standing rules

Decisions that apply to all work below, not to any one phase. They are here
rather than in a phase doc because they outlived the phases that produced them.

### Coverage increases monotonically

**A later change may subdivide a span or attach detail to it, never reduce
coverage.** Specificity increases; the tiling property does not degrade.
Splitting the one large-object span into one span per object is the intended
shape of "more specific"; introducing a span kind that leaves bytes
unaccounted for is not.

The rule is only real because something checks it. `every_fixture_tiles_exactly`
asserts tiling over every fixture, and the cases that matter most are the
degenerate ones: `--data-only` (no DDL), `--schema-only` (no data),
`--inserts` (no `COPY` blocks at all), and the concatenated multi-database
shape. Absent that test, "spans sum to file size" decays into an aspiration the
first time a span kind is added.

### The input contract is valid PostgreSQL, not `pg_dump`'s output

**pgdq reads plain SQL that a PostgreSQL server would accept, whoever wrote
it.** `pg_dump` is the producer we verify against because it is the one we can
run six versions of, not the boundary of what we accept. A file that is
hand-written, hand-edited, or emitted by another `pg_dump`-compatible tool is
in scope, and a wrong answer on one is a defect like any other.

Two consequences, and the second is what the rule is really for:

- **A declared type is read the way PostgreSQL's own type system reads it**,
  because that is the type the values in the `COPY` block were written by.
  `resolve_declared_type` is already nothing but this — `int4` → `Int32`,
  typmod splitting, the domain walk — so a spelling read *more literally* than
  the server reads it is the anomaly, not the interpretation.
- **A resolution outcome states only what the DDL supports.** Refusing a
  column is always available; refusing it under a label that makes a false
  claim about the column is not.

When another producer is identified, its output is incorporated into the
fixture generators the same way `pg_dump`'s is — the rule below is about the
method, not about `pg_dump`.

### Expand the generated fixtures freely; verify objectively wherever possible

**When a decision depends on the exact bytes some producer writes for a shape,
put that shape in `scripts/fixture_schema_*.sql` and regenerate, rather than
reasoning out what the output must be.** This is closed, and the bias is
deliberately lopsided: a fixture column costs one generator run across six
majors and a few lines of schema, while a wrong inference costs a design built
on it. We have paid the second price more than once.

- 4.1 specified the array and composite fixture shapes by reasoning about
  `array_out`; two values it did not know it needed surfaced only afterwards
  and earned 4.1.1 — an array over a domain whose base is `box`, whose
  separator is `;` and which therefore walks straight through a refusal written
  against the declared spelling (I22), and a zero-field composite (I23).
- 4.4 typed `d[]` where `d` is a domain over an array as `List<List<T>>` by
  composing the mapping rules. No such column exists in the fixtures, so six
  majors of round-trip tests passed over a column no value can fill; the real
  literal is one brace deep (I26), and finding that cost a slice (4.4.2).
- The sweep that found it also found five shapes that worked and were pinned
  by nothing at all — domain over composite, array of that, composite inside
  composite, array of a user range, domain over a range. 4.4.2's
  `t_nested_array` holds all five.

The corollary is the part that is easy to skip: **a shape observed to work is
not covered until a fixture holds it.** "The recursion handles it" is the
reasoning that failed above, and it will keep failing, because the recursion is
right about the type and says nothing about the bytes.

**Half the rule is mechanical.**
`tests/pgtype.rs::every_resolution_outcome_is_produced_by_a_real_fixture_column`
requires each `ColumnResolution` variant to have a real fixture column behind
it, and its exhaustive match makes a new variant a compile error until it is
listed — so a new refusal cannot land without the `pg_dump` output that
reaches it. The other half, a shape that resolves to an existing outcome and
merely works, stays a judgement call; naming that limit beats a check implying
coverage it does not have.

The cost is real and bounded — regeneration needs Docker and the six
PostgreSQL images (`CLAUDE.md`), and added columns widen literals other tests
read. Pay it. The alternative is discovering the shape from a user's dump.

**Where a fixture is impossible, the invariant proving it impossible stands in
for one.** Some behaviour is reachable only from input a producer we can run
provably never emits — `integer[][]` survives no round trip through
`format_type` (I21), so no generator input makes `pg_dump` write it. That is
not an exemption from the rule but a case of satisfying it: the objective
evidence exists, it is in the invariants register, and it is stronger than a
fixture would be. Such behaviour is pinned by unit test over a hand-built
`TypeDef` list, and the entry it rests on is cited at the test. The rule's
target is *guessing*, and a verified invariant is the opposite of a guess.

**The citation is the check.** A test claiming this carve-out names its `I<n>`
at the test site, or it is not covered by it — which puts the behaviour inside
the register's own ritual, where every entry carries a re-verify command that
gets walked at each new PostgreSQL major. That is a stronger guarantee than a
second enforcement mechanism would be, and it is what stops the carve-out from
becoming a way to skip a fixture that was perfectly possible.

**The check.** Every standing rule here carries one, and this rule's is
partial by nature: a test walks every fixture, resolves every column of every
block, and asserts that **each `ColumnResolution` variant is produced by at
least one real fixture column**. Adding a resolution outcome without a fixture
that reaches it then fails a test instead of relying on discipline — which is
exactly what 4.4 would have hit. It lands with 4.4.2, reusing the fixture-walk
helper `tests/map.rs` already has.

What it does *not* check is the other half: a shape that resolves to an
existing outcome and merely works — the five the sweep found — is still a
judgement call. That gap is named rather than papered over, because a check
claiming more coverage than it has is worse than one that states its limit.

### Refactor when the shape stops fitting

**A refactor that would survive the roadmap — or at least its next several
phases — and that reduces complexity or increases flexibility is taken when it
is noticed, not deferred to a tidying pass that never comes.** The 1.0
architecture is meant to be as clean and elegant as we can make it, and rushing
to a done state without periodic refactoring is how a codebase turns to
spaghetti.

The rule is a tie-breaker, not a licence. It applies when the alternative is
choosing between two acceptable placements of the same logic, or when a slice
has just added the second special case to something that wanted one. It does
not license rewriting a mechanism whose shape is merely unfamiliar, and it
never widens a slice's behaviour: a refactor taken under this rule is
behaviour-preserving, and anything that changes what the code *does* is a
decision, so it needs the ordinary treatment — grill it, amend the spec, give
it a slice number.

**The check.** A refactor taken under this rule names, at the point it lands,
which later phases it expects to survive and what it made simpler. If neither
can be stated, the rule did not apply and the change is a preference.

### A slice row that commits to a measurement names its instrument

**A slice row promising a figure says what will take it.** Not the number, not
the threshold — the instrument: a microbenchmark, an end-to-end run under a
named command, a comparison against a named control. A row that names only the
deliverable leaves the instrument to whoever implements it, and the implementer
will pick the cheapest one that can be argued to satisfy the words.

This is narrow on purpose. It binds rows that commit to a *measurement*, where
the instrument is the part most likely to be under-determined and where two
instruments can differ by an order of magnitude in cost while both answering to
the same sentence. Everywhere else the ordinary spec discipline is enough.

**A named instrument is satisfied by whatever it resolves — including a
bound.** The row commits to running the instrument, not to the answer coming
back above its floor. A null or negative reading is a result, and the
instrument's own floor is part of the deliverable: state it beside the figure,
and name the sharper instrument without building one. A row is unsatisfied
only if the instrument it named was not built or not run. This is the second
failure of the same row-shape, and it points the other way from the first:
4.6.1 named its instrument, ran it, and got the composite column's share back
*below* what differencing two generated files can resolve. Rescuing that with
an unbudgeted second instrument is scope growth arriving through a measurement
row — the hazard an unattended session is most likely to walk into, since
building something looks more like finishing than reporting a bound does.
Reasoning: [`../status/history/2026-08-27.md`](../status/history/2026-08-27.md),
"The review of the six measurement decisions".

**Why it is a rule rather than a note.** 4.6's row asked for two figures. For
one it named both instruments — "a `decoders.rs` micro **and** `pgdq query
--schema-mode typed` against `strings`" — and got both. For the other it named
only "composite decode throughput", got a micro, and earned **4.6.1** at
review to supply the end-to-end half. Same row, same author, same slice: the
half that named its instrument was delivered whole. Every phase from here is
measurement-heavy — P7 is an entire performance campaign — so the hazard
is live for four unwritten specs. Reasoning:
[`../status/history/2026-08-27.md`](../status/history/2026-08-27.md), "4.6.1
is earned, and the spec row's ambiguity is why".

### A test may assume the tools `mise` pins

**A test that needs a pinned tool asserts its presence; it does not skip
itself.** `mise.toml` is the project's declaration of what a checkout has, and
`mise install` is one command — which is the point of using `mise` at all: a
CI run is one step away from every tool the suite needs. So a missing pinned
tool is a misconfigured environment, and the honest report is a failure naming
`mise install`, not a green run with a printed skip.

**Why it is a rule.** A conditional skip is invisible in a passing suite, and
the test most likely to carry one is a test nothing else covers — which is
exactly when its silent absence costs the most.
`pgdump_query-cli/tests/perf_generator_fidelity.rs` is the whole example: one
test, the suite's only guard against `scripts/generate_perf_data.py` drifting
away from what `pg_dump` writes, and it skipped itself when `uv` was absent.
Three infidelities had already survived in that generator for want of any
guard at all.

**The check.** A test that reaches for an external tool either asserts it is
present, or is gated behind an env var that defaults to *enforcing* — never a
silent skip. A genuinely machine-local resource, which `mise` cannot pin, is
the exception and gates the other way: unset by default, absent in every other
checkout, per `CLAUDE.local.md`'s rule for the koji replica.

### Four decisions that keep later phases additive

Plain-format-only and single-threaded is a deliberate scope, not a limitation
to design around. These four choices are what make P7 and P8 additive
rather than a rewrite, and they are cheap to hold to — so hold to them, even
where the work in front of you would not require them.

- **The COPY TEXT decoder stays independent of where its bytes came from.**
  `copy.rs` operates on a caller-owned slice and never assumes "a file at
  offset N". Every dump format stores table data as this same COPY TEXT
  payload, so this decoder is the one component all of P8 reuses verbatim.
- **Structure discovery is a separate concern from row decoding.** `scan.rs`
  finds `COPY` boundaries in a plain file; an archive reads them from a TOC.
  Keeping "what entries exist and where are their bytes" apart from "decode
  these bytes into rows" is what lets a container layer slot in later.
- **The cache format is versioned and records what produced it.** A serialized
  `DumpIndex` carries a format-version field and a container-kind tag, so
  archive-derived indexes and entry-relative offsets are a later variant rather
  than a breaking change. A cache whose version or kind is not recognised is
  treated as absent, which the "never required for correctness" rule makes
  safe.
- **`ResumeToken` exposes no fields, ever.** Its contents today are a file
  offset plus a row index, but a raw file offset is meaningless inside a
  compressed archive entry. Opaque now means the representation can change
  without an API break.

P7 adds a fifth that already binds: the batch layer builds `Utf8View`
arrays over the scanner's existing chunk buffer instead of copying field bytes
out of it. See
[`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md).

### Permanent non-goals

- **Writing or modifying dump files.**
- **A full SQL/DDL parser** — only enough recognition to locate `COPY` blocks,
  classify statements into spans, and extract columns and types from
  `CREATE TABLE`.
Note that CSV-format `COPY` blocks are **not** on this list. They are a Future
item; see below.

## P11 — Typed predicates

**Specified.** The binding statement is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md): what a
filter *means* — full boolean structure with a three-valued evaluator,
type-aware comparison on nested columns, typed equality, and the retirement of
`KD7`'s ordering-register divergences. Progress is the checklist in
[`../status/STATUS.md`](../status/STATUS.md).

*The reason that used to defer it behind the engine story is withdrawn:* that
"DataFusion hands a `TableProvider` an `Expr` tree and asks, per filter,
whether the pushdown is exact, inexact or unsupported", so designing a boolean
expression language before seeing that would be inventing a second one to
reconcile. That shape is documented, stable, and readable from the DataFusion
source without building anything. The withdrawn reason is recorded so it is not
re-derived.

## P7 — Scan performance

**Inbox:** [`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

Concentrated optimization of the local-file read path: SIMD-accelerated
structure discovery, zero-copy row extraction into Arrow buffers, bulk UTF-8
validation, and device-aware parallelism (sequential on rotational media,
parallel on NVMe). Full sketch, including the measurements that should gate
each piece and the decisions it constrains, in this phase and before it:
`docs/design/roadmap-P7-scan-performance.md`.

Scheduled ahead of the engine story, and ahead of P8 and P10, for three
independent reasons. Local-file performance is a project goal rather than a
later optimization, and two deficiencies wait on this phase (`KD5`, `KD9`). The
format work multiplies the surface area any later optimization has to be
correct against, so the fast path should exist first and archive containers
should be built to fit it. And statistics need the sparse row index this phase
builds.

**Two reasons for a later placement have been withdrawn, and both are recorded
so they are not re-derived.** The first was that pushdown "changes which bytes
get touched at all, so optimizing the pre-pushdown parser would partly optimize
code that pushdown deletes" — pushdown deletes no parser code and never did; a
projection skips `decode_field` and the builder append for a column nobody
asked for and changes nothing about what the scanner does
([`architecture.md`](architecture.md), "Projection").

The second was that the engine story's `object_store` backend "settles the I/O
layer that any readahead or parallelism scheme has to live behind". That
question is already settled *here* rather than there:
[`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md) rejects mmap
precisely because it bypasses `ByteRangeSource` and so could never be the path
an `object_store` backend takes, and commits instead to positioned reads behind
that same abstraction. What genuinely does not transfer is the *tuning* —
readahead depth and chunk-size defaults measured against local devices say
nothing about a high-latency ranged backend — and that is a second set of
measured defaults the engine story adds, not a rework of this phase.

## P12 — The ADBC type floor

**Inbox:** [`roadmap-P12-adbc-type-floor-inbox.md`](roadmap-P12-adbc-type-floor-inbox.md) — the survey
that discovered this phase, filed as facts its grilling must not miss. Drain it
when grilling this phase.

Declare the Arrow type the **Arrow ADBC PostgreSQL driver** returns for a given
PostgreSQL type to be our **floor**: wherever that driver yields a real Arrow
type, ours is never a widening of it. Doing better is expected and already
happens — `numeric(p,s)` is a decimal where ADBC returns a string, an enum is a
dictionary, a range is a struct where ADBC returns opaque bytes — but doing
worse becomes a defect with a name rather than an unbounded backlog item.

This absorbs the Future item *"Exhaustive built-in type coverage, with tests to
match"*, which is what it replaces: an open-ended "every built-in type,
eventually" becomes a bounded target set by somebody else's shipped driver,
against which the answer "are we there" is a check rather than a judgement.

Three things make it a phase rather than a mapping change, and the second is
the one that decides what the phase actually produces:

- **The floor has to be taken, not transcribed.** It is one C++ `switch` in an
  actively developed upstream, so a table copied into a doc decays silently.
  `generate_fixtures.py` already stands a Postgres container per major and
  takes the comparison oracle; an ADBC schema oracle is the same apparatus
  pointed at a different question.
- **Their bar is not our bar.** ADBC reads the *binary* wire format with the
  catalog in hand, so every value is session-independent and its bottom is raw
  bytes plus a type name. We read text, under
  [`architecture.md`](architecture.md)'s "the dump alone determines the value",
  and our bottom is the file's own text. Some rows are therefore below the
  floor **by decision**, and stating which, with the reason, is part of the
  deliverable — not an omission from it.
- **Where the floor and an existing design conflict, one of them gives.** The
  array census is the live case: ADBC names `List<T>` from the type alone and
  flattens what does not fit, where we demote to a string and say so. Resolving
  that either scopes the rule to fidelity or pulls in the Future item *"the
  shape-general array representation"*, and that is a decision this phase
  makes.

**The floor covers a field's metadata, not only its Arrow type.** Decided ahead
of the grilling, because it changes what the phase is for: a `FixedSizeBinary(16)`
that does not say it is a UUID is below the floor even though its type matches.
What that obliges is larger than it first looks — ADBC writes a
`POSTGRESQL:type` key on **every** non-root field, and `arrow.opaque` (with a
`type_name` and `vendor_name`) on every field it cannot model — so carrying the
declared PostgreSQL type name on every column is this phase's, and the question
of whether a column holding readable text may honestly claim `arrow.opaque`,
whose meaning is *cannot interpret*, comes with it. The two canonical extension
names that need no such decision are already out, as `M29`.

**Scheduled after P11, ahead of P10 and P6; its position relative to P7 is
free.** After P11 because `builtin_scalar` answers "which Arrow type" and "how
do two of these compare" in one arm, and the register-to-oracle reconciliation
requires a case for every arm — so a type mapped before that register settles
writes arms against a moving spec. Ahead of P10 because a per-row-group minimum
over a `Utf8View` column is a lexicographic bound where a typed one is a real
one, and statistics are worth more over the wider type coverage. Ahead of P6
because "the schema you get is at least as good as ADBC's" is an
embedder-facing promise, and P6 is the phase that presents promises over
mechanisms that have stopped moving.

**Three of the survey's rows were never this phase's**, and landed as
out-of-band work during P11's run rather than waiting: `oid`, which maps to an
unsigned integer under the existing bar, and the `arrow.json` and `arrow.uuid`
extension names, which attach to types we already emit and are validated
against exactly those storage types. That is `M29` in the ledger below; see
[`../status/history/2026-08-31.md`](../status/history/2026-08-31.md), "`oid`,
and the two canonical extension names". Everything else the survey turned up
needs a decision this phase has not made yet.

## P10 — Per-row-group column statistics

**Inbox:** [`roadmap-P10-row-group-statistics-inbox.md`](roadmap-P10-row-group-statistics-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

Sketched as pushdown's companion until that grilling separated them. Three
things make it a phase rather than a companion, and the last one also fixes
where it sits in the table above:

- It is the only work here that spans **all four layers**, which
  [`layering.md`](layering.md) calls the sharpest test of its own rules.
- It is the only work here whose bug is a **wrong answer** rather than a slow
  one — see "The correctness asymmetry" below — so it cannot share a review
  cycle with a self-contained query-API change (`../process.md`, "Size a slice
  by its review, not by its scope").
- **It needs an addressing scheme P7 owns.** Statistics attach to row groups,
  the row group is the sparse row index's checkpoint interval, and
  `CopyBlock::sparse_index` is a reserved `None` that
  [`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md) fills. So
  this phase is scheduled after that one; running it earlier means inventing a
  second addressing scheme that P7 then has to reconcile with the one it
  wanted. It is not only an addressing question: this phase's best outcome —
  sortedness plus the sparse index turning a range predicate into a binary
  search for a byte range, below — is *unreachable* without that index, so
  running it first would deliver row-group pruning and leave the payoff that
  motivated the phase on the table.

Reasoning for the split:
[`../status/history/2026-08-29.md`](../status/history/2026-08-29.md), "Statistics
are a phase, not a companion".

Parquet-style statistics, gathered during a scan and persisted in the cache, so
a later query can skip data instead of reading it. Needs typed columns (a
min/max needs a parsed value), pays off in this phase (the pruning consumer),
and already has its cache slot reserved (`CopyBlock::column_stats`, always
`None`).

Starting set, cheapest and most useful first:

- **`null_count`** — nearly free, and directly answers `IS NULL` / `IS NOT NULL`.
- **Sortedness** — a tri-state (`ascending` / `descending` / `unordered`) plus
  NULL placement. One comparison per value, two bits stored.
- **`min_value` / `max_value`** — for collation-independent orderable types only;
  see the trap below.
- **Distinct count** — deliberately *not* in the starting set. It needs a hash set
  or an HLL sketch, which is a different cost class from everything above.

**Attach these to row groups, not to whole `COPY` blocks.** Per-block is the
granularity that suggests itself, but it is close to useless on exactly the
tables big enough to matter: koji's blocks run to billions of rows, and the
min/max of a monotonic `id` column over a whole block spans the entire domain, so
it prunes nothing. Parquet's win comes from row-group granularity, and there is
already a natural unit to reuse — the sparse row index checkpoints every 8192
rows (`docs/design/roadmap-P7-scan-performance.md`). Statistics attach to
those checkpoints; block-level statistics are then just the roll-up, free to
compute and still worth storing for the coarse first pass.

**Sortedness is worth more here than min/max, and costs less.** `pg_dump` emits
rows in physical heap order, and for an append-only table that is very often
ascending by surrogate key — much of koji (build, task and RPM id columns) should
qualify. A column confirmed ascending, combined with the sparse index, turns a
range predicate into a **binary search for a byte range** rather than a
scan-and-prune over row groups. That is a qualitatively better outcome than page
pruning, and it is the reason to put sortedness ahead of min/max rather than
treating it as a nice extra.

Two conditions on it. Sortedness is only a guarantee over a **fully scanned**
block — an incremental scan that stopped partway can honestly say "ascending so
far," which is not something a query may rely on, so the flag has to be tied to
the block's scan watermark rather than set optimistically. And NULL placement
must be recorded, not assumed.

**The correctness asymmetry is the thing to get right.** The standing rule is
that the cache is a best-effort accelerator, never required for correctness: a
stale structural index costs a rescan and nothing else. **Statistics break that
symmetry.** A stale or wrong statistic causes a wrong *answer* — pruning a row
group that does in fact contain matching rows silently drops data, with no error
to notice. So statistics cannot inherit the structural index's relaxed
validation:

- The dump-file identity check (size/mtime) that the structural cache treats as
  advisory — a mismatch is a diagnostic, not a hard failure
  ([`architecture.md`](architecture.md), "The cache") — is **mandatory** before
  any statistic is trusted.
- Statistics must be **discardable independently** of the structural index, so a
  cache written by a version with a stats bug can be downgraded to "structure
  only" rather than thrown away.
- Each entry records the **type it was computed as**. The type mapping will
  keep changing; a min/max computed under an older mapping must not be silently
  reused under a newer one.

**Trap: string min/max is a correctness bug, not an optimization.** PostgreSQL
orders `text` by collation — koji's own header records `LOCALE = 'en_US.UTF-8'` —
while Arrow and DataFusion compare byte-wise. A byte-wise min/max used to prune a
collation-ordered predicate can exclude rows that actually match. So restrict
min/max to types whose ordering is collation-independent: integers, `numeric`,
dates/timestamps, `boolean`, `uuid`. This is what makes the "basic orderable types
such as int" instinct the right starting point rather than merely the easy one.
Floats need an explicit NaN and `-0.0` policy before they join the list — this is
the same footgun that forced Parquet to rework its own float column ordering.
Text min/max stays open only if the collation is recorded and matched.

**Cost: this converts the index pass into a full parse.** `build_index()` today
finds block boundaries and counts rows without ever splitting a field. Statistics
require splitting every field of every row and parsing the tracked ones. koji
measured 243 MB/s at ~33% of one core, so an HDD scan has headroom to absorb it,
but the same scan is CPU-bound on NVMe and there the tax is real. Statistics
gathering should therefore be **opt-in and column-selectable**, not something
`pgdq parse` does by default.

**Cost: the sizing is not negligible.** At 8192-row groups, koji's 19.58B rows
give ~2.4M row groups; at roughly 24 bytes per column per group (min, max,
null_count, flags) and ~10 tracked columns, that is on the order of **half a
gigabyte** of statistics. That is ~0.07% of the 784 GB file — a defensible ratio,
comparable to Parquet's own footer overhead — but it is ~30x the sparse index it
rides on. So the statistics row-group interval should be tunable *independently*
of the sparse index interval; coarsening it to every 64k rows cuts the volume 8x
while still pruning far better than per-block would.

Finally, in incremental mode statistics accumulate as a side effect of scans the
caller asked for anyway, so coverage is naturally partial. The cache must record
which row groups actually have statistics — absent is a normal state, not a
defect.

**A statistic never helps the scan that gathered it, and that is the whole
shape of the payoff.** A query is two passes: the mapping pass walks every row
between `scanned_through` and the target, and only then does replay read the
target block. Statistics are written by the first pass and read by a *later*
query's planning — so the run that pays the parse tax gets nothing back, and
every benefit lands on a subsequent query against a cache that survived. That
puts two things in the frame together whenever this phase's value is argued:
the cache's own lifetime, which pre-1.0 ends at the next format bump
(`architecture.md`, "The cache" — koji's cache was unreadable within days), and
the opt-in-and-column-selectable rule above, which is what keeps a caller who
will never benefit from paying. Reasoning:
[`../status/history/2026-08-29.md`](../status/history/2026-08-29.md), "Pushdown
cannot touch the mapping pass".

## P6 — Embeddable engine story

**Inbox:** [`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

The least-specified phase — the user has explicitly flagged unfamiliarity
with this space, so treat its eventual grilling session as needing real
research (prior art from `object_store`/DataFusion/similar embedded-source
crates), not just architectural taste. Rough shape, informed by the decisions
under "Standing rules" above, made to keep this open:

- Feature-gated `object_store`-backed I/O implementation of the MVP's
  internal byte-range trait, alongside the lightweight local-only default —
  unlocks S3/GCS/Azure and any other `object_store`-supported backend.
- Python bindings (likely `pyo3`), as a new workspace member.
- Apache DataFusion `TableProvider` integration, as a new workspace member —
  the async core and the `Utf8View` column choice were made with this
  destination specifically in mind.
- Apache Spark / Trino integration — order and approach TBD; likely follows
  whatever pattern the DataFusion integration establishes, if applicable.

**Scheduled after the three above**, because it is the phase that *presents* a
surface over mechanisms they are still changing. A `TableProvider` commits to
what the predicate can express and to the I/O layer beneath it; built while
either is in motion, it is built twice. Each phase ahead of it hands it a
settled input instead — the predicate surface from typed predicates, and the
byte-range abstraction with its measured defaults from scan performance — and
this is also the least-specified phase, whose grilling needs real research
rather than architectural taste, so it gains most from going last. Its inbox is
the largest of the five and none of it decays by waiting: the entries are
questions this phase must answer, not evidence that ages.

## P8 — Format coverage beyond plain COPY TEXT

**Inbox:** [`roadmap-P8-format-coverage-inbox.md`](roadmap-P8-format-coverage-inbox.md) — facts earlier
phases filed for this one. Drain it when grilling this phase.

Everything that widens the set of `pg_dump` outputs we can read. Two
independent tracks; A is listed first because it is cheap, not because it
matters more.

**Track A — variants within plain format.** `--inserts` /
`--column-inserts` output, and any other gaps the compatibility matrix in
`docs/design/pg-dump-compatibility.md` turns up as it fills in via the fixture
tooling. Small: a second row-source implementation feeding the same
batch layer, and its target is **already located and attributed** —
`InsertRun::table` and `row_count` come out of the map, so this track adds
parsing, not scanning.

**Not CSV-format `COPY` blocks** — see "Permanent non-goals" above. Track A is
`--inserts`, and nothing else that once sat beside it.

**Track B — archive container formats.** `--format=custom`,
`--format=directory`, and `--format=tar`. Formerly a permanent non-goal;
now planned, because the project's ambition (an embeddable, Arrow-native,
engine-integrated source) puts it in a different class from tools that only
need to read a dump once. Reading a table out of a custom-format archive into
a DataFusion query is squarely in scope for what this library is for.

The work is a **container layer**, not a new parser. All four formats store
table data as the same COPY TEXT payload the MVP already decodes; what
differs is how you find an entry and how you get its bytes:

| Format | How structure is found | Entry bytes |
|---|---|---|
| plain | scan for `COPY ... FROM stdin;` headers | raw, inline |
| custom | parse the `PGDMP` header + TOC | per-entry compressed block |
| directory | read `toc.dat`, one file per entry | per-file compressed |
| tar | read the tar member index + `toc.dat` | uncompressed tar members |

So the layer sits between `ByteRangeSource` (below) and the batch/stream API
(above), and reuses `copy.rs` verbatim. Two things
in it are genuinely new and should be scoped as such when this phase becomes
current:

- **Per-entry streaming decompression** (gzip, and lz4/zstd for PG 16+
  archives). This is why the MVP's "input is already decompressed" rule is
  scoped to plain format rather than stated globally — archives compress
  *internally*, so that rule cannot hold here.
- **Non-seekable positions within an entry.** A raw file offset is not a
  resumable position inside a compressed stream, so `ResumeToken` stays
  opaque and the cache gains an entry-relative addressing mode.

The decisions that keep all of this additive rather than a rewrite are listed
under "Four decisions that keep later phases additive" above.

## Out-of-band work

Small work that belongs to no phase: a CLI ergonomics change, a defect fix
that changes no decision. It gets a number `M<k>` and **one terse ledger line**
— date, what changed, and the history entry that says why — in a table below
the watermark, started again the first time an item lands after a keystone.
Nothing else: no spec (there was no intent doc to write), and no notes doc,
because the history entry *is* the notes. If out-of-band work turns up a fact
an unspecified phase needs, that fact goes in that phase's inbox, as always.

**A number is allocated on admission, not on landing.** An item queued for
later takes its `M<k>` and its row when it is admitted, with the Date column
empty until it lands — so a queued item can be cited by number, and so this
table stays the authority on which numbers are spent. Row order is allocation
order, which is why a queued row may sit above one that landed before it.

**Admission rule.** An item is out-of-band only if it changes no decision any
spec records **and** fits one session. Anything that changes a decision goes
back through grilling → spec amendment → a numbered slice; that rule is what
keeps this section from becoming where design work goes to avoid review.

**Ledger lines stay one line each.** This section is read as an index, never as
an account — the detail lives in the dated history entry it points at. It grows
until a keystone, which strikes it along with the phase docs and leaves a
watermark saying which numbers are spent (`../process.md`, "The out-of-band
ledger is struck too").

**M1–M27 are struck**, and nothing at or below `M27` is reused. That is a
high-water mark rather than a claim that every one of them landed: `M14` was
absorbed into `M17` and `M1`/`M2` were folded into a phase slice, and their
numbers are spent all the same. What each struck item did is filed by
subject — `architecture.md` for a mechanism, `measurements.md` for an apparatus
change, `layering.md` and `../process.md` for a rule — and why it was done is
in the dated history entry it was filed under. The table opens again at `M28`,
the first item to land after this keystone.

| # | Date | What changed | Why |
|---|---|---|---|
| `M28` | 2026-08-31 | `DumpMetadata::types` holds one entry per type, not one per `CREATE TYPE`: a definition replaces an entry of the same name and a shell never replaces a completion (I11), so `TypeKind::Base` is reachable, the reconciliation's restated walk and the library's agree on where `public.mybase` lands, and `pgdq info`'s `user-defined types` counts a completed base type once | [`../status/history/2026-08-31.md`](../status/history/2026-08-31.md) |
| `M29` | 2026-08-31 | `oid` maps to `UInt32` and compares unsigned, with a `public.t_oid` fixture column and an oracle case behind it, and a `uuid`/`json`/`jsonb` column's field carries its canonical Arrow extension name — the three rows of the ADBC floor that need no decision `P12` has not made | [`../status/history/2026-08-31.md`](../status/history/2026-08-31.md) |
| `M30` | 2026-09-01 | an acknowledgement excuses a commit, so one unexamined commit on a path makes every earlier entry on it inert — `--stale` now names an inert entry and what holds the path red, `acknowledged.py` states the rule where an entry is written, and `c614c4b` is acknowledged for the four figures 11.4 re-reddened | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M31` | 2026-09-01 | `deficiencies.py` carries a fourth relation — a `(b)` entry naming a slice and that slice's checklist line naming the entry, reconciled both ways over a new parser for `STATUS.md`'s slice checklists — so a re-slice fails the check rather than owing a re-target on discipline; an unsliced owning phase and a ticked line are the two boundaries where no obligation exists | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M32` | 2026-09-01 | `deficiencies.py`'s pairing, reviewed: a ticked checklist line's `KD<k>` is a citation that resolves against an `<!-- deficiency-watermark: … -->` allocated range rather than the index, a `(b)` entry may not name a ticked slice, and the roadmap's phase index gains a `Complete` state so an entry owned by a finished phase fails instead of going quiet — a stranded entry drops to `(c)` unless a phase absorbs it | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M33` | 2026-09-01 | `deficiencies.py` holds the phase index's other pairing: a phase carrying a `## P<N> progress` checklist is `Current` and a `Current` phase carries one, which closes the wrap-that-half-happened edge `M32` could only write down — the in-flight state was already in the vocabulary and unused, so the `Specified`/`Sliced` split `M32` costed and rejected was the same rule under a new word | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M34` | 2026-09-01 | The comparison oracle asks each pair through two typed *columns* rather than a cast of a `text` parameter, so a case carries the collation its type gives a column instead of the one its input derives — `name` is asked bare and answered under `C`, and `public.text_c` joins the cases as the domain whose collation comes from neither a clause nor `default` | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M35` | 2026-09-01 | The comparison register is asserted against every committed oracle cell over six majors — 28,536 of them, through the same `resolve_term`/`Predicate::matches` path a `--filter` takes — skipping `E`-cells, which record what the server refused rather than how it compares; the `jsonb` cases first grew to one value per branch of `compareJsonbContainers`, so the exception set is met rather than empty and its `{"a": "a"}`/`{"a": "A"}` entry is the string leaf the register knowingly gets wrong | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M36` | 2026-09-01 | An exception in the oracle-answer test's pair list must disagree in **every** major that carries the case and under **all four** asserted operators, rather than somewhere in the walk — the key is the case, so a collation that moves for one major, or a comparator that stops being antisymmetric, leaves the block passing today | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M37` | 2026-09-01 | `Error::PredicateValueDecode` names the form the column's `CompareKind` accepts, not only the value it refused — so `--filter 'flag=true'` is told a `boolean` is written `t` or `f` instead of being told `true` does not parse as a `boolean`, and `interval`, `inet` and `macaddr` are answered by the same edit | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M38` | 2026-09-01 | `accepted_form` renders its `CompareKind`'s payload where the payload is the answer — an enum's declared labels rather than "one of the type's own declared labels", and the scale in `numeric(p,s)`, whose clause reads "as a number, or `NaN`" and so is false for the literal-finer-than-the-scale refusal that arm actually raises | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M39` | 2026-09-01 | `info --verbose` prints an enum column's declared labels on a continuation line beneath it, uncapped and single-quoted, and `--json` carries them as a `labels` field — the build resolved them and showed them to nobody, leaving a user who mistyped a label to learn the type name from `info` and grep the dump for its `CREATE TYPE` | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |
| `M40` | 2026-09-01 | `info --verbose` lists the user-defined types beneath the count that had been their only trace — one line per type, every `TypeKind` arm rendered and the enum arm carrying its labels — and `M39`'s per-column `labels` field leaves `--json`, which had been exporting `metadata.databases[].types[]` in full all along | [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md) |

**One live obligation outlived them.** An `INSERT`-run scan costs
**mid-teens times** a `COPY` scan per byte, CPU-bound, which argues for a scanner-level `INSERT`
path — and *that* changes a decision, so it goes through grilling → spec
amendment → a numbered slice rather than through this section. It is filed in
the scan-performance phase's inbox
([`roadmap-P7-scan-performance-inbox.md`](roadmap-P7-scan-performance-inbox.md))
until then.

## Future — wanted, unscheduled

Work we intend to do without committing it to a phase. An item moves out of
this section when it acquires a phase number, not when it acquires a design.

- **TOC attribution across an intervening statement, so `--disable-triggers`
  dumps stay attributed.** I31 puts `ALTER TABLE … DISABLE TRIGGER ALL;` — and
  a `SET SESSION AUTHORIZATION DEFAULT;` ahead of the first entry — between a
  `-- Data for Name:` block and its data, and no data span in such a file
  carries a TOC entry as a result (deficiency `KD1`). Not a
  correctness hazard and opt-in, which is why it is here rather than scheduled.
  Three coordinated changes: the `Data for` comment must absorb into
  `Mode::Statement`, `push_statement_span`'s `Framing` veto must not eat an
  owned `toc`, and `on_copy_start`'s `Mode::Idle` arm must inherit
  `governing_toc`. The third is why this is a graded slice rather than
  out-of-band work — `architecture.md` states `governing_toc`'s inheritance as
  a decision. The fixture half is a seventh `edge_cases` flag set (`--data-only
  --disable-triggers`), which by I31's scope limit is the only combination that
  emits anything.

- **Collation-aware comparison: the `S`-irrelevant set widened, then the
  remainder deferred to an environment.** Call the server that wrote the dump
  `S` and the environment pgdq runs in `E`. A column that states a collation
  other than `C`/`POSIX` is ordered bytewise today, so `<`/`>` return a row set
  the server would not (deficiency `KD7`). The partition that shapes the fix is
  exhaustive, which is why it is two increments and not a queue of collations:

  **The `S`-irrelevant half is exact and needs no environment.** A collation
  whose ordering reduces to `memcmp` is right on every server, whatever its
  libc, ICU, platform or major (I43). Today the register resolves two stated
  names, `C` and `POSIX` — plus `name`, which reaches the same verdict through
  its type's default rather than through a clause — where I43's set also holds
  `ucs_basic` and anything the file declares under the **builtin** provider,
  whose provider `pg_dump` writes verbatim (I42). Widening it is a parser over `CREATE COLLATION` and a
  register arm — no FFI, no environment, no conditional — and each column it
  reaches moves from an advisory divergence to an exact answer. This is where
  the increment starts, and it is worth landing even if the second half never
  is.

  **The `S`-relevant remainder cannot be closed from the file, at any price.**
  A plain dump carries a collation's name and never its version (I32, I42), and
  the ordering *is* a function of that version — measured, not supposed: ICU 70
  → 76 moves 3.0% of a broad Unicode corpus, and the same locale name under ICU
  versus glibc moves 5,796 of 5,998 strings. So bundling rules for `en_US.utf8`
  is not a cheaper approximation of this work, it is a different and wrong
  answer: it would be a snapshot of one glibc's tables presented with no sign
  that it is one. There is no third source. Either pgdq answers bytewise and
  registers the divergence, as it does now, or it **delegates to a provider in
  `E`** — the same libc and ICU the server would call — and states which one it
  used.

  **Deferral makes the conditional the user's to discharge, which is the whole
  move.** pgdq cannot know `S`'s provider version; a user who owns `S` usually
  can, and can run pgdq in an environment that matches it. That is a recourse
  they can execute — matching an *image* is cheap where restoring the dump into
  a real server is exactly the cost pgdq exists to avoid — and it is the same
  shape as `--database` resolving an ambiguity this build refuses to guess.
  It transitively reaches every collation, ICU included, without pgdq carrying
  a single collation rule. What it does not do is close `KD7` absolutely: the
  verdict such a comparison earns is *agrees with a server*, conditional on the
  provider matching, never the unqualified *agrees, on every server* that the
  `S`-irrelevant set earns. Two things make that honest rather than hopeful —
  `ucol_getVersion()` reproduces `pg_collation.collversion` exactly, so pgdq can
  report the version it computed under, and pgdq can name what a user must
  match.

  **The feasibility is demonstrated, not assumed**, by a spike outside this
  repo (`pgcollate`; see `CLAUDE.local.md` for the path). A zero-dependency Rust
  binary, built once, reproduced real `ORDER BY` byte for byte across seven
  container environments — PG 13–18, glibc and musl, ICU 70/76/78 — over a
  5,998-string corpus for six collations each, with zero mismatches. Its
  constraints, which any spec here inherits: ICU must be `dlopen`'d and its
  symbols are version-suffixed, so the `postgres` binary's own ELF `DT_NEEDED`
  is what names the right soname; a fully static build is foreclosed, which
  costs this project nothing since it already ships glibc-only; the `varstr_cmp`
  tie-break is load-bearing; and `=` is not `cmp() == 0`, since `texteq` never
  consults the collation for a deterministic collation.

  **Unscheduled, and deliberately after P11 rather than before it.** Nothing in
  the current register is shaped against this — new arms split
  `ComparisonDivergence::NonBytewiseCollation` additively, and a collator would
  ride in `ComparisonPlan`, which is already per-column and already carries two
  per-column facts. P11's own collation slices are a **prerequisite** and not a
  duplicate: nothing can defer to a collation it has not read out of the
  preamble. It acquires a phase number when it acquires a design, which is a
  grilling to run after P11 wraps. Reasoning and evidence:
  [`../status/history/2026-09-01.md`](../status/history/2026-09-01.md),
  "Collation splits by whether the answer depends on the source server".

- **CSV-format `COPY` blocks, as part of alternate-format support, post-1.0.**
  `pg_dump` has no CSV mode at all (I13), but `psql` writes `COPY ... WITH
  (FORMAT csv)` and that is valid PostgreSQL, so it is inside the input
  contract — just not built. What it costs is a second field decoder with its
  own quoting rules, plus the fixtures to pin it, and `copy.rs`'s independence
  from its byte source ("Four decisions that keep later phases additive") is
  what makes it a variant rather than a rework. Not to be confused with
  `--format`, which names the archive container and is P8 Track B.

- **A real type-name tokenizer, shared by the preamble grammar and `pgtype`.**
  Today `preamble::extract_type_words` captures a declared type as
  whitespace-delimited words and `parse_ident` dequotes an identifier, so a
  type name needing quotes (I29) is stored dequoted in `TypeDef.name` while the
  declaration that uses it keeps its quotes — the two never compare equal, and
  the column degrades to `Unknown` (deficiency `KD4`). A tokenizer
  that understands quoted identifiers would let the name and the declaration
  agree, and would also let `array_element` decide quoting deliberately rather
  than by the accident that its strip helpers bail on a trailing `"`. Strictly
  additive: it only ever promotes a column that is `Utf8View` today. Not
  scheduled because no `pg_dump` of a database with ordinary type names reaches
  it, and it is the input contract rather than the output that makes it wanted.

- **Caller-supplied type mapping.** Let a caller override the
  PostgreSQL-type→Arrow-type resolution: per column, per declared type, or
  wholesale. Two uses, and the second is the important one. It lets a caller who
  distrusts our mapping substitute their own; and it lets a caller **pin a
  schema** against the drift that progressive type coverage otherwise causes, by mapping everything to a string type and getting the unparsed
  values, CSV-style. That makes it the mitigation named under "Pre-1.0" above.
  `SchemaMode::Strings` is the crude version of this that ships today.

- **The shape-general array representation, as a selectable alternative.**
  `Struct{dims: List<Int32>, lbounds: List<Int32>, elements: List<T>}` is
  lossless for every array PostgreSQL can produce — any dimensionality, any
  lower bound, varying freely from row to row — where the `List<T>` that ships
  today covers only the uniform 1-D case and degrades the rest to `Utf8View`
  ([`architecture.md`](architecture.md), "What the census decides, and who may
  believe it"). `List<T>` was chosen on koji-shaped evidence: short,
  uniform, one-dimensional arrays, where the struct costs +16 bytes per row and
  the loss of `List` as the signal every generic Arrow consumer reads as "this
  is an array". **A schema of matrices or scientific data inverts that
  arithmetic**, and those users should get the struct rather than a string. So
  this is a *knob*, not a replacement: the caller selects which representation
  an array column resolves to. It belongs beside **caller-supplied type
  mapping** above, which is the same knob at a different granularity, and
  adding it breaks nothing — it only ever changes columns that are `Utf8View`
  today, or a shape the caller has told us to represent differently.

- **A diagnostic for a brace run past `MAXDIM`.** `array_out` cannot emit more
  than 6 leading braces (I25), so a longer run means the file is not `pg_dump`
  output — hand-edited, concatenated, or damaged. The census records it and
  resolution deliberately declines to treat it as evidence, so the column keeps
  its optimistic type and the row surfaces as an ordinary `Error::FieldDecode`
  when something reads it. Nothing reports it at *scan* time, so a file nobody
  queries that column of stays silently damaged. The file-level `Diagnostic`
  channel is where this belongs; it is unscheduled because no fixture produces
  the shape.

- **A per-path shape census, so nested arrays get the same treatment as
  top-level ones.** The census records a shape per *column*, which fixes
  a top-level array column's dimensionality exactly and leaves an array
  *inside* a composite (or inside another array's element type) on the
  optimistic path permanently: its shape has nowhere to be recorded, so a
  multi-dimensional or `[lb:ub]`-decorated value there stays a hard
  `FieldDecode` even after `pgdq parse`
  ([`architecture.md`](architecture.md), "What the census decides, and who may
  believe it"). Keying the census by a *path* within the column
  rather than by the column closes that, at the cost of a bigger cache record
  and a per-path walk. Deferred on frequency — a composite with a
  multi-dimensional array field is rare even by that phase's standards — and it
  is purely additive whenever it lands: it only ever converts a hard error into
  a resolved type, so nothing that works before it works differently after.

- **Let a live scan emit rows again, by carrying the map in the resume token.**
  Map-building is separate from row emission
  ([`architecture.md`](architecture.md), "Query: mapping and streaming are
  separate passes"), which costs a second read of the queried block: once to find its extent, once to emit its rows. The
  interleaved form can be recovered without reintroducing the unmapped hole
  that motivated the split, because `ResumeToken` is opaque and valid only
  within the producing process — so it can carry the live segment's in-flight
  spans and its open `CopyStart`, and a resumed stream can splice them in
  rather than re-deriving them. Deliberately deferred: it is an optimization
  over a query path still being iterated on, and it should be revisited once
  the feature set is settled rather than designed around now.
