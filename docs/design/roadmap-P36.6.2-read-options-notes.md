# P36.6.2 notes — `scan` and `preamble_only` take only the fields they read

`pgdt_dump::ReadOptions` holds the read chunk and the line limit, which is
everything `ChunkedScanner` reads. `ScanOptions` is now that plus the
`Parallelism` and the cancellation: `ScanOptions { read, cancel,
parallelism }`. `scan`, `scan_preamble`, `preamble_only`, and
`pgdt_mapping`'s `build_index` and `build_map` take a `ReadOptions` and a
`Parallelism`. `ChunkedScanner::new` takes a `ReadOptions`. None of them is
handed a cancellation it would ignore (D26, D77).

## The shape

- **`scan` announces no cancellation.** It announces the read size, the
  parallelism and the wait policy, and nothing else.
  `internal::announce_cancellation` is the announcement on its own. The
  two passes that open with the preamble prepass make it before they call
  `scan_preamble`: `map_file` and `pgdt_query`'s `map_for_query`. Both made
  it before this slice too, through `scan`, so a read the prepass has in
  flight is still dropped (D26).
- **`announce_read_loop` keeps its signature.** It is the read hints plus
  the cancellation, for the passes' own loops: the forward pass, the
  back-fill, the census re-read and the replay.
- **`build_index` and `build_map` are narrowed too.** The row names only
  `scan` and `preamble_only`. But these two are `scan` with a builder, and
  they ignored `cancel` in exactly the same way, so leaving them on
  `ScanOptions` would have kept the shape the row removes.
- **`pgdt` derives both options from its flags.** `ReadFlags::options` gives
  the `ReadOptions`. `--preamble-only` passes those options and the
  resolved parallelism, which is what it read before.

## Negative results

- **Taking `cancel` off `ScanOptions` and making it `map_file`'s argument**,
  the way D77 places the statistics request, was not done. `pgdt_query`'s
  mapping pass polls the same field: `map_for_query` turns an interrupt
  into `ScanCancelled` (D48). Its entry points take the field through
  `ScanOptions`, so moving it would rewrite the stream entry points that
  36.8 replaces. This is under "Decisions worth another look".
- **A flat `ReadOptions` beside a flat `ScanOptions`** was refused. Two
  structs would declare the same two fields, and something would have to
  keep them equal.
- **`Parallelism` inside `ReadOptions`** was refused. `ChunkedScanner` does
  not read it, and a query's replay announces a parallelism different from
  its mapping pass's.

## What the next round inherits

- **The replay still takes a `ScanOptions`.** It reads `read`, and it
  announces the cancellation alongside `QueryOptions.parallelism`. 36.8's
  plan decides what the replay takes (spec, "The opened dump", cluster C).
- **36.6.3** moves the refusal of an uncheckable source to where a read
  begins. For `preamble_only` that is its start, which this slice leaves as
  it was.
