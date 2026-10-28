# P15.18.2 notes — what a speculation holds bounded

**Every term a speculation holds now has a bound in `gz-seek`; none of them is
charged yet.** The charge is 15.18.3's (`KD141`, its detail now at
`Chunking::task_bytes` in `pgdt-io`'s `first_pass/ahead.rs`). Why the bounds
cut a guess rather than raise a charge is `docs/design/decisions.md`, "D122".

## What landed

- **The speculation holds no events.** `Decode` counts them, and keeps only
  what a guess reads: the last place it reached (`Reached`) and the
  plaintext offset of the member start it is in. The guess it builds is
  unchanged.
- **`EVENTS_A_GUESS`** is 16 × (`SPECULATION_LIMIT` / 16,383), sixteen times
  the blocks a writer that never flushes can end under the limit. A guess
  past it is cut at its last boundary and continued.
- **The compressed bound** is the limit's worth of bytes past the chunk's
  end, or past a continuation's start where that lies beyond it
  (`bound_past`). It is checked at every boundary and wherever the input runs
  out, so the input stops at the bound plus the feed that reached it.
- **A sink grows no further than its limit asks.** `Held::new(history,
  limit)` caps the room at history + limit + 258 + 32. The cap rests on
  `inflate.rs`: `codes` begins a step only below the limit, a literal writes
  one value, a copy at most 258, and a stored run never more than the room
  left (`State::Stored`). `Held::grow` debug-asserts it. A plain sink begun
  where a marked one came clean is made for the limit less what was marked.

## What 15.18.3 inherits

- **The joint maximum.** The marked sink is at most 2 · (L + 290) bytes, and
  it stays at its grown size once the decode comes clean. The plain sink
  beside it is at most (L − m) + W + 290 bytes, `m` being what was written
  marked. Below the cap a buffer still grows by doubling, so either can hold
  up to twice what it held at its last grow.
- **The input** is at most the chunk, the limit's worth past it, one feed
  and the symbol lookahead, in an `Input` that grows by doubling.
- **A taken chunk's decode** records at most about 2 × `EVENTS_A_GUESS`
  events (`size_of::<(u64, Event)>()` each, which `Decoded::held` already
  counts). Over a BGZF chunk it records about four a member, and the chunk's
  bytes bound its members. `EVENTS_A_GUESS` is `pub(crate)`, so a charge in
  `pgdt-io` that names it needs it exported.
- **A single block past the bound is cut back to the boundary before it.**
  The continuation from there meets the same block. A block whose compressed
  bytes alone pass the limit's worth (over 8/15 of the limit in plaintext,
  nearly all 15-bit literals) gives it no boundary, so the chain ends and the
  frontier decodes the block itself.

## Negative results

- **The round this slice resumes was killed by the host's OOM killer.** Its
  draft set `EVENTS_A_GUESS` to `1 << 30`, contradicting its own doc comment,
  and the inside-a-chunk test sized its run from that constant. Building
  that run would take about 1.6 billion blocks, and `truth` records an event
  for each. The constant now follows its doc comment, and the test checks
  that its run stays under 2^16 blocks before building it.
- **The bound is not a parameter that a test sets small.** The test runs the
  shipped constant, which then has to be small enough for a test to reach.
  That is the property a bound must have anyway (D122).
