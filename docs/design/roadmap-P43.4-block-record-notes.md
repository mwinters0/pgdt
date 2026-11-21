# P43.4 — `CopyBlock` as header, address and record: notes

Slice 43.4 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). The
phase's cache-format bump, to 89: a record's fields follow its address, its
longest line is block-relative, and an `INSERT` run holds a record. The shape
is D168.

## What the next slices inherit

- **`BlockAddress` and `BlockRecord` are in `persisted/span.rs`**, with
  `CopyBlock { header, partition_root, address, record: Arc<BlockRecord> }`.
  `BlockAddress::id()` is the one derivation of a map block's `BlockId`, and
  `CopyBlock::id` is gone. `array_shapes()` and `unrepresentable()` are
  `BlockRecord`'s. `BlockRecord` derives `Default`, which is how an `INSERT`
  run's record is built: `row_count` set, the rest empty. A run holds its record
  inline, since nothing shares it.
- **The scanner's line is a `ScannedLine`** (`pgdt-dump`'s `scan.rs`), at its
  file offset, with `fold`. `CopyEnd` carries it, and the interior's pieces
  fold it. It becomes the record's `LongestLine` through
  `ScannedLine::in_block(data_offset)`, at two sites: `Builder::on_copy_end`,
  and the back-fill's `CachedBlockChanged` check. 43.6's `finish` takes the
  scanner's line through the same call. A piece resumed with no block address
  (`scan_piece`) has none to subtract, which is why the conversion waits for
  the record.
- **Every display site adds the data offset back**: `info --detail`'s
  `longest line:` and the replay's held-line refusal (`held_line`). `info
  --json` prints the cache's shape, so a block is `header`, `partition_root`,
  `address` and `record` there, and `longest_line.offset` is block-relative.
  `docs/manual/dump-inspection.md`, `scripts/row_density.py` and
  `measure.py`'s input profile read the new shape.
- **A record is changed only through `Arc::make_mut`**: the back-fill's
  `Rereads::record`, `settle.rs`'s census, and `cache/store.rs`'s
  attributed drop of a freshly loaded map's statistics, which is unique there
  and so copies nothing. 43.7's merge through `BlockRecord` replaces the
  back-fill's field-by-field writes. The copy happens once, at the top of
  `record`.
- **`Segment` holds `header` and `address`.** `DynamicRead` holds every
  matched block's record by id (`TablePartitions::records`). `enter` takes the
  segment's header and address, and `DynamicBlock::new` takes the header,
  address and record. Whether the query uses statistics is now asked where a
  record's statistics are read: in `DynamicBlock::new`, and at the top of
  `cut_under`'s loop. Before, it stripped them from a whole-block copy in
  `TablePartitions::new`. `matches` is a plain `to_vec()`, each record shared
  with the map. `believed_columns` and `DynamicPruning::new` take a header and
  an address.
- **`DataBlock::Copy` holds a `CopyBlock`, unboxed.** The box was there because
  a block set the size of every span. A block is now smaller than `SpanBody`'s
  `Table` variant, so the box bought nothing.

## Negative results

- **A segment and a matched block still clone the header.** "No whole block
  cloned" holds for the record, which is what a census or statistics made
  costly. Sharing the header too would put an `Arc` around `CopyHeader`, which
  the spec does not place.
- **`a_replay_holds_no_block_s_statistics` now holds the record.** A segment
  cannot hold a record by type, so the test counts the record's `Arc` after the
  matched blocks drop, and the statistics' `Arc` through it.
- **Every query row pays a few instructions a row in the replay, accepted.**
  The `--count-pair` resolves a rise on all four query rows, though no row's
  source changed. Callgrind places it in `replay`'s closure, the same in each:
  stack reloads on the row path, register allocation having moved with the
  closure's per-segment locals, a header borrowed and an address copied where
  the parent borrowed the block. Reworking it would rest on codegen, which D108
  refuses. Allocator placement is the rest, and the typed query's rise, which
  callgrind does not reproduce, is layout. The data-level parse's rise is layout
  too: the code it changed, the cache save serializing a record out of line,
  falls (`runs/count-pair-43.4-20261121/callgrind/reading.txt`).
