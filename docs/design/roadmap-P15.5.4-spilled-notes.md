# P15.5.4 notes — single-member gzip, the first pass's windows spilled

A gzip first pass holds none of the windows it places (D115, RT24; `KD130`
struck).

- **`deflate/first_pass/spill.rs` is where they go**, `PassWindows`: each
  window is written at the running sum of those before it to a file
  `tempfile::tempfile_in` makes with no name, and read back with
  `read_exact_at`, as `pgdt-dump`'s section is; one `(offset, length)` pair a
  checkpoint is resident. The file is made at the first window holding bytes,
  so a pass that places none — a file under the spacing, a pass resumed into
  its last unit, an `info` or a registration that builds a pass and reads
  nothing — makes none and cannot be refused for it. A save is unchanged: it
  reads every window through the saved index's provider, which is this one.
- **`pgdt_io::open` takes the directory**, its third argument, passed on to
  `FirstPass`'s constructors; `CacheMode::open` names the enabled cache's
  parent, `.` for a path with none, or `std::env::temp_dir()` under a
  disabled cache (`spill_dir`, private: `open_source`'s caller built its
  source itself).
- **A spill file that cannot be made fails the read that places the window**,
  an `Error::Io` keeping the OS's kind and naming the directory; a pass
  stopped by it does not continue, as after any frontier failure.
- **The pass's charge names its skip scratch**: `default_worker_memory` is
  `PASS_BYTES`, both decodes and `SKIP_SCRATCH`, and the one partition's term
  one decode and the scratch.
- **The evidence**: `first_pass/tests.rs`'s
  `a_first_pass_spills_its_windows_where_it_is_told` (both feeds: the spill
  directory empty halfway and at the end while every window reads back as
  `gz_seek`'s pass placed it; a file under the spacing read whole over a
  directory that does not exist, and a larger one refused there naming it);
  `cache/open/tests.rs`'s
  `a_first_pass_spills_beside_its_cache_or_in_the_temp_directory`; every
  earlier first-pass, determinism and remote test, now reading windows back
  out of the file.

## What later slices inherit

- **15.8's speculative units place windows through `PassWindows::place`**,
  which takes them in checkpoint order (`debug_assert`): a parallel pass
  placing out of order needs the offsets keyed by checkpoint rather than
  pushed.
- **15.7's manual** says where a first pass writes, beside `--dtcache` and
  `TMPDIR`, in `dump-inspection.md`; nothing user-facing says it yet.
- **15.7's resident readings** read a pass holding no window, so a first
  pass's resident is its decodes, its scratch and its buffer pool.

## Negative results

- **A file made eagerly at open** was not taken: `info` and the DataFusion
  provider open a first pass over an uncached or partial gzip file before
  refusing to scan it, and a cache directory they cannot write would have
  turned "no cache at …" into a spill refusal.
