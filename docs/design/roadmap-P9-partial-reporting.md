# P9 — Partial reporting and machine-readable resolution

What pgdq can say about a dump it has only partly read, and in what form.

Every mechanism this phase needs already exists: `stream::map_forward` resumes
from a cache frontier and saves after every block, `print_index` resolves each
block's columns for `--verbose`, and `CacheStatus` already distinguishes a
finished cache from an unfinished one. What is missing is that the CLI throws
all of it away — a partial cache can answer nothing, a resolution nobody can
script against is computed and discarded at the JSON boundary, and an hour of
scanning can be triggered by a command that reads like a question.

The number is 9 because 5–8 are taken and this project does not renumber a tail
([`../process.md`](../process.md), "Slice numbering"). The phase runs after
P4 and before P5; [`roadmap.md`](roadmap.md) carries the ordering.

There is no `roadmap-P9-*-inbox.md` — the phase did not exist when earlier
phases were filing facts. Two facts *this* phase produces are filed into
[`roadmap-P6-embeddable-engine-inbox.md`](roadmap-P6-embeddable-engine-inbox.md), where the embedded API
has to answer them.

## The split the CLI does not currently draw

Three verbs, and the boundary between them is blurred in both directions:

| Command | Today | Cost of the blur |
|---|---|---|
| `parse` | Always scans from byte 0, ignoring any existing cache; writes the cache once, after the scan returns | A koji parse killed at minute 50 leaves nothing. There is no way to finish an unfinished scan |
| `info` | Falls back to a full `build_index` whenever the loaded cache does not cover the file; cache-only mode bails outright on an incomplete cache | A question can cost ~54 minutes. A half-finished cache can answer nothing about the half it holds |
| `query` | Scans incrementally, stopping when its target settles, persisting per block | None — this is the behaviour the other two should have had |

The asymmetry is not a defect anyone introduced; it is what happens when the
eager path and the incremental path are built at different times for different
reasons. `query` got the incremental machinery because a cold query on a
784GB file had to be affordable. `parse` and `info` never needed it, so they
never got it, and the result is that the *only* way to produce a resumable
partial cache is to interrupt a query — the one command whose purpose is not
producing caches.

## `info` reads; `parse` scans

**`info` never scans.** It reports whatever the cache holds, however partial,
and marks the coverage. With no usable cache it errors and says a `parse` is
required, rather than starting one on the user's behalf.

**`parse` is the only scanner**, and `--preamble-only` moves to it.

*Rejected:* keeping `--preamble-only` on `info` as the documented exception,
on the grounds that it is bounded by construction and costs seconds. A rule
with one exception is a rule nobody can state, and the exception is not
cheap in the way that matters — the surprise is not the *duration* of an
unrequested scan but that one happened. `index::preamble_only` is unchanged;
only the verb it hangs off moves, and `info` reads the preamble-only cache it
leaves behind.

*Rejected:* keeping `--preamble-only` on `info` as a pure display filter
(metadata only, no block listing). Its name states a scan extent, and after
this phase `info` has no scan extent — a preamble-only cache prints only
metadata because that is all it contains. Against a *complete* cache the flag
would be a display convenience whose name talks about scanning, and `--verbose`
/`--map` already control detail.

*Rejected:* `info --source` reporting from a partial cache by default while
`--dqcache` alone keeps refusing, or a `--no-scan` flag to opt into the cheap
answer. The first silently changes what an existing command answers; the second
adds a flag whose behaviour duplicates a command that already exists. Both
preserve the blur this phase exists to remove.

**Both invocation forms stay.** `--source X` locates and validates against
`X.dqcache`; `--dqcache P` alone reads P with no live file to check against.
They answer different questions — "what do we know about this file" versus
"what is in this cache" — and only the first can detect that the cache went
stale. The cache records the source's size, so the coverage line works either
way.

**`query` is untouched.** It must scan, and its two-path model (stop at the
target, or `ScanExtent::Full`) is what makes a cold query on a large dump
affordable. That `info` refuses to scan while `query` scans freely is not an
inconsistency to resolve: `query` is asked for rows that only exist in the
file, where `info` is asked what is known.

