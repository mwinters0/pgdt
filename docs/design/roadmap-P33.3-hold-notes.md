# P33.3 notes — hold only what is decoded

**The line a carry pass scans is frozen for its row and dropped after it, and
a replay keeps it as one of its batch's blocks and holds no read chunk under
it.** The figure's re-take is what remains: `long-line-resident` is taken from
the commit carrying this change, never from a tree carrying it uncommitted
(`measurements.md`, "A figure may be published outside the sweep"), by a
detached `cd scripts && uv run measure.py --figure long-line-resident`, which
took about twenty minutes at 33.1.

- **`ChunkCarry` freezes the completed line** (`pgdt-dump`'s
  `scan/chunked.rs`): a line no chunk has ended grows in a `Vec` where it
  lies; once a chunk ends it, or the file does, its allocation becomes the
  `Bytes` the carry pass scans (`Passes::held`, `ChunkEvents::held`), and the
  next chunk's absorb drops it. Neither buffer is kept at a line's high-water
  mark; `the_carry_keeps_no_line_past_its_row` holds what the carry keeps
  allocated after a line 1,024 chunks long to four chunks.
- **The replay retains the carried line ahead of the chunk that completed it**
  and releases everything it has read once a chunk's events run out
  (`stream/replay.rs`, `RetainedChunks::release_through`): a row of the carry
  pass lies whole in that line, so an unescaped value spanning chunks is a
  view into it, and what a chunk leaves unconsumed is copied into the carry.
  `a_long_value_is_a_view_into_its_carried_line` (`pgdt-query`'s
  `tests/batch.rs`) fails on the parent at both: the value is a copy, and
  65 reads are alive at once against at most 3.
- **Every serial loop and the interior worker free the line alike**, all
  reading through `ChunkCarry`; only the replay keeps it past its chunk.

## What the re-take is read against

An account from the code, written before the readings (`.claude/skills/evidence/SKILL.md`, rule 1):

- **A one-worker parse** held the carry's two buffers at a line's pages and
  the growth's copy beside them; it now holds the growing `Vec` alone, whose
  last doubling has the old and new allocation alive at once — at most about
  twice the line. Its secant falling from about four to about two confirms
  this; one staying near four says the allocator kept the freed line, or a
  term the 33.1 notes left unattributed is not the carry's.
- **A one-partition query** held the carry, every chunk under it and a copy
  of the value; it now holds the growing line, then that line as a block and,
  for escaped text and bytea, one decoded copy — about twice the line for
  text, plus seven eighths of it for escaped and a half for bytea. A text
  secant past about two names a copy this account does not.
- **Four workers or partitions** are read against their own one-worker
  column, not against these predictions (33.1's notes: the two loops
  differ).

## What 33.3.1 inherits

- **The carry is where a streamed line would not be held**: a pass decoding
  nothing still holds each line whole while it grows, in `ChunkCarry`, which
  33.3.1 changes for the serial loop and the worker together.
- **A replay's retained blocks are the carried line and the current chunk**,
  nothing more, so a ledger (33.4) charging a held line charges the line once.

## Negative results

- **Recycling the frozen line's allocation** is not possible once a batch
  holds it, and recycling it when no view was taken saves an allocation per
  chunk boundary at the cost of keeping a long line's bytes; it was not done.
