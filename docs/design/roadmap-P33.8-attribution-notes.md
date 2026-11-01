# P33.8 notes — attribution

**Both terms 33.3's re-take left unattributed now have names.** The third byte
a line byte that a one-worker parse and a one-partition query hold is
mimalloc's delayed purge of the line's outgrown buffers; a decoded copy enters
a one-partition query's peak as two live copies over escaped text and three
over bytea, each beside the held line, past what the growth holds. No
production code. The readings are a probe, not a figure (`measurements.md`,
"What an instrument can see"): `runs/p33.8-attribution/`, whose `probe.sh`
carries the account and its falsifiers as written before the first run, `log`
every leg's peak resident, and `*.introspect` the instrument's reports. Each leg
is `long-line-resident`'s command at one worker or partition, its inputs read
from the SSD rather than tmpfs, on the shipped build at `6dd4523c` and the
`introspect` build of the same tree, at mimalloc's shipped purge delay and with
`MIMALLOC_PURGE_DELAY=0`.

## What the readings name

- **The line's buffer ends at up to twice the line.** `ChunkCarry`'s line is a
  `Vec` grown a chunk at a time, seeded at the chunk's remainder plus a chunk,
  so its capacities double from there and the last can land anywhere in
  `[L, 2L)`; the figure's inputs start each line about 2 KB into a chunk, so a
  512 MiB line or one at the bound ends in about `2L` of capacity, touched to
  `L`. The counter's `live_peak_bytes` is that capacity, not a live copy, on
  every parse and text query leg.
- **The growth's copy is the final doubling's.** mimalloc's `realloc`
  allocates, copies and frees (3.3.2's `_mi_theap_realloc_zero`, the version
  `libmimalloc-sys` 0.1.49 builds by default), so the outgrown buffer
  and its successor are resident together at the copy: about the final
  capacity, between one and two lines by where the line sits in the doubling
  series. With every freed buffer purged on free, a one-worker parse and a
  one-partition text query hold that and nothing else, within a few MiB of
  `2L` plus the base, at 512 MiB and at the bound.
- **The third byte is mimalloc's retention.** Every outgrown buffer is an
  arena page freed back to its arena and kept committed and resident until a
  purge, which mimalloc runs from an allocator call no sooner than its
  `purge_delay`, 1000 ms, after the free; a line's doublings come faster, so
  the earlier buffers stay resident. They sum to about the final capacity
  `C`, so a one-worker parse holds `L + C`: over text that sums every cell of
  `long-line-resident`'s one-worker column, 32 MiB to the bound, to within a
  constant, the process's base ([`measurements.md`](measurements.md), "What a
  held line holds resident") — three lines where `C` is about `2L`, fewer at
  288 MiB, where it is not. At a purge delay of 0 it is gone, so it is no live
  term. How much is retained depends on how fast the line grows: the text
  query at the bound held about half a line less here than in the figure's
  sitting, whose inputs were on tmpfs, and the figure's escaped-text bound
  reps spread as far.
- **An escaped text value is copied twice.** `unescape_owned` writes it into a
  `String` of the field's length, and `push_utf8view_field`'s `Cow::Owned` arm
  copies that into the `StringViewBuilder` (`pgdt-query`'s `batch/chunks.rs`):
  with the held line, about `2.75L` resident at the decode, past the growth's
  `2L`, and the counter's live peak is the line's capacity plus both.
- **A hex bytea value is copied three times.** pg_dump writes `\\x`, so the
  field is escaped and unescaped whole into a `String`, `decode_bytea` decodes
  that into a `Vec` of half, and `BinaryBuilder::append_value` copies the `Vec`
  (`batch/builder.rs`'s `append_typed`): about `3L` with the line.
- **A strict parse over bytea copies it twice**: `check_row` unescapes the
  field (`pgdt-mapping`'s `statistics/gather/strict.rs`) and `field_key` keys it
  as decoded bytes (`pgdt-codec`'s `key/order.rs`), about `2.5L` with the line.
  A default parse over escaped text or bytea, and a strict one over text, hold
  only the growth.
- **Why the figure's copies add less than the account summed**: at the shipped
  purge delay the decode's buffers land in the retained outgrown ones, which
  are resident already, so a copy adds to the peak only past what retention
  holds.

## What the later slices inherit

- **The spec's "at most one decoded copy" is not yet true** of escaped text,
  bytea, or a strict bytea check; that removal is admitted as 33.9 (`STATUS.md`).
- **The capacity, the growth's copy and the retention are gone for a line past
  its chunk**, which grows in a mapping of its own at exactly its length
  ([`roadmap-P33.10-mapping-notes.md`](roadmap-P33.10-mapping-notes.md)); a
  decoded copy is still the allocator's, and 33.9's.
- **33.3.1 rewrites the carry a streamed pass grows its line in**; a pass that
  decodes still grows it as above.

## Negative results

- **`mimalloc_committed_peak_bytes` does not see a purge here.** On Linux
  mimalloc purges with `MADV_DONTNEED` and keeps the range marked committed
  (`_mi_prim_decommit`), so `committed` is unchanged at a purge delay of 0
  while resident falls: its gap above the counter bounds retention, it does not
  measure it. The instrument told the terms apart only beside peak resident
  and the purge delay.
- **heaptrack places the live terms and sees no retention**: its recording of
  the bytea query on the `system` build
  (`runs/p33.8-attribution/heaptrack-query-bytea512.txt`) puts the peak heap
  at the counter's live peak, in four sites — the carry's growth
  (`chunked.rs`, `absorb`), `unescape_owned`, `decode_bytea_hex` and
  `append_typed`'s `BinaryBuilder` — and glibc grows a block past its mmap
  threshold in place, so neither the growth's copy nor the retention exists
  there to see.
