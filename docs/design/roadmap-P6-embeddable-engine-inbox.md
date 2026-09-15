# P6 inbox — facts filed for its grilling

Evidence found in earlier phases that P6 (embeddable engine story) will
need. **This is a queue, not a document**: when P6 is grilled, walk every
entry, fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## There are two diagnostic channels, and the sink is where they unify

**Fact.** A `DumpIndex` carries `diagnostics: Vec<diagnostic::Diagnostic>`
(file-level: tiling failures, cache mtime mismatches, the TOC-coverage figure). A `ResolvedSchema` separately carries `notes:
Vec<resolve::ColumnNote>`, one per column. They are **not** one type, and
cannot be: `DumpIndex` is L1 while `ColumnResolution` is an L2 conclusion
about PostgreSQL type semantics, so a single enum would have L1 name an L2
type. What they share is the `Severity` scale — `diagnostic::Severity`, which
derives `Ord` (`Info < Warning < Error`) precisely so a consumer can filter
both channels against one threshold.

**Why P6 cares.** The P3 spec defers the caller-supplied sink (the
`tracing-subscriber` shape) to P6 and says "a sink can drain this list,
so nothing here forecloses it". That is now load-bearing in a specific way:
**the sink is the designated unification point.** A `Diagnostic` trait that
both types implement is the shape that was left open — deliberately, because
unifying at the storage type is what the layering forbids and unifying at the
drain point is what it permits. A P6 design that gives the sink only one
of the two channels re-opens a question that was already settled the other
way.

**Origin.** 2026-08-24. See
[`decisions.md`](decisions.md),
"The file map and the preamble", and
[`../status/history/2026-08-24.md`](../status/history/2026-08-24.md), "The
diagnostic vocabulary is a scale, not a type".

**Contingent on.** `Severity` staying derivable for `ColumnNote` (it is a
method, not a field, today) and on no third diagnostic producer appearing with
a shape neither channel fits.

---

## A default query cannot detect every ambiguous table, and the API says nothing about it

**Fact.** A query stops mapping once its target is settled
(`stream::target_settled`, the `ScanExtent::UntilTargetSettled` default), so a
second `COPY` block for the same qualified name **past** the stopping point is
never seen and `Error::AmbiguousTable` is not raised for it — the query
returns the candidate it found, silently. The stop rule catches the two shapes
that announce themselves in the prefix: a matching block carrying the
partition-root marker (I2), and a file containing any `\connect` at all
(`pg_dumpall`, concatenation, `--create`). What stays undetectable is a file
whose *first* segment is an ordinary dump with something concatenated after
it — nothing before the stopping point says so. `ScanExtent::Full`, or any
query against an already-`pgdq parse`d file, detects it exactly. Rows are
never a union either way.

**Why P6 cares.** This is the one place where the default query path
returns a *possibly wrong answer with no signal*, rather than an error or a
degraded one — and P6 is where what the embedded API promises a caller
gets decided. Three shapes are open and only P6 can pick: leave the
default as-is and document it; make `ScanExtent::Full` the default and pay a
full scan per cold query; or emit a `Diagnostic` on every early stop saying
the answer is conditional on no concatenation, which costs nothing and turns
silence into a filterable signal. The third interacts directly with the sink
entry above, which is why both are filed here.

**Origin.** 2026-08-24 (the decision), carried through
P3's end-of-phase grilling as an accepted deficiency. It is `KD6`, whose detail
paragraph is [`decisions.md`](decisions.md)'s "D49";
this phase is the destination that entry names.

**Contingent on.** Early stopping surviving as the default, and on no cheaper
concatenation detector turning up — a prefix-visible marker would collapse the
question entirely.

---

## Push mode has no non-test consumer left

**Fact.** `batch::read_table`, the push-mode entry point, is a ~20-line drain
over the pull-mode `stream::table_stream` — it forwards each batch to the
callback and returns `(ResolvedSchema, Option<ResumeToken>)`. Because `pgdq query` is a pull-mode caller (it needs the stream's
`NestedPlan`s while rendering), nothing outside `pgdump_query/tests/` and
`pgdump_query/benches/whole_file.rs` calls it. The shared scan loop is
unaffected and still exercised by every CLI invocation; what has no
non-test caller is the drain itself and the `ControlFlow::Break` →
resume-token path.

