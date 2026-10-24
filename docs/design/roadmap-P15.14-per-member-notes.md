# P15.14 notes — BGZF members coalesced per member

`gz_seek::Forward` decides each member by its own header and the unit it would
join, not by whether every header so far was BGZF's (D116). `KD135` is struck.

- **Two flags replace the run's one**: whether the member being read is
  BGZF's (no block boundary inside it is a checkpoint, and its unit records no
  CRC-32 at its trailer), and whether the unit since the last checkpoint began
  at a BGZF member start (a later BGZF member may join it while it is short of
  the spacing). A deferred member start is deferred only into such a unit, so
  at a header both flags take the header's answer, whether the member joined
  the unit or was placed.
- **A resume consults nothing before the member**: at a member start both
  flags start false, so the start is placed again before its header is read,
  as every resume point was placed in the uninterrupted pass; at a block
  boundary the member is plain. The read of the unit before it is gone.
- **What a file now indexes to**: a BGZF run after a plain member is the
  index a walk of that run alone builds, moved to where it sits, whether the
  file opens with the plain member or carries it among BGZF ones; a plain
  member is unchanged. A file the walk accepts has only BGZF headers, so its
  index is the walk's as before (`a_forward_pass_over_bgzf_builds_the_walk_s_index`).
- **`CACHE_FORMAT_VERSION` 78**: a gzip cache an earlier pass saved over such
  a file holds checkpoints this rule would not choose, and a resume from one
  would mix the two. Plain and `.xz` caches encode as at 77.
- **The evidence**: `gz-seek`'s
  `bgzf_members_are_coalesced_on_both_sides_of_a_plain_member` and
  `a_file_opening_with_a_plain_member_coalesces_the_bgzf_members_after_it`,
  each asserting the index equal to its parts' own passes moved into place,
  both failing on the rule this slice replaced; and
  `a_resumed_pass_builds_the_uninterrupted_index` grown a file opening with a
  plain member, resumed from every checkpoint.

## What later slices inherit

- **15.8** inherits this rule for a fetched BGZF file's units taken from its
  headers and for a speculative pass over a file mixing the two kinds: where
  a BGZF member's start falls depends on its own header, where its unit
  began and that unit's plaintext so far, never on a header earlier than the
  unit's.
