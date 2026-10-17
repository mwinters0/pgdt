# P36.6.1 notes — the mapping pass takes the open

`pgdt_mapping::map_file` and `pgdt_dump::preamble_only` take an `Opened`
whole, in place of a source and a `CacheMode`. They build forward from the
cache the open decoded and judged, keep the open's watch, and save under the
mode the open carries. `pgdt parse` opens through `CacheMode::open`, so its
cache is decoded once: the claim's decode before the scan is gone from
`parse`. `pgdt query` still opens through `open_for_scan` and the claim until
36.8.

## The shape

- **`Opened` carries the mode it was opened under** (`Opened::mode`). The
  pass needs the mode to save and to refuse a recorded field
  (`refuse_recorded`), and carrying it means the cache a pass saves to is
  the one the open read. `pgdt info` and `PgDump::open` ignore it.
- **`CacheMode::open_source(Arc<dyn ByteRangeSource>)` opens a source
  already in hand**: the watch first, then the cache decoded once and
  judged against the source, with every refusal word for word
  `CacheMode::open`'s. Nothing the cache records reaches a source built
  before it was read, so an `.xz` whose seek table the cache holds is opened
  from its `Origin` instead. Every test that built its own source, a wrapper
  over another included, opens through it.
- **A cache refusal is the open's, not the pass's.** An unusable cache, a
  strict-identity refusal and an uncheckable source are refused before
  `map_file` or `preamble_only` is called. A cache the mode may replace
  reaches them as `Held::Replacing`, which starts cold, as
  `CacheLoad::Unusable` with no refusal did.
- **`parse` takes the cached statistics heap from the open**, the loaded
  index's `statistics_heap_bytes`, where it took `CacheClaim::Settles`'.
  The index is handed to the pass, which bills it as `Term::Loaded` as
  before, so nothing is decoded only to be sized and dropped.
- **The observation count is unchanged.** The open observes the source for
  its watch and again for the cache's stored size, as the pass's own watch
  and load did, so the identity tests' `Shifting` counts hold as written.

## Consequences

- **On `parse`, a strict-identity refusal and an uncheckable source now
  come before the resolved arrangement's announcement.** The stated flags
  and the limit are still announced first. Before, both refusals came from
  inside `map_file`, after the arrangement line. 36.6's notes named this as
  a cost of moving `parse` ("Negative results"); nothing pins the order.
- **`map_file` consumes the `Opened`**, so a caller wanting the source
  afterwards clones the `Arc` first, as `parse` does for its size and its
  arrangement. 36.7 has the pass hand back the parts a `Dump` is built from.

## Negative results

- **`map_file` taking the source, the watch and the `Held` as separate
  arguments** was refused. They are one open's values, and passing them back
  apart is `KD115`'s shape.
- **`map_file` and `preamble_only` taking `&CacheMode` beside the
  `Opened`** was refused: a mode passed apart can name a cache other than
  the one the open read and judged, which the pass would then save over.
- **`Opened` carrying only what the mode still decides after the open**,
  where it saves, was refused. Its strictness is the watch's and its
  `overwrite_unusable` is spent into `Held::Replacing`, but both are the
  open's own values, so they cannot disagree with what was done, and a
  narrower type would be a second public name and a second `save`.
- **`Opened` generic over a borrowed source**, so a test wrapper could keep
  borrowing the source it wraps, was refused. It would put a lifetime on the
  type the `Dump` is to own. The wrappers own their inner source instead.
- **A test helper restoring the old five-argument call** was refused. The
  refusal tests in `tests/cache.rs` and `tests/identity.rs` now assert the
  open's error, which is where the refusal lives.

## What the next round inherits

- **36.6.2** narrows `scan`'s and `preamble_only`'s options; the
  cancellation announcement D26 relies on moves to the prepass's callers.
- **36.6.3** moves the uncheckable-source refusal out of `SourceWatch::open`
  to where a read begins. The mapping pass and `preamble_only` now hold the
  open's watch, so their start is where that refusal goes.
