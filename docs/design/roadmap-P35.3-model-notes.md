# P35.3 notes — the cycles broken in place

What the next slices inherit from breaking the crate-crossing edges inside
`pgdump_query`. The assignment is `tests/layering.rs`'s `CRATES` and
`DEPENDS` tables, and the reasoning [`decisions.md`](decisions.md), "D68" and
"D74"; the vocabulary that moved is `src/model.rs` and its five submodules.

## What 35.4 inherits

- **The carve follows `CRATES`.** Every module is assigned and the test
  reads a module's production submodules too, so `model/` is checked as one
  module. Dump and codec are siblings; `io` may use the model, though nothing
  in it does yet.
- **What `model` reaches with `pub(crate)` from other crates** is what its
  `internal` module will hold: `FieldCount`, `BlockObserver`, `vec_heap`,
  `text_heap`, `min_rows_group`, `max_rows_group`,
  `deserialize_block_statistics`, `DatabaseMetadata::empty`, `TableDef::add`,
  `Unrepresentable::add`, `BlockCensus::new` and `merge`, and the
  `pub(crate)` methods on `BlockStatistics`, `ColumnStatistics`,
  `FieldRefusal` and `IgnoredRefusals`. The submodules are private and
  re-exported flat by glob, which carries `pub(crate)` items through.
- **A dependency's error is met by `map_err`** in four modules: `io`
  (`Error::xz`, `Error::join`), `leader` (`Error::join`), `batch`
  (`Error::arrow`), `cache` (`Error::cache_encode`). `Error::xz_cause` is the
  downcast a caller telling one `xz_seek` failure from another matches on, as
  `open_local`, `open_remote` and the tests do. The core still names `chrono`
  (a refusal's date) and `thiserror`, neither an error it carries.
- **Model doc links name other modules by `crate::` path** (`crate::map::…`,
  `crate::statistics::…`, `crate::preamble::…`); each breaks at the carve and
  is rewritten to the crate it lands in.
- **Tests moved with what they test**: `Cancellation`'s and `cut`'s and
  `worker_count`'s into `io/tests.rs`, the array-shape census's inline in
  `model/block.rs`, `canonical_type_name`'s inline in `lex.rs`, the refusal
  record's into `model/statistics/tests.rs`. `io`'s tests that drive a scan or
  a stream are where 35.2 left them.
- **`measure.py`'s mechanism paths name the model files** beside the modules
  they came from (`MAP`, `PREAMBLE`, `UNREPRESENTABLE`, `STATISTICS`), so the
  carve repoints them again.

## What 35.7 inherits

- `gather` and `summary` still import `predicate::{ValueKey, field_refused,
  grammar_refuses}`; both are query, so no edge crosses today, and the codec
  cannot take them back while they sit in a query module.

## Negative results

- **Moving a persisted type between modules moves no cache byte**: bincode
  encodes no path, and `persisted_index_is_pinned_to_the_format_version`
  passes at the version it had, so nothing here bumps
  `CACHE_FORMAT_VERSION`.
