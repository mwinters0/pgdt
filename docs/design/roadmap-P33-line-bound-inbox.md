# P33 inbox — facts filed for its grilling

Evidence P33 (every line PostgreSQL writes and reads) will need. **This is a
queue, not a document**: when P33 is grilled, walk every entry, fold it into
the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## A reader's carried line is charged to no budget

**Fact.** A block source states its per-worker cost as its chunk and decoder
(`io.rs`, `worker_memory`, from `reader_bytes`); the line a reader carries
across chunks (`ChunkCarry`) is charged nowhere, `MEMORY_RESERVE` absorbing
it while it is bounded at 64 MiB. Under I71 each concurrent reader may carry
up to 1 GiB, and the scanner holds a line whole before emitting its row
(`decisions.md`, "D23").

**Why P33 cares.** "Memory may slow a long line and never refuses one under
the bound" needs a mechanism: how many readers may carry a long line at once,
how one is admitted when the budget is drawn, and what is said when a stated
`--memory` cannot hold one line of the bound at all — that refusal naming
memory, not the line. `MEMORY_RESERVE`'s premise changes with it
(`decisions.md`, "I/O, memory and parallelism").

**Origin.** The 2026-10-03 session that withdrew the out-of-band item hardening `max_line_bytes`.

---

## The leader's row-start search grows a read to `max_line_bytes`

**Fact.** `leader.rs`'s `scan_partition` finds a piece's first row start by
doubling its read from the piece's offset until a row start is in it,
ceilinged at `max_line_bytes`; `stream.rs`'s `first_row_start` does the same
for a resume point. A piece wholly inside a long line reads up to the line's
length to learn it holds no row start.

**Why P33 cares.** At I71's bound that is up to 1 GiB read, and repeated by
every piece the line covers, before any is found empty. Whether a piece inside
a line is merged into its predecessor, handed to the worker carrying the line,
or found some other way is the leader's decision.

**Origin.** As above. The tests asserting the growth ceiling are
`leader.rs`'s `no_read_a_worker_makes_exceeds_the_chunk_size` and
`…_the_stated_unit`.

---

## Arrow arrays bound a batch's bytes, not only its rows

**Fact.** A `Utf8`/`Binary` array's offsets are `i32`, so its values total
under 2 GiB; a `Utf8View` data buffer is bounded the same way per buffer. Two
values of nearly 1 GiB each cannot share a `StringArray`.

**Why P33 cares.** A batch sized by rows alone fails on two long values in
one column; the phase decides whether a batch is cut by bytes too, and where.

**Origin.** As above; not yet reproduced against the batch builder
(`batch.rs`), so contingent on it holding no byte cut today.

---

## A compressed source holds its blocks while a line spans several

**Fact.** A line at I71's bound spans up to 9 of `blocks128.xz`'s 128 MiB
blocks and over 40 of the multistream download's (`CLAUDE.local.md`'s koji
sample). What the block cache retains while one reader's carry crosses
blocks has not been read.

**Why P33 cares.** If a carry pins the blocks it crossed, the memory account
above is per block as well as per byte.

**Origin.** As above; contingent on reading `io.rs`'s `BlockCache` for it.

---

## `--max-line-bytes` and `pgdump.max_line_bytes` lose their meaning

**Fact.** Both set `ScanOptions::max_line_bytes`, today the one bound on a
line. Under I71 the refusal is PostgreSQL's and fixed, so neither decides
which dumps are read. The Future item "A query's line limit taken from the
map" — the cache recording the longest line a parse saw, so a query reads
with it — existed only to lift the knob's default for a query, and moved
the cache format.

**Why P33 cares.** Whether the knobs survive as a memory cap below the
bound, or are removed (pre-1.0, no shim), is the phase's call, and the
Future item is answered by it either way.

**Origin.** As above; the Future item was moved here from `roadmap.md`.

---

## A line under the bound can still hold a field PostgreSQL refuses

**Fact.** I71 bounds the line; a field's input function bounds its value
separately. An array whose text is under the bound but whose binary form
passes `MaxAllocSize` is refused by `array_in`; `COPY FROM`'s per-field
buffer holds the line de-escaped, which is never longer than the line.

**Why P33 cares.** The line's ceiling is P33's and a field's is the value
rule's (`roadmap.md`, "A literal is guaranteed in `*_out`'s form and never
read past `*_in`'s"); the grilling says which phase owns a field refused for
size, and whether any other `COPY FROM` limit binds under I71. The
de-escaped buffer's bound is argued from `CopyReadAttributesText`, not yet
read on every major.

**Origin.** As above.
