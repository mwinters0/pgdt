# P33.11 notes — each block's longest line

**Every block in the map records its longest data line**: its start and its
length, line ending included, the first of the longest
(`CopyBlock::longest_line`, a `LongestLine`; [`decisions.md`](decisions.md),
"D140"). `CACHE_FORMAT_VERSION` is 84. `pgdt info --detail` prints it beneath
each block holding a row, after the byte offsets; `--json` has it as each
block's `longest_line`.

- **The scanner measures it**: `CopyScanner` folds each row it hands out,
  held or streamed, and its `CopyEnd` states the block's
  (`CopyEnd::longest_line`), so the serial forward pass, `build_map`'s eager
  pass and a back-fill's re-read get it with nothing added to their loops. A
  piece of the interior reads its own scanner's (`CopyScannerExt::longest_line`)
  or its streamed row's length, and `merge` and the window loop fold the
  pieces in file order, so a split block states what a serial read states.
- **A back-fill holds it to the record**, beside the terminator, the end and
  the row count: a re-read finding another longest line refuses with
  `CachedBlockChanged`, as one finding another row count does.
- **What proves it**: `crates/pgdt-mapping/tests/longest_line.rs` maps five
  blocks — a long row, two tied, CRLF rows, none, and a long row among enough
  rows to be cut into pieces — at both levels at one worker and four, and
  through `build_map`, and holds each block's record to the lines read off
  the file's bytes. It fails with a piece's own line or a streamed piece's
  line dropped. The interior's unit tests now hold a split block's
  `CopyEnd`, longest line included, to the serial scanner's at every cut.
  `partial_reporting.rs`'s `info_states_each_blocks_longest_line` holds
  `--detail`'s lines to `--json`'s, block for block.

## What 33.4 and 33.5 inherit

- **The charge is read off `LongestLine::len`**, a number, so
  `Arrangement::resolve` is handed it without pgdt-io seeing the model (the
  spec's "Where it lives"). A read over several blocks plans with the largest
  of theirs.
- **A block mapped before this build has no record**: its cache is refused
  by the version, so every map a read plans with carries one.

## Negative results

- **`Event::CopyEnd` is boxed now**, as `CopyStart` is: the line pushed it
  past the size `scan.rs` asserts an event holds to, a `Row` and its tag. It
  costs one allocation a block. **`DataBlock::Copy` holds a `Box<CopyBlock>`**
  for the same reason: the field took `CopyBlock` past clippy's
  `large_enum_variant` margin over an `InsertRun`, and a `DataBlock` sits in
  every `SpanBody`, so the block set the size of every span of a map. Boxed,
  no cache byte moves (the persisted digest is unchanged by it).
- **No count pair was taken**: the row path gains one comparison a row, and
  P33's spec adopts no pricing of a hot-path change
  ([`decisions.md`](decisions.md), "D108").
