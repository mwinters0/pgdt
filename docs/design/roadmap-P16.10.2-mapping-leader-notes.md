# P16.10.2 — The mapping pass runs the leader

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "The interior split" — in particular its
"The mapping pass is the leader".

## What exists now

**`stream::map_forward`'s `CopyStart` arm offers each open `COPY` region to
`leader::scan_region`.** On `Closed` the workers' census goes onto the
`Builder`, the block is closed through the same code a serial `CopyEnd` goes
through, and the serial scanner, the `ChunkCarry` and `read_pos` are put back
down at `end_offset`. On `Declined` nothing happens and the serial scanner
keeps the region. On `Cancelled` the index is saved at the last spliced
watermark and the scan reports itself interrupted.

**So `--jobs` buys a `parse` something for the first time**, and — because the
CLI's `--jobs` still defaults to available parallelism — a default
`pgdq parse` and a default `pgdq query`'s mapping pass are now parallel. `16.16`
is what takes that default back to 1 until `16.13` licenses it with a number.

## The calls worth knowing about

**`close_copy_block` is a free function because the sequence is subtle, not
because it is long.** Splice, then the settled test that reads the map the
splice just wrote, then the save, then the return — two copies of that ordering
are two orderings free to drift, and "a parallel scan's cache is byte-identical
to a serial one's" is a claim about the two paths closing a block *identically*.
It answers a three-variant `BlockClose` rather than a `MapStop`, because
`Continue` means different things to its two callers: the `CopyEnd` arm falls
through and the `CopyStart` arm has a scanner to reposition.

**The carry is discarded, not fixed up.** Whatever it held was the front edge of
a chunk the workers have since read whole, and `end_offset` is a line start, so
a fresh `ChunkCarry` is the only correct one. Trying to keep it would mean
reasoning about a partial line inside a region that has already been re-read by
somebody else.

**Repositioning happens after the event loop, not in the arm.** `event` borrows
the span, which borrows `chunk` and the carry, so `scanner` and `carry` cannot
be moved while it is alive. The arm sets a `resume_at: Option<u64>` and breaks;
the `for pass` loop breaks on it *without* calling `carry.consumed`, since the
carry is about to be thrown away; and the outer loop resumes and `continue`s.
That is why this row was its own slice — a labelled-break rework of the loop
every `parse` runs through is not the same review as a new pure module.

**The three hints are not re-announced, and that is a contract rather than an
oversight.** `scan_region` restores `WaitPolicy::NeverWait` on its way out,
announces the same `Parallelism` the enclosing loop did, and never touches the
read size. A comment at the reposition says so; if a future scheduler starts
announcing a read size of its own, that comment is the thing that has to change
with it.

**The enclosing loop's chunk is held across the region, and it is not charged.**
`map_forward` reads under `NeverWait`, so its buffer carries no charge and the
workers' ceiling is not narrowed by it — the charge riding on the buffer rather
than on the pool's current policy is what makes that come out right. It is real
memory, and it is one chunk.

**`Builder::absorb_census` takes `&[ArrayShape]`, not an `Interior`.** `map.rs`
is L1 and `leader.rs` is L4, so the L1 shape vector is what crosses. It unions
rather than replaces and it is length-tolerant, which is what a header-less
block needs: `on_copy_start` sizes the census to zero there and the workers'
union is what grows it.

**Two test wrappers gained forwarding methods, and the reason is the same in
both.** `tests/wait_policy.rs`'s `RecordingSource` and `tests/map_file.rs`'s
`CancelsPast` now forward `partitions` (and the hints), because the trait's
default declines to advise and a source that declines is never cut — a wrapper
that stopped short would have made the leader decline every region and turned
both new tests into the serial path compared to itself.

## What the next slice inherits

**The leader really is reached, and it was checked rather than assumed.** Under
`a_parallel_mapping_pass_builds_the_index_a_serial_one_does`, 360 regions are
scheduled and closed and 126 are declined; the split is asserted from outside by
`a_parallel_scan_grants_the_wait_inside_the_mapping_pass_and_takes_it_back`,
which reads the `MayWait` announcement only the scheduler makes. A `parse` of a
300 MiB generated file writes a byte-identical cache at `--jobs` 1, 4 and 24 at
the shipped chunk size. That is a smoke test, not a figure — no timing from it
may be quoted.

**`16.11` is error ordering, and the shape it needs is here.** `run_region`
dispatches a window through `futures::future::try_join_all`, so the error raised
today is the first in *partition* order among one window's reads, exactly as
`pgdq query`'s fill round is. The two now have the same defect for the same
reason, and 16.11 owns both.

**`16.12`'s determinism test has its apparatus already.**
`assert_matches_eager` plus `map_file` under a stated `Parallelism` is the
comparison; what 16.12 adds is every fixture rather than four, and the cache
bytes rather than the in-memory index.

**On a plain file the chunk pool caps effective workers at four, and that is
now a throughput ceiling rather than a curiosity.** `LocalFileSource::partitions`
answers one read chunk per partition, so `worker_count` at the 64 MiB default
affords 64 of them and `--jobs` is what binds; but `hint_parallelism` clamps the
chunk pool to `POOL_DEPTH` slots, so `slots()` is 4 at the 1 MiB chunk and a
fifth worker blocks. The wait is working exactly as designed — it is why
`16.10.1`'s test at eight jobs sees 328 blocked acquisitions — and the
consequence is that a plain-file `parse` above `--jobs 4` runs four fused
workers and queues the rest. `POOL_DEPTH` was deliberately left alone here: the
argument beside `hint_parallelism` is about a *query*'s resident set and it is
untouched by this slice, and moving the cap is a memory-accounting change that
belongs with `16.15`, which is already the row that makes one number bound both
terms. `16.13` is where it acquires a price.

**`16.13` measures a moved arrangement.** As of this slice a `parse` figure
re-taken today times a different execution, not the same one under a moved
library — the first slice of this phase for which that is true of `parse`
(`16.9` did it for `query`). `--jobs`' default is what carries it, so `16.16`
changes what is measured as much as `16.13` does; take them in that order or
say which default a figure was taken under.
