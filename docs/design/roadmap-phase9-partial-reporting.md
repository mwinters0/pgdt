# Phase 9 — Partial reporting and machine-readable resolution

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
Phase 4 and before Phase 5; [`roadmap.md`](roadmap.md) carries the ordering.

There is no `roadmap-phase9-inbox.md` — the phase did not exist when earlier
phases were filing facts. Two facts *this* phase produces are filed into
[`roadmap-phase6-inbox.md`](roadmap-phase6-inbox.md), where the embedded API
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

## What a partial index actually lacks

Coverage is stated **once**, at the top, and nothing below it is qualified.

The human form is a completion line — `Scan completion: 76% (12345 bytes)` —
and the JSON carries the components as separate fields rather than a rendered
string.

*Rejected:* a per-record partiality flag. It would always carry the same value,
which reads as if it could vary. The reason it cannot is Phase 4.5.1's: a block
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
Phase 4.5.1 there are six distinct answers to "why is this column a string",
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
