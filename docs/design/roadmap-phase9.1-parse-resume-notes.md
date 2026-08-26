# 9.1 — `parse` resumes and saves as it goes

What the rest of Phase 9 inherits from the slice that made partial caches
*producible* on purpose rather than only as a side effect of interrupting a
query.

## The shape

`stream::map_file(source, scan_options, cache) -> Result<(DumpIndex, u64)>` is
the new public entry point, and `pgdq parse` is its only caller. It is
`map_forward` with no stop target, plus the three things only a scan that
reached EOF may do. The `u64` is the frontier the run *started* from — 0 for a
cold scan — which is all the CLI needs for its resume line.

`index::build_index` is untouched and still the eager, cache-blind producer.
Two whole-file producers now exist and `tests/map_file.rs` is what keeps them
from drifting, alongside `tests/map.rs`'s existing
`build_index_spans_match_build_map_exactly`.

## Three calls the next slice should know about

**`map_forward`'s stop rule became `target: Option<(&str, Option<&str>)>`,
replacing `table: &str, selector: Option<&str>, extent: ScanExtent`.** `None`
means "run to EOF". The alternative was passing a sentinel table name under
`ScanExtent::Full`, which is dead data every later reader has to prove is
unused. `ScanExtent` still exists and still means what it did; `table_stream`
maps it onto the `Option` at the call site. No behaviour changed.

**Metadata is recomputed in `map_file`, not in `map_forward`.** This is the
load-bearing one. `map_forward` never touches `index.metadata`, because
`preamble::dump_metadata_from_spans` may only be called at one of two
boundaries — EOF, or the first `COPY` header of the current database (I1) — and
a `CopyEnd` watermark is neither: a trailing `Unscanned` tail would make the
last database's `preamble_complete` a lie. `map_file` runs at EOF, so it
recomputes there and every `\connect`ed database comes back
`preamble_complete`, which is what makes "a full `pgdq parse` types every
database" true. A query still only ever has `scan_preamble`'s first-database
capture, and `table_stream` clones `metadata` *before* calling `map_forward`,
so nothing about the streaming path changed.

**A partial cache's `metadata` therefore holds only the databases the prepass
captured, even when the spans already cover a later one's DDL.** That is the
exact state 9.4's `MetadataNotScanned` exists for, and it is why
`pgdump_query-cli/tests/partial_reporting.rs` builds its truncated caches by
keeping *n* databases explicitly rather than re-deriving metadata from the
truncated spans.

**Diagnostics: `map_file` keeps only `CacheMtimeChanged` from the load and
recomputes the rest.** `map_forward` assigns `index.diagnostics` wholesale at
EOF, and an index loaded from a complete cache never enters that path at all,
so both the tiling check and the TOC-coverage figure are recomputed in
`map_file` over the finished spans. Filtering rather than taking the whole
loaded list matters because 9.2 made `cache::load` recompute those two figures
too — taking everything would duplicate them.

## Truncating an index by hand

A complete scan's spans are **greedy**: a block's span runs on to wherever the
next one starts, past its own `end_offset`. `map::Builder::snapshot` closes
spans *at* the watermark instead. So a hand-built partial index (as
`tests/partial_reporting.rs` needs) has to keep spans by `start < frontier` and
then clamp the last one's `end` to the frontier — filtering on `end <=
frontier` silently drops the very block the scan had just banked.

## The write amplification, and why there is no throttle

Serializing the whole cache at every `CopyEnd` is 74 whole-cache writes on
koji against the one the previous build did, and it costs **+50 s on a 3300 s
scan, about 1.5%** — inside the spread between the two independent baseline
scans of the same file. The saves' total bytes are bounded above by 18.3 MB
against 784 GB read. The figure, its table and its container recipe are in
[`measurements.md`](measurements.md), "koji full scan".

So the save throttle the phase reserved as a tuning knob **is not built**, and
nothing downstream should assume a save interval exists. What would change the
answer is a dump with orders of magnitude more `COPY` blocks than koji's 74,
since the cost is per block and each save is the *whole* cache: a file with
10,000 small blocks pays 10,000 serializations of a cache that is itself
proportional to the block count, which is quadratic where koji's is not. No
such sample exists here to measure against.
