# The file map and the preamble

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. The file map and the preamble: how a dump is tiled into
spans, what the TOC comments contribute, how `DumpIndex` keeps each fact once, and what the census and the
unrepresentable count record for a typed query to read back. `pgdt-dump`'s `map`, `preamble` and `ddl`, and the model's
`DumpIndex`.

### D30 A span is classified by the grammar; the TOC anchors and enriches, never required
A span is classified by the statement grammar and the TOC comment is enrichment: TOC-driven segmentation is unsound on
non-`pg_dump` input and a dollar-quoted body can hold a TOC-shaped line (I3), so a TOC block anchors a span where one
exists and the grammar closes every statement without one. A tiling hole is `TilingBroken`, never a refusal. The
preamble prepass (byte 0 to the first `COPY` header, I1) runs ahead of either mapping entry point's pass unless the
cache holds it, so a scan stopped anywhere, its first chunk included, still states the first database's DDL. Span text
is sliced from the file after building, never accumulated, and `Data` spans store none. Evidence: `preamble-prepass`.

### D31 `Span::toc` is "belongs to", vetoed after classification; boundary rules that read as bugs
Follow-ons inherit `governing_toc` (else coverage reads half on healthy input) and `toc_owned` counts objects; `Framing`
vetoes after the transition that seeded the inheritance, and `Connect` is never seeded. The boundary predicate refuses
`"Data for "` and accepts `"Statistics for "`, which matters under `--disable-triggers` (I31). A blank line does not
close a pending comment (`_printTocEntry` writes `--\n\n`); `scan_preamble` retreats to `pending_comment_start()` rather
than guessing the comment's kind.

### D33 Bulk regions are one span kind; large objects skip at the scanner, `INSERT` runs at the map
Only `DataBlock::Copy` has inner offsets, only `COPY` having a row reader; a data span absorbs the TOC comment pending
at its start (`KD1` where none is). A large-object region, a bare `BEGIN;`/`COMMIT;` pair, is skipped unread at the
scanner (I12). `INSERT` runs fold in `feed_line` with no scanner state, their boundaries resting on no line-anchored
invariant, the run's end lexed as the scanner lexes; the cuts `KD9` names wait for P8's row reader.

### D34 `DumpIndex` stores no fact twice but sortedness, `metadata` and `scanned_through`; whole-file facts need `is_complete`
Every fact has one owner: `blocks()` and `metadata` are views over `spans` (`metadata` computed once) and diagnostics
are never persisted. Whole-file facts (roles, tablespaces, a reported schema) are believed only once `is_complete`; a
streamed schema commits over the blocks it replays, ungated (I2). Statistics sit in their block behind an `Arc`, so a
map clone copies a reference (`KD5`), and a row group is a byte range, so no piece needs a global row index. A back-fill
keeps a block's columns, and its size unless a stated size or bound it did not record moves it, a maximum it breaks
re-reading it at the size its groups predict. Rejected: `SparseRowIndex`; padded `character` bounds and entries (keyed
alike, but past the cap); a back-fill dropping only re-read blocks' columns.

### D35 The census is type-blind, records both dimension bounds, and runs at the data level
`ArrayShape::observe` reads the leading brace run off still-escaped bytes in the model (I15, I25); min and max depth, or
`{1,2}` beside `{{1,2}}` resolves to a wrong `List`. A table any column of which is at the data level is censused, and
so is every block a query's pass maps: gating on a full scan left early blocks uncensused. The metadata level records
`None` and, but under a strict parse, splits no field; a typed query re-reads such a table for itself, writing nothing,
and a `Dump`'s plan refuses it. Rejected: degrading to the DDL's `List`; a census-only level a user can reach; the
census as a `Table`'s own step, which a `Dump`'s would offer. Evidence: `statistics-gathering`, profiled.

### D96 The unrepresentable count rides the census, typed by the DDL, in two tiers, under a recorded calendar
Every census-taking read counts, per block and column, the values the declared type's typed pair (no census) cannot
hold: each leaf by its own type, lexically, only boundary years and an interval's long hour part by arithmetic; a nested
value once, in its worst leaf's tier — `format` past Arrow's spec, `engine` past `calendar_end` (RT21). Counts persist
(D74); a cache counted under another calendar is refused (D20). Rejected: a decode per field; a type-blind test (`text`
reading `infinity`); counting as a query reads (timing decides); one tier; counting a value the server refuses.
Evidence: `every_extreme_is_held_by_arrow_or_recorded`.

### D36 The preamble grammar dispatches on fixed keywords and never guesses
Unrecognized lines are ignored, so `--binary-upgrade` noise is free (I5, I6). `record_type` keys on name (I11); a
composite's field list is all-or-nothing, `record_out` being positional (I23); a `--create` dump's pre-`\connect`
segment is not a database (I9). The model keeps the dump's text (D74): a declared type as its words; a collation clause
verbatim, `None` meaning no clause (I37); a name in one spelling, both sides of a lookup compared in it (I29);
`INHERITS` parents, `OF` type and a partition's bound as text (I86). Of what the DDL implies it keeps only a column's
`NOT NULL`, an enum's label completeness and a collation's `deterministic` (I42). Rejected: a name's parts dequoted
(`"a.b".c` is `a."b.c"`); references flattened in at the fold, which a later `ADD COLUMN` misses.
