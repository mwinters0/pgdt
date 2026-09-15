# P20.11 — The cache file streamed at both ends: notes

`cache::save` encodes a borrowed `CacheFileRef` through a `BufWriter` into a
file beside the cache and renames it over the cache; `cache::read_cache_file`
decodes through a `BufReader`. Why is [`decisions.md`](decisions.md), "D78".
`cache.rs`'s own tests hold the two properties: a save's bytes are what the
owned `CacheFile` encodes to in one buffer, over a plain and an `.xz` source
with bounds and dictionaries gathered, and a save stopped partway leaves the
previous cache whole with its partial file beside it — stopped at that moment
rather than killed, no test racing a signal ([`decisions.md`](decisions.md),
"D73").

## What the next slices inherit

- **Neither end of the file is a term any more.** 20.7's decline and 20.8's
  attribution meet a save holding the index it was handed and nothing encoded,
  and a load holding what it has decoded so far plus a reader's buffer. A
  `query` still decodes the cache twice in turn — `cache::claim`, dropping its
  index, then `cache::load` — which is D78's whole-file decode, not this slice's.
- **A save no longer clones the index.** It borrows it; `CacheFile` and
  `CacheFileRef` list the same fields in the same order, and the byte-identity
  test is what catches the two drifting.
- **A killed save leaves `<cache>.<pid>-<n>.tmp`** beside the cache, removed by
  nothing. The manual says it is safe to delete
  ([`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`parse`:
  reading the dump"); the naming is under STATUS's "Decisions worth another
  look".
- **A file cut short reads `Unreadable`, and any other I/O failure mid-decode
  is `Error::Io`**, as a failed whole-file read was before.
- **The save-count trace still counts one open a save**: `measure.py`'s
  `count_saves` filters on the cache path, which begins the beside file's path.
- **How fast a load decodes through a reader rather than a slice is unpriced.**
  Every figure reading a cache is already red under `measure.py --stale`, and
  20.9 re-takes them.

## Negative results

- **A garbage length in a foreign file costs what it did.** bincode's owned
  decode of a string or byte vector allocates the declared length before
  reading it, from a slice as from a reader, so streaming the load neither
  opens nor closes that exposure.
