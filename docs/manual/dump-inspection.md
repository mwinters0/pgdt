# Dump inspection

`pgdt` tells you what is in a dump file — tables, roles, indexes, functions,
everything `pg_dump` wrote — without decoding any row data. It takes two
commands, and the split between them is the thing to learn first:

```sh
pgdt parse --source mydump.sql   # reads the dump, writes mydump.sql.dtcache
pgdt info  --source mydump.sql   # reports what that cache holds
```

**`parse` reads your dump. `info` never scans it.** On a
5KB dump the difference is invisible; on a 784GB one, `parse` is an hour of
disk and `info` is instant. Splitting them means a command that reads like a
question — "what is in this file?" — can never turn into an hour of I/O you
did not ask for.

Run `parse` once per dump file. Everything after that is `info`, answered from
the cache, and `query`, which answers from the cache *and* the blocks it
needs — row data is never cached, so `query` always reads the file.

## `parse`: reading the dump

```sh
pgdt parse --source mydump.sql
```

It scans the file end to end, writes the cache beside it
(`mydump.sql.dtcache`), and prints the same listing `info` prints. Pass
`--dtcache <path>` to put the cache somewhere else — worth doing when the dump
sits on a read-only mount, since the default location is next to the dump.

**An interrupted parse is not wasted work.** The scan banks its progress at
`COPY` block boundaries as it goes, so a run killed at minute 50 of 60 leaves
most of those 50 minutes on disk. Run `parse` again and it picks up where it
stopped:

```
$ pgdt parse --source koji.dump
resumed a previous scan at byte 612000104448 of 784019857152
...
```

Resuming is the default, and there is no flag for the opposite: **delete the
cache file** if you want a scan from byte 0. Running `parse` against a file
that is already fully cached costs nothing and says so — unless it asks for
statistics the cache does not hold, or is `--postgres-invalid-values strict`
over tables no `strict` parse checked, when it re-reads only those tables
(see "`--statistics-level`" below).

The finished result is identical either way — a resumed scan and a
straight-through one produce the same index, byte for byte.

**A value PostgreSQL refuses stops it**, at the first, as it stops a restore of
the dump: `parse` fails naming the table, the column, the line — by the
number the restore's own error gives it and by its byte offset — and the value,
and the cache holds the scan as far as it last banked.
`--postgres-invalid-values ignore` goes on past each instead, recording them
in the cache, so a later `parse` without it fails on the first just the same
where it records statistics of that value's column.
`--postgres-invalid-values strict` checks every value rather than those it
reads for statistics, re-reading the tables the cache holds that no `strict`
parse checked — a resumed scan before it reads on — and, where the cache
already covered the whole file, saying so on the line above the listing.
Which values it checks, and what to do about one, is
[`type-handling.md`](type-handling.md), "When a value does not match its type".

**Ctrl-C stops it cleanly.** On `SIGINT` (Ctrl-C) or `SIGTERM` (`docker stop`,
`kill`), `parse` stops at the next block or chunk boundary, writes everything
it has scanned to the cache, says where it stopped, and then ends by that same
signal — which a shell reports as 130 or 143, and which stops a script that
called it, as Ctrl-C stops any other command there:

```
$ pgdt parse --source koji.dump
^C
interrupted at byte 41231843328 of 784019857152 — the cache at koji.dump.dtcache holds the scan so far
re-run `pgdt parse --source koji.dump` to continue
```

An interrupt that arrives before the first banking says so instead — `nothing
was scanned or written`, and the re-run it suggests starts from the beginning
rather than continuing. No cache file is left behind in that case, so nothing
is named. One that arrives after the scan has finished — while the listing
prints — finds nothing to stop: the cache is whole, and the process ends at
once, cutting the listing short; `pgdt info` prints it from the cache.

**Run as a container's init process** — `docker run image pgdt …` makes it
one — no signal sent to `pgdt` but `SIGKILL` can end it unhandled, so an interrupted
`parse` exits 130 or 143 instead of dying by the signal, and every command,
`parse` before its scan included, exits 128 plus the number of any signal that
would end it elsewhere: `docker stop`'s `SIGTERM` and Ctrl-\'s `SIGQUIT` among
them. A crash's signal — `SIGSEGV`, `SIGBUS` and the like — still ends it as a
crash; sent by hand, it is ignored.

A clean stop loses whatever the scan has done since it last banked, which is
the block it was reading plus any that finished in the moment before the
signal. A **second** Ctrl-C exits immediately without waiting for the write,
and so does `kill -9`, a power cut or a crash — they fall back to that same
last banking. `parse` banks often enough that the skipped work stays a small
fraction of the scan, and it spaces the bankings by what one costs rather than
by a fixed interval, because a scan that banked after every block would spend
more time banking than scanning. On a large dump, where blocks are minutes
apart, every block is banked and a clean stop loses only the one it was
reading. Nothing is ever left *corrupt*: a write is made beside the cache and
swapped in whole once finished, so a kill in the middle of one leaves the
previous banking in place, and a file named after the cache and ending `.tmp`
beside it, which is safe to delete. `pgdt info` states how far the cache goes.

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
pgdt parse --source mydump.sql.xz
pgdt info  --source mydump.sql.xz
pgdt query --source mydump.sql.xz --table public.widgets
```

Everything works exactly as it does on an uncompressed file: the same listing,
the same cache (written to `mydump.sql.xz.dtcache`), the same rows. Recognition
is by content, not by name, so a compressed dump called something other than
`.xz` is still read as one, and a file *named* `.xz` that is really plain text
is still read as plain text.

**Other compression is not read yet.** `.gz`, `.zst` and `.lz4` — including
what `pg_dump -Fp --compress=…` writes — still have to be decompressed before
pgdt sees them.

**One `.xz` file in three is slow to seek into, and pgdt says so when it is.**
An `.xz` file made of a single compressed block has nothing to seek by, so
reading anything but the start of it means decoding from the beginning:

```
diagnostics:
    [warning] this .xz source has no seek structure (1 block(s), one stream) — every read decodes the file from byte 0; recompress with `xz -T0` or `--block-size=<size>` for random access
```

`parse` pays it once: it reads forwards, and then once more from the start of
the file, at the end of the scan, to collect the schema text it saves. `query`
is where you would feel it, and only on a
large one — a single-block file the budget has room to hold four times over,
with room to spare for the decoder itself, is
decoded once and read from there, so only a bigger one pays the decode again on
every backward read. How big that is depends on the read-buffer budget, which
is carved out of your memory allowance (below, "`--jobs` and `--memory`"). The
remedy is in the message —
recompressing with `xz -T0` or an
explicit `--block-size` produces a file pgdt can seek into. Files that
`xz` produced with threads, or that were made by concatenating several `.xz`
files, are already seekable and earn no warning.

You can also ask a file what shape it is in, at any time and without reading
it: `pgdt info --detail` prints one line naming the container's blocks,
streams and largest block. It comes out of the cache, so it costs nothing and
works from a `--dtcache` alone:

```
$ pgdt info --source mydump.sql.xz --detail
compression: xz — 5700 block(s) in 1 stream(s), largest block 134217728 bytes uncompressed
```

`largest block` is what pgdt's read buffers have to clear **four times over**,
with the read buffer and the decompressor's own working memory beside it —
roughly 10 MB more, for the reasons below under "`--jobs` and `--memory`", where
`--memory` has to clear that *plus* a 384 MiB reserve. The
alternative way to learn it is `xz --list`, which on a file of many
concatenated streams reads every one of their footers.

Note that pgdt has to read the file's block index before it can read anything
else. On a file built from many concatenated streams — the shape a chunked
download or a `cat a.xz b.xz` produces — that index costs one disk seek per
stream, which on a very large file can be a minute or more. You pay it once:
`pgdt parse` saves the index in the cache beside the dump, and every command
after that takes it from there. A file compressed in one go with `xz -T0` or
`--block-size` never has the problem at all — its index is a single read
whatever the file's size.

If the cache stops matching the file it sits beside — you replaced the dump,
or pointed `--dtcache` at another file's cache — pgdt says so rather than
quietly working around it. All three commands refuse, `parse` included: a
cache that does not describe this file describes some *other* file, and
scanning would write over it. Delete it, or point `--dtcache` somewhere else,
and `parse` builds a fresh one — or, where the file really is replaced under
the same name, pass `--overwrite-unusable-cache` (below).

### Reading a dump over HTTP

`--source` takes a URL as readily as a path, on all three commands:

```sh
pgdt parse --source https://example.com/dumps/mydump.sql
pgdt info  --source https://example.com/dumps/mydump.sql
pgdt query --source https://example.com/dumps/mydump.sql --table public.widgets
```

Nothing is downloaded whole. pgdt asks the server for the byte ranges it
actually needs, so a `query` answered from a cache reads one block's worth of
bytes rather than the file. **The server has to support ranged requests**; one
that ignores `Range` and answers with the whole object is refused by name,
before any of it is fetched.

**The cache goes in the working directory.** With a local dump it sits beside
the file; a URL has no "beside", so the default is the URL's last path segment
plus `.dtcache`, in whatever directory you ran the command from —
`https://example.com/dumps/mydump.sql` becomes `./mydump.sql.dtcache`. `--dtcache
<path>` states somewhere else, and `--dtcache none` turns it off where the
command allows. A URL that names no object — a bare host, or a path ending in
`/` — is refused: there is no dump named there to read.

That default means two same-named dumps from **different hosts**, read in one
directory, share a cache file. If their sizes differ the second run refuses,
naming the URL it refused for; if they match, it reads the first one's map and
says on stderr that the cache was written for a dump fetched from somewhere
else. `--strict-identity=location` turns that warning into a refusal, and
`--dtcache <path>` keeps them apart in the first place.

**No credentials are sent, ever.** A URL carrying `user:password@` is refused
rather than quietly stripped, so nobody is left believing a password went out.
For a private object, use a **presigned URL**: the signature rides in the query
string, which pgdt preserves on every request it makes.

`http://` and `https://` are the schemes read over the network. `file://` is
accepted too and means a path on this machine, so having learned that
`--source` takes URLs you are not then wrong about the local case. Every other
scheme is refused by name. If you have a local file whose first path segment
contains a colon, write it `./that:file` so it is read as a path.

