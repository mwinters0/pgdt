# P17.1 — `CacheMode::load` stops collapsing its four unusable statuses

What 17.2 inherits.

## The type the reason arrives in

`CacheMode::load` answers `Result<CacheLoad>` (`cache.rs`), where `CacheLoad`
is `Index(DumpIndex)` plus `Disabled`, `Missing`, `Unreadable`,
`UnsupportedVersion` and `SourceChanged { cached_stored_size, live_stored_size
}`. The four unusable arms are `CacheStatus`'s own, carried across unchanged;
`Index` is `Valid` and `Incomplete` alike, which is the distinction this method
exists to erase and the reason `CacheStatus` itself is not what comes back.

**`Disabled` is the arm that is not a `CacheStatus`.** It says the *caller*
opted out and no path was consulted, so 17.2's refusal must not fire on it —
`--dqcache none` never had a cache to protect. It is the reason a caller
answers five outcomes rather than four, and the reason a plain
`Result<CacheStatus>` would not have done.

**There is no accessor collapsing the reasons back to an `Option`**, and that is
deliberate: one would rebuild the collapse under a shorter name at exactly the
three sites 17.2's refusal has to reach. Tests that only want the index say so
in a `let CacheLoad::Index(x) = … else { panic!(…) }`.

## Where 17.2 puts the refusal

Three scan entry points call `load`, and each now spells all five arms out:

- `index::preamble_only` (`index.rs`)
- `stream::map_file` (`stream.rs`)
- `stream::table_stream` (`stream.rs`)

Each answers every non-`Index` arm with `DumpIndex::default()` today, which is
byte for byte what `unwrap_or_default()` did. 17.2 changes one arm —
`SourceChanged` — at each of the three, and the exhaustive match is what makes
that a compiler-checked edit rather than a search. The `SourceChanged` payload
travels for D7's message: both stored sizes are in hand at the site that
refuses, so the CLI does not have to re-`stat` anything to say what it found and
what it expected.

Note what is *not* in hand there: the cache **path**. `CacheMode::Enabled(path)`
holds it and the entry points take `&CacheMode`, so the refusal can name the
path — but `CacheLoad` deliberately does not carry it, since `Disabled` has
none.

## What this slice did not change

Nothing behaves differently, which was the slice's whole review question. The
CLI is untouched — `discard_unusable_cache` still runs at `parse`'s startup on
a `Recognized::Mismatch`, and D8 removes it in 17.2. `cache::load`,
`load_offline` and `CacheStatus` are unchanged; `pgdq info` still matches on
`CacheStatus` directly and prints its four sentences.

`architecture.md`, "The cache" was rewritten where it stated the collapse as a
property of the design — that claim is falsified by this slice, so it is
corrected here rather than in 17.2. The sentence 17.2 owns is the other one:
"Size mismatch invalidates (`SourceChanged` — an unusable outcome, not a new
hard-error path)", which is still true today and is left standing.

## Evidence

`cache_mode_load_names_each_unusable_status` (`tests/cache.rs`) puts all four
unusable conditions to `CacheMode::load` and asserts the variant each produces,
including both sizes on `SourceChanged`; the `Disabled` arm is asserted by
`disabled_cache_ignores_an_existing_file_and_persists_nothing`, which used to
assert `is_none()` and now names the reason. Everything else is the existing
suite, which is what a no-behaviour-change slice has for evidence.
