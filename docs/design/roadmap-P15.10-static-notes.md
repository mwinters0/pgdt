# P15.10 notes — checkpoints inside a static block

A forward pass now places a checkpoint inside a static block, so a member
written by `deflate_quick` (level 1 under zlib-rs or a default-built zlib-ng)
is checkpointed at the spacing like any other. `KD129` is struck. The engine
reads each block's three header bits itself. Where the pass sets a mark, it
walks a static block's codes with `native::Walk`, which writes nothing, and
caps the inflater's output at the first symbol at or past the mark. That
symbol is reported as `Event::Symbol`. Its checkpoint is `Start::Symbol {
window, last }`. A decode resumes there by priming a static header (`last`
as its final-block bit) ahead of the byte's remaining bits (D118, RT23).
Every byte handed out is still the engine's (D117).

- **Placement.** The rule is now "the first place a decode resumes at or
  after each multiple": a block boundary, a member start, or a static
  block's symbol other than its end-of-block code. Excluding that code
  means a boundary at the same plaintext comes first, and no unit is empty
  (`placement.rs`).
- **The parallel pass.** The frontier's engine walks as the serial one does.
  A chunk decode walks nothing, but records `Static { bit }` at each static
  block's header. `Parallel::cut_at_static` cuts a taken chunk at the first
  static block whose span holds the next multiple, its stop that block's
  start. The frontier then decodes the block itself. A cut at the chunk's
  start takes nothing. Cutting keeps `compressed_position` monotone, so the
  fetched feed's one download holds what the frontier reads next. An
  earlier version left a chunk mid-serve, which moved the position back
  behind the chunk's stop and underflowed `Parallel::read`.
- **The walk holds a few KiB.** It is handed at most `WALK_PUSH` bytes at a
  time and drops what it has walked past. The inflater takes no byte the
  walk does not hold, so a forward pass stays within `DECODER_BYTES` and its
  window copy, which is `pgdt-io`'s `DECODE_BYTES` term
  (`a_forward_pass_walking_a_static_block_holds_no_more_than_its_charge`).
  The first version kept everything handed to it until a step ended, which
  was the whole input on a level-1 member.
- **A unit ending before a symbol** finishes at its plaintext end against
  its CRC-32 (`UnitTask::ends_at_symbol`). Its compressed end is not held,
  because the inflater has read on into the next symbol (Decisions worth
  another look).
- **The cache** persists `symbol: Option<bool>` beside each checkpoint's
  window. `CACHE_FORMAT_VERSION` is 79. A symbol with no window is refused
  as undecodable.
- **D19's gzip warning** now fires only for a writer that coded more than
  twice the spacing in one dynamic block. No zlib level does that at the
  shipped spacing. Its text no longer offers "a level above 1" as a remedy.
  The tests were written for each case. `pgdt-dump`'s map test uses one
  repeated row at level 6, a dynamic block of a few MiB against its
  256 KiB spacing. `crates/pgdt/tests/gzip_source.rs` hand-writes one
  dynamic block of 52 MiB (`one_dynamic_block`), checked by `flate2`'s
  inflater.
- **Evidence**: `a_static_block_is_checkpointed_at_the_spacing` covers a
  level-1 member at the shipped test spacing and at 100 bytes, where the
  final static block is checkpointed too (`last: true`). Every unit decodes
  alone, and a pass resumed after each checkpoint builds the uninterrupted
  index. `a_static_block_between_dynamic_ones_is_checkpointed_inside_it`
  covers a member written at levels 6, 1 and 6 (`test_support::gzip_levels`).
  `the_index_is_the_serial_pass_s_at_every_worker_count` runs over that
  member (`"levels"`). Its taken chunks are cut at the static block, at the
  chunk's start and inside it. Without the cut, those chunks would hand out
  the block unwalked.

## What later slices inherit

- **15.7.1**: a level-1 file is seekable once indexed. Its first pass is
  still serial in practice: speculation looks for dynamic block headers
  only, and a one-block file has none past its start. So 15.7.1's
  parallel-first-pass figure on such a writer's file prices the frontier
  alone. No figure prices the walk. A profile showing it is D118's
  reopening condition.
- **The manual** (15.7.2) can say that any zlib level is seekable once
  indexed. The warning now describes an encoder outside the zlib family.

## Negative results

- **`Z_TREES` cannot report a block's type through zlib-rs 0.6.8.** After
  its header stop on a stored block, it leaves its mode at `Stored`. The
  next call reads the block's data as its lengths ("invalid stored block
  lengths" at level 0). Reading the three bits from the bytes a stop's
  unused bits came from needs no stop and holds on every engine.
- **A payload ending in one long run has no symbol to checkpoint in its
  final static block.** Its few hundred bytes were one or two matches. The
  test payload ends in literals for that reason.
- **A `deflateParams` level change cannot write a mixed member through
  `flate2` over zlib-rs.** `set_level` there flushes with no output buffer
  and fails with a stream error. `gzip_levels` concatenates sync-flushed
  raw streams instead.
