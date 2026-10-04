# P31.14 — `--postgres-invalid-values=strict`: notes

What the phase's wrap inherits. The decision is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
cache records the refusals an ignoring parse went past"; where it now sits is
D103. The refusal it extends is 31.12's
([`roadmap-P31.12-field-refusal-notes.md`](roadmap-P31.12-field-refusal-notes.md)),
the record beside its mark 31.13.2's
([`roadmap-P31.13.2-per-column-refusals-notes.md`](roadmap-P31.13.2-per-column-refusals-notes.md)).

## What exists

- **`PostgresInvalidValues::Strict`**, on `ScanOptions` alone in effect: `pgdt
  parse --postgres-invalid-values strict` (`CliParseInvalidValues`; `query`'s
  flag keeps `default|ignore`), and a query or the provider handed it reads as
  under `Default`. The shell and `pgdump.*` still refuse the word.
- **The check is `predicate::field_refused`**: a scalar is `field_key`'s
  `Refused` under its column's PostgreSQL kind, divergent or not; a nested
  value goes through each container's *input* grammar (`nested::parse_*`, I44,
  I47), each element read as a field of its type and a range's bounds through
  `make_range` (I46) — `checked_key`, which reads every element whatever an
  earlier one answered. A `Refused` or `Unanswerable` plan, and `Text` kinds,
  check nothing.
- **`gather::Gatherer` carries the checks** (`checks`, one `FieldCheck` per
  column, shared with its pieces): each row is checked before it is gathered
  and after a decline, a refusal folding through `absorb` with its line as
  31.12.2 numbers it. `gather::checker` is a `Gatherer` that gathers nothing
  (`gathers: false`): it opens no group, never declines, and answers
  `BlockGathered::Checked`. Under `Strict`, `observer_for` returns one for a
  block no request tracks, and `observer_tracking` adds the checks.
- **`CopyBlock::checked_in_full`**, set by `Builder::on_copy_end` from
  `BlockObserver::checks_every_field` and by a re-read from
  `BlockReread::checked`. `CACHE_FORMAT_VERSION` is 52 and `PERSISTED_INDEX`
  re-pinned: every block persists the flag.
- **Held blocks**: `backfill_statistics` takes every block with the flag clear
  under `Strict`, gathering in the same read whatever it lacks
  (`Backfill::Lacking`); a resumed map short of EOF runs it first over the
  held blocks alone (`Backfill::Unchecked`) and saves, so the first refusal
  met is the first in file order. A check-only re-read sets no census, as a
  strict mapping pass records nothing more of a metadata-level block.
  `MapRun::checked` counts them; `pgdt parse` names them on its resume line.
- **`refuse_recorded` still returns early for every mode but `Default`**:
  `Strict` re-reads the block holding a record, the record holding only what
  an ignoring parse's gathering keyed.
- **`pgdt info`** prints `checked: every value, by a strict parse` under such a
  block; `--json` carries `checked_in_full`.
- **Evidence**: `tests/statistics.rs`'s
  `a_strict_parse_refuses_the_fields_a_default_one_leaves_to_a_query` (a
  metadata-level column, an array element, a value past
  `DICTIONARY_ENTRY_MAX_BYTES`, a range out of order, a declined block's row;
  serial and four workers, which fails with the pieces' checks removed),
  `a_strict_parse_checks_each_held_block_no_strict_parse_checked` (one read
  checking and gathering, none the second time, an ignoring parse's record
  passed over for the re-read's earlier refusal) and
  `a_strict_parse_resuming_checks_what_the_cache_holds_first` (which fails
  with the resumed pass removed); `tests/decode.rs`'s
  `a_strict_parse_refuses_no_fixture_field_but_the_listed_ones` (every
  fixture, data and metadata level); `predicate.rs`'s
  `a_strict_check_finds_a_refusal_anywhere_in_a_field`; `pgdt`'s
  `a_strict_parse_checks_a_held_cache_and_info_says_so`.

## Negative results and limits

- **A type held as its text is checked for nothing** but `json`, `bit`,
  `bit varying` and the geometric types, which 31.16–31.18 read by their
  input grammars — `xml`, `money`, a user base type — nor is a range
  declaring its own `canonical` function; no reader here decodes them. The
  manual says so. What stays outside is the roadmap's boundary, and 31.21
  names it per dump.
- **No null in a `NOT NULL` column is refused**, under any mode: 31.20. A
  length past a `varchar(n)` or `char(n)` typmod is, since 31.19.
- **What a strict observer holds to check — a comparison plan per column —
  is not charged to the statistics account** (D81): it is fixed per block,
  shared with the pieces and freed with the observer.
- **An enum label is refused only where its labels are exact**, as under
  `Default` (I70, `KD87`); `KD88` is unchanged for a query.

## What the wrap inherits

- Every P31 slice is ticked. `M210` and `M211` stay queued, blocking nothing.
