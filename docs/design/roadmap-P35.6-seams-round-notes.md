# P35.6 notes — the seams round

What the wrap inherits from cutting the four files `file_limits.py` named past
a hard limit. Each became its module's directory of one file a concern, the
old file keeping the module doc, the `mod` lines and the re-exports that keep
every path a caller names. No body changed and no test moved; a test file
gained only the imports its parent no longer carries. `CACHE_FORMAT_VERSION`
stands.

## How it fell

- **`pgdt-io`'s `io.rs`** → `source` (the read contract and its vocabulary,
  the default chunk), `parallelism` (`Parallelism`, `WorkerMemory`,
  `Partitioning`, `worker_count`, `cut`), `memory` (the allowance and its
  discovery), `local` (the plain source and `BufferPool`), `blocks`
  (`BlockCache` and `XzBudget`, which both `.xz` sources read through), `xz`,
  `fetched`, `origin`, `remote` (declared under `http`, so its items carry no
  `cfg` of their own) and `cancel`.
- **`pgdt-codec`'s `decode.rs`** lost its non-API checks: `network`, `json`,
  `bit`, and `geometric` (the I74 section, `strtod` with it, as the file had
  it). Each is a private child its parent re-exports `pub(crate)`, so
  `key::order` and `datetime_in` call `decode::<item>` as before.
- **`pgdt-query`'s `gather.rs`** → `tracking`, `strict`, `gatherer`, `column`,
  `bounds`, `order`, `dictionary`, `charge`; **`stream.rs`** → `resolve`,
  `map`, `backfill`, `table`, `plan`, `segment`, `map_query`, `note`,
  `partition`, `replay`, `dynamic`, `table_partitions`. Both module docs stay
  where they were, their links pointing into the children.
- **What a sibling or a test reaches went `pub(super)`**, fields included;
  nothing outside its module reaches anything new. A link a sibling's doc
  cannot resolve, the item being private to another child, is plain code.
- **`measure.py` declares each new file**: `READ`, `DECODE`, a new `STREAM`
  that `SCAN` and `MAP_BUILD` name, and `STATISTICS`'s gather files, a
  declared path being matched by prefix. `test_measure`'s checks that read the
  read path's constants off the source read every file of `READ`.

## What the wrap inherits

- **No file is past a hard limit.** Past a soft one: `main.rs`,
  `predicate.rs`, `key/order.rs`, `pgtype.rs`, `decode.rs`, `batch.rs`,
  `preamble.rs`, `nested.rs` and `pgdt-dump`'s `map.rs`. The round took the
  four files the spec's "over what is still red" names and no other.
  `decode.rs` stays past its soft limits because what is left is its API,
  which the namespace's own file defines (the spec, "What this phase does not
  do").
- **`statistics.rs` stays its own module**: the 2026-10-10 entry left renaming
  it or folding it into a split `gather` to this round, and nothing in the
  cut called for it. `gather`'s children read the account it defines.
- **The crate questions, asked.** Since 2026-07-01, mapped by the carve's
  assignment, most commits touching what is now `pgdt-dump` touched
  `pgdt-query` too, and after the cut every item `pgdt_dump::internal`
  exports is read by `pgdt-query`'s `stream` alone — the skill's two marks of
  a pair that should merge. D74 answers it: the parsers compile no Arrow, and
  the mapping pass gathers statistics, which decode values. No split is due:
  `stream`'s mapping pass and its replay share `gather`, which is Arrow-side.

## Negative results

- **`cargo fix` leaves an unused trait import in place** (`StreamExt`,
  `BlockObserver`, the `*Ext` traits): rustc reports it with no suggestion,
  so a cut copying a file's imports into each child removes those by hand.