**Why P6 cares.** P6 is where a push consumer would appear or fail
to: a DataFusion `TableProvider` pulls, and the Python binding's shape is
undecided. So P6 is the point at which push mode either acquires its
first real caller or is deleted as an API surface kept alive by its own tests.
Pre-1.0 there is no compatibility reason to keep it (`roadmap.md`, "Pre-1.0"),
and no correctness reason either — anything it does, draining the stream does.
Deciding this before the binding is designed avoids designing a binding
*around* a function that should not have survived.

**Origin.** Reviewing 4.4's unattended calls, 2026-08-26. See
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).

**Contingent on.** `read_table` staying a thin drain — if it ever regains
logic of its own, it is no longer free to delete, and the coverage question
comes back with it.

---

## A partial `DumpIndex`'s blocks are each fully censused; the *table's* set may not be

**Fact.** Every `COPY` block in a map carries a complete
array-shape census — a block enters the map only at a `CopyEnd` watermark, and
every mapping pass censuses. `stream::table_stream` exploits this by unioning
the censuses of exactly the blocks it will replay, which needs no completeness
test: it is a claim about the rows being handed back, and nothing outside that
set is emitted. A caller holding the `DumpIndex` directly has no such bound.

**Why P6 cares.** A DataFusion `TableProvider` states a schema *before*
and independently of any scan, and a Python binding will be asked for "the
schema of table T". Both are the reported kind of claim, not the streamed kind,
so both need to decide what to say when the index is partial and a block past
the frontier could disagree: report the optimistic type, refuse, or expose the
partiality to the caller. P9 answers this for the CLI (report, and mark
the coverage); the embedded API has no equivalent place to put a mark.

**Origin.** 2026-08-26, grilling the census's open decisions. See
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).

---

## Machine-readable resolution is keyed by `COPY` block, and block-to-table is left to P6

**Fact.** P9's `--json` export emits one resolution record per `COPY`
block, identified by `(database, qualified name, header_offset)`, and does not
roll blocks up into tables. Per-table is not well-formed today: one table can
span blocks (I2), and a header-less block takes placeholder `column1…N` names
from its first row (`batch::column_names`), so a rollup needs a rule for
disagreeing blocks and for column identity.

**Why P6 cares.** A `TableProvider` has no choice — it must present one
schema per table — so P6 is where that merge rule gets written, and it
cannot be deferred again. P9 deliberately left the question open rather
than guessing at a rollup the embedded API would then have to contradict. A
per-table rollup in the CLI export is purely additive once the rule exists.

**Origin.** 2026-08-26, grilling the census's open decisions. See
[`../status/history/2026-08-26.md`](../status/history/2026-08-26.md).

---

## A reported schema degrades where a streamed one refuses, and the CLI now does both

**Fact.** The same "this database's DDL was never read" condition has two
answers in the tree, deliberately. `stream::resolve_block` raises
`Error::MetadataNotScanned` before a batch exists, because a stream hands back
rows and a wrongly-typed one is a wrong answer with no signal.
`resolve::resolve_columns` marks the column
`ColumnResolution::MetadataNotScanned` and carries on, because a *listing*
covers every block in the index and one unresolvable block must not sink the
document. `pgdq info --json` exports the second form; `pgdq query` gets the
first.

**Why P6 cares.** A `TableProvider`'s `schema()` and a Python binding's
"give me the schema of table T" are the *reported* kind of claim, but the scan
they front is the *streaming* kind — so P6 has to pick which of the two
existing answers an embedder gets, or a third, and it cannot inherit one by
accident. This sharpens the "a partial `DumpIndex`'s blocks are each fully
censused" entry above: that one asks what a schema says about array shape when
the map is partial; this one asks what it says when a whole database's DDL is
missing, and the tree already contains both answers rather than none.

**Origin.** 2026-08-26. See [`decisions.md`](decisions.md),
"D43".

