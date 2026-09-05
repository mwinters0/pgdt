# P7 — Scan performance: what the phase leaves behind

The phase's mechanisms are filed by subject and are **not repeated here**:
[`architecture.md`](architecture.md), "Where a scan's time goes" for the
decomposition, "The scanner never owns the bytes it scans" and "Execution model
and API surface" for the read path, "A row's bytes are validated once, in bulk"
and "Predicates" for the row walk, "Decoders and render-back" and "The nested
literal codec" for the decode and render paths, "Bulk regions" for the `INSERT`
scan, "`parse` resumes, and saves as it goes" for the splice gate, and "The
allocator is the binary's choice"; [`measurements.md`](measurements.md) for
every figure and every apparatus rule. This is a wrap after a keystone, so
consolidating the slice notes verbatim would rebuild the second authority the
keystone removed ([`../process.md`](../process.md), "A wrap after a keystone is
an audit, not a transcription").

What is left is this doc: the phase's **negative results**, which are
recoverable from nothing, and the handful of live obligations that outlive it.

## What the wrap's audit moved

Four things the slices learned had not reached the subject-filed docs, and
three of them were the `af15eac` fold-in's own consumer re-read stopping short:

- **`architecture.md`'s allocator section still carried the previous stamp's
  ranking** and the sentence "nothing beats the platform allocator anywhere",
  which the sweep falsified. The `allocator` figure names that file in
  `quoted_by`, so the re-read was owed and missed. Corrected, together with the
  census reconciliation number (+0.048 → **+0.057 s**), three of the projection
  table's per-column deltas (a `smallint` +0.13 → **+0.12**, the other fifteen
  scalars +11.03 → **+4.46**, the composite +0.98 → **+0.75**), and the
  `query-profile` section's closing paragraph, which still quoted the phase's
  opening 31× and 13.3× as current and still promised a re-take that has since
  happened.
- **The `text[]` profile input had no specification outside a slice notes
  doc.** It is the file the refused viewing builder's reopening trigger is read
  on, and it is not a registered input, so the trigger was unusable without it.
  Its specification and the reasoning for not committing a generator flag now
  sit beside the refusal itself.
- **The unclaimed 2–3 instructions a field in `RowSplit`'s memoization** — a
  preallocated `ends` written by index — were left for a slice that turned out
  to rewrite a different module. Written down beside the mechanism with the
  reason nobody took them, so the next session reaching for the `unsafe`
  version meets the cheaper safe one first.
- **"Nothing may be committed between the legs of a sweep pair"** existed only
  in a `runs/` handoff. `emit()` reads `git_head()` once per invocation and a
  pair is two invocations, so a commit between them turns the drift table into
  a measurement of that commit. Now a standing rule in `measurements.md`.

## Negative results

Every lever in the spec's table was measured and several were refused. **A
refusal is a delivered row, not a skipped one** — the spec's own rule — and
each of these cost a measurement to reach, so none of them is recoverable from
the code.

**Three levers were refused after being priced, and two of the three were the
largest prizes on the list.**

- **The viewing builder for a nested `Utf8View`** — the lever table's largest
  admitted prize, closed at **24% of its own gate** (0.237 µs/row against
  1 µs/row) on a file built to flatter it. What took it apart was the
  decomposition rather than an attempt: the bucket that made it look large was
  three `malloc`/`free` pairs a row inside the decoders `append_typed` calls,
  and removing those left 94% of what remained still in the decoders. Filed
  beside the mechanism with its reopening trigger and the input that reads it.
- **The typed column build** — `append_typed`'s Arrow appends are **52 ns a
  row**, 1.6% of a typed library row, below every instrument this campaign
  owns. Pre-sizing the builders was the one obvious candidate inside it and is
  refused at under 6 ns a row, and it would make a one-row batch allocate 8192
  slots per column: a memory regression bought with nothing.
- **`posix_fadvise(SEQUENTIAL)` and double-buffered readahead.** Both are
  bounded by the same arithmetic — no scheme that overlaps I/O with parsing can
  put a scan below the time the device takes to deliver the bytes, which on the
  fastest disk this project owns is **5.8%** of a cold `COPY` scan, and under
  1% on the SATA SSD. The chunk-size sweep is also the readahead-depth
  experiment the ceiling alone could not supply: if cold time were limited by
  how deeply the device was being asked to read ahead, the large chunks would
  be faster; they are monotonically slower from 1 MiB up, so there is no depth
  left to buy and a hint asking for more is asking for the thing that costs.
  `posix_fadvise` would additionally put a `libc`/`rustix` dependency in L1,
  which has none; double-buffered readahead is a rework of three read loops, a
  second in-flight buffer against a design built on flat single-digit-megabyte
  RSS, and a second thing for the interrupt guard and the query path's chunk
  retention to be correct about.

**Two shapes were tried and were slower, and each cost a build to find.**
Neither is a subtlety of this codebase; both are properties of `String` and
`core::fmt` a reviewer would otherwise have to take on trust. `write!(out,
"{value}")` is not `to_string` without the allocation — it reaches `Display`
through `core::fmt::write` where `i32::to_string` is specialised away from
`core::fmt` entirely, and it cost **287 instructions per array element**. And a
`String` that starts empty is grown twice by a ten-digit value, which is why
`push_padded` reserves its whole length once and `render_field`'s wrapper
starts at `String::with_capacity(16)`. Both are recorded beside the mechanism;
they are repeated here because the phase paid for them twice, once in each
direction.

