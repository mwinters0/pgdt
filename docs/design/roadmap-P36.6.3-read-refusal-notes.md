# P36.6.3 notes — an uncheckable source refused where a read begins

`SourceWatch::open` no longer refuses. It still hints the selection to the
source, and observes the baseline. The refusal is
`SourceWatch::require_checkable`, which answers `Error::SourceUncheckable`
where the watch's in-flight check binds and the source answers
`in_flight_unchecked`. `StrictIdentity::between_runs` is gone, so `NONE` is
again the only selection with the in-flight check off (its rustdoc, D21).

## The shape

- **Three reads call it at their start**: `map_file`, `preamble_only` and
  `pgdt_query`'s `map_for_query`, which both stream entry points run first.
  Each calls it before the pass reads anything, so nothing is saved.
- **`PgDump::open` calls it after the open**, so registration still refuses
  before any scan. It comes before the incomplete-cache refusal: a dump with
  no cache over an uncheckable server names the in-flight check, not the
  `parse` that would build the cache.
- **`pgdt info` opens under the selection as given.** Its watch now hints
  the binding selection to the source, as every other command's does.
  `crates/pgdt/tests/remote.rs` pins that it is still not refused.

## Consequences

- **On `parse`, the refusal follows the arrangement's announcement again**,
  coming from inside `map_file`, as it did before 36.6.1. A strict-identity
  refusal of a weak signal is still the open's, so on a server that fails
  both, that one is now reported first.
- **The tests moved with the refusal.** `crates/pgdt-query/tests/identity.rs`
  asserts that the open succeeds and that the mapping pass, `preamble_only`
  and a query each refuse, reading nothing. The CLI test adds
  `parse --preamble-only`, and `pgdt sql` refused at registration over a
  complete cache, with `--strict-identity=none` reading it.

## What the next round inherits

- **36.8's `Table::plan` calls `require_checkable`** when `map_for_query`
  goes, as the history entry for 36.6's review placed it (history,
  2026-10-17, "36.6's calls reviewed"). The `Dump` owns the watch, so it is
  the plan that knows a read is about to begin.
