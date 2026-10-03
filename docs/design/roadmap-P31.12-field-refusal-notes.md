# P31.12 — A field PostgreSQL refuses fails the parse: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"What the register finds, and when the phase ends"; the rule is
[`roadmap.md`](roadmap.md), "A literal is guaranteed in `*_out`'s form and
never read past `*_in`'s"; the entry it closes was `KD75`. The row was split
([`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "31.12
lands split"): the kinds with no marked refusal are 31.12.1's, `KD83`.

## What exists

- **`decode::Unread` tells a refusal from text this build does not read**:
  `Refused` is answered only at a check carrying a `pg-refuses` marker, and
  everything else is `Unparsed` (D55). The readers a field's key goes through
  answer `decode::Read<T>`: `decode::float_in` (moved out of `predicate.rs`,
  field and literal sharing it), `predicate::int_in` (the field now bounded by
  its column's width too), `civil_days`, `date_days`, `parse_time_of_day`,
  `extract_offset`, `timestamp_micros_wide`, `interval_parts`,
  `typmod_unscaled_digits`, `network_key` and `parse_jsonb`, whose cursor sets
  a flag at its marked checks; `numeric_in_stores`' `false` and
  `decode_uuid`'s `None`, a whole port, map to `Refused`.
  `predicate::field_key` is `order_key` with the reason, `order_key` its
  `.ok()`; the `decode_*` the typed read calls keep their `Option`.
- **The parse decodes where the statistics gatherer keys**:
  `BoundsGatherer::observe` keys a field by `ValueKey::of_field`, and a
  `Refused` stops the block's observer, which frees what it holds as a
  decline does and answers `BlockGathered::Refused(FieldRefusal)` — the first
  in row order, a piece's kept only where no earlier row's was, and a refusal
  outranking a decline. `Builder::on_copy_end` turns it into
  `Error::FieldRefused`, pushing no span, so `map_forward` returns before the
  splice and the save; `reread_block` does the same, so a back-fill and
  `gather_block_statistics` fail alike.
- **Not checked at parse**: a column at the metadata level, a nested leaf, a
  value past `DICTIONARY_ENTRY_MAX_BYTES`, a declined block's rows, a bytewise
  kind (`text`, `bytea`), and a kind with no marked check (`KD83`). A query
  decoding one fails with `FieldDecode`, as it did. A query mapping for itself
  gathers nothing (`Pass::Query`), so it fails at its typed read the same way.
- **A text-held `numeric(p,s)` (past 76 digits) fails the parse past its
  precision**: gathering keys its field through the typmod, and keying is
  decoding — the question 31.15's notes left here.
- **A float past its range is refused by every reader**, the typed read
  included: `finite_spelling` is gone, and an underflow to zero is refused
  beside I57's overflow. A float literal in DataFusion's semantics, which took
  the field's reader, is read by Rust's parse, an infinity past the range,
  which `a_float_literal_past_its_range_is_not_refused_in_datafusion_semantics`
  holds.
- **`CACHE_FORMAT_VERSION` is 47**: `types/extra-float-digits-0` saves no
  data-level map. `PERSISTED_INDEX` is re-pinned over the sweep that maps that
  column at the metadata level; `GOLDEN_ORDER`'s digest did not move.
- **The sweeps share one list**, `pgdump_query/tests/common/refused.rs`
  (`REFUSED_FIELDS`, `sweep_request`, `past_refused`), which
  `datafusion-pgdump`'s and `pgdt`'s tests include by `#[path]`: a sweep maps
  a refused column at the metadata level, its table still censused and
  gathered, and `value_oracle.rs` asserts its read refuses.
- **Evidence**: `tests/decode.rs`'s
  `every_refused_field_fails_a_data_level_parse_reading_it` (every major) and
  `a_numeric_field_is_rounded_to_its_scale_and_refused_past_its_precision`;
  `tests/statistics.rs`'s `a_parse_fails_at_the_first_field_postgresql_refuses`
  (serial, 2, 4 and 8 jobs, and a back-fill; a shortfall failing nothing);
  `predicate.rs`'s
  `a_field_postgresql_refuses_is_told_from_one_this_build_does_not_read`;
  `decode.rs`'s `a_float_past_its_range_is_refused_and_one_in_it_read` and the
  `Refused`/`Unparsed` split of each date, time, interval and typmod test.

## Findings

- **A marked check can be reached by a shortfall's spelling**, and each was
  read for one. Two were: the run-together `+0530` reached `extract_offset`'s
  hour check as an hour of 530, so an hour of more than two digits is
  `Unparsed` before it; and `numeric_in_stores` counts digits on any text,
  where `numeric_in` checks its bounds after an exponent moves the point, so a
  bare `numeric`'s field is bounded only once the exponent-free grammar has
  read it. Every other marked check sits behind a grammar no wider than the
  server's at the parts it bounds.
- **The line is named by its byte offset**, as `LineTooLong` and
  `UnterminatedCopyBlock` name theirs; nothing counts lines (STATUS, "Decisions
  worth another look").

## What the slices after this inherit

- **31.12.1** gives `KD83`'s kinds their `*_in` grammar's refusals, each
  marked against an invariant, through `field_key`'s `Unparsed` arms.
- **31.13's `ignore`**: the 2026-10-02 entry has a field spelled as I57's
  float read as `DBL_MAX` under it, where `float_in` now refuses it in every
  reader. Reading it so takes the clamp back under that mode alone
  (`finite_spelling`, `git show 09107de4:pgdump_query/src/decode.rs`).
- **31.14's `strict`** decodes what this leaves unchecked; a clean `strict`
  parse is a full check only once `KD83` is closed.