**A compressed dump is read over HTTP too.** A URL whose bytes turn out to be
`.xz` — whatever it is named, recognition being by content here as everywhere —
is read block by block, fetching each block's compressed bytes and decoding
them locally. What that costs depends entirely on the cache. With one, a query
is a cache read, an identity check and one block's bytes. **Without one, the
first run walks the file's stream footers, and that is several round trips per
stream** — a footer, an index, a header and a padding probe each: a dump
compressed as thousands of small streams costs tens of thousands of requests
before any row is read. pgdt says so on stderr before it starts. It is
paid once — keep the cache the run writes — and if you would rather not pay it
at all, fetch the file once and parse the local copy.

**A network failure stops the run**, naming the URL and what went wrong. It is
not treated as an interruption, because nobody asked for it — but a `parse`
banks its progress to the cache as it goes, so re-running it continues from
where the last save left off rather than from byte 0.

**Ctrl-C is answered at once**, rather than at the end of the request in
flight: that request is dropped where it stands, and the run reports and exits
exactly as an interrupted local `parse` does.

**This is correctness, not speed.** A remote read uses one reader by default
and the same 1 MiB requests a local read uses, both of which were chosen
against local devices; a compressed one fetches each block's compressed extent
whole. Nothing here is tuned for a network yet.

### `--strict-identity`: when a moved file should stop the run

Size is not the only thing pgdt knows about your dump — it also records the
file's modification time when it writes the cache. By default that time is
**advisory**: a dump that was copied to another machine, restored from backup
or simply `touch`ed still has the same bytes, and refusing to read it would be
refusing a file that is perfectly good. `parse`, `query` and `info` all say so
and carry on.

`--strict-identity` turns that into a refusal, for a pipeline where the file
genuinely should not have moved. It takes a comma-separated selection, and the
flag on its own means `time,location`:

```sh
pgdt query --source mydump.sql --table public.widgets --strict-identity=time
```

- **`time`** binds the modification signal: a local file's modification time,
  and for a URL the server's `Last-Modified` and its `ETag`: a strong tag
  decides it wherever both the cache and the server have one, then
  `Last-Modified`, and a weak tag (`W/"…"`, which promises equivalent content
  rather than the same bytes) only where neither side has a `Last-Modified`. A
  cache written against a different signal stops the run instead of reporting
  it — and so does a source that has none to offer at all, since the honest
  answer there is that it cannot give the guarantee you asked for. A weak tag
  can show a change and never confirm there was none, so tags that match where
  either is weak stop the run too, where without the flag they are not
  mentioned.
- **`location`** binds where a source was fetched from — the URL a remote cache
  records. A local file was not fetched from anywhere and records no origin, so
  two local runs always agree and this binds nothing there.
- **`advisory`** is the default, stated: nothing binds between runs and the
  check below stays on — the same as leaving the flag off.
- **`none`** binds nothing at all, and is the only way to turn off the check
  below.

`advisory` and `none` each stand alone, so naming either beside another term
is refused.

The flag asks the same question of all three commands, so `info` stops too
rather than reporting, and the refusal reads the same on each: the dump you
named, then the cache, what failed and the two modification times it compared
(seconds and nanoseconds since the Unix epoch, which `date -d @<seconds>` reads
back), which is more than the diagnostic says. The dump's name leads it because
a cache's own is not always enough — over HTTP the cache is named after the
URL's last segment, so `mydump.dtcache` does not say which `mydump` asked for
it. It does need a source to ask about: cache-only
`info`, with no `--source`, is answering from the cache alone and there is no
identity there to bind, so the flag is refused as a usage error.

**A file that changes while pgdt is reading it stops the run**, and `none` is
the only thing that turns that off.
That is a different question from the one above: between runs, a moved file is
usually the same bytes in a new place, but *during* a run, bytes changing
underneath a read that has already returned some of them cannot produce a right
answer — the map or the rows would be mixed from two versions of the file. So
pgdt checks as it banks the cache, once more when the run finishes, and
whenever a read fails or bytes do not parse — a file cut short or rewritten is
often first met that way — and stops if the file moved, saying so rather than
reporting the short read or the bad row. **Over HTTP the server does the checking**, on every
request: each ranged GET names the version the run opened on — its `ETag`, or
its `Last-Modified` where it sent no tag or only a weak one (`W/"…"`, which a
server will not match a version against) — so an object rewritten mid-scan is
refused on the first read after it happens rather than at the next save. A
server that sends neither, or a weak tag alone, gives nothing to name, so no
check could see a change: such a run is refused before it reads anything, and
`--strict-identity=none` reads it anyway, with no in-flight check at all.

```
$ pgdt parse --source https://example.org/mydump.sql
Error: https://example.org/mydump.sql: nothing can tell whether the dump changes while it is being read — the server states neither an entity tag nor a `Last-Modified`, so no read can be pinned to the version this run opened on — so the run was refused before reading it; pass `--strict-identity=none` to read it anyway
```

```
$ pgdt parse --source mydump.sql
Error: mydump.sql: the dump changed while it was being read — its stored size went from 4096 to 8192 byte(s) — so nothing was saved and no cache was removed; re-run against a file nothing is rewriting, or pass `--strict-identity=none` to read it anyway
```

**Nothing is saved and nothing is deleted.** The check says when the change was
noticed, never when it happened, so everything the run read is suspect and none
of it is written down; the cache already on disk describes the file as it was
and is left exactly as it is. Re-run once the file has settled, or pass
`--strict-identity=none` to get a warning instead of a stop.

**Rows already printed are not taken back.** `pgdt query` writes rows as it
reads them, so a query that stops this way may have printed some rows read
from the changed file first; the non-zero exit status and the error are what
say the output cannot be trusted, and a pipeline should check them before
using any of it.

A dump replaced by *rename* — the usual way a pipeline publishes a new one — is
not this case: pgdt goes on reading the file it opened, finishes the run it
started, and the new file is picked up by the next one.

### `--chunk-size`: you almost certainly do not need it

`parse` and `query` read the dump in 1 MiB pieces. `--chunk-size <bytes>`
changes that, and the reason to mention it at all is to say that the default
was chosen by measurement rather than by taste: over six sizes from 64 KiB to
16 MiB, on a SATA SSD, an NVMe drive and a RAM disk, 1 MiB was the fastest on
the NVMe — the only real device of the three whose speed a chunk size can
change at all — and neither size either side of it was an improvement. On the SATA SSD every size reads the same, the device being the
whole cost. On the RAM disk, which is not storage, 4 and 8 MiB come out slightly
ahead of 1 MiB; that is the per-chunk work rather than anything a disk
does, and it is not what the default is chosen on.

Two things are worth knowing if you change it anyway. **Small is slower**:
64 KiB costs about 50% more CPU than 1 MiB, because the per-chunk work is paid
sixteen times as often. **Large costs memory, and the cost levels off**: read
buffers are reused at whatever size you ask for, and the pool holds four of
them or the read-buffer budget's worth, whichever is fewer — so under a 64 MiB
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
34 MiB resident for every worker the budget allows — the block that worker is
decoding, the read buffer, and the decompressor's own working memory — over a
pool that keeps a block for every slot but the one being filled: three of them
at one worker, and one more for each worker past four. So the first worker on
such a file costs about 106 MiB, the next three about 34 each, and every one
after that about 58. That is what buys
reading the same block repeatedly for free; the block size is set when the file is compressed
(`xz --block-size=`), not when it is read. A file whose blocks leave the budget
no room for one such reader is read a different way — see `--memory` below.

