# P16 — parallel scan and extraction: consolidated notes

What this phase leaves that subject-filing has no home for. What it committed
to is [`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what it
built is [`architecture.md`](architecture.md), filed by mechanism — the block
pool and its byte budget, backpressure and `WaitPolicy` under "Execution model
and API surface"; `XzSource`'s internal concurrency, its block cache and the
partitioning advisory's two answers under "The compressed source";
`table_stream_partitions` and the CLI's k-way merge under "Partitioned replay";
`leader.rs` and the mapping pass that runs it under "The interior split"; the
`tracing` output under "Status output"; and the determinism sweep under
"Testing philosophy". Each carries its own rejected alternatives and its own
limitations there.

**This doc is short on purpose.** A wrap after a keystone is an audit, not a
transcription (`../process.md`, "Consolidation at wrap"): mechanism facts went
into the architecture doc, evidence into
[`measurements.md`](measurements.md), and facts a later phase needs into that
phase's inbox. What is left is below.

## What a fixture-scale test can stop asserting without failing

Two of this phase's tests were, or could become, a configuration compared to
itself — green, fast, and asserting nothing. Neither failure is visible from a
passing run, so both are written down rather than left to be rediscovered.

**A reference leg that inherits a default is not a reference.**
`pgdump_query-cli/tests/parallelism.rs` took its reference from a `query` with
no `--jobs` at a time when that meant the machine's available parallelism, so
both sides of the assertion were partitioned runs and any *consistent*
mis-ordering would have passed. Both reference runs are pinned at `--jobs 1`
now, and `determinism.rs` states its reference the same way for the same
reason. The general form: where a test's control is "the default", the test
stops testing the moment the default moves — and the default is exactly the
thing a phase like this one moves.

**`the_lowest_offset_error_is_the_one_the_merge_raises` discriminates only at
the row counts it was built against.** The generated dump puts `zzzEARLY` at
row 18,000 and `zzzLATE` at row 22,000 because, at `--jobs 2`, the byte
midpoint of the data region falls near row 20,900 — so the two bad rows land in
different sub-streams and the early one is several batches into sub-stream 0.
The assertion (the *earlier* value is named) is correct at any split, so the
test stays green if a change to `stream::cut`/`distribute` puts both rows in
one sub-stream, or if `max_rows` rises above sub-stream 0's row count. Nothing
checks that. A change to either should re-derive the two row numbers rather
than trust a green run.

## Apparatus practice this phase paid for

**Smoke-run a new figure's command shapes before handing them to a sitting.**
`parallel-scan-throughput`'s first real run deadlocked (`M70`: a `--jobs` parse
of a block-decoding `.xz` never finished). It was found by running the figure
at `PGDQ_MEASURE_SIZE_GIB=0.2 --reps 1` into throwaway cache and warm
directories, which cost four minutes; the alternative was finding it partway
into a sitting with the readings lost. A figure whose legs are *new command
shapes* is worth one such run. The harness itself gave no misleading answer —
it hung visibly on the second reading — but it did not time out either, because
a `pgdq` deadlocked on a pool slot outlives `SIGTERM`.

**A leg that dies without a memory log leaves nothing to diagnose but its exit
code.** The koji verification's first attempt was OOM-killed and had to be
re-run from the start to find out why, because nothing was sampling the
container. The second run sampled the cgroup's `anon`, the process's
`VmRSS`/`VmHWM`, its thread count and its arena count every 60 s, which is what
attributed the kill to glibc's arenas
([`architecture.md`](architecture.md), "Execution model and API surface").

**Archive a killed run's artifacts rather than deleting them.** Doing so bought
a serial-against-serial cache comparison across two runs hours apart for
nothing ([`measurements.md`](measurements.md), "koji full scan").

**The koji verification script does not resume a killed leg.** It is a `runs/`
artifact — one machine's paths, nothing in the repo consuming it — so this is
not a defect to fix but a thing to know before re-running it: restarting it
restarts leg 1 too, which is an hour thrown away. A leg that must be stopped is
stopped with `sudo nerdctl stop`, which reaches the interrupt guard and banks
the cache, and the same command then resumes it (`../../CLAUDE.md`,
"Long-running processes").

## Filed for a later phase

Everything this phase turned up for a phase with no spec yet went into that
phase's inbox as it was found, not here. The one entry filed at the wrap is
[`roadmap-P15-gzip-inbox.md`](roadmap-P15-gzip-inbox.md), "No test in the tree
asserts that a compressed `parse` is deterministic in `--jobs`" — the claim
holds once, at koji scale, on one machine, and the phase that adds the second
decompressing source either inherits that gap or closes it.

`P19`'s inbox carries the defaults question this phase deliberately did not
settle, including whether one `max_source_span` allowance should be divided
among sub-streams rather than charged to each
([`architecture.md`](architecture.md), "Execution model and API surface").
