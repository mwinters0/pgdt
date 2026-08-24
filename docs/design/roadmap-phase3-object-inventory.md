# Phase 3 — Full DDL object inventory

The binding specification for Phase 3: what it does and why. Indexed from
[`roadmap.md`](roadmap.md), whose "Phase 3" section is the origin sketch and
is kept for the reasoning it carries; where the two disagree, this doc wins.
How it lands in code goes in `roadmap-phase3-object-inventory-notes.md`, not
here.

**Status: in specification.** Sections below are written as they are settled.

## What this phase is for

A second use case, distinct from the embeddable-query-source goal the rest of
the roadmap is organized around: **a sysadmin or engineer handed a dump file
of unknown origin, wanting to understand what it needs before loading it
anywhere.** Concretely — which roles must exist on the restore target, which
tablespaces, what objects the file contains, and whether any of it is
something we don't understand.

Its exit criterion is the **full file map**: an ordered set of spans that
tiles the file, every byte belonging to exactly one span, spans summing to
the file size, none overlapping, each attributed to something.

## Scanning: one statement-driven pass, TOC comments as an enrichment layer

**Decision.** A single pass parses DDL statements directly. TOC header
comments (I3) are read as an *enrichment layer* over that pass, never as its
only source of structure.

The pass is statement-driven because a `pg_dump`-compatible dump from
elsewhere in the ecosystem may carry no TOC comments at all, and a map that
silently produces nothing on such a file is worse than one that produces a
coarser map. TOC comments are read because three things are genuinely only
available there.

What the TOC layer contributes, and what is lost without it:

| TOC contributes | Without it |
|---|---|
| **Owner for objects that emit no `OWNER TO`** — 81% of koji's entries (indexes, constraints, ACL, defaults, sequence-sets inherit ownership rather than setting it) | Owner known only for independently-owned objects. Role *discovery* is unaffected; "which objects does role X own" is not answerable |
| **A kind label for statements the grammar doesn't recognize** — `POLICY`, `TRANSFORM`, `USER MAPPING`, … | Those spans classify as unknown rather than known-but-unhandled |
| **Grouping** — a definition and its trailing `ALTER … OWNER TO` are one TOC entry | They tile as two adjacent spans instead of one |
| **Kind for text-ambiguous statements** — `ALTER TABLE … ADD CONSTRAINT` is `CONSTRAINT` / `FK CONSTRAINT` / `CHECK CONSTRAINT` | Recoverable by parsing the constraint body; the comment is just cheaper |

Tiling holds either way, so this is graceful degradation rather than a second
code path, and there is no fallback implementation to defer.

**Choosing TOC-driven segmentation as the primary structure was considered and
rejected.** Its appeal is a closed, upstream-verifiable vocabulary (~63 `Type:`
values — see the invariant register) and free tiling boundaries. It fails on
input that isn't `pg_dump`'s own archiver, and I3's own caveat — a
dollar-quoted function body can contain a line that looks like a TOC header —
means the statement grammar has to corroborate regardless.

**This is not a performance decision.** Both approaches are line-oriented
passes over the same bytes; on koji that is 154KB of DDL against 784GB of
data, unmeasurable either way. Bulk-region skipping — the one place cost is
real — does not depend on TOC comments either; see "Bulk regions" below for
how each region's end is found.

**TOC coverage is recorded per file**, as the count of spans **attributed to a
TOC entry** against the number of spans produced. A file with zero of them is
a normal, reported state — the signal that the map is running degraded — not
an error and not a silent fall-through.

Attribution, not header-bearing: a span whose statement is a follow-on of the
object before it (`ALTER … OWNER TO`, `ALTER TEXT SEARCH CONFIGURATION … ADD
MAPPING FOR`, `ALTER EVENT TRIGGER … DISABLE`) inherits the governing entry's
header and counts as covered. Counting only header-*bearing* spans reports
~50% on a healthy, fully-TOC'd `pg_dump` file, which cannot distinguish the
degraded case the figure exists to name — see "Span boundaries" below.

Evidence: [`../status/history/2026-08-23.md`](../status/history/2026-08-23.md)
and [`../status/history/2026-08-24.md`](../status/history/2026-08-24.md).

## What a span carries

**Decision.** Kind, name, schema, owner, byte span, and the raw statement
text — plus a **modelled cross-reference set**, accumulated during the scan
and cached: **referenced roles and referenced tablespaces, and nothing else.**

The object model stays read-side: a classification plus raw text, not a
write-compatible representation. Ordering/dependency fidelity, exact
comment/whitespace preservation, and OID stability are writer concerns the
sysadmin use case doesn't need. Storing the raw text is cheap and doesn't
foreclose a future writer, but round-trip fidelity does not shape this design.

The cross-reference set is modelled rather than derived at display time
because the whole point is answering "which roles does this dump need" without
rescanning the file — a grep over raw text at display time isn't cacheable.
It is limited to roles and tablespaces because modelling per-kind detail
(index definitions, constraint expressions, function signatures) is the
write-compatible representation this phase rules out.