**A parallel `query` may point you at this flag.** Where the memory budget
seats fewer sub-streams than `--jobs` asked for, the plan says a smaller read
chunk is what would seat more. That is true about seats and says nothing about
speed: `query` reads its sub-streams one after another once each has handed
over its first batch (see "`--jobs` and `--memory`"), so a smaller chunk buys a
larger count, the per-chunk cost above, and little measured gain. Raising `--memory` instead does not help there either — see
"`--jobs` and `--memory`".

The flag exists for a device unlike any of those three. If you have one and
find a size that beats 1 MiB on it, that is worth reporting.

### `--max-line-bytes`: a dump holding very large values

One row of a `COPY` block is one line of the file, and pgdt holds a line whole
while it scans it. So that a malformed file cannot grow that without bound,
`parse` and `query` refuse a line longer than 64 MiB, with an error naming the
byte offset it starts at. A dump that really does hold a value that large — a
`text` or `bytea` column of hundreds of megabytes — is read by stating a larger
limit on both commands:

```sh
pgdt parse --source big.sql --max-line-bytes 1073741824
pgdt query --source big.sql --table public.documents --max-line-bytes 1073741824
```

The limit is what one row may cost in memory, so raise it to what the file
needs rather than as far as it goes. It is checked as each read completes, so a
line up to one read chunk (`--chunk-size`) past the limit is held, and read or
refused by where the reads fall rather than by its length.

### `--statistics-level`: what `parse` records for later queries

`parse` records every table at one of two levels. **The metadata level** is
where the table's data lies and how many rows it holds, and nothing drawn from
those rows. **The data level**, the default, is what queries use: the shapes of
the table's array values — how many dimensions each array column's values
have, which its declared type cannot say — and **statistics**. A table at the
metadata level can still be queried: `query` reads its rows once more first,
every time, to learn those shapes, holds what it learned for that query alone
and writes nothing, printing one line to stderr as it starts —

```
2026-07-23T15:10:02.114820317Z  INFO table mapped at the metadata level: reading its rows again for this query's census table="public.orders" blocks=1
```

— and the DataFusion provider does not list it, naming the `parse` that would
record it at the data level. `--schema-mode strings` reads neither, so it
reads a metadata-level table as it reads any other. `info` says which level
each table's data was recorded at, on a `level:` line under it.

At the data level `parse` gathers statistics: for every stretch of each
table's data — a **row group**, one mebibyte of it, doubled for a table whose
data would take more than 4,096 groups until it takes no more, doubled
again for a table whose rows are wide, and halved for one too dense for a
stated maximum (both below) — the number of
rows, each column's number of NULLs, and, where pgdt compares a column's values
exactly, its least and greatest value and its distinct values (up to 64, none
longer than 256 bytes; past either, that group records no distinct values for
the column). A text column keeps its least and greatest value by bytes whatever
its collation; `query` reads them only under `C` or `POSIX`, where bytes are the
server's order, and a DataFusion query, which compares text by bytes, reads
them under any. A DataFusion query compares every column as the value it
receives, which for some types is not PostgreSQL's order — an enum by its
label's text, a bare `numeric` or an `inet` by its text, an `interval` by
months, then days, then time — so those columns keep a second least and
greatest value in that order; and a column `query` keeps none for — `jsonb`, a
`character(n)` outside `C`, a type pgdt does not order — keeps one by bytes,
which only a DataFusion query reads.
They are stored in the cache beside the rest of the index; `info --detail`
sums them per table and column, and `info --json` exports every group's (below), and
`query` reads them to skip what its filter rules out (below).

**A row group is Parquet's row group, used the same way**: a stretch of a
table's rows, across every column, whose statistics let a query skip it. Four
things differ.

- **It indexes the dump rather than holding data.** The rows stay in the dump
  as `pg_dump` wrote them and pgdt never rewrites it; a group is a range of its
  bytes, and its statistics live in the cache beside the rest of the index.
- **It is bounded by bytes, not rows.** A row belongs to the group its first
  byte lies in, so a group a long row spans from end to end holds no row at
  all, which a Parquet row group never does.
- **Its size can change after the fact.** A Parquet file's row groups are fixed
  when it is written; here a `parse` stating another size, minimum or maximum
  re-reads the table and replaces its groups (below).
- **It is much finer by default.** A mebibyte of text is nearer a Parquet
  page than a Parquet writer's usual row group of about a million rows —
  though a page holds one column, and a group every column of its rows.

Gathering reads every value of every column, so it costs a `parse` time, memory
and cache space that grow with the dump; the workers `--jobs` asks for gather
as they read, and record exactly what one worker would — bar a block several
of them read at once, whose statistics they can together run out of the
allowance for where one would not, so that it declines (below). The metadata
level reads no value at all — but under `--postgres-invalid-values strict`,
which reads every one — and scans as fast as the file allows:

```sh
pgdt parse --source big.sql --statistics-level metadata                  # nothing drawn from the rows
pgdt parse --source big.sql --statistics-level metadata,public.orders=data
pgdt parse --source big.sql --statistics-level data,public.items.blob=metadata
pgdt parse --source big.sql --row-group-size 65536                       # finer groups
pgdt parse --source big.sql --row-group-min-rows 4096                    # fewer, fuller groups
pgdt parse --source big.sql --row-group-max-rows 4096                    # more, emptier groups
```

The value is a level for every table, then comma-separated overrides, each a
table (`schema.table`, or a bare `table` matching any schema) or a single
column (`schema.table.column`) and its own level. The second line records
every table at the metadata level but `public.orders`; the third records every
table at the data level, `public.items` gathering statistics on every column
but `blob`. **The most specific entry naming a column decides its level** — a
column entry, then a qualified table, then a bare one, then the first entry —
so `metadata,public.items=data,public.items.blob=metadata` says the same as the
third line, and a table any of whose columns is at the data level has its
array shapes recorded. One table or column named twice is refused. A name is
split at its dots, so a quoted identifier containing one cannot be named.
`--row-group-size` states the bytes of data each group covers, a power of
two kept exactly however long the table: a smaller group records more finely
where values lie and costs memory and cache space in proportion.

**A table of wide rows gets fewer groups.** A group costs the same memory
whether it holds one row or thousands, so once a table's data is read, its
groups double until at most half of them fall short of
`--row-group-min-rows` rows, 1,024 by default, or the table is one group. Rows
up to about 1 KiB wide keep the mebibyte; a table averaging 4 KiB a row ends
at 4 MiB a group. `--row-group-min-rows 0` doubles nothing; a larger minimum
keeps fewer, fuller groups, which a query skips less precisely.

**`--row-group-max-rows` asks for the other side of that trade**, and there is
no default: state it and no group size is chosen that would put more than that
many rows in the 90th-percentile group, so at most a tenth of a table's groups
hold more. It wins wherever it and `--row-group-min-rows` cannot both be met,
and it lifts the 4,096-group ceiling as well — you asked for the groups, so
nothing quietly takes them away, and a table dense enough to need many of them
costs the memory and the cache space they take. A table too dense to meet it at
a mebibyte a group is read **again**, once the rest of the file is scanned, at
the finer size its own groups predict; if its rows cluster so that even that
misses, the run keeps what that read gave rather than reading it once more, and
every run under that maximum says so on stderr:

```
2026-07-23T15:10:09.570016894Z  INFO row groups still hold more rows than the stated maximum table="events" group_size=262144 max_rows=3000
```

A stated `--row-group-size` is exact, so it is refused beside
`--row-group-min-rows` and `--row-group-max-rows`, a maximum below the
minimum in force is refused, and none of the three is accepted where every
table is at the metadata level. No statistics flag combines with
`--preamble-only`, which reads no row.

**Asking for statistics the cache lacks re-reads what lacks them.** Once the
rest of the file is scanned, `parse` re-reads each table's data an earlier run
mapped without the statistics this one asks for — at the metadata level,
recording its array shapes as well, without the least and greatest values this
build keeps for a column, at a group size other than a
`--row-group-size` stated now, or under bounds other than a
`--row-group-min-rows` or `--row-group-max-rows` stated now — one `COPY`
block at a time, banking them as it goes, so an interrupted re-read continues
where it stopped. A block this run scanned is re-read too where it is too
dense for a stated maximum (above), that being the one thing a single read
cannot deliver. A block an earlier run gathered under other bounds can be read
twice in one run for that reason: first at a mebibyte a group, as a table is
gathered cold, then at the finer size that read's groups predict. A re-read keeps every column the block already had, and a
group size and bounds left unstated keep the size a block was gathered at, so
a flagless `parse` over a cache gathered at 65536 re-reads nothing. **Nothing
recorded is ever dropped**: a `parse` at the metadata level over a cache holding
the data level leaves it there. It prints
its count to stderr:

