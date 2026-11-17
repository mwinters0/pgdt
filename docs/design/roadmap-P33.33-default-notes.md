# P33.33 notes — the purge delay the binary's default

**`pgdt` runs mimalloc at a 100 ms purge delay unless `MIMALLOC_PURGE_DELAY`
states one**, 33.32's choice (`roadmap-P33.32-purge-notes.md`, "The choice"),
set in the binary alone (`decisions.md`, "D13"). No figure is taken here; 33.7
re-takes `long-line-resident` and its hand runs under it.

- **How it is set**: `alloc::configure`, the first call in `main`, gives
  mimalloc the default by `mi_option_set_default`, which leaves an option the
  environment set as the process loaded in place. It is the option the
  `MIMALLOC_PURGE_DELAY=100` legs set, read afresh by every purge mimalloc
  schedules (v3's `arena.c`, `mi_arena_purge_delay`), so 33.32's 100 ms legs
  ran what ships now. Under `introspect` too; `system` and `jemalloc` set
  nothing. The two calls are the shipped binary's `unsafe` site, so `main.rs`
  no longer forbids it (D27).
- **How the default is read**: `pgdt --version` names the delay mimalloc runs
  under, read back from it once `configure` has run — ` (purge delay: 100 ms)`
  after the allocator's marker, or what the environment stated — and
  `measure.py`'s `binary_purge_delay` reads that. `MIMALLOC_VERBOSE=1`, which
  33.32 asked, prints mimalloc's options as the process loads, before `main`,
  so it names mimalloc's own 1000 ms on the shipped binary.
- **The option's index is counted by hand**: `libmimalloc-sys` 0.1.49 declares
  no constant for `mi_option_purge_delay`, so `MI_OPTION_PURGE_DELAY` is its
  place in `mimalloc.h`'s `mi_option_e`, 15. `tests/allocator.rs` holds it to
  mimalloc's own reading of the variable: under a stated delay `--version`
  must name it, which an index naming another option cannot; it fails at 16.
  The block tracker's test now takes the same constant.
- **The manual says it** in the allocator callout under "`--jobs` and
  `--memory`": the default, the variable, `--version`'s marker, and that a
  longer delay can outgrow a container sized at `--memory`.

## What the next slice inherits

- **33.7's re-takes run under the default**: the `long-line-resident` sitting
  and both hand runs, the strict one included, every pass's resident moving
  with it. A run stating nothing now reads what 33.32's 100 ms leg read.
- **`purge-delay`'s and `purge-delay-resident`'s shipped leg now runs at
  100 ms**, the value `PURGE_DELAY_LEGS`' last leg states, so a re-take reads
  that delay twice and mimalloc's 1000 ms in no column. The legs are left as
  33.32 drew them; a re-take that wants the old default priced adds it as a
  stating leg. Their published tables are `7860c5fb`'s and say the shipped leg
  ran at 1000 ms.

## Negative results

- **No compile-time default**: v3's option table hard-codes 1000 ms with no
  define to override it (`options.c`, `mi_options`), and `libmimalloc-sys`'s
  build passes none.
- **No safe route**: the `mimalloc` crate exposes no option, and setting the
  variable from `main` comes after mimalloc read it, so the binary's first
  `unsafe` outside the instrument is two option calls.
