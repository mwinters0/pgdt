# P15.7.2 notes — the record

**The manual and the compatibility matrix now say what P15 built.**
[`../manual/dump-inspection.md`](../manual/dump-inspection.md) gains "gzip
files: the first pass builds the index". It covers the first pass and its
lines, its one-reader scan beside the workers that inflate ahead, the default
count, and which writers' files those workers lift. It also covers the resume,
the open size, where the windows spill, the query's note, the warning, the
BGZF walk, and the `--detail` line. "Reading a dump over HTTP" gains gzip and
BGZF, and `--jobs`, "Status on stderr", `scan arrangement`'s `bound_by` and
`--json`'s `compression` say what is gzip's. In
[`pg-dump-compatibility.md`](pg-dump-compatibility.md), the `.gz` row is split
from `.zst`/`.lz4` and reads `Tested`. The option-register row no longer says
nothing reads `--compress`'s output. [`../manual/pgdt-sql.md`](../manual/pgdt-sql.md)
names gzip as a source.

## What the wrap inherits

- **The README's compression line** (`gzip (non-seekable) / bgzip (seekable)`)
  is the wrap's to tick, and its wording is wrong for what shipped: a single
  member is seekable once its first pass has built the index.
- **No throughput number is in the record.** The manual says only that a
  level-6 writer's file is lifted "several times" and a level-1 writer's is
  not. The figures stay in [`measurements.md`](measurements.md), "What a
  parallel gzip first pass buys". The manual quotes one charge, about 109 MiB
  a first-pass worker, read off `Chunking::task_bytes` as `resolved the
  arrangement` states it (114,115,162 bytes at `--jobs 1`).
- **The stderr examples were captured** by a contained hand run of a
  release-image build of the crates as they stand, unchanged since
  `fff4da1b`, over `control_gz` and `control_bgzf`. Only the paths are renamed
  and some lines left out. The gzip warning's example is the exception: it is
  built from the message's format, with no file written to raise it.

## Negative results

- **`scan started` on a fresh first pass reads `bytes=at least 0`**, the open
  extent before anything is decoded. The manual explains it and does not
  change it.
- **A `parse` whose `--dtcache` names a read-only directory fails naming no
  path** (`KD151`), before the first pass's own refusal. That refusal names
  its directory, and it is what the manual quotes.
