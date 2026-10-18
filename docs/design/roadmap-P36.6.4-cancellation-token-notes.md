# P36.6.4 notes — the cancellation is the mapping pass's argument

`map_file(opened, &ScanOptions, &GatherOptions, &StatisticsRequest,
&CancellationToken)` takes `tokio_util::sync::CancellationToken`, required.
`ScanOptions` is `{ read, parallelism }`, `Copy`, and `scan`,
`scan_preamble`, `preamble_only`, `build_index` and `build_map` take
`&ScanOptions` again. pgdt-io's `Cancellation` and its module are gone, and
`ByteRangeSource::hint_cancellation` takes the token. No query entry point
takes one. D26, D48, D77 and D90 say so.

## The shape

- **`map_file` hints the source once, before its first read.** Before this
  slice, every pass's loop announced the cancellation again through
  `announce_read_loop`. The token is the same for the whole run and a source
  keeps it, so one hint covers the prepass, the forward pass and the
  back-fill. `announce_read_loop` is now the three read hints alone, and
  `announce_cancellation` is gone. pgdt-dump no longer knows of cancellation,
  and does not depend on tokio-util.
- **Inside pgdt-mapping the token is an argument**, `&CancellationToken`,
  beside the `&ScanOptions` of `map_forward`, `backfill_statistics`,
  `reread_rows`, `scan_region` and their helpers. `observe_rows` takes the
  `ReadOptions` alone. `cancelled_read` reads the token.
- **A query's passes poll a token nothing holds.** `map_for_query` makes one
  and hands it to `map_forward` and the metadata-level census's
  `reread_rows`. Neither can stop short, so the `Interrupted` arm is an
  `assert_eq!` and the census's `None` an `expect` (D26).
- **`gather_block_statistics` answers `Result<BlockReread>`**, not
  `Result<Option<_>>`. It takes no token, being no mapping pass, and its
  `None` meant only a cancellation.
- **The interrupt guard takes the token by value** and clones it into each
  signal's task.

## Negative results

- **`a_cancelled_query_errors_rather_than_returning_a_prefix` is deleted.**
  It pinned a behaviour this slice removes: a query handed a fired token. A
  dropped stream is the query's cancellation (D90), and no test pins a drop.
- **The replay and `TablePartitions::plan` still take a `ScanOptions`**, of
  which they read `read`. The spec has the replay take a `ReadOptions`
  (spec, "The small fixes"). The row does not name it. Changing it changes
  `TablePartitions::plan`, which 36.8 replaces, as 36.6.2's notes left it.
- **`run_until_cancelled` differs from the old `until_cancelled` in one
  case.** A token already fired returns `None` without starting the request.
  The old one started it and dropped it at its first poll. Either way the
  remote source answers `ScanCancelled` and sends nothing that completes.

## What the next round inherits

- **36.8's early-stopped pass takes the token** (history, 2026-10-18, "The
  cancellation is the mapping pass's argument"). `map_for_query`'s unfired
  token is where it goes, and the two arms above become reachable again. 36.8
  designs what they answer.
- **36.7's back-fill as one call** starts from `gather_block_statistics`
  returning a `BlockReread` outright.
