# P15.15 notes — a taken chunk decoded by the engine

Every byte `gz_seek::Parallel` hands out is now the engine's (D117). A
speculation hands back a guess — where its chunk begins and stops, the
plaintext's length between, and the history at the stop, still marked
(`speculate.rs`, `Guess`) — and its plaintext goes nowhere. The chain
(`parallel/chain.rs`) resolves those histories in order from a place the
frontier stands at, giving each chunk ahead a window, and queues a decode of
each through the engine (`chunk.rs`, `ChunkDecode`), run on the caller's
threads like a speculation. The frontier takes a decode only standing at its
start with the very history it began with, every trailer it holds matching.

- **A speculation stops where the next chunk's search begins**: at the first
  member start past its chunk, or block boundary there whose next block is
  one the search looks for (`find::is_searched_for`). Under 15.8's rule — any
  boundary past the chunk — a chunk ending at a fixed block stopped short of
  the next one's start, and a chain breaks there: the frontier decodes on to
  the next guess's start and anchors again, and the decodes after it wait for
  that. `a_chunk_is_the_file_from_its_start_to_its_stop` asserts each guess
  begins at the stop before it, at three levels.
- **The frontier decodes the chunk it anchors at itself**, the chain resuming
  at that chunk's stop; and a decode still queued, never handed out, where it
  stands is dropped for the frontier to decode, rather than waited for. It
  waits only for a task handed out.
- **A window the frontier contradicts drops the generation**: every later
  decode rests on it, so all are dropped, a running one's result too when it
  comes back, and the frontier decodes on until a guess begins where it
  stands. `a_decode_the_frontier_contradicts_is_decoded_again` tampers every
  third guess's history.
- **A decode compares no trailer** (`Engine::unchecked`), the member's
  plaintext before the chunk being the frontier's to know: it reads each into
  its events, and the frontier compares them before taking it, as it did a
  native chunk's.
- **A decode writes at most its guess's length**: an engine decode running
  past it means the guess was wrong, so it hands back nothing, and the room it
  holds is bounded.

## What 15.8.1 inherits

- **The contract.** `read` as `Forward::read`, supplying from
  `compressed_position`, which jumps past a decode taken, until a step
  `needs_input`. `task` hands out a `ChunkTask` — a decode the chain has a
  window for, in chunk order, else a speculation on the next chunk ahead —
  fed from `wants` and finished into a `ChunkDone` for `done`. A step whose
  `wait` names a `TaskId` stands where that task's result may be used, and the
  next read waits for it. `wants(id)` says whether a task handed out is still
  of use, a dropped generation's decode or a passed chunk's speculation being
  of none.
- **What is billed.** A speculation running holds about a chunk of compressed
  bytes and at most the limit of plaintext, twice over while it writes
  markers; a decode running, its 32 KiB window, an engine (`DECODER_BYTES`)
  and at most its guess's plaintext, so at most the limit. `held` is what the
  results waiting for the frontier hold: each guess's marked history, up to
  64 KiB, until the frontier passes it, and each decode's plaintext. Nothing
  is charged anywhere yet.
- **Resume** is unchanged: `Parallel::resume` over the partial index the
  serial pass saves.
- **Nothing is timed.** Each byte is now decoded twice, natively and by the
  engine, both on the caller's threads; whether that keeps the pass from
  lifting the inflate at the counts a machine runs is 15.7's reading, which
  D117 reopens on.

## Negative results

- **A chain still breaks** where a guess was cut short by refusals past a
  boundary, or began at a false start: the next guess does not begin at its
  stop. One cut by its limit is continued from its stop since 15.16. The frontier decodes on and anchors at the next
  guess's start, so what it costs is that stretch decoded serially.
