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
query's census, a replay whose batch, static filter and sorted stop read no
field, a piece's resync and a segment's row-start search, each then flat in
memory at any line length.
**A query is charged for the table it reads, its cache cold or warm**: its
mapping pass censuses every block it maps (`decisions.md`, "D35"), and a census
holding its lines would charge a cold query, and refuse it, for a table it never
reads. **A query, decoding a line into its batch, holds it whole**
(`decisions.md`, "D23") and is charged for it, below. Reopens: readers that
resume across a line's runs, which a gathering parse's streaming builds (below)
— a query's own tree, grilled apart from a parse's.

**A census holds only the fields it decodes.** Its row streams, folded field by
field as its bytes go by and counted against its header's width as a held row is
(`decisions.md`, "D28"): the array-shape census as one fold over byte ranges,
answering exactly what it answers over a whole field, the slice path being that
fold fed once; a counted scalar from a head no longer than the longest literal
its tier test answers for; and a counted container — an array, record, range or
multirange over a datetime, `interval` or decimal `numeric` leaf — held whole
and drawn from the ledger past its chunk term as a query's held line is (below),
its tier read by `decode_*`, the output grammar, as a gathering parse's is. The
head is exact because the codec's datetime
decoders refuse a literal past the buffer PostgreSQL parses one in, as
`date_in`, the timestamp inputs and `interval_in` do (`M276`). Rejected: a
block holding its lines wherever its counter tests a container, which holds a
long `text` line for a short `timestamptz[]` beside it; a clipped head for the
shape census, which moves a `text` column's persisted census for no array's
sake; counting a container through the `*_in` walks a gathering parse streams,
which read spellings `decode_*` refuses — `{ infinity }` in a `date[]` — so the
census would count a field the query reading it refuses. Reopens: a cold query
over a `text` column charged for a long `timestamptz[]` field beside it that it
never reads — P40's ([`roadmap.md`](roadmap.md), "P40 — Every reader streams").

