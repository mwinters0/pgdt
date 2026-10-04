# P31.22 — What a reader cannot read, refused where every major refuses it: notes

What the slices after this one inherit. The row was split, the dates, times,
timestamps and `interval` going to 31.22.1
([`../status/history/2026-10-04.md`](../status/history/2026-10-04.md), "31.22
lands without the date and time types"). The mechanism is 31.12.1's: the
reader stays as narrow as it was, and a classifier runs where it fails
([`roadmap-P31.12.1-input-function-ports-notes.md`](roadmap-P31.12.1-input-function-ports-notes.md)).

## What exists

- **`decode::int_unread`** (I80), called by `predicate::int_in` where Rust's
  parse fails: v16's `pg_strtoint*` grammar, which reads every text v13 to v15
  read, with the width (I60). Refused where it refuses or the value is past
  the width.
- **`decode::float_unread`** (I81), called by `float_in`: glibc's `strtod`
  grammar between blanks (`strtod_token`), a decimal token's range by
  `float_in` itself (I59), a hexadecimal one's by
  `hex_float_out_of_range`, rounding to nearest even into the type's
  significand and subnormals (`Float::SIGNIFICAND_BITS`, `MAX_EXP`,
  `MIN_SUBNORMAL_EXP`).
- **`decode::numeric_unread`** (I82), called by `typmod_unscaled_digits` and
  by `field_key`'s bare `numeric` arm under a `NumericColumn`: the text read
  by `numeric_spelling_v14` and by `numeric_spelling_v16`, each spelling held
  to the column by `NumericSpelling::stored` — the storage bounds for a bare
  column (I63), rounding and precision for a typmod'd one through
  `typmod_round`, the rounding split out of `typmod_unscaled_digits` (I51),
  an infinity under no typmod. Refused only where both refuse. A non-decimal
  integer is converted to decimal by `radix_to_decimal` only where its width
  in bits does not settle the bound, the one width `10^131072` shares with
  values below it (`NUMERIC_INTEGER_BITS_MAX`).
- **`predicate::jsonb_unread`** (I41): a text `JsonCursor` neither reads nor
  refuses is refused where `decode::json_in` refuses it; trailing text after
  a value now reaches it rather than reading as unparsed. A document past
  `JSONB_MAX_DEPTH` stays unparsed.
- **`uuid` needed nothing**: every failure of `decode_uuid` was already
  `uuid_in`'s (I64).
- **Under `default` too**: these refusals are `field_key`'s, so a parse
  keying such a field fails at it, as 31.12's rule has every refusal fail.
- **`CACHE_FORMAT_VERSION` is 60**: a block's `checked_in_full` and an
  ignoring parse's record now cover these refusals. No persisted byte moved;
  both pinned digests re-pinned to the version alone.
- **Evidence**: `predicate.rs`'s
  `a_field_no_reader_reads_is_refused_only_where_every_major_refuses_it`,
  every case put to each type's input function in a `postgres:<major>-trixie`
  container at all six majors, `numeric_in` called with the column's typmod;
  `decode.rs`'s `a_float_past_its_range_is_refused_and_one_in_it_read` and
  `a_field_is_put_through_its_typmod_as_copy_puts_it`. Four tests that used
  `nope`, `x` or `000000x` as an integer no reader reads now use `0x1F`,
  `0x00006` or ` 7`, which some major reads.

## Negative results and limits

- **A cast of a literal is not `COPY`'s reading of a typmod'd field**: the
  cast calls `numeric_in` with no typmod and coerces after, so `1e-16384` as
  `numeric(10,2)` is refused by the cast and read by `COPY`. A probe of a
  typmod'd input calls the input function with the typmod (I82).
- **`numeric_in` broke I35's additivity outside the oracle's cases**: v13 to
  v15 read `1e 5` and `1e -5`, by `strtol`, and v16 refuses them, while v16
  reads `0e1073741823`, which v13 to v15 refuse. Both are refused nowhere;
  I35's scope limit, the roadmap's exceptions and the manual's "Where
  PostgreSQL majors differ" name it beside `oidin`.
- **No reader was widened**: a blank around a number, a hexadecimal float,
  `nan(…)`, an exponent in a `numeric` field, `0x1F` and `1_000` stay
  unparsed, so a query decoding one still fails with `FieldDecode`.
- **A filter literal is unchanged**: `order_key` keeps no `Unread`, so a
  literal no reader reads is `PredicateValueDecode` either way.

## What the slices after this inherit

- **31.22.1** owes the date, time, timestamp and `interval` readers a
  classifier each, for `ParseDateTime`'s and `DecodeDateTime`'s grammar
  (`DecodeInterval`'s for `interval`). Some spellings are decided by the
  restoring server — a field order under its `DateStyle`, a zone name or
  abbreviation in its tables — and are refused nowhere; `KD90` holds the
  detail. The probe harness is not kept: a container per major, each case
  hex-encoded into a temporary table and put to a PL/pgSQL function trapping
  the input function's error.
