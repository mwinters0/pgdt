# P30.2 notes — the allocator move

What the later slices inherit from making mimalloc `pgdt`'s default. The
mechanism is `pgdt/src/alloc.rs` and `pgdt/Cargo.toml`'s `[features]`; why it
is mimalloc is [`decisions.md`](decisions.md), "D13". Nothing was re-taken
here; [`measurements.md`](measurements.md)'s session stamp names the sitting
that took the figures under mimalloc.

## What 30.3 to 30.5 inherit

- **A build names exactly one allocator.** `default = ["mimalloc"]`;
  `system` and `jemalloc` are features of their own, and any two of the three,
  or none, is a `compile_error!`. `introspect` counts as mimalloc: it stands
  beside the default and is refused beside `system` or `jemalloc`. So a leg is
  built `--no-default-features --features <leg>`, and a featureless
  `--no-default-features` build does not compile — upstream's benchmarks read
  that build as the platform allocator; here it would be the `system` leg under
  a name nobody chose.
- **`datafusion-cli-pgdump` declares its own `#[global_allocator]`** in
  `src/main.rs`. When it becomes a library that `pgdt` links, that static has
  to stay with its binary, or the composed `pgdt` has two global allocators —
  which `rustc` refuses, and which `alloc.rs`'s refusals do not see.
- **`ALLOCATOR_LEGS[0]` is the default build's**, held to `pgdt/Cargo.toml`'s
  `default` by `test_measure.py`'s `test_the_first_leg_is_the_default_build_s`.
  A dry run names it as the reference; a real sitting reads the reference off
  the binary as before.

## What 30.6 and P23 inherit

- **30.1's warning against running `--figure reserve` is discharged**: the
  instrument legs and the black-box legs are both mimalloc again.
- **`rss-attribution`'s extra legs are `system` and `jemalloc` now**, the
  shipped binary being mimalloc. Its published table carries the old legs'
  readings under the old keys, so it is replaced whole at the next re-take, as
  the `allocator` table is, whose reference column becomes `mimalloc`.
- **The gate's `system` legs are not built here.** `ensure_allocator_binary`
  builds a `system` binary; running `reserve`'s flagless legs and the
  `parallel-*` contract on it is 30.6's.
- **A two-heap `reserve` sitting prints its readings**: one table, a column a
  reading, each headed with the memory it covers and none subtracted, beside
  the counter's own line; the one-heap account stays withheld.
- **The mechanism leg's premise does not reach the Rust heap.** The leg exists
  to read glibc's dynamic mmap threshold retaining a block buffer in every
  decoding thread's arena; on the default build the block buffers are
  mimalloc's, so `MALLOC_ARENA_MAX` reaches only what C allocates. The
  rendered paragraph says so on a two-heap sitting and the leg is unchanged;
  whether it still earns its place is P23's, its sketch in `roadmap.md` says.

## Negative results

- **heaptrack sees C's allocations alone on the default build** — `liblzma`'s
  and `aws-lc`'s, Rust frames appearing only as their callers — because
  mimalloc is linked without `override`. The recipe still builds the default
  `profiling` binary, and its text now says what that sees; a recording of
  Rust's own allocations is a `system` build's.
- **The manual's `MALLOC_ARENA_MAX` advice no longer stands as measured.** Its
  saving was read with the Rust heap on glibc; on the default build the cap
  bounds only the decoder's arenas, and what it saves there is what the
  `reserve` figure's arena legs read on the shipped build, which
  `docs/manual/dump-inspection.md` states.