Roles reach the set from three places, and koji needs all three: the TOC
comment's `Owner:` field, `ALTER … OWNER TO`, and `GRANT`/`REVOKE`/`ALTER
DEFAULT PRIVILEGES FOR ROLE`. koji's `backup` role is reachable *only* through
post-data `GRANT` statements — the concrete case for scanning past the first
`COPY` block. `PUBLIC` is a pseudo-role and is never reported as one.

Tablespaces reach it from two: the TOC comment's optional `; Tablespace: <name>`
suffix, which `_printTocEntry()` appends whenever the entry has a non-default
tablespace and `--no-tablespaces` was not given, and the `SET default_tablespace
= …;` statements `_selectTablespace()` emits ahead of a definition. `pg_default`
is the implicit default and is never reported.

Both sets are stored flat and per-file; a per-database view is a filter over
each span's `database` attribution, not a second stored structure.

## Bulk regions: one span kind, three producers

**Decision.** A `Data` span kind covers every bulk region — `COPY` blocks,
the `BLOBS` large-object region, and `INSERT` runs — each recognized at its
opening line and skipped to the end of the region rather than walked
statement by statement.

Every gap between `COPY` blocks being schema-sized is what makes the scan
cheap, and there are exactly two exceptions:

- **Large objects** (I12) — hex encoding roughly doubles the objects' size, so
  the region can run to hundreds of gigabytes. Contents are never parsed;
  there is nothing in them a query engine wants. Per-object subdivision is
  deferred: the only identity the region carries is the OID in each
  `lo_create('<oid>')` opener, and recovering it costs a walk of the whole
  region, which is exactly what one span avoids paying.
- **`--inserts` / `--column-inserts` output**, which contains **zero** `COPY`
  blocks — so "the gap between `COPY` blocks" is the entire file. A
  koji-scale `--inserts` dump is ~1TB of `INSERT INTO` lines. Without this
  span kind the scan would walk every one of them, making the phase's cost
  claim false for an input the fixture tooling already generates.

**One span *kind*, three payload shapes.** `SpanBody::Data` carries a
`DataBlock` enum — `Copy(CopyBlock)` / `InsertRun` / `LargeObjects` — rather
than three sibling `SpanBody` variants. The three genuinely do not share a
shape (only `CopyBlock` carries the inner offsets a row reader seeks by, per
"`COPY` blocks are the one exception" below), but two call sites want the
predicate *is this span bulk row data* without caring which:
`crate::map`'s text suppression and `crate::preamble`'s derived-view walk.
Flattening turns each into a three-arm match that a future fourth bulk
producer would silently miss, and it would change `blocks()`/`blocks_for`'s
`&CopyBlock` signature for no gain.

Treating all three as one kind generalizes the large-object fast path instead
of adding a second special case, and makes the cost claim unconditional: the
scan walks only schema-sized regions, for every `pg_dump` output rather than
for koji-shaped ones. It also front-loads what Phase 8 Track A needs — an
`INSERT` run already located and attributed to a table, so that phase adds a
row parser, not a scanner.

### The three regions do not share an end marker

They are one span kind, but finding where each *ends* costs differently, and
only two of the three have an invariant behind them:

| Region | End marker | Guarantee |
|---|---|---|
| `COPY` block | `\.` alone on a line | I7 — `LF 5C 2E` cannot occur at the start of a data line |
| `BLOBS` | `COMMIT;` | I12 — the payload is a bytea hex literal, which cannot contain a line break |
| `INSERT` run | next TOC header | **none** — see below |

**A `BLOBS` region can be more than one `BEGIN;`/`COMMIT;` pair.** Confirmed
by slice 3.1's fixture (I12): v13-16 always emit exactly one `BLOBS` entry
covering every large object in the database, but v17+ gives each large
object its own `BLOB METADATA`/`BLOBS` entry pair. Since this phase treats
"the large-object region" as one `Data` span regardless of major version,
finding the end of *the region* on v17+ means walking past every
consecutive `BLOB METADATA`/`BLOBS` TOC header, closing at each one's own
`COMMIT;`, until a TOC header of neither kind is reached — not stopping at
the first `COMMIT;` the way `COPY` does at its first `\.`.

**`INSERT` runs need a string-aware scan, not a line-anchored check.** A
`pg_dump --inserts` value is a single-quoted SQL literal, and a value carrying
a newline puts the rest of its statement on the next physical line, which
begins `');` rather than `INSERT INTO`. So the run's end is found by tracking
`'` (with `''` doubling; `standard_conforming_strings = on` means there are no
backslash escapes) to locate real statement ends, then closing at the next TOC
header. That is a `memchr`-class pass over the region rather than a memcmp per
line — an order cheaper than parsing statements, and unconditionally correct,
which is what the single-`Data`-kind argument depends on. Phase 8 Track A needs
the same string-aware splitter to read `INSERT` rows at all, so this is not
work that phase repeats.

Closing an `INSERT` run at the next line-anchored `--` instead was rejected: a
value containing a newline followed by `--` breaks it, and the difference never
shows up at fixture scale, so the unsoundness would ship untested.

An `INSERT`-format dump also emits a `TABLE DATA` TOC header for a table with
**zero** rows, so a data span containing no data at all is a shape the map
handles, not an anomaly.

