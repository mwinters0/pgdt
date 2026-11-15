# P33.5.1 notes — the pools shrink and regrow

**A reader that parks gives back what its source keeps for it, and takes it
again as it resumes** (D162). `ParkedReader::park(source)` announces the park
(`ByteRangeSource::hint_parked(true)`) and its drop the resume, so a reader
leaving its pass by an error or a cancellation resumes too. Nothing parks yet:
33.5.2 and 33.5.3 are the callers.

## What the next slices inherit

- **A reader parks holding no read.** The guard is the promise: its chunk is
  the read it dropped before parking, which no pool has to give back. A reader
  still viewing a decoded block keeps that block alive, evicted or not.
- **What each source gives back**:
  - a plain file, nothing in its pool: its chunk pool is `POOL_DEPTH` deep
    whatever the reader count, so a parked reader's dropped chunk may land on
    the free list, the flat term `KD24` leaves unbilled;
  - a compressed file, local or fetched: the block pool's depth is the readers
    still reading (`POOL_DEPTH.max(reading)`), so above `POOL_DEPTH` readers
    each park gives back one retained unit, and every park drains the list to
    one below the slots at once (`BlockCache::give_back`), the block the parked
    reader last decoded among what goes. Together that is what
    `WorkerMemory::at(n) − at(n − k)` bills, the per-worker unit and the pool
    term, where the announced count is the delivered one;
  - a gzip first pass: its own pool is a chunk pool and keeps its depth; the
    count is kept and replayed on the indexed source at the hand-over.
- **Where a stated `--jobs` outruns the budget (`KD21`), parking gives back
  no retention**: the depth comes off the announced count, under which the
  budget still binds the slots. A ledger crediting a parked reader's term in
  33.5.2 is crediting bytes the pool did not free there; the marker says so.
- **Only the pools move.** The budget announced through `hint_parallelism` is
  untouched, so `block_path`, `block_decode_bytes` and `partitions` answer as
  before the park: a resumed reader reads in the arrangement it was carved in.
- **A lowered limit is given back at once on every pool**
  (`BufferPool::set_limits`), not only at a park: a re-announced smaller
  budget drops the free buffers over it as it is stated.
- **A resume past every park counts nothing**: the count saturates at zero.
- **Test wrappers forward `hint_parallelism` and not `hint_parked`**
  (`pgdt-query`'s `map_file.rs`, `identity.rs`, `wait_policy.rs`,
  `partitioned_replay.rs`, `pgdt`'s `determinism.rs`, `pgdt-mapping`'s
  `backfill/tests.rs`): the default ignores a park, so a parking test through
  one must forward it.

## Negative results

- **A decode in flight at the park lands after the drain**, so the list can
  stand at `slots − 1` plus the decodes that were in flight until the next
  decode drains it, as any decode's landing leaves it.
- **No count pair**: `BlockCache::slot` now drains through `give_back`, the
  same code once a block decode, and nothing a row or a byte runs changed
  (D108's trigger).