```
2026-07-23T15:10:02.114820317Z  INFO statistics back-fill started blocks=12
2026-07-23T15:10:09.570016894Z  INFO statistics back-fill complete blocks=12
```

**`parse` says how much memory the statistics held**, in one line on stderr
as it finishes, whether it gathered them, re-read them or only loaded them from
the cache:

```
2026-07-23T15:10:09.570412108Z  INFO statistics held bytes=121335995 peak_bytes=407126070 retained_peak_bytes=121335995 loaded_peak_bytes=126535120 gathering_peak_bytes=121346806 pieces_peak_bytes=0 interned_peak_bytes=160449920
```

`bytes=` is what the statistics held when `parse` finished and `peak_bytes=`
the most they held at once, counted as the sizes pgdt asked the allocator for
— the statistics alone, not everything the process holds. The rest are the
parts of it, each giving its own peak, reached at its own moment, so they do not
add up to `peak_bytes=`:

- `retained_peak_bytes=` — statistics gathered by this run, kept until it
  exits;
- `loaded_peak_bytes=` — statistics read from the cache;
- `gathering_peak_bytes=` — a table's statistics while its data is still being
  read;
- `pieces_peak_bytes=` — what the workers `--jobs` asks for gather from their
  stretches of a table before those are joined into the table's;
- `interned_peak_bytes=` — a second copy of each distinct value, kept while a
  table is read and let go when its data ends.

A `parse` with no statistics to hold — at the metadata level over a cache
holding none — prints no such line, and `query` never prints one.

**A block whose statistics will not fit the memory you allowed is skipped, not
gathered badly, and pgdt says which.** What the statistics of one run may hold
is what the allowance leaves once the workers and the fifth left free are paid
for ("`--jobs` and `--memory`" below), and the line naming it is on the
`resolved the arrangement` line as `statistics_bytes=`. A `COPY` block that
would pass it drops what it had gathered and gathers no more; the scan finishes
normally and stderr carries one line for the block. Nothing else declines
*because of* that block — but the allowance is filled cumulatively and nothing
releases what earlier blocks kept, so on a dump long enough to fill it every
block after that point declines too, and the statistics you get are a prefix of
the file rather than a sample of it:

```
2026-07-23T15:10:09.570412108Z  INFO statistics declined: this block's do not fit the allowance, and a larger --memory is what re-reads it table=public.wide header_offset=4823 declined_under_bytes=89128960 allowance_bytes=89128960
```

**Nothing is killed for want of statistics**, and the remedy is memory: a table
of a thousand text columns will never gather under a small container however it
is tuned, so raise `--memory` (or give the container more) and parse again.

**The cache records that it declined, and the number it declined under.** A
later `parse` at the same allowance or a smaller one leaves the block alone and
prints that line again rather than re-reading 300 GB to decline a second time;
one at a larger allowance re-reads it. `--row-group-size` and
`--row-group-min-rows` do not help here: how fine the groups are is your
choice and is never quietly changed to fit memory, because a cache must not
depend on the container that happened to write it.

A file rewritten in place at the same size since it was scanned is refused
here, with a message naming the dump, the block and the cache file, if a
re-read table's data no longer ends where the cache says it does: delete that
cache and parse again.

**`query` skips every group its statistics rule out.** Given a `--filter` or
`--where`, a group in which no row can satisfy it is never read, and stderr
says how much was skipped:

```
note: row-group statistics rule out 11 of 12 group(s), so 45092 of the 48592 byte(s) of rows this table holds are not read
```

**Data sorted on a column the filter bounds is read no further than the
bound.** Where the statistics record that a table's data holds a column in
ascending order, and the filter requires `<` or `<=` of it — alone, or joined
to other terms by `AND` — reading stops at the first row past the bound, since
no later row can satisfy it; descending order stops `>` and `>=` the same way.
A stop is found only as rows are read, so where one ended reading early stderr
says so after the rows, in bytes that add to the skipped groups':

```
note: reading stopped early in 1 block(s) sorted past the filter's bound, so a further 375 byte(s) of rows are not read
```

A stop that never came before a table's last row saved nothing, and prints
nothing.

The rows are the ones `query --statistics none` prints, which reads every row.
Two things are not the same: a value that fails to decode is reported only
where its row is read, so one in a skipped group or past a stopping row can go
unreported until `--statistics none` reads it; and the statistics are trusted as the rest of the
cache is, by the file's size, so a file rewritten in place at the same size
has stretches skipped by what they held before, and may lose rows it holds now
— delete the cache and parse again.

### `--jobs` and `--memory`: the workers and the allowance

`parse` and `query` take two more numbers: how many workers to ask for, and how
much memory the process may hold.

**`--memory <bytes>` is how much memory pgdt may hold resident — the number you
would give the container. Left unstated, pgdt reads the memory limit it is
actually running under.** A number you type replaces what was discovered and is
carved up in exactly the same way, so the two are one setting reached two ways:

- **384 MiB comes off the top** for everything a buffer budget does not cover:
  threads, the allocator's own retention, the program itself. What is left is
  the **read-buffer budget**, and that is the number every message below and
  every `memory_bytes=` on stderr names.
- **The worker count is then held so that a fifth of the whole allowance stays
  unspent**, because what kills a container is one run's peak.
- **pgdt takes inside that what the *file* asks for**, not the whole of it —
  one reader's worth for each worker it would run.
- **The statistics a cache already holds are counted before the workers**: a
  `parse` over a cache with statistics in it pays for them out of the same
  four fifths, so a large cache means fewer workers — and where the count
  cannot fall, a plain dump or a single worker, a smaller read-buffer budget.
  A `query` pays for them only while it maps what the cache does not yet
  reach: the rows it then reads hold none of them, so that part is sized as if
  the cache held no statistics at all.
- **What is left under that fifth is what a gathering `parse`'s statistics may
  hold**, the cache's own included, and a block whose statistics will not fit
  it is skipped rather than gathered — see "`--statistics-level`: what `parse`
  records for later queries" above. It is the `statistics_bytes=` on the `resolved the arrangement` line.

So `--memory 1073741824` in a 1 GiB container asks for exactly what that
container already told pgdt, and the read-buffer budget that follows is
640 MiB at most. A stated allowance is not a promise of extra room: it is the
same carve, which is what makes it a number an operator can size a container
from.

The one place the read-buffer budget bites is `.xz` input. A compressed file is
normally read a whole block at a
time, which is what makes reading the same block twice free — but that needs
room for **four** blocks at one worker: the one that worker is decoding, which
is the one it then keeps, and three more, because the pool keeps four slots, or
one per worker where you allow more. The decompressor's own working memory goes beside
them — mostly the dictionary size the file's own header declares, which is
about 9 MB all told for an ordinarily compressed file. A file whose blocks do
not leave room for all of that is read
through the streaming decoder instead. That is still correct and still complete; what
it costs is that reading *backwards* means decoding forward from the start of
the block again, which `query` does routinely and `parse` does once, at the end
of a scan, to collect the schema text it saves. A run that stops partway into a
block — a `query` whose table ends there — also decodes the rest of that block
before it finishes, because the block's integrity check covers all of it: a
damaged block fails the run with a non-zero exit, even where the rows read out
of it are already printed.

Whether that happens is a question about *your* allowance rather than about the
file alone: a 24 MiB-block file needs about 106 MiB of read buffers, and one
written by `xz -9 -T0` — whose threaded blocks are about 192 MiB, beside a
64 MiB dictionary — needs about 834 MiB, each on top of the reserve.
**`query` tells you when the budget is short**, once, on stderr, naming the
file's largest block beside the budget that declined it — and where that budget
came from, the stated allowance included, since the budget is never the number
you typed:

```
warning: this .xz source has 5700 block(s) to seek by, but its largest is 134217728 byte(s) and a memory budget of 67108864 byte(s) leaves no room for one reader of it — so it is read through the streaming decoder and every backward read decodes forward from its block's start; raise the memory budget to 547391264 byte(s) or more to read it a block at a time — the budget in force is 67108864 (discovered: /sys/fs/cgroup/memory.max states a limit of 469762048 byte(s))
```

If you have the memory, `--memory 1073741824` buys the block path back: 1 GiB
leaves 640 MiB of read buffers, which clears the 547391264 the message asks
for. That is the whole of it: the allowance is a separate number from `--jobs`,
so raising it works at any worker count and you do not have to ask for a second
worker to make it count. If you do not have the memory, nothing is wrong —
the file reads fine, just with more decoding on backward reads.

