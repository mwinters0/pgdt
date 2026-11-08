# P33.13.3 notes — a strict check holds no text a grammar reads

**A strict check reads a field of a type ordered by nothing off its bytes**,
through its `TextGrammar`'s push machine, holding a fixed state where it held
the part's text ([`decisions.md`](decisions.md), "D144", "D146"). The row was
split from what the spec's "What a held line costs" asks of every scalar
reader; the readers an order key is read by are 33.13.4's
([`../status/history/2026-11-08.md`](../status/history/2026-11-08.md)). No
reading was taken. What proves it is
`crates/pgdt-query/tests/decoded_copies.rs`'s
`a_strict_check_holds_no_text_a_grammar_reads` — a `polygon[]` element of
many points and a `bit varying[]` element of many bits, eight MiB each, fed in
runs of 64 KiB, the peak under four runs — and
`a_strict_parse_holds_no_text_of_a_column_a_grammar_reads`, a `path` and a
`bit varying` column written with an escape every few bytes, a refused one of
each among them; each fails on its parent.

- **`GeometricWalk`** (`value/geometric/walk.rs`): the seven geometric
  `*_in` as one tokenizer and a grammar per type, `path_decode`'s and
  `path_in`'s question of the last `(` read on as both answers.
  `value/geometric.rs` keeps the arithmetic a `line` is built by; the seven
  `&str` readers are its tests' (`geometric/tests.rs`), the walk fed whole.
- **`Strtod`** (`value/libc.rs`): `strtod` a byte at a time, refusing where
  it would leave a byte it read unread; `HexDigits` the hexadecimal rounding
  it and the slice `strtod` share.
- **`BitWalk`** (`value/bit.rs`): `bit_in` and `varbit_in`, a count and a
  flag.
- **`grammar/walk.rs`**: `Check::Grammar` holds the walk where it held the
  text, a `box[]` position is `Check::Array` over `;`, and
  `FieldWalk::grammar` reads a column a grammar reads, which
  `grammar_refuses` feeds a `&str` whole. A strict parse's `check_row`
  (`statistics/gather/strict.rs`) walks such a column as it walks a nested
  one.
- **What proves the walk equal**: `geometric/differential.rs` holds it to
  the slice scanners it replaced (`geometric/prior_shape.rs`) over a free-form
  corpus, built and damaged literals, and numbers whose rounding at either
  edge of `double precision`'s range is decided hundreds of digits in, which
  fails with the kept digits cut short; `check/tests.rs`'s
  `a_field_read_by_a_grammar_is_refused_as_its_text_is` feeds a grammar's
  field over runs and cuts, and a `box[]`'s whitespace runs.

## What 33.13.4 inherits

- **What the 33.13.1, 33.17, 33.18 and 33.21 notes address to 33.13.3 about
  a scalar part is 33.13.4's**: `Check::Scalar` still collects a part
  `field_key` reads, and a range's bounds are still collected.
- **`Strtod` reads a number with no read-ahead**, so a `float8in` reader
  can take it, refusing what follows a number but blanks; the slice `strtod`
  stays for `datetime/grammar.rs`, and `strtod_token` for `float_unread`.
- **`Check::Grammar`'s mark clones its walk**, a box and a number's kept
  digits, once per whitespace run in an unquoted element.

## Negative results

- **No count pair was taken**: the readers are a strict parse's alone; the
  one shared change is `hex_float`'s rounding moved into `HexDigits`
  unchanged, which the slice `strtod` reaches only past `0x`.
- **A run's end is invisible to these walks**, fed a byte at a time, so the
  differential test feeds whole; only `FieldWalk` is tested over cuts.
