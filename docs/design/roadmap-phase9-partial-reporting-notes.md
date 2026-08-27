# Phase 9 — Partial reporting and machine-readable resolution: notes

The phase's residue after the wrap audit. Every mechanism it built is described
by subject in [`architecture.md`](architecture.md) — the verb split and the
`parse` loop under "CLI surface", the four-way `CacheStatus` and the recomputed
diagnostics under "The cache", the recurring metadata boundary under "The
preamble grammar and `DumpMetadata`", `snapshot`'s two sound boundaries under
"The file map" — so what is left here is what subject-filing has no home for:
what the phase tried and abandoned, and where its facts went.

## What the phase got wrong on the way, and what that is worth

**A real-scale sample cannot decide a cost that is per-block.** 9.1 was told to
build the save throttle "only if the number demands it", measured koji — 74
blocks over 784 GB, +1.5% wall, 18 MB written — and correctly declined. The
number was sound and the conclusion was wrong, because every save serializes
the *whole* index, so the cost is O(blocks²) and koji cannot express the
regime: 4000 small blocks in a 2 MB file cost 44 s. The throttle became slice
9.5. The transferable part is that koji is the project's only real dump and it
is one *shape* — big blocks, few of them — so a per-block or per-span cost
measured only there is unmeasured. `scripts/generate_block_count_bench.py`
exists to be the other axis.

**A spec that enumerates check points licenses the ones it forgot.** The
interrupt guard was specified as "read the flag once per chunk", which is right
for koji's hundred-gigabyte blocks and useless on a 4000-block dump whose whole
23-second scan is two chunks — the first end-to-end `SIGTERM` test sat there
for twenty seconds. The fix was to state the *rule* ("read the flag at every
point the loop can cheaply reach") and let the two current sites be its
instances. `architecture.md` carries it that way, with the two limits the rule
does not remove.

**Two `MetadataNotScanned`s, and the record of why they stay was wrong twice.**
9.4 added `ColumnResolution::MetadataNotScanned` beside the existing
`Error::MetadataNotScanned` on the streaming path — a reported schema degrades
where a streamed one refuses. 9.5.1 then made the mapping pass state each
database's DDL at that database's first `COPY` block, which removed the only
producer either could see: a block in the map always has its database covered.
The grilling that followed found the queued justification for keeping them
false in its own terms (`stream::resolve_block` is private, and all three call
sites read `metadata` *after* the mapping pass, so a carried `ResumeToken` does
not reach it either). Both are kept anyway, as what stands between a future
reordering of that read and a silently wrongly-typed row, and the error path is
now pinned by a unit test calling `resolve_block` directly rather than left as
untested defence.

**The spec's discriminating test could not be written as specified.** 9.5.1 was
to assert `MetadataNotScanned` on a third database's blocks after cancelling
inside the second — but a scan stopped in database 2 has banked no database-3
blocks at all, so there is nothing to resolve. The checkable half of the same
claim is what `tests/map_file.rs` asserts: both finished databases come back
`preamble_complete` with real DDL, every banked block resolves without
`MetadataNotScanned`, and resuming still matches an eager pass span for span. A
prepass-only implementation fails it, which is the discrimination the spec
wanted.

**A stopgap fixture was built and then removed.** 9.5.1 first tested the
recurring boundary against two concatenated copies of `create.sql` with the
second's database renamed, because `edge_cases/dumpall.sql` carried `COPY`
blocks in only one of its three databases. The 2026-08-27 grilling reversed
that: a mechanism licensed by I1's *scope limit* should not be verified only
against a file `pg_dump` never wrote, and the out-of-band round **M9** gave the
fixture a second data-carrying database. The concatenated construction stayed
regardless — it is a different shape (I9) and it is what `cat a.sql b.sql`
produces — so both run the same assertions.

## Where the phase's facts went

- **Mechanism** — [`architecture.md`](architecture.md), by subject, sections
  named above.
- **Figures** — [`measurements.md`](measurements.md): "koji full scan — the
  regression check" (the write amplification, and the stop-report-resume-compare
  run whose resumed cache is byte-identical to a straight-through scan's),
  "Per-block cache saving is quadratic in block count, and so is the map", and
  "The preamble prepass is bounded by the schema, not by the dump".
- **External behaviour** — I1's entry in
  [`postgres-invariants.md`](postgres-invariants.md) gained this mechanism
  under *Relied on by*; the recurring recompute is licensed by its scope limit,
  not by a new invariant.
- **Accepted deficiencies** — [`../status/STATUS.md`](../status/STATUS.md),
  "Known gaps": the remaining half of the mapping quadratic, and the two array
  refusals Phase 4 left.
- **Facts for phases with no spec** — [`roadmap-phase6-inbox.md`](roadmap-phase6-inbox.md)
  (four entries: the two diagnostic channels, the reported-vs-streamed schema
  answer as narrowed by 9.5.1, cancellation and what an embedder's idiom does
  with it, and per-block keying leaving block-to-table to that phase) and
  [`roadmap-phase7-inbox.md`](roadmap-phase7-inbox.md) (the span splice, which
  is the quadratic the throttle did not remove).

## What the next phase inherits directly

`stream::map_file` is a second whole-file producer beside `index::build_index`,
and they are pinned to each other by `tests/map.rs`'s
`build_index_spans_match_build_map_exactly` and `tests/map_file.rs`. Anything
that changes how a map is assembled has to keep the property those tests
encode: **how a map was assembled must not be visible in it** — the same
property `splice` was built to preserve, now also spanning an interrupt and a
resume.