## The map is the structure, not a description of it

**Decision.** `DumpIndex` grows an ordered `spans: Vec<Span>` as its **primary**
structure. `Span::Data` holds a `CopyBlock` **inline**; `blocks()` becomes a
derived iterator and `blocks_for` a filter over it. `CopyBlock` itself is
unchanged, so Phase 7's reserved `sparse_index` and Phase 5's `column_stats`
are unaffected.

The alternative the roadmap sketch leaned toward — `spans` alongside `blocks`,
with `Span::CopyData { block: usize }` indexing into it — was rejected because
it gives the same byte offsets two owners. "Spans sum to the file size" could
then be true while `blocks` disagreed, which is precisely the failure the
tiling invariant exists to catch. The migration cost is a filter change in
`stream.rs`'s segment planner, not a redesign.

The cache format version bumps; pre-1.0 that is free.

### The span is the container; `DumpMetadata` becomes a derived view

**Decision.** Parsed content lives **in the span**, once. `DumpMetadata` /
`DatabaseMetadata` survive as **derived view types** — built by filtering
already-parsed spans, never by re-parsing raw text and never stored — so
nothing is held in two places and nothing is thrown away to be re-derived.

Shape: common fields on the span, per-kind payload in a body enum.

```
struct Span { start, end, database, toc: Option<TocHeader>, text: …, body: SpanBody }
enum SpanBody { Table{columns}, TypeDef{…}, Extension{…}, Data(CopyBlock),
                Framing{…}, Unparsed, Unscanned }
```

`SpanBody`'s vocabulary is small and is *not* the TOC's ~63 kinds; those are a
label carried on `TocHeader` and attached to `Unparsed` spans, which is what
distinguishes known-but-unhandled from unrecognized.

**L2 is untouched.** `resolve.rs` only ever reads `metadata.databases` and
filters it, so preserving `DumpMetadata`'s shape as a view keeps every L2
signature identical; only `preamble.rs` (producer), `index.rs`, and `cache.rs`
change. The view is built once when a `DumpIndex` is produced or loaded and
memoized as `#[serde(skip)]`, the same treatment `diagnostics` gets — so it
costs nothing per query and, being derived, cannot diverge from the spans.

Storing the parse in a separate `DumpMetadata` *and* raw text in spans was
rejected as holding one fact in two places; storing only raw text and
re-parsing per query was rejected as throwing away work the scan already did.

## Scan coverage is a prefix, expressed as a span

**Decision.** Bytes no scan has walked are covered by an explicit `Unscanned`
span, so **every** `DumpIndex` tiles the file — a partial one included. The
tiling test therefore has no exemption for incremental scans, and exemptions
are how invariants rot.

**Coverage is a prefix, by construction.** `table_stream` never jumps forward
over unread bytes: a live segment always starts at `scanned_through` and walks
contiguously from there (see "Mapping and streaming are separate passes"), so
there is exactly one `Unscanned` span and it is always trailing —
informationally identical to `scanned_through`.

The structure is nonetheless a span list rather than a watermark field,
because prefix-ness is a property of the current scan strategy, not of the
format. Phase 7's device-aware parallelism
([`roadmap-phase7-scan-performance.md`](roadmap-phase7-scan-performance.md))
scans an NVMe-backed file out of order, which is the plausible future source of
interior holes; a watermark would have to be unwound to allow them, a span list
would not.

An `Unscanned` span later subdividing into real spans is the reference case for
the monotonic-coverage rule below: specificity increases, coverage never
decreases.

## Mapping and streaming are separate passes

**Decision.** A live scan's only job is to **extend the map**. Rows are emitted
only by replaying a block the map already contains — `Segment::Known`. A
`Segment::Live` never yields a batch.

A cold query therefore runs in two phases: walk forward from `scanned_through`,
recording every block it passes and classifying the DDL between them, until the
queried table's block **closes**; then replay that block for its rows. The
target block's bytes are read twice — once to find its extent, once to emit its
rows — and that is the price of the split.

**Why the split.** Interleaving the two makes the map race the rows. A caller
can pause anywhere, and a `ResumeToken` captured mid-block points at a byte the
map has not reached: the recorder persists a block only when it *closes*, and a
`yield` suspends the stream before that runs, so the bytes between the map's
frontier and the pause point belong to no span. Reconciling a resumed segment's
freshly-built spans against that hole is repair work with a case per pause
shape, all of it on the path with the least test coverage, and all of it failing
silently — as a cache that no longer tiles rather than as an error. Separating
the passes deletes the hole instead of repairing it: every resume point is
inside a fully mapped region by construction, so a resumed stream continues a
replay rather than falling back to a live scan, which also closes the
[`roadmap-phase1-mvp.md`](roadmap-phase1-mvp.md) known gap that said it could
not.

**A cold query stops at the queried table, not at EOF — unless the block says
otherwise.** Today's live segment always runs to `size`, so a cold query
against the first table of a 1TB file costs a full 1TB scan. Bounding it to
`[0, target.end_offset)` is what makes this phase's "don't walk bytes you don't
need" claim true of the query path and not just of the map, and it is also what
pays for the split's second read rather than adding it on top.

