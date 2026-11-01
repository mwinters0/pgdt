# P33.10 notes — a held line grows outside the allocator

**A carried line longer than its read's chunk moves to an anonymous mapping of
its own**, grown to exactly the bytes written by `mremap` and unmapped with its
last `Bytes` ([`decisions.md`](decisions.md), "D139";
[`runtime-invariants.md`](runtime-invariants.md), "RT29"). The readings are a
probe, not a figure (`measurements.md`, "What an instrument can see"):
`runs/p33.10-line-mapping/`, whose `probe.sh` carries the account and its
falsifiers as written before each run, `log` every leg, `growth.out` and
`growth-thp.out` the growth legs; on the shipped build of `c6770f46` plus this
change, built in the image.

- **`CarriedLine` and `LineMapping`** (`pgdt-dump`'s `scan/chunked/line.rs`):
  the carry's line is a `Vec` while it is no longer than the chunk
  (`ChunkCarry::new`, which a read loop and an interior worker hand their
  `chunk_size_bytes`), and a `LineMapping` from the extension taking it past;
  frozen, a mapping is handed over as `Bytes::from_owner`, so a replay's batch
  keeps the mapping and the carry nothing. `pgdt-dump` no longer forbids
  `unsafe`: the mapping's system calls are its one site (D27). Off Linux it is
  a `Vec` (P34's inbox).
- **What proves it**: `the_carry_keeps_no_line_past_its_row` holds what the
  carry keeps while carrying a 64 KiB line to the line and four chunks, which a
  `Vec`'s doubling fails (89,088 bytes on the parent's shape); `streamed_line.rs`
  now reads the peak resident set beside the largest allocation, so a
  metadata-level parse is held to holding nothing near its row where a
  data-level one holds it resident and allocates nothing near it.

## What the readings found

- **The falsifier held**: the one-worker data-level parse over four lines of
  I71's bound peaks at one line and 44.6 MiB, 1094292 KiB, where the figure at
  `6dd4523c` read three lines; at `MIMALLOC_PURGE_DELAY=0` it is 2.9 MiB lower.
  The 44.6 MiB past the line are the base, the read buffers and the statistics,
  not attributed term by term: the account predicted 25–40.
- **The growth's gap is material, and it is the page size, not the remaps.** At
  the bound the line grown by remapping writes in about twice the time of one
  written into a mapping sized for it, and drops in about twenty-five times it,
  together some 180 ms a line against a parse's 1.7–2.4 s; at 32 MiB and
  512 MiB in the same proportion. `growth-thp.out` names it: the sized mapping
  is backed whole by transparent huge pages (THP is `always` here) and the
  grown one by none, so it faults and frees 4 KiB pages, 262,144 at the bound.

## What the later slices inherit

- **The gap is closed by the growth, not by the arrangement**: 33.12 grows a
  held line in huge-page steps, its mapping kept aligned (the spec's "What a
  held line costs"), so 33.4's charge carries less than one huge page past a
  held line's bytes and sizes nothing from its block's longest line.
- **A draw in bytes held is the line**: a frozen mapped line is a `Bytes` of
  exactly its length, wherever a batch keeps it. A line no longer than its
  chunk is still the allocator's, inside the reader's chunk term.
- **A query's decoded copies (33.9) are still the allocator's**; nothing here
  moves them.

## Negative results

- **No count pair was taken**: the row path is unchanged, the carry's extend
  running once a chunk boundary, and P33's spec adopts no pricing of a
  hot-path change ([`decisions.md`](decisions.md), "D108").