**Narrowed by 9.5.1, 2026-08-27.** The mapping pass now states a database's
DDL at that database's first `COPY` block (I1's recurring boundary), so no
mapping scan produces the condition at all — a block in the map always has its
database covered, cold query and post-`parse` query alike. Both answers stay,
and P6 still has to pick, but the caller who can present the condition is
now an embedder holding metadata from *its own* index, or one carrying a
`ResumeToken` across a re-scan — not an ordinary partial scan.

**Contingent on** `stream::resolve_block` keeping its pre-resolution check —
if that ever moved into `resolve_columns`, the two answers would collapse into
one and the question would be settled by default rather than deliberately.

---

## A scan is cancellable, and the library's answer to a cancelled *query* is an error

**Fact.** `ScanOptions::cancel: Option<Arc<AtomicBool>>` (default `None`) stops
a mapping scan cooperatively — read once per chunk and at every completed
`COPY` block. `stream::map_file` reports it as `MapRun::interrupted` and the
cache holds everything up to the last watermark the save throttle's gate opened
at — not necessarily the last completed block; `stream::table_stream`
instead yields `Error::ScanCancelled { scanned_through }` on its first poll
past the mapping pass, because rows from the blocks a stopped mapping pass
happened to reach are a prefix of the answer with nothing saying so. The CLI
wires the flag for `pgdq parse` only. **The flag is read before the read, not
during it**, so a scan blocked inside `ByteRangeSource::read_range` does not
notice until that read returns; `scan::scan` (and so `index::scan_preamble`)
ignores the flag entirely, on purpose — stopping there could not be told from
reaching the first `COPY` header, and a truncated preamble would be cached as a
complete one.

**Why P6 cares about that part specifically.** Both limits are invisible
on a local file and neither is on `object_store`: a ranged GET against remote
storage can take seconds and can hang, and the preamble prepass — an
uncancellable region today — is the *first* thing a cold query does, and since
9.5.1 the first thing a cold `parse` does too. So the
phase has to decide whether remote I/O gets its own cancellation (a timeout, or
a cancel token passed into the source) rather than inheriting a flag the read
path never checks.

**Why P6 cares.** Both embedding surfaces have their own cancellation
idiom and neither is this flag: a DataFusion `TableProvider`'s stream is
cancelled by **dropping** it mid-poll, and a Python binding's caller expects
`KeyboardInterrupt` to reach the GIL. So P6 has to decide three things it
cannot inherit — whether `ScanOptions::cancel` is exposed at all or stays a CLI
mechanism; what a dropped `TableStream` does to the partial map (today: the
cache holds whatever the last save banked, and nothing is written on drop); and
whether `ScanCancelled` becomes a DataFusion error or is folded into the
"stream ended" path. The cheap default — leave the field public and let an
embedder set it — is also the one that puts an error variant into an engine
that would rather see a stream end.

**Origin.** 2026-08-27. See [`decisions.md`](decisions.md),
"D63".

---

## An embedder gets no signal when an ordering comparison diverges from PostgreSQL

**Fact.** P5 added ordering operators that compare typed, and maintains a
register of every Arrow type such a comparison can land on. Four rows do not
match the server: `Utf8View` from a text type (PostgreSQL orders by collation,
which a plain dump does not record — I32), `Utf8View` from a bare `numeric`
(orders lexicographically), `Utf8View` from every other text-held type
(`interval`, `time with time zone`, `json`/`jsonb`, the network types — each
has a server operator of its own), and `Dictionary` from an enum (PostgreSQL
orders by declaration order).

**`TableStream::comparison_notes()` is what an embedder can read**, and `pgdq
query` prints each of them once on stderr. It is per **term**, not per column:
`=` routes through the same comparison plan the ordering operators do, and most
divergences reach ordering alone, so one column filtered with `<` and `=` can
carry one note. Each note carries a *path*, so a divergence inside an array
element or a composite field names its position. It is a **third channel**
rather than a widening of either existing one, and deliberately: the signal is
per-column *and* conditional on a predicate, which makes it L4, while
`DumpIndex.diagnostics` is L1 and `ResolvedSchema.notes` is L2 — so writing it
into either inverts the layering.

