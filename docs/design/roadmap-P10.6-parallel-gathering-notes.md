# P10.6 — Parallel gathering: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Gathering across the layers". Nothing a query does changed, and no figure was
taken.

## What exists

- **A gathered block is offered to the leader like any other.** `map_forward`
  installs the observer and hands `Builder::block_observer` to
  `leader::scan_region`; each partition observes its rows into an observer
  `BlockObserver::piece` made, carried through `scan_partition`'s reads, and
  `run_region` absorbs them in file order up to the partition holding the
  terminator, the rule `merge` stops by. A block closed there is finished at
  `CopyEnd` exactly as a serial one is. `BoundBy::Statistics` and
  `gathering_shortfall` are gone, so a gathering `parse` prints no `scan
  arrangement` line of its own.
- **`Gatherer::join` is the whole fold.** A piece lists no group ahead of its
  first row's and holds that group open (`Gatherer::head`) rather than closing
  it; the join continues the block's open group where the indices agree,
  closes it where the piece's next group starts, and appends the piece's
  closed groups, re-interning dictionary entries in the order a pass would
  have. Each ordered column carries its first value to the join —
  `FirstValue`, a key or the first `CLIP_BYTES` bytes and the length, which is
  all `Clipped::locate_bytes` reads.
- **Bytewise extremes join by their heads alone** (`Clipped::order`); the
  rustdoc on `Clipped` is why that stores what one pass stores.

## Tests

- `gather.rs`'s `pieces_joined_in_file_order_gather_what_one_pass_gathers` —
  random blocks of the three bytewise kinds and a keyed one, cut at random rows
  with empty pieces between, joined against one pass, `BlockStatistics` equal.
  Its floors count cuts inside a group, ordered columns, truncated upper bounds
  and dictionaries past the count cap.
- `pgdump_query-cli/tests/determinism.rs`'s
  `every_fixture_gathers_the_same_statistics_at_every_stated_parallelism` —
  every fixture at a 32-byte group, `--jobs 8` and `3` at chunks the leader
  cuts at, the cache byte for byte against `--jobs 1`.
- `tests/statistics.rs`'s `a_gathering_scan_is_the_serial_scan_whatever_the_worker_count`
  pins an ascending and a descending column across the cuts, and
  `tests/wait_policy.rs` now gathers, so a gathering scan is shown reaching the
  scheduler. `status_output.rs`'s `a_gathering_parse_is_not_corrected`
  replaces the test of the removed line.

## For the slices after

- **10.7.** Landed: [its notes](roadmap-P10.7-backfill-notes.md). A back-fill re-reads a block whole; offering it to `scan_region`
  with the block's observer is the parallel path, nothing else being needed.
- **10.10.** `statistics-gathering` can run a gathering `parse` at a stated
  `--jobs`; the harness's shapes still state `--jobs 1`.

## Negative results

- **Seven hand mutations each failed the random test**: the join's row-order
  step dropped, a keyed and a bytewise extreme joined the wrong way, the
  dictionary count cap skipped at a join, a first value cut at a character,
  the joined group's row count dropped, and the order flags not folded. Of the
  two also run against the fixture sweep, the dropped row count failed it and
  the dropped step did not: over `pg_dump` output a lost step at a join rarely
  changes a block's order, so the random test is the one that guards it.
- **Two mutations passed, being equivalent**: a bytewise tie taking the later
  head, and `Clipped::order`'s `None` read as an order. Both store the same
  bounds, which is the `Clipped` rustdoc's claim, tested.
- **A window's partitions past the terminator gather too** before they are
  discarded, as they already scanned (`KD22`); what they hold is unbilled with
  the rest of statistics (`KD28`).
