# P43 notes — the block model: identity, address and record

What the phase leaves the next one, and what it tried and refused. The spec is
[`roadmap-P43-block-model.md`](roadmap-P43-block-model.md); the mechanisms'
decisions are [`decisions.md`](decisions.md), D167–D170, each crate's part
[`concepts.md`](concepts.md). The per-slice notes went at the wrap, and git
holds them at `6fca6c23`. The phase filed and struck no `KD<k>` and owned none.

## What the next phase inherits

- **P8 fills a run's record, not its shape.** `InsertRun` holds a
  `BlockRecord` inline, built by `Default` with `row_count` set and the rest
  empty; nothing shares it, so it is not behind an `Arc` as a `CopyBlock`'s is.
  A run's address stays P8's (the spec, "What the phase does not own").
- **P41's entry arm is one more `BlockAddress` variant.** `BlockAddress` is the
  four absolute offsets today, a struct, and `BlockAddress::id()` the one
  derivation of a map block's `BlockId`; `CopyStart::id()`, the scanner's, is a
  header's, since a `CopyStart` knows two of the four offsets. Every offset in a
  `BlockRecord` is from the block's data — `LongestLine` included, its display
  sites (`info --detail`, the replay's `held_line`) adding the data offset back
  — so a record needs nothing from P41.
- **A host with no line machine records with no builder.** `open` answers the
  block's `Recorder`; `finish` takes the row count, terminator and longest line
  from the host, as the scanner hands them over in `CopyEnd`, and answers the
  `BlockRecord` `on_copy_end` pushes. P41's reader calls `finish` directly.
- **P21's record for a replayed block has a policy to start from.**
  `Policy::Rereads(Reread::Censuses)` is a re-read that censuses and observes
  nothing, `settle.rs`'s; a query gathering statistics through the replay would
  add an arm, not a host.
- **The open has three policies**: `Maps(Pass)`, `Rereads(Reread)` and
  `Eager`, the last the fixtures' alone, never offered to the interior; the
  fixtures reach the open and the one-block back-fill (`backfill_one`) through
  `pgdt_mapping::internal`.

## Negative results

### Shapes refused

- **`BlockId` is not under `persisted/`**, though the spec's table places it
  there: never persisted, so it is no serde shape, and it sits in `span.rs`
  beside the derivation.
- **A segment and a matched block still clone the header.** Sharing it would
  put an `Arc` around `CopyHeader`, which nothing has placed; "no whole block
  cloned" holds for the record, which is what a census or statistics made
  costly.
- **`DataBlock::Copy` holds a `CopyBlock` unboxed.** The box kept a block from
  setting every span's size; a block is now smaller than `SpanBody`'s `Table`
  arm.
- **The recorder holds no row count** (D169), against the spec's list: the
  scanner counts rows a block recording nothing hands to no recorder.
- **The eager pass names `attach_text` and `derived_diagnostics` through
  `pgdt_dump`**, not `pgdt_mapping::internal` as the spec words it:
  `pgdt-fixtures` already depends on `pgdt-dump` behind `mapping`, and a
  re-export would be a second path to one item.
- **Two functions keep `clippy::too_many_arguments` in the phase's reach.**
  `ColumnGatherer::new`'s eight are a resolved column's parts, and the tests
  build one from a bare `ComparisonPlan`, so taking the resolved column moves
  fixtures, not the gatherer. The fixtures' `backfill_block` keeps the
  signature it had in `pgdt-mapping`; it opens no pass.
- **The strict checker keeps a `Charge` that charges nothing**: it counts the
  checker among the observers the account may be short of, without which a
  strict parse's table of checks (D81) falls outside the instrument's
  tolerance (`StatisticsAccount`, "What it does not see").
- **Fields that were module-private in `pgdt-model` are crate-private** —
  `ColumnStatistics`' `null_counts`, `bounds`, `datafusion_bounds` and
  `unrepresentable`, `Unrepresentable`'s `format` and `engine` — so their
  methods read them from beside `persisted/`. Inside the crate a raw read is
  refused by the type's doc, no longer by the compiler.
- **A persisted shape's doc keeps its links to its reader**, against the edges
  (`module_graph.py --docs`): the shapes are read beside the tree by design.

### Behaviour that moved, and nothing depends on

- **A streamed row cut short by the file's end is no longer dropped from a
  piece's census.** It ends a partition with no terminator, which is either
  past the terminator, never absorbed, or in an unterminated block, whose
  region fails; the census is never read.
- **The exits that bank after an unwinding read save, then check**, where they
  checked, then saved. An enabled save checks the watch before writing
  (`CacheMode::save`), so the same checks run and refuse at the same one.
- **A re-read's observer is built after `reread_rows`'s extent check**, so a
  block refused as `CachedBlockChanged` builds none.
- **The back-fill's credit to the account runs outside the statistics scope.**
  `StatisticsAccount::apply` allocates nothing, so the instrument's
  attribution does not move.
- **A stopped piece's head is left as it was**: a decline or refusal frees the
  piece's `Groups` and the block drops the piece rather than joining it.

### What the count pairs found, accepted under D108

Every rise below rests on codegen or layout, which D108 refuses to rework; the
readings are `runs/count-pair-43.<M>-20261121/callgrind/reading.txt`.

- **43.4**: every query row a few instructions a row in `replay`'s closure —
  stack reloads, register allocation moved with the per-segment locals; the
  typed query's further rise is layout, as is the data-level parse's, whose
  changed code (the save serializing a record out of line) falls.
- **43.5**: the data-level parse a few instructions a field and a row, and the
  strict parse over `long_escaped_32` a few thousand: `Reader::fold` carries
  `&mut Groups`, spilled on the fast path, `Reader::begin_row` is out of line,
  `FieldCheck::refused` no longer inlined. Moving the groups inside the reader
  to drop the argument rests on codegen too.
- **43.6**: a held row's census folding every field where an observer reads
  the row rose the data-level parse, in `CensusFold::fold`'s `memchr2` on every
  field (`runs/count-pair-43.6-20261121/`); with the whole-row gate kept the
  parse falls below its parent (`runs/count-pair-43.6-rework-20261121/`, D169).
  The metadata-level parse rises by `Recorder::row`'s prologue ahead of its
  early return.
- **43.7**: the data-level parse over `long_bytea_32` rises in `memcpy` under
  `ColumnGatherer::close_group`, unchanged code called as often; the open's own
  code nets a fall.
- **43.3, 43.7.1 and 43.8 took no pair**: none changed the row path beyond one
  `block.id()` per segment or one `matches!` per block.
