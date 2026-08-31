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
[`architecture.md`](architecture.md),
"Diagnostics: a file-level channel on `DumpIndex`", and
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
paragraph is [`architecture.md`](architecture.md)'s "One target per query";
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

**Origin.** 2026-08-26. See [`architecture.md`](architecture.md),
"Joining a header against the metadata".

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
cache holds everything up to the last completed block; `stream::table_stream`
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

**Origin.** 2026-08-27. See [`architecture.md`](architecture.md),
"`parse` resumes, and saves as it goes".

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

**`TableStream::ordering_notes()` is what an embedder can read**, and `pgdq
query` prints each of them once on stderr. P11 renames it `comparison_notes`
(equality diverges under a non-deterministic collation too) and gives each note
a *path*, so a divergence inside an array element or a composite field names
its position — read the current method name off the source rather than this
entry.
 It is a **third channel** rather
than a widening of either existing one, and deliberately: the signal is
per-column *and* conditional on a predicate, which makes it L4, while
`DumpIndex.diagnostics` is L1 and `ResolvedSchema.notes` is L2 — so writing it
into either inverts the layering.

**Why P6 cares.** The sink now has **three** channels to reconcile, not two,
and the third is the one whose shape is least settled: a caller has to know to
call `ordering_notes()` and to call it after the schema resolves, which is
exactly the kind of "remember to ask" the sink exists to replace. A sink design
that carries the two older channels and has no place for an L4,
query-conditional note leaves this one as a method an embedder must poll.
Deciding that deliberately is fine; discovering it after the sink ships is not.

**Origin.** P5 grilling, 2026-08-29; landed and widened by `P5.6`, 2026-08-30.
Register: [`architecture.md`](architecture.md), "Ordering operators compare
typed"; evidence:
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

So `OrderingSupport::{Agrees, Diverges, Refused}` is **not** the same
trichotomy as `{Exact, Inexact, Unsupported}`, despite the shapes matching:
`Agrees` → `Exact`, and both `Diverges` and `Refused` → `Unsupported`.
Nothing this project produces is naturally `Inexact`, and a provider that maps
the two registers one-to-one is unsound.

**Why P6 cares.** The provider's per-filter verdict is decided here, the
mapping looks obvious enough to be written without checking, and its failure
mode is a wrong row set with no error.

**Origin.** P11 grilling, 2026-08-31, from the v55 source. The register itself
is [`architecture.md`](architecture.md), "Ordering operators compare typed".

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
