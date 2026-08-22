# Phase 1 (MVP) — Implementation Notes

Phase 1 is complete: every functional item in
[`roadmap-phase1-mvp.md`](roadmap-phase1-mvp.md) is implemented. That doc
remains the specification — *what* the MVP does and why. This one records
*how* it landed, for the phases building on top of it: where each piece lives,
and the implementation-level facts a later phase would otherwise have to
re-derive from the code.

Not a changelog, and not a status doc — for what is and isn't built now, see
[`../status/STATUS.md`](../status/STATUS.md).

## Module map

| Concern | Where |
|---|---|
| Byte-range I/O trait + local-file impl | `pgdump_query/src/io.rs` |
| `COPY` block structure discovery | `pgdump_query/src/scan.rs` |
| COPY TEXT field splitting / unescaping | `pgdump_query/src/copy.rs` |
| Eager full-file index (`DumpIndex`, `CopyBlock`) | `pgdump_query/src/index.rs` |
| Structure cache (envelope, `CacheMode`) | `pgdump_query/src/cache.rs` |
| Arrow batch assembly, push-mode `read_table` | `pgdump_query/src/batch.rs` |
| Pull-mode `table_stream`, `ResumeToken`, cache replay | `pgdump_query/src/stream.rs` |
| Post-parse predicate | `pgdump_query/src/predicate.rs` |
| CLI (`pgdq parse` / `info` / `query`) | `pgdump_query-cli/src/main.rs` |

Integration tests mirror that split: `tests/batch.rs`, `tests/stream.rs`,
`tests/cache.rs`, `tests/query_cache.rs`, plus an `insta` snapshot of the whole
event stream over `tests/data/edge_cases.sql`.

## Facts that constrain later work

**The scanner never owns the bytes it scans.** `CopyScanner` is a synchronous
state machine; the caller holds the buffer, passes a slice, and the scanner
reports how much it consumed. This is what keeps events zero-copy and lets the
same state machine back both the async driver (`scan()`) and the pull-mode
`Stream` without a second parser. The consequence: the caller's buffer is the
only thing bounding memory, which is why `ScanOptions::max_line_bytes` exists
and why exceeding it is a hard error. Reasoning in
[`../status/history/2026-08-22.md`](../status/history/2026-08-22.md).

**A block's schema is per-block, not per-table.** A `COPY` header with no
explicit column list gets placeholder names (`column1`, `column2`, …) sized
to the first row's field count — Phase 1 has no DDL parsing to name them
from. Two blocks of the same table can therefore carry different schemas, which
is why predicate column resolution happens once per block rather than once per
query. Phase 2's typed columns replace the placeholder path, not the per-block
resolution.

**The zero-copy `Utf8View` path has three sharp edges.** Per
[`roadmap-phase5-scan-performance.md`](roadmap-phase5-scan-performance.md), a
field with no escapes is appended as a view into the Arrow `Buffer` backing the
read chunk it came from (`append_block`/`append_view_unchecked`), not copied.
Only escaped fields, and the rare field straddling two read chunks, take the
copying `append_value` path. Around that:

- Chunk buffers are retained in a small deque and evicted only once the scanner
  has moved past them for good — a finished batch pins its source chunks.
- A chunk's cached `StringViewBuilder` block index is invalidated on **every
  flush**, because `StringViewBuilder::finish()` resets the builder's internal
  block list. Anything that adds a new flush trigger must honour this.
- Non-UTF8 field bytes are a hard `Error::InvalidUtf8`, not a lossy conversion.

**Three cache fields are reserved and always `None`.** `CopyBlock::sparse_index`
(`SparseRowIndex`), `CopyBlock::column_stats` (`RowGroupStats`), and
`DumpIndex::metadata` (`DumpMetadata`) are serialized from the first release but
never constructed by any code. Populating any of them — Phase 2's metadata,
Phase 3's statistics, Phase 5's sparse index — is additive, not a format-version
bump. The types are placeholders: their real shape is each phase's own design
work.

**Cache reads and writes have deliberately opposite failure modes.** An
unrecognised `format_version`/`container_kind`, or bytes that don't parse as a
cache at all, load as `Ok(None)` — indistinguishable from a missing file.
`cache::save` propagates I/O failures as `Error::Io`. Phase 3 statistics break
the assumption that makes the read side safe (a stale statistic yields a wrong
answer, not a rescan), so the dump-file identity check currently filed under
"Configurable (future)" becomes mandatory there.

**Cache-consulting queries run as a `Segment` sequence.** A query resolves the
cache into one `Segment::Known { start, end }` per already-cached block matching
the query table, then one trailing `Segment::Live { start }` past the cache
watermark. Nothing outside a `Known` range is ever read, so non-matching cached
blocks cost zero I/O. Both kinds drive identical `CopyStart`/`Row`/`CopyEnd`
handling. A `Live` segment additionally feeds every block it finds — matching or
not — to a private `Recorder` that persists the growing index after each
completed block and once more at true EOF; a dedup guard by `header_offset`
makes that idempotent, so an early `ControlFlow::Break` or a dropped stream
leaves correct partial progress rather than nothing.

**`ResumeToken` carries a file offset, a cumulative row count, and — for a
token taken mid-block — the block's header and in-block row count**, which is
what lets a fresh stream reconstruct the same schema and continue with no gap
or repeat. A reserved `generation` field is always 0. The token exposes no
public fields and must stay that way: a raw file offset is meaningless inside a
compressed archive entry (Phase 6).

**Error surface**: `Io`, `Join`, `UnterminatedCopyBlock`, `LineTooLong`,
`InvalidUtf8`, `CacheEncode`, `CacheDisabled`, `UnknownPredicateColumn`. The CLI
uses `anyhow` over these.

## Validation

The full-file scan is validated against the real 784GB koji sample — 74 blocks,
19.58B rows, all bytes accounted for, flat ~9 MiB RSS, no
`UnterminatedCopyBlock`; `lock_monitor.activity` (whose row data contains a
literal `COPY … TO stdout;` substring) parsed as one correct block, confirming
line-anchored detection against the case that motivated it. Numbers and their
consequences for index sizing are in
[`../status/history/2026-08-22.md`](../status/history/2026-08-22.md).

Decoder correctness rests on a round-trip against real `pg_dump` output on all
three fixture versions: `scripts/fixture_schema.sql`'s `public.escapes` table
holds one row per codepoint (`chr(n)`), so the test compares against a value it
computes itself rather than a hand-transcribed literal — covering every escape
`pg_dump` emits, the raw control bytes it leaves unescaped, and 2-, 3- and
4-byte UTF-8. The hand-written `tests/data/edge_cases.sql` is deliberately
timestamp-free so its snapshots stay stable; the generated `fixtures/{13,16,18}`
tree gets structural assertions instead, since its timestamps change on
regeneration.

Chunk-boundary correctness is asserted directly rather than assumed: the event
stream is identical across chunk sizes 1…4096, and the batch tests exercise the
zero-copy, straddling, and decode-copy paths against the same expected output.