**Where the default comes from, when you state nothing.** pgdt reads the
cgroup limit a container or a systemd unit sets, taking the smallest that
binds, including limits set above you that your own cgroup does not show, and
carves it exactly as above. So a container given 512 MiB has 128 MiB to read
inside, and one given 3 GiB has 2.625 GiB, without you restating on the
command line what you already told the orchestrator.

**That number is a ceiling, not the budget.** What pgdt takes inside it is what
the *file* asks for — one reader's worth for each worker it would run, which is
about 1.3 GiB for a 24 MiB-block `.xz` on a 24-core host, so that file in the
3 GiB container reads at about 1.3 GiB and not at 2.625. **A plain dump asks
for nothing of its own and gets the 64 MiB pgdt has always used**, or the
ceiling where that is smaller — and that is true of a stated allowance as much
as a discovered limit, so `--memory` does not buy a plain dump larger read
buffers. `--chunk-size` is what changes what a plain read holds. And where the
ceiling affords fewer readers than the file
would have run, **the worker count comes down with the budget** instead of being
asked for and left undelivered: the same compressed file in a 512 MiB container
has a 128 MiB ceiling, which affords the 106 one block-decoding reader wants and
not the 140 two of them do, so it reads a block at a time with a single worker
and a budget of 106 MiB — and the run says as much before it starts (below,
"Status on stderr").

**The fifth left free is a second thing the worker count answers to.** pgdt
takes the largest worker count whose predicted total — the readers' own
buffers, plus 256 MiB for everything a scan holds outside them, plus the
statistics the cache holds — still fits in four fifths of the allowance, and reads with that many. In a 1 GiB container a
24 MiB-block `.xz` reads with ten readers rather than the eleven the ceiling
alone would buy. **Below about 640 MiB the fifth costs you nothing**, because
the 384 MiB already taken off the top is the tighter of the two; above it, it
is what stops a wider machine reading the same allocation with more readers and
less room. It never costs you the last reader: one worker runs at any allowance,
however small, and the budget reported is never more than that many workers
would hold — though where the ceiling cannot hold even one block-decoding
reader, the file is read through the streaming decoder and holds far less than
the budget reported. If you would rather have the readers than the headroom,
state a larger `--memory`: the headroom is a share of whatever number is in
force, so raising the number raises both.

Two things follow, and both are deliberate. **A very small allowance gets a
very small budget rather than a floor**: at 384 MiB or less there is nothing
left after the reserve, and pgdt reads compressed input through the streaming decoder and
plain input a chunk at a time, which is correct and slower. `query` says so on
stderr when it happens, naming the budget in force beside what one reader of
that file holds and what would lift it, so a slow run inside a tight container is never silent about
why it is slow. And **where no limit is set at all**, pgdt does not size itself
from the machine's RAM: it takes what the file asks for exactly as above, held
under half of what the machine reports as available — half rather than all
because that figure is an estimate two processes reading at once would each see
the whole of. There is no fifth held back there, so a cache's statistics do not
lower the workers either.

If that is more than you want a flagless run to take, state `--memory`; a
number you type wins over anything discovered, in both directions.

**`--jobs <n>` is how many workers pgdt may ask for. Left unstated, the file
decides.** A plain (uncompressed) dump reads serially: on every disk we have
measured, a single worker already reads such a dump at the speed the disk
delivers the bytes, and we have no measurement of several workers doing better
on one. State `--jobs` if you want workers on one anyway — from a RAM disk or a
page cache the file is already sitting in, where the disk is not the cost, a
second is worth about a third, and more give some of it back. An `.xz` dump on disk takes the CPUs this process was
given — the machine's cores, or fewer where a container quota says so, since
decompression is the part of the work that more cores finish sooner, from four
of them up: two workers read a compressed dump no faster than one — and fewer
still where the file itself has fewer blocks than that:
pgdt splits a compressed file at its block boundaries, so a file with six
blocks reads with at most six workers however wide the machine — and with
fewer on a machine narrower than that, the count being the smaller of the two.
That is what `parse` gains: a typed `query` reads a compressed dump's rows back
no faster with more workers than with one, and a plain one about a tenth
faster. An `.xz` dump read over HTTP takes one worker. **How many of those workers actually read is then bounded by
`--memory`**: one reader of an ordinary 24 MiB-block file wants about
106 MiB once the pool's four slots are counted, so a read-buffer budget of
64 MiB delivers one worker whatever `--jobs` says, reading through the
streaming decoder. Left unstated the budget is chosen to afford the count, and
where the allocation cannot afford it the count itself is lowered to what can
be paid for. **An allowance you state lowers a count the file recommended in
exactly the same way** — `--memory 469762048` on that dump leaves 64 MiB of
read buffers and reports one worker, not twenty-four, because one is what it
will get. Whatever the file would choose, a
`--jobs` you type wins outright, in both directions: `--jobs 1` reads a
compressed dump serially, `--jobs 8` splits a plain one, and neither is lowered
by any budget — how many of those workers really read is then decided from the
budget, as it always was.

It states what is asked for rather than what you get: two input shapes admit no
parallelism at all whatever you set, and a plain file gets fewer workers than
you named. Both are below. The `scan started` line says the count that was
resolved (below, "Status on stderr"), which is where to look if you want to know
what a flagless run chose — and where the budget or the file then delivers fewer
readers than that, a `scan arrangement` line beneath it says so and says what
would buy them back. The four-worker ceiling on a plain file, below, prints no
such line.

For `query` it cuts the row reading up: the parts of the file holding the rows
you asked for are split into at most this many pieces and printed back in the
file's own order — so the rows and their order are the same at every setting,
and only the reading changes. **A failure is the same at every setting too**:
if two rows in the file cannot be read, the one that comes first is the one you
are told about, whichever reader happened to reach its row first, so re-running
to confirm a failure names the same row again. **What it buys today is
little**: to keep the file's order while holding at most one batch from each
piece, `query` reads the pieces one after another once each has handed over
its first batch, so the extra workers read only those first batches at the same
time, on a plain file or a compressed one.

> **A query's read-buffer budget covers two costs on a plain file and one on
> a compressed one.** Every piece costs what its worker holds to read. On a
> plain file it also holds its in-flight batch until that batch is handed
> over — up to 64 MiB worth, the size `query` batches to when nothing is in its
> way — so the number of pieces you get is that budget divided by *that sum*.
> **The batch is what gives way, not the piece count**: under a 64 MiB budget —
> which is what a plain file gets whatever `--memory` says, or the ceiling where
> that is smaller, its reads asking for
> no budget of their own — 64 MiB batches leave room for one piece, so `query`
> shrinks the batches until the pieces you asked for fit, down to a floor of one
> read buffer, and says on stderr how big the batches ended up and how many
> pieces that bought. More pieces therefore cost batch size rather than being
> refused, and `--memory` is what buys the batch size back. On an `.xz` file read a block at a time
> no second cost is charged: the batch is counted as holding its rows inside the
> block its worker already decoded, so the pieces are bounded by what one reader
> holds — a block, a read buffer and the decompressor's own working memory, over
> the pool's four slots. That is about 106 MiB for the first piece on a
> file of 24 MiB blocks and 34 to 58 for each one after it, so a 64 MiB budget
> affords none of them there — the file is read through the streaming decoder
> instead — and `--memory` is what buys them back. A batch whose rows
> run across several decoded blocks keeps every one of them alive until it is
> handed over, and that is not counted, so a parallel `query` of a compressed
> file can hold more than that budget names. `parse` carries no batch cost on
> either shape — it builds none — so its `--jobs` is bound by the decode cost
> as described above. When `--jobs` asks for more pieces than the budget
> affords, `query` says so on stderr, naming the terms it was charged and the
> budget that declined them, so you know which number to raise.

For `parse` it cuts the *inside* of a `COPY` block up. Once pgdt has read a
block's `COPY … FROM stdin;` header it knows everything until the block's end
marker is rows, so it hands that stretch out to this many readers at once and
folds their answers back into the one result — the same block list, the same
row counts, the same cache, whatever you set.

**Raise it for a dump of a few large tables; leave it at `--jobs 1` for a dump
of many small ones.** pgdt cannot know where a block ends until it finds the
end marker, so each reader is given a stretch sized for a large block. Where
the block really is that large, the readers share it. Where it is much smaller,
they read past its end and that work is thrown away — and every reader you add
widens the stretch that gets thrown away, so on a dump of many small tables a
high `--jobs` is **slower** than `--jobs 1`, several times over, and reads many
times more from the disk. The answer is identical either way; only the time
differs. There is no line warning you about this, so if a scan of a
small-table dump is slower than you expected, try `--jobs 1`.

