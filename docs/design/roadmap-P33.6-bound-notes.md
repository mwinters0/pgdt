# P33.6 notes — the bound

**`LINE_BOUND`, `pgdt-core`'s `copy.rs`, is I71's `MaxAllocSize − 1`**, and
every read loop holds a line to it (D164). The check is `ChunkCarry::passes_bound`,
asked before a read is drawn for or absorbed — by `ChunkedScanner::next_chunk_folding`
(`hold_to_bound`) and by an interior worker (`scan_partition`) — so a line past
the bound is refused at its start and never held, drawn or charged past it. A
read is clamped to the bound (`ReadOptions::chunk`), an interior worker's
whole-unit first read included, so a line one read holds is within it. The check
that ran where a chunk's passes ended (`ChunkEvents::end_pass`) is gone.

`Error::LineTooLong { offset, table, bound }` names the line's start, the
bound its reader read to — `LINE_BOUND` outside a test, carried rather than
named in `error.rs`, which naming `copy.rs`'s constant puts in a module cycle
with it — and, where its reader knows it, the `COPY` block's table: the scanner where it entered the
block at its header (`CopyScanner::copy_table`), the back-fill and the replay
from their block (`Error::in_copy_of`), an interior region from its own table
unless an earlier piece held the terminator. Its message says no restore reads the
line; I71 is named in its rustdoc, not its text, no other refusal citing a
register entry.

**The offset cut is a ceiling on `max_bytes`**: a `RowBatcher` whose projected
schema holds an `i32`-offset type (`Binary`, `Utf8`, `List`, `Map`, a struct or
dictionary over one) flushes once its text reaches `OFFSET_CUT_BYTES`,
`i32::MAX + 1 − LINE_BOUND`, whatever `QueryOptions` say; a row's fields being
shorter than its line, the next row cannot carry an offset past `i32::MAX`.

## What the next slices inherit

- **The bound reaches a test only through `ReadOptions::line_bound`**, a field a
  build has only under `cfg(test)` or `pgdt-dump`'s `test-support`;
  `ReadOptions::line_bound()` is `LINE_BOUND` everywhere else. A production
  caller outside `pgdt-dump` states its chunk by `ReadOptions::with_chunk_size`,
  which compiles alike with and without the field; a struct update over it
  warns in a build without it.
- **33.7's hand runs need no setting**, and `long-line-resident`'s legs state
  none: `measure.py` dropped `--max-line-bytes` and `SET pgdump.max_line_bytes`
  from every leg, so the figure's commands moved and it is stale until 33.7
  re-takes it. Its published description still says the legs stated the
  bound, true of the readings it holds.
- **The real-bound test** is `crates/pgdt/tests/line_bound.rs`, two tests in
  `.config/nextest.toml`'s `line-bound` group of one thread: about ten seconds
  in the image, writing two gibibyte dumps apiece
  (`runs/33.6-line-bound/nextest.log`).
- **Each path at both edges** is `crates/pgdt-query/tests/line_bound.rs` (the
  serial and split mapping pass, `.xz` and gzip sources, a resumed pass, the
  replay serially and over four and eight partitions),
  `scan/chunked/tests.rs`'s `a_line_is_refused_exactly_past_the_bound` (held,
  streamed, outside a block, ended by the file's end) and
  `interior/tests.rs`'s `a_worker_holds_a_line_to_the_bound_exactly`.

## Negative results

- **A long row right after a block, read by a split pass, names no table**: a
  piece past the terminator fails the block's region (`KD134`, rewritten to say
  so), and naming that region's table would name the wrong one.
- **A replay leaving a segment between chunks never meets a line past the
  bound** it does not read, the check now preceding the read; a replay reads
  only mapped blocks, whose mapping pass refused such a line already.
- **No count pair**: the check moved from where a chunk's passes end to before
  a read is absorbed, both once a chunk; `RowBatcher::new` runs once a block.
  The one per-row difference is data, not code: `should_flush`'s `max_bytes`
  test, already evaluated per row, now compares where it found `None` for a
  query of an offset-bounded column with no byte cap (STATUS, "Decisions worth
  another look").
- **The bound is not tested at the real size through the provider or the
  back-fill**: the real-bound test runs `pgdt parse` and `pgdt query`, as the
  spec names; both share `ChunkedScanner` with the paths it reads.
