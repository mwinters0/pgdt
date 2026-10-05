# P30.1 notes — the instrument on mimalloc

What the next slices inherit from moving `introspect` onto mimalloc. The
mechanism is `pgdt/src/introspect.rs` and `pgdt/src/alloc.rs`; what each
instrument covers is [`measurements.md`](measurements.md), "What an instrument
can see, and what only a sitting can".

## What 30.2 inherits

- **`introspect` already names mimalloc** — `--version` prints `(allocator:
  mimalloc) (instrument: counting-allocator)` — and accepts the `mimalloc`
  feature beside it, the counter then being the global allocator; only
  `jemalloc` is refused. Making mimalloc the default leaves the instrument's
  arm as it is; what moves is `alloc.rs`'s default arm, its
  `not(feature = "introspect")` guard on the plain `MiMalloc` static, and the
  `system` leg becoming a feature of its own.
- **heaptrack sees only C on a mimalloc build.** It hooks `malloc` through
  `LD_PRELOAD`, and mimalloc is linked without `override`, so once the profiling
  build is mimalloc a recording carries `liblzma` and `aws-lc` and no Rust
  allocation. `measurements.md`'s heaptrack paragraph and `measure.py`'s
  heaptrack comment say "C and Rust alike"; both are true of the `system` build
  only, and are apparatus text 30.2 rewrites.
- **Do not run `--figure reserve` between this slice and 30.2.** Its instrument
  legs now run mimalloc while its black-box legs run the `system` default, so
  the check holding the two to one arrangement compares across allocators.

## What 30.6 and P23 inherit

- **`run_reserve` withholds its account from a two-heap report.** The account
  subtracts the counter's high-water and the decoder dictionaries from glibc's,
  which held the Rust heap only while the counter stood over glibc; a report
  carrying `mimalloc_scope` gets a paragraph saying the account is withheld, the
  counter's own line (allocator-independent) and the check beside it. Reports
  labelled `glibc_scope=whole-process`, every one under `runs/` before this
  slice, still render the one-heap account. The two-heap account is P23's
  (`roadmap.md`, "P23 — Statistics coverage and the resident reserve").
- **The readings the report gives.** `mimalloc_committed_*` and
  `mimalloc_reserved_*`, each now and at its high-water, read out of
  `mi_stats_get_json`; the document follows verbatim. On an overcommitting
  kernel mimalloc counts an arena's slices as committed when it first hands them
  out (`arena.c`, the `adjust` around `mi_reserve_os_memory_ex2`), so
  `committed` is not the arena's reservation, which `reserved` is.

## Negative results

- **A release mimalloc keeps only its OS-level statistics.** `MI_STAT` is 0
  without `MI_DEBUG` (`types.h`), so `malloc_normal`, `malloc_requested`,
  `page_committed` and the size bins are never updated (`alloc.c`, `page.c`,
  under `#if MI_STAT>0`) and read zero in the JSON. Only `committed`,
  `reserved` and the call counters are readings; the report's keys take
  nothing else.
- **mimalloc's `committed` at exit is not a release.** The smoke run
  (`runs/30.1-instrument-smoke/report.txt`, a four-reader `.xz` parse) exited
  with `committed` at its high-water: mimalloc purges after a delay, so the
  current value at exit is retention, not what the run ended holding. The same
  run's counter high-water sits below mimalloc's committed one and glibc's heap
  holds about the readers' dictionaries, which is the two-heap split the
  report's scope note states.
