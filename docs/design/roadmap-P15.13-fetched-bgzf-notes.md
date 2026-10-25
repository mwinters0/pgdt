# P15.13 notes — a fetched BGZF file read by a first pass

An uncached BGZF file over HTTP is read by the first pass any other gzip file
is read by, once, and the index it builds is the one a local walk reads off
the same file's headers (D116). The local arm still walks.

- **`gz_seek::Forward` coalesces BGZF members**: a member start is a
  checkpoint only at the file's start or where the unit since the last one
  holds the spacing, no block boundary is one, and a unit records no CRC-32.
  A member start the spacing does not owe waits for its header (the engine's
  new `Event::Header`), and is placed where that header is not BGZF's; this
  slice ended the run there for good, and 15.14 decides each member by its
  own header. The rule is intrinsic to the pass, so it applies on both arms:
  **a local file whose walk finds its chain broken is now read by a pass
  coalescing its BGZF members**, where before it placed every member start —
  the index differs, the plaintext and its checks do not.
- **The member count moved from `gz_seek::End` into the index**:
  `from_parts(spacing, checkpoints, members, end)`, `End` now plaintext and
  compressed only, and an incomplete index counts each member beginning at or
  before its last checkpoint. A coalesced unit holds members no checkpoint
  begins, so a pass resumed from a saved index could not otherwise end with
  the walk's count. The persisted gzip parts carry it (`CACHE_FORMAT_VERSION`
  77; plain and `.xz` caches encode as at 76).
- **A resumed pass reads the member's header again** at a member start (from
  15.14 the rule's only input there). `Forward::index` answers the index a
  pass was resumed over until it places that member start again.
- **`open_remote` hands every uncached gzip file to `FirstPass::fresh`**;
  `walk_fetched` and the walk's `fetches` field are gone. The first pass's
  started and complete lines are its announcement, at `info`; D64 now states
  the level by whether a build's cost is the scan's own.
- **The evidence**: `gz-seek`'s `a_forward_pass_over_bgzf_builds_the_walk_s_index`
  (equal indexes at four spacings),
  `bgzf_members_are_coalesced_on_both_sides_of_a_plain_member` (as 15.14
  rewrote it), and
  `a_resumed_pass_builds_the_uninterrupted_index` resumed from every checkpoint
  of a BGZF file and of one whose run ends mid-file; `pgdt-io`'s
  `a_first_pass_over_a_bgzf_file_builds_the_walk_s_index` (both feeds, and
  stopped halfway and resumed on the other); `remote.rs`'s
  `a_remote_bgzf_file_is_read_once_by_a_first_pass_that_builds_the_walk_s_index`
  (the probe and no walk at open, the ranged bytes after it no more than the
  object, the local walk's index) and
  `parse_over_a_remote_bgzf_file_announces_its_first_pass_and_no_walk`.

## What later slices inherit

- **15.8** widens the fetched arm's pass, which already reads BGZF: its
  units can be taken from the headers ahead of the frontier, and the index it
  must build is the one this pass builds, coalescing included.
- **15.7.2's manual** writes gzip, BGZF included, into "Reading a dump over
  HTTP"; this slice corrected only `dump-inspection.md`'s sentence saying a
  BGZF file is read as an `.xz` file is from the first command, now said of a
  local one.

## Negative results

- **The run is not ended by a stated length or an `ISIZE` the decode
  contradicts**, though the walk refuses both: the pass learns either only at
  the member's end, and a pass resumed at the next member start could not
  recover it, so resume would diverge from the uninterrupted pass. Each such
  member is still checked by its own trailer.
- **The fetched arm was not given a caller-chosen mode** (`Forward::bgzf`):
  the mode would have had to be persisted for a resume, and a broken chain
  would have had a different index on each arm.
