# P33.3.3 notes — a query's census streams a line

**A block a pass only censuses streams a row that outgrows its chunk**, its
fields folded into the census as their bytes go by: a cold query's mapping
pass holds no line of a table it does not read, serially and in its interior
workers, but a counted container's field, held whole in a mapping of its own
past its chunk ([`decisions.md`](decisions.md), "D138", "D139"). What the
census records is what one holding every row records. No figure was taken: no
leg of `long-line-resident` is a cold query over a table beside the long one.

- **The carry hands a streamed row's fields over** (`pgdt-dump`'s
  `scan/chunked.rs`, `FieldFold`): each run of a field's bytes, its index
  beside it, a field its fold `holds` grown in the carry's own `CarriedLine`
  and handed over once. A trailing CR no backslash escapes is held back a run
  and stripped where the row ends after it, as a `Row`'s `raw` strips it
  (`EndingScan::escapes`). `ChunkedScanner::next_chunk_folding` takes the
  fold; `next_chunk` is it with none.
- **The census's fold** (`pgdt-model`'s `block.rs`, `CensusFold`): the shape
  census by `ShapeFold`, which `ArrayShape::observe` is fed once; the count
  from what `FieldCount::keeps` says of the column — `Kept::Nothing`,
  `Head(n)` or `Whole` — a row wider than its header growing the shapes at
  its end as `observe_row` grows them.
- **The head is bounded by construction** (`pgdt-codec`'s
  `unrepresentable.rs`): `Counter::tier_of` holds a scalar's text to
  `Leaf::longest_counted` — a spelling's length, or a datetime reader's work
  buffer and a space per field (`longest_read`) — and the head is four
  escaped bytes to each (`ESCAPED_BYTES_PER_BYTE`). The bound changes no
  count: `a_scalar_is_counted_from_a_head_of_its_field` pads each reader's
  literal with zeros until it refuses it and finds it inside the bound.
- **Who folds**: the forward pass hands `Builder::row_fold`, made by
  `count_block` for a censused block and unioned into its census at
  `on_copy_end`, a censused block with no observer and no recorded rows no
  longer `reads_rows`; a worker folds into `CensusPlan::fold`, carried
  through `spawn_blocking` beside its carry, a streamed piece taking what it
  folded and one the file's end left unconsumed dropping it.
- **What proves it**: `crates/pgdt-query/tests/streamed_census.rs` — a cold
  query beside a sixteen-MiB row holds under a quarter of it where the long
  field is `text` or a `date` read from its head, and holds it where it is a
  `date[]`, at one worker and at four, each census the whole-file pass's; and
  every block of every major's `types` fixture censused by a cold query at
  reads of 16 and 61 bytes, at one and four workers, as the whole-file pass
  censuses it. The resident half fails with the forward pass's `reads_rows`
  or the worker's `streams` reverted. Beside it, `block.rs`'s
  `a_row_folded_in_runs_is_censused_as_one_held_whole`, the carry's
  `a_streamed_row_reads_as_the_row_it_stands_for` (a fold recording each
  field, one held whole), and the interior's
  `a_censusing_worker_streams_a_row_and_censuses_it_as_held`.

## What 33.4 inherits

- **A held container field is a held line to the ledger**: it grows in the
  carry's `field`, past its chunk in a `LineMapping`, and a draw belongs
  there as it does for `line`. A head (`Kept::Head`, about a kilobyte at
  most) never passes a chunk term.
- **The passes that still hold a line** are those reading a field whole: a
  replay reading a field, a parse's field its statistics or check reads whole
  ([`roadmap-P33.19-fold-notes.md`](roadmap-P33.19-fold-notes.md)), and
  TimescaleDB's recorded catalog blocks.

## Negative results

- **No count pair was taken**: `ArrayShape::observe` is now `ShapeFold`'s
  state machine on every data-level row holding a `{` or `[`, and P33's spec
  adopts no pricing of a hot-path change ([`decisions.md`](decisions.md),
  "D108").
- **The bound's slack is the readers' grammar**: `ParseDateTime` skips any
  run of spaces, so the server reads a padded literal past any bound; no
  reader here reads one, so none is counted, as before this slice.
