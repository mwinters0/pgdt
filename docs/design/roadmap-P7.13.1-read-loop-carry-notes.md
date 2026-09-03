# P7.13.1 — The copy into each read loop's own buffer

What the rest of P7 inherits: **nothing copies file bytes any more.** All three
read loops scan the chunk where the reader left it, carrying only the one line
that straddles its front edge, and `__memmove_avx_unaligned_erms` — 38.0% of a
warm `parse`'s user time, the largest single term left after 7.13 — is out of
the profile entirely. The mechanism and its rejected alternatives are filed by
subject: [`architecture.md`](architecture.md), "The scanner never owns the bytes
it scans" for the carry, and "parse-profile" for what a `parse` profile is now
made of. This doc holds the apparatus, the two invariants the safety argument
rests on, and what 7.6 and 7.7 walk a row inside.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/scan.rs`, `ChunkCarry` / `ChunkPass` | the whole mechanism: the carried line, `absorb` to complete it from the chunk's first line-terminated prefix, `span` for what each pass scans, `consumed` for what each pass leaves |
| `pgdump_query/src/scan.rs`, `ChunkCarry::PASSES` | the two-pass order, iterated identically by all three loops |
| `pgdump_query/src/scan.rs`, `scan` | loop 1 |
| `pgdump_query/src/stream.rs`, `map_forward` | loop 2 |
| `pgdump_query/src/stream.rs`, `table_stream`'s replay loop | loop 3 — the one that also retains chunks in `batch::SourceChunk` |
| `pgdump_query/src/scan.rs`, `mod tests` | seven: the carry's bound, event-stream invariance under the split, the empty carry, the prefix rule, the no-newline fallback, the carry pass's `eof`, and an unterminated block at every chunk size |

`CopyScanner` itself is untouched — no signature, no state, no event. So is
`io.rs`, `batch.rs`, `map.rs` and every caller above them. The change is three
loop bodies and one new L1 type.

## The shape, and why it needs no scanner change

`next_event` stops only where it finds no newline, so **what a chunk leaves
unconsumed is always a single unterminated line** — never two, never a partial
plus a whole one. That is the fact the whole design turns on, and it is why the
carry is a line rather than a buffer: per chunk, one row's worth of copying
instead of the chunk's length, plus the `drain` that used to shift the remainder
down.

`CopyScanner::base` is an absolute file offset and `take_consumed` resets `pos`,
so bracketing each pass with `take_consumed` is exactly what a single buffer's
refill already did. Handing it two different buffers inside one chunk is
therefore not a new capability being asked of it; it is the existing contract
used twice.

Two edges are worth not re-deriving:

- **The carry pass takes `eof` only when the chunk has nothing left for the
  in-place pass** (`split == chunk.len()`). Give it `eof` any earlier and the
  scanner treats a line still being carried as the file's last one, which
  silently turns a straddling row into a short row. `ScanOptions`' unterminated
  `COPY` block and large-object errors both hang off that flag, and the test
  that drives an unterminated block at every chunk size is what holds it.
- **`want == 0` is a real iteration**, not one to skip: it is how a scan that
  ends exactly on a chunk boundary reaches the scanner with `eof` set. Each loop
  substitutes `Bytes::new()` rather than branching around the passes.
- **A carry the scanner did not finish would mis-base the in-place span**, by
  exactly the residue, since `take_consumed` would leave `base` short of
  `chunk[split]`. It cannot happen — a carry that gained a line-terminated
  prefix is entirely consumable, and the only carry that is not took the whole
  chunk and leaves the in-place span empty — and `span`'s `debug_assert` is what
  keeps it that way under every future edit.

## The two invariants the safety argument rests on

Neither is new — both were already true, which is what made this slice a rework
of three loop bodies rather than a redesign.

- **A chunk's `Bytes` outlives the pass that scans it.** 7.13's pool returns a
  buffer only when the last reference to it dies, so an in-place scan is not
  also a lifetime problem. The replay loop was already retaining that same
  `Bytes` in `batch::SourceChunk`, which is why the query path's chunk retention
  needed no decision after all: `chunks.front().end() <= floor` pops a chunk only
  once the scanner has walked past its last byte, and the row that straddles the
  next boundary is carried rather than scanned, so it never reaches the eviction
  needing a chunk that has gone.
- **The zero-copy `Utf8View` path never depended on which buffer a row was read
  out of.** `push_utf8view_field` locates a field's chunk by *absolute file
  offset* and copies when it finds none. A row scanned in place and a row
  reassembled on the carry resolve identically, and a field that straddles two
  chunks falls back to a copy exactly as it did before.

## What it was worth

The load-bearing evidence is deterministic, for the reason 7.13 gave: a profile
is a proportion and this machine is not reliably quiet.

| | before | after |
|---|---|---|
| user instructions, warm 3.00 GiB `parse` | 1,703,555,926 | 1,403,769,503 (−17.6%) |
| the same over the `--arrays` file | 18,080,399,505 | 17,780,795,698 (−1.7%) |
| `query --schema-mode strings`, whole run | 26.86 G | 26.29 G (−2.1%) |
| `query --schema-mode typed`, whole run | 96.93 G | 95.59 G (−1.4%) |
| wall / user / system, warm `parse` | 0.48 / 0.17 / 0.30 s | 0.40 / 0.11 / 0.30 s |
| peak RSS, warm `parse` | 7.9 MB | 6.0 MB |

Medians of five reps each; the instruction spreads are 0.0003% and 0.0022% of
their own medians, and the timings are **interleaved** before/after reps in one
window rather than two runs of five, because half an hour separated the first
two attempts and the machine was measurably quieter for the second. `before` is
a `release` build of `9f3ce5c` in a temporary git worktree with its own target
dir, which is the shape the harness already uses for `pgdq-before-throttle`.

**The absolute saving is the same on both 3.00 GiB files** — 0.2998 G
instructions on the control, 0.2996 G on `--arrays` — which is the check that
what left is a per-byte-of-file cost and not something shape-dependent. Only the
share differs, because the brace-bearing file spends 20× as long per byte.

**This one reaches wall time, and 7.13 did not.** The phase spec priced the
allocation's prize as *inside* the quarter-second discovery already sat within;
the copy's is not. 0.48 s → 0.40 s on tmpfs, with system time flat, is the read
path giving back what it was spending on memory bandwidth it did not need.

**RSS falls by about the chunk size.** Each loop held a `Vec` sized at
`chunk_size` (1 MiB by default, and `table_stream` allocated one per block); the
carry is a row. Nothing was designed around the 8 MB figure, but
[`measurements.md`](measurements.md)'s koji row — "RSS flat, ~9 MiB" — is now
overstated by about a megabyte. It is a figure, so it is not edited here: 7.12's
koji regression run is what re-takes it.

## The profile the phase reads from here

Eight consecutive `parse` profiles of the control, and `__memmove_avx_…` takes
no samples above the 0.5% floor in any of them. What is left is the grammar
(45.1% + 3.6% out of line) and the census (33.8% + 1.2%), with the event
callback at 7.2% and `next_event` itself at 2.6%.

**Eight, because the split between the two searchers does not reproduce.**
`memchr::One` spans 39.8–51.8% across the set and `memchr::Two` 28.8–37.8%,
anti-correlated — sample skid between two adjacent hot loops. Their *sum* is
78.7% ± 3 and is the number to reason with. A later slice quoting one of those
rows alone should take its own eight.

The `INSERT` and `query` profile tables in `architecture.md` are left as taken
and say what they are now understated by; only the row that went to nothing was
struck.

## What 7.6 and 7.7 inherit

They were ordered after this slice because they rework how a row is walked
inside the buffer it replaces. Concretely:

- **A row's bytes now usually live in the chunk's own `Bytes`**, not in a
  scanner-owned `Vec`. For 7.6's bulk `simdutf8` over a chunk's whole-row prefix
  that is the shape it wanted: the prefix is contiguous in the chunk, and the
  one row per chunk that is not is the carry, which is contiguous in the carry.
- **There is no longer a single buffer holding "everything not yet consumed".**
  A pass boundary sits between the straddling row and the rest of the chunk, so
  anything that wants to look ahead across a chunk boundary has to do it through
  `ChunkCarry` rather than by indexing one `Vec`.
- **`ScanOptions::max_line_bytes` is now checked against the carry**, which is
  the same quantity the one-buffer loop's post-`drain` length was. A change that
  makes the carry hold more than one line has to revisit that check.

## Deliberately not done

- **No change to `ByteRangeSource`.** The read-into-caller-buffer signature was
  rejected beside the mechanism at 7.13 partly because it was the only apparent
  route to deleting this copy. It was not, so the refusal is now free; the
  rejected-alternative paragraph in `architecture.md` says so.
- **No sharing of the two-pass loop across the three call sites.** Each loop's
  event body is different and one of them `yield`s, so a closure cannot carry
  it; what is shared is the state (`ChunkCarry`) and the order (`PASSES`), which
  is the part that can be got wrong.
- **No sweep.** Every figure that times a `pgdq` run already read stale on
  `scan.rs` and `stream.rs` before this slice, and a stale figure obliges no
  sweep; the wrap's pair (7.12) re-takes every table.
- **No re-take of the `INSERT` or `query` profile tables.** Both are moved only
  by the row that went to zero, and re-taking either would mean publishing a
  fresh sitting of everything else beside it.
