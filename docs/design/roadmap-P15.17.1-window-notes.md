# P15.17.1 notes — a speculation that holds a window

A speculation's sinks now hold a window, not the chunk (`gz-seek`'s
`native/sink.rs`). Each keeps what was written from 32 KiB before the last
place its guess may stop, and never less than the last 32 KiB. It is one
buffer whose front is dropped in place once the part released is half of it.
`Decode::push` releases at each boundary and member start. A marked copy is
made in bulk: markers for any part before the decode's start, the held values
copied as `Plain` copies them, and only the part of the source before the last
marker searched for the last marker the copy writes. `Plain` is windowed the
same way, so a decode that comes clean no longer grows toward the limit either.
Nothing outside the speculation changed: no guess, decode or index differs,
and `Chunking::task_bytes` still prices the worst case, a block as long as the
limit, which 15.18.1 settles.

**Not a ring of one window**, which the row named. A guess cut by the limit or
by refusals stops at the last boundary its decode reached, up to a block back,
and reads the 32 KiB before that. So the buffer slides, its rustdoc saying why.

- **The evidence.** `a_released_sink_holds_a_window_and_the_last_32_kib` runs a
  marked and a known-history decode over a 6 MB payload, releasing at each
  boundary. It checks that the last 32 KiB at every boundary resolve to the
  file's, and that capacity stays under a bound set by the longest block and
  the window, under half the plaintext.
  `a_marked_sink_s_last_marker_is_the_last_it_wrote` checks the bulk copy's
  marker bookkeeping against a scan of everything written, from every third
  boundary at three levels.
  `an_overlapping_copy_repeats_its_markers` covers a marker repeated by an
  overlapping copy, which the payloads never reach. Changing the repetition
  arithmetic fails only this test.

## What the probe found

A hand probe, outside the apparatus: `runs/p15.17.1/probe.sh`, run contained
over `control_gz`, with the plain parse of `control` as the floor. It ran on
two profiling builds made in the image from one target directory, HEAD's
(`pgdt-before`) and this tree's (`pgdt-after`). Each leg ran three reps under
`perf stat`, with user-mode cycles, instructions, page faults, local-cache and
DRAM fills, and L2 misses. The 1, 2 and 4 legs were then run seven reps on the
after build. Medians and per-byte rates are in `runs/p15.17.1/table.txt`
(`table.py`), the raw lines in `before/`, `after/` and `after-gate/`. Profiles
at 1, 4 and 12 jobs on the after build are in `runs/p15.17.1/profile/`. The two
500 MB binaries were deleted after the sitting.

- **The gate.** At two and four jobs the pass is level with serial, each
  median inside the spread of the seven reps. At twelve, the physical count,
  it takes about half the serial time, where HEAD's was level with serial.
  Twenty-four is a little faster again. Serial is unchanged.
- **What the rework removed.** At every count above one, instructions per
  byte, DRAM lines per byte and page faults all fell, the more so the higher
  the count. HEAD's twelve-job pass spent about twice this tree's CPU, and
  its twenty-four-job pass more.
- **Separating the slowdown**, by placement, with `taskset` on this 3900X
  (core k is CPUs k and k+12; three cores share an L3):
  - *A shared L3 matters most.* Two jobs on two cores of one CCX run as fast
    as unpinned. On two CCXs they are far slower, and so are three jobs on
    three CCXs against three on one. Twelve jobs on two CCXs' six cores and
    their siblings beat twelve cores across four CCXs.
  - *A core shared by SMT siblings* costs about as much as the split L3 at two
    jobs.
  - *The clock* falls a few percent from one busy core to all of them (cycles
    over task clock). That is small beside the rest.
  - *Memory bandwidth* binds nothing. DRAM traffic at 24 jobs is a small
    fraction of this machine's bandwidth, before the rework and after it.
- **The account at twelve jobs** (`profile-j12-flat.txt`, callers in
  `profile-j12-callers.txt`): about three fifths of the CPU is the
  speculation. That is `ChunkTask::supply`, with the native decoder inlined,
  plus the bulk copies it calls and `Input::push`. About a fifth is the
  engine's inflate, the chunk decodes and the frontier's together. A few
  percent is a chunk decode zeroing its limit-sized buffer
  (`ChunkDecode::begin`, `vec![0; limit + 1]`). A few percent is serving
  (`Pass::serve`), and a few is the scan. The workers are nearly saturated, so
  the pass is bound by the speculation's cost. A speculation still costs
  several of the engine's inflates of its chunk (`KD143`, rewritten).
  **Serving is not where the high counts are held**, so no serving increment
  is earned.

## What the next slice inherits

- **The default worker count stays the logical count.** Twenty-four jobs is
  no slower than twelve, so D2 gains that rejection. 15.7.1 times that
  default.
- **What a speculated chunk costs sets where the tasks pay**, 15.17.2's
  (`KD143`). At twelve jobs nearly every chunk is speculated, and the pass
  spends several times serial's CPU, a speculated byte costing that many
  serial ones. Wall time near the CPU over the jobs at four, eight and twelve
  puts the break-even count above four, the baseline. Below it the frontier
  decodes alone, so two and four jobs are level with serial by 15.17's
  construction, and their tasks' CPU is spent for nothing. What the CPU over
  the jobs leaves at each count is unattributed. Only a cheaper speculated
  chunk moves that count: the speculation's marked decode and copies,
  `Input::push`'s growth, and a chunk decode zeroing its buffer.
- **A probe pinned to as many CPUs as `--jobs`** oversubscribes them. The
  scan's thread is one more beside the frontier and the tasks. So the pinned
  twelve-job legs are slower than unpinned, and they compare only with each
  other.

## Negative results

- **`ls_mabresp_rmt_cache` reads zero** on this single-socket part. It does not
  separate one CCX's cache from another's, so the probe counts L2 misses
  instead. Placement is what separated the L3.
- **The probe cannot see which thread spent what.** `perf stat` counts the
  whole process. The per-thread split above is the profile's.