The exception is I2: one `COPY` header name can own several blocks in one dump,
because every leaf partition of a load-via-partition-root table writes a header
naming the **root**. Those blocks are *not* adjacent — entries sort by the
partition's own name — so reading forward until the next block names a
different table does not enumerate them; only EOF does. `pg_dump` marks exactly
these blocks, with a `-- load via partition root <root>` line between the TOC
comment and the header, and the marker survives `--no-comments`, `--data-only`
and `--inserts`.

**So the rule is: stop when the matching block closes, unless that block
carried the marker, in which case continue to EOF.** `map::Builder` records the
marker on the block it precedes — `CopyBlock` gains `partition_root:
Option<String>`, read straight off the line, which is
[`layering.md`](layering.md) rule 5's "store what the dump said, never what we
concluded" and persists it for free. A caller that wants the full scan
regardless asks for it explicitly rather than inferring it from cache state.

Two residues, both in `STATUS.md`'s "Known gaps": a file concatenating two
dumps of the *same* database name has no early signal at all, and ambiguity
detection still sees only candidates the scan reached — see "One target per
query" in
[`roadmap-phase2-typed-columns.md`](roadmap-phase2-typed-columns.md), whose
already-recorded gap this widens. A later slice can replace the marker rule
with a provable one: I1 puts every `ALTER TABLE … ATTACH PARTITION` ahead of
all data, so the leaf set of a queried root is knowable before the first block
— but that needs partition DDL parsing, which belongs with 3.3/3.4's grammar,
not here.

**The deferred alternative, on purpose.** `ResumeToken` is opaque and valid only
within the producing process, so it could instead carry the live segment's
in-flight spans and its open `CopyStart` — which would close the hole with no
repair *and* keep live emission, removing the double read. That is an
optimization over a shape we are still iterating on, so it is deferred rather
than rejected; it is filed under "Future — wanted, unscheduled" in
[`roadmap.md`](roadmap.md) and should be revisited once the feature set is
settled.

## Span boundaries: statement-anchored, object-attributed, greedy

**Decision.** A span opens at the `--` of its TOC comment — or, where there is
no TOC comment, at the first byte of its first statement — and runs to the byte
before the next span opens. Interstitial blank lines are absorbed into the
preceding span. **There are no whitespace or comment-run span kinds.**

**A span is one statement; a TOC entry may own several of them.** One archive
entry routinely emits a `CREATE` plus its follow-on statements — `ALTER …
OWNER TO`, `ALTER TEXT SEARCH CONFIGURATION … ADD MAPPING FOR` (one per
mapping), `ALTER EVENT TRIGGER … DISABLE` — and only the first is preceded by
the entry's `-- Name: …; Type: …` comment. Statement completion closes a span
in all of them, so those follow-ons are spans of their own; **each inherits
the governing entry's `TocHeader`** rather than carrying `None`.

`Span::toc` therefore means *the TOC entry this span belongs to*, not *the TOC
comment this span starts with*; a span records separately whether it carried
the header text itself, which is what keeps "opens a new archive entry"
answerable. Inheritance runs until the next TOC comment, `COPY` block, or
large-object region opens — the same three signals that close a governing
entry's own span — and is cleared by any span that is not a plain statement:
`Framing`, `Connect` and `VersionHeader` never inherit, so mid-file framing is
not attributed to the object before it.

**An object census counts header-bearing spans**, not attributed ones — one
per archive entry. `pgdq info`'s `object kinds:` breakdown is that census, so
after inheritance it reads "seven tables", not "seven tables plus their owner
statements". The figure that was wrong as *coverage* is exactly right as a
*census*; 3.3.1 relocates it rather than discarding it.

Merging those follow-ons into the object's span was rejected: it reduces
specificity, which the standing rule below forbids, and it deletes the
byte-exact statement boundaries a future writer or a `--filter` would need.
Leaving them unattributed was also rejected: it puts object identity in
adjacency, where only a reader's eye can recover it, and it is what made the
TOC-coverage figure read ~50% on healthy input. Attribution by inheritance is
enrichment, which is the direction the standing rule permits.

Evidence and measurements:
[`../status/history/2026-08-24.md`](../status/history/2026-08-24.md).

The roadmap sketch listed `comment-run (n lines)` and `whitespace-run` as span
kinds. They are dropped: content-anchored spans force a rule for where trailing
whitespace belongs that nothing upstream guarantees (the blank-line counts
between objects are regular in practice but promised nowhere), and every extra
span kind is another opportunity to leave a byte unattributed — the exact
failure the tiling invariant exists to catch. Statement-anchored spans make the
boundary rule one sentence, keep span count proportional to the DDL rather
than to the whitespace between it, and — with the TOC inheritance above —
let `pgdq info` group its listing by object without filtering noise out of
it.

The roadmap's stated motive for those kinds — making "this dump has zero
comments" objectively queryable — is served instead by the TOC-coverage figure
above plus a per-span record of whether it carried a TOC header. Byte-exact
"how much of this file is blank" accounting is the one thing lost, and nothing
needs it.

