# P15.18.4 notes — what a taken decode and a member's header hold bounded

**Every term a task holds now has a bound in `gz-seek`, a BGZF chunk's and
a header's included.** None of them is charged yet: that is 15.18.3's
(`KD141`). Why a BGZF member is recorded by its start, header and end
rather than cut is `docs/design/decisions.md`, "D123".

## What landed

- **A taken decode inside a BGZF member** records its start, its header,
  its end and one piece. A block's end is recorded there only where it is
  the decode's stop, which no chain-read guess has. A decode that begins
  inside a member cannot tell whether that member is BGZF, so it records
  everything until the next header. Its guess was decoded, so its count
  bounds it already.
- **A chain read counts three events a member** (`EVENTS_A_MEMBER`)
  against `EVENTS_A_GUESS`. It is cut at the last member start under the
  count and continued, exactly as at the plaintext limit. The count cuts
  before the limit only where members average under about 3 KiB of
  plaintext, which a BGZF writer's full 64 KiB members do not.
- **A member's `Header`** holds its first `KEPT` bytes (18, asserted to
  cover `BGZF_HEADER_LEN`) and a running CRC-32. It no longer holds every
  byte. Both the engine and a speculation read headers through it.
- **A speculation reading a header** is now cut at its compressed bound,
  as a deflate block is. A guess whose next member's name runs past that
  bound is cut at the member's start. The continuation from there meets
  the same name and hands back nothing, so the frontier decodes the
  member itself.

## What 15.18.3 inherits

- **A taken decode's events and pieces** are each at most about twice
  `EVENTS_A_GUESS`, whichever kind of guess let it begin, at
  `size_of::<(u64, Event)>()` and `size_of::<(u64, u64, u32)>()` apiece
  (`Decoded::held` counts both).
- **A speculation's input** is at most its chunk, the limit's worth past
  it and one feed, a header now included. This closes the exception the
  15.18.2 notes left open for headers.
- **15.18.3's opening reading stands.** Its leg, `repeated_gz`, holds no
  BGZF member and no long header, so nothing it read has moved.

## Negative results

- **A file whose plaintext is a small fraction of its compressed bytes
  gives the caller no chance to run decodes.** `Parallel::read` returns
  only at a checkpoint or when its output buffer is full. A file of
  one-byte members fills neither, so a pass over it given a 64 KiB buffer
  ran the whole file at the frontier and took nothing. The new pass test
  gives the pass 64 bytes of room a step for that reason. Only a crafted
  file has that ratio, so nothing here changes the pass.
