# Dump inspection

`pgdq` tells you what is in a dump file — tables, roles, indexes, functions,
everything `pg_dump` wrote — without decoding any row data. It takes two
commands, and the split between them is the thing to learn first:

```sh
pgdq parse --source mydump.sql   # reads the dump, writes mydump.sql.dqcache
pgdq info  --source mydump.sql   # reports what that cache holds
```

**`parse` is the only command that reads your dump. `info` never does.** On a
5KB dump the difference is invisible; on a 784GB one, `parse` is an hour of
disk and `info` is instant. Splitting them means a command that reads like a
question — "what is in this file?" — can never turn into an hour of I/O you
did not ask for.

Run `parse` once per dump file. Everything after that is `info` and `query`,
answered from the cache.

## `parse`: reading the dump

```sh
pgdq parse --source mydump.sql
```

It scans the file end to end, writes the cache beside it
(`mydump.sql.dqcache`), and prints the same listing `info` prints. Pass
`--dqcache <path>` to put the cache somewhere else — worth doing when the dump
sits on a read-only mount, since the default location is next to the dump.

**An interrupted parse is not wasted work.** The scan banks its progress at
`COPY` block boundaries as it goes, so a run killed at minute 50 of 60 leaves
most of those 50 minutes on disk. Run `parse` again and it picks up where it
stopped:

```
$ pgdq parse --source koji.dump
resumed a previous scan at byte 612000104448 of 784019857152
...
```

Resuming is the default, and there is no flag for the opposite: **delete the
cache file** if you want a scan from byte 0. Running `parse` against a file
that is already fully cached costs nothing and says so.

The finished result is identical either way — a resumed scan and a
straight-through one produce the same index, byte for byte.

**Ctrl-C stops it cleanly.** On `SIGINT` (Ctrl-C) or `SIGTERM` (`docker stop`,
`kill`), `parse` stops at the next block or chunk boundary, writes everything
it has scanned to the cache, says where it stopped, and exits 130 or 143 so a
script can tell an interrupt from a failure:

```
$ pgdq parse --source koji.dump
^C
interrupted at byte 41231843328 of 784019857152 — the cache at koji.dump.dqcache holds the scan so far
re-run `pgdq parse --source koji.dump` to continue
```

A clean stop loses whatever the scan has done since it last banked, which is
the block it was reading plus any that finished in the moment before the
signal. A **second** Ctrl-C exits immediately without waiting for the write,
and so does `kill -9`, a power cut or a crash — they fall back to that same
last banking. `parse` banks often enough that the skipped work stays a small
fraction of the scan, and it spaces the bankings by what one costs rather than
by a fixed interval, because a scan that banked after every block would spend
more time banking than scanning. On a large dump, where blocks are minutes
apart, every block is banked and a clean stop loses only the one it was
reading. Nothing is ever left *corrupt*: the cache either loads or it does not,
and `pgdq info` states how far it goes.

### `parse --preamble-only`

If you only want the header block — server and `pg_dump` versions, extension
and user-defined-type counts — `--preamble-only` stops at the start of the
first table's data instead of reading the whole file. Its cost does not depend
on the dump's size: preamble-only on a 5KB dump and on a 500GB one take the
same time, because `pg_dump` always writes every `CREATE EXTENSION`/`CREATE
TYPE` ahead of any table's data.

It leaves an ordinary — partial — cache, which `info` then reads like any
other. Roles, tablespaces, the object-kind summary and the table listing are
unavailable until you run a full `parse`, because they require having read the
rest of the file.

### `.xz` files are read directly

If your dump is `.xz`-compressed, hand it over as it is — there is nothing to
decompress first, and no flag to pass:

```sh
pgdq parse --source mydump.sql.xz
pgdq info  --source mydump.sql.xz
pgdq query --source mydump.sql.xz --table public.widgets
```

Everything works exactly as it does on an uncompressed file: the same listing,
the same cache (written to `mydump.sql.xz.dqcache`), the same rows. Recognition
is by content, not by name, so a compressed dump called something other than
`.xz` is still read as one, and a file *named* `.xz` that is really plain text
is still read as plain text.

**Other compression is not read yet.** `.gz`, `.zst` and `.lz4` — including
what `pg_dump -Fp --compress=…` writes — still have to be decompressed before
pgdq sees them.

