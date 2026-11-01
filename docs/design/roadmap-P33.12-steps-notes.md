# P33.12 notes — a held line grows in huge-page steps

**A mapped line grows in whole steps, its start on a step's boundary**: the
kernel's PMD huge page where that is at most 2 MiB, the base page elsewhere
(`LineMapping::step`, read once from `hpage_pmd_size`). A growth asks `mremap`
to grow in place and, where it cannot, moves by `MREMAP_FIXED` to an aligned
range it reserved inaccessible (`reserve_aligned`); the first mapping is placed
the same way. It asks for no huge pages ([`decisions.md`](decisions.md),
"D139"; [`runtime-invariants.md`](runtime-invariants.md), "RT30"). The
readings are a probe, not a figure: `runs/p33.12-line-steps/`, whose
`probe.sh` carries the account and falsifiers as written before each leg,
`log` every leg, `growth.out`, `growth-compacted.out` and
`parse-textbound.out`; on the shipped build of `a131a153` plus this change,
built in the image.

- **What proves it**: `a_moved_mapping_keeps_its_alignment_and_its_bytes`
  forces the move path (`grow_by_moving`, test-only); the line tests hold the
  mapping to whole steps and an aligned start after every extension; and
  `the_carry_keeps_no_line_past_its_row` carries a line of two and a quarter
  steps, where whole steps fall short of a doubling's capacity, so it still
  fails a `Vec`. Whether a step is *backed* by a huge page is the host's
  policy, so no test asserts it; the probe reads it.

## What the readings found

- **The gap 33.10 found is closed.** After `vm.compact_memory`, the line grown
  by `LineMapping` writes and drops at the pace of one written into a mapping
  sized for it at 32 MiB, 512 MiB and I71's bound, each backed whole by huge
  pages, the bound's moved eight times and aligned throughout
  (`growth-compacted.out`); 33.10's grown line took about twice the write and
  twenty-five times the drop, backed by none.
- **A move keeps its alignment and its huge pages**: three, seven and eight
  moves at the three lengths, every line ending on a step's boundary, none
  backed less than the sized leg. Most growths extend in place.
- **Without compaction the host decides, for both legs alike**: the first
  sitting backed the bound's line about 86% in huge pages in both legs, its
  write and drop slower in both, with fewer free 2 MiB blocks than the line
  needs and `defrag` at `madvise`, which does not compact at a fault
  (`buddyinfo-after-growth.txt`). That is RT30's scope limit, not the growth.
- **The parse leg's peak is one line, the base and under a step**: a
  one-worker parse over four lines of the bound peaks 600 KiB above 33.10's
  reading. No parse timing is concluded.

## What the later slices inherit

- **33.4's charge for a held line past its chunk is its bytes rounded up to a
  step**, `LineMapping::mapped`, under one step past them; a step is 2 MiB
  here and the base page on a kernel without THP or with a larger PMD page.
- **A move transiently holds a reservation of the new length plus a step** of
  address space beside the old mapping, inaccessible and uncommitted; only an
  address-space limit sees it.

## Negative results

- **No move failed to keep its alignment**, so neither shape the spec rejects
  for that case is reopened.
- **No count pair was taken**, as at 33.10: the row path is unchanged.
