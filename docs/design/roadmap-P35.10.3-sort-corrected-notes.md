# P35.10.3 notes — 35.10.1's sort corrected

What 35.10.2 and 35.5 inherit from correcting the two calls 35.10.1 got wrong
([`../status/history/2026-10-10.md`](../status/history/2026-10-10.md), "The
codec's ordering keys, reviewed" and "The per-block re-read, reviewed"). Each
crate's `lib.rs` is where it lands. No body changed and no test moved;
`CACHE_FORMAT_VERSION` stands.

## How it fell

- **`pgdt_codec::key` is a fifth API namespace**, holding `ValueKey` alone.
  `ValueKey::compare` is no longer gated, so the codec's `test-support`
  feature gates only `scalar_tier` and `scalar_read`. Every signature
  `ValueKey` has names API alone (`pgtype::CompareKind`, `decode::Read`).
  `OrderKey`, `NestedKey`, `Side`, the functions building and comparing them
  and the refusal checks stay in `internal`.
- **The re-read is at `pgdt-query`'s root again**: `gather_block_statistics`,
  `BlockReread`, `bounded_columns` and `StatisticsBackfill`, with
  `StatisticsRequest::backfill` inherent and `StatisticsRequestExt` gone.
  `KD115`'s marker stays on `backfill`. `BlockGathered` is back at
  `pgdt-model`'s root.
- **`pgdt-query` has no `internal` module**: no crate sits above it but
  embedders, and its own integration tests now reach only its API.
- **The facade restores the five names 35.10.1 dropped**, and does not
  re-export `key`, since no embedder names it.

## What 35.10.2 and 35.5 inherit

- **Five namespaces to lay out**: `cache`, `decode`, `pgtype`, `instrument`
  and now `key`. Nearly all of `key.rs` is not API, so its seam runs between
  `ValueKey` and everything else. Naming the child module is 35.10.2's call.
- **35.5's check holds of this tree**: `ValueKey` has the one path
  `pgdt_codec::key::ValueKey`, nothing is both API and `internal`, and no
  `pgdt_query::internal` path is left to refuse.
