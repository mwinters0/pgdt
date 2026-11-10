# P33.13 notes — a parse holds no copy of a text it reads

**A gathering parse reads a long escaped field off its bytes, and a strict
parse holds a text to its typmod the same way**, so neither holds an
unescaped copy of a `text`, `character` or `bytea` value beside its line
([`decisions.md`](decisions.md), "D142"). No reading was taken; what proves
it is `crates/pgdt-query/tests/decoded_copies.rs`'s allocation count:
`a_parse_holds_no_copy_of_an_escaped_text` (a default and a strict parse
over `text` and `varchar(n)`, and a field past its typmod still refused) and
`a_strict_bytea_check_holds_no_copy`, tightened from one copy to none. Both
fail on the parent; the first fails on its strict leg with the gatherer
alone changed.

- **`TextFold`** (`pgdt-codec`'s `value/text.rs`, `internal`): a text read a
  run at a time — UTF-8 or not, its length, where its trailing blanks begin
  and its characters before them. `char_typmod_refuses` is its fold of one
  slice, so the typmod rule is stated once (`TextRead::typmod_refuses`).
- **`text_typmod`** (`grammar/check.rs`): where `field_refused` refuses a
  field only past a text's typmod; `field_refused` answers through it, and
  a strict parse's `check_row` reads such a field off `Unescaped`,
  unescaping it only to name it once refused.
- **`bytea_unread` reads an iterator, and `bytea_escape_head`** is
  `decode_bytea_escape` over one, so a long `bytea`'s placement and refusal
  are read off its bytes.
- **`LongText` and `Field`** (`pgdt-mapping`'s `gather/clip.rs`): an
  escaped field past `LONG_FIELD_HEAD`, read once into a head, a
  `TextRead` and its `Unescaped`; one whose head is all of it is gathered
  whole. Bounds read a `Head`, a head and a length, which `Clipped` and the
  row order already read no more than.
- **What proves the reading equal** to an unescape: `bounds/tests.rs`'s
  `a_long_field_read_off_its_bytes_is_bounded_as_its_text` (each bytewise
  kind with and without a typmod, `bytea` in every form, a keyed kind) and
  `gather/tests.rs`'s `long_escaped_fields_gather_what_they_gather_written_plain`
  (dictionaries, refusals under `ignore`, row order), each failing under a
  mutation of the typmod count or the trimmed head.

## What 33.13.1 inherits

- **A column summing, or counting unrepresentable values, still unescapes a
  long escaped field whole** (`ColumnGatherer::observe`): a sum needs the
  whole integer's digits and a container's tier its elements. No `pg_dump`
  writes an escape in such a column, so no dump in hand pays it; the copy is
  drawn from the ledger, and P40 owns its removal.
- **A strict parse's check of a nested, `json` or `jsonb` value, and of any
  other scalar, still unescapes it and builds what `checked_key` and
  `field_key` build**; the row's walk is what removes them, 33.13.1's for a
  nested, `json` or `jsonb` value
  ([`roadmap-P33.13.1-walk-notes.md`](roadmap-P33.13.1-walk-notes.md)).

## What 33.4 inherits

- **A parse holds no copy of a text, `character` or `bytea` value**,
  gathering or strict, and no line of one past its chunk
  ([`roadmap-P33.19-fold-notes.md`](roadmap-P33.19-fold-notes.md)); what it
  holds of a nested, `json` or `jsonb` value is in 33.13.1's notes.

## Negative results

- **No count pair was taken**: an escaped field past `LONG_FIELD_HEAD`, a
  few hundred bytes, is now folded rather than unescaped on every data-level
  parse, and P33's spec adopts no pricing of a hot-path change
  ([`decisions.md`](decisions.md), "D108").
