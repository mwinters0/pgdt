# P36.11.1 notes — The shell's spelling

`datafusion-pgdump` names no front end's command: `CacheNotComplete` and
`MetadataLevel` state the mapping pass in the library's terms and carry it as
a `MappingPass` (the location as the open was handed it, the cache path it
stated, the invalid-values mode), `MetadataLevel` the table beside it. A front
end hands a `Spelling` on `PgDumpOptions::spelling` and to
`register_table_factory`, and every refusal of what those build is
`Error::Spelled`, its sentence closed by the spelling in parentheses, before it
crosses DataFusion's boundary. `pgdt sql` hands one grammar per registration:
`--dump`'s suffixes, or a statement's `pgdump.` options
(`crates/datafusion-cli-pgdump/src/spelling.rs`). D107 and D90 are rewritten.

## The shape

- **`Error::Spelled { refused, spelling }`**, made by one crate-private
  `Speller` the dump holds (and the replay, for a sub-stream's refusal): the
  open, `PgDump::table`, the catalog's naming refusals and each
  `RefusedTable`, a pushed filter's, a plan's, the statistics' and a stream's
  refusal all pass through it, and a spelled refusal is never spelled again.
  `Error::unspelled` reaches the refusal beneath. The top of what DataFusion
  carries is still `datafusion_pgdump::Error` (D74).
- **`Error`'s `Display` and `source` are hand-written**, thiserror dropped
  for it: a derived `#[source]` on a `Box<E>` hands back the box, whose own
  `source()` is `E`'s, so a walk skipped the boxed refusal. That was already
  so for `MetadataLevel`, whose library refusal no source walk reached; it is
  now pinned (`a_table_asked_for_at_the_metadata_level_is_refused_as_such`).
- **`CatalogName(String)` is three variants**, `CatalogNameNeeded`,
  `CatalogNameRefused` and `DatabaseUnnamed`, since what a front end spells
  must be read off the variant (D107): the shell spells the first two as
  `NAME=` given or left out; the third has no remedy.
- **The shell's spelling is exhaustive** over the provider's variants and the
  library's, as `pgdt`'s `refusal::spelling` is, pinned by
  `each_refusal_is_spelled_in_its_registration_s_grammar`; end to end, a `-f`
  statement's refusal printed by upstream's `exec` carries it
  (`a_refusal_is_spelled_in_the_grammar_that_registered_its_dump`).

## Negative results

- **The shell spells only the scan replacing an unusable cache**: it has no
  cache-path option, so `pgdt parse --overwrite-unusable-cache` alone, and a
  file that is not a pgdt cache is left unspelled.
- **A refusal naming no source is spelled without one**: `StrictIdentityUnmet`
  and `CachedBlockChanged` carry the cache, not the dump, so the shell says
  `pgdt parse` bare, as `pgdt` does.
- **Not counted.** No per-row or per-field code moved; a stream clones the
  speller once per partition run and consults it only on a refusal.

## What the next round inherits

- **The spelled refusal's shape is reshaped by `M229`**, ahead of 36.12:
  `Error` a struct holding a `Refusal` and its spelling, in place of the
  `Spelled` variant
  ([2026-10-19](../status/history/2026-10-19.md), "A spelled refusal is the
  error's field, not a variant").