## `parse` resumes, and saves as it goes

**Resume is the default.** `parse` continues from the frontier of any cache
that matches the source. After this phase, "finish the scan" is the
overwhelmingly common reason to run `parse` twice on one file.

*Rejected:* a `--restart` flag. Removing or renaming the cache file says the
same thing without a flag, and this replaces today's documented promise that
`parse` always scans fresh — a promise worth replacing rather than preserving
behind an opt-out.

**`parse` persists after every completed block**, which is what makes resuming
worth building at all: today the cache is written only after `build_index`
returns, so an interrupted scan leaves nothing to resume *from* and nothing for
`info` to report. `map_forward` already saves per block at a `CopyEnd`
watermark — a resumable point by construction, since the scanner is back in its
`Outside` state there — so this is a second caller for an existing loop, not
new machinery.

**The write amplification is measured on koji, in the slice that introduces
it.** The cache is serialized whole on each save, and a full scan has orders of
magnitude more blocks than an early-stopping query, so the per-save cost that
is invisible for `query` may not be for `parse`. A throttle — save at most
every N seconds or N bytes — is the tuning knob to reach for **only if the
number demands it**; choosing an interval up front means choosing it with no
evidence. The figure and its command go in
[`measurements.md`](measurements.md), not in a notes doc.

**A resumed `parse` says so, then prints the whole index.** The listing
describes the file's state after the run, not the invocation's diff — the same
reason `STATUS.md` describes what is. But a resumed run that silently prints a
full listing gives no signal that it resumed rather than rescanned, which is
exactly what a user checking on an interrupted koji scan needs. One line naming
the resume point precedes the listing.

## The save throttle, and saving on the way out

The koji figure asked the right question of the wrong dump. At 74 blocks over
3300 seconds the per-block save is +1.5%, so 9.1 built no throttle — but each
save serializes the **whole** index, so total work is O(blocks^2), and koji
cannot see that regime. Measured on a synthetic dump in `pg_dump` order, wall
time is 0.62s / 2.54s / 10.73s / 44.36s at 500 / 1000 / 2000 / 4000 `COPY`
blocks, a clean 4x per doubling, against **under 10ms** for the same byte count
in a single block. Every run is 99% CPU: the cost is serialization, not the
write. A schema with a few thousand tables, or one partitioned table with a
daily leaf over a decade, pays tens of seconds of pure overhead on a
two-megabyte file. Figures and commands are in
[`measurements.md`](measurements.md).

**The throttle is self-tuning, not an interval.** Skip a block's save unless
the elapsed time since the last save is at least `K` times what the last save
*took*, always saving at EOF. That bounds the overhead at roughly `1/K` of
scan time in every regime without a constant that is wrong in one of them: a
cheap cache saves often, an expensive one saves rarely, and koji — whose blocks
are ~45s apart and whose saves cost well under a second — is untouched.

*Rejected:* "save at most every N seconds" and "every N bytes". Both need a
number chosen against one dump shape, and both are wrong on the other: N
seconds is too frequent for a slow save and too rare for a fast one, and N
bytes is blind to the fact that the cost tracks block count, not bytes read.

**A throttled scan must still save what it has when it is killed.** On
`SIGINT` and `SIGTERM`, `parse` saves what it holds and exits. Without it the
throttle would trade a measured cost for an unmeasured one, and the container
form in `CLAUDE.md` — where a stop is a `SIGTERM` — is the shape most likely to
hit it.

What the guard costs the throttle is close to nothing, because the skipped
saves were only ever writes of state the **in-memory** index still holds: the
splice, the roles, the tablespaces and `scanned_through` are updated at every
`CopyEnd` whether or not the save runs. So a graceful interrupt loses only the
block in flight, exactly as it did before the throttle, and the throttle's
window is exposed to `SIGKILL`, power loss and panics alone.

