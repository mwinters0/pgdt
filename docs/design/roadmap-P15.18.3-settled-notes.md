# P15.18.3 notes — `Chunking::task_bytes` settled

**The charge is set, and the re-take is not taken.** Every term of a task's
charge is now a bound `gz-seek` states on what the code writes, and no decode
buffer is freed while the pass runs (`docs/design/decisions.md`, "D124"). The
`gzip-resident` re-take that closes the row is taken from a commit carrying
the pool.

## What landed

- **`gz-seek` states its bounds beside the code they bound.**
  `ChunkTask::speculation_bytes`, `ChunkTask::decode_bytes` and
  `ChunkTask::GUESS_BYTES` are `const` and public, built from
  `speculate.rs`'s and `chunk.rs`'s `most_held`. The constants they need
  (`LONGEST_HEADER`, `PAD`, `ROOM_PAST_LIMIT`, `STATE_BYTES`, `CHAIN_PAST`,
  `EVENTS_A_DECODE`, `PIECES_A_DECODE`) sit with the code they describe.
  Compile-time assertions hold each task's own structs under `STATE_BYTES`.
- **The joint maximum of a speculation's sinks is both rooms, three limits.**
  The plain sink is made for the whole limit at the decode's start. A member
  beginning inside a marked decode writes into it without the limit being
  reduced by what was marked. So a marked run past half the limit, followed by
  a member running on to the limit, fills both sinks together.
  `a_speculation_s_sinks_fill_together_within_its_bound` builds that file and
  reaches most of the bound. A sink come clean gets a plain sink made for the
  limit less what was marked, so the clean path stays under the sum.
- **A speculation's input** is its chunk, the limit's worth past it (or a
  BGZF chain read's member and header, whichever is longer), a block's
  header, one feed and the padding. A local task's feed is `INPUT_CHUNK`; a
  fetched one's is a piece of a chunk. The charge takes the longer of the two.
- **A decode** holds its window, its plaintext and a byte, its events and
  pieces, its engine and its state. Its events are bounded by its guess:
  fewer than `EVENTS_A_GUESS` and the three a speculation passes between two
  checks of the count, doubled for each static block's start, plus its stop.
  `a_decode_of_a_guess_the_count_cut_records_within_its_bound` decodes a
  count-cut guess over empty fixed blocks, which comes within a few events of
  that bound.
- **Queued decodes are charged with the guesses.** A queued decode's guess
  stays in the chain's guesses, which `GUESSES_A_TASK` already counts, so a
  guess's term carries the window of the decode behind it.
- **The decodes' plaintext buffers are pooled in `gz_seek::Parallel`**
  (`parallel/chain.rs`). Each is made with room for the pass's limit and a
  byte, lent a decode as `Chain::next_task` hands it out, not as it is
  queued, and recycled wherever the pass has done with it: served, refused at
  the take, dropped as passed or by a reset, or landing for a generation no
  longer current. One is made only where none is spare, so the buffers never
  outnumber the decodes out at once, which the charge already covers.
  `a_decode_s_plaintext_buffer_is_lent_the_next` holds a caller with two
  tasks out to three buffers over a file of many chunks. The charge's freed
  term is gone, so a first-pass worker's bill falls by a limit and a byte
  (`runs/p15.18.3/trial-bills/`). A decode whose task the caller drops, or
  whose task panics, takes its buffer with it; `pgdt-io` runs every task it
  takes and hands each result back.
- **A pass's limit now sizes every decode buffer**, so `Parallel::new` needs
  one a caller can allocate. The `gz-seek` tests that passed `usize::MAX` for
  no limit pass `NO_LIMIT`, 4 MiB, past every test file's plaintext.
- **`measure.py` mirrors the new charge, and `GZPASS_BUDGET` rises to
  4 GiB.** The old 3 GiB no longer afforded twenty-four workers a task each
  under `margin_allowance`, at either chunk length
  (`test_the_budget_affords_the_widest_row_at_either_chunk`). The figures'
  container follows as `budget + PARALLEL_HEADROOM`. `gzip-first-pass`
  states the same allowance, so its command moves too, though its rows'
  counts do not. The trial bills at the new allowance are
  `runs/p15.18.3/trial-bills/`, read into `test_measure.py`'s
  `GzipResident.STATED_BILLS`.

## What the re-take inherits

- **Command**: `cd scripts && uv run measure.py --figure gzip-resident`, run
  from a commit carrying the pool. It is long, so follow `CLAUDE.md`'s
  "Long-running processes". The first-pass legs' bills it holds the run to
  are `GzipResident.STATED_BILLS`, read from the pool's trial runs.
- **Done when no cell is under-billed.** Over-bills are expected at every
  count. The charge bounds a crafted file, and `control_gz` and `repeated_gz`
  fill little of it.
- **`KD148` is now what an allocator keeps of a buffer a task outgrows.** That
  covers a speculation's input and sinks, and a decode's events, as each
  grows. It is not the decode buffer, which is pooled and never outgrown. It is
  `(c) unowned`: no file `gzip-resident` reads makes those buffers large.

## Negative results

- **Counting capacity would double the input term for nothing.** A
  speculation's input reserves twice its chunk and grows by doubling, so its
  capacity reaches the next power of two past what it writes. Pages it never
  writes are never touched. Retention of the buffer it outgrew is `KD148`'s.
- **The 15.18.1 notes' "one block cannot be both" does not hold for the
  sinks.** It reasoned about one block's ratio. The sinks fill together
  across a member boundary, so their maximum is their sum.