**Two signals close a span, and both are needed.** The next TOC comment
block's arrival is one — recognized lexically, by the presence of a `-- Name:
…; Type: …` line, never by parsing its fields, so this is a boundary
mechanism and not an early draw on the enrichment layer. It is load-bearing
rather than merely convenient: `scan.rs` emits no `Event::Line` for a
dollar-quoted line, including the one carrying a `CREATE FUNCTION`'s own
closing `;`, so a detector watching only for statement completion can never
observe such a statement ending.

The other is statement completion itself, for content with no TOC comment —
and it needs `scan.rs` to emit a **position-only event marking where a
dollar-quoted region ended**, added in slice 3.2.3. Without it, the first
dollar-quoted body in a header-less file absorbs every statement after it into
one span: measured, and recorded in
[`../status/history/2026-08-23.md`](../status/history/2026-08-23.md). That
event carries an offset and nothing else — the dollar-quoted lines themselves
stay unsurfaced, so L1's event contract still says nothing about DDL text.

This is what makes "graceful degradation" true rather than aspirational. The
degraded map is *coarser* — no owner, no kind label, no grouping, and nothing
to inherit, so every statement stands alone — but it is still one span per
statement rather than one span for the rest of the file, which is the claim
the decision to reject TOC-driven segmentation rests on.

**Framing spans.** The file prologue and epilogue carry no TOC header and are
not archive entries: the `-- PostgreSQL database dump` banner, `\restrict`, the
two version-header lines (I9), and the `SET`/`set_config` block written by
`_doSetFixedOutputState()`; then the `-- PostgreSQL database dump complete`
banner and `\unrestrict`. They are spans like any other, classified as framing.

**`COPY` blocks are the one exception, deliberately.** A data span's outer
boundary starts at its TOC comment, but a row reader needs to seek to the first
byte of the first row and to know where the last row ends. `CopyBlock` already
carries exactly this from Phase 1 — `header_offset`, `data_offset`,
`terminator_offset`, `end_offset` — and `Span::Data` holds the block inline, so
the invariant is:

```
span.start ≤ header_offset < data_offset ≤ terminator_offset < end_offset ≤ span.end
```

This is a one-off for `COPY`, not a design standard: no other span kind carries
inner offsets, and none should acquire them without the same
seek-into-the-middle justification.

## Diagnostics: a file-level channel on `DumpIndex`

**Decision.** `DumpIndex` grows `diagnostics: Vec<Diagnostic>`, marked
`#[serde(skip)]` so it is **not persisted**. A cache-identity warning, a
tiling failure, the TOC-coverage figure and an unrecognized span all report
there, as a `{severity, kind}` pair.

Phase 2's per-column outcome hangs off `ResolvedSchema` instead; a cache mtime
mismatch has no column and no schema to attach to, and the library cannot
`eprintln!` — it is destined to sit inside DataFusion. Not persisting the list
matters: a cached diagnostic would replay a warning about a check that *this*
run performed successfully. Recomputing on load is cheap and correct.

**One severity scale, two types — not one enum.** This originally read "…so
Phase 2's per-column resolution outcomes … all speak one vocabulary", meaning
a single type. That cannot be built: `DumpIndex` is L1 and
`resolve::ColumnResolution` is an L2 conclusion about PostgreSQL type
semantics, so a `DiagnosticKind` variant carrying one would have L1 name an L2
type — [`layering.md`](layering.md) rule 1, and against L1's premise that it
parses a declared type as an opaque string and never interprets it. What is
genuinely shared is the `Severity` scale and the shape, so that is what is
shared: `Diagnostic` is the file-level channel in L1, and `ColumnNote` is the
per-column record in L2 (one per column, always present — a record, not an
exception report), reporting its severity through the same scale. Unifying at
the *drain* point stays open: the Phase 6 caller-supplied sink below can take
both. Reasoning:
[`../status/history/2026-08-24.md`](../status/history/2026-08-24.md).

Returning diagnostics alongside every result was rejected as changing every
public signature for something most callers ignore. A caller-supplied sink (the
`tracing-subscriber` shape) is the right long-term embedder story but belongs
to Phase 6; a sink can drain this list, so nothing here forecloses it.

## Cache: the dump file's identity is checked, not assumed

**Decision.** The cache records the dump file's **size and mtime** as observed
at scan time, and both are checked **whenever the source file is revisited** —
continuing an incremental scan, replaying a `Segment::Known`, or reading a
span's bytes.

- **Size mismatch is an error.** Any byte in the file could have moved, so
  every offset in the cache is meaningless. The whole cache is invalidated,
  not repaired.
- **mtime mismatch is a loud warning, not an error.** mtime handling varies too
  much across filesystems and operating systems (granularity, preservation
  across copies and restores, network filesystems) for a mismatch to be
  conclusive evidence of a changed file.

