# P7.13 — The read path's per-chunk allocation

What the rest of P7 inherits: the zeroing half of "who owns the bytes between
the kernel and the scanner" is gone, the copy half is not and is now `7.13.1`,
and the `allocator` figure's ranking changed shape once the allocation it was
really measuring was removed. The mechanism and its rejected alternatives are
filed by subject — [`architecture.md`](architecture.md), "Execution model and
API surface" for the pool, "`parse`: half the wall is the kernel, and half of
what is left is copying" for what it was worth, and "The allocator is the
binary's choice" for what the re-take says. This doc holds the apparatus, the
split, and one harness defect that would have published the wrong table.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/io.rs`, `BufferPool` / `PooledBuffer` | the whole mechanism: a four-slot free list on `LocalFileSource`, an owner that returns its buffer on drop, and an 8 MiB ceiling on what is kept |
| `pgdump_query/src/io.rs`, `LocalFileSource::read_range` | takes a pooled buffer, reads into it, returns `Bytes::from_owner(…).slice(..len)` |
| `pgdump_query/src/io.rs`, `mod tests` | four: the recycled-buffer hazard, retention under a live reference, the ceiling, and the bound plus smallest-fit |
| `scripts/measure.py`, `READ` | `pgdump_query/src/io.rs` as its own staleness mechanism, added to the twelve figures that time a `pgdq` run over a file |
| `scripts/measure.py`, `_ALLOC_BUILT` / `ensure_allocator_binary` / `run_allocator` | the leg cache is per-process, not per-machine, and every leg is built before the first reading |

No scanner, map or batch code changed. `ByteRangeSource` is untouched.

## What it was worth, and the instrument that says so

**A profile is a proportion and this machine was not quiet**, so the load-bearing
evidence is the deterministic one: user instructions per warm 3.00 GiB `parse`
fall **1,882,404,237 → 1,705,902,670**, −9.4%, each side at ±0.00% over five
`perf stat -r 5` reps. Host user time falls 0.23 s → 0.18 s in a quiet window,
and `__memset_avx2_unaligned_erms` — 23.2% of the old `parse` profile — takes no
samples above the 0.5% floor in any of three fresh ones.

**Wall time barely moves and that is the honest headline.** A warm host `parse`
was 0.55 s and is 0.54 s: the prize was inside the quarter-second the phase spec
already priced discovery at, which is what that spec's "its prize is inside the
quarter-second above, not additional to it" said it would be. What the change
buys is not a faster `parse` on tmpfs; it is 9.4% of the CPU a scan spends, and
the removal of the syscall storm below.

## The allocator question is closed, and both readings that kept it open were this

`--figure allocator`, re-taken here: `jemalloc` **1.02× / 1.11× / 1.06×** and
`mimalloc` **1.00× / 0.99× / 1.01×** over `parse`, a `strings` query and a typed
one. Nothing beats the platform allocator anywhere, so **it stays** and the
deadline 7.3 set — settle before the wrap's sweep pair — is discharged rather
than deferred a second time.

The two cells that moved are the two the decision hung on, and both were this
slice's allocation rather than an allocator:

- `jemalloc`'s `parse` was **1.87×**, all of it system time from 3,161
  `madvise` calls against glibc's 50 — the per-chunk buffer handed back to the
  kernel once per chunk. It is 1.02×.
- `mimalloc`'s `typed` was **0.96×** in both of 7.3's sittings, on
  non-overlapping spreads, and was the whole of the case for adopting. It is
  1.01×, its spread now *above* the reference's.

7.3's notes said the post-7.13 ranking "is the one that should decide
adoption", and it decided it the other way from where the argument had got to.
The figure and the reasoning are filed by subject —
[`measurements.md`](measurements.md), "Which allocator a figure was taken
under", and [`architecture.md`](architecture.md), "The allocator is the binary's
choice".

**The sitting is a partial one and the table says so.** The harness's own note
asks that `census-brace-free` and `nested-end-to-end` be emitted with it, so
the reference column is shared rather than self-measured. They were not:
`census-brace-free`'s readings are in turn shared with both throughput tables,
so the honest set is five figures, which is most of a sweep and belongs to the
wrap. What that costs is only what the table already said of every sitting of
itself — its absolutes may not be set beside another table's — and nothing at
all to its ratios.

## The harness would have published the wrong table, and did once

`ensure_allocator_binary` short-circuited on `runs/pgdq-alloc-<leg>` existing.
That file survives between sessions, so the first re-take here timed **last
session's** jemalloc and mimalloc binaries against **this** session's reference
— a comparison of two different sources, reported as a comparison of two
allocators. It is silent by construction: a stale leg still answers `--version`
with its own allocator name, which is the check that was in place.

Two things followed. The cache is now keyed on `_ALLOC_BUILT`, a **per-process**
set, so every run of the harness rebuilds each leg once from the current source;
`cargo` is incremental, so a leg whose source has not moved costs about five
seconds. And the builds moved to the top of `run_allocator`, ahead of the first
reading, because a `cargo` build across 24 cores moves the very number the next
rep takes — the same rule that keeps a sweep off a busy machine.

The tell, for a later session: the log line `building the <leg> allocator leg
into …` must appear once per non-reference leg *before* `rep1`. A run that goes
straight to `rep1` is timing binaries it did not build.

## `io.rs` was invisible to `--stale`, and it is the read path

No figure declared `pgdump_query/src/io.rs`. A change to the largest single term
in a warm `parse`'s user time would have read green against every table it
moved. It is declared as its own mechanism (`READ`) rather than folded into
`SCAN`, because `nested-end-to-end` and `census-attribution` declare no scanner
path and are still moved by it.

## What 7.13.1 inherits

**The seam is where the confidence changes.** The row this slice came from
paired a contained change to one module with a rework of three already-tested
scan loops — `scan::scan`, `stream::map_forward`, and the replay loop in
`stream::table_stream` — each with its own `drain(..used)` carry. That is two
review cycles, and [`../process.md`](../process.md)'s own sizing rule names this
exact pairing. `7.13.1` is the second.

Three facts it starts from rather than re-deriving:

- **The chunk copy is 38.0% of a warm `parse`'s user time**, now that the
  zeroing is gone, and the largest single term left. On the `--arrays
  --composite` file it is 5.7% against `map::Builder::on_row`'s 85.6%, so the
  prize is a property of the brace-free shape.
- **The pool makes the chunk's buffer outlive the read**, which is what a
  design that scans the chunk in place needs: `read_range`'s `Bytes` is already
  the thing `batch::SourceChunk` retains for zero-copy views, so a loop that
  stops copying is not also introducing a lifetime problem.
- **The carry is the whole difficulty.** A line straddling a chunk boundary is
  why each loop keeps a buffer at all. Copying only `carry ++ chunk[..=first_lf]`
  and then scanning the rest of the chunk in place is the shape that removes the
  megabyte without touching `CopyScanner`, whose `base` is absolute and so
  survives being handed two different buffers in one iteration. The fallback —
  a chunk with no LF in it — is the existing behaviour.

## Deliberately not done

- **No change to `ByteRangeSource`.** A read-into-caller-buffer signature would
  delete the scanner's copy at the same time, and it is rejected beside the
  mechanism: it departs from `get_range` where the trait exists to mirror it,
  and `spawn_blocking` cannot borrow, so it would be this pooling protocol with
  the pool moved into every caller.
- **No re-derivation of the `query` profile table.** Only its `read_range`
  thread row is falsified, and that row goes to nothing; re-taking the other
  rows would have meant publishing shares from a sitting this machine was too
  busy to give. Removing that thread's samples re-bases every share upward by
  its own share and moves no absolute, so the per-row budget an embedder pays is
  unchanged.
- **No sweep.** Twelve figures now declare `io.rs` and read stale on it. A
  stale figure obliges no sweep, and the wrap's pair re-takes every table.
- **No `posix_fadvise`, readahead or chunk-size change.** Those are 7.8's, and
  they are measured against a figure this slice does not take.
