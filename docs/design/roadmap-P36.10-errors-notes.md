# P36.10 notes — Errors in library terms

`pgdt_core::Error` names no front end's flag or command: each sentence ends
with its remedy in the library's terms — the strings schema mode, binding no
identity signal, a mapping pass, naming a table's database, having a scan
replace a cache. `pgdt` spells them in `refusal.rs` (was `source.rs`), the
binary's whole reporting layer; `pgdt info` says a cache's refusal in the
library's sentence; the adapter's `Error::MetadataLevel` carries the library's
refusal and adds the parse; `KD92` is struck. D107 is the decision; D66's
wording follows it.

## The shape

- **One spelling, by variant.** `refusal::spelling` is an exhaustive match
  over `pgdt_core::Error`, as `about_the_source` is, answering the flag or
  command for each remedy in the library's order, which `spelled` appends in
  parentheses. Twelve variants spell; the rest name no remedy pgdt has a
  flag for. Pinned by `each_refusal_names_the_flag_its_remedy_is`, which also
  asserts the library's own sentence holds no `--`.
- **Two ways out of a command, both spelled.** A refusal about the source is
  stringified by `refusal()` with the source's name before and the spelling
  after; every other one keeps its type and goes up by `?`, and `main` folds
  `spelled_chain` over what `commands` returns: each `pgdt_core::Error` in the
  chain is said spelled, the context around it kept, and an error with nothing
  to spell is returned untouched, type and backtrace included. No call site
  has to remember to spell.
- **`info` and the scans say one sentence.** `unusable_cache_message` takes the
  library's `CacheUnusable` and adds only the `pgdt parse` its ways out
  enable; the cache-only mode says the library's sentence, ways out included,
  where it stated the fault alone. What `info` loses: "where this build reads
  <N>" on another build's cache (pgdt-core cannot see `CACHE_FORMAT_VERSION`,
  which is pgdt-model's) and its own "has changed since it was parsed" — the
  manual's table now quotes the library's "was written for a source of".
- **The adapter's metadata-level refusal is the library's.** `PgDumpTable::build`
  no longer pre-checks the census: `Table::schema` refuses, and `build` wraps
  that refusal with the parse that puts the table at the data level. Its
  sentence names the table as the library does, `public.meta`, without the
  database the hand-written one added; a registration's finding still leads
  with the catalog path. `parse_command` adds `--postgres-invalid-values
  ignore` to a dump opened under `Ignore`, and `parse_at_data_level` builds
  on it, so both of the provider's parses carry the mode.

## Negative results

- **`pgdt sql` adds no spelling.** Its errors are DataFusion's CLI printing
  the adapter's; `datafusion-cli-pgdump` is plumbing (spec, "Scope"), so a
  shell user reads "with `time` bound" where it read `--strict-identity=time`.
- **Out of scope, still naming a flag**: pgdt-model's `--block-size` in an xz
  diagnostic is `xz`'s flag.
- **Not counted.** No hot-path code moved.

## What the next round inherits

- **36.11** keeps `Error::CacheNotComplete`'s and `MetadataLevel`'s parses as
  the provider's spelling; `PgDump::table`'s ambiguity sentence ("name the
  schema or the database") is still its own copy of `AmbiguousTable`, the
  selector matching it re-implements going with `PgDump` over a `Dump`.