**Where it lives**: the carry counts a streamed row's fields and hands each
byte range, its field's index beside it, to a fold its reader supplies —
scanning, no decoding (`pgdt-dump`'s `scan/chunked.rs`, D74); the census's fold
over them, `observe_row` that fold fed once, and `FieldCount` saying per column
whether a head is kept, are model's (`block.rs`), the answer
the codec's (`unrepresentable.rs`), as `concepts.md`, "What a pass records of a
block's rows", places the census. A replay decides as a row outgrows its chunk,
from what its plan reads — its batch, static filter and sorted stop — and never
from a dynamic filter's state, which arrives after the line charge is carved
(`decisions.md`, "D160").

**A gathering parse streams a line too**, strict or not, **over the types a
dump's long fields hold**: `text`, `character`, `bytea`, `json`, `jsonb`, the
containers whose walks feed their parts — an array, record, range or multirange
— and the `bit`, `varbit` and geometric readers only a strict parse reads. Every
statistic it gathers and every check it makes over them is fed its field's runs
as they go by and keeps a bounded state per column — a head (`decisions.md`,
"D76"), a length, a fold's carried state — so a parse holds nothing that grows
with such a field. **A scalar that an order key or a query's batch reads** — an
integer, `oid`, `boolean`, enum, `uuid`, `numeric`, float, datetime, `interval`
or network address, bare or as a container's part — **is collected whole and
read by its slice reader**, the one `field_key` and the batch read it with, and
that copy is drawn from the ledger past its chunk term as a query's held line is
(below): a parse is never refused for memory where its line alone could be held.
Each streamed reader resumes across
runs with one grammar per type, a slice being one run, and **looks no further
ahead than a fixed bound**, carrying what lies past it as state — a `bytea`
read once for both its answers — and **reads an unquoted element's whitespace
run on into its part**, rewound where the run turns out trailing: the container
marks its part where the run starts, the part restoring what it held there, and
a refusal met inside the run stands only where it turns out interior, as `{1\v}`
is a valid `json[]`. The one state that
grows with a field is a `json` value's open containers, a bit a level and so at
most an eighth of it, uncharged: PostgreSQL bounds that depth only by
`max_stack_depth`, a setting, and nothing refuses one here
([`postgres-invariants.md`](postgres-invariants.md), "I72"). Rejected: holding the
line, which charges a parse for values it reads only to keep heads of, on the
dumps whose long lines cluster three to nine in a chunk; holding a long `text`
or `jsonb` field whole, the line again on the dumps whose long lines are one
such field; a push machine for every scalar, which costs `field_key` and a
typed query on every short value for a long scalar no dump in hand holds
([`roadmap-P33.22-priced-notes.md`](roadmap-P33.22-priced-notes.md)); a fork keeping its runs
back to where it forked, a line's worth on one whitespace run, and the run held
until its end decides it, as its bytes or their run lengths, the same on a run
alternating its bytes, which no `pg_dump` writes and a restore reads; a rewind
in each leaf reader, one per type where `Parts` has a few implementors; charging the
depth bits, the ledger back for a parse over no document a dump holds; refusing
past a depth, which PostgreSQL does not. Reopens: no resumable reader keeping
one grammar per type, which a second parser would not answer; a dump whose long
field is a keyed scalar, or a container of one, that a parse must read under a
`--memory` below it — P40's ([`roadmap.md`](roadmap.md), "P40 — Every reader
streams").

**Where it lives**: `BlockObserver` is a fold over a row's field runs, as the
census's is — pgdt-model's (`statistics.rs`), implemented by pgdt-mapping's
gatherer and strict check, adapted to the carry's fold by `map.rs` beside the
census's — a held row being that fold fed once, so a data-level parse reads
rows through the path a metadata-level one streams them on. Rejected: a
streaming observer beside `observe_row`, two routes into one column's
statistics.

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
will meet into the ledger before a reader starts — the longest of the blocks it
reads a byte of, nothing where its plan reads no field, a long row of that plan
streamed and kept whatever a dynamic filter later reads — so that line is admitted
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
the buffer is freed after its row rather than kept at its high-water mark.
**What only reads a value holds nothing that grows with it**: a filter's term,
a strict parse's check and a parse's statistics walk the field as `COPY`'s
unescaped bytes as they go — a comparison stopping at its first difference, a
check holding no tree, statistics keeping a head (`decisions.md`, "D76") —
so the one decoded copy a value has is its batch's. **But an ordering key over
a value the server normalizes** — a `jsonb` object, whose pairs it sorts and
de-duplicates, a multirange, whose members it sorts and merges — holds the
least that order needs: each pair's position in the field, sorted, its value
walked from there, and each member's bounds. **A nested value is decoded
straight into its builders** as its walk reads each element, with no staging of
its elements, a refused field ending its query; one a tier test reads
([`decisions.md`](decisions.md), "D98") has its leaves tested by the same walk,
staging nothing, before it is decoded — a second read only in a block counting
such a value — and the census's and a parse's tier read that walk too. **A
nested value past its chunk term is decoded into builders reserved to its
bound**, its batch cut before it so they start empty, each buffer reserved to
the field's length at that buffer's density, so its copy never grows by
doubling: what it holds is what is reserved and charged, on every allocator, as
the held line's is (below). A query then holds the
line and that copy — about the line for an unescaped value, twice it for an
escaped one, one and a half for hex bytea and up to twice it for
`bytea_output = escape`'s form, which no dump pins (I4) — where today it holds
three to five times it ([`measurements.md`](measurements.md), "What a held line
holds resident"). Rejected: an unescaped value shared between a filter and its
batch, which still copies for every row the filter drops and ties the predicate
to the batcher's buffers; a key built whole for a value only compared, which
over an array or `jsonb` is a tree of tens of bytes an element; refusing an
ordering term over a long `jsonb` or multirange, which a key of positions reads;
a tested container's elements staged, a 24-byte slot for an element of two
text bytes, on the ground that its type never grows long, which holds for a
scalar and not its container; a builder made for the one value and copied in;
a nested value's builders grown by doubling, as a batch's are within its span
(D141), which past its chunk term holds the outgrown buffer beside its
successor and, under mimalloc, keeps it to its purge; every factor above one
doubled instead, which over-charges an allocator growing in place.

**The charge is the held line plus the bound its decode adds**: a parse, strict
or not, streams its line and draws only for the scalar text it collects, at most
its line's length (above); a query draws once plus its batch's
decode, bounded by the largest **decode factor** among the columns it reads —
the most its column's Arrow type and plan hold per text byte, known from the
schema before a reader starts — and by each ordering term's **key factor** over
a normalized type, what its positions hold per text byte. A decode factor is one
for text and `bytea`, every decode of them shortening a value, and more for an
array of fixed-width elements, whose `{1,2}` spends two text bytes on each
eight-byte `int8`. Whether that
matches what is resident is a reading, an evidence slice's on the reference
dump's clustered lines, never an argued constant. Rejected: one factor for
every pass, which charges a parse for a copy it never makes; one factor for
every query, which charges a query over text for `int8[]`'s.

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
a copy, in huge-page steps, its mapping kept aligned to a huge page** so the
kernel can back it with transparent huge pages: its commit, address space and
resident set are its bytes and less than one step more, which its charge
carries. **The step is the kernel's PMD huge page where that is at most 2 MiB**
— a 4 KiB base page, on x86-64 or arm64 — and the base page elsewhere, growing
as an exact mapping does: a step is slack every held line is charged, so it
stays small beside a line of at least one chunk, and a kernel whose huge page
passes the ceiling faults in base pages large enough that the gap the step
closes is mostly gone. **The mapping asks for no huge pages** (`MADV_HUGEPAGE`):
whether a step is backed by one is the host's THP policy, so under `madvise` or
`never` it is faulted in base pages as an exact one is. That is Linux's
(`decisions.md`, "D104"); macOS, which has no `mremap`, is P34's. Rejected:
growing it to exactly its bytes, whose mapping always ends at its last byte and
so is faulted and freed in base pages, the whole of its gap to a line mapped at
its length (`roadmap-P33.10-mapping-notes.md`, "What the readings found");
mapping each held line at its block's longest line, which a block's first read
cannot know and a strict overcommit counts whole for every line it holds;
reserving I71's bound of address space per held line, which never moves it and,
reserved inaccessible and made writable step by step, commits only what it
writes, but which an address-space limit counts in full, refusing a line under
the bound for memory it never touches; the PMD huge page whatever its size, up
to 512 MiB of slack a line on a 64 KiB-page kernel; asking for huge pages, which
under the kernel's default defrag compacts memory at the fault, an unmeasured
stall on a fragmented host, and overrides a policy that is the host's.

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
the allowance's cap less one reader's term, `at(1)`, and what stays resident
or reserved beside the line as the run is carved — the statistics it keeps,
other live scans' longest lines — cannot hold its charge, whatever `--jobs`
says, under an estimate as under a stated or discovered allowance. The carve fills its cap with readers, leaving the
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
on every dump holding no long line, where the map's longest line costs nothing;
a session memory pool's grant off the floor, a limit on DataFusion's operators
rather than a holding, which may pass the allowance and would refuse every long
line; the statistics a mapping pass gathers after the carve off it — all they
may hold, which takes most of the room off the floor; their yielding to a long
line, which makes the cache depend on when one was held; or what they hold at
the draw, which makes a refusal depend on timing.

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
  session's `ScanBudget`, and bills each live scan's line charge to every scan
  drawn after it, off the margin's ceiling as what the session holds is.
- **The held line and the line streamed** — pgdt-dump's `scan/chunked.rs`,
  the carry and its draws; the resync where it is, pgdt-mapping's interior and
  pgdt-query's `replay/segment.rs`.
- **The resumable readers** — pgdt-codec's, the unescaper with them, each fed a
  field's runs as the carry's fold hands them over. The mark and the rewind are
  `Parts`'s (`value/source.rs`), the trait a container's walk hands its parts
  through, each implementor restoring its own state or truncating what it
  collected. Rejected: a reader pulling
  its source's next chunk itself, a codec walk driving pgdt-dump's reader, its
  tab split, CR hold and chunk retention beneath it (`decisions.md`, "D74").
- **The offset cut** — pgdt-query's `batch.rs`, "Arrow batches", beside the
  flush triggers it joins.
- **The longest line** — pgdt-model's `CopyBlock`, recorded by pgdt-mapping's
  passes; `Arrangement::resolve` is handed its charge as a number, pgdt-io
  seeing no model.
- **The factors** — a decode factor beside each of pgdt-query's builders,
  which write the Arrow form it bounds, and a key factor beside pgdt-codec's
  normalized keys; pgdt-query folds them over a query's columns and terms and
  hands `Arrangement::resolve` one number beside the longest line, a parse
  handing none. Rejected: a per-type table in pgdt-codec, stating a fact about
  pgdt-query's builders a crate away.
- **The refusals** — pgdt-core's `error.rs`: `LineTooLong` loses its limit and
  names I71's bound; a line outgrowing the allowance is a refusal of its own,
  naming the line's start, the bytes it needs, the allowance and what stays
  beside the line off it, which
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
clustering lines near the bound. **Each term of a factor no count of Arrow's
buffers sees has a leg**, from the slice that computes the factors on: a query
over an `int8[]` of single digits, a fixed-width array's form, where a
builder's growth shows; one over a `text[]` of one-byte elements, the stage a
part's token and unquoted copy hold beside its view; and an ordering filter
over a `jsonb` object of empty keys, a key factor, each the densest text its
term is stated over. Each reads its lengths at one reader, what one line holds
being a term's question, and each reads the bound beside them, the cell the
floor's charge for a factor is otherwise extrapolated to. Each leg's factor is read off its planned
query by the instrument, never restated, and the stated allowance is sized from
the largest so it never binds on a line. Rejected: a leg for the largest
factor, which reads the form `int8[]`'s does and a count of Arrow's buffers
already checks per type; a factor restated in `measure.py`, a second copy
nothing keeps in step; a user surface showing it, which nothing user-facing
needs; a term's leg at four readers, which reads how many lines the cut holds
at once, as every other kind's column does; a `jsonb` object as `jsonb_out`
writes one, distinct keys and its spaces, which reads below the factor's
density and so cannot show it understated. It is taken on today's code first, pricing
the copies, and re-taken once the ledger lands, confirming the factors or
re-setting them, and showing a parse flat in its line length. **The
acceptance is two hand runs on the reference dump**, read with no setting and
recorded in `runs/` and the day's history entry: a data-level, strict parse
under a stated `--memory` below its longest line, flat and never waiting on a
line, holding none, its long field a `text` column a parse streams — below
that line the reserve leaves the reference dump one reader, so its readers
and statistics are the allowance's, not the line's; and a query whose batch reads its long field under a `--memory`
carving two readers or more below what the lines its clusters hold at once
charge, so its arrangement rather than its timing makes readers wait and give
up their lines, read as not refused, its peak resident within that `--memory`
and its rows an unlimited run's, the one reading of the ledger over a real
dump.
Rejected: a `--memory` only below what its clusters hold at once, which may
carve one reader, where nothing waits or parks; a count of waits and parks,
user surface no user needs, or an `introspect` count the shipped build's run
cannot read; a park read over a real dump, which only an uncarved estimate
outside a container's limit makes, the ledger's tests reading parks. The
dump is machine-local and not ours, so it is never in `measure.py`. Today's
uncharged outcome to set it against is a whole data-level parse at 24 workers
under a 12 GiB container limit with the limit raised to 512 MiB, read without
refusal and counted against the catalog
(`runs/p38.7-datanommer-count-20261031-1224/`); its cache is kept in
`/mnt/ssd/fedora/scratch/pgdump_query/datanommer/`. No resident reading was
taken of it.

**A reader moved onto a hot path is priced** ([`decisions.md`](decisions.md),
"D108"): a slice changing code a `--count-pair` row runs per row or per byte —
what `field_key` reads a key with, what a query's batch decodes a column with,
and the path a parse's row and long field take to the gatherer and the strict
check — takes a count pair at its parent and its tree, a rise it resolves placed
and then reworked or accepted. Rejected: a memory phase leaving its readers'
speed unpriced, which accepts a cost on the statistics and query paths that no
reading states; a trigger naming the key's and the batch's readers alone, which
leaves a default parse's every row free to move unpriced.

**A reader made a push machine answers as the one it replaces**: a differential
test holds it to a test-only copy of its predecessor, a spelling read as
unparsed staying unparsed, and reading more is a change of its own. Rejected:
widening what a type reads in the slice that makes it stream, a contract change
and a cache format's bump inside a review sized for memory.

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
