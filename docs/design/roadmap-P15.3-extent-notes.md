# P15.3 notes — `extent()`

`ByteRangeSource::size()` and `size_is_exact()` are one `extent()` answering
`pgdt_io::Extent`, `Exact(n)` or `Open { at_least }`; every source shipped
answers `Exact`, so nothing a plain or `.xz` run does changed
([`roadmap-P15-gzip.md`](roadmap-P15-gzip.md), "Settled in the grilling";
[`decisions.md`](decisions.md), "D6").

- **What each site does with `Open`.** A read loop to the source's end is
  `ChunkedScanner::to_source_end`, which reads by `read_at_most` and takes the
  end from the read that answers short (or the empty one after a last full
  chunk), `ChunkedScanner::end` saying where; a loop bounded by a block keeps
  `ChunkedScanner::new` and the strict `read_range`. `scan` answers the end it
  reached. `scan_region` never cuts an open source and reports a
  `bound_by=source` shortfall where more than one reader was asked for.
  `splice` and `preamble_only` push no `Unscanned` tail where the end is not
  known, so a map ends at its watermark; the cache persists the extent the
  source states at the save (`CacheFile::extent`, `LoadedCache::extent`), and
  its tiling is checked to the watermark on load. A mapping pass that reached
  the end takes it from `ChunkedScanner::end` or the map's watermark, never
  from the source's later answer. `Error::MapIncomplete`'s `size` is `Option<u64>`;
  `pgdt info` prints bytes scanned and no ratio, and `--json`'s `total_size`
  is `null`. `pgdt parse` asks the extent after its run.
- **What 15.5 inherits.** A gzip source answers `Open { at_least }` until its
  first pass reads the end, and must answer `read_at_most` and `stored_size`
  itself: both defaults refuse an open extent (`Extent::require_exact`). Once
  it answers `Exact` it must never answer `Open` again. Its first-pass
  `PlanNote` replaces nothing here: today such a run states the `scan
  arrangement` shortfall line with no `would_hold_bytes`, which is the line
  that note's wording has to be read against.
- **`crates/pgdt-query/tests/open_extent.rs` is the evidence**: a source over a
  plain file stating no end until a read reaches it maps what the plain file
  maps at one and four workers and an odd chunk; a preamble-only pass over it
  saves an open extent and a map ending at its watermark with no tiling
  finding, which `Dump::opened` refuses and a later pass finishes into the
  same map. `scan::chunked::tests::a_scan_finds_an_open_end_where_a_read_answers_short`
  holds the scan's events and end against the exact case at every chunk size,
  a whole number of chunks included.
- **`CACHE_FORMAT_VERSION` is 73**: the envelope's length field became an
  `Extent`.

## Negative results

- **No `Unscanned` tail for an open source**, neither to the bound nor
  open-ended ([`decisions.md`](decisions.md), "D6").
- **`DumpIndex::is_complete(size)` stays in the model**, which cannot name
  `pgdt_io::Extent`; the library's own sites ask `Extent::reached_by` instead,
  and tests holding a plain file's cache still read it through a `u64`.
