# P15.16 notes — a chunk cut at its plaintext limit continued

A guess the plaintext limit cuts is now marked `cut`, and `gz_seek::Parallel`
hands out a speculation continuing it from its stop
(`Speculation::continuing`), with the same end as the chunk's own guess. That
covers a decoded guess cut at a block boundary or a member start, and a BGZF
chain cut at a member start. The pieces chain, so the limit bounds what one
speculation or decode holds and no longer ends the chain of windows (D117).
The limit is `gz_seek::SPECULATION_LIMIT`, twice the largest block zlib ends,
and `pgdt-io`'s `Chunking::SHIPPED` passes it. That one constant is the only
change outside `gz-seek`.

- **A chunk holds as many guesses and decodes as its guess was cut into.**
  The chain keys both by the bit they begin at, not by chunk. A `TaskId`
  names a continuation by its start and a decode by its start and generation.
  A guess beginning at a stored header's alias lies within the 8 bits before
  the header (`Chain::guess_at`).
- **Order.** `task_through` hands out a decode the chain can begin first,
  then a continuation, then the next chunk's speculation. A continuation is
  handed out only on a chunk at or before `last`, the chunk its start lies
  in. One the frontier has passed is dropped unrun.
- **A continuation from a member start reads BGZF headers only there**
  (`Members::at`). A plain member there is decoded from its header, and a
  lone BGZF header falls back to the search, as on a chunk's own speculation.
- **The frontier waits for a running continuation that begins where it
  stands**, as it waits for a chunk's own speculation.
- **The evidence**: `a_limit_never_ends_the_chain` runs every task ahead,
  with chunks holding more plaintext than twice the largest block. The
  frontier is anchored no more often than with no limit, it takes more
  decodes than chunks, and the index is the serial pass's. It fails with the
  continuation's queueing disabled (level 0 anchored again).
  `a_limit_cuts_a_guess_continued_from_its_stop` checks that the pieces make
  up the uncut guess, its length and its history at the stop. It covers
  block boundaries, plain member starts and, in
  `a_bgzf_chunk_s_chain_is_cut_where_it_cannot_be_read_on`, a BGZF chain.
  `a_continuation_is_handed_out_within_its_bound` checks the bound.

## What later slices inherit

- **15.7** reads the pass at `SPECULATION_LIMIT`, held fixed across its
  chunk-length legs. `Chunking::task_bytes` is unchanged: a continuation
  holds no more than a speculation. Queued continuations hold nothing and
  are not counted by `Parallel::pending`.
- **15.10** inherits a chain whose only breaks are a writer ending no block
  under the limit (`KD129`), refusals past a boundary, a BGZF chain's
  damage, and a false start.

## Negative results

- **A piece that reaches no boundary under the limit hands back nothing**, so
  a writer whose blocks are longer than the limit still breaks the chain
  there. zlib-rs's level 1 ends no block at all.
- **A continuation that a refusal cuts past its chunk's end finds nothing.**
  Its search range ends at the chunk's end, so the frontier decodes on.
- **A continuation never shares a start with a chunk's own guess.** A cut lies
  either before the next chunk's first byte or at a boundary past it whose
  next block no search looks for.
