# P15.17.3 notes — a chunk decode that does not zero its buffer

A taken chunk's decode (`gz-seek`'s `chunk.rs`) now writes its plaintext into
a vector's spare capacity, `Vec::with_capacity(limit + 1)`, where it wrote
into `vec![0; limit + 1]`. Nothing else changed. The guesses, decodes and
index are the same as before. The buffer is still the decode's own, handed to
the pass as `Decoded::plain` and dropped once served, at the same capacity, so
`Decoded::held` and `Chunking::task_bytes` charge what they did, a pool
being refused until a reading prices its pages (D117).

- **The mechanism is `zlib.rs`'s `Out`**: a slice or a vector's spare
  capacity, which `Raw::inflate` writes after what was written through it.
  Each byte the inflater reports joins the vector's length by one `set_len`
  in the module that already holds every `unsafe` in the crate. The engine's
  `step` wraps a slice in it, and `step_appending` wraps a vector. `Running`
  lost its `produced` count, the vector's length being it.
- **What it relies on is RT26**: the zlib API's `inflate` writes the bytes it
  reports whatever the buffer held, under each of the three engines.
- **The evidence.**
  `a_step_appending_to_spare_capacity_decodes_as_one_into_a_slice`
  (`engine/tests.rs`) decodes three members at levels 0, 6 and 9. It uses
  four supply sizes and three rooms, and checks that appending gives the
  slice path's plaintext and the same events at the same offsets. A
  temporary trace showed the final block's end landing inside an appending
  step with output on either side, the second `inflate` call of `Engine::inflate`. The
  existing chunk and parallel tests now all run through the appending path,
  the limit's refusal among them. `gz-seek`'s suite passes under `zlib-rs`,
  `zlib-ng` and `zlib`.

## What the next slice inherits

- **No reading was taken.** The cost removed was a sliver of the CPU at
  four jobs and a few percent at twelve (15.17.1's notes, "What the probe
  found", and 15.17.2's, "The account"). 15.17.4 opens on this tree and bars against it, so neither
  slice is credited with the other's change.
- **The pages a decode touches** are unread. Nothing writes the buffer
  before the engine does, but whether its pages come fresh from the OS or
  reused from the allocator is mimalloc's business, and 15.17.1's profile,
  user cycles only, cannot see a fault. 15.17.4's page-fault recording reads
  where they land, 15.18 what stays resident.
- **The forward pass, a unit decode and the frontier's engine** still write
  into slices their callers own, through `step`. Whether a caller zeroes one
  is the caller's, and the 15.17.1 profile named none of them.