It also bounds how many decoded `.xz` blocks are kept at once — four, or one
per worker you allowed where that is more, if the read-buffer budget leaves
room for them. So on a compressed
file `--jobs` and `--memory` are worth raising together: more workers
with no room to hold what they decode buys less than either number suggests.

> **On an `.xz` file, expect a parallel scan to hold more than the
> read-buffer budget names.** The budget charges every reader the block it
> holds and the pool's other slots besides — three of them at up to four
> workers, and one short of the worker count above that — so what a scan holds beyond the
> budget is first the part no byte budget covers at all:
> threads, decoder state kept outside the pools, and the allocator's own
> retention, which the callout below is about. **Asking for more workers than
> the budget affords adds a second part**: the pool keeps a block for each
> worker you asked for, not only for the ones that read, and that part grows
> with the budget — at `--jobs 24` a compressed scan holds a few megabytes more
> than a 128 MiB budget and a few hundred more than a 512 MiB one. The number
> to raise when a compressed scan is short of memory is still
> `--memory`, since it is what decides how many readers there are;
> raising `--jobs` past what it affords adds workers pgdt will not use, and
> blocks it will keep for them.

> **Under a container memory limit, leave room for the allocator as well.**
> glibc gives each thread that allocates its own memory arena, which it keeps
> rather than returns. pgdt runs a thread for each piece of work it has in
> flight, so raising `--jobs` raises the arena count with it. So size a cgroup
> at what `--memory` names rather than at the read-buffer budget: the reserve
> and the fifth are exactly what the difference between the two is for, and
> they are taken from a stated allowance and a discovered limit alike. Where
> `--jobs` asks for more workers than the budget affords, the pool keeps a
> block for each of them (above), which is the one term that margin was not
> sized against. `MALLOC_ARENA_MAX` bounds the arena
> count if you want to set it, and 2 is the smallest useful value. **It gives
> real memory back on a compressed scan.** What it saves shows up where many
> block-decoding readers run and grows with how many there are, and it is no
> substitute for sizing the cgroup above the budget. It is a trade rather than
> free memory: fewer arenas than readers means those readers contend for the
> allocator, on exactly the scans where the cap saves anything. **A
> plain file is a different matter and needs nothing**: read with `--jobs` set
> it holds a few megabytes whether you allow two workers or twenty-four, so
> there is nothing there for the cap to take back. How much it is worth on your
> own compressed workload is not a number we can quote you yet.
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
is the shape where the number you state is not the number you get. pgdt's read
buffers are pooled four deep on such a file, and a worker needs one to read
with, so a fifth worker waits for a fourth to finish: above `--jobs 4` you get
four. What that ceiling costs has not been measured on its own; on every disk
we have measured, a single worker's structure scan of a plain file already
keeps up with the disk, so on that shape the disk is what you are waiting
for.

### Status on stderr

`parse`, `info` and `query` all write a line to stderr the moment there is
something worth watching a long run for: an `.xz` file's seek-table walk, and
each of the two passes a `parse` (or a `query`'s mapping pass) makes over the
file — a bounded prepass that reads only far enough to find the header
metadata, then the real scan. Every line opens with an RFC3339 timestamp, so
it lines up with anything else read off the same clock — a `dmesg` entry, a
cgroup sample, an orchestrator's own log:

```
$ pgdt parse --source koji.dump.xz
2026-07-23T14:02:11.104297118Z  INFO no memory limit found: nothing is enforcing one on this process jobs_flag=(not stated) memory_flag=(not stated)
2026-07-23T14:02:11.104382771Z  INFO seek table build started path=koji.dump.xz
2026-07-23T14:03:36.881940552Z  INFO seek table build complete path=koji.dump.xz streams=31150 blocks=31150
2026-07-23T14:03:36.881975330Z  INFO resolved the arrangement jobs=24 (recommended by the source) memory_bytes=1435282176 (no limit found: what this source asks for) statistics_bytes=25087383552 (half of what the machine reports available)
2026-07-23T14:03:36.882015206Z  INFO preamble scan started bytes=784019857152 chunk_size=1048576 jobs=24 memory_bytes=1435282176
2026-07-23T14:03:36.891402337Z  INFO preamble scan complete bytes=98304 reached_eof=false
2026-07-23T14:03:36.891455118Z  INFO scan started bytes=784019857152 resumed_from=98304 chunk_size=1048576 jobs=24 memory_bytes=1435282176
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
directly").

**Two of those lines say what this run is going to do, and where each number
came from.** Both are printed by `parse` and by `query` alike, and they are two
rather than one because half of the answer needs the file and half does not.

The **first** comes before anything is read, so a mistyped flag is confirmed
straight away rather than after an `.xz` file's seek-table walk — which on the
dump above is the minute and a half between it and the next pair. It opens with
which of two situations pgdt found itself in: `running inside a stated memory
allocation`, naming the limit in `limit_bytes` and the file that set it in
`limit_read_from`, or `no memory limit found: nothing is enforcing one on this
process`. That distinction is the whole reason the pair exists — under an
allocation somebody set, pgdt fills it; with nothing set, pgdt is a guest on a
machine nobody promised it and stays inside half of what the kernel reports
free, which can quietly buy fewer readers than the file could have used. Beside
it are your two flags exactly as typed — `jobs_flag` and `memory_flag`,
reading `(not stated)` where you gave none — which is
the line to check a value against when a run does something you did not expect.

The **second**, `resolved the arrangement`, comes once the file has been opened
and asked what it recommends. It is the only one that can report a count cut to
fit, since the cutting is that recommendation meeting the budget.

`jobs` is the worker count, and it reads differently depending on who chose
it. `(recommended by the source)` is the flagless case above: an `.xz` dump
takes the CPUs the process was given, where a plain dump would say `jobs=1
(recommended by the source)`. Where the memory available could not afford that
many readers it says so — `(recommended by the source; lowered from 24 by the
allocation)` — and that lowered number is what will actually run. A
`--memory` you typed lowers it the same way, and the clause then reads
`by the stated allowance`, so the number to raise is named rather than left to be
guessed at. A `--jobs` you typed is printed `(stated)` and is never lowered: it
says what was asked for, and how much of it the budget delivers is decided
later.

`memory_bytes` is the byte budget actually governing reads, whether or not you
asked for one, and it names its own origin the same way:

- `(stated: --memory allows N resident byte(s))` — what your `--memory`
  leaves for read buffers, with the number you typed beside it, the two never
  being the same.
- `(discovered: <file> states a limit of N byte(s))` — what this *dump* asks
  for, inside a cgroup limit less the 384 MiB reserve and the fifth of the
  limit a flagless run leaves free; it is the smallest of the three and not the
  ceiling itself. The cgroup file is named because a
  `memory.high` throttle and a `memory.max` kill are different things and either
  can be set on a parent cgroup you did not create.
- `(no limit found: what this source asks for)` — the number above: what
  twenty-four readers of this file's 24 MiB blocks want, taken whole because
  nothing capped it.
- `(default: no limit found)` — 64 MiB, which is what an unlimited host leaves
  a plain file, whose reads ask for no budget of their own.

`statistics_bytes` is what a gathering `parse`'s statistics may hold, carved
from the same allowance once `memory_bytes` is spent, with where the allowance
came from beside it: `(stated)` for a `--memory` you typed, `(discovered)` for
a cgroup limit, and `half of what the machine reports available` for a host
that set none. A block whose statistics would pass it declines, and says so
(above, "`--statistics-level`: what `parse` records for later queries"); on the one
host that states no limit and whose available memory cannot be read it reads
`(none: …)` and nothing declines. `query` gathers nothing, so the number binds nothing there.

**This number bounds the whole scan, not each block in turn**, and what a
finished block gathered is held until the scan ends. So on a dump long enough
to reach it, the blocks before that point keep their statistics and **every
block after it declines** — a later query then prunes over the early part of
the file and reads the rest whole. A run that does this says so once per
declined block. Raising `--memory` moves the point where it happens, and is
what re-reads the blocks that declined.

`scan started` below repeats the two resolved numbers without the provenance
clauses — it keeps the bare `(default)` marker where no budget was stated or
discovered — so a log line naming a scan says what produced everything that
follows it.

**`scan arrangement` is what says how many readers really ran.** `scan
started`'s `jobs=` is the count `resolved the arrangement` announced — except
on a `query` over a cache holding statistics, where `resolved the arrangement`
names what the rows are read under and `scan started` names the mapping that
comes first, which pays for those statistics and so may run fewer workers on a
smaller budget. That mapping pass's own lines say so: each is prefixed
`mapping{held_bytes=… replay_jobs=9}:`, `held_bytes` being what the cache's
statistics hold and `replay_jobs` the count `resolved the arrangement` named,
present only where the mapping pass runs fewer. A query the cache already
settles runs no mapping pass and prints none of them. Mind the prefix when
searching a log for `jobs=`: `replay_jobs=9` contains it. Two things can still
cut the count: a compressed dump whose largest block the
budget cannot hold is read through the streaming decoder and is **serial
whatever `--jobs` said**, and a budget too small for the readers asked for buys
fewer of them. Either prints one line, once per scan:

```
2026-07-23T14:03:36.891455118Z  INFO scan started bytes=784019857152 resumed_from=98304 chunk_size=1048576 jobs=24 memory_bytes=67108864
2026-07-23T14:03:36.912771904Z  INFO scan arrangement jobs=1 asked=24 bound_by="source" would_hold_bytes=111183648
```

`jobs=` is what is running, `asked=` is what `scan started` announced,
`bound_by=` is which of the two cut it — `source` for a container path the
budget could not afford, `budget` for readers it could not afford — and
`would_hold_bytes=` is what the arrangement that was refused would have held,
which is the read-buffer budget to raise `--memory` past. **No such line means the
count was started as announced.**

**What no line means is that the count was started — not that every worker
read at once, nor that it was the right count.** On a plain file above
`--jobs 4` the workers past the fourth wait their turn for a read buffer (see
`--jobs` above). On a dump of many small tables every reader you asked for does run,
and reading past the end of each small block is what most of them spend their
time on, so the scan is slower than `--jobs 1` with nothing printed to say so.
See `--jobs` above.

A query's mapping pass may print `scan complete` at the offset it stopped
rather than the file's end, once its target table is settled (`reached_eof=false`). Running `parse` against a file
that is already fully cached is not a scan and prints neither pass, matching
"costs nothing and says so" above; one re-reading blocks for statistics they
lack prints the two `statistics back-fill` lines shown under "`--statistics-level`",
and one holding statistics at all ends on the `statistics held` line described
there.

**These times are diagnostics, never figures.** This project admits a
performance number only as a measurement taken under its own stated apparatus
(`docs/design/measurements.md`); a duration logged here is whatever your
machine and disk happened to be doing at the time, not something to quote as
a benchmark.

There is no flag yet to raise, lower, or silence this output — a `-vvv` and a
`--quiet` are on the list, unallocated.

## `info`: reporting what is known

```sh
pgdt info --source mydump.sql
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
    level: data
    columns: id integer, name text, balance numeric(10,2)
public.events (98765 rows)
    level: metadata
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
  on, so it is reported unless you pass `--strict-identity=time`, which stops
  the run instead; see "`--strict-identity`" above), or,
  in cache-only mode below, a reminder that you're looking at historical data. Nothing appears here on an unremarkable run beyond the
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
  row count, the level `parse` recorded it at (see "`--statistics-level`"
  above) and its column list. This is the view to read when you're deciding
  what to query. A block holding values PostgreSQL refuses that a `parse
  --postgres-invalid-values ignore` went past says so on a `refused by
  PostgreSQL:` line per column holding one: the column, how many, and the
  first by its `COPY` line, value and offset ([type
  handling](type-handling.md), "When a value does not match its type"). One a
  `parse --postgres-invalid-values strict` checked says so on a `checked:
  every value, by a strict parse` line.

