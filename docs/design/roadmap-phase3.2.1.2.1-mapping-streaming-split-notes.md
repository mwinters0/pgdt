# Phase 3.2.1.2.1 — mapping and streaming split apart: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"Mapping and streaming are separate passes", which states the decision. This
records how it landed and what the next slice inherits. Per `CLAUDE.md`, this
file is consolidated into a single phase-level notes doc (and removed) once all
of Phase 3 lands.

## The shape `stream.rs` now has

`table_stream` is two phases with a hard boundary between them, and nothing
yields until the first is done:

1. **`map_forward`** — a free `async fn`, not part of the generator. It walks
   from `index.scanned_through`, drives a `map::Builder`, and after every
   `CopyEnd` splices the builder's `snapshot` onto the base spans, advances
   `scanned_through`, and persists. It returns when the target is settled or
   at EOF.
2. **Replay** — for each matching block in file order, a scanner over exactly
   `[block.header_offset, block.end_offset)` producing batches.

`Segment` and `Recorder` are both gone. So is every piece of bookkeeping that
existed only to keep the two passes in step: `block_start`, the incremental
live ambiguity check, the `is_live` branch in each event arm, and
`current_database` (the `Builder` owns the `\connect` tracking now, and a
replay segment takes its attribution from the block it is replaying).

**A live pass never parses a row.** `Event::Row` is `{}` — the scanner finds
the `\.` terminator, which is all the extent needs — so the mapping phase
allocates no `SourceChunk`s and takes no zero-copy views. That is why the
double read is cheaper than it sounds: the first pass over the block does no
per-row work at all.

## What the next slice inherits

**`splice` owns the seam, and `Builder` does not.** `Builder` has no start
offset; a builder that begins partway through a file opens its first span at
its own first recognized content, which is *after* the byte it began reading
whenever blank lines separate the two. `splice` closes that by **extending the
preceding span to where the next one starts** — the same rule `push_span`
applies to every other boundary, and the one the design's "Span boundaries:
object-anchored and greedy" states (interstitial blank lines belong to the
span before them).

An earlier attempt put a start floor on `Builder` instead, stamping its first
span at the segment start. It tiles, but it hands the blank line *after* a
block to the following span, where a single-pass scan hands it to the block —
so a map assembled from several scans stopped agreeing with a map built in
one. The test now asserts that agreement (`full.spans == eager.spans`), which
is the property to preserve: **how a map was assembled must not be visible in
it.** Anything that changes where a scan can stop has to keep that true.

**`target_settled` is the stop rule, and it is deliberately conservative.**
Two things make a further block able to share the queried name, and both veto
an early stop:

- a matching block carrying `partition_root` (I2 — the blocks are not
  adjacent, so only EOF enumerates them);
- **any `Connect` span anywhere in the map.** A `pg_dumpall`, a concatenation
  or a `--create` dump can define the same qualified name again in a later
  database, and stopping early would return one candidate's rows where
  `Error::AmbiguousTable` is owed. This was not in the plan; the existing
  `querying_a_table_name_shared_by_two_databases_errors_without_a_database_selector`
  test caught it. A `batch_options.database` selector does **not** lift it —
  two `\connect` segments can name the same database.

So the early stop applies to exactly the common case: a single-database,
non-partition-root dump. koji is one.

**Ambiguity is now detected before any row goes out.** The check runs once,
over every candidate the map holds, between the two phases. The Phase 2 known
gap that said a cold query could emit rows before `Error::AmbiguousTable`
surfaced is closed — for everything the scan reached. What replaces it is
narrower and is in `STATUS.md`'s "Known gaps".

**`ResumeToken` lost its `database` field.** It existed to carry `\connect`
state into a resumed *live* segment; resumed streams now replay out of the
map, which already attributes every block. `InCopyResume::database` stays —
that one is the paused block's own attribution, used to resolve its schema.

**Resume no longer has a fallback path.** A resume point is inside a mapped
block by construction, so `resume` is just "skip blocks that end at or before
the token's offset, and start the first surviving one at the token's offset
instead of at its header". The `roadmap-phase1-mvp.md` known gap about
resuming out of a cache replay degrading to a live scan is gone with it.

**`CopyBlock::partition_root` is set by `map::Builder`, from a line, not a
conclusion.** The marker is separated from its `COPY` header by a blank line,
which closes whatever comment block held it — so no `Mode` still carries it
when `on_copy_start` runs. It is tracked on the builder itself
(`pending_partition_root`), recognized in `feed_line` before mode dispatch,
consumed by the next `CopyStart`, and cleared by the next TOC `Name:` line so
an entry that turned out not to be table data leaves nothing for the following
one to inherit.

**The preamble prepass's spans are kept.** They tile `[0, preamble_end)` and
are exactly the prefix `map_forward` splices onto; discarding them (which
3.2.1 did) would leave the map with nothing beneath its frontier.

## Fixtures

`scripts/fixture_schema_partitions.sql` is new, under
`{default, --load-via-partition-root}`. The `_a`/`_m`/`_z` naming is
load-bearing and the file says so: `TABLE DATA` entries sort by the
*partition's* own name, so `evt_m`/`feel_m` exist to be emitted *between* two
partitions of the same root. `feel` is hash-partitioned on an enum column,
which makes `pg_dump` force load-via-partition-root with no flag at all — so
the `default` set exercises the multi-block shape on all six majors, not just
the flagged one. `spread` has a partition with no rows, so a root name owns a
zero-row block too.

`tests/map.rs`'s fixture discovery is directory-driven, so these were pulled
into `every_fixture_tiles_exactly` without touching it.

## What this slice does not do

- **Span text and the `Diagnostic` channel** — 3.2.2. `check_tiling` still has
  no production caller; the tiling claim is asserted in tests only.
- **The `-- load via partition root` marker is not surfaced anywhere a user
  can see it.** `pgdq info`'s block listing is 3.5.
- **A TOC-commented `COPY` block still tiles as two spans**, not one: the
  blank line between the comment and the header pushes the builder into
  `Mode::Statement`, so `on_copy_start` closes that as its own span and the
  `Data` span starts at `header_offset`. The design's `span.start <=
  header_offset` invariant holds either way, and this predates the slice —
  but "COPY blocks are the one exception" reads as though the comment is
  absorbed, and it is not. 3.3's TOC layer is where entry-to-span attachment
  gets settled properly.
