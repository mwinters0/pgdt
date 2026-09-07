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
large one. The remedy is in the message — recompressing with `xz -T0` or an
explicit `--block-size` produces a file pgdq can seek into. Files that
`xz` produced with threads, or that were made by concatenating several `.xz`
files, are already seekable and earn no warning.

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
them or 64 MiB's worth, whichever is fewer — so a 16 MiB chunk is 64 MiB of
resident memory on top of whatever the scan already holds, and a 32 MiB chunk
is the same 64 MiB rather than double it. Past 64 MiB the pool keeps a single
buffer, which is the size you asked for.

What it already holds does not grow with the *size* of the dump — a 3 GiB file
costs no more than a 2 MB one, a few megabytes either way — but it does grow
with the number of tables in it, by roughly 10 KB each. A dump of a few thousand
tables is tens of megabytes resident before any chunk size is chosen.

The flag exists for a device unlike any of those three. If you have one and
find a size that beats 1 MiB on it, that is worth reporting.

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

Add `--verbose` to also see each block's byte offsets and, per column, what
it became: the Arrow type it resolved to, or — for a column that came back as
a string — why (see [type handling](type-handling.md) for what "resolved"
means and why a column sometimes isn't).

`--verbose` also turns the `user-defined types` count into a listing of the
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
reaching for `--verbose`'s finer detail on one specific block. On a partial
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
- **`resolution`**, one record per `COPY` block, with the per-column outcome
  `--verbose` renders as prose. Each column carries its name, the declared
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
combined with `--verbose` or `--map`, since the full object already carries
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
