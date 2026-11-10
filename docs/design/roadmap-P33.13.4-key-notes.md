# P33.13.4 notes — a strict check holds no text of a scalar a push machine keys

**An integer's, an `oid`'s, a `boolean`'s, an enum's and a `uuid`'s order key
is read by a push machine** fed its text a byte at a time: `field_key` feeds
it a `&str` whole and a strict check a part's runs, so a strict check reads
such a scalar position, and a scalar column's escaped field, holding none of
it ([`decisions.md`](decisions.md), "D147"). The row was split from what its
checklist line asked of every reader an order key is read by; the rest are
33.13.5's ([`../status/history/2026-11-08.md`](../status/history/2026-11-08.md)).
No reading was taken. What proves it is
`crates/pgdt-query/tests/decoded_copies.rs`'s
`a_strict_check_holds_no_text_of_a_scalar_it_keys` — an integer element and
field of eight MiB of leading zeros, an `oid` and a `boolean` element of eight
MiB of leading blanks, a `uuid` of eight MiB, fed in runs of 64 KiB, refused
ones among them, the peak under four runs — which fails on its parent, whose
`Check::Scalar` collected the part.

- **`IntIn`** (`value/int.rs`): `pg_strtoint64_safe`'s grammar from v16, its
  magnitude saturated, and whether the text is `*_out`'s plain decimal;
  `int_in` is it fed a `&str`.
- **`OidIn`** and **`OidBase`** (`value/int.rs`): the plain decimal this
  build reads beside each major's `strtoul`, three machines in step.
- **`BoolIn`** (`value/bool.rs`): a word of at most five letters between
  blanks. `decode_bool` stays the batch's reader of `boolout`'s `t` and `f`.
- **`UuidIn`** (`value/uuid.rs`): `decode_uuid` is it fed a `&str`, so a
  query's `uuid` batch reads through it too.
- **`KeyRead`**, **`KeyMark`** and **`EnumIn`** (`key/order.rs`): the
  dispatch, a mark a machine's copy, and an enum's label held to
  `NAMEDATALEN`. Every other kind is `KeyRead::Collected`, its text handed
  to `field_key`'s slice reader at its end.
- **`walks`** (`grammar/check.rs`) names a scalar column a machine keys, so a
  strict parse's `check_row` reads such a field through `FieldWalk`.
- **What proves each machine equal** to the slice reader it replaced:
  `key/order/differential.rs` holds it to `key/order/prior_shape.rs` over every
  text of up to five bytes drawn from the bytes its grammar turns on, and
  built and damaged texts past them; a mutation of the decimal underscore
  rule and one of `t`'s blanks each fails it.

## What 33.13.5 inherits

- **`KeyRead::Collected` is the seam**: a kind moves by gaining a machine in
  `KeyRead::machine`, `walks` then naming its columns with no change of its
  own, and `field_key`'s slice arm for it going.
- **`Strtod`** (`value/libc.rs`) is the `float8in` reader's; a datetime's
  text is bounded by its work buffer (`within_workbuf`, `longest_read`) but
  for its blanks, which `ParseDateTime` skips one at a time.
- **A range's bounds are still collected** (`RangeWalk`, `checked_range`),
  each keyed once it ends; an integer bound is keyed by `IntIn` from there.

## What 33.4 inherits

- **A data-level parse of an escaped integer column still unescapes a long
  field whole**, the column summing (`ColumnGatherer::observe`, D142): a
  collected scalar's text, drawn from the ledger.

## Negative results

- **Its count pair is 33.22's**: `field_key` over these five kinds feeds a
  machine a byte at a time where `str::parse` and the slice readers read a
  slice — on every data-level parse's bounds and every query's filter — as
  `decode_uuid` does on a query's `uuid` batch, and the pair resolved and
  placed a rise there ([`roadmap-P33.22-priced-notes.md`](roadmap-P33.22-priced-notes.md)).
  33.17 found the push machine dearer than the pull walk on a short scalar
  fed one run ([`roadmap-P33.17-form-notes.md`](roadmap-P33.17-form-notes.md)).
- **The allocation test reads no enum**: `comparison_for` with no catalog
  plans none. Its state is its label, at most 64 bytes, and
  `differential.rs` holds it to its slice reader.
- **A `uuid`'s hyphen after its last group needs no rule**: past its 32nd
  digit only a closing brace is read.