**Why P6 cares.** The sink now has **three** channels to reconcile, not two,
and the third is the one whose shape is least settled: a caller has to know to
call `comparison_notes()` and to call it after the schema resolves, which is
exactly the kind of "remember to ask" the sink exists to replace. A sink design
that carries the two older channels and has no place for an L4,
query-conditional note leaves this one as a method an embedder must poll.
Deciding that deliberately is fine; discovering it after the sink ships is not.

**Origin.** P5 grilling, 2026-08-29; landed and widened by `P5.6`, 2026-08-30.
Register: [`decisions.md`](decisions.md), "D55"; evidence:
[`../status/history/2026-08-29.md`](../status/history/2026-08-29.md) and
register entries I32 and I33.

---

## A divergent comparison must be reported `Unsupported`, never `Inexact`

**Fact.** DataFusion v55's `TableProviderFilterPushDown` has three values, and
`Inexact` makes a specific promise: "the provider might still return some
tuples that do not pass the filter" — a **superset**, which DataFusion then
re-filters above the scan
(`datafusion/expr/src/table_source.rs`, v55.0.0).

A comparison this project classifies as *divergent* does not satisfy that. A
bytewise-collated `<` on a `text` column, or an enum compared by label text,
can **omit** a row that PostgreSQL's own operator would have passed — so
pushing it down as `Inexact` returns a silently wrong answer, not a
conservative one. Divergent maps to `Unsupported`.

So the register's three answers — a `ComparisonPlan` that agrees, one that
diverges, and `Refused` — are **not** the same trichotomy as
`{Exact, Inexact, Unsupported}`, despite the shapes matching: agreement →
`Exact`, and both divergence and refusal → `Unsupported`.
Nothing this project produces is naturally `Inexact`, and a provider that maps
the two registers one-to-one is unsound.

**Why P6 cares.** The provider's per-filter verdict is decided here, the
mapping looks obvious enough to be written without checking, and its failure
mode is a wrong row set with no error.

**Origin.** P11 grilling, 2026-08-31, from the v55 source. The register itself
is [`decisions.md`](decisions.md), "D55".

Two further facts about the mapping, from the v55 source:

- **Struct equality against a literal is coerced by field *name*** in
  DataFusion (`datafusion/sqllogictest/test_files/struct.slt`: `s = {y: 2, x:
  1}` matches `{x: 1, y: 2}`), where PostgreSQL's `record_eq` is positional. It
  costs nothing here because a composite's fields are always built in
  declaration order, which is also the order the dump writes them — but a
  provider that reorders or name-matches would be answering a different
  question from the one pgdq answers.
- **`List` columns already have full `=`/`<`/`<=`/`>`/`>=` in v55**,
  element-wise and lexicographic
  (`datafusion/sqllogictest/test_files/array_query.slt`), so nothing about
  DataFusion obstructs pushing a nested comparison down. Whether it *should* be
  pushed down is the `Unsupported`/`Exact` question above, per element type.

---

## The floor is checkable, and stating it is P6's to do

**Fact.** P12 built the mechanism that makes *"the Arrow schema you get is at
least as good as the ADBC PostgreSQL driver's"* a checked claim rather than an
aspiration: `fixtures/<13–18>/adbc/floor.tsv` records what
`adbc_driver_postgresql` 1.12.0 answers for every declarable `pg_catalog` type,
`scripts/floor_mapping.py` joins it against `builtin_scalar` and fails in both
directions, a floor pass of `generate_fixtures.py` ends by running it, and the
three rows outside the rule each carry a written stance. **Nothing about it is
in the manual**, deliberately: the promise is embedder-facing and there is no
embedder-facing surface to read it against.

**Why P6 cares.** P6 is that surface, so P6 is where the promise either gets
made or does not — and the decision is now a documentation choice rather than
an engineering one, since the evidence exists and regenerates itself. Two
things bear on it: the floor is a *release* (`apache-arrow-adbc-24`), so a
published promise inherits the obligation to say which release it is about and
to re-take the sweep when the pin moves; and `money` is deliberately below it
(`KD13`), so any wording has to be "at least as good, with one named
exception" rather than an unqualified claim.

**Origin.** P12, 2026-09-03. The mechanism is
[`decisions.md`](decisions.md), "D38"; contingent on the pin, since a driver release that answers a type
differently changes what the promise would say.

---

