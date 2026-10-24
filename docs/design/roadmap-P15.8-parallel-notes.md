# P15.8 notes — a parallel first pass, `gz-seek`'s side

`gz_seek::Parallel` is the forward pass with chunks of the file ahead of it
found and decoded speculatively on the caller's threads, building the index the
serial pass builds whatever ran (D117). `pgdt-io` does not call it yet: driving
it from the first pass is 15.8.1
([`../status/history/2026-10-24.md`](../status/history/2026-10-24.md), "15.8
split: `gz-seek`'s parallel pass, and the first pass over it").

- **The placement rule is one module both passes apply**, `placement.rs`, cut
  out of `Forward` unchanged; `Forward`'s tests pass as they did. Its input is
  the events either decode reports — member start, header, block boundary,
  member end — and the CRC-32s are the decode's to supply.
- **The frontier is a forward pass on the engine.** At each block boundary or
  member start it reaches it looks for a chunk beginning exactly there; one
  whose markers resolve against the frontier's history and whose trailers all
  match is handed out, and the frontier stands at its stop. A chunk beginning
  with a stored block begins at any of that header's aliases (`find.rs`,
  `stored_length_at`).
- **A speculation** (`speculate.rs`) searches its chunk for a dynamic or stored
  block's start or a member's header (`find.rs`), decodes from it with the
  crate's own decoder (`native/`) to the first boundary or member start at or
  past the chunk's end, and records every event with its plaintext offset and
  each bytes piece's CRC-32. A refused candidate is searched past, up to
  `REFUSALS_PAST_A_BOUNDARY` refused only after a boundary; a limit, or a
  refusal past those, ends the chunk at its last boundary.
- **The native decoder refuses what zlib's `inflate_table` and `inflate`
  refuse**, and is held to the engine's plaintext and boundaries from a
  stream's start and, with markers, from every boundary of every level's
  stream (`native/tests.rs`).
- **The evidence**: `the_index_is_the_serial_pass_s_at_every_worker_count`
  (seven file shapes, BGZF and mixed members among them, three chunk sizes,
  two limits, and the speculations run not at all, ahead, lazily and on
  threads), `a_pass_takes_the_chunks_it_is_handed`,
  `a_resumed_pass_builds_the_uninterrupted_index` (from a serial pass's
  partial index), `damage_is_refused_as_the_serial_pass_refuses_it`, and
  `a_speculation_over_any_bytes_hands_back_a_chunk_or_nothing`.

## What 15.8.1 inherits

- **The contract.** `read` as `Forward::read`, supplying from
  `compressed_position`, which jumps past a chunk taken, until a step
  `needs_input`; a step whose `wait` names a chunk stands where it may begin,
  and the next read waits for `speculated` to be handed that chunk's result.
  `speculate` hands out chunks in order ahead of the frontier; `pending` and
  `held` are what the caller bills. A speculation holds about a chunk of
  compressed bytes and at most the limit of plaintext, twice over while it
  writes markers; nothing of it is charged anywhere yet.
- **The chunk length and the plaintext limit are the caller's** and no
  figure has chosen either; `CHECKPOINT_SPACING` is not one of them, a chunk
  being compressed bytes and a unit plaintext.
- **Resume** is `Parallel::resume` over the partial index the serial pass
  saves, so a first pass interrupted in either mode resumes in either.
- **A BGZF file's chunks begin at member headers** the search finds by magic,
  decoded with no unknown history; whether that is the spec's "units from its
  members' headers" for a fetched file, or the arm walks headers ahead of the
  frontier, is 15.8.1's call, as is whether the first pass's scan partitions.
- **Nothing is timed.** How fast the native decode runs, and so what a pass
  gains at a given count, is 15.7's reading of the arm 15.8.1 wires.

## Negative results

- **A stored header has aliases**: its three header bits and its padding are
  zeros, so a search from any earlier bit whose bits up to the boundary are
  zero — any ASCII byte's high bit — reads the same length and decodes the
  same block. Every alias decodes alike, so a chunk is taken at any of them
  rather than the search guessing which is the header.
- **A false dynamic header can decode a garbage block to its end-of-block
  code**, its codes complete, and a chunk ended at that garbage boundary loses
  the true start later in its range; so a refusal past a boundary is searched
  past too, bounded for damaged bytes.
- **Fixed and last blocks are not searched for**: three bits with nothing to
  check, they would be found at every bit. A writer whose blocks are mostly
  fixed, or one that ends no block (`KD129`, zlib-rs's level 1), gives chunks
  nothing to begin at, and the frontier decodes those bytes itself.
- **After a jump, the engine is fed only the part of the given input at its
  own position**: the rest of the caller's slice sits behind the jump. A step
  that stops for that says so, rather than reading as needing input.
