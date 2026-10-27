# P15.18.1 notes — `Chunking::task_bytes` accounted

**No term moves.** Every term of `Chunking::task_bytes`
(`crates/pgdt-io/src/source/compressed/deflate/first_pass/ahead.rs`) was read
against the code and against 15.18's recording. The charge bounds every file
`gzip-resident` reads, but the code does not make it a bound. The two terms
that hold nothing on the limit leg are the two the code under-charges at
worst, so neither can be cut. The work that makes the charge a bound by
construction is split off as 15.18.2, the bounds in `gz-seek`, and 15.18.3,
the charge in `pgdt-io`
([`../status/history/2026-10-27.md`](../status/history/2026-10-27.md),
"15.18.1's split").

## The account

`L` is `SPECULATION_LIMIT`, `W` is 32 KiB, and the chunk and `INPUT_CHUNK` are
1 MiB each. A task is charged `chunk + L + 2L + 2L + 5·2W + INPUT_CHUNK`.

| Term | What the code holds | Bounded by the term? | On the limit leg |
|---|---|---|---|
| chunk | the speculation's input from its decode's start to the chunk's end | yes, with what `Input::release_before` leaves unreleased, under 1 MiB or half of what it holds | within the 1 MiB a worker the recording gives every speculation term |
| `L`, compressed past the chunk | the input from the chunk's end to the stop | **no** (`KD141`): within one dynamic block, up to 15/8 of its plaintext; across blocks the search does not look for, fixed ones, bounded by nothing, the stop being the first dynamic or stored block past the chunk's end (`is_searched_for`); the decode's `events` grow with every block | nothing; 24 : 1 |
| `2L`, marked plaintext | `Marked`'s `Held<u16>`: the run since the last boundary and `W` before it, released only at a grow freeing half | **no** (`KD141`): `Held::grow` doubles, and `resize` zero-fills, so every page is resident. Where the run it keeps is past about `2^23` values at a grow from `2^24`, the buffer becomes `2^25` values, 64 MiB, about `4L` | about a megabyte, with everything else a speculation holds |
| `2L`, decodes | `DECODES_A_TASK` × tasks decodes handed out or done (`Chain::decodes`), each `out` sized to its guess's length plus a byte, not zeroed, and written whole | yes; the decode the frontier is serving has left the count and sits in the frontier's own task charge | one and a half buffers a worker at the peak |
| `5·2W`, guesses | `GUESSES_A_TASK` × tasks speculations or guesses, each guess's `end` at most `W` values | short: the queued decodes' windows (`ChunkDecode::held`), each done decode's window, `events` and `pieces`, and the chain's end window are uncharged | inside the megabyte |
| `INPUT_CHUNK` | `run_task`'s read buffer | yes | yes |

**The `2L` sink term holds for zlib's default blocks.** At `memLevel` 8 a block
is at most a quarter of `L`. At a grow from `2^24` values the run a sink
keeps is then under half the buffer, so the grow drops it rather than
doubling, and the buffer stops at `2^24` values, just under `2L`. Only a run of more than about `2^23` values that stays marked reaches
the doubling. That takes a block as long as zlib's at `memLevel` 9, nearly all
long matches, whose boundary before it falls between about 8.32M and 8.42M
values into the decode, no earlier grow having compacted. GNU gzip writes
blocks that long at its default level, 32,767 symbols where zlib at
`memLevel` 8 writes 16,383 (`runs/p15.18.1-review/reading.txt`). So
`pg_dump | gzip` reaches the doubling on near-repetitive content, such as a
zero-filled large object.

**`KD141`'s compressed-bytes clause understated it.** The clause said nine
eighths of the limit. A dynamic block's literal codes reach 15 bits, and empty
or near-empty fixed blocks carry no plaintext for the limit to count. A writer
flushing with `Z_PARTIAL_FLUSH` writes those.

## The remainder past the heap, named

**About one freed decode buffer a worker that the allocator keeps
resident.** No reading separates it. The name comes from the allocators'
source, and the reading's magnitude is consistent with it.

- **mimalloc**, the shipped build's: `libmimalloc-sys` 0.1.49 builds mimalloc
  v3.3.2 (no `v2` feature), whose `purge_delay` option defaults to 1000 ms
  (`c_src/mimalloc/v3/src/options.c`). That is longer than the limit leg's
  whole first pass, so pages a freed decode buffer leaves stay committed
  through it.
- **glibc**, the `system` twin: free an `mmap`ed block up to
  `DEFAULT_MMAP_THRESHOLD_MAX` (32 MiB on 64-bit) and the dynamic `mmap`
  threshold rises to its size and the trim threshold to twice that
  (`mallopt(3)`, `M_MMAP_THRESHOLD`). From the first freed decode buffer on,
  every later one comes from a thread's arena and is trimmed only past twice
  its size.

**It scales with the workers**, as both mechanisms do, so it belongs in a
task's charge, never in `MEMORY_UNPOOLED_BOUND`. Today the slack of the two
terms that hold nothing on zlib's files covers it. It is `KD148`.

**The reading that would separate it**, and 15.18.3's opening: the limit
leg's 24- and 4-job pair with each allocator's retention turned off. That is
`MIMALLOC_PURGE_DELAY=0` on the shipped build, and on the `system` one a
stated `MALLOC_MMAP_THRESHOLD_`, which turns the dynamic threshold off. The
remainder falling toward the heap's terms confirms the name. Not falling
leaves it unattributed again.

## What 15.18.2 and 15.18.3 inherit

- **Which terms fill together has not been worked out.** The sink's `4L`
  needs a long marked run, and the compressed `L` a block near 1 : 1. One
  block cannot be both. Nothing here computes the joint maximum, which
  15.18.3 needs before it sets a term.
- **Events are bounded over the whole decode**, not only past the chunk.
  An event is 24 bytes, one per block, and an empty fixed block is ten bits.
  So a chunk of them after a dynamic block holds about 20 MB that no term
  charges. zlib never writes two such blocks in a row, since `deflate.c`
  refuses a second flush with no input between. A damaged or crafted file
  does.
- **The bounds go in `gz-seek`, not in the charge.** Over blocks the search
  skips, the compressed bytes past the chunk have no maximum a charge could
  state. A guess cut where they would pass a bound and continued from its stop
  keeps the chain, as a limit-cut guess does (`Speculation::continuing`).
- **A sink grown no further than its limit asks** means capping
  `Held::grow` at `L + W` plus slack. That is a cold path. A decode under the
  limit never writes past that cap, which 15.18.2 checks before relying on
  it.
- **D117 already names pooling** a decode's buffer, where an account names
  it. The retention is that account, if 15.18.3's reading confirms it.

## Negative results

- **The secant cannot settle a term.** On the limit leg every term whose
  worst case outruns its charge holds nothing, so the reading is far under the
  sum. A term cut on that reading would be cut exactly where the code
  under-charges.
- **The 15.18 notes' "two limits for a block as long as the limit" is a
  floor.** The doubling makes the sink's worst case about four.