This closes what `roadmap-phase1-mvp.md` filed under "Configurable (future)".
It becomes load-bearing here because a span records where its statement text
lives, and Phase 5's statistics will need it for a stronger reason still — a
stale statistic causes a wrong *answer* rather than a wasted scan
([`roadmap.md`](roadmap.md), "The correctness asymmetry is the thing to get
right").

Span text is nonetheless **stored in the cache**, not re-read on demand, so
that `pgdq info` answers from the cache alone and a stale cache is merely
stale rather than misleading. Size is bounded by schema size — koji's entire
DDL surface is 154KB — with a **per-span 64KB cap** above which the span keeps
offsets plus a `truncated` marker, so one pathological function body cannot
make the cache unbounded. `Data` spans never store text.

## Span text comes from the file, not from the parser

**Decision.** A span's text is **sliced from the file by offset** when the span
closes, never accumulated from `Event::Line`.

`scan.rs`'s outside-block arm `continue`s on any line inside, entering, or
leaving a dollar-quoted string, so no `Event::Line` is emitted for it — which
is why `preamble.rs` needs no defence against a `CREATE FUNCTION` body, and
equally why an accumulated span text would be missing every function body in
the file. Slicing by offset makes text a pure function of the span's
boundaries rather than of parser state, which is the same property the tiling
invariant wants, and costs nothing: the bytes are in the scan buffer already.

Surfacing dollar-quoted *lines* as events stays rejected: it leaks a Phase 3
concern into L1's event contract, and slicing by offset already gives the text.
A **position-only** event is a different matter, and is added in slice 3.2.3 —
see "Span boundaries" below.

The stored text is the **whole span**, TOC comment and trailing blank lines
included — the span is the tiling unit, and the comment is context a reader
wants.

**The statement accumulator needs hardening first.**
`preamble.rs`'s `statement_complete` tracks paren depth and single-quoted
strings but treats an apostrophe inside a `--` comment or a double-quoted
identifier as opening a string that never closes. Unreachable today (it starts
only on five `CREATE`/`ALTER TYPE` keywords, for which `pg_dump` emits neither
shape); Phase 3 absorbs arbitrary statements, which makes both reachable. Fixed
in slice 3.2.

## Tiling is verified at runtime and reported as a diagnostic

**Decision.** Coverage is checked whenever a map is completed. A failure emits
a high-severity `Diagnostic` and leaves the map usable. The tiling test over
every fixture exists as well; the runtime check is not a substitute for it.

Tiling is an assertion about *our own parser*, not about the input — a hole
means we have a bug, not that the dump is bad. The check is O(spans) against a
scan that just read the entire file, so it is free, and what it guards is a
silently dropped region on a dump shape no fixture covers: exactly the case a
test cannot catch, and exactly the case that matters when the input is "a dump
file of unknown origin."

Failing the scan outright was rejected — a map with a hole is still more useful
than no map, and refusing to answer "which roles does this need" over an
accounting discrepancy serves nobody.

## Standing rule: coverage increases monotonically

**From Phase 3 onward, a later phase may subdivide a span or attach detail to
it, never reduce coverage.** Specificity increases monotonically; the tiling
property does not degrade. Splitting the one large-object span into one span
per object is the intended shape of "more specific"; introducing a span kind
that leaves bytes unaccounted for is not. This outlives Phase 3 the way
[`layering.md`](layering.md) outlives the phase that introduced it.

The rule is only real if something checks it. A test asserts tiling over every
fixture, and the cases that matter most are the degenerate ones: `--data-only`
(no DDL), `--inserts` (no `COPY` blocks at all), `--schema-only` (no data at
all), and the concatenated multi-database shape. Absent that test, "spans sum
to file size" decays into an aspiration the first time a span kind is added.

## Implementation slices

Ordered so each slice makes the next one's mistakes visible; the cheap,
no-code, evidence-gathering slice goes first.

| Slice | Scope |
|---|---|
| **3.1** | A third fixture schema (`objects`) covering the TOC kinds neither existing schema produces, plus **large objects**, plus a `--verbose` flag set. No library code. |
| **3.1.1** | Fixtures for the four shapes 3.1's set never produced, all of them read out of upstream source and never checked against real output: the TOC comment's `Tablespace:` field and a non-default `SET default_tablespace` (a `CREATE TABLESPACE` in the generator's own container, which `docker exec` already reaches), a `REVOKE`, and `TOC_PREFIX_STATS` — the last needing version-conditional flag sets in `SCHEMAS`, since `--with-statistics` is v18-only. Earned by a wrong contract: 3.1's set is what the enrichment layer and cross-reference set were built against, and it silently omits shapes both of them claim to parse. |
| **3.2** | `map.rs` as a standalone module: the span model, the tiling invariant with its test over every fixture, cache identity checking, and the hardened statement accumulator — statement-driven pass only, no TOC enrichment. Updates `layering.md`'s module table and Arrow-free check for `map.rs`. |
| **3.2.1** | The map becomes `DumpIndex`'s primary structure, per "The map is the structure, not a description of it": `spans` primary with `blocks()`/`blocks_for` derived, `Span::Data` holding `CopyBlock` inline, spans persisted (cache format bump), and `build_index` producing spans **in its existing pass**, driving the same `map::Builder` `build_map` does, rather than as a second one. `crate::stream::table_stream`'s `Recorder` appends each live-discovered block as its own `Span::Data`. `Unscanned` becomes a span a real incremental scan produces (`preamble_only`, the one genuinely partial scan today), not a reserved variant. |
| **3.2.1.1** | `DumpMetadata` as a memoized derived view over `spans` (`dump_metadata_from_spans`), replacing the separate `PreambleBuilder` pass — `SpanBody` grows `Connect`, `VersionHeader` and `AlterTypeAddValue` to carry what `Framing`/`Unparsed` couldn't, verified against the old pass's output across every fixture before `PreambleBuilder` was deleted. |
| **3.2.1.2** | `map::Builder` gains `snapshot`, a non-consuming "spans so far" read at any boundary where `mode` is `Idle` (e.g. right after `on_copy_end`) — the capability gap `finish`'s once-only, consuming signature left, found while scoping this slice. `push_span` now fixes up the previous span's `end` at push time rather than deferring every span's `end` to a single end-of-scan pass, which is what makes `snapshot` possible without `finish`'s loop. No caller yet. |
| **3.2.1.2.1** | Mapping and streaming become separate passes in `stream.rs` (see that section): a `Segment::Live` classifies the DDL between the blocks it discovers — the phase's own "cheap tier" — and never yields a batch, stopping once the queried table's block closes unless that block carried the partition-root marker; rows come only from a `Segment::Known` replay over an already-mapped block, resume included. `map::Builder` gains a seeded constructor (start offset plus in-scope database) so a segment beginning partway through the file tiles from its own first byte, and `Builder::snapshot` is spliced onto the base spans after each completed block. `CopyBlock` gains `partition_root` (I2). A `partitions` fixture schema lands with it, under `{default, --load-via-partition-root}` — the default flag set alone produces the multi-block shape, since `pg_dump` forces the mode for hash-on-enum partitioning. A query-built `DumpIndex` then tiles the way `build_index`'s does, with no exemption for resumed streams. Earned by 3.2.1.2's mis-sizing, not by a wrong contract. |
| **3.2.2** | Additive remainder: span text sliced from the file and stored in the cache with its 64KB-per-span cap and `truncated` marker, and the file-level `Diagnostic` channel on `DumpIndex` — through which the runtime tiling check and the cache's mtime warning are reported. |
| **3.2.3** | A position-only `scan.rs` event marking where a dollar-quoted region ended, and `map.rs` closing a statement on it — so a TOC-comment-less dump degrades to one span per object rather than to one span for the rest of the file. See "Span boundaries". |
| **3.3** | The TOC enrichment layer: owner, kind labels, the `Tablespace:` field, TOC-coverage reporting. |
| **3.3.1** | TOC inheritance for follow-on statements: a span continuing the object before it carries the governing entry's `TocHeader` rather than `None`, with a separate record of whether it carried the header text itself, and `toc_coverage_diagnostic`'s numerator becomes attributed spans. `pgdq info`'s `object kinds:` breakdown switches to header-bearing spans in the same slice, since it is an object census rather than a span census and inheritance would otherwise double-count. Earned by a wrong contract, not by mis-sizing — 3.3 shipped a coverage figure that reads ~50% on a healthy, fully-TOC'd dump, which is the one case it exists to distinguish. See "Span boundaries" and "TOC coverage". |
| **3.4** | The cross-reference set — referenced roles and tablespaces. `objects.rs` splits out of `preamble.rs` here or in 3.3 if that module passes ~1500 lines. |
| **3.6** | The `Data`-span fast path for the two bulk regions the generic statement grammar merely tiles correctly rather than skipping: `INSERT` runs (the string-aware scanner this table originally placed in 3.2) and the large-object region, grouped into one `Data` span apiece. Carries this phase's two "Verification" measurements. Ordered before 3.5 so the CLI's span listing shows the shape the map keeps. |
| **3.5** | CLI surface: `pgdq info` gains role, tablespace and object-kind summaries by default and a `--map` span listing; `docs/manual/` gains the dump-inspection page. |

**3.2.1, 3.2.2 and 3.2.3 were earned, not planned.** The first two come from
3.2 being mis-sized: as originally written it paired a self-contained new
module with a rework of the tested core query path, which is two review
cycles' worth of confidence in one slice. Its row above is the scope that
landed; the remainder became those two follow-ups. 3.2.3 is the other kind —
3.2 shipped a boundary rule that is correct for `pg_dump`'s own output and
collapses on the header-less input the "Scanning" decision is justified by.
Reasoning for all three:
[`../status/history/2026-08-23.md`](../status/history/2026-08-23.md).

**3.2.1.1, 3.2.1.2 and 3.2.1.2.1 were earned the same way 3.2.1/3.2.2 were, in
three rounds, each splitting off the same kind of remainder.** 3.2.1's own row
first split off a combined 3.2.1.1 (`DumpMetadata`'s derived-view rework and
`stream.rs`'s live-segment DDL classification bundled together) because both
turned out to need real design decisions rather than being plumbing over
already-decided shapes. Once inside that combined slice, the same mis-sizing
pattern repeated: the `DumpMetadata` rework was self-contained and fully
verifiable against the old implementation's output, while the `stream.rs`
piece turned out to need a `map::Builder` capability (a non-consuming
snapshot) that doesn't exist yet — a second, independent reason not to review
it alongside the first. Its row above is the scope that landed as 3.2.1.1; the
remainder became 3.2.1.2. Scoping 3.2.1.2 in turn found that even the
`map::Builder` capability and its `stream.rs` wiring don't belong in one
review: the capability is mechanical and directly testable against the
existing `Builder` state machine, while wiring it into `stream.rs` is a rework
of an already-tested core path (resume tokens, ambiguous-table detection, the
`Recorder`). That remainder became 3.2.1.2.1, whose row above was then
rewritten: the open questions it was carrying were all consequences of
interleaving map-building with row emission, and the answer was to stop
interleaving them rather than to reconcile them one pause shape at a time.
Reasoning: [`../status/history/2026-08-24.md`](../status/history/2026-08-24.md).

**3.6 is numbered out of order deliberately** — it was split out of 3.2 for
the same sizing reason, but it is not a `<N>.<M>.<K>` follow-up to it: its
content is independent of everything 3.2 landed, and it runs after the
enrichment slices rather than before them.

**3.1 must produce a large-object fixture**, generated by hand
(`lo_from_bytea`/`lo_import` against a scratch database) since no `pg_dump`
flag conjures one. I12's fast-path skip is currently asserted from source
reading alone, and the large-object region is the only part of this phase's
cost argument with no evidence behind it — koji has no large objects either.
The `--verbose` flag set is there because it is the one documented way to make
the TOC comment block taller than three lines.

**Cache identity checking sits in 3.2, not later**, because it is the
precondition for trusting any cached span offset and the tiling test is the
first thing that reads them back.

Current fixture coverage, for reference: `edge_cases`/`types` real `pg_dump`
output produces `TABLE`, `TABLE DATA`, `CONSTRAINT`, `FK CONSTRAINT`,
`FUNCTION`, `SCHEMA`, `TYPE`, `DOMAIN`, `SHELL TYPE`; `objects` (slice 3.1,
`scripts/fixture_schema_objects.sql`) adds `ACL`, `AGGREGATE`, `BLOB
METADATA`/`BLOB`, `BLOBS`, `CAST`, `COLLATION`, `COMMENT`, `CONVERSION`,
`DEFAULT`, `DEFAULT ACL`, `EVENT TRIGGER`, `INDEX`, `INDEX ATTACH`,
`MATERIALIZED VIEW`(`DATA`), `POLICY`, `PUBLICATION`(`TABLE`/`TABLES IN
SCHEMA`), `ROW SECURITY`, `RULE`, `SEQUENCE`(`OWNED BY`/`SET`), `SERVER`,
`STATISTICS`, `SUBSCRIPTION`, `TABLE ATTACH`, `TEXT SEARCH
CONFIGURATION`/`DICTIONARY`, `TRIGGER`, `USER MAPPING`, `VIEW`. Still
unexercised by any fixture, and why (see that file's header): `SECURITY
LABEL` (needs a security-label provider extension), `ACCESS METHOD`,
`OPERATOR`(`CLASS`/`FAMILY`), `TRANSFORM`, `TEXT SEARCH PARSER`/`TEMPLATE`
(all need a C-level handler function), and the dump-level-metadata kinds
`DATABASE`(` PROPERTIES`)/`ENCODING`/`SEARCHPATH`/`STDSTRINGS`.

## Module and layer assignment

**Decision.** A new `map.rs` owns `Span`, `SpanBody`, and the tiling check.
`preamble.rs` keeps the DDL grammar and becomes its statement parser;
`index.rs` keeps `DumpIndex`. All are **L1** — byte offsets, span structure,
DDL text grammar and cache shape are L1 concerns by
[`layering.md`](layering.md)'s own table, and no Arrow type appears in any of
them.

The widened grammar this phase adds — `GRANT`/`REVOKE`/`ALTER DEFAULT
PRIVILEGES`, `OWNER TO`, `SET default_tablespace`, plus the hardened statement
accumulator — is a lot to add to a `preamble.rs` already at 1014 lines, and the
expected end state is a further split into `objects.rs`. That split is *not*
made up front: guessing the seam before the grammar is written is how it lands
in the wrong place. Slice 3.2 lands `map.rs`; if `preamble.rs` passes ~1500
lines, 3.3 or 3.4 splits `objects.rs` out and updates `layering.md`'s table in
the same change. All candidates are L1, so the split carries no layering
consequence — which is why it is safe to defer.

## Verification

**Decision.** Two measurements gate the phase, and the phase does not wrap
without both. They belong to slice 3.6, which lands the bulk-region fast path
they measure.

- **A synthetic large-object dump**, a few GB, on the SSD. The large-object
  fast path is the only part of the cost argument with no evidence behind it at
  any size — I12 proves the region's *shape* from source, not that our skip is
  cheap — and a few GB is enough to distinguish "skipped" from "walked" without
  an hour of HDD time. Its generator is small: the region is three statement
  forms.
- **A koji full re-scan**, as a regression check against Phase 1's 243 MB/s
  baseline. koji has neither large objects nor `INSERT` runs, so it tests only
  that the map doesn't regress the ordinary case — which is worth knowing, and
  is the case every user hits. This is a ~1 hour job and follows `CLAUDE.md`'s
  long-running-job protocol: launch detached, record the log path, let a later
  session read the result.
