# P38.7 — The record: notes

What 38.7 leaves the wrap. The spec is
[`roadmap-P38-descendants.md`](roadmap-P38-descendants.md), "What is shown,
claimed and checked" and its slice 7. The user-facing account is
[`../manual/descendants.md`](../manual/descendants.md); the README's
supported/unsupported list and
[`pg-dump-compatibility.md`](pg-dump-compatibility.md)'s inheritance,
declarative-partitioning, `--load-via-partition-root` and TimescaleDB rows
cite it. Both modes in one statement is checked by `pgdt`'s
`tests/descendants.rs`, `pgdt_sql_reads_both_modes_by_one_dump_attached_twice`:
one inheritance fixture attached as `whole` and as `own` (`:inheritance=only`),
each table's two counts joined in one statement and each held to its own
oracle, at both flag sets.

## The hand run on the reference dump

**`SELECT count(*) FROM datanommer2.public.messages` answers 530,657,112**,
the catalog's chunks' rows by the survey's per-block counts
(`runs/datanommer-survey/tables.tsv`: 728 blocks naming `public.messages` their
hypertable, the root's own block none). `pgdt info` agrees on every line the
spec's logical model names: `public.messages: 728 descendant(s), 530657112
row(s) in all`; TimescaleDB 2.13 inferred from its catalog's columns, one
hypertable of 728 chunks, all uncompressed, none absent, dimension `timestamp`;
the chunks' blocks folded under the root; no finding but TOC coverage. Its
total over all 761 blocks, 1,025,133,886 rows, is the survey's own count of
data lines, a separate program's.

The run was `pgdt parse --max-line-bytes 536870912` at the data level, then
`pgdt info`, then the count in `pgdt sql` under `SET pgdump.max_line_bytes`,
the query stating the limit again because the cache carries none
([`roadmap-P33-line-bound-inbox.md`](roadmap-P33-line-bound-inbox.md), "The
refusal names an offset and a limit, and the parse saves nothing after it");
each in `scripts/contained.py --memory 12g`, built at `334ed472` with no
production code changed since. `runs/p38.7-datanommer-count-20261031-1224/`
holds `run.sh`, the binary, `parse.log`, `info.txt` and `count.csv`. Its wall
clock, untimed and no figure: 42 min 48 s from launch to the count's exit, by
`parse.log`'s timestamps 17 min building the `.xz`'s seek table over its
106,071 streams and 25.5 min scanning, at the `jobs=24` the source
recommended; `info` and the count took seconds. The data-level cache it wrote
is kept beside the link it read through,
`/mnt/ssd/fedora/scratch/pgdump_query/datanommer/`, for any later hand run on
the dump.

## Negative results

- **Nothing refuses one path attached twice**: two `--source`s naming one
  file under two catalog names read the one cache, each under its own mode.
