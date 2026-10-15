# P36.3.2 notes — one census field, one stored database

`CopyBlock`'s census is one `census: Option<BlockCensus>`, and a block's
database is stored only on its span (D34). Both change what a cache persists:
`CACHE_FORMAT_VERSION` is 69, and `persisted_index_is_pinned_to_the_format_version`
is re-pinned beside it. `golden_order_is_pinned_to_the_format_version` takes
the new version and keeps its digest unchanged, since no comparison moved.
Nothing else the cache holds moved.

## What landed beside the spec's words

- **`InsertRun` and `LargeObjectRegion` lose their `database` too.** They
  were the same copy of their span's field, and nothing read them.
  `LargeObjectRegion` is now an empty struct, kept so `DataBlock`'s variants
  keep one shape each.
- **A block's database is read by pairing it with its span.**
  `DumpIndex::blocks_with_database()` yields `(Option<&str>, &CopyBlock)`.
  `blocks()` and `blocks_for(&str)` still yield the block alone, because
  most callers, nearly all of them tests, read nothing of its database.
  `tables()` and `blocks_of(&TableName)` go through the pair.
  `TableName::of` and `names` take the database beside the block.
- **A function that resolves a block against the DDL takes the database as
  its own argument, after `metadata`.** That covers `bounded_columns`,
  `strict_unchecked`, `gather_block_statistics`, `reread_rows`,
  `prune_block` and `DynamicPruning::new`.
- **A replay plans one table's blocks in one database**, and that database
  is the plan's (`ReplayPlan::database`). The mapping pass's `MappedTable`
  carries it, settled where the ambiguity check settles the target.
  `TablePartitions::plan` and `table_schema` take it from the `TableName`.
  `PlannedBlock` still records the database it was resolved for, so
  `activate`'s lookup is unchanged.
- **`CopyBlock::array_shapes()` and `unrepresentable()`** read the census's
  halves. `set_census` is gone: a pass records a census by assigning the
  field.
- **`info --json` follows the model.** A block's `array_shapes` and
  `unrepresentable` are now under `census`, as `shapes` and
  `unrepresentable`, and a block has no `database` (its span still does).
  The manual says so.

## What the next slice inherits

- **`(metadata, database)` travels as a pair** through every function that
  resolves a block's columns, the pairing cluster D names. 36.7's back-fill
  call and 36.8's `Dump` hold the map. They can take the database off the
  span themselves instead of asking the caller for it.
- **`blocks_for(&str)` goes with the selector in 36.8.** The two production
  callers that needed the database (`target_settled` and `map_for_query`)
  filter `blocks_with_database()` by `CopyHeader::matches` instead.
