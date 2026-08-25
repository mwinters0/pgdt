# Dump inspection

`pgdq info` tells you what is in a dump file — tables, roles, indexes,
functions, everything `pg_dump` wrote — without decoding any row data. Run it
before writing a query, or whenever you just want to know what a dump
contains.

```sh
pgdq info --source mydump.sql
```

The first time you run it against a file, it does a full scan and writes a
cache next to the file (`mydump.sql.dqcache`) so every later `info` or
`query` against the same file is instant. Pass `--dqcache <path>` to put the
cache somewhere else, or `--dqcache none` to skip it. If the file changes
size, the cache is invalidated automatically and pgdq scans again; you never
need to delete it by hand.

## The default view

```
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

2 COPY block(s), 111110 row(s), 48213911 bytes scanned
```

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
- **The table listing** is what Phase 1 always showed: one entry per `COPY`
  block, in file order, with its row count and column list. This is the
  view to read when you're deciding what to query.

Add `--verbose` to also see each block's byte offsets and, for any column
whose type could not be resolved, why (see
[type handling](type-handling.md) for what "resolved" means and why a column
sometimes isn't).

## Everything else in the file: `--map`

The table listing above only shows `COPY` blocks — the actual row data.
Everything else in the file (every `CREATE TABLE`, index, trigger,
publication, comment block, and stretch of framing) is there too; `--map`
lists all of it, in file order, instead of just the tables:

```
$ pgdq info --source mydump.sql --map
[0, 35) framing
[35, 622) SCHEMA public
[622, 981) TABLE public.accounts
[981, 1054) CONSTRAINT public.accounts accounts_pkey
[1054, 49267) COPY public.accounts (12345 rows)
[49267, 49803) INDEX accounts_name_idx
...

312 span(s), 48213911 bytes scanned
```

Every byte of the file shows up in exactly one line — that's a property pgdq
checks on every scan, not just a description of the output. Reach for
`--map` when you want to see what's actually in a dump beyond its tables (an
unusually large comment block, a publication you didn't know about, where a
particular index sits) or to narrow down where something looks off before
reaching for `--verbose`'s finer detail on one specific block.

A dump run with `--inserts`/`--column-inserts` shows a table's data as an
`INSERT run` entry instead of a `COPY` block — same idea, a table's rows
merged into one line, just written differently by `pg_dump`. A dump
containing large objects (`lo_create`/`lowrite` calls, not `COPY` data) shows
their whole region as a single `large objects` entry — pgdq accounts for the
bytes but does not read large-object contents; see
[`docs/design/roadmap.md`](../design/roadmap.md), "Large objects: ranges, not
contents", if you need to know why.

## Skipping the scan entirely: `--preamble-only`

If you only want the header block — versions, extension and type counts —
`--preamble-only` answers from the start of the file alone, without reading
the rest. Its cost does not depend on the dump's size: a preamble-only
`pgdq info` on a 5KB dump and on a 500GB one take the same time, because
`pg_dump` always writes every `CREATE EXTENSION`/`CREATE TYPE` ahead of any
table's data. Roles, tablespaces, the object-kind summary and the table
listing are all unavailable in this mode — they require having scanned the
rest of the file, which is exactly what `--preamble-only` skips.

## Scripting against the output: `--json`

Every mode above (the default listing, `--map`, `--preamble-only`, and
cache-only) also takes `--json`, which prints the same information as one
JSON object on stdout instead of formatted text:

```sh
pgdq info --source mydump.sql --json | jq '.spans | length'
```

**This is a raw dump of pgdq's internal representation, not a designed API.**
There's no schema, no compatibility promise across versions, and no attempt
to make the shape convenient — field names, nesting, and what's included can
all change as the underlying code does. Reach for it when you need something
the text views above don't show (or don't show in a shape you can parse),
and expect to adjust your `jq`/script when you upgrade pgdq. `--json` can't
be combined with `--verbose` or `--map`, since the full structure already
contains everything either would add.

## Inspecting a cache with the dump gone: no `--source`

`pgdq info` can answer entirely from a saved `.dqcache` file, with no dump
file in reach at all — deleted, moved elsewhere, or never local to this
machine. Drop `--source` and pass `--dqcache <path>` on its own:

```sh
pgdq info --dqcache mydump.sql.dqcache
```

This works for the default listing, `--map`, and `--preamble-only` alike —
whichever one you'd run against the live file. It's for exactly the
sysadmin-facing use case this whole page is about: keep a folder of
`.dqcache` files from dumps you no longer keep around, and still be able to
answer "what tables did this have," "which roles did it need," "did the
schema change since last time," without the multi-hundred-gigabyte file
itself.

Cache-only mode never falls back to scanning — there's no dump file to scan
— so it's stricter than the live modes above. `pgdq info --dqcache <path>`
(default or `--map`) needs a cache that covers the *whole* file; one written
by `--preamble-only` doesn't qualify, and only `--preamble-only` itself will
read it. Every cache-only answer also carries a `diagnostics:` line saying
so — it's unverified, historical data from whenever the cache was last
saved, since there's no live file left to check it against.
