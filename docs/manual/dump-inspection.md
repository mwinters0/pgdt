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

A clean stop like that loses only the block it was reading. A **second** Ctrl-C
exits immediately without waiting for the write, and so does `kill -9`, a power
cut or a crash — those fall back to the last save the scan happened to take,
which can be a few blocks earlier, because a scan that saved after every block
would spend more time saving than scanning. Nothing is ever left *corrupt*: the
cache either loads or it does not, and `pgdq info` states how far it goes.

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
  it declares. A dump taken with `--create` (or `pg_dumpall`) that touches
  more than one database repeats this block once per database, each under
  its own `database: <name>` line.
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

### When `info` says it cannot answer

`info` exits non-zero rather than scanning. Four things can go wrong, and they
are four different messages because they mean four different things — even
though every one of them is fixed by running `pgdq parse`:

| Message | What happened |
|---|---|
| `no cache at …` | You have not parsed this file yet. |
| `… is not a pgdq cache` | Something else is at that path. Check `--dqcache`. |
| `… was written by a different pgdq build` | The cache format changed under you. Pre-1.0 this happens; nothing is migrated. |
| `… has changed since it was parsed` | The dump file's size no longer matches. Every offset in the cache could be wrong. |

The last one is the one worth reading closely: it is not "your cache went
missing", it is "your file is not the file you parsed."

`--dqcache none`, which for `query` means "ignore the cache", is rejected on
`info` — with nothing to read and no scan to fall back on, there would be
nothing left to report.

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

The one thing that *does* change is a column of a database the scan never
reached. In a `pg_dumpall` or `--create` dump with several databases, a scan
that stopped inside a later one never read that database's `CREATE TABLE`
statements, and `--verbose` says so per column:

```
    id: metadata not scanned — the scan never reached this database's DDL; finish the parse
```

That is different from `not declared`, which means the dump never explained the
column at all and is final.

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
[`docs/design/roadmap.md`](../design/roadmap.md), "Large objects: ranges, not
contents", if you need to know why.

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
