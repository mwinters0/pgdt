# P17 — cache replacement: what the wrap leaves behind

The phase's mechanisms are filed by subject and are not repeated here:
[`architecture.md`](architecture.md), "The cache" holds the rule, the five-arm
`CacheLoad`, where the refusal's path comes from and the four alternatives the
phase refused; "The CLI's two refusals are worded as one" holds the two
conditions and the one tail they share; "The compressed source" holds the
contradicted compression claim, and what the size mismatch still pays to reach
its refusal. This doc holds what subject-filing has no home for.

## The residue: one cost, admitted rather than taken

A size-mismatch refusal over an `.xz` file still walks the stream footers first
— 85 s on the koji download, one read on every other shape — because
`cache::known_compression` collapses every unusable outcome to
`KnownCompression::Unknown` and `open_local` therefore has nothing to go on.
Admitted as `M62` with an empty `Blocks` column, since no decision the spec
records says where that answer is read and no slice was built around it
([`roadmap.md`](roadmap.md), "Out-of-band work";
[`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "The
size-mismatch refusal still walks an `.xz` file's footers first").

**It earned no `KD<k>`, and that is a decision rather than an oversight.** The
register's `(b)` stance wants a defect with a known fix and a named
destination; this one has both, and the ledger row *is* the destination. An
entry would put a second index over one queued item, and the property the
register exists for — that a session touching the mechanism meets its
limitations — is served by the fact sitting in "The compressed source" beside
the refusal it qualifies.

## No invariants entry is owed

Nothing in this phase depends on how `pg_dump` writes anything. The refusal is
keyed on a `stat` of the file at the cache path's recorded stored size against
the live source's, which is a property of the filesystem and of this project's
own envelope. `postgres-invariants.md` is untouched, deliberately.

## Negative results

**The reversal was cheap because the first slice bought a compiler-checked
edit.** 17.1 replaced `Option<DumpIndex>` with a five-arm `CacheLoad` and
changed no behaviour at all, so 17.2's whole library change was one arm at each
of three exhaustive matches, and "did we miss a scan entry point" was answered
by the compiler rather than by a search. A phase that reverses a decision
already implemented at several sites should expect to want that shape: the
slice that makes the sites enumerable lands first and is reviewable on the
question "does anything behave differently?", which has a yes/no answer.

**Guarding inside `cache::save` looks like the complete fix and is the wrong
one.** It would cover a future embedder as well as these three callers, which
is exactly its problem — it turns a write into a policy decision the embedder
cannot override — and it pays an envelope decode on every throttled save, which
for a many-streams `.xz` means re-decoding a 31,150-entry seek table repeatedly.
The mistake being guarded is made once, at the start. Filed beside the mechanism
too; repeated here because it is the alternative a fresh reading of the problem
reaches first.

**A refusal that still wrote is indistinguishable from one that did not**,
until the bytes at the cache path are compared. The error variant, the path and
both sizes are identical in either case, so the assertion that carries the
phase's guarantee is the one that reads the file back — not the one that names
the error.

## What went elsewhere

- **P14's inbox** — the mismatch payload is size-shaped
  (`CacheLoad::SourceChanged`, `Error::CacheSourceMismatch`), and a remote
  source identified by an ETag has no two sizes to quote.
- **The out-of-band ledger** — `M62`, above.
- **`architecture.md`** — everything else, at the wrap audit as well as in the
  slices; what the audit had to move is
  [`../status/history/2026-09-06.md`](../status/history/2026-09-06.md), "P17 is
  wrapped".
