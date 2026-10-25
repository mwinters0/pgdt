# P15.8.3 notes — a fetched BGZF file's units from its members' headers

A speculation over a chunk holding a BGZF header no longer decodes it: its
guess is read off the member chain (`gz-seek`'s `speculate/members.rs`, D117).
It begins at the first BGZF header at or past the chunk's first byte and stops
at the first member start at or past the chunk's end, or the file's end, which
is where a decoded speculation over the same chunk stops. Its length is the
`ISIZE`s between, and it carries no history, a member beginning with none. The
plaintext a taken chunk hands out is still the engine's decode of it from that
start. The read is fed from wherever the task's feed reads, so on a fetched
first pass it comes out of the one download (15.8.2).

- **The file decides, chunk by chunk**: the pass is not told it is reading
  BGZF. A chunk holding no BGZF header is searched as before, and each member
  in a chain is read by its own header, as the placement reads it (D116). So
  the read also applies to a local BGZF file whose broken chain sends it to a
  first pass, up to the break.
- **The chain is cut** where it cannot be read on: at a plain member's start,
  which states no length; at the last member start reached where a stated
  length leads to no member, an `ISIZE` passes 64 KiB, a member runs past the
  file's end, or the plaintext would pass the limit. A first header whose
  length lands inside the chunk on no member start is taken for no header, and
  the search goes past it. The member reaching the chunk's end is borne out
  too, by the header after it (15.8.5).
  A first member alone past the limit hands back nothing. A cut at the limit
  is continued from its stop, as a cut speculation is (15.16); any other
  breaks the chain of windows, and the frontier decodes on.
- **Nothing in `pgdt-io` changed.** A task reading a chain holds less than a
  speculation's charge (`Chunking::task_bytes`), and stops no further into
  the next piece than a decoded speculation does, so the download's room is
  unchanged.
- **The evidence**: `gz-seek`'s `a_bgzf_chunk_is_read_off_its_members_headers`
  (every chunk's guess the same with each member's deflate data overwritten,
  over a BGZF file and one mixing plain members, at two chunk lengths and two
  feeds), `a_bgzf_chunk_s_chain_is_cut_where_it_cannot_be_read_on`,
  `bgzf_damage_is_refused_as_the_serial_pass_refuses_it` (a wrong CRC-32,
  `ISIZE` and deflate byte refused with the serial pass's error, a wrong
  `BSIZE` building its index), and the existing
  `the_index_is_the_serial_pass_s_at_every_worker_count` over its BGZF and
  mixed files; `pgdt-io`'s `a_fetched_bgzf_file_inflates_ahead_over_one_download`
  (a sequenced take at two, four and eight workers, the walk's index, and the
  transport's ranges tiling the object once).

## What later slices inherit

- **15.7.1's readings** read a BGZF first pass whose tasks decode each byte once,
  through the engine; a single member's are still decoded natively first.
- **15.16** continues a limit-cut speculation, and a chain cut at the limit
  from its stop, read off the headers there.

## Negative results

- **A chunk shorter than a member may hold no member start.** The decode of the
  chunk before then runs across it, to the first member start past its end,
  so the chunk is never taken itself. The bytes are still decoded ahead, so
  `a_pass_takes_the_chunks_it_is_handed` counts a BGZF file's takes against
  its chunks that hold a member start. Such a chunk is still speculated
  natively, but where its guess begins, inside that member, no chain reaches.
- **A one-task fetched pass over chunks shorter than its members took
  nothing**, in a first draft of the `pgdt-io` test at the 8 KiB test chunk
  against members of about 21 KB. The pass holds each guess until its frontier
  stands past the guess's start, and a frontier inside a single-block member
  stands nowhere. With the guesses held, `pending()` reached the gate
  `Ahead::top_up` holds it to, so no decode was handed out before the frontier
  reached the chunk and decoded it itself. The shipped chunk is many members
  long, so the test runs at chunks of 64 KiB. Nothing about the gate changed.
