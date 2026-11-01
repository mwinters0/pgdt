# P33 — Every line PostgreSQL writes and reads

**A `COPY` line is read up to the bound PostgreSQL holds it to, and refused only
past it.** What `pg_dump` writes is the floor and what PostgreSQL reads the
ceiling, for a line as for a value (`roadmap.md`, "The input contract is valid
PostgreSQL, not `pg_dump`'s output"; "A literal is guaranteed in `*_out`'s form
and never read past `*_in`'s"); for a line the two are one bound,
[`postgres-invariants.md`](postgres-invariants.md), "I71". A line under it is
read whatever the chunk size, `--jobs`, the leader's split, a compressed
source's blocks or the resume point. Memory may slow a long line and never
refuses one under the bound. This closes `KD93`.

The user-facing case is a dump whose lines run long for reasons nobody can see
from outside it — Datanommer's, read today only after raising a limit by trial,
its longest line 462 MB (`runs/datanommer-survey/report.md`, machine-local).
**The answer is no setting at all**: nothing to discover, and no ceiling taken
from the hardware, PostgreSQL's being the only one.

## The bound is a constant, and the knobs go

`--max-line-bytes`, `pgdump.max_line_bytes` and `ReadOptions::max_line_bytes`
are removed, pre-1.0 with no shim. The bound is one constant, I71's, measured
exactly — terminator included — on every path that reads a line: the serial
scan, a piece of the leader's split, a query's segment. A line past it is
refused as one PostgreSQL would not read: the refusal names the bound and
I71, the line's start, and its table, never a setting. A cap below the bound
would refuse a valid line for memory's sake, which `--memory` alone governs.
The Future item "a query's line limit taken from the map" is answered: there is
nothing left for it to lift.

## A pass holds a line only where it decodes it

**A pass that decodes no field of a line streams it**: it finds the line's end
and counts the row without holding its bytes. That is a metadata-level parse, a
query's census, a piece's resync and a segment's row-start search, each then
flat in memory at any line length. **A pass that decodes a line holds it whole**
(`decisions.md`, "D23") and is charged for it, below.

Rejected: streaming field by field in a gathering parse, which keeps only the
heads statistics clip (`decisions.md`, "D76"). It decodes a value the pass never
sees whole, and a value refused by its `*_in` — strict mode's check, a json or
array value — needs the whole value, so it needs a streaming input function
throughout the codec. Reopens: a codec whose decoders stream.

## A held line is charged, and admitted in file order

**A run's arrangement carves a ledger from its allowance, and a reader whose held
line grows past its chunk term draws the growth from it.** Where a draw cannot
be met the reader waits, and **the waiting line earliest in the file is admitted
first**, so the leftmost line always progresses and no pass deadlocks; file order
is the order the leader consumes its pieces in already. **A query's guarantee
holds while its caller polls or drops each partition stream it holds**, the
ordinary `Stream` contract, which DataFusion's executors meet by a task per input
partition or a re-poll on wake: a holder gives its line up only when next
polled, so a stream left unpolled mid-line keeps its draw, and
`TablePartitions`'s rustdoc says so. Rejected: a ledger owning each held buffer
to drop it under a suspended holder, which frees memory a stream is mid-way
through using; the replay driving its readers in tasks of its own, threads by
surprise (`decisions.md`, "D1"). **A line is refused for
memory only when it cannot be held with nothing else drawn**, and that refusal
names `--memory` and the bytes the line needs, not the line.

Rejected: a reservation of the bound per worker, which multiplies; one reader
at a time holding more than its chunk, which is this ledger of one and wastes
the concurrency clustered lines offer (three to nine lines over 64 MiB in one
Datanommer chunk); an uncharged carry under the reserve, which is today's and
which `MEMORY_RESERVE` absorbs only while a line is bounded at 64 MiB.

**A block's longest line is recorded in the map**, its start and its length as
I71 measures it, by every pass that maps the block, which finds each line's end
whether or not it holds the line. It is a fact of the dump, never a charge, which
is the pass's and the build's, and `pgdt info` shows it. **A read over mapped
blocks plans with it**: its arrangement carves the charge of the longest line it
will meet into the ledger before a reader starts, so that line is admitted
without waiting, and a read whose longest line one reader could not hold is
refused before it reads a byte, naming that line's start. Every query has it,
the provider reading only a complete map (`decisions.md`, "D90"); a mapping pass
meets its lines unknown, and for it, and for held lines that coincide, the
ledger's waiting is what holds.

**A waiter keeps what it holds until the earliest waiter needs it.** When the
earliest waiting line cannot be admitted, later holders give theirs up, the
latest in the file first, until it can; a reader that gave its line up re-reads
it from the line's start once admitted, the source being the spill — every
source is positioned, and a fetched one re-fetches. A re-read costs time, never
an answer. Rejected: dropping on every wait, which re-reads where a sibling was
about to finish; refusing admission to a new long line while an earlier one
waits, which still wedges on two lines that each began short and grew.

## What a held line costs

**The copies go before the charge is set**, since a ledger sized for copies the
design can remove is sized for waste. The held line's buffer is handed to the
Arrow builder as a block, as a chunk is, so an unescaped value spanning chunks
is not copied again; the replay releases the chunks under a line it has copied;
the buffer is freed after its row rather than kept at its high-water mark. A
query then holds the line and at most one decoded copy — about the line for an
unescaped value, twice it for an escaped one, one and a half for hex bytea and up
to twice it for `bytea_output = escape`'s form, which no dump pins (I4) —
where today it holds three to five times it ([`measurements.md`](measurements.md),
"What a held line holds resident").

**The charge is the held line plus the bound its decode adds**: every decode
shortens a value, so a pass that decodes a whole value — a query, a strict
parse — draws twice the held bytes, and a default parse, whose statistics keep
clipped heads (`decisions.md`, "D76"), draws once. Whether that matches what is
resident is a reading, an evidence slice's on the reference dump's clustered
lines, never an argued constant. Rejected: one factor for every pass, which
charges a default parse for a copy it never makes.

**A line that outgrows its chunk grows outside the allocator**, in memory the
carry holds for it alone: once there it never moves, so no growth holds it
twice; what it holds allocated is what it has written; and dropping it after its
row returns its pages at once. So the charge is in bytes held, on every
allocator. Rejected: growing it through the global allocator and charging what
that allocator keeps — capacity up to twice the line, the outgrown buffer beside
its successor at the last doubling, and under mimalloc each outgrown buffer
resident until its purge — a factor of one allocator's habits, over-charging
every other (`decisions.md`, "D13") and varying with how fast the line grows;
the binary lowering mimalloc's purge delay, which removes the retention alone,
for every buffer the process frees, at an unmeasured cost (`KD148`'s class).
**It moves there when it outgrows its chunk**, where its draw begins, so a line
no chunk outgrows never leaves the carry, and **it grows by remapping, never by
a copy**: its commit and address space are its bytes. That is Linux's
(`decisions.md`, "D104"); macOS, which has no `mremap`, is P34's. Rejected:
reserving I71's bound of address space per held line, which never moves it but
which a strict overcommit or an address-space limit counts in full, refusing a
line under the bound for memory it never touches.

**The charge is in bytes, never in blocks.** A held line copies out of the
chunks it crosses, so on a compressed source it pins only its current chunk's
block, whose retention the block pool already bills; a line spanning many of a
source's blocks costs its bytes, not theirs.

## The leader and the replay read a line once

**A reader grows its held line read by read and never re-reads it**; today the
owner of a line doubles its read from the line's start, reading it about twice.
A piece inside a line still resyncs by searching its own range for a row start,
holding nothing: that read runs at the device's rate, in parallel, and is how a
piece learns it is empty, so a line costs about twice its length in reads.
**A query's segment searches for its first row start only to its own limit**, as
a piece does, not to its block's end; a segment holding none is empty, its
predecessor reading through the line. Rejected: a hand-off from a piece finding
no row start to the leader, which saves only pieces in later windows, the
leader dispatching a window's at once; merging empty pieces into their
predecessor at the next cut, which saves the same.

**A batch is cut before a value would carry an offset-bounded column past its
type**, whatever `QueryOptions` say. Text builds `Utf8View`, bounded per value
and block only by `u32::MAX`; bytea's `Binary` has `i32` offsets, and Arrow's
builder panics past them rather than refusing. The default span cap keeps a
batch under them, but `max_source_span` and `max_bytes` are a caller's to lift,
and then only `max_rows` cuts. So a batch flushes before a row whose value
would pass the bound: one value under I71's always fits in an empty batch, so
the cut is unconditional and costs nothing at the default. Rejected: a
`max_source_span` made mandatory under a ceiling, which makes a knob that only
bounds pinning bound correctness too; leaving the panic to whoever lifts the
cap. What DataFusion does when it concatenates such batches is not the phase's.

## The floor is one reader

**A line is refused for memory only when one reader could not hold it**: when
the allowance's cap less one reader's term, `at(1)`, cannot hold its charge,
whatever `--jobs` says. The carve fills its cap with readers, leaving the
ledger little, so when the earliest line cannot be admitted after later holders
have given theirs up, **the pass parks readers, the latest in the file first**,
each returning its standing term — its chunk and its share of the pools'
retention — until the line fits; they resume as its row is emitted and its draw
returned. This makes the pools able to shrink and regrow, `BufferPool`'s slots
and the block cache's retention, which today they cannot. An allowance nobody
stated — a library caller's `Serial` with none (`decisions.md`, "D1") — carves
a ledger without limit, whose draws never wait.

Rejected: refusing when the carve's leftover and the lines others give up fall
short, under which a line read at `--jobs 1` is refused at `--jobs 24`; a
ledger reserved for one line at the bound in every carve, which costs readers
on every dump holding no long line, where the map's longest line costs nothing.

## Where it lives

Placement on the concept map ([`concepts.md`](concepts.md)); no crate gains an
edge, pgdt-dump depending on pgdt-io already.

- **The bound** — pgdt-core's `copy.rs`, "The `COPY` format", citing I71: the
  scanner, the leader and the replay read it, and it sits below them all.
- **The ledger** — a module of pgdt-io's, "Memory and parallelism", made only
  by `Arrangement::resolve` (`decisions.md`, "D83"), so no pass holds one not
  carved from its allowance. A pass is handed it with its arrangement, never as
  a `ScanOptions` field: options are configuration reused across runs and a
  ledger is one run's state, as the cancellation is (`decisions.md`, "D26").
  The DataFusion provider carves one per scan, inside that scan's draw on the
  session's `ScanBudget`.
- **The held line and the line streamed** — pgdt-dump's `scan/chunked.rs`,
  the carry and its draws; the resync where it is, pgdt-mapping's interior and
  pgdt-query's `replay/segment.rs`.
- **The offset cut** — pgdt-query's `batch.rs`, "Arrow batches", beside the
  flush triggers it joins.
- **The longest line** — pgdt-model's `CopyBlock`, recorded by pgdt-mapping's
  passes; `Arrangement::resolve` is handed its charge as a number, pgdt-io
  seeing no model.
- **The refusals** — pgdt-core's `error.rs`: `LineTooLong` loses its limit and
  names I71's bound; a line outgrowing the allowance is a refusal of its own,
  naming the line's start, the bytes it needs and the allowance, which
  `pgdt`'s `refusal.rs` spells with `--memory` as its remedy and the shell's
  `spelling.rs` with `pgdump.memory`.

## What the phase does not own

- **A field refused for its size** under a line under the bound — an
  `array_in` value whose binary form passes `MaxAllocSize` — is the value
  rule's ("Strict parse" on the concept map). Nothing refuses one today, and a
  `(c)` `KD<k>` is filed for it by the phase's first slice.
- **A CR-terminated row** (`KD101`): the bound counts a CR terminator as I71
  does; a resync finding only LF stays `KD101`'s.
- **A refusal's save**: a line past the bound is an invalid dump, aborting as a
  field refused at parse does and as a restore under `ON_ERROR_STOP` does; a
  memory refusal keeps what the throttled saves banked, as any failed parse.
- **`MEMORY_RESERVE`** no longer absorbs a carried line once the line is
  charged; re-setting it is P23's (`KD34`), told in its inbox.

## How it is shown

**Every path is tested at both edges of the bound**: a line of exactly I71's
bound, terminator included, read, and one a byte longer refused — the serial
scan, a piece, a segment, an `.xz` and a gzip source, a resume point — and the
ledger's admission order, giving up and re-reading, parking and floor refusal,
with a query's partitions driven as `CoalescePartitionsExec` drives them; and a
batch of near-bound bytea values under a lifted span cap cut before its offsets
overflow.
Each path takes the bound as a crate-private argument whose public value is
I71's constant, so the logic is tested at a small bound and a small allowance
deterministically, and no caller outside the crate can reach it: it is not a
setting by another name. **One end-to-end test reads a line at the real bound**
and refuses one a byte longer, through `pgdt parse` and `pgdt query` over a
plain and an `.xz` source, in a nextest group of one thread, proving the
constant reaches every path and nothing downstream bounds a line lower.
Rejected: real-bound tests throughout, gigabytes per test; small-bound tests and
an asserted constant alone, which prove nothing about the wiring.

**The charge factors are a claim about resident memory, so a figure takes them**:
`long-line-resident`, `measure.py`'s, the peak resident by pass kind and line
length — default parse, strict parse, a query over text, escaped text and
bytea, serial and at four workers (`roadmap.md`, "Four workers is the
optimization baseline; twenty-four is the guard") — over a generated input
clustering lines near the bound. It is taken on today's code first, pricing
the copies, and re-taken once the ledger lands, confirming the factors or
re-setting them. **The acceptance is a hand run on the reference dump** under a
stated `--memory` below what its clusters hold at once: read with no setting,
slowed and not refused, recorded in `runs/` and the day's history entry. The
dump is machine-local and not ours, so it is never in `measure.py`. Today's
uncharged outcome to set it against is a whole data-level parse at 24 workers
under a 12 GiB container limit with the limit raised to 512 MiB, read without
refusal and counted against the catalog
(`runs/p38.7-datanommer-count-20261031-1224/`); its cache is kept in
`/mnt/ssd/fedora/scratch/pgdump_query/datanommer/`. No resident reading was
taken of it.

## Slices

**The first slice exists to produce evidence**, and the bound is raised last,
so no commit reads a line at I71's bound uncharged. The leader and the replay
are reworked apart from the ledger, a tested core path and a new mechanism
being two reviews (`../process.md`, "Size a slice by its review, not by its
scope").

1. **Evidence**: the `long-line-resident` generator and figure on today's code;
   the `(c)` `KD<k>` for a field refused for its size. No production code.
2. **Each line read once**: the owner grows its line, never re-reading; a
   segment's row-start search ends at its limit; the growth tests asserting no
   read past the chunk.
3. **Hold only what is decoded**: a pass decoding nothing streams a line; the
   held line freed after its row and handed to the builder as a block; the
   replay releasing chunks under a copied line; the figure re-taken.
4. **The ledger**: draws, file-order admission, giving up and re-reading, the
   factors, the unlimited ledger of an unstated allowance, a query's polling
   contract stated on `TablePartitions` and tested — under today's
   64 MiB limit, so it is exercised before it is relied on.
5. **The floor**: the pools shrinking and regrowing, readers parked, a line one
   reader cannot hold refused for memory and its remedies spelled.
6. **The bound**: I71's constant on every path, measured exactly;
   `LineTooLong` reshaped; a batch cut before an offset overflows; the flag,
   the setting, their manual section and tests removed; the real-bound test;
   `KD93` struck.
7. **Acceptance**: the figure re-taken, the factors confirmed or re-set; the
   hand run on the reference dump.