Add `--detail` to also see each block's byte offsets and, per column that has
something to say, what it became: the Arrow type it resolved to, or — for a
column that came back as a string for a reason — why (see [type
handling](type-handling.md) for what "resolved"
means and why a column sometimes isn't). A column that is simply text says
nothing and prints no line. On a compressed dump it also prints
the container's shape, described under "`.xz` files are read directly" above.

`--detail` also turns the `user-defined types` count into a listing of the
types themselves, one line each, in the order the dump declares them:

```
user-defined types: 7
    public.mood        enum: 'sad', 'ok', 'happy', 'has space', 'it''s fine'
    public.empty_enum  enum: (no labels)
    public.text_c      domain over text COLLATE pg_catalog."C"
    public.point2d     composite: x double precision, y double precision
    public.myrange     range over integer
    public.mybase      base type
    public.shellonly   shell type
```

That is every type, not only the enums, and each line carries whatever its
kind has to say: an enum's labels, a domain's base type, any `COLLATE`
clause and a `NOT NULL`, a composite's fields, a range's subtype. The two `pg_dump` never
writes but pgdt can still meet are spelled out rather than left blank —
`composite: (fields not parsed)` for a body pgdt could not read (that is the
one case that changes how a column of the type resolves), and `range (subtype
not parsed)`. An enum whose labels the dump changes in a way pgdt does not
read lists those it did with `(labels not read exactly)` after them, or says
`enum: (labels not read)`; see [type handling](type-handling.md#when-a-value-does-not-match-its-type)
for what that changes. A C-level type says `base type` or `shell type` because that is
genuinely all the dump records about it: the *server* knows how to parse its
values, and the dump does not say.

Nothing else in `info` says what a user-defined type is — a column names only
its declared type — so this is where you find out what `public.mood` is
before going looking for what it holds.

An enum column also gets its declared labels, in the type's own order, on the
line beneath — the same list, repeated where you are already looking:

```
public.t_enum_domain (4 rows)
    level: data
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

#### Values a column's type cannot hold

A typed column can hold a value PostgreSQL accepts and its Arrow type cannot:
`infinity` or `-infinity` in a `date`, `timestamp`, `timestamptz` or
`interval`, `NaN` in a `numeric(p,s)`, `time` `24:00:00`, an `interval` whose
time part is longer than about 2562047 hours, or a timestamp past
`294247-01-10`. A table recorded at the data level counts them, per column,
and `--detail` says how many beneath the column's type:

```
public.t_date (7 rows)
    level: data
    columns: id integer, v_date date
    id: Int32
    v_date: Date32
        unrepresentable: 2 value(s) Arrow cannot hold
```

A `date` or timestamp past `262142-12-31` is held by its Arrow type and
cannot be displayed by a DataFusion query, which formats dates through a
calendar ending there; the line counts those apart, as `N past 262142-12-31,
which a DataFusion query cannot display`. A value inside an array, a range or a
composite makes the whole value one such value, counted once. The line appears
only where the column holds one. A query reads each such value as NULL, the
dates past that calendar too in a DataFusion query, and says how many a column
it prints holds; `pgdt query --unrepresentable text` instead prints such a
column as its text, and `--unrepresentable refuse` refuses a query printing
one before a row is read, naming it and the count
([type handling](type-handling.md), "A value its column cannot hold reads as
NULL").

`--detail` closes its listing, above the totals, with what `parse` gathered
(see "`--statistics-level`" above), one line per table and one beneath it per column:

```
statistics:
    public.ordered: statistics over 1 of 1 block(s), group size 4096 bytes; 83 rows and 4049 bytes per group over 12 group(s)
        id: over 1 of 1 block(s), ascending, bounds in 12 of 12 group(s), dictionary in 0 of 12 group(s)
        default_text: over 1 of 1 block(s), unsorted, bounds in 12 of 12 group(s), dictionary in 12 of 12 group(s)
        note: not gathered
```

- **The table line** says over how many of the table's blocks statistics were
  gathered — every block whose `COPY` names the table, which includes a
  partitioned table's partitions only in a dump taken with
  `--load-via-partition-root` — the group size they were gathered at, and how many
  rows and bytes a group actually held, on average. A group in which no row
  starts, left by a row longer than the group size, is counted as `empty` and
  left out of the averages and of every share below.
- **Each column line** says over how many blocks that column was gathered,
  then its **order**: `ascending`, `descending` or `unsorted` row by row
  through each block (a column whose blocks differ says how many are which),
  followed by how many groups carry a least and greatest value — or `no bounds`
  where pgdt keeps none for the column, an array or a composite for one; a
  text column's are by bytes, and its order too, whatever its collation, as
  are those of the columns only a DataFusion query reads (above) and of a
  column no `CREATE TABLE` declares, which is every column of a
  `--data-only` dump. A second
  set, in a DataFusion query's order, is not summed here. Last, how many groups carry a list of
  distinct values, or `no dictionary` where it does not compare them exactly.
  A group holding only NULLs carries no bounds. `not gathered` is a column
  `--statistics-level` left at the metadata level.

A cache whose blocks carry no statistics at all prints `statistics: none gathered`. No
group's own values are shown here; `--json` carries every one of them.

### When `info` says it cannot answer

`info` exits non-zero rather than scanning. Seven things can go wrong, and they
are seven different messages because they mean seven different things:

| Message | What happened |
|---|---|
| `no cache at …` | You have not parsed this file yet. |
| `… is not a pgdt cache` | Something else is at that path. Check `--dtcache`. |
| `… is cut short or damaged` | A pgdt cache, but not all of it is there. |
| `… was written by a different pgdt build` | The cache's format version is not this build's. Pre-1.0 this happens after an upgrade; nothing is migrated. |
| `… has changed since it was parsed` | The dump file's size no longer matches. Every offset in the cache could be wrong. |
| `… records compression details that … contradicts` | The cache says this file is compressed and it is not, or the other way round, or the seek table it recorded does not fit the file. |
| `… counted the values a query cannot display against a calendar ending …` | The cache counts the dates a DataFusion query cannot display (below, "Values a column's type cannot hold") against a calendar that ends on another day than this build's, which only a library upgrade moves. Its counts are not this build's. |

**Only the first sends you straight to `pgdt parse`.** `parse` and `query`
refuse every other one too, before reading more of the dump than its first
bytes, rather than scanning and
writing over what they found: each is almost always a wrong path or a file that
changed, and a cache that does not describe this file is a valid index for
*some* file. So the last five name three ways out — **remove it, name a
different cache path, or pass `--overwrite-unusable-cache`** — and `parse`
builds a fresh one once you have taken any of them.

`--overwrite-unusable-cache`, on `parse` and `query`, starts cold over such a
cache and replaces it. It is for a dump whose file is replaced under the same
name — a nightly export at a stable URL — where a refusal each time would be
noise. It never replaces something that is not a pgdt cache, which could be
anything, the dump itself included; and it governs only the cache found at the
start, a dump that changes while it is being read still stopping the run.

`--dtcache none`, which for `query` means "ignore the cache", is rejected on
`info` — with nothing to read and no scan to fall back on, there would be
nothing left to report. If you reached for it because the directory beside the
dump is read-only, put the cache somewhere else instead:

```sh
pgdt parse --source /readonly/dump.sql --dtcache ~/dump.dtcache
pgdt info  --source /readonly/dump.sql --dtcache ~/dump.dtcache
```

## Reading a partial answer

A cache from an interrupted `parse` (or from `--preamble-only`) covers part of
the file, and `info` reports it rather than refusing:

```
Scan completion: 63% (494022873 bytes)

...
public.accounts (12345 rows)
    level: data
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
caveat. Finish the `parse` and the listing grows; nothing in it changes but
an array column's resolution, which a partial map reads optimistically until
every block's array shapes are in.

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
$ pgdt info --source mydump.sql --map
Scan completion: 100% (48213911 bytes)

...

[0, 35) framing
[35, 622) SCHEMA public
[622, 981) TABLE public.accounts
[981, 1054) CONSTRAINT public.accounts accounts_pkey
[1054, 49267) COPY public.accounts (12345 rows)
[49267, 49803) INDEX accounts_name_idx
...

312 span(s)
```

Every byte of the file shows up in exactly one line — that's a property pgdt
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
their whole region as a single `large objects` entry — pgdt accounts for the
bytes but does not read large-object contents; see
[`docs/design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)'s
"Large objects (BLOBs)" row if you need to know why.

## Scripting against the output: `--json`

`pgdt info --json` prints everything as one compact JSON object — a single
line — on stdout instead of formatted text:

```sh
pgdt info --source mydump.sql --json | jq '.spans | length'
```

It is the whole cache file. Beside the file map is what the cache records about
itself and the dump it was written from: `format_version`, the cache's on-disk
version; `container_kind`; `identity`, holding under `LocalFile` the dump's
`stored_size` in bytes on disk and its `mtime` as `[seconds, nanoseconds]` when
the cache was saved, which a dump is checked against before its cache is used;
`seek_table`, holding under `Xz` an `.xz` dump's every stream and block
with their offsets and sizes, or `null` for a plain dump; and `calendar_end`,
the last day, as days from 1970, of the calendar its unrepresentable counts
were taken against (below). On top of that it
carries three things the text views state differently:

- **Coverage as components**, not as the rendered percentage —
  `scanned_through` and `total_size`, so you compute whatever ratio you want.
- **`compression`**, the container's shape — `container`, `streams`, `blocks`
  and `max_block_uncompressed` — or `null` for a plain dump. The same three
  numbers `--detail` prints on one line, read off `seek_table`.
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
  several `COPY` blocks, and pgdt does not yet have a rule for merging blocks
  that disagree, so grouping them is left to you.

The file map's own `COPY` blocks carry their **`array_shapes`**, one per
column, `null` for a block recorded at the metadata level, their
**`unrepresentable`** counts, one `{format, engine}` pair per column and
`null` exactly where `array_shapes` is — `format` counting the values the
column's Arrow type cannot hold and `engine` those it holds past the calendar
end the cache's `calendar_end` states, as days from 1970 — and their
**`statistics`**, exactly as the cache holds them — `null` for a block `parse`
gathered nothing for. Each has its `group_size`, a `groups` array giving every
group's `rows` and `bytes`, and one entry per column of the block's header,
`null` for a column no `parse` that gathered the block kept at the data level: the column's `declared_type` and `collation`, its
`null_counts` per group, `bounds` (a block-wide `sortedness` and per group a
`min`, `max`, `min_exact` and `max_exact`, or `null` — an `_exact` flag is
`false` where a value too long to store was cut to a prefix below it or a
successor above it — over the values the column's Arrow type can hold; and
beside them `every`, the same over every value in PostgreSQL's order, where
some group holds one the type cannot, and `displayable`, over the values
within the calendar end as well, where some group holds one past it, each
`null` otherwise and, where present, `null` for every group holding no such
value, whose bounds are `bounds`' own, and for one holding no value it takes), `datafusion_bounds` (the same, in a
DataFusion query's order, for a column whose PostgreSQL order is another, and
`null` for every other column), `unrepresentable` (per group, the block's
`{format, engine}` count above, for a column some group of which holds such
a value, and `null` for every other), `dictionary` (the block's
distinct `entries` once each, and per group a list of indices into them, or
`null`), `sums` (per group, the sum of its values — a `numeric(p,s)`'s
counted in units of `10^-s`, a `NaN` left out as a NULL is — for an integer
or `oid` column and a
`numeric(p,s)` of at most 38 digits, and
`null` for every other column and for one holding a value that is not its
type; a sum wraps at 128 bits, which can be past what a JSON reader parses
as a number), and `value_bytes` (per group, the bytes of its values' text,
for every column). Every per-group array is
as long as `groups`. Nothing is summed per
table the way `--detail` sums it; that is yours to do, and the export grows
with the dump — every group of every column is in it.

Beside it is **`statistics_declined`**, `null` for a block that declined
nothing and otherwise the statistics allowance the block's statistics would not
fit (above, "`--statistics-level`: what `parse` records for later queries"). It is
how a script tells a table nobody asked statistics for from one that asked and
was refused the memory, and the number in it is the one to parse with more
than. **`ignored_refusals`** is `null` too, or the values PostgreSQL refuses
that an ignoring `parse` went past in the block, as `columns`, one per column
holding one in header order: `first` (its row's `offset` from the block's
data, its `COPY` `line`, its `column` by position in the header, its
`declared_type` and `value`) and `count`. **`checked_in_full`** is `true` for a
block a strict `parse` checked every value of.

**This is a raw dump of pgdt's internal representation, not a designed API.**
There's no schema, no compatibility promise across versions, no version field
for this shape (`format_version` versions the cache file, not the JSON), and no
attempt to make the shape convenient — field names, nesting, and what's
included can all change as the underlying code does. Reach for it when you need
something the text views don't show (or don't show in a shape you can parse),
and expect to adjust your `jq`/script when you upgrade pgdt. `--json` can't be
combined with `--detail` or `--map`, since the full object already carries
everything those two format for a human.

## Inspecting a cache with the dump gone: no `--source`

`pgdt info` can answer entirely from a saved `.dtcache` file, with no dump
file in reach at all — deleted, moved elsewhere, or never local to this
machine. Drop `--source` and pass `--dtcache <path>` on its own:

```sh
pgdt info --dtcache mydump.sql.dtcache
```

This works for the default listing, `--map` and `--json` alike — whichever one
you'd run against the live file, partial caches included. It's for exactly the
sysadmin-facing use case this whole page is about: keep a folder of `.dtcache`
files from dumps you no longer keep around, and still be able to answer "what
tables did this have," "which roles did it need," "did the schema change since
last time," without the multi-hundred-gigabyte file itself.

The one thing this form cannot do is check the cache against anything. With
`--source`, pgdt compares the file's size against the cache's and tells you if
the file changed; with no `--source` there is nothing to compare, so every
cache-only answer carries a `diagnostics:` line saying it is unverified,
historical data from whenever the cache was last saved.
