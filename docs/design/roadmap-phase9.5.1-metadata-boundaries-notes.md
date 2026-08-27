# 9.5.1 — `parse` states its metadata at every legal boundary

What the phase wrap and the koji run inherit from the slice that made an
interrupted scan's cache *typed* rather than merely labelled.

## The shape

Three edits, one mechanism.

**The prepass, in `stream::map_file`.** `index::scan_preamble` runs before
`map_forward`, under the same "unless the first database's `preamble_complete`
is already known" guard `table_stream` has always used, and saves before the
mapping loop's first cancel check. It runs **only for a scan starting at byte
0**: its spans *are* a tiling of `[0, preamble_end)`, i.e. the prefix a resumed
scan splices onto, not something to splice onto one — so running it over a
resumed map would assign `index.spans` a preamble-only list while
`scanned_through` stayed high, which is a corrupt index rather than a wasted
read. `resumed_from` is captured before it, so the prepass's own advance never
prints as a resume point.

**The recurring recompute, in `stream::map_forward`.** At `Event::CopyStart`,
when the builder's governing database differs from the one the metadata in hand
was computed at, `index.metadata` is recomputed from
`splice(prefix, builder.snapshot(boundary), …)`. `metadata_covers:
Option<Option<String>>` is the state — the outer `None` meaning "no metadata at
all", the inner one "a database with no `\connect` to name it" — seeded from
`metadata.databases.last()`, because `dump_metadata_from_spans` finalizes the
last database it walks, so the last entry is the one whose boundary the
computation stood on.

**The clone move, in `table_stream`.** `index.metadata` is read *after*
`map_forward`, not before. That is the whole of what makes a cold query type a
`pg_dumpall`'s later databases, and it is the queued out-of-band item this
slice absorbed.

## Five calls the wrap should know about

**`Builder::snapshot` is sound right after `on_copy_start`, and that is a
second boundary its docs did not name.** `on_copy_start` flushes any pending
large-object region and `mem::replace`s the mode to `Idle` in every arm, and
the `Data` span is still *pending* — so `self.spans.last()` is the DDL span
before the block and closing it out at the block's start offset is exactly
right. The offset to close at is `pending_comment_start().unwrap_or(header_offset)`,
read **before** `on_copy_start` consumes the comment, which is the same idiom
`scan_preamble` uses and the reason a `-- Data for Name:` comment is not
swallowed into the preceding span.

**The trigger is the database, not the block.** Recomputing at every
`CopyStart` and leaning on idempotence would put a third whole-index-sized cost
in the loop `measurements.md` already prices two in. A single-database dump —
koji included — recomputes **zero** times inside `map_forward`: the prepass
already stood on that same offset, so the first `CopyStart`'s database equals
the covered one.

**A metadata-less partial cache self-heals.** The prepass is skipped on a
resume, so a cache written by the previous build (which banked no metadata at
all) would still have none — except that `metadata_covers` is then `None`,
which differs from any database, so the next `CopyStart` recomputes. If the
scan reaches EOF instead, `map_file`'s own recompute covers it. Nothing needed
a cache format bump.

**The two `MetadataNotScanned`s are no longer equally reachable, and only one
of them is reachable at all.** A database's DDL is stated before any of its
blocks can be banked, so a block in the map always has its database covered.
`ColumnResolution::MetadataNotScanned` survives that because `resolve_columns`
is `pub` and takes caller-supplied metadata; `Error::MetadataNotScanned` does
not — `stream::resolve_block` is private, its three call sites are all in
`table_stream`, and all three use the `metadata` read *after* that call's own
`map_forward`, `resume_state` included, so a carried `ResumeToken` does not
reach it either. Both are kept: the check is what stands between a future
reordering (moving that read back above the mapping pass) and a silently
wrongly-typed row. Because that left it as *untested* defence — the test that
exercised it was this slice's own rewrite — `stream.rs` gained a unit test
calling `resolve_block` directly. `partial_reporting.rs` still pins the
reporting half, against a deliberately hand-built pairing
(`write_truncated_cache`'s `keep_databases`) rather than one a producer leaves.
The narrowing is filed into `roadmap-phase6-inbox.md`, where the embedded API
has to pick between the two answers.

**The spec's third-database assertion could not be written as stated.** It asks
for `MetadataNotScanned` on "the third's blocks" after cancelling inside
database 2 — but a scan stopped in database 2 has banked no database-3 blocks
at all, so there is nothing to resolve. What the test asserts instead is the
checkable half of the same claim: both finished databases come back
`preamble_complete` with real DDL, every banked block resolves without
`MetadataNotScanned`, and resuming still matches an eager pass span for span. A
prepass-only implementation fails it, which is the discrimination the spec
wanted.

## Testing the recurring half needs the *right* multi-database fixture

`edge_cases/dumpall.sql` — the fixture the spec names — had `COPY` blocks in
only **one** of its three databases when this slice landed, so the file's first
`COPY` header closed out every database that could matter and the recurring
boundary was never reached twice. The vehicle this slice used was therefore the
concatenated pair of `edge_cases/create.sql` copies with the second's database
renamed (`tests/map_file.rs`'s `multidb`, the same construction
`tests/pgtype.rs` and `partial_reporting.rs` use): two databases, both with
blocks. 9.5's `CancelsPast` trips the flag at a chosen file offset, so
cancelling inside the second database's data is deterministic.

**That stopgap is gone.** The 2026-08-27 grilling reversed it, and the
out-of-band round that followed gave `generate_fixtures.py` a second
data-carrying database, `pgdq_tenant`, so `dumpall.sql` itself reaches the
boundary twice — see [`architecture.md`](architecture.md), "Fixtures". The
recurring-boundary test runs on the real dump; verifying a mechanism licensed
by I1's *scope limit* only against a file `pg_dump` never wrote was the gap.

**The concatenated case stays even so.** A real `pg_dumpall` and a bare
concatenation are different shapes — I9: `pg_dumpall` passes `--create` for
ordinary databases but writes `postgres`/`template1`'s `\connect` lines
itself, so those segments' version headers land *after* their `\connect` — and
the concatenation is what a user gets from `cat a.sql b.sql`, which nothing
else covers. So `map_file.rs` has both — the same assertions run over each —
and the helper does not go away.

## Verified with a real signal

The library tests cannot deliver one — a fixture parses in milliseconds — so
the end-to-end check is by hand against a 4000-block bench file
(`scripts/generate_block_count_bench.py`), whose parse takes ~20 s: `SIGTERM`
at 6 s → exit 143, cache at 67%, and `pgdq info --verbose` on it prints
`id: Int32` / `amount: Decimal128(12, 2)` where the same command before this
slice printed `not declared — no DDL explained this column` for every column.
Resuming produced a cache **byte-identical** to a straight-through parse's.

The cold-parse wall time on that file is 20.4 s against 9.5's 23.5 s — noise on
an unpinned machine, not an improvement to claim. The prepass reads the
preamble once and `map_forward` resumes from where it stopped, so nothing is
read twice. What the prepass itself costs is
[`measurements.md`](measurements.md), "The preamble prepass is bounded by the
schema, not by the dump": 63,333 bytes on koji, and 0.04 s on the
most preamble-heavy shape available.

## What the koji run adds

The real-scale half of the prepass, and the item `STATUS.md` puts on that run's
checklist: an interrupted koji cache must come back **typed**, not reporting
`not declared` for every column. koji is single-database, so it cannot observe
the recurring half at all.