**`unsafe` was reachable four times and taken none of them**, and in three of
the four the safe shape was also the faster one: `str::get` over a validated
prefix rather than `from_utf8_unchecked`; a `&'static str` hex table with
`push_str` rather than a `Vec<u8>` with `String::from_utf8_unchecked`, which
beat both alternatives while doing strictly less work; `Vec::push` and indexing
in `RowSplit` rather than a raw write; and escaping an array element in place
by shifting bytes through `String::as_mut_vec`, which is the only one where the
`unsafe` version would have been faster and it buys one allocation per array
*value*. Worth stating as a pattern: on this workload the allocation is the
cost and the bounds check is not.

**Three instruments were considered and not adopted.** `callgrind` /
`iai-callgrind` would give per-function counts immune to machine state, which
is exactly what a shared machine wants, and it is refused as the *primary*
instrument because several levers — the allocator, zero-copy views, chunk
sizing — change memory behaviour rather than instruction count, so it would
report the phase's headline work as free. Reach for it if a lever turns out to
be instruction-bound; the phase used `perf stat -e instructions:u` in exactly
that role wherever a lever was, and that deterministic reading rather than a
wall figure is what most of the landed slices are argued
on. `samply` wants `perf_event_paranoid = 1`, which is a
machine change, for a richer reader rather than an answer `perf` cannot give.
And no third allocator leg was measured: two replacements both lost, and a
third on the strength of that would be a fishing expedition — `ALLOCATOR_LEGS`
takes one by a single entry if anyone wants it.

**mmap is the intuitive I/O choice and is the wrong default**, on three
grounds that are independent of any measurement: it bypasses `ByteRangeSource`,
so it could never be the path a ranged backend takes and adopting it means
maintaining two readers; page faults on a 784 GB file on slow media are
synchronous and uninterruptible, with no way to bound prefetch depth or time
out; and I/O errors arrive as `SIGBUS` rather than `Result`, which for a tool
whose whole premise is reading a file bigger than memory is a bad trade. It
stays available as a possible local-only fast path gated on a measurement
showing it beats `pread` by enough to justify a second code path — a
measurement nobody has taken, and the wins the phase found were all inside the
existing shape.

**The HDD stays koji-only and stays a regression check.** A synthetic 3 GiB
file on a rotational disk measures one file's layout, and koji already answers
the only HDD question the design has: the scan is device-bound there, so no
default this phase picked changes anything. Adding an HDD throughput figure to
the sweep would cost minutes per sweep to re-confirm a bound two 54-minute runs
already agree on to within 1%.

**The chunk-size default is one measured constant and not a runtime probe.**
Choosing it from `/sys/block/<dev>/queue/rotational` is Linux-only and degrades
exactly where this tool runs — `/sys` may be masked inside a container, and on
LVM, dm-crypt, MD, NFS or an overlay, resolving a path to its backing device is
a walk with several ways to be wrong. The sweep then made the question moot:
1 MiB is the fastest row of the swept range, its neighbours are ties, and every
row is 1.00× on the SATA SSD, so the lever is worth **nothing** rather than
"at most 8.9%", and there is no spread for adaptivity to chase.

## What the phase did not settle, and left as a bound

**No device faster than the 970 EVO Plus was measured.** The 5.8% I/O ceiling
is a fact about the fastest disk we own, and a device on which parse CPU
exceeded read time would reopen `fadvise` and readahead at once. That is not a
deficiency and has no owner: it is a bound stated with its apparatus, and what
would promote it is hardware rather than a decision.

**`Mode::Statement` still accumulates a `String` and re-walks it per line.**
The `INSERT` fast path took that cost off `Mode::InsertRun`, where it was
gigabytes; a statement span carries its text because `classify_statement` and
`extract_statement_cross_refs` read it, so the buffer cannot simply go, and the
per-line re-walk could be made incremental with a second `StatementScan` beside
it. It is a few megabytes of DDL in a file of gigabytes.

**A shared split cannot cross into the mapping pass.** The census's own field
split runs in `map_forward` and the predicate's and batcher's in the replay,
across the hard boundary "Query: mapping and streaming are separate passes"
describes; sharing one split across it *is* the interleaved form that section
records as deliberately deferred. The census's re-split was made cheap instead,
which is where the six-fold `parse` improvement on brace-bearing files came
from — a change aimed at the query path that moved a `parse` figure.

## Live obligations

- **`KD5`, `KD9` and `KD13`** are the register entries this phase touched.
  `KD5` is rewritten to its `--dqcache none` residual and re-homed onto the
  parallel-scan phase; `KD9` is rewritten to a measured residual — 4.9× a
  `COPY` scan's per-byte CPU warm, 2.62× the device's own time cold on NVMe —
  and stays owned by the format-coverage phase, whose row reader extends the
  very scan both remaining cuts are in. Neither is struck.
- **The inboxes are filed and not repeated here.** The parallel-scan phase's
  carries this phase's answers on coverage and on the census; the
  format-coverage phase's carries the reusable statement-end primitive; the
  remote-input phase's carries the chunk-size finding; the embeddable-engine
  phase's carries the per-row budget and why no library-only figure exists.
- **`M52`, `M53`, `M55`, `M56`, `M57` and `M58`** are admitted and unlanded in
  the out-of-band ledger. None blocks anything.
