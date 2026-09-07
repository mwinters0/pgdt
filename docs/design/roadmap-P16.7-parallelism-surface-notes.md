# P16.7 — The `Parallelism` value

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "Execution model and API surface".

## What exists now

**A two-state enum in L1 (`io.rs`), and two option fields that carry it.**
`Parallelism` is `Serial` or `Workers { jobs: NonZeroUsize, memory_bytes: u64 }`,
`Default` is `Serial`, and `ScanOptions::parallelism` and
`QueryOptions::parallelism` both carry it. Nothing reads either field.

**One worker is spelled `Serial`, and that is the whole point of the
constructor.** `Parallelism::workers(jobs, memory_bytes)` answers `Serial` for a
`jobs` of one or zero, so no `Workers` of one exists to be told apart from the
serial path. Zero reading as one mirrors `xz_seek::Bulk::new`, whose reason
carries over unchanged: it is a shape a caller's own arithmetic produces, and an
error would only make them do that arithmetic twice. The consequence worth
knowing is that `--jobs 1` is the serial path as a property of the *value*, so
the CLI does not have to arrange it.

**It is in `io.rs` rather than beside either options struct.** Both structs are
above L1 or in it and may name it; the two mechanisms that will read it — the
buffer pools' budget and the block-decode decision — are `io.rs`'s, and
`Partitioning` (the *source's* half of the same question) is already there.

## The calls worth knowing about

**`jobs` is `NonZeroUsize`, so no clamp is remembered anywhere.** The
alternative — `usize` with the pool clamping at use time — puts the same rule in
every consumer and makes a zero that reached a scheduler a runtime question
rather than an unrepresentable one. The one place a `usize` is accepted is the
constructor, which is where a caller's arithmetic arrives.

**Neither field is defaulted inside `Workers`.** A worker count derived from the
machine would be this library making the caller's device-awareness decision from
underneath them, and a byte budget is the only one of the two that can be
promised to a cgroup — both arguments are `xz_seek::Bulk`'s and both hold here.

**`Copy`, not just `Clone`.** The value is two words and is read on the hot
path's cold side (once per scan, once per pool sizing); `ScanOptions` is
`Clone`-only because of the `Arc<AtomicBool>` beside it, which is unrelated.

## What the next slice inherits

**`16.7.1` is the rest of this row, and it is where the design decision is.**
`POOL_BUDGET_BYTES` is still a constant, the block pool's depth is still
`POOL_DEPTH`, and `BLOCK_DECODE_MAX_BYTES` is still a second constant beside
them. Making the caller's number decide all three turns on a choice with two
defensible answers, and neither is free:

- **Default budget stays at 64 MiB**, the cap becomes the budget, and every slot
  count in the tree is unchanged — at the price that a file whose largest block
  is between 64 and 256 MiB now falls back to the streaming reader instead of
  block-decoding. That is the budget being *honoured* rather than a regression,
  and `xz-seek` endorses the fallback in as many words
  (`vendor/xz-seek/src/plan.rs`: a budget too small for one worker "clamps to
  one and reports", so the caller "can fall back to its own streaming decoder").
  What it costs is a `query` over `koji-…blocks128.xz` under the default; the
  remedy is `--parallel-memory`, which is in the user's hands.
- **Default budget rises to 256 MiB**, the cap is unchanged in effect, and no
  file changes read path — at the price that the block pool's slot count goes
  from 2 to 4 on a 24 MiB-block file (the serial `parse` probe's 64.7 MiB
  becomes roughly 112) and the chunk pool's ceiling claim in
  [`../manual/dump-inspection.md`](../manual/dump-inspection.md) moves from
  64 MiB to 256.

**The depth clamp is the second half of that decision.** `POOL_DEPTH`'s 4 is
"what the replay path wants" for *chunks*; under `Workers { jobs }` a pool that
retains one block per concurrent reader wants `jobs`, so the natural derivation
is `max(POOL_DEPTH, jobs)`. Left unwritten deliberately: it is only reviewable
against the budget answer above, and against the holder analysis `16.4.1`
carries.

**`16.4.1` still waits, and now on `16.7.1` rather than on this slice.** A
waiting acquire needs a budget the *pool* reads; a value nothing consumes is not
one. The spec's binding-orderings line carries the amendment.

**No figure changed colour.** The diff touches `io.rs`, `scan.rs` and
`batch.rs`, which are declared paths for most of the register — and every figure
declaring any of them was already stale on that same path before this slice, so
`--stale` reads exactly as it did: seventeen red, `nested-decode-micro` green.
Nothing here is claimed excused. What the diff adds is two words to
`ScanOptions` and `QueryOptions` and an enum no production path constructs, so
no registered command shape reads the value; the structs themselves are still
built by every scan, which is why this is reported as unchanged colour rather
than as reachability ([`measurements.md`](measurements.md), "A stale figure does
not oblige a sweep").