**The guard is a cooperative flag, read at every point the mapping loop can
cheaply reach it** — today once per chunk *and* at every completed block. The
principle is the rule, not the two sites: this spec originally named only the
chunk check, and the block-rich case walked straight through it. `map_file` owns the `DumpIndex` for the whole scan, so
racing `ctrl_c` against that future in the CLI would *destroy* the map rather
than save it — cancellation has to reach inside the loop. `ScanOptions`
carries an `Option<Arc<AtomicBool>>`, defaulting to `None` so no existing
caller changes. Chunk granularity is what gets a scan out of a block big
enough that its `CopyEnd` is an hour away: koji's largest block is hundreds of
gigabytes, and a Ctrl-C that waits for the next block boundary is
indistinguishable from a hang. The `CopyEnd` check covers the opposite
extreme, which the chunk check alone leaves unresponsive — a block-rich dump
can spend its whole multi-second scan inside two chunks (amended 2026-08-27,
[`../status/history/2026-08-27.md`](../status/history/2026-08-27.md)). The
interrupt path needs no snapshot logic of its own — `index` is consistent at
the last completed block — so it is `cache.save`, then return an outcome the
CLI can tell apart from EOF.

*Rejected:* `tokio::select!` in the CLI over `ctrl_c` and the scan. It reads as
the obvious form and it is the one that silently discards the work.

