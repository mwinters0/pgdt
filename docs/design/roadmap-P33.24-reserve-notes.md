# P33.24 notes — a nested value's copy never doubles

**A nested value past its chunk term is a batch of its own, decoded into
builders reserved to it** ([`decisions.md`](decisions.md), "D154"). What
proves it is `crates/pgdt-query/tests/decoded_copies.rs`'s
`a_long_nested_value_is_decoded_into_reserved_builders` and `tests/batch.rs`'s
`a_long_nested_value_is_a_batch_of_its_own`, each failing on the parent, and
`batch/builder/factor/tests.rs`'s `a_reserved_column_holds_its_reservation`.

- **`reserved_column_builder`** (`pgdt-query`'s `batch/builder.rs`): the
  column's builder tree made with Arrow's `with_capacity` throughout, each
  list's child given one entry for every `part_bytes` of the field's text —
  the fewest bytes a part is written in (`shortest`, which the factor reads
  too), its delimiter included —
  and a `bytea` part's values the text's length (`ByteaBuilder::with_capacity`).
  An enum's labels are its type's, so only its keys are reserved. Arrow 59's
  builders have no `reserve`, so a builder is reserved by being made, which an
  empty batch allows.
- **`RowBatcher::cuts_before`**: a batch holding a row, before a row with a
  walked value longer than its read's chunk (`long_field`, the plan's
  `chunk_size_bytes`), is flushed by the replay ahead of the row; its token
  resumes at that row, its block's row count one short of the scanner's.
  `push_field` replaces such a value's builder with a reserved one where the
  batch holds no row, and `holds_long` flushes the batch after the row, so no
  later row grows a reserved buffer.
- **The factor is the reservation** (`factor.rs`'s `reserved`): every
  buffer's own density summed, where `form` takes the largest. Unchanged for a
  one-dimensional array of fixed-width elements, a range array and a
  multirange; up by a text byte for `text[]` (`177/16`), by a decoded byte for
  `bytea[]` (`97/16`) and a record array holding text, and by each dimension's
  lists for a multidimensional array (STATUS, "Decisions worth another look").

## Its count pair, reworked, placed and accepted

**`df2d20ca` against the tree resolves a rise on `query --schema-mode
strings` alone and a fall on the other three query rows**; neither parse
moves (`runs/count-pair-33.24-rework-20261111/log`). callgrind reproduces the
rise on both rep pairs and places it (`…/callgrind/reading.txt`, the legs in
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.24/`), D108's falls
unplaced.

- **What is left is `cuts_before` asked of every kept row**: an out-of-line
  call whose six saved registers outnumber its three tests, and the replay's
  row path setting it up. It is the test D154's cut needs before a row is
  pushed; shrinking it is a cold split or an inlining attribute, which D108
  refuses. `Option::is_some_and` called out of line in the replay, inlined
  at the parent, is layout. **Accepted.**
- **Reworked first**: the slice as first written read a column's walked
  plan off a vector of its own on every field, where the parent tests the
  builder's variant, already loaded; its pair resolved a rise on all four
  query rows (`runs/count-pair-33.24-20261111/`, placed in its
  `callgrind/reading.txt`). `push_field` and `cuts_before` now ask
  `RowBatcher::walks_column`, the parent's test, and a column's type and plan
  are read only to reserve a long value's builders.

## What 33.16.2's figure left unnamed is the doubling

The second values-sized buffer `long-line-resident` read over `int8[]` and
`text[]` at 32 and 128 MiB is the first long row's builder doubled past the
power of two its values end just under, by the short rows sharing its batch
([`measurements.md`](measurements.md), "What a held line holds resident";
`runs/33.24-attribution-20261111/results.txt`). The cut and the reservation
remove it; 33.7's re-take reads what remains.

## What 33.4 inherits

- **A reservation is charged whole**: a sparse value reserves what its
  densest form would hold — an escaped `text[]` of six-byte elements reserves
  a view for every two bytes — and the factor charges that. Its untouched
  pages are address space and commit, not resident.
- **The cut is the plan's chunk**, the term a held line draws beyond; the
  ledger's chunk term, once it carves one, is the same number.

## Negative results

- **A reserve on a non-empty builder** was not written: Arrow 59's builders
  expose none, and growing one holding the batch's earlier rows copies them
  beside the reservation.
- **A text part's bytes are not reserved**: `StringViewBuilder` writes a part
  longer than its block into a block of the part's length, and shorter ones
  into blocks of at most two MiB, so they never double.
