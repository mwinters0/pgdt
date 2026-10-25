# P15.8.5 notes — a BGZF chunk's every member length borne out

`gz-seek`'s chain read (`speculate/members.rs`) takes a member's stated length
only where a BGZF header, a plain member's magic or the file's end follows it.
That was already so for a member ending inside the chunk; it now holds for the
member reaching past the chunk's end as well, a lone first header included. A
length nothing bears out cuts the chain at that member's start, and where that
start is the chain's first, the search goes on past it, so a false or lone
header falls back to the native search instead of handing out a guess no decode
takes. Bearing out comes before the `ISIZE` and limit checks, so a false first
header whose `ISIZE` alone passes the limit is searched past too, and does not
hand back nothing. Nothing outside `gz-seek` changed (D117).

- **The read goes one header further**: the 18 bytes after the member that
  reaches past the chunk's end. With the shipped 1 MiB chunk they lie in the
  piece after the chunk, which a fetched pass's room already holds for the
  speculation's stop. With a chunk under 64 KiB plus a header they can lie one
  piece further on. A task asking for a piece outside the room is refused and
  hands back nothing (`first_pass/download.rs`), so this costs a guess and
  never the index. `a_fetched_bgzf_file_inflates_ahead_over_one_download`
  still passes at its 64 KiB test chunk.
- **The evidence**: `a_bgzf_chunk_s_chain_is_cut_where_it_cannot_be_read_on`
  now lies about the length of the member reaching past the chunk's end as
  well as one inside it. `a_lone_bgzf_header_nothing_bears_out_is_searched_past`
  writes a BGZF header, with a stated length past the chunk's end and an
  `ISIZE` BGZF allows, into a stored block's plaintext, so the file is sound,
  and asserts that the chunk is searched as any file's chunk is. Both fail
  against 15.8.3's read.

## Negative results

- **A length one byte too long proves nothing about the member reaching the
  chunk's end.** The `ISIZE` it reads is shifted one byte into the next
  header, whose magic makes it pass 64 KiB, so 15.8.3's read already cut there.
  The test instead makes the length land inside the member after it and writes
  an `ISIZE` BGZF allows there, so only the bytes after it can refute it.
