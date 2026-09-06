# P13.5 — End-to-end `.xz` tests and differential parity

## What landed

**`pgdump_query-cli/tests/xz_source.rs`**, four tests driving the real `pgdq`
binary (`CARGO_BIN_EXE_pgdq`) against `.xz` input — nothing before this slice
did; 13.1–13.4 exercised `open_local`/`build_index`/`XzSource` directly
(`pgdump_query/tests/cache.rs`).

**Two fixtures, generated at test time, not committed**, per the phase spec's
own "Fixtures" section — both `xz`-compressing the same hand-written dump
`tests/cache.rs`'s own xz tests already use,
`pgdump_query/tests/data/edge_cases.sql` (2,352 bytes, no `CREATE TABLE` DDL,
so every column resolves `Utf8View` either way):

- **Seekable, multi-block**: `xz --block-size=512`, the same block size
  `tests/cache.rs`'s `xz_compress` uses — 5 blocks on this fixture, confirmed
  with `xz --list -v`.
- **Non-seekable, single block**: a bare `xz` invocation — one stream, one
  block, D2's diagnosed shape and the decode-from-zero backward-read path.

**Four tests:**

- `seekable_xz_parses_to_the_same_index_as_plain` — `pgdq parse` then
  `pgdq info --json` against the plain file and the seekable `.xz`; the two
  JSON documents are asserted **fully equal**, including whatever diagnostics
  a DDL-less dump always earns (`TocCoverage`'s `Info` note) — neither side
  carries `NonSeekableCompressedSource`.
- `non_seekable_xz_parses_to_the_same_index_plus_a_warning` — same shape, but
  the non-seekable fixture's `diagnostics` array is asserted to carry D2's
  warning where the plain file's does not; with `diagnostics` removed from
  both, the rest of the JSON is asserted equal. A second pair of `info`
  (non-JSON) calls checks the warning's **rendered text** names both remedies
  (`xz -T0`, `--block-size=<size>`) on the xz side and appears on neither the
  plain side.
- `query_widgets_agrees_across_plain_and_both_xz_shapes` — `pgdq query --table
  public.widgets` against all three sources (each already `parse`d, so `query`
  reads through the persisted cache — D5's `CompressionIndex::Xz` field round
  trips, not just the live decoder), asserting byte-identical stdout across
  all three and `6 row(s)` on stderr for each.
- `filtered_query_agrees_across_plain_and_both_xz_shapes` — the same, with
  `--filter name=alpha --no-columns`, exercising a typed forward-only decode
  path rather than the replay read above.

## What was decided along the way

**Comparison is by parsed `--json`, not by the text listing.** The two
diagnostics differ by exactly one entry (D2's warning) but everything else —
spans, per-block resolution, coverage — must be identical, and a parsed
`serde_json::Value` equality after removing the `diagnostics` key says that
directly. The text listing was used only for the one thing JSON can't check as
naturally: that the diagnostic's *rendered wording* is what D2's spec
paragraph specifies (`xz -T0`, `--block-size=<size>`), which is what a user
actually reads.

**Row-count parity is asserted via the `N row(s)` stderr line, not via
`stdout.lines().count()`.** `edge_cases.sql`'s row 3 (`gamma`) carries a
literal embedded newline in its `description` (from the dump's `\n` escape),
so a naive line count overcounts by one — caught by this slice's own first
run of the test, not reasoned out in advance. The row-level assertion that
actually matters is stdout byte-equality across the three sources, which does
not depend on how many lines that renders as.

**Both fixtures are `parse`d before `query` runs**, rather than passing
`--dqcache none`, so `query`'s reads for all three tests go through a
freshly-built, freshly-loaded cache — the persisted `CompressionIndex::Xz`
envelope field (D5) and its round trip through `cache::save`/`load`, not just
a from-scratch `XzSource` scan. This is closer to how `pgdq` is actually run
in sequence and costs nothing extra, since each fixture lives in its own
`tempfile::tempdir()` with no colocated-cache collision risk.

**Only `public.widgets` is queried end to end.** The fixture's other tables
(`empty_table`, `"My Schema"."Odd Table"`, `no_column_list`) are covered by
the full-index parity check (`info --json` resolves every block in the file,
xz included), just not by a `query` invocation of their own. `widgets` alone
already carries the byte-decode hazards worth re-checking through a
compressed source — an embedded backslash, a NULL, an empty string, a
COPY-like phrase inside a value — so a second table would add fixture
plumbing without adding decode coverage.

## Testing

`cargo test --workspace`, `cargo clippy --workspace --all-targets` and
`cargo fmt --check` all pass with no new warnings. `xz` 5.8.3 (this machine's
`/usr/bin/xz`) is what both fixture helpers shell out to; per the standing
rule for a tool `mise` does not pin, a missing binary fails the test loudly
(`.expect(...)`) rather than skipping silently.

## What this changes for measurement

Nothing. `pgdump_query-cli/tests/xz_source.rs` is test-only and touches no
timed path; no figure in `measurements.md` is newly stale from this slice
(D3's dyn-compatible trait already made every read-path figure stale as of
13.1, and this slice adds no further read-path code).

## What remains for P13's wrap

This was the last slice on P13's checklist — every row in
[`../status/STATUS.md`](../status/STATUS.md)'s "P13 progress" is now ticked —
but the wrap itself (consolidating `13.1`–`13.5`'s per-slice notes into one
`roadmap-P13-compressed-input-notes.md`, deleting the per-slice files,
rewriting `STATUS.md` to drop the checklist, and setting the roadmap index row
to `Complete`) is deliberately left for its own round rather than folded into
this one: `process.md`'s loop treats "land a slice" and "wrap the phase" as
separate steps, and a wrap's own obligations — checking whether any `KD<k>`
entry needs re-homing, auditing whether anything the slices learned belongs in
`architecture.md` — are exactly the kind of separate review this slice's own
"land exactly one slice" boundary exists to keep out of the same cycle.
