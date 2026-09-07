# P16.6 — `ByteRangeSource::partitions`

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "Execution model and API surface" and
"The compressed source".

## What exists now

**A fifth defaulted trait method, and it has no consumer.**
`ByteRangeSource::partitions(range) -> Partitioning` answers where this source
is willing to be split and what one concurrent reader costs it resident. It is
synchronous and pure — it reads state the source already holds and does no I/O —
which is what makes it reviewable with nothing calling it: a pure function's
whole contract is its return value, and a fixture pins it.

Three answers ship:

| Source | Boundaries | `partition_bytes` |
|---|---|---|
| the default body | one partition | 0 |
| `LocalFileSource` | `Anywhere` | one read chunk |
| `XzSource`, block-decoding | `At(block starts inside the range)` | one decoded block + one chunk buffer |
| `XzSource`, streaming fallback | one partition | one chunk buffer |

**The default declines rather than permits.** A source that has not thought
about being read concurrently answers one partition, so a scheduler cannot
split it on the assumption that silence was consent. That is the opposite
choice from the other four defaults, each of which answers what a plain local
file would.

**`XzSource`'s answer is read off the read path it took, not off the seek
table.** `partition_advice` is a free function over `(table, blocks, chunk
bytes, range)`, and the `blocks: Option<&BlockCache>` argument is the whole
decision: `None` — the streaming fallback — is one partition however many block
boundaries the table has, because reaching an offset inside a block there means
restarting that block and discarding forward, so two workers each force the
other's restart. Taking it as a free function is what makes that arm assertable:
a real file reaching the fallback has blocks above `BLOCK_DECODE_MAX_BYTES`,
which is a quarter-gigabyte of plaintext each and no fixture at all, so the test
hands a synthetic multi-block table in with `blocks: None`.

## The calls worth knowing about

**Two arms, not three.** `PartitionBoundaries` is `Anywhere` or `At(Vec<u64>)`,
and an empty `At` is one partition. A third `Single` arm was refused: it would
be operationally identical to `At([])`, and a second spelling for one outcome
reads as a second authority the way the free list's rejected byte cap did. What
`Single` would have carried is that the emptiness is a *policy* rather than an
absence, and that sentence is in the enum's doc comment and in the source's,
where the reason is.

**`Partitioning`'s fields are private and `at()` sorts.** The accessor promises
ascending, deduplicated offsets, so the guarantee is a property of the value
rather than of every source that builds one — which matters because the next
two sources to answer this (gzip, remote) will build one from a different
index.

**`partition_bytes` is what the *source's own pools* hold per partition, and
the doc names what it excludes.** For the compressed source that is a decoded
block slot plus the chunk buffer a straddling read is assembled into — 32 MiB
against koji's 24 MiB blocks, which is the number the phase's thesis uses.
Outside it are `xz-seek`'s fixed compressed input buffer and the LZMA2
dictionary; the dictionary is in each *block header*, which the seek table does
not carry, and `xz_seek::RangePlan::footprint` excludes it for that same reason
while naming 8 MiB a worker as the practical allowance. A guessed dictionary
would be wrong by 8× on a `-9` file, so the honest number plus its exclusions
beats a complete-looking one.

## What the next slice inherits

**A worker budget is `N × partition_bytes` plus what this method does not
charge for.** 16.7's `--parallel-memory` is where the dictionary allowance and
`xz-seek`'s input chunk get added, and where the two pools' budgets are made to
sum. Nothing here raises `POOL_BUDGET_BYTES`, so a `--jobs 8` run against
today's constant still thrashes — the block pool retains two blocks at 24 MiB
whatever the advisory says a partition costs.

**The advisory is an advisory: nothing enforces alignment.** A scheduler is free
to split an `XzSource` somewhere the advice did not offer, and pays two decodes
of the shared block for it
([`architecture.md`](architecture.md), "The compressed source"). 16.8 and 16.10.1
are what honour it.

**16.4.1 is not the next slice; 16.7 is.** Backpressure has no budget to wait
on until a caller states one, and — the finding this slice turned up — cannot
rest on the block pool's release-before-acquire discipline on the query path,
where `batch::RetainedChunks` holds views into more blocks than the pool has
slots. The reasoning is
[`../status/history/2026-09-07.md`](../status/history/2026-09-07.md), "16.4.1
waits on 16.7", and the spec's binding-orderings line now carries the
dependency. That finding also cost the row its option-validation commitment,
which the same day's review withdrew (that file, "The wait is exempted by
holder class, not validated by an option pair").
