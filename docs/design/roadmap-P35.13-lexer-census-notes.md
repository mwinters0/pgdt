# P35.13 notes — the lexer to pgdt-dump, the census fold to pgdt-model

The first cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
design, its "Across crates" items for SQL lexing and the census. Every body
moved whole; nothing persisted changed, so no format bump.

## Where the cut fell

- **The lexer is private to pgdt-dump**: `lex.rs` beside `scan.rs`,
  `mod lex`, its items `pub(crate)`, gone from pgdt-core's `internal`.
  Nothing above dump named any of them, tests included.
- **pgdt-core's `type_name.rs` holds `parse_type_name` and
  `canonical_type_name`**, still reached through `internal` by dump's DDL
  grammar, the model's catalog and the codec's register. Its one test stays
  inline. pgdt-core's doc names its four concerns again.
- **`census_row` is `BlockCensusExt::observe_row`**, its body unchanged, in
  pgdt-model's `block.rs` beside `merge`; pgdt-model takes `memchr` for its
  first pass. pgdt-dump's `internal` lost `census_row`, so
  `pgdt_query::stream::backfill` reaches only model for the census, and
  `leader.rs` no longer imports `map.rs` at all: 35.18 moves it without
  that edge.
- **Both inline test modules went out of line**: `lex/tests.rs`, as the
  design draws it, and `scan/tests.rs`, which the design ties to the
  lexer arriving beside `scan.rs` (its "Tests"). The fixture SQL naming a
  scan test by file follows it.

## What later cuts inherit

- **Two `observe_row`s sit in `block.rs`**: the census's, on
  `BlockCensusExt`, and the statistics observer's, on `BlockObserver`. A
  profile tells them apart by trait path; `measurements.md`'s
  `statistics-gathering` prose names which it means.
- **`measure.py`'s `SCAN` and `emitter_register.py`'s `READERS` name
  `crates/pgdt-dump/src/lex.rs`**; `type_name.rs` is in neither, being off
  the scan path and matching no keyword the register extracts (the register
  stays green). `UNREPRESENTABLE` already declared `block.rs`, which now
  carries the fold too, so every data-level figure still declares the
  census.
- 35.15 redraws pgdt-dump's tree around `lex.rs` and `scan.rs` as they now
  stand; the DDL grammar's own helpers (`ddl/lex.rs` in the design) are a
  different file from this lexer.
