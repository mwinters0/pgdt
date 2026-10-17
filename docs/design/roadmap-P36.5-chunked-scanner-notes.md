# P36.5 notes — `ChunkedScanner`

The chunked protocol is `pgdt_dump::ChunkedScanner`, in
`crates/pgdt-dump/src/scan/chunked.rs`, beside the state machine it drives.
The protocol is the line carried across a chunk boundary, the two passes
over each chunk, and the line limit. All four serial read loops call it:
the preamble scan (`scan`), `map_forward`, the back-fill's `observe_rows`,
and the replay. `ChunkCarry` and `ChunkPass` are private to the module.
D23 names it.

## The shape

- **A loop asks for a chunk, then drains its events.** `next_chunk(source)`
  reads, and `ChunkEvents::next_event` walks both passes and checks the
  line limit where a non-last chunk runs out. After draining, `at_end` says
  whether that chunk was the last. Each loop keeps its own work between
  chunks: `map_forward` and `observe_rows` poll the cancellation and match
  `cancelled_read` on the read's error, and the replay consults its dynamic
  filter and retains the chunk (`ChunkEvents::read`). `skip_to` is where
  `map_forward` puts the scanner down after the leader closed a block.
- **An event borrows the chunk, not the `ChunkEvents`.** So the replay can
  ask `chunk.scanner()` where it stands while it holds a row, and can keep a
  span's validated prefix across rows. `ChunkEvents::span` gives the span
  and its offset. The replay keys its validated prefix by that offset,
  because two spans that each held a row start at different offsets.
- **The carry is two buffers.** The carry pass's line is not written while
  its chunk is scanned. What the passes leave over goes into a second
  buffer. `absorb` swaps the two rather than copying, and an unfinished
  line stays where it lies, so no extra bytes are copied. This is what lets
  the events of both passes carry the chunk's lifetime.
- **The interior split's reference scan in `interior/tests.rs` hands the
  scanner the file as one buffer.** The scanner over one buffer is what the
  two passes reduce to with an empty carry.
- `scan/chunked.rs` also takes `ScanOptions`, `SCAN_LINE_DEFAULT_MAX_BYTES`,
  `announce_read_loop` and `scan` out of `scan.rs`. The edge runs one way,
  from the read loop to the state machine. `KD93`'s marker moved with
  `max_line_bytes`. `measure.py`'s `SCAN` paths name the new file.

## Negative results

- **One event stream across chunks** was refused. `map_forward` awaits
  inside its handler and polls between chunks, and the replay consults its
  dynamic filter between chunks. A stream that hides the boundary leaves
  them no point to do it at.
- **A callback per event**, the shape `scan` takes, does not serve
  `map_forward`, whose handlers await.
- **A lending `next_event` over one carry buffer** does not compile. An
  event from the carry pass borrows the buffer that recording the pass's
  consumption must then write. It is the borrow checker's conditional-
  return case, and the two buffers above remove it.

## The count: the slice's own move, accepted

`measure.py --count-pair HEAD` on the uncommitted tree, whose parent is
`7b6981b1` (36.4.5's code, docs since), ran detached
(`runs/count-pair-36.5-20261017/log`, its `HANDOFF.md` beside it). It
exited 0 after 27.2 minutes, and **all five rows resolved as rises** of
23.3 to 27.6 million instructions: +2.634% on the metadata-level `parse`,
under +0.14% on the other four. The reading written before it ran
expected no row to resolve, or a fall, so the slice was refused and the
rise counted exactly.

**It is the slice's own code, one more move of each event.**
`runs/attr-36.5-20261017-callgrind/` runs the metadata-level and
data-level `parse` under callgrind on the pair's two legs, copied to
`/mnt/ssd/fedora/scratch/pgdump_query/attr-36.5/`, where its `run.sh` names
them. `analyze.py`'s rules were written before the count, and
`reading.txt` is what it printed.
- **Both shapes reproduce the native rise to within 0.3%**, every rep
  pair, and all of it is in changed code. The data shape's statistics
  bytes match on both legs.
- **About 96% is in `map_forward`'s body**, polled as often in both
  builds. The rest is `next_chunk`, `feed`, `end_pass` and dropping the
  `Ok(None)` that ends a pass, once per chunk or pass.
- **The body runs about 30 more instructions per event** (`hot.txt`, the
  instructions run 100k+ times in each build). `ChunkEvents::next_event`
  tests the scanner's `Result<Option<Event>>` for `Ok(None)` before
  handing it back. The caller's `?` and `Some` then test it again, and the
  88-byte payload is moved once more than the parent moved it.
- **The query and `pgdt sql` rows were not counted exactly.** They read a
  cache an untimed `parse` wrote, so their rise is the replay's loop. It
  is the same size per row as the mapping loop's, and is unattributed
  beyond that.

**No shape of this slice's code as clean recovers it.**
`probe/probe.txt` in that job lists the variants, counted on a no-LTO
probe build that reproduces the rise:
- **A cold continuation for the pass switch recovers about a quarter**,
  with or without the hot path's one `next`. Its saving rests on inlining
  attributes, which 36.4's rework refused (history, 2026-10-16, "36.4's
  rework recovers the data-level rise; the summary reads by kind").
- **An infallible `end_pass`, the line limit checked by `next_chunk`
  before it reads, recovers the same quarter.** It changes where a long
  line is refused for a loop that leaves between chunks, the replay's
  dynamic filter for one, and so prices a semantic change for codegen.
- **Unwrapping the scanner's result and rebuilding it recovers nothing**,
  as `next_event`'s comment says.
- **Boxing `Event::CopyStart` recovers about two thirds, and with the cold
  continuation nearly all.** It is not this slice's code. `CopyStart`
  sets `Event`'s size, so every move of every event shortens, the
  parent's as much as the tree's. Its own pair would count a fall, not
  recover 36.5's rise.

**It is accepted** (spec, "What a hot-path slice owes"), and 36.5 is
ticked. The rise is the size of the indirection the reshape adds.

## What the next round inherits

- **36.6 is counted against the commit that carries this slice.**
- **Boxing `CopyStart` is the candidate the count found**, admitted as
  `M225` ([ledger](out-of-band.md)). It is counted by its own
  `--count-pair`, and `probe/probe.sh` re-probes it before then.
- `ChunkedScanner::new` takes a `&ScanOptions` for its chunk size and line
  limit, so 36.6's narrowing of what `scan` takes reaches it too.
