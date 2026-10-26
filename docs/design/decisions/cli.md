# The CLI

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. `pgdt`'s commands: what `info` never does, the save
throttle and interrupt handling, how `--jobs` and `--memory` are announced, byte-identical output, and `--json`'s shape.
`crates/pgdt`.

### D61 `info` never scans
The surprise is that a scan happened at all; `parse` scans ahead, a `query` maps only what the cache lacks (D48), and
`--preamble-only` hangs off `parse`. It reports the cache through `CacheMode::open`, or `OfflineCache` with no source,
never a `pgdt_query::Dump`, which holds no partial map, container or envelope. Rejected: a scanning fallback;
`--no-scan`; a `Dump` where the cache is complete, a second path for one cache.

### D62 The save throttle is a ratio, and its gate opens splice too
A save is skipped unless `K` times the last save's duration has elapsed since it: a ratio bounds overhead in every
regime, with no interval to tune. The map is rebuilt only where the gate opens, each rebuild being `KD5`'s cost, the
gate opening too on a block that could settle a queried table. Rejected: every N seconds or bytes; a disabled-cache
floor. Evidence: `per-block-quadratic`.

### D63 The interrupt flag is read at both extremes; resume is the default
Per chunk and at every completed block, since neither alone bounds the response. A resumed scan is span-identical to a
straight-through one, which is what makes an interrupted `parse` saved work. Rejected: `select!` over `ctrl_c` (drops
the map); a `--restart` flag.

### D64 An omitted `--jobs` asks the source; provenance is the CLI's fact
Absence recommends (the source's `default_workers`), a stated count is never lowered, zero is refused; whether a number
was typed or discovered rides the `Arrangement`'s `Allowance`, never `Parallelism`, which every pool reads. The flags
and the limit are announced before the open, the arrangement after it, since a fresh `.xz` walks footers first; the
limit is read once, so the limit announced is the one carved. Status goes to one stderr subscriber, no terminal
detection; its durations are no figures. An index build is announced by its source: at `info` where its cost is the
scan's own (a local walk; a gzip first pass, saying why the scan is one reader), at `warn` before a cost beyond it (a
fetched `.xz` walk). Rejected: the first pass's reason in the arrangement's lines (codec knowledge in the arrangement).

### D66 Output of what `pg_dump` writes is byte-identical whether typing is on or off, but for a value its type cannot hold or an `escape` `bytea`
Every value renders back to its `*_out` text, which is what `pg_dump` wrote — a hand-written spelling `*_out` never
writes rendering as `*_out` would — but one its type cannot hold, NULL in the null mode (D98), and an `escape` `bytea`,
`hex` (I56), which `byteain` reads alike. One contradicting its type is `FieldDecode` naming the strings schema mode,
never a null. Rejected: Arrow's display; a `bytea`'s form kept per database or flagged, read by no comparison.

### D67 `--json` is the internal struct; a flag's help is its doc comment
The whole cache file, keyed by block, no version of its own, compact and streamed, group values in: a script sums its
own rollup, a table spanning blocks (I2) with no merge rule but statistics' sums, `--detail`'s alone. Coverage is one
line at the top, nothing below qualified. No `help` attributes but `sql`'s, upstream's style; pages are snapshotted with
width and bare-flag assertions. `sql --dump`'s value name spells its suffixes past the page, its spec line exempt from
the width assertion, until one pass over CLI output (`roadmap.md`, "Future — wanted, unscheduled"). Rejected:
`long_help` per flag; a rollup; that value name cut to a placeholder piecemeal.