**One `.xz` file in three is slow to seek into, and pgdq says so when it is.**
An `.xz` file made of a single compressed block has nothing to seek by, so
reading anything but the start of it means decoding from the beginning:

```
diagnostics:
    [warning] this .xz source has no seek structure (1 block(s), one stream) — every read decodes the file from byte 0; recompress with `xz -T0` or `--block-size=<size>` for random access
```

`parse` is unaffected: it only ever reads forwards, so it costs the same on
such a file as on any other. `query` is where you would feel it, and only on a
large one — a single-block file the budget has room to hold twice over, with
room to spare for the decoder itself, is
decoded once and read from there, so only a bigger one pays the decode again on
every backward read. How big that is depends on the budget, and unless you set
one the budget depends on the machine (below, "`--jobs` and
`--parallel-memory`"). The remedy is in the message —
recompressing with `xz -T0` or an
explicit `--block-size` produces a file pgdq can seek into. Files that
`xz` produced with threads, or that were made by concatenating several `.xz`
files, are already seekable and earn no warning.

You can also ask a file what shape it is in, at any time and without reading
it: `pgdq info --detail` prints one line naming the container's blocks,
streams and largest block. It comes out of the cache, so it costs nothing and
works from a `--dqcache` alone:

```
$ pgdq info --source mydump.sql.xz --detail
compression: xz — 5700 block(s) in 1 stream(s), largest block 134217728 bytes uncompressed
```

`largest block` is what `--parallel-memory` has to clear **twice over**, and
the alternative way to learn it is `xz --list`, which on a file of many
concatenated streams reads every one of their footers.

Note that pgdq has to read the file's block index before it can read anything
else. On a file built from many concatenated streams — the shape a chunked
download or a `cat a.xz b.xz` produces — that index costs one disk seek per
stream, which on a very large file can be a minute or more. You pay it once:
`pgdq parse` saves the index in the cache beside the dump, and every command
after that takes it from there. A file compressed in one go with `xz -T0` or
`--block-size` never has the problem at all — its index is a single read
whatever the file's size.

If the cache stops matching the file it sits beside — you replaced the dump,
or pointed `--dqcache` at another file's cache — pgdq says so rather than
quietly working around it. All three commands refuse, `parse` included: a
cache that does not describe this file describes some *other* file, and
scanning would write over it. Delete it, or point `--dqcache` somewhere else,
and `parse` builds a fresh one.

### `--chunk-size`: you almost certainly do not need it

`parse` and `query` read the dump in 1 MiB pieces. `--chunk-size <bytes>`
changes that, and the reason to mention it at all is to say that the default
was chosen by measurement rather than by taste: over six sizes from 64 KiB to
16 MiB, on a SATA SSD, an NVMe drive and a RAM disk, 1 MiB was the fastest on
the only one of the three where the size made any difference at all, and the
sizes either side of it were ties rather than improvements.

Two things are worth knowing if you change it anyway. **Small is slower**:
64 KiB costs about 50% more CPU than 1 MiB, because the per-chunk work is paid
sixteen times as often. **Large costs memory, and the cost levels off**: read
buffers are reused at whatever size you ask for, and the pool holds four of
them or `--parallel-memory`'s worth, whichever is fewer — so under a 64 MiB
budget a 16 MiB chunk is 64 MiB of resident memory on top of whatever the scan
already holds, and a 32 MiB chunk is the same 64 MiB rather than double it.
Past that the pool keeps a single buffer, which is the size you asked for.

What it already holds does not grow with the *size* of the dump — a 3 GiB file
costs no more than a 2 MB one, a few megabytes either way — but it does grow
with the number of tables in it, by roughly 10 KB each. A dump of a few thousand
tables is tens of megabytes resident before any chunk size is chosen.

**An `.xz` source costs more than a plain one**, and by an amount the *file*
chooses as much as you: it decodes a whole compressed block at a time and keeps
as many of them as the budget affords, so a file of 24 MiB blocks adds about
48 MiB resident for every worker the budget allows. That is what buys reading the same block
repeatedly for free; the block size is set when the file is compressed
(`xz --block-size=`), not when it is read. A file whose blocks are too large
for the budget to hold two of is read a different way — see `--parallel-memory`
below.

The flag exists for a device unlike any of those three. If you have one and
find a size that beats 1 MiB on it, that is worth reporting.

### `--jobs` and `--parallel-memory`: the workers and the budget

`parse` and `query` take two more numbers: how many workers to ask for, and a
bound on what those may hold in memory.

**`--parallel-memory <bytes>` is how much memory pgdq's read buffers may
hold. Left unstated, pgdq works it out from your memory allocation and the
file.** It is a bound rather than a target: pgdq will not exceed it by
allocating a buffer bigger than you allowed. The one place that bites is `.xz`
input. A compressed file is normally read a whole block at a
time, which is what makes reading the same block twice free — but that needs
room for **two** blocks, one being held while the next is decoded, and for the
decompressor's own working memory beside them — mostly the dictionary size the
file's own header declares, which is about 9 MB all told for an ordinarily
compressed file. A file whose blocks do not leave room for all of that is read
through the streaming decoder instead. That is still correct and still complete; what
it costs is that reading *backwards* means decoding forward from the start of
the block again, which `query` does and `parse` never does.

Whether that happens is a question about *your* budget rather than about the
file alone: a 24 MiB-block file needs about 58 MiB, and one written by
`xz -9 -T0` — whose threaded blocks are about 192 MiB — needs about 400 MiB.
**`query` tells you when the budget is short**, once, on stderr, naming the
file's largest block beside the budget that declined it:

```
warning: this .xz source has 5700 block(s) to seek by, but its largest is 134217728 byte(s) and a memory budget of 67108864 byte(s) leaves no room for one reader of it — so it is read through the streaming decoder and every backward read decodes forward from its block's start; raise the memory budget to 278955808 byte(s) or more to read it a block at a time
```

If you have the memory, `--parallel-memory 536870912` buys the block path back.
That is the whole of it: the budget is a separate number from `--jobs`, so
raising it works at any worker count and you do not have to ask for a second
worker to make it count. If you do not have the memory, nothing is wrong —
the file reads fine, just with more decoding on backward reads.

**Where the default comes from, when you state nothing.** pgdq reads the
memory limit it is actually running under — the cgroup limit a container or a
systemd unit sets, taking the smallest that binds, including limits set above
you that your own cgroup does not show — and takes that minus a fixed 256 MiB
for everything a byte budget does not cover: threads, the allocator's own
retention, the program itself. So a container given 512 MiB scans inside
256 MiB and one given 3 GiB inside 2.75 GiB, without you restating on the
command line what you already told the orchestrator.

Two things follow, and both are deliberate. **A very small allocation gets a
very small budget rather than a floor**: at 256 MiB there is nothing left after
the reserve, and pgdq reads compressed input through the streaming decoder and
plain input a chunk at a time, which is correct and slower. And **where no
limit is set at all**, pgdq does not size itself from the machine's RAM: it
takes what the *file* needs — one reader's worth for each worker it would run,
which is about 1.4 GiB for a 24 MiB-block `.xz` on a 24-core host — and never
more than half of what the machine reports as available. A plain dump asks for
nothing, so it stays on the 64 MiB pgdq has always used.

If that is more than you want a flagless run to take, state
`--parallel-memory`; a number you type wins over anything discovered, in both
directions.

**`--jobs <n>` is how many workers pgdq may ask for. Left unstated, the file
decides.** A plain (uncompressed) dump reads serially, because splitting one is
slower than not splitting it. An `.xz` dump takes the CPUs this process was
given — the machine's cores, or fewer where a container quota says so, since
decompression is the one part of the work that a second core reliably finishes
sooner — and fewer still where the file itself has fewer blocks than that:
pgdq splits a compressed file at its block boundaries, so a file with six
blocks reads with six workers on a machine of any width. **How many of those workers actually read is then bounded by
`--parallel-memory`**: one reader of an ordinary 24 MiB-block file wants about
58 MiB, so a budget of 64 MiB delivers one worker whatever `--jobs` says. Left
unstated the budget is chosen to afford the count — but a budget *you* state,
or a small memory limit, is what decides how many of those workers there really
are. Whatever the file would choose, a `--jobs` you type wins outright, in
both directions: `--jobs 1` reads a compressed dump serially, and `--jobs 8`
splits a plain one.

It states what is asked for rather than what you get: two input shapes admit no
parallelism at all whatever you set, and a plain file gets fewer workers than
you named. Both are below. The `scan started` line says the count that was
resolved (below, "Status on stderr"), which is where to look if you want to know
what a flagless run chose.

For `query` it cuts the row reading up: the parts of the file holding the rows
you asked for are split into at most this many pieces, read at the same time,
and printed back in the file's own order — so the rows and their order are the
same at every setting, and only the reading changes. **A failure is the same at
every setting too**: if two rows in the file cannot be read, the one that comes
first is the one you are told about, whichever reader happened to reach its row
first, so re-running to confirm a failure names the same row again. What it buys today is
overlap in the *reading*; the rows are still turned into output one thread at
a time, so on a plain file on a fast disk raising it will not show up on a
clock. On a compressed file the reading is the expensive part, and there it
can.

> **A query's `--parallel-memory` covers two costs on a plain file and one on
> a compressed one.** Every piece costs what its worker holds to read. On a
> plain file it also holds its in-flight batch until that batch is handed
> over — up to 64 MiB worth, the same default `query` always batches to — so
> the number of pieces you get is `--parallel-memory` divided by *that sum*.
> Under a 64 MiB budget — which is what a plain file gets unless you raise it —
> the batch term alone accounts for the whole of it, so
> `query --jobs N` on a plain file runs one piece, serially, however large `N`
> is; two pieces need about 145 MiB. On an `.xz` file read a block at a time
> there is no second cost: the batch holds its rows inside the block its own
> worker already decoded, so the pieces are bounded by what one reader holds —
> two blocks, a read buffer and the decompressor's own working memory. That is
> about 58 MiB for a file of 24 MiB blocks, so a 64 MiB budget affords one
> piece there and `--parallel-memory` is what buys a second. `parse` carries no batch cost on
> either shape — it builds none — so its `--jobs` is bound by the decode cost
> as described above. When `--jobs` asks for more pieces than the budget
> affords, `query` says so on stderr, naming the terms it was charged and the
> budget that declined them, so you know which number to raise.

For `parse` it cuts the *inside* of a `COPY` block up. Once pgdq has read a
block's `COPY … FROM stdin;` header it knows everything until the block's end
marker is rows, so it hands that stretch out to this many readers at once and
folds their answers back into the one result — the same block list, the same
row counts, the same cache, whatever you set. A block too small to be worth
splitting is read the way it always was, so a dump of many small tables mostly
ignores the flag and a dump of a few huge ones mostly does not.

It also bounds how many decoded `.xz` blocks are kept at once — one per worker
you allowed, if `--parallel-memory` leaves room for them. So on a compressed
file `--jobs` and `--parallel-memory` are worth raising together: more workers
with no room to hold what they decode buys less than either number suggests.

> **On an `.xz` file, expect a parallel scan to hold more than
> `--parallel-memory` names.** The budget bounds what pgdq *keeps* between
> reads; each worker also holds the block it is decoding at that moment, and
> the budget is what decides how many workers there are. So the number to raise
> when a compressed scan is short of memory is `--parallel-memory`, and raising
> `--jobs` past what that budget affords adds workers pgdq will not use.

> **Under a container memory limit, leave room for the allocator as well.**
> glibc gives each thread that allocates its own memory arena, which it keeps
> rather than returns. pgdq runs a thread for each piece of work it has in
> flight, so raising `--jobs` raises the arena count with it. So size a cgroup
> above what `--parallel-memory` names rather than at it — a few hundred
> megabytes above it on a compressed file, which is roughly a fixed margin
> rather than something that grows with the budget you set. A flagless run
> already leaves that margin for itself — the 256 MiB reserve above — so this
> is advice about a budget *you* state. `MALLOC_ARENA_MAX` bounds the arena
> count if you want to set it, and 2 is the smallest useful value; how much it
> saves on a given workload is not something we can currently quote you a
> number for. **This is not only a compressed-file concern**: a plain file
> read with `--jobs` set holds roughly 8 MB more for each worker you allow,
> and that is arena retention rather than anything the budget names, so it is
> the one case where `MALLOC_ARENA_MAX=2` removes essentially all of it.
> Restricting the container's CPUs is a partial substitute at best:
> it lowers the count an `.xz` file picks when you state no `--jobs`, because
> that count is read from the CPU quota — but it does nothing to a `--jobs` you
> typed, and arena memory does not fall away in proportion to the thread count
> in any case.

**Two shapes will never get parallelism, whatever you set.** An `.xz` file with
a single block has no seam to split at — the warning above says so when you hit
it. And an `INSERT` run — a dump taken with `pg_dump --inserts` — has no
line-anchored statement boundary a second reader could start from, which is
unfortunate, because it is also the shape that costs the most: about five times
a `COPY` block's CPU per byte.

A plain (uncompressed) file *is* split, but do not expect much from it, and it
is the shape where the number you state is not the number you get. pgdq's read
buffers are pooled four deep on such a file, and a worker needs one to read
with, so a fifth worker waits for a fourth to finish: above `--jobs 4` you get
four. That ceiling costs little today, because pgdq's structure scan of a plain
file is already faster than any disk we have measured — on that shape the disk
is what you are waiting for, and raising `--jobs` moves a number that was not
the bottleneck.

### Status on stderr

`parse`, `info` and `query` all write a line to stderr the moment there is
something worth watching a long run for: an `.xz` file's seek-table walk, and
each of the two passes a `parse` (or a `query`'s mapping pass) makes over the
file — a bounded prepass that reads only far enough to find the header
metadata, then the real scan. Every line opens with an RFC3339 timestamp, so
it lines up with anything else read off the same clock — a `dmesg` entry, a
cgroup sample, an orchestrator's own log:

```
$ pgdq parse --source koji.dump.xz
2026-07-23T14:02:11.104382771Z  INFO seek table build started path=koji.dump.xz
2026-07-23T14:03:36.881940552Z  INFO seek table build complete path=koji.dump.xz streams=31150 blocks=31150
2026-07-23T14:03:36.882015206Z  INFO preamble scan started bytes=784019857152 chunk_size=1048576 jobs=24 memory_bytes=1636608768
2026-07-23T14:03:36.891402337Z  INFO preamble scan complete bytes=98304 reached_eof=false
2026-07-23T14:03:36.891455118Z  INFO scan started bytes=784019857152 resumed_from=98304 chunk_size=1048576 jobs=24 memory_bytes=1636608768
2026-07-23T14:47:52.317660814Z  INFO scan complete bytes=784019857152 reached_eof=true
```

**The two passes are named apart deliberately.** `preamble scan` is the
bounded prepass that stops at the first table's data — it is what
`--preamble-only` runs on its own, and what an ordinary `parse` or `query`
runs first whenever the header metadata is not already cached. `scan` is the
real read, the one that can run for an hour. A single uninterrupted `parse`
prints both, in that order: seeing `preamble scan complete` immediately
followed by a `scan started` naming a nonzero `resumed_from` is not a resume —
it is the ordinary shape of a cold run, the second pass picking up where the
first left off. A **genuine** resume looks different: the header metadata is
already in the cache, so the preamble pass does not run at all, and the log
opens straight on `scan started` naming the byte the interrupted run reached.

The seek-table lines only appear on a fresh `.xz` file — the walk they report
is what a cache's persisted table exists to skip (above, "`.xz` files are read
directly"). `scan started` names the arrangement `--jobs`/`--parallel-memory`
resolved to, once, so a log says what produced everything that follows it. This
is where a flagless run says what the file and the machine chose: `jobs=24`
above is an `.xz` dump taking the CPUs the process was given, where a plain dump
would say `jobs=1`. `memory_bytes` is the byte budget actually governing reads,
whether or not you asked for one — the number above is what twenty-four readers
of this file's 24 MiB blocks want, on a host with no memory limit set and the
room to allow it. Under a limit it would be that limit less the 256 MiB reserve,
or whichever of the two is smaller. It is marked `(default)` only on a run that
resolved to a single worker *and* found no limit to read, which is the one case
pgdq can currently tell "nobody asked" from "you asked for exactly that" in; a
stated `--parallel-memory` is always printed bare.

A query's mapping pass may print `scan complete` at the offset it stopped
rather than the file's end, once its target table is settled (`reached_eof=false`). Running `parse` against a file
that is already fully cached is not a scan and prints neither pass, matching
"costs nothing and says so" above.

**These times are diagnostics, never figures.** This project admits a
performance number only as a measurement taken under its own stated apparatus
(`docs/design/measurements.md`); a duration logged here is whatever your
machine and disk happened to be doing at the time, not something to quote as
a benchmark.

There is no flag yet to raise, lower, or silence this output — a `-vvv` and a
`--quiet` are on the list, unallocated.

## `info`: reporting what is known

```sh
pgdq info --source mydump.sql
```

```
Scan completion: 100% (48213911 bytes)

server version: 16.4
pg_dump version: 16.4
extensions: 2
user-defined types: 3

diagnostics:
    [info] TOC coverage: 213/224 span(s) attributed to a TOC entry

roles: app_user, backup
tablespaces: fast_ssd
object kinds:
    ACL: 4
    FUNCTION: 2
    INDEX: 5
    TABLE: 8
    TRIGGER: 1
    ...

public.accounts (12345 rows)
    columns: id integer, name text, balance numeric(10,2)
public.events (98765 rows)
    columns: id integer, occurred_at timestamp with time zone, payload jsonb

2 COPY block(s), 111110 row(s)
```

- **`Scan completion`** is the first line, always, and it is the *only* place
  coverage is stated — see "Reading a partial answer" below.
- **The header** is the dump's own preamble: the server and `pg_dump`
  versions it was taken with, and how many extensions and user-defined types
  it declares. The type count counts *types*, not statements — `pg_dump`
  writes some of them twice, and each such type is counted once. A dump taken
  with `--create` (or `pg_dumpall`) that touches more than one database
  repeats this block once per database, each under its own `database: <name>`
  line.
- **`diagnostics`** is anything worth telling you that isn't a table, role,
  or object: how much of the map is explained by `pg_dump`'s own per-object
  comments (`TOC coverage`), a cache whose recorded mtime no longer matches
  the file's (still used — mtime alone isn't reliable enough to invalidate
  on), or, in cache-only mode below, a reminder that you're looking at
  historical data. Nothing appears here on an unremarkable run beyond the
  coverage figure.
- **`roles`/`tablespaces`** list every role and tablespace the scan found
  referenced anywhere — an object's owner, a `GRANT`/`REVOKE`, a non-default
  tablespace assignment. Neither line appears if the dump doesn't reference
  any (the common case for a single-owner dump with no custom tablespaces).
- **`object kinds`** is a count of every kind of object the dump defines —
  read straight from `pg_dump`'s own per-object comments, so the kind names
  (`TABLE`, `INDEX`, `FK CONSTRAINT`, `MATERIALIZED VIEW`, ...) are
  `pg_dump`'s own vocabulary, not ours.
- **The table listing** is one entry per `COPY` block, in file order, with its
  row count and column list. This is the view to read when you're deciding
  what to query.

Add `--detail` to also see each block's byte offsets and, per column, what
it became: the Arrow type it resolved to, or — for a column that came back as
a string — why (see [type handling](type-handling.md) for what "resolved"
means and why a column sometimes isn't). On a compressed dump it also prints
the container's shape, described under "`.xz` files are read directly" above.

`--detail` also turns the `user-defined types` count into a listing of the
types themselves, one line each, in the order the dump declares them:

```
user-defined types: 7
    public.mood            enum: 'sad', 'ok', 'happy', 'has space', 'it''s fine'
    public.empty_enum      enum: (no labels)
    public.text_c          domain over text COLLATE pg_catalog."C"
    public.point2d         composite: x double precision, y double precision
    public.myrange         range over integer
    public.mybase          base type
    public.shellonly       shell type
```

That is every type, not only the enums, and each line carries whatever its
kind has to say: an enum's labels, a domain's base type and any `COLLATE`
clause, a composite's fields, a range's subtype. The two `pg_dump` never
writes but pgdq can still meet are spelled out rather than left blank —
`composite: (fields not parsed)` for a body pgdq could not read (that is the
one case that changes how a column of the type resolves), and `range (subtype
not parsed)`. A C-level type says `base type` or `shell type` because that is
genuinely all the dump records about it: the *server* knows how to parse its
values, and the dump does not say.

Nothing else in `info` names a user-defined type, so this is where you find
out that `public.mood` exists before going looking for what it holds.

An enum column also gets its declared labels, in the type's own order, on the
line beneath — the same list, repeated where you are already looking:

```
public.t_enum_domain (4 rows)
    columns: id integer, v_mood public.mood
    id: Int32
    v_mood: Dictionary(Int32, Utf8)
        labels: 'sad', 'ok', 'happy', 'has space', 'it''s fine'
```

That is the whole list, however long it is, and it is the answer to "what may
I write on the right of `--filter 'v_mood=…'`". Each label is quoted the way
the dump itself writes it — an interior `'` doubled — so a label with a space
or a comma in it can still be told apart, and so you can paste one straight in:
`--filter "v_mood='has space'"`. The quotes are optional for an ordinary
label; `--filter 'v_mood=sad'` is the same term.

Labels are listed for a plain enum column and for a domain over one. An enum
*inside* an array or a composite does not get them, and does not need them — a
filter cannot compare against a single label there anyway.

### When `info` says it cannot answer

`info` exits non-zero rather than scanning. Five things can go wrong, and they
are five different messages because they mean five different things:

| Message | What happened |
|---|---|
| `no cache at …` | You have not parsed this file yet. |
| `… is not a pgdq cache` | Something else is at that path. Check `--dqcache`. |
| `… was written by a different pgdq build` | The cache format changed under you. Pre-1.0 this happens; nothing is migrated. |
| `… has changed since it was parsed` | The dump file's size no longer matches. Every offset in the cache could be wrong. |
| `… records compression details that … contradicts` | The cache says this file is compressed and it is not, or the other way round. |

**The first three send you straight to `pgdq parse`; the last two do not.** The
split is whether the file at the cache path is worth keeping. For the first
three it is not — there is no cache there, or what is there is not one, or it is
one this build cannot read — so `parse` simply scans over it.

The last two say one thing two ways: *this cache was written from a different
file*. It is not "your cache went missing", it is "your file is not the file you
parsed". `parse` refuses both rather than scanning and writing over what it
found, because a cache that does not describe this file is a valid index for
*some* file. So both messages name the only two ways out — **remove it, or name
a different cache path** — and `parse` builds a fresh one once you have taken
either. There is no flag that overrides this.

`--dqcache none`, which for `query` means "ignore the cache", is rejected on
`info` — with nothing to read and no scan to fall back on, there would be
nothing left to report. If you reached for it because the directory beside the
dump is read-only, put the cache somewhere else instead:

```sh
pgdq parse --source /readonly/dump.sql --dqcache ~/dump.dqcache
pgdq info  --source /readonly/dump.sql --dqcache ~/dump.dqcache
```

## Reading a partial answer

A cache from an interrupted `parse` (or from `--preamble-only`) covers part of
the file, and `info` reports it rather than refusing:

```
Scan completion: 63% (494022873 bytes)

...
public.accounts (12345 rows)
    columns: id integer, name text, balance numeric(10,2)

1 COPY block(s), 12345 row(s)
```

**The part a user will not guess: the records are not themselves
provisional.** A partial index is missing *records*, not confidence. A `COPY`
block enters the map only once the scan has walked it end to end, so every
block listed — its row count, its columns, its offsets — is as final as it
would be after a complete parse. There is no half-known block; there are only
blocks past the frontier that are not there at all.

That is why coverage is stated once, at the top, and nothing below it carries a
caveat. Finish the `parse` and the listing grows; nothing in it changes.

**Column types included**, and that holds for a `pg_dumpall` or `--create`
dump with several databases too. Each database's `CREATE TABLE` statements sit
ahead of its own data, so a scan that banked any of a database's `COPY` blocks
has necessarily read that database's DDL first — and `parse` writes it down at
that point rather than only at the end. An interrupted parse of a
five-database dump comes back fully typed for the databases it finished, and
simply has no blocks yet for the ones it did not reach.

## Everything else in the file: `--map`

The table listing above only shows `COPY` blocks — the actual row data.
Everything else in the file (every `CREATE TABLE`, index, trigger,
publication, comment block, and stretch of framing) is there too; `--map`
lists all of it, in file order, instead of just the tables:

```
$ pgdq info --source mydump.sql --map
Scan completion: 100% (48213911 bytes)

[0, 35) framing
[35, 622) SCHEMA public
[622, 981) TABLE public.accounts
[981, 1054) CONSTRAINT public.accounts accounts_pkey
[1054, 49267) COPY public.accounts (12345 rows)
[49267, 49803) INDEX accounts_name_idx
...

312 span(s)
```

Every byte of the file shows up in exactly one line — that's a property pgdq
checks on every scan, not just a description of the output. Reach for
`--map` when you want to see what's actually in a dump beyond its tables (an
unusually large comment block, a publication you didn't know about, where a
particular index sits) or to narrow down where something looks off before
reaching for `--detail`'s finer detail on one specific block. On a partial
cache, the bytes past the frontier show up as a single `unscanned` entry.

A dump run with `--inserts`/`--column-inserts` shows a table's data as an
`INSERT run` entry instead of a `COPY` block — same idea, a table's rows
merged into one line, just written differently by `pg_dump`. A dump
containing large objects (`lo_create`/`lowrite` calls, not `COPY` data) shows
their whole region as a single `large objects` entry — pgdq accounts for the
bytes but does not read large-object contents; see
[`docs/design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)'s
"Large objects (BLOBs)" row if you need to know why.

## Scripting against the output: `--json`

`pgdq info --json` prints everything as one JSON object on stdout instead of
formatted text:

```sh
pgdq info --source mydump.sql --json | jq '.spans | length'
```

Alongside the file map it carries two things the text views state differently:

- **Coverage as components**, not as the rendered percentage —
  `scanned_through` and `total_size`, so you compute whatever ratio you want.
- **`compression`**, the container's shape — `container`, `streams`, `blocks`
  and `max_block_uncompressed` — or `null` for a plain dump. The same three
  numbers `--detail` prints on one line.
- **`resolution`**, one record per `COPY` block, with the per-column outcome
  `--detail` renders as prose. Each column carries its name, the declared
  PostgreSQL type, the outcome as a token (`mapped`, `varying_array_shape`,
  `metadata_not_scanned`, …), the Arrow type, and the nested plan. This is the
  only machine-readable form of "why is this column a string".

  There is no per-column `labels` array: the whole preamble is already in the
  object, so every user-defined type is under `metadata.databases[].types[]`
  with its kind and payload — an enum's labels included, unquoted, JSON having
  its own string encoding. Join a column's `declared` against that list and you
  get an answer for `public.mood[]` as well as for `public.mood`.

  Records are keyed by **block**, not by table — one table's data can occupy
  several `COPY` blocks, and pgdq does not yet have a rule for merging blocks
  that disagree, so grouping them is left to you.

**This is a raw dump of pgdq's internal representation, not a designed API.**
There's no schema, no compatibility promise across versions, no version field,
and no attempt to make the shape convenient — field names, nesting, and what's
included can all change as the underlying code does. Reach for it when you need
something the text views don't show (or don't show in a shape you can parse),
and expect to adjust your `jq`/script when you upgrade pgdq. `--json` can't be
combined with `--detail` or `--map`, since the full object already carries
everything those two format for a human.

## Inspecting a cache with the dump gone: no `--source`

`pgdq info` can answer entirely from a saved `.dqcache` file, with no dump
file in reach at all — deleted, moved elsewhere, or never local to this
machine. Drop `--source` and pass `--dqcache <path>` on its own:

```sh
pgdq info --dqcache mydump.sql.dqcache
```

This works for the default listing, `--map` and `--json` alike — whichever one
you'd run against the live file, partial caches included. It's for exactly the
sysadmin-facing use case this whole page is about: keep a folder of `.dqcache`
files from dumps you no longer keep around, and still be able to answer "what
tables did this have," "which roles did it need," "did the schema change since
last time," without the multi-hundred-gigabyte file itself.

The one thing this form cannot do is check the cache against anything. With
`--source`, pgdq compares the file's size against the cache's and tells you if
the file changed; with no `--source` there is nothing to compare, so every
cache-only answer carries a `diagnostics:` line saying it is unverified,
historical data from whenever the cache was last saved.