## Two-thirds of a typed `pgdq query` is the CLI, so no published figure describes what an embedder pays

**Fact.** Every `query` figure in [`measurements.md`](measurements.md) times
`pgdq query … >/dev/null`, and a sampling profile of that command puts
`pgdq::print_batch` — the CLI turning each `RecordBatch` back into TSV — at
**25.75%** of a `strings` query's user time and **36.42%** of a typed one's,
and it was 34.0% and 62.8% when this entry was filed — the render path has since
been reworked three times and the library is now the larger bucket in both
modes. An embedder consuming `RecordBatch`es pays none of it. Measured on the
3.00 GiB control: the library's own typed extraction is **3.10 µs a row against
`strings`'s 2.42** — a factor of 1.3, where the wall times a reader would quote
now show 1.38 and once showed 2.4. **Read every one of those numbers off the
budget when P6 comes up rather than off this entry**, which is the point of the
entry rather than a caveat on it: the pair was 4.06 and 2.48 when it was filed.
The same figures also
all run `--dqcache none`, so each one contains a full mapping pass and reads the
file exactly twice (2.0000× its bytes, counted with `strace`); an embedder with
a cache reads it once. Both are in
[`decisions.md`](decisions.md), "D29".

**Why P6 cares.** P6 is the first surface with an audience that consumes
batches rather than text, so it is the first place anyone will ask "how fast is
it" and mean the library. Quoting `measurements.md` at that audience overstates
the typed cost by roughly 2.5× and understates how much a warm cache buys. The
decision P6 has to make is whether it states a performance claim at all and, if
so, against what instrument.

**The instrument was deliberately not built, which narrows the question rather
than leaving it open.** A figure that stops at the batch would need either a
published bench through push mode — whose only caller is tests, and whose fate
is the entry above — or a measurement-only flag in the shipped binary, against
a doc whose standing position is that a figure here is a CLI figure. So the
library's cost is kept as *proportions* instead:
[`decisions.md`](decisions.md), "D29", splits
a control row four ways in each mode, and a change to a timed path re-reads it
in the same change that re-takes a figure. **That is what P6 has to read a claim
off**, and it is a table of profile shares rather than medians — so a claim
built on it is a ratio or a per-row cost, never a throughput with an apparatus
line behind it. If P6 wants the latter, building the instrument is P6's work and
not inherited.

