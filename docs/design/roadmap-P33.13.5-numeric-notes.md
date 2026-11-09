# P33.13.5 notes — a strict check holds no text of a `numeric`

**`numeric_in`'s reader is a push machine** fed its text a byte at a time:
`field_key` feeds it a `&str` whole and a strict check a part's runs. It holds
the significant digits its column's answer reads and counts every other, so a
strict check reads a `numeric` position, and a `numeric` column's escaped
field, holding none of a padded value
([`decisions.md`](decisions.md), "D148"). The row was split from what its
checklist line asked of every remaining reader an order key is read by; the
rest are 33.13.6's
([`../status/history/2026-11-08.md`](../status/history/2026-11-08.md)). No
reading was taken. What proves it is
`crates/pgdt-query/tests/decoded_copies.rs`'s
`a_strict_check_holds_no_text_of_a_numeric`: elements and fields of eight MiB
of leading zeros, bare and under a typmod, one of a fraction's zeros, and one
of escaped trailing tabs, fed in runs of 64 KiB, refused ones among them, with
the peak under four runs. It fails on its parent, whose `KeyRead::Collected`
collected the part.

- **`NumericIn`** (`value/numeric/input.rs`): `Plain`, this build's
  `[-]digits[.digits]`, holding an integer's digits from its first nonzero and
  a fraction's to its last nonzero, each capped by `caps`; `Spelling`, v14's
  and v16's `numeric_in`, holding counts and, for a typmod'd column, the
  digits from the first significant one through the one rounded; the text's
  first nine bytes for `numeric_out`'s special spellings.
- **`NumericIn::whole`** feeds the majors only where `Plain` refuses. It is
  `typmod_unscaled_digits`, which the batch's `Decimal128` and `Decimal256`
  builders and `statistics/stored.rs` read through, and `NumericKey::parse`, a
  literal's reader. `numeric_unread`, `NumericSpelling` and the two slice
  spellings are gone.
- **`KeyRead::Numeric`** (`key/order.rs`) carries the kind for
  `numeric_key`, which answers the special values first, as `field_key`
  did. `KeyRead::whole` is what `field_key` feeds.
- **What proves the machine equal** to the slice readers it replaced:
  `key/order/differential.rs`'s `a_numeric_is_read_as_its_slice_readers_read_it`
  holds `field_key` over twelve `numeric` kinds, `typmod_unscaled_digits` at
  five typmods and `NumericKey::parse` to `value/numeric/prior_shape.rs` over
  every text of up to five bytes on the bytes the grammars turn on, built and
  damaged texts past them, and texts at I63's bounds, undamaged for their
  length. Two mutations each fail it: v14's exponent bound read as v16's,
  which only a blank after the `e` exposes, and the fraction's cap one short.

## What 33.13.6 inherits

- **What 33.13.4's notes address to 33.13.5 is 33.13.6's**, `numeric`'s
  item aside: `Strtod` for `float8in`, a datetime's text bounded but for its
  blanks, and a range's bounds still collected.
- **`KeyRead::whole`** is where a machine running a second reading only on
  failure, as `NumericIn` does, takes a `&str` fed whole.

## Negative results

- **Its count pair is 33.22's, and resolved no row**: `field_key` and the
  batch's decimal read now feed a machine a byte at a time where a slice
  reader read a slice — on every data-level parse's bounds, every query's
  filter and every typed decimal column
  ([`roadmap-P33.22-priced-notes.md`](roadmap-P33.22-priced-notes.md)).
- **A typmod this build does not read holds its key's digits in a strict
  check**, which answers only whether it is refused: the column refuses
  nothing past the grammar, and its key is those digits.
- **A bare column's radix spelling holds up to I63's width in bits of
  digits**, a binary one of `0b` and 435,412 digits holding them all, where
  only the decimal length decides. No `pg_dump` writes one.
