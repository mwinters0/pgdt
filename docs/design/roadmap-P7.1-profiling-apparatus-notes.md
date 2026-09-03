# P7.1 — The profiling apparatus

What 7.2 inherits: a profile whose every frame is named, the instrument's own
floor measured rather than assumed, and six first profiles to read the
decomposition out of. The apparatus's reasoning is in the harness beside the
code — `scripts/measure.py`, `profile_recipe` — and this doc holds only what
the next slice needs and what is not recoverable from the tree.

**No library code changed, and no figure was taken.** A profile is a `runs/`
artifact: no medians, no apparatus gate, no `measurements.md` marker
([`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md), "How this
phase measures").

## Module map

| File | What it is |
|---|---|
| `Cargo.toml`, `[profile.profiling]` | inherits `release`, adds line tables, keeps symbols. `release` is untouched, which is what keeps every published figure comparable |
| `scripts/measure.py`, `profile_recipe` / `cmd_profile` | the whole sequence with paths filled in, printed and never run — the `--koji-recipe` precedent |
| `scripts/measure.py`, `profile_argv` | the `pgdq` flags one profiled shape runs |
| `scripts/test_measure.py`, `ProfileRecipe` | ten assertions, each of them a way to get a plausible profile of the wrong thing |
| `CLAUDE.local.md`, "The profiler on this machine" | `perf` 7.2.2-1, `perf_event_paranoid = 2`, the debuginfod URL, and where profiles land |
| `runs/profile-<shape>-<input>.{data,txt}` | the six first profiles |

## The recipe is printed for the opposite reason koji's is

koji's invocation is owned by the harness because it costs an hour and three
hand-maintained copies had drifted. This one is owned because a profile is
**not** a figure and must not become one: the moment the harness *runs* it,
somebody wants reps, a median and a marker, and the phase is back to spending a
quiet machine on questions a proportion already answers.

`--profile-recipe` therefore prints and returns. What holds the two halves
together is `test_measure.py`'s
`test_a_profiled_shape_is_the_shape_the_sweep_times`, which reconciles
`profile_argv` against `_script` shape by shape: they cannot be one function —
`_script` builds a container command line with a `time` builtin in front — but a
flag that moves in one and not the other gives a profile of something no figure
measures, and nothing else would notice.

## Five ways to get a plausible profile of the wrong thing

Each fails *silently*: a profile comes back, it just describes something else.
All five are in `profile_recipe`'s docstring — the `profiling` binary rather
than `release`, `-C force-frame-pointers=yes` on the build line,
`--call-graph fp` matching it, a warm input, and libc's symbols. The last is the
one that was actually costing us the answer.

## The libc frames were the apparatus's real defect

This machine's `/usr/lib/libc.so.6` is stripped and its distribution ships no
debug package, so `perf` reported the hottest part of a warm `parse` as a column
of bare addresses. Named, they are `__memmove_avx_unaligned_erms` and
`__memset_avx2_unaligned_erms` — **together ~48% of that profile**, and the
exact pair a phase about zero-copy exists to see. An apparatus that hides them
would have sent 7.2 to write a decomposition out of the third-largest bucket.

This `perf` links `libdebuginfod` and exposes no flag for it (`perf
--debuginfod=…` is rejected by its option parser, and `DEBUGINFOD_URLS` in the
environment is ignored), so the recipe fetches the debuginfo itself into
`~/.debug/<dso>/<build-id>/debug`, which is `perf`'s own build-id cache: no
root, no package, and undone by `rm -r ~/.debug`. `PGDQ_PROFILE_DEBUGINFOD` set
empty skips the step, which is right on a machine whose libc already carries
symbols.

## The instrument's own floor, measured

Three reps of `parse` over the control, at two frequencies, self-overhead per
bucket:

| Bucket | 3 reps at 997 Hz | 3 reps at 4999 Hz |
|---|---|---|
| `__memmove_avx_unaligned_erms` | 25.8 / 33.6 / 30.9 | 30.9 / 30.6 / 30.2 |
| `__memset_avx2_unaligned_erms` (worker thread) | 24.9 / 17.5 / 26.9 | 29.2 / 21.3 / 18.9 |
| `memchr::One::find_raw_avx2` | 16.9 / 24.3 / 15.6 | 15.1 / 18.5 / 19.7 |
| `memchr::Two::find_raw_avx2` | 18.6 / 14.9 / 12.5 | 15.2 / 17.9 / 18.6 |

`parse` is the thin shape — ~0.85 s warm, so 997 Hz yields ~850 samples, and at
that count the two `memchr` kernels swapped rank between reps. `PERF_FREQ` is
therefore **4999**, which is measured rather than preferred, and the kernel's own
ceiling here is `perf_event_max_sample_rate` = 49000.

**What does not tighten is the worker thread's `memset`**, and that is the
reading 7.2 needs most: a ten-point spread that four times the samples does not
touch is the *workload*, not the instrument. Read it as "the chunk zeroing is a
fifth to a quarter of a `parse`", never as a number.

The three long shapes need none of this — `query` yields 20K–100K samples — so
one run per shape is what the recipe takes. Nothing here licenses reading a
five-point difference between two profiles as a result.

## What the six first profiles say

Single runs, self-overhead, `--percent-limit 0.5`, warm on tmpfs. **This is not
the decomposition** — 7.2 owes `architecture.md` that, and owes it a reading of
the call graphs these files carry rather than of the flat table below.

`__memset_avx2_unaligned_erms` is the one bucket outside the `pgdq` thread —
`perf` attributes it to `tokio-rt-worker`. Everything else below is `pgdq`
itself.

**`parse`, control (4K samples) / arrays (9K):**

| control | arrays | symbol |
|---|---|---|
| 29.4% | 5.6% | `__memmove_avx_unaligned_erms` |
| 20.7% | 3.8% | `memchr::One::find_raw_avx2` |
| 18.4% | 3.1% | `memchr::Two::find_raw_avx2` |
| 18.3% | 3.8% | `__memset_avx2_unaligned_erms` |
| 5.2% | 0.7% | `pgdq::main::{closure#0}` |
| 0.6% | **81.4%** | `map::Builder::on_row` |

**`query --schema-mode strings`, control (26K) / arrays (31K):**

| control | arrays | symbol |
|---|---|---|
| 22.5% | 14.6% | `copy::decode_field` |
| 13.9% | 11.0% | `__memmove_avx_unaligned_erms` |
| 7.0% | 5.4% | `__memset_avx2_unaligned_erms` |
| 6.5% | 5.6% | `core::str::converts::from_utf8` |
| 5.8% | 5.2% | `core::slice::memchr::memchr_aligned` |
| 5.1% | 4.3% | `GenericByteViewArray<StringViewType>::value` |
| 4.8% | 4.3% | `memchr::One::find_raw_avx2` |
| 2.2% | 2.3% | `batch::RowBatcher::push_row` |
| 2.0% | 1.4% | `batch::render_field` |
| — | 21.2% | `map::Builder::on_row` |

**`query --schema-mode typed`, control (58K) / arrays (98K)** — a long tail,
nothing above 8%:

| control | arrays | symbol |
|---|---|---|
| 7.9% | 3.7% | `copy::decode_field` |
| 6.3% | 2.9% | `alloc::fmt::format::format_inner` |
| 6.2% | 4.9% | `__memmove_avx_unaligned_erms` |
| 5.7% | 2.3% | `core::fmt::write` |
| 3.6% | — | `decode::decode_bytea` |
| 2.6% | 4.7% | `_int_malloc` |
| 2.6% | 3.3% | `malloc` |
| — | 8.8% | `nested::needs_quote` |
| — | 5.2% | `map::Builder::on_row` |
| — | 4.1% | `batch::append_typed` |
| — | 3.3% | `nested::scan_token` |

Three things are visible from the flat table alone, and all three are
hypotheses until 7.2 reads the call graphs:

- **Nearly half of a warm `parse` on the control is `memmove` plus `memset`** —
  copying and zeroing, not grammar. The spec's baseline puts everything the
  `COPY` grammar, the map and the census do together at 36% of `parse`'s own
  elapsed time; this says where a good part of the rest is.
- **`decode_field` is the single largest `strings` bucket** and stays largest
  when arrays are added, which is the shared row machinery rather than the
  nested path.
- **`typed` has no dominant bucket.** Formatting (`format_inner` + `write` +
  `pad_integral` ≈ 15% on the control) and the allocator (`malloc` +
  `_int_malloc` + `cfree` ≈ 8%) are each larger than any single decoder — which
  is the first evidence for the allocator lever, taken before it is measured.

## What 7.2 should know before it reads them

- **`--dqcache none` means the query profiles include the mapping pass**, which
  is what the `query-strings` / `query-typed` figures time too. So `map::Builder`
  appearing in a *query* profile is the map being rebuilt, not a query-path
  cost, and the split between the two is a subtraction 7.2 has to make
  deliberately.
- **The `INSERT` +10% differential profile is not reachable from this recipe.**
  It needs a second binary built at `b70589f` and a profile of `insert_run.sql`,
  and neither the recipe's input list nor `ensure_before_binary` (which builds
  `BEFORE_COMMIT`, a different commit) provides one. That is 7.2's own
  apparatus work, and it is small: a git worktree, one build, and two
  `perf record`s diffed by function.
- **A profile of the `arrays` file's `parse` is a profile of the census.** The
  brace-free control is where the "census costs ~8% of a warm scan" figure comes
  from; on a brace-bearing file the same pass is the overwhelming majority of
  the scan. Both readings are true and they are about different inputs.

## Deliberately not done

- **No reps and no median in the recipe.** A profile is read for a proportion;
  giving it a median would make it a figure with none of a figure's apparatus.
  The floor above is the answer to "how solid is one profile", and it is
  measured once rather than re-taken every time.
- **`samply` was not adopted.** It wants `perf_event_paranoid = 1`, which is a
  machine change, and the spec named it as optional for a richer reader rather
  than for an answer `perf` cannot give.
- **`callgrind` stays where the spec left it** — available for a lever that
  turns out to be instruction-bound, and rejected as the default because the
  allocator, zero-copy views and chunk sizing change memory behaviour rather
  than instruction count. The `memmove`/`memset` pair above is exactly that
  case, and it is the phase's largest `parse` bucket.

## One thing the landing commit owes

`session-drift` declares `scripts/measure.py`, so this slice turns it red. The
mechanical oracle applies — **reachability**: `--profile-recipe` is a new
subcommand, no sweep command shape executes it, and nothing on a timed path
changed. An `acknowledged.py` entry cannot name its own sha, so it lands as a
follow-up exactly as `175f83e` did for `7545dc6`.