**And the allocator is the embedder's, not ours, which sharpens what a claim
may say.** `pgdq` links the platform allocator by its own choice — a
`#[global_allocator]` in `pgdump_query` would impose one on every embedder — so
an embedder's numbers are under whatever their binary chose, and that is
measurably not the same number — though by less than this entry once said. On
the three headline shapes `jemalloc` now reads 1.02×/1.12×/1.10× against the
platform allocator and `mimalloc` 0.97×/0.97×/0.99×, where the first sitting
read 1.87×/1.19×/1.07× and 0.98×/0.96×/0.96×; the largest cell there was
measuring an allocation in the read path that has since gone
([`measurements.md`](measurements.md), "Which allocator a figure was taken
under"). So the per-row cost P6 would quote is still allocator-conditional and
a claim has to name the allocator it was taken under — but the honest spread is
now single-digit percent rather than a factor, which is a weaker caveat than
the one filed. Origin: the allocator reading, 2026-09-03.

**Origin.** The scan-performance decomposition, 2026-09-03
([`../status/history/2026-09-03.md`](../status/history/2026-09-03.md)),
narrowed the same day when the maintainer settled that library-only *figures*
are not required so long as library-only performance stays easy to understand —
the expectation being that the library numbers are the more stable of the two
and the ones this phase's audience actually needs.
Contingent on the CLI's output path: a cheaper `print_batch` moves the
proportion without moving what the library costs.

---

## The partitioned scan surface exists, and it was shaped against `TableProvider::scan`

**Fact.** `table_stream_partitions` splits `TableStream` into N sub-streams
over a complete map, each internally in file order, and the library hands those
out rather than merging them — `pgdq query` does its own k-way merge on source offset when a human
wants file order. Running the partitions sequentially *is* the serial path, so
the parallelism knob's "off" setting is not a second implementation. The
library defaults to `Parallelism::Serial` and adds no `rt-multi-thread`
feature: workers go through `tokio::task::spawn_blocking`, so the embedder's
runtime flavour stays the embedder's choice and the caller's runtime governs
how much parallelism exists.

**Why this phase cares.** This is the shape a `TableProvider` consumes
directly, and it was chosen on the argument that an engine partitions by
construction — so the question this phase inherits is no longer "how do we
parallelize for DataFusion" but "does DataFusion's partition model actually
line up with `COPY`-block-derived partitions, and what does it want when the
answer is one partition". Two known mismatches to check against real
DataFusion rather than against the sketch: the partition count is decided by
the *file* (block extents, and below them the compressed source's blocks) where
an engine usually asks for a count it chose; and a partition's row count is
unknown until it is read unless its block was gathered, and then only per row
group (`statistics::RowGroup`), a byte range a partition's cuts need not
follow.

**One public method exists for the merge alone**: `TableStream::batch_source_offset`,
the offset of the batch just yielded's first row, which is what `pgdq query`
sorts one-batch-per-partition on. An engine that schedules the partitions
itself never needs it — it is the same shape of question as `read_table`
having no non-test caller, and it comes to this phase for the same reason: a
surface kept for one consumer is a surface this phase decides whether the
embedded API promises.

**Why the ordering still holds.** This phase's reason for going last is that it
presents surfaces over mechanisms that are still moving. The one beneath it is
now settled — the byte source's shape, its partitioning advisory, and what a
parallel read promises in bytes — which is one fewer moving part rather than a
reason to bring this phase forward.

**Origin.** The parallel-scan work's grilling, 2026-09-06; the shipped
mechanisms are [`decisions.md`](decisions.md), "D51" and
"I/O, memory and parallelism".

---

## Reproducible error ordering across sub-streams is the *caller's*, and only `pgdq query` has it

**Fact.** When several sub-streams of one partitioned replay fail, which
failure the user sees is decided entirely by the code driving them. `pgdq query`
arranges the file's answer: it records a sub-stream's failure instead of
raising it, marks every sub-stream at or after it dead, drains the ones before
it, and raises the lowest-indexed error — index order being file order, since
sub-stream `k` reads a contiguous run of blocks after `k-1`'s. The **library
does none of that**. `table_stream_partitions` hands out N independent streams
and says nothing about their failures' relative order, so an embedder that runs
them concurrently and propagates the first error it observes gets a different
row named on each run over an unchanged file.

**Why this phase cares.** It is the same question `batch_source_offset` raises
one line up, with a sharper edge: file order is a *nicety* an engine may not
want, but a reproducible error message is something a caller would assume it
has and cannot get from this API. Three answers are open and this phase picks
one — promise nothing and document it; publish the ordering key the CLI uses so
a caller can implement the rule; or offer a merged, ordered entry point beside
the partitioned one for callers that want the CLI's behaviour. Note that the
mapping pass's own workers *do* order their errors inside the library
(`leader::run_region` drains a window in file order), so the asymmetry is
between the two passes rather than a blanket "the library does not order
errors".

**Origin.** The parallel-scan work's error-ordering slice, 2026-09-07
([`decisions.md`](decisions.md), "D52").

---

## A partitioned replay cannot be resumed, and the token says so rather than lying

**Fact.** `table_stream_partitions` takes no `resume` argument, and a
sub-stream's `ResumeToken` is stamped with the partition it came from — so
feeding one back to `table_stream` is `Error::ResumeQueryMismatch` rather than
a silent replay of every matching row from that offset onward. A token carries
a file offset and nothing about the range its stream was confined to, and the
ranges are decided per call from the caller's `Parallelism` and the source's
own advice, so nothing about a partitioned run is reconstructible from a token
alone ([`decisions.md`](decisions.md), "D51").

**Why this phase cares.** Resume is part of what the embedded API promises,
and it is currently a whole-stream promise. An engine that pulls partitions
and wants to stop and continue needs either a token that carries its own
partition's extent, or a way to ask for the same split twice — and which of
those is right is a question about what the embedded surface guarantees, not
about the replay. It is worth deciding before the surface is frozen, because
`ResumeToken` is opaque today and adding a field to it is free, where changing
what a token means after an embedder holds one is not.

**Origin.** The parallel-scan work's partitioned-replay slice, 2026-09-07
([`decisions.md`](decisions.md), "D51").

---

## `Parallelism::discover()` is a single-tenant assumption, and a `TableProvider` is not single-tenant

**Fact.** Read from `/mnt/wd12t/upstream/datafusion/origin-main` at `ccfe40b18`
(workspace `55.0.0`), 2026-09-10.

- **A join across three registered tables is three provider objects and three
  `scan()` calls.** `register_table` stores the `Arc` and `SchemaProvider::table()`
  hands back a clone, so the object registered is the object used; physical
  planning calls `scan_with_args` once per `TableScan` node
  (`datafusion/core/src/physical_planner.rs:609`). A self-join is one object and
  two calls. A provider served through a *dynamic* schema provider — which is how
  `datafusion-cli` answers `SELECT * FROM 'file.parquet'` — is built fresh per
  query (`datafusion/catalog/src/dynamic_file/catalog.rs:135-144`).
- **DataFusion never sizes a memory pool from the machine or from a cgroup.**
  `RuntimeEnvBuilder::build()` defaults to `UnboundedMemoryPool`
  (`datafusion/execution/src/runtime_env.rs:494`); there is no `MemAvailable`,
  `cgroup` or `sysinfo` read anywhere in `datafusion/` or `datafusion-cli/`. A
  bound exists only where the embedder set one — `datafusion-cli -m/--memory-limit`
  with `--mem-pool-type greedy|fair`, or `SET datafusion.runtime.memory_limit`.
- **The pool is reachable from a custom provider but explicitly does not cover
  it.** `Session::runtime_env()` in `scan()` and `TaskContext::memory_pool()` in
  `execute()` both reach it, and `MemoryPool::memory_limit()` answers
  `Infinite`/`Finite(n)`/`Unknown`. But the pool's own contract
  (`datafusion/execution/src/memory_pool/mod.rs:42-186`) says it "does NOT track
  and limit memory used internally by other operators such as `DataSourceExec`"
  and that "operators should not reserve memory for the batches they produce".
  **No built-in read path registers** — Parquet, CSV, JSON, Arrow, Avro and
  `ListingTable` register nothing; the only `MemoryConsumer` in any datasource
  crate is the Parquet *writer* (`datasource-parquet/src/sink.rs`). What registers
  is sorts, aggregates, `RepartitionExec` and `BufferExec`.
- **The worker count is pulled, not pushed.** A provider reads
  `state.config().target_partitions()` — default `available_parallelism()`
  (`datafusion/common/src/config.rs:853`) — and declares what it chose through
  `output_partitioning()`. `ExecutionPlan::repartitioned` defaults to `Ok(None)`,
  so a declared count stands. `datafusion-cli` has **no parallelism flag**; the
  count comes from `DATAFUSION_EXECUTION_TARGET_PARTITIONS` or `SET`.

**Why P6 cares.** `Parallelism::discover_for(jobs, memory)`, given a source's
recommendation, resolves a budget of at most the process's cgroup limit less the
384 MiB reserve, or at most half of `MemAvailable` where no limit is found;
`Parallelism::discover()` passes `available_parallelism()` as the count and no
recommendation, which lands on the 64 MiB default budget instead. Both readings
are about *the process*, and a `TableProvider` is one tenant of it. Three
providers each calling `discover_for` with a compressed source's recommendation
on a three-file join could budget three times the limit less the reserve, or
150% of the memory an unlimited host reports free, and ask for three times its
CPUs, and nothing in DataFusion would stop them: the default pool is unbounded,
and even a bounded one neither tracks sources nor tells one what its share is.

The two halves have different answers, which is the part not to re-derive:

- **The count is solved and needs no new API.** `target_partitions` is what
  DataFusion wants the provider to honour, and `discover_for(jobs, memory)`
  already takes the count as a parameter rather than reading it. A
  provider passes `target_partitions` in.
- **The budget has no reading to take.** `memory_limit()` reports the whole
  session's pool, shared with every other operator and explicitly not covering
  sources, so any division of it is a guess. **P6 has a real design question
  here, not an obvious answer** — see the convention below, which rules out the
  first thing to reach for.

**How a source is idiomatically configured: counts, pulled, never bytes.** Asked
separately, and the answer is uniform across the tree.

- **A memory-shaped constructor parameter has no precedent.** Every `with_*` on
  `ListingOptions`, `ParquetSource`, `CsvSource`, `MemTable`, `StreamConfig` and
  the `datafusion-examples` custom providers was enumerated; not one takes a byte
  budget. `ParquetSource::with_metadata_size_hint` is an I/O read size by its own
  doc, not a budget. `MemTable` — which holds everything in RAM — takes a
  partition count and no memory parameter at all.
- **Nor is the count a constructor parameter.** The idiom is to take *nothing* at
  construction and pull `state.config().target_partitions()` inside `scan()` and
  `context.session_config().batch_size()` (rows, default 8192) inside `execute()`.
  `ListingOptions` deliberately has no `target_partitions` field. The payoff is
  that an embedder's existing `SET` reaches our library for free.
- **Bytes are derived from counts, never the reverse.** The one source-side byte
  computation in the tree divides: `FileGroupPartitioner::repartition_evenly_by_size`
  does `total_size.div_ceil(target_partitions)` (`datasource/src/file_groups.rs:216-220`),
  where the byte figure is a floor below which not to split. `FairSpillPool`
  divides a fixed total by a live consumer count. **No partition or worker count
  is derived from a byte budget anywhere in the tree**, which is the opposite of
  the direction our rule composes in.
- **The documented advice matches**: `docs/source/user-guide/configs.md:281-303`
  tells a user under a tight memory limit to *lower `target_partitions` and
  `batch_size`* — the budget is fixed on the runtime and the count is the knob
  that fits it. The same file (268-270) states that `repartition_file_min_size`
  "does not apply to user defined data sources".
- **The one in-tree pattern for handing a source a byte budget is a bounded
  object, not a number.** `CacheManagerConfig::metadata_cache_limit` (50 MiB
  default) is set once on the `RuntimeEnv`, and the Parquet source consumes it as
  an opaque handle via `runtime_env().cache_manager.get_file_metadata_cache()`.
  That is the closest precedent to what our pools would need, and it is worth
  weighing against a constructor parameter when P6 is grilled.

What this does *not* require is a change to the shipped default. The library's own
default is `Serial` and `discover()` is opt-in, so nothing oversubscribes unless an
embedder asks it to. The gap is that nothing says `discover()` assumes it is the
only tenant. That is written beside the mechanism as a **property with a remedy
in hand** rather than a deficiency
([`decisions.md`](decisions.md), "I/O, memory and parallelism").

**Origin.** 2026-09-10, grilling the status line's provenance entry under `STATUS.md`'s
"Decisions worth another look" — the maintainer asked what a three-file join would
budget. See [`../status/history/2026-09-10.md`](../status/history/2026-09-10.md),
"The provenance call is affirmed, and `discover()` is single-tenant".

**Contingent on.** DataFusion 55's `MemoryPool` contract continuing to exclude
data sources, and `target_partitions` remaining pull-style. Re-check both at the
version P6 actually targets.

## `bytes::Bytes` is in the trait's signature and the library does not re-export it

**Fact.** `ByteRangeSource::read_range` names `bytes::Bytes` in its signature,
and `pgdump_query` re-exports neither the type nor the crate. Anything outside
this workspace that implements the trait therefore has to take a direct
dependency on `bytes` and keep its version in step with ours. The CLI already
pays it: `bytes` is a **dev**-dependency there purely so a test double can be
written.

**Why this phase cares.** P6 is the phase where an embedder — rather than one
of our own tests — implements a source. Re-exporting `bytes::Bytes` from
`pgdump_query` is the other answer and costs one line; the reason it was not
taken when the source's worker default landed is that a dev-dependency was enough for a test, which is not
the case a public trait is for. Decide it when the embeddable surface is
specified, alongside whatever else the crate re-exports.

**Origin.** The source's own worker default, 2026-09-09. The trait's
current shape is [`decisions.md`](decisions.md), "I/O, memory and parallelism".