**The CLI catches both signals and never traps the user.** `SIGINT` and
`SIGTERM` both set the flag (the CLI crate gains tokio's `signal` feature);
the run prints where it stopped and that re-running resumes, and exits non-zero
by signal — 130 and 143 — so a script can tell an interrupt from a failure. A
**second** signal kills immediately rather than being swallowed, so a save that
wedges cannot hold the process.

**One rule for both callers, with every exit saving unconditionally.**
`map_forward` is one loop with two callers, so the throttle applies to `query`
too; that is harmless — a query stops at its target and saves a handful of
times — except for one sharp edge: the save at the last `CopyEnd` before an
early stop is what persists the map for the next query, and skipping *it*
would throw the scan away. So the throttle governs mid-scan saves only, and
EOF, target-settled and interrupt all save. `K = 20` is the starting constant
(~5% of scan time); the 500/1000/2000/4000-block series is what says whether it
over- or under-shoots, rather than an argument in prose.

## What a partial index actually lacks

Coverage is stated **once**, at the top, and nothing below it is qualified.

The human form is a completion line — `Scan completion: 76% (12345 bytes)` —
and the JSON carries the components as separate fields rather than a rendered
string.

*Rejected:* a per-record partiality flag. It would always carry the same value,
which reads as if it could vary. The reason it cannot is P4.5.1's: a block
enters the map only at a `CopyEnd` watermark, and every mapping pass censuses,
so **every record a partial index holds is complete in itself**. What a partial
index lacks is records, not confidence — there is no half-known block, only
blocks past the frontier that are not there at all.

That is worth stating in the spec because it is the non-obvious part. "Partial"
suggests every answer is provisional; here the file's extent is the only thing
that is.

## Naming why a cache is unusable

`CacheStatus::Absent` collapses four causes — absent, foreign bytes,
unrecognised format version, source-size mismatch — deliberately, because every
caller's response was "scan it anyway". `info` no longer has that response, so
the information now has a consumer and the enum splits.

The error distinguishes **"you have never parsed this file"** from **"your file
changed since you parsed it"**, even though both end in `pgdq parse`. The
remedy is the same; the fact the user needs to know is not.

**`mtime_changed` becomes a warning line** beside the coverage line. A cache
whose recorded mtime differs while its size matches stays usable — that is the
cache module's existing, deliberate weighting of weak evidence against strong —
but under the old behaviour a suspicious cache was about to be overwritten by a
rescan. Now it is the answer being reported, so the weak signal has to surface.

## Machine-readable resolution

`--json` exports `DumpIndex`, an L1 structure with no resolved schema, so
everything `pgdq info --verbose` prints per column — the outcome, the declared
PostgreSQL type, and since 4.4.1 the resolved Arrow type — is human-only. After
P4.5.1 there are six distinct answers to "why is this column a string",
and no script can reach any of them. `print_index` already computes the whole
thing per block; the data is discarded at the JSON boundary.

**Keyed by `COPY` block.** Each record identifies its block — `database`, the
qualified name, `header_offset` — and carries what `ResolvedSchema` already
holds: the Arrow schema, the per-column outcome, the declared type, the nested
plan.

*Rejected:* keying by table. It is what a script most likely wants and it is
not well-formed yet: one table can span blocks (I2), and a header-less block
takes placeholder `column1…N` names from its first row (`batch::column_names`),
so a rollup needs a rule for disagreeing blocks and for column identity. Phase
6's `TableProvider` has no choice but to write that rule, so guessing at one
here means the embedded API would have to contradict it. Per-block keying
leaves the grouping with the consumer, which is where it honestly sits.

*Rejected:* shipping both — per-block records plus a rollup that omits any
table whose blocks disagree. That is the same guess with a fallback bolted on.
A rollup is purely additive once the merge rule exists.

**A seventh `ColumnResolution`: `MetadataNotScanned`.** Resolving a block's
columns needs the preamble of *that block's* database. A partial cache always
holds the first database's, since `scan_preamble` runs before anything else,
but a `pg_dumpall` or `--create` dump whose scan stopped inside a later
database has blocks whose DDL was never read. The streaming path already
refuses that outright (`Error::MetadataNotScanned`, not a silent degradation);
a reported export cannot, because one unresolvable block must not sink the
document.

*Rejected:* reusing `NotDeclared`. It means "the dump never explained this
column" and is final, where the new variant means "finish the parse and ask
again" — identical-looking output, opposite advice. *Rejected:* emitting such a
block with its L1 facts and no resolution section, which makes the export's
shape vary per block; a value a consumer can branch on is worth more than an
absent key.

**No stability promise and no `version` field.** The shape stays undocumented
until after 1.0, per [`roadmap.md`](roadmap.md)'s "Pre-1.0". A version field is
exactly the compatibility shim that section forbids and would be the only one
in the tree; `STATUS.md`'s "`--json` carries no shape promise at all" stands.

## Where the code goes

No new module. The work lands in `pgdump_query-cli/src/main.rs` (the verb
split, the coverage line, the JSON assembly), `cache.rs` (the `CacheStatus`
split — L1, the on-disk cache format's concern), and `resolve.rs` (the
`ColumnResolution` variant — L2, a conclusion about PostgreSQL type semantics).

`parse`'s resume path reuses `stream::map_forward` rather than growing a
parallel loop in `index.rs`. That puts block-sequencing and cache-segment
planning in L4 where [`layering.md`](layering.md) already assigns them, and it
is what keeps the two producers from drifting — `tests/map.rs`'s
`build_index_spans_match_build_map_exactly` exists because that drift is the
failure this codebase has already had to defend against once.

*Rejected:* teaching `index::build_index` to resume. It is the eager,
whole-file producer, and giving it a frontier makes it a second implementation
of `map_forward` with the same splice-onto-a-prefix logic and a different set
of bugs.

## What the manual must say

The `info` and `parse` pages describe the behaviour this phase replaces, so
both are rewritten rather than amended:

- **`info` never scans**, and what to do when it says a parse is required. The
  two invocation forms and what each can detect.
- **`parse` resumes**, and that an interrupted parse is not wasted work. That
  removing the cache file is how to force a fresh scan.
- **How to read the coverage line**, and — the part a user will not guess —
  that a partial index's records are not themselves provisional.

The `--json` shape is **not** documented, per the no-promise decision above.
Saying "there is no stable shape; read the text output or accept breakage" is
the honest sentence, and it belongs in the manual even though the schema does
not.

## Slices

Ordered so the thing that *produces* partial caches lands first, and the
reporting slices have real material to be tested against rather than hand-built
fixtures. The measurement sits in the slice whose design it could overturn.

| # | Slice |
|---|---|
| 9.1 | `parse` resumes from a matching cache and persists after every completed block, via `map_forward`; the resume-point line. Plus the koji write-amplification measurement. No output shape changes |
| 9.2 | `info` stops scanning: `CacheStatus::Absent` splits, the "run `pgdq parse`" errors, the mtime warning, `--preamble-only` moves to `parse`. Both invocation forms unchanged |
| 9.3 | The coverage line — `Scan completion: 76% (12345 bytes)` in text, the components as separate fields in JSON |
| 9.4 | `--json` carries per-block resolution, including `ColumnResolution::MetadataNotScanned` |
| 9.5 | The self-tuning save throttle and the interrupt guard, earned from the measurement 9.1 was asked to take. Plus `scripts/generate_block_count_bench.py` and the block-count series in `measurements.md` |
| 9.5.1 | **Earned**, not planned: `parse` states its `DumpMetadata` at every legal boundary — a preamble prepass before mapping (as `table_stream` already does), and a recompute at each `\connect`ed database's first `COPY` block — so an interrupted `parse`'s cache reports `MetadataNotScanned` rather than `NotDeclared`, and is typed for every database segment the scan finished. Absorbs the queued out-of-band move of the EOF recompute into `map_forward` |

**9.5.1 was earned by 9.5's verification.** `map_file` runs no preamble
prepass, so an interrupted `parse` leaves a cache with no `DumpMetadata` at
all, and `resolve_columns` answers `NotDeclared` for every column of every
block in it — the final, "the dump never explained this column" answer, where
the truth is "finish the parse and ask again". That is precisely the confusion
9.4 added `MetadataNotScanned` to prevent, and 9.4 could not see it: it
asserted the variant against hand-built truncated caches, which carry the
metadata a *query*'s prepass captures. The producer, not the resolver, is what
is wrong.

**Scoped at the 2026-08-27 review to every legal boundary, not just the first.**
`dump_metadata_from_spans` may be called at exactly two kinds of point — EOF, or
the start of the current database's first `COPY` block (I1) — and anywhere else
"would make the trailing database's `preamble_complete` a lie"
(`preamble.rs`). The second kind **recurs**: it is reached once per
`\connect`ed database, and `map_forward` already sees the event with the
governing database tracked. A prepass alone would leave a `pg_dumpall` parse
interrupted in database 3 holding DDL for database 1 only, having read
database 2's entire preamble. Recomputing at each such boundary is legal by the
rule above, costs one recompute per database, and is free on every
single-database dump (koji included). Because that puts the recompute inside
`map_forward`, this slice also **absorbs** the out-of-band item that moved the
EOF recompute out of `map_file` — it is the same code, and the divergence that
item removes (a cold query and a warm one typing a `pg_dumpall` alike) is this
defect at the other end. The other two queued drive-bys (`--dqcache none` error
text, `TOC_PREFIX_STATS`) are unaffected and still ride together afterwards.

**The recompute fires once per database, not once per block.** `map_forward`
sees `Event::CopyStart` for every block, and the obvious implementation —
recompute at each one and let it be idempotent — is O(blocks) whole-file
metadata recomputes, a *third* quadratic in the loop where 9.5 measured the
first two. The trigger is a `CopyStart` whose governing database differs from
the one the last recompute covered, so a single-database dump recomputes
exactly once, at the file's first block: the same point the prepass stops at,
which makes the prepass and the recurring rule one mechanism rather than two.

**The recurring half is tested on a real `pg_dumpall` fixture.**
`fixtures/<major>/edge_cases/dumpall.sql` spans more than one database
(`tests/map.rs`), and 9.5's `CancelsPast` trips the cancel flag once a read
reaches a chosen *file offset*, so the interrupt is deterministic. Cancel
inside the second database's data: the first two databases must come back
`preamble_complete`, the third must not, and `resolve_columns` must give real
types for the first two databases' blocks and `MetadataNotScanned` for the
third's. A prepass-only implementation passes every prepass test, which is why
this one is required rather than optional — and koji, being single-database,
cannot observe the recurring half at all. What the koji wrap run adds is the
real-scale half of the *prepass*: an interrupted koji cache must come back
typed rather than reporting `not declared` for every column.

**I1's `Relied on by` gains this mechanism.** The recurring recompute is
licensed by I1's *scope limit* — the invariant is per database, "hence
re-arming the preamble search at each `\connect`" — and the same entry's note
that `MetadataNotScanned` "can only ever name a later database" narrows once
this lands: it names a later database whose **first `COPY` the scan has not yet
reached**.

Reasoning:
[`../status/history/2026-08-27.md`](../status/history/2026-08-27.md).

**9.5 was earned by 9.1's own measurement.** The spec told 9.1 to build a
throttle "only if the number demands it", and the koji number did not — but
koji has 74 blocks and the cost is quadratic in that count. The number that
demands it came from a dump shape koji cannot represent, which is why this is a
slice earned from a discovery rather than a knob 9.1 should have built blind.

**9.3 is small enough to fold into 9.2 and is kept separate anyway.** It is the
phase's only user-facing formatting decision, and the one most likely to come
back after a look — which is exactly the seam worth being able to review alone.
Merging it would mean re-reviewing the `info` rework to change a percentage
format.

**9.1 must precede 9.2** for a reason beyond ordering: until `parse` can leave
a partial cache, the only way to produce one for a test is to interrupt a query
mid-flight, and a phase whose subject is partial caches should not be tested
against caches built by an unrelated command's edge case.

## Verification

- **An interrupted `parse` leaves a cache that `info` reports and `parse`
  finishes**, and the finished index is identical to one `build_index`
  produced in a single pass — span for span, including the census. This is the
  phase's central claim and the one that would fail silently.
- **The saves stay a bounded fraction of the scan as the block count grows** —
  the 500/1000/2000/4000-block series re-run against the throttle, which must
  turn its 4x-per-doubling into a curve that tracks the scan instead. The
  series comes from `scripts/generate_block_count_bench.py` (`--blocks`,
  `--out`, following `generate_insert_run_bench.py`'s conventions), because a
  verification that cannot be re-run is not one: `generate_perf_data.py` is
  parameterized by size and this benchmark is parameterized by block count, so
  it is a second script rather than a section in that one. And an interrupted
  `parse` still leaves a loadable cache: the guard is what makes the throttle
  safe, so it is verified with it.
- **The guard's real-scale test rides on P4's wrap koji run**, which is
  the only place the shape that matters exists: an interrupt arriving inside a
  hundred-gigabyte block, against a cache already holding dozens. That run is
  stopped partway with `nerdctl stop` — a `SIGTERM`, which is what the guard
  catches — checked for a loadable cache reporting partial coverage, then
  resumed to completion and compared against the 9.1 figures. One scan serves
  both that and the identity check; if the stop proves awkward to sequence
  unattended, identity keeps priority and the interrupt test drops to fixture
  scale.
- The koji write-amplification figure exists in
  [`measurements.md`](measurements.md) with its command, whether or not it
  triggers the throttle. A number that only appears when it is bad is a number
  nobody re-runs.
- **`info` with no cache exits non-zero and names `parse`**; `info` against a
  size-mismatched cache says the file changed, distinguishably. Two messages,
  both asserted, because collapsing them is the thing being undone.
- **A partial cache's `info` output lists exactly the blocks the map holds**,
  with the coverage line above it and no per-record qualification — asserted
  against a cache truncated at a known block boundary.
- **The JSON export's per-block resolution matches `--verbose`'s text for the
  same index**, column for column. One resolution pass, two renderings; a
  second implementation is the failure mode here.
- **A `pg_dumpall` fixture scanned only into its first database exports its
  later-database blocks with `MetadataNotScanned`**, not `NotDeclared` — the
  distinction the variant exists for, asserted where it would otherwise be
  invisible.
- `every_fixture_tiles_exactly` still passes. Nothing here changes the map's
  content, only who builds it and when it is written.

No performance number gates the phase. The koji figure informs the throttle
decision inside 9.1; it does not pass or fail the slice.
