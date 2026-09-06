# P17 — the library never replaces cache data automatically

**Status: specified, not started.** Slice progress is
[`../status/STATUS.md`](../status/STATUS.md), never this file.

## What this phase does

**A cache that does not seem to describe its source is reported to the caller,
never silently replaced.** Today every command that scans writes over whatever
sits at its cache path, so aiming `--dqcache` at the wrong file destroys an
index that was valid for its own input, with no warning and nothing to recover
from. After this phase the library refuses, says what it found, and leaves the
decision — and the file — with the caller.

## Why, and what it reverses

`pgdq parse --source A.dump --dqcache B.dqcache` loads `B`'s cache, gets
`SourceChanged`, discards it as unusable, scans `A`, and overwrites `B` within
the first throttled save (`architecture.md`, "`parse` resumes, and saves as it
goes"). `B`'s owner is never told. The mistake that produces this — pointing a
path flag at the wrong path — is an ordinary operational slip, and the cost of
it is unbounded: on a koji-scale dump the destroyed index represents an hour of
scanning.

This **reverses a decision the record already carries**: "Size mismatch
invalidates (`SourceChanged` — an unusable outcome, not a new hard-error path)"
(`architecture.md`, "The cache"), settled in P3 and implemented in P9. That
sentence is right about *reading* — a mismatched cache is indeed not a fatal
condition to read past — and wrong about *writing*, which it never separated
out. This phase splits the two: unusable for reading stays unusable, and
unusable for reading stops licensing a write.

Being a decision reversal is also why this is a phase rather than an
out-of-band item; the ledger's admission rule sends it here
(`../process.md`, "Out-of-band work").

**It is not compressed-input work.** `SourceChanged` is source-agnostic — a
plain `.dump` cache is destroyed by exactly the same path — and the hazard was
found while landing `M61` rather than caused by it.

## Decisions

**D1 — The library never replaces cache data automatically.** Where the cache
at a path does not seem to describe the source, the mismatch is reported and
the caller decides what to do about it. This is the phase's whole content; every
decision below follows from it.

**D2 — The colocated default is included.** `<dump>.dqcache` refuses on a
mismatch exactly as an explicit `--dqcache <path>` does. There is no workflow in
which silently discarding an index is the intended outcome, and the colocated
path being *usually* safe is not a reason to carve it out — the phase's
guarantee is worth more without an asterisk on it.

**D3 — Incompleteness is not mismatch.** A partial scan whose cache still
matches its source resumes exactly as it does today. `Incomplete` is a
statement about coverage; mismatch is a statement about identity, and this
phase touches only the second.

**D4 — Mismatch stays the stored-size evidence, and mtime alone stays a
diagnostic.** A changed mtime over an unchanged stored size remains a usable
cache carrying `CacheMtimeChanged`. mtime moves without content moving
routinely — `rsync`, a restore from backup, a `touch`, a filesystem copy — so
refusing on it would fire on healthy input, and a refusal that fires on healthy
input is one people learn to work around. This phase is about not *destroying*
on a mismatch, not about widening what counts as one.

**D5 — There is no override in the API.** A caller that genuinely wants to
replace a cache removes the file or names another path. See the rejected
alternatives.

**D6 — The refusal fires before the scan, and `CacheMode::load` stops
collapsing.** `load` currently answers `Ok(None)` for `Missing`, `Unreadable`,
`UnsupportedVersion` and `SourceChanged` alike, on the stated ground that the
distinction "would be information with no consumer". D1 creates the consumer:
`Missing` must lead to a scan-and-save and `SourceChanged` must not. The scan
entry point therefore has the reason in hand before it does any work, which is
what lets an hour-long scan be refused in the first second rather than after.

**D7 — The CLI fails, with the message as the deliverable.** `parse` reports
what it found, what it expected, and the two ways out — remove the cache, or
name a different path — and does not prompt. `parse` runs detached, under
`setsid` and in containers; a tool that blocks on stdin there hangs instead of
failing. `info` and `query` already report an unusable cache and read nothing,
and are expected to need no behaviour change, only re-checking.

**D8 — `M61`'s startup deletion goes.** `discard_unusable_cache` deletes a
condemned cache so nothing resumes from its span index. Under D1 that is the
prohibited act, one step earlier than the overwrite it was guarding against; the
refusal replaces it, and nothing resumes from the index because nothing scans.

## Rejected alternatives

***Rejected:* a `--force` flag, or a `CacheMode` variant meaning "replace
regardless".** It exists to be set once, in a script, and never reconsidered —
and from that moment the guarantee is gone for every subsequent run, including
the runs where the path was wrong. Removing a file is already in every caller's
vocabulary and is visible in the code that does it.

***Rejected:* refusing only when `--dqcache <path>` was given explicitly.** It
protects the case that is easiest to argue for and leaves the default path
silently replacing. See D2.

***Rejected:* prompting for confirmation.** See D7.

***Rejected:* the guard inside `cache::save`.** It would cover every caller
including a future embedder, but it turns a write into a policy decision an
embedder cannot override, and it pays an envelope decode on every throttled
save — which for a many-streams `.xz` means re-decoding a 31,150-entry seek
table repeatedly. The mistake being guarded is made once, at the start.

***Rejected:* treating this as a correction to P13 or as out-of-band work.**
See "Why, and what it reverses".

## Slices

**17.1 — `CacheMode::load` stops collapsing its four unusable statuses.** The
reason reaches the scan entry point; every caller updated to name the statuses
it already handles. **No behaviour change** — the review question for this
slice is precisely "does anything behave differently?", and the answer must be
no, which the existing tests are what establish.

**17.2 — The refusal, and the deletion goes.** The scan entry point refuses on
a source mismatch before doing any work (D1, D6); `discard_unusable_cache` is
removed (D8); `architecture.md`'s "not a new hard-error path" is reversed in
this same change, since this is the change that falsifies it.

**17.3 — The CLI surface.** `parse`'s message (D7); `info` and `query`
re-checked against D1 and changed only if they need it.

**A doc or manual claim is corrected by the slice that falsifies it**, not
deferred to 17.3 — `../process.md`, "Where does this fact go?" binds hardest on
the manual, and 17.2 is expected to falsify at least one claim there.
