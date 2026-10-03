# P31.13.1 — A block records the refused fields an ignoring parse went past: notes

What the slices after this one inherit. The decision is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
cache records the refusals an ignoring parse went past"; where it now sits is
D103. The refusal it records is 31.12's
([`roadmap-P31.12-field-refusal-notes.md`](roadmap-P31.12-field-refusal-notes.md)),
the mode 31.13's
([`roadmap-P31.13-postgres-invalid-values-notes.md`](roadmap-P31.13-postgres-invalid-values-notes.md)).

## What exists

- **`CopyBlock::ignored_refusals`**, a boxed `IgnoredRefusals`, per column
  since 31.13.2
  ([`roadmap-P31.13.2-per-column-refusals-notes.md`](roadmap-P31.13.2-per-column-refusals-notes.md)).
  `FieldRefusal` is persisted as it stands.
- **The observer records it**: `ColumnGatherer::observe` answers `Observed`,
  its `ignored` set where `Ignore` kept a refused field out of the group;
  `Gatherer` adds it to its own record, a piece's folded in by `absorb` with
  its first's line numbered past the block's rows, as a refusal's is.
  `BlockObserver::take_ignored` hands it over before `finish`, so the
  `BlockGathered` variants did not change.
- **Who writes it**: `Builder::on_copy_end` for a mapped block;
  `backfill_statistics` from `BlockReread::ignored`, merged per column
  (31.13.2).
- **Who reads it**: `stream::refuse_recorded`, after `map_file` loads its
  cache and before anything is read, under `Default` alone — the first block
  in file order holding a record in a column the request tracks (31.13.2),
  failing with `Error::FieldRefusedRecorded`: the
  `FieldRefused` a read of the dump raises, boxed, and the cache's path, its
  message that one's with one sentence added. Boxed, as the block's record is,
  to keep `Error` and `DataBlock` under clippy's size lints.
  A query, the provider and `gather_block_statistics` do not read it.
- **`pgdt info`** prints `refused by PostgreSQL:` lines under such a block, and
  `--json` carries the field with the rest of the block.
- **Evidence**: `tests/statistics.rs`'s
  `a_parse_refusing_fields_fails_with_what_an_ignoring_parse_recorded` (no
  read of the block, the cache file unchanged, a metadata-level request and an
  ignoring re-run passing) and
  `a_parse_ignoring_refused_fields_keeps_no_statistic_of_one`, now asserting
  the record serially, at 2, 4 and 8 jobs and on a back-fill; `gather.rs`'s
  `a_refusal_is_numbered_by_its_row_in_the_block_however_it_was_cut`, half its
  rounds under `Ignore`, which fails with the fold's line adjustment removed;
  `pgdt/tests/statistics.rs`'s
  `parse_and_query_each_take_postgres_invalid_values` for `info` and the
  refusing parse's message.

## Negative results and limits

- **A declined block's record stops at its decline**: a declined observer keys
  no field, so the count, and whether there is a record at all, covers the
  rows before it. This is the same reach a refusing parse has, which never
  checks a declined block's rows either; 31.14's `strict` re-read is what
  covers them.

## What the slices after this inherit

- **31.14** adds its checked-in-full mark beside this record on the block. A
  `strict` parse re-reads a block holding a record rather than failing from it
  (D103), so `refuse_recorded` serves `Default` alone.
